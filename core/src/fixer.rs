//! "Fix library": tidy the music folder the way you'd do it by hand in Picard, in one go.
//!
//! * Duplicates: the same recording filed twice (often under two spellings of the artist, e.g.
//!   宇多田ヒカル / Hikaru Utada / Utada) is found by its audio fingerprint (chromaprint's `fpcalc`,
//!   the same thing Picard's "Scan" uses) plus its length, so different names don't hide it.
//!   The best copy stays, playlists are pointed at it, and the others are *moved* to
//!   ~/music-dupes-review/<date>/ (never deleted).
//! * Split albums: when one album was filed under two album artists, the smaller half takes the
//!   bigger half's names so Navidrome shows one album.
//! * Gaps: missing covers (from another song of the same album, or Deezer), missing album artist,
//!   track number (from the file name) and year are filled in. Existing names are never changed.
//!
//! Everything it does is written to a manifest so `undo` can put it back. New downloads go
//! through `on_download`, so they don't need fixing later.
use crate::deezer::Deezer;
use crate::library::{artist_overlap, norm};
use crate::subsonic::Client;
use anyhow::{Context, Result};
use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::{ItemKey, Tag};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

const AUDIO: [&str; 9] = ["mp3", "flac", "m4a", "opus", "ogg", "wav", "aac", "wma", "aiff"];
/// Two files are the same recording if their lengths are this close (s)...
const SAME_LEN: f32 = 2.0;
/// ...and this share of their fingerprint bits agree (different songs sit around 0.5–0.65).
const SAME_AUDIO: f32 = 0.82;

// ------------------------------------------------------------------ index

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct FileInfo {
    pub size: u64,
    pub mtime: u64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub track: Option<u32>,
    pub disc: Option<u32>,
    pub year: Option<u32>,
    pub duration: f32,
    pub bitrate: u32,
    pub cover: bool,
    /// chromaprint, first 120 s (filled in when needed)
    pub fp: Option<Vec<u32>>,
}

impl FileInfo {
    fn ext(rel: &str) -> String {
        rel.rsplit('.').next().unwrap_or_default().to_lowercase()
    }
    fn display_album_artist(&self) -> &str {
        if self.album_artist.is_empty() { &self.artist } else { &self.album_artist }
    }
}

/// Every audio file under the library folder, by path relative to it. Cached on disk and only
/// re-read for files whose size or modification time changed.
pub struct Index {
    pub root: PathBuf,
    pub files: HashMap<String, FileInfo>,
}

/// One cache file per library folder.
fn index_path(root: &Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    root.hash(&mut h);
    crate::cache_dir().join(format!("library-index-{:08x}.json", h.finish() as u32))
}

fn lock() -> &'static Mutex<()> {
    static L: Mutex<()> = Mutex::new(());
    &L
}

fn stat(p: &Path) -> Option<(u64, u64)> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.len(), m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs()))
}

fn read_info(p: &Path, size: u64, mtime: u64) -> FileInfo {
    let mut fi = FileInfo { size, mtime, ..Default::default() };
    let Ok(tf) = lofty::read_from_path(p) else { return fi };
    let props = tf.properties();
    fi.duration = props.duration().as_secs_f32();
    fi.bitrate = props.audio_bitrate().unwrap_or(0);
    if let Some(tag) = tf.primary_tag().or_else(|| tf.first_tag()) {
        fi.title = tag.title().map(|s| s.to_string()).unwrap_or_default();
        fi.artist = tag.artist().map(|s| s.to_string()).unwrap_or_default();
        fi.album = tag.album().map(|s| s.to_string()).unwrap_or_default();
        fi.album_artist = tag.get_string(&ItemKey::AlbumArtist).unwrap_or_default().to_string();
        // a blank track number reads back as 0
        fi.track = tag.track().filter(|n| *n > 0);
        fi.disc = tag.disk().filter(|n| *n > 0);
        fi.year = [ItemKey::Year, ItemKey::RecordingDate, ItemKey::OriginalReleaseDate]
            .iter()
            .filter_map(|k| tag.get_string(k))
            .find_map(|s| s.get(..4).and_then(|y| y.parse().ok()));
        fi.cover = !tag.pictures().is_empty();
    }
    fi
}

impl Index {
    pub fn load(root: &Path) -> Index {
        let files = std::fs::read(index_path(root)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Index { root: root.to_path_buf(), files }
    }

    pub fn save(&self) {
        if let Ok(b) = serde_json::to_vec(&self.files) {
            let p = index_path(&self.root);
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            let tmp = p.with_extension("json.tmp");
            if std::fs::write(&tmp, b).is_ok() {
                let _ = std::fs::rename(&tmp, p);
            }
        }
    }

    /// Bring the index up to date with the folder (`progress(done, total)`).
    pub fn refresh(&mut self, mut progress: impl FnMut(usize, usize)) {
        let mut paths = vec![];
        walk(&self.root, &mut paths);
        let total = paths.len();
        let mut fresh = HashMap::with_capacity(total);
        for (i, p) in paths.iter().enumerate() {
            let Ok(rel) = p.strip_prefix(&self.root) else { continue };
            let rel = rel.to_string_lossy().to_string();
            let Some((size, mtime)) = stat(p) else { continue };
            let info = match self.files.remove(&rel) {
                Some(f) if f.size == size && f.mtime == mtime => f,
                _ => read_info(p, size, mtime),
            };
            fresh.insert(rel, info);
            if i % 25 == 0 {
                progress(i, total);
            }
        }
        self.files = fresh;
    }

    fn abs(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    /// Fingerprint the given files that don't have one yet (4 at a time, low priority).
    pub fn fingerprint(&mut self, rels: &[String], mut progress: impl FnMut(usize, usize)) {
        let todo: Vec<String> = rels.iter().filter(|r| self.files.get(*r).map(|f| f.fp.is_none()).unwrap_or(false)).cloned().collect();
        let total = todo.len();
        let done = Arc::new(Mutex::new(Vec::<(String, Option<Vec<u32>>)>::new()));
        for chunk in todo.chunks(4) {
            let hs: Vec<_> = chunk
                .iter()
                .map(|rel| {
                    let (p, rel, done) = (self.abs(rel), rel.clone(), done.clone());
                    std::thread::spawn(move || {
                        let fp = fpcalc(&p);
                        done.lock().unwrap().push((rel, fp));
                    })
                })
                .collect();
            for h in hs {
                let _ = h.join();
            }
            let got: Vec<_> = std::mem::take(&mut *done.lock().unwrap());
            for (rel, fp) in got {
                if let Some(f) = self.files.get_mut(&rel) {
                    // an empty print marks "tried, couldn't" so it isn't retried every time
                    f.fp = Some(fp.unwrap_or_default());
                }
            }
            progress(total - todo.len().min(total) + chunk.len(), total);
        }
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().and_then(|x| x.to_str()).map(|x| AUDIO.contains(&x.to_lowercase().as_str())).unwrap_or(false) {
            out.push(p);
        }
    }
}

fn fpcalc(p: &Path) -> Option<Vec<u32>> {
    let out = Command::new("nice").args(["-n", "19", "fpcalc", "-raw", "-length", "120"]).arg(p).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let line = s.lines().find(|l| l.starts_with("FINGERPRINT="))?;
    let v: Vec<u32> = line[12..].split(',').filter_map(|x| x.parse::<i64>().ok()).map(|x| x as u32).collect();
    (!v.is_empty()).then_some(v)
}

/// Share of fingerprint bits that agree at the best small alignment (0..1).
pub fn similarity(a: &[u32], b: &[u32]) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.;
    }
    let mut best = 0f32;
    for off in -8i32..=8 {
        let (mut same, mut n) = (0u32, 0u32);
        for i in 0..a.len() {
            let j = i as i32 + off;
            if j < 0 || j as usize >= b.len() {
                continue;
            }
            same += 32 - (a[i] ^ b[j as usize]).count_ones();
            n += 32;
        }
        if n >= 32 * 40 {
            best = best.max(same as f32 / n as f32);
        }
    }
    best
}

// ------------------------------------------------------------------ plan

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct TagEdit {
    pub rel: String,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub track: Option<u32>,
    pub year: Option<u32>,
    /// cover image to embed (bytes are fetched when applying)
    pub cover_from: Option<CoverSrc>,
    /// shown in the summary
    pub why: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum CoverSrc {
    File(String),
    Url(String),
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct DupGroup {
    pub keep: String,
    pub remove: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Plan {
    pub dupes: Vec<DupGroup>,
    pub edits: Vec<TagEdit>,
    /// files it couldn't do anything sensible with (e.g. no tags and no match)
    pub unsure: Vec<String>,
    /// same audio but different song and artist: likely a wrong download (left alone)
    pub mismatched: Vec<(String, String)>,
    /// (playlist, song) listed more than once in the same playlist
    pub repeats: Vec<(String, String)>,
    pub files: usize,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.dupes.is_empty() && self.edits.is_empty() && self.repeats.is_empty()
    }
    pub fn removed(&self) -> usize {
        self.dupes.iter().map(|g| g.remove.len()).sum()
    }
    pub fn count(&self, what: &str) -> usize {
        self.edits.iter().filter(|e| e.why.contains(what)).count()
    }
}

pub struct Progress<'a>(pub &'a dyn Fn(&str, usize, usize));

/// Look at the whole library and work out what to fix (changes nothing).
/// `client`: also look for songs listed twice in the same playlist (None in tests).
pub fn plan(root: &Path, dz: &Deezer, client: Option<&Client>, progress: Progress) -> Result<(Plan, Index)> {
    let _g = lock().lock().unwrap();
    let mut idx = Index::load(root);
    idx.refresh(|d, t| (progress.0)("Reading your music files", d, t));
    idx.save();

    // ---- duplicates: same length, then same fingerprint
    let mut by_len: Vec<(&String, f32)> = idx.files.iter().filter(|(_, f)| f.duration > 5.).map(|(k, f)| (k, f.duration)).collect();
    by_len.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut pairs = vec![];
    for i in 0..by_len.len() {
        for j in i + 1..by_len.len() {
            if by_len[j].1 - by_len[i].1 > SAME_LEN {
                break;
            }
            pairs.push((by_len[i].0.clone(), by_len[j].0.clone()));
        }
    }
    let need: Vec<String> = pairs.iter().flat_map(|(a, b)| [a.clone(), b.clone()]).collect::<HashSet<_>>().into_iter().collect();
    idx.fingerprint(&need, |d, t| (progress.0)("Comparing how songs sound", d, t));
    idx.save();
    let mut parent: HashMap<String, String> = HashMap::new();
    let mut mismatched: Vec<(String, String)> = vec![];
    fn find(p: &mut HashMap<String, String>, x: &str) -> String {
        let up = p.get(x).cloned().unwrap_or_else(|| x.to_string());
        if up == x {
            return up;
        }
        let r = find(p, &up);
        p.insert(x.to_string(), r.clone());
        r
    }
    for (a, b) in &pairs {
        let (fa, fb) = (&idx.files[a], &idx.files[b]);
        if similarity(fa.fp.as_deref().unwrap_or(&[]), fb.fp.as_deref().unwrap_or(&[])) >= SAME_AUDIO {
            if !related(fa, fb) {
                // same sound, nothing else in common: one file is probably the wrong song
                mismatched.push((a.clone(), b.clone()));
                continue;
            }
            // every matched file gets an entry (pointing at itself until joined), so groups
            // below include the roots too
            parent.entry(a.clone()).or_insert_with(|| a.clone());
            parent.entry(b.clone()).or_insert_with(|| b.clone());
            let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
            if ra != rb {
                parent.insert(ra, rb);
            }
        }
    }
    let mut groups: HashMap<String, Vec<String>> = HashMap::new();
    for k in parent.keys().cloned().collect::<Vec<_>>() {
        let r = find(&mut parent, &k);
        groups.entry(r).or_default().push(k);
    }
    // how many songs each album folder holds (keep copies inside fuller albums)
    let mut dir_count: HashMap<String, usize> = HashMap::new();
    for k in idx.files.keys() {
        *dir_count.entry(parent_dir(k)).or_default() += 1;
    }
    let mut dupes = vec![];
    for (_, mut g) in groups {
        g.sort();
        g.dedup();
        if g.len() < 2 {
            continue;
        }
        g.sort_by(|a, b| keep_score(&idx, b, &dir_count).partial_cmp(&keep_score(&idx, a, &dir_count)).unwrap().then(a.cmp(b)));
        dupes.push(DupGroup { keep: g[0].clone(), remove: g[1..].to_vec() });
    }
    dupes.sort_by(|a, b| a.keep.cmp(&b.keep));
    let removed: HashSet<&String> = dupes.iter().flat_map(|g| g.remove.iter()).collect();

    // ---- split albums: names of the kept copy win for the rest of its album
    let mut edits: HashMap<String, TagEdit> = HashMap::new();
    fn edit<'a>(m: &'a mut HashMap<String, TagEdit>, rel: &str) -> &'a mut TagEdit {
        m.entry(rel.to_string()).or_insert_with(|| TagEdit { rel: rel.to_string(), ..Default::default() })
    }
    for g in &dupes {
        let k = &idx.files[&g.keep];
        for r in &g.remove {
            let f = &idx.files[r];
            let same_album = norm(&f.album) == norm(&k.album) && !k.album.is_empty();
            let other_side = norm(f.display_album_artist()) != norm(k.display_album_artist()) || f.album != k.album;
            if !same_album || !other_side {
                continue;
            }
            // every remaining song of the removed copy's album joins the kept album
            for (rel, o) in &idx.files {
                if removed.contains(rel) || parent_dir(rel) != parent_dir(r) || norm(&o.album) != norm(&f.album) {
                    continue;
                }
                if o.display_album_artist() != k.display_album_artist() || o.album != k.album {
                    let e = edit(&mut edits, rel);
                    e.album_artist = Some(k.display_album_artist().to_string());
                    e.album = Some(k.album.clone());
                    e.why = add_why(&e.why, "merged split album");
                }
            }
        }
    }

    // ---- gaps
    let total = idx.files.len();
    let mut unsure = vec![];
    let mut album_cover: HashMap<String, Option<String>> = HashMap::new(); // dir -> a file with a cover
    for (rel, f) in &idx.files {
        if f.cover {
            album_cover.entry(parent_dir(rel)).or_insert(Some(rel.clone()));
        }
    }
    let mut dz_cover: HashMap<(String, String), Option<(String, Option<u32>)>> = HashMap::new();
    let mut rels: Vec<&String> = idx.files.keys().filter(|r| !removed.contains(*r)).collect();
    rels.sort();
    for (i, rel) in rels.iter().enumerate() {
        if i % 25 == 0 {
            (progress.0)("Looking for missing details", i, total);
        }
        let f = &idx.files[*rel];
        if f.title.is_empty() && f.artist.is_empty() {
            unsure.push((*rel).clone());
            continue;
        }
        if f.album_artist.is_empty() && !f.artist.is_empty() && edits.get(*rel).and_then(|e| e.album_artist.clone()).is_none() {
            let e = edit(&mut edits, rel);
            e.album_artist = Some(f.artist.clone());
            e.why = add_why(&e.why, "album artist");
        }
        if f.track.is_none() {
            if let Some(n) = track_from_name(rel) {
                let e = edit(&mut edits, rel);
                e.track = Some(n);
                e.why = add_why(&e.why, "track number");
            }
        }
        if !f.cover {
            match album_cover.get(&parent_dir(rel)).cloned().flatten() {
                Some(src) => {
                    let e = edit(&mut edits, rel);
                    e.cover_from = Some(CoverSrc::File(src));
                    e.why = add_why(&e.why, "cover");
                }
                None if !f.album.is_empty() => {
                    let key = (norm(f.display_album_artist()), norm(&f.album));
                    let found = dz_cover.entry(key).or_insert_with(|| deezer_album(dz, f.display_album_artist(), &f.album)).clone();
                    match found {
                        Some((url, year)) => {
                            let e = edit(&mut edits, rel);
                            e.cover_from = Some(CoverSrc::Url(url));
                            e.why = add_why(&e.why, "cover");
                            if f.year.is_none() && year.is_some() {
                                e.year = year;
                                e.why = add_why(&e.why, "year");
                            }
                        }
                        None => unsure.push((*rel).clone()),
                    }
                }
                None => unsure.push((*rel).clone()),
            }
        }
    }
    let mut edits: Vec<TagEdit> = edits.into_values().collect();
    edits.sort_by(|a, b| a.rel.cmp(&b.rel));
    unsure.sort();
    unsure.dedup();
    let mut repeats = vec![];
    if let Some(c) = client {
        (progress.0)("Checking playlists", 0, 0);
        for p in c.playlists()? {
            let mut seen = HashSet::new();
            for t in c.playlist(&p.id)?.tracks {
                if !seen.insert(t.id.clone()) {
                    repeats.push((p.name.clone(), t.title.clone()));
                }
            }
        }
    }
    Ok((Plan { dupes, edits, unsure, mismatched, repeats, files: total }, idx))
}

/// Whether two files with the same audio are plausibly the same song (shared artist or title
/// words, in any spelling), rather than one being a mislabelled download.
fn related(a: &FileInfo, b: &FileInfo) -> bool {
    let words = |s: &str| -> HashSet<String> {
        s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 3).map(str::to_string).collect()
    };
    let nonempty = |x: &str, y: &str| !x.is_empty() && !y.is_empty();
    (nonempty(&a.artist, &b.artist) && artist_overlap(&a.artist, &b.artist))
        || (nonempty(a.display_album_artist(), b.display_album_artist()) && artist_overlap(a.display_album_artist(), b.display_album_artist()))
        || (!norm(&a.title).is_empty() && norm(&a.title) == norm(&b.title))
        || !words(&a.title).is_disjoint(&words(&b.title))
        || (!norm(&a.album).is_empty() && norm(&a.album) == norm(&b.album))
}

fn add_why(w: &str, what: &str) -> String {
    if w.is_empty() { what.to_string() } else if w.contains(what) { w.to_string() } else { format!("{w}, {what}") }
}

fn parent_dir(rel: &str) -> String {
    Path::new(rel).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()
}

/// "03. Title.mp3" / "2-03. Title.mp3" / "03 - Title.mp3" -> 3
fn track_from_name(rel: &str) -> Option<u32> {
    let name = Path::new(rel).file_name()?.to_str()?;
    let head: String = name.chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect();
    let num = head.rsplit('-').find(|s| !s.is_empty())?;
    let rest = &name[head.len()..];
    (rest.starts_with(". ") || rest.starts_with(" - ") || rest.starts_with(' ')).then(|| num.parse().ok()).flatten().filter(|n| *n > 0 && *n < 500)
}

/// Which copy of a duplicate to keep: best sound, then tags/cover, then the fuller album.
fn keep_score(idx: &Index, rel: &str, dir_count: &HashMap<String, usize>) -> f64 {
    let f = &idx.files[rel];
    let ext = FileInfo::ext(rel);
    let lossless = matches!(ext.as_str(), "flac" | "wav" | "aiff") || f.bitrate > 700;
    let quality = if lossless { 320. } else { f.bitrate.min(320) as f64 };
    let tags = f.cover as u8 as f64 + f.track.is_some() as u8 as f64 + f.year.is_some() as u8 as f64 + (!f.album_artist.is_empty()) as u8 as f64;
    quality * 1000. + tags * 100. + (*dir_count.get(&parent_dir(rel)).unwrap_or(&1)).min(99) as f64
}

/// Cover (and year) of an album on Deezer, if one clearly matches.
fn deezer_album(dz: &Deezer, artist: &str, album: &str) -> Option<(String, Option<u32>)> {
    let found = dz.search_albums(&format!("{artist} {album}")).ok()?;
    let a = found.into_iter().find(|a| norm(&a.title) == norm(album) && artist_overlap(&a.artist, artist))?;
    let url = a.cover.clone()?.replace("250x250", "1000x1000").replace("500x500", "1000x1000");
    Some((url, a.year.parse().ok()))
}

// ------------------------------------------------------------------ apply / undo

#[derive(Serialize, Deserialize, Default)]
pub struct Manifest {
    pub when: u64,
    pub trash: PathBuf,
    /// (from rel, to absolute) for moved duplicates
    pub moved: Vec<(String, PathBuf)>,
    /// tags as they were before: rel -> previous values
    pub tags: Vec<(String, OldTags)>,
    /// playlists as they were (id, song ids)
    pub playlists: Vec<(String, Vec<String>)>,
    /// .m3u8 files as they were (path, content)
    pub m3u: Vec<(PathBuf, String)>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct OldTags {
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub track: Option<u32>,
    pub year: Option<String>,
    pub had_cover: bool,
}

fn history_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_default().join("crate/fix-history")
}

pub struct Report {
    pub removed: usize,
    pub edited: usize,
    pub playlists: usize,
    pub failed: Vec<String>,
}

/// Navidrome's id for each library file. Navidrome reports file sizes, which almost never
/// collide, so size + type finds the song even though its paths aren't real ones.
fn navidrome_ids(client: &Client, idx: &Index) -> Result<HashMap<String, String>> {
    let mut by_key: HashMap<(u64, String), Vec<(String, f32, String)>> = HashMap::new();
    let mut off = 0;
    loop {
        let o = off.to_string();
        let b = client.call("search3", &[("query", ""), ("songCount", "500"), ("songOffset", &o), ("albumCount", "0"), ("artistCount", "0")])?;
        let songs = b["searchResult3"]["song"].as_array().cloned().unwrap_or_default();
        for s in &songs {
            let key = (s["size"].as_u64().unwrap_or(0), s["suffix"].as_str().unwrap_or_default().to_lowercase());
            by_key.entry(key).or_default().push((
                s["id"].as_str().unwrap_or_default().to_string(),
                s["duration"].as_f64().unwrap_or(0.) as f32,
                norm(s["title"].as_str().unwrap_or_default()),
            ));
        }
        off += songs.len();
        if songs.len() < 500 {
            break;
        }
    }
    // also by title + type, for files whose size changed since Navidrome last scanned them
    // (e.g. retagged in Picard a moment ago)
    let mut by_title: HashMap<(String, String), Vec<(String, f32)>> = HashMap::new();
    for ((_, ext), v) in &by_key {
        for (id, dur, title) in v {
            by_title.entry((title.clone(), ext.clone())).or_default().push((id.clone(), *dur));
        }
    }
    let mut out = HashMap::new();
    for (rel, f) in &idx.files {
        // the size on disk right now, not when the library was checked
        let size = stat(&idx.abs(rel)).map(|s| s.0).unwrap_or(f.size);
        let by_size = by_key.get(&(size, FileInfo::ext(rel))).and_then(|c| {
            if c.len() == 1 { Some(c[0].0.clone()) } else { c.iter().find(|x| x.2 == norm(&f.title) && (x.1 - f.duration).abs() < 2.).map(|x| x.0.clone()) }
        });
        let pick = by_size.or_else(|| {
            let c = by_title.get(&(norm(&f.title), FileInfo::ext(rel)))?;
            let near: Vec<&(String, f32)> = c.iter().filter(|x| (x.1 - f.duration).abs() < 1.5).collect();
            // only when it's unambiguous
            (near.len() == 1).then(|| near[0].0.clone())
        });
        if let Some(p) = pick {
            out.insert(rel.clone(), p);
        }
    }
    Ok(out)
}

/// Carry out a plan. Writes a manifest first-thing so a crash halfway can still be undone.
/// `client` None skips Navidrome and .m3u8 playlists (tests on a scratch folder).
pub fn apply(plan: &Plan, idx: &Index, client: Option<&Client>, progress: Progress) -> Result<Report> {
    let _g = lock().lock().unwrap();
    let now = std::time::SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let trash = dirs::home_dir().context("no home")?.join("music-dupes-review").join(format!("crate-fix-{now}"));
    let mut man = Manifest { when: now, trash: trash.clone(), ..Default::default() };
    let man_path = history_dir().join(format!("{now}.json"));
    std::fs::create_dir_all(history_dir())?;
    let save = |m: &Manifest| {
        if let Ok(b) = serde_json::to_vec_pretty(m) {
            let _ = std::fs::write(&man_path, b);
        }
    };
    let mut rep = Report { removed: 0, edited: 0, playlists: 0, failed: vec![] };
    let mut unmapped: HashSet<String> = HashSet::new();

    // 1. playlists: point removed copies at the kept one, and drop repeats
    if let (Some(client), true) = (client, !plan.dupes.is_empty() || !plan.repeats.is_empty()) {
        (progress.0)("Updating playlists", 0, 1);
        let ids = navidrome_ids(client, idx)?;
        let mut swap: HashMap<String, String> = HashMap::new();
        for g in &plan.dupes {
            for r in &g.remove {
                match (ids.get(&g.keep), ids.get(r)) {
                    (Some(keep), Some(old)) => {
                        swap.insert(old.clone(), keep.clone());
                    }
                    // can't tell which Navidrome song this is: moving it could leave a playlist
                    // pointing at nothing, so leave this copy where it is
                    _ => {
                        unmapped.insert(r.clone());
                    }
                }
            }
        }
        for p in client.playlists()? {
            let cur: Vec<String> = client.playlist(&p.id)?.tracks.into_iter().map(|t| t.id).collect();
            let mut seen = HashSet::new();
            let new: Vec<String> = cur.iter().map(|id| swap.get(id).cloned().unwrap_or_else(|| id.clone())).filter(|id| seen.insert(id.clone())).collect();
            if new != cur {
                man.playlists.push((p.id.clone(), cur.clone()));
                save(&man);
                client.playlist_replace(&p.id, &cur, &new)?;
                rep.playlists += 1;
            }
        }
        // sockseek's .m3u8 playlists name files by path
        let pl_dir = idx.root.parent().map(|p| p.join("Playlists"));
        if let Some(Ok(rd)) = pl_dir.as_ref().map(std::fs::read_dir) {
            let lib = idx.root.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let mut rename: HashMap<String, String> = HashMap::new();
            for g in &plan.dupes {
                for r in &g.remove {
                    rename.insert(format!("../{lib}/{r}"), format!("../{lib}/{}", g.keep));
                }
            }
            for e in rd.flatten() {
                let p = e.path();
                if !p.extension().map(|x| x.to_string_lossy().starts_with("m3u")).unwrap_or(false) {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&p) else { continue };
                let mut seen = HashSet::new();
                let new: Vec<String> = text
                    .lines()
                    .map(|l| rename.get(l).cloned().unwrap_or_else(|| l.to_string()))
                    .filter(|l| l.starts_with('#') || l.trim().is_empty() || seen.insert(l.clone()))
                    .collect();
                let new = new.join("\n") + if text.ends_with('\n') { "\n" } else { "" };
                if new != text {
                    man.m3u.push((p.clone(), text));
                    save(&man);
                    std::fs::write(&p, new)?;
                }
            }
        }
    }

    // 2. tags (before moving anything, so a covers-from-file source still exists)
    let total = plan.edits.len();
    for (i, e) in plan.edits.iter().enumerate() {
        (progress.0)("Fixing tags and covers", i, total);
        let path = idx.abs(&e.rel);
        match edit_tags(&path, e, idx) {
            Ok(old) => {
                man.tags.push((e.rel.clone(), old));
                rep.edited += 1;
            }
            Err(err) => rep.failed.push(format!("{}: {err:#}", e.rel)),
        }
        if i % 20 == 0 {
            save(&man);
        }
    }
    save(&man);

    // 3. move duplicates out of the library
    for g in &plan.dupes {
        for r in &g.remove {
            if unmapped.contains(r) {
                rep.failed.push(format!("{r}: left in place (couldn't match it to its Navidrome song; run Check library again in a minute)"));
                continue;
            }
            let from = idx.abs(r);
            let to = trash.join(r);
            let moved = std::fs::create_dir_all(to.parent().unwrap()).and_then(|_| move_file(&from, &to));
            match moved {
                Ok(()) => {
                    man.moved.push((r.clone(), to));
                    rep.removed += 1;
                    remove_empty_dirs(from.parent(), &idx.root);
                }
                Err(err) => rep.failed.push(format!("{r}: {err}")),
            }
            save(&man);
        }
    }
    if let Some(c) = client {
        c.start_scan();
    }
    Ok(rep)
}

fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_err() {
        std::fs::copy(from, to)?;
        std::fs::remove_file(from)?;
    }
    Ok(())
}

/// Remove `dir` and everything under it that is only empty folders.
fn prune_empty(dir: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else { return false };
    let mut empty = true;
    for e in rd.flatten() {
        let p = e.path();
        if !(p.is_dir() && prune_empty(&p)) {
            empty = false;
        }
    }
    empty && std::fs::remove_dir(dir).is_ok()
}

fn remove_empty_dirs(mut d: Option<&Path>, root: &Path) {
    while let Some(dir) = d {
        if dir == root || !dir.starts_with(root) || std::fs::remove_dir(dir).is_err() {
            break;
        }
        d = dir.parent();
    }
}

fn open_tag(path: &Path) -> Result<lofty::file::TaggedFile> {
    let mut file = Probe::open(path)?.read()?;
    if file.primary_tag().is_none() {
        let tt = file.primary_tag_type();
        file.insert_tag(Tag::new(tt));
    }
    Ok(file)
}

fn edit_tags(path: &Path, e: &TagEdit, idx: &Index) -> Result<OldTags> {
    let cover = match &e.cover_from {
        Some(CoverSrc::File(rel)) => lofty::read_from_path(idx.abs(rel))
            .ok()
            .and_then(|f| f.primary_tag().or_else(|| f.first_tag()).and_then(|t| t.pictures().first().map(|p| p.data().to_vec()))),
        Some(CoverSrc::Url(u)) => crate::http_get(u).ok(),
        None => None,
    };
    let mut file = open_tag(path)?;
    let tag = file.primary_tag_mut().context("no tag")?;
    let old = OldTags {
        album_artist: tag.get_string(&ItemKey::AlbumArtist).map(str::to_string),
        album: tag.album().map(|s| s.to_string()),
        track: tag.track(),
        year: tag.get_string(&ItemKey::Year).map(str::to_string),
        had_cover: !tag.pictures().is_empty(),
    };
    if let Some(v) = &e.album_artist {
        tag.insert_text(ItemKey::AlbumArtist, v.clone());
    }
    if let Some(v) = &e.album {
        tag.set_album(v.clone());
    }
    if let Some(n) = e.track {
        tag.set_track(n);
    }
    if let Some(y) = e.year {
        tag.insert_text(ItemKey::Year, y.to_string());
    }
    if let (Some(bytes), false) = (cover, old.had_cover) {
        let mime = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) { MimeType::Png } else { MimeType::Jpeg };
        tag.push_picture(Picture::new_unchecked(PictureType::CoverFront, Some(mime), None, bytes));
    }
    tag.save_to_path(path, WriteOptions::default())?;
    Ok(old)
}

/// Whether there's a fix that can be undone.
pub fn last_fix() -> Option<(PathBuf, u64)> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(history_dir()).ok()?.flatten().map(|e| e.path()).filter(|p| p.extension().map(|x| x == "json").unwrap_or(false)).collect();
    v.sort();
    let p = v.pop()?;
    let when = p.file_stem()?.to_str()?.parse().ok()?;
    Some((p, when))
}

/// Put back everything the last fix did (files, tags, playlists).
pub fn undo(root: &Path, client: Option<&Client>) -> Result<usize> {
    let _g = lock().lock().unwrap();
    let (path, _) = last_fix().context("nothing to undo")?;
    let man: Manifest = serde_json::from_slice(&std::fs::read(&path)?)?;
    let mut n = 0;
    for (rel, from) in &man.moved {
        let to = root.join(rel);
        if let Some(d) = to.parent() {
            std::fs::create_dir_all(d)?;
        }
        if move_file(from, &to).is_ok() {
            n += 1;
        }
    }
    for (rel, old) in &man.tags {
        let p = root.join(rel);
        let Ok(mut file) = open_tag(&p) else { continue };
        let Some(tag) = file.primary_tag_mut() else { continue };
        match &old.album_artist {
            Some(v) => {
                tag.insert_text(ItemKey::AlbumArtist, v.clone());
            }
            None => tag.remove_key(&ItemKey::AlbumArtist),
        }
        match &old.album {
            Some(v) => tag.set_album(v.clone()),
            None => tag.remove_album(),
        }
        match old.track {
            Some(t) => tag.set_track(t),
            None => tag.remove_track(),
        }
        match &old.year {
            Some(v) => {
                tag.insert_text(ItemKey::Year, v.clone());
            }
            None => tag.remove_key(&ItemKey::Year),
        }
        if !old.had_cover {
            tag.remove_picture_type(PictureType::CoverFront);
        }
        if tag.save_to_path(&p, WriteOptions::default()).is_ok() {
            n += 1;
        }
    }
    for (p, text) in &man.m3u {
        let _ = std::fs::write(p, text);
    }
    if let Some(c) = client {
        c.start_scan();
    }
    // Navidrome needs to see the files again before the playlists can point at them
    if let (Some(client), false) = (client, man.playlists.is_empty()) {
        std::thread::sleep(std::time::Duration::from_secs(15));
        for (id, songs) in &man.playlists {
            let cur: Vec<String> = client.playlist(id).map(|p| p.tracks.into_iter().map(|t| t.id).collect()).unwrap_or_default();
            let mut q: Vec<(&str, &str)> = vec![("playlistId", id.as_str())];
            for s in songs {
                q.push(("songId", s.as_str()));
            }
            let _ = cur;
            let _ = client.call_post("createPlaylist", &q);
        }
    }
    let _ = std::fs::rename(&path, path.with_extension("undone"));
    prune_empty(&man.trash);
    Ok(n)
}

// ------------------------------------------------------------------ new downloads

static AUTO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Whether new downloads are checked for duplicates and album spellings (Settings).
pub fn set_auto(on: bool) {
    AUTO.store(on, std::sync::atomic::Ordering::Relaxed);
}
pub fn auto() -> bool {
    AUTO.load(std::sync::atomic::Ordering::Relaxed)
}

/// What to do with a freshly downloaded, tagged file before it's filed.
pub enum Arrival {
    /// file it, using these album artist / album names (an existing album's spelling, so the
    /// album isn't split across two names)
    File { album_artist: String, album: String },
    /// the library already has this recording at this path: drop the download, use that
    Duplicate(String),
}

/// Check a new download against the library: is it already there under another name, and does
/// its album already exist with a different spelling of the artist?
pub fn on_download(root: &Path, file: &Path, album_artist: &str, album: &str) -> Arrival {
    let duration = lofty::read_from_path(file).map(|f| f.properties().duration().as_secs_f32()).unwrap_or(0.);
    let _g = lock().lock().unwrap();
    let mut idx = Index::load(root);
    idx.refresh(|_, _| {});
    // same recording already in the library?
    let near: Vec<String> = idx.files.iter().filter(|(_, f)| (f.duration - duration).abs() <= SAME_LEN).map(|(k, _)| k.clone()).collect();
    if !near.is_empty() && duration > 5. {
        idx.fingerprint(&near, |_, _| {});
        if let Some(fp) = fpcalc(file) {
            let best = near.iter().filter_map(|k| Some((k, similarity(&fp, idx.files.get(k)?.fp.as_deref()?)))).max_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((k, s)) = best {
                if s >= SAME_AUDIO {
                    let k = k.clone();
                    idx.save();
                    return Arrival::Duplicate(k);
                }
            }
        }
    }
    idx.save();
    // an album of the same name whose artist is the same, spelled differently?
    let (na, nal) = (norm(album_artist), norm(album));
    let existing = idx
        .files
        .values()
        .filter(|f| !nal.is_empty() && norm(&f.album) == nal)
        .find(|f| {
            let fa = norm(f.display_album_artist());
            fa == na || artist_overlap(f.display_album_artist(), album_artist) || (!fa.is_empty() && !na.is_empty() && (fa.contains(&na) || na.contains(&fa)))
        });
    match existing {
        Some(f) => Arrival::File { album_artist: f.display_album_artist().to_string(), album: f.album.clone() },
        None => Arrival::File { album_artist: album_artist.to_string(), album: album.to_string() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn track_numbers_from_names() {
        assert_eq!(track_from_name("A/B/03. Song.mp3"), Some(3));
        assert_eq!(track_from_name("A/B/2-08. Song.mp3"), Some(8));
        assert_eq!(track_from_name("A/B/12 - Song.mp3"), Some(12));
        assert_eq!(track_from_name("A/B/1999.mp3"), None);
        assert_eq!(track_from_name("A/B/Song.mp3"), None);
    }
    #[test]
    fn identical_prints_match() {
        let a: Vec<u32> = (0..200u32).map(|i| i.wrapping_mul(2654435761)).collect();
        assert!(similarity(&a, &a) > 0.99);
        let b: Vec<u32> = (0..200u32).map(|i| i.wrapping_mul(40503).rotate_left(7)).collect();
        assert!(similarity(&a, &b) < 0.7);
    }
}
