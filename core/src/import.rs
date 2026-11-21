//! Bring Spotify playlists over from Exportify (exportify.net) CSV files: songs you already have
//! go straight into a Navidrome playlist of the same name, and the rest are looked up in the
//! catalog and queued for download into that playlist.
use crate::deezer::{track_from, Deezer, DzTrack};
use crate::library::{artist_overlap, norm};
use crate::model::Track;
use crate::subsonic::Client;
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Debug, Default)]
pub struct CsvSong {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f64,
    pub isrc: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct CsvPlaylist {
    pub name: String,
    pub songs: Vec<CsvSong>,
}

/// Read one Exportify CSV. The playlist is named after the file (Exportify names files after
/// the playlist, with `_` for spaces).
pub fn read_csv(path: &Path) -> Result<CsvPlaylist> {
    let mut r = csv::ReaderBuilder::new().flexible(true).from_path(path).with_context(|| format!("couldn't open {}", path.display()))?;
    let head: Vec<String> = r.headers()?.iter().map(|h| h.trim().to_lowercase()).collect();
    let col = |names: &[&str]| head.iter().position(|h| names.contains(&h.as_str()));
    let (Some(ti), Some(ai)) = (col(&["track name"]), col(&["artist name(s)", "artist name"])) else {
        bail!("{} isn't an Exportify playlist file (no \u{201c}Track Name\u{201d} / \u{201c}Artist Name(s)\u{201d} columns)", file_name(path));
    };
    let (al, du, is) = (col(&["album name"]), col(&["duration (ms)", "track duration (ms)"]), col(&["isrc"]));
    let mut songs = vec![];
    for rec in r.records() {
        let rec = rec?;
        let get = |i: Option<usize>| i.and_then(|i| rec.get(i)).unwrap_or_default().trim().to_string();
        let title = get(Some(ti));
        if title.is_empty() {
            continue; // local files and podcast episodes come out blank
        }
        songs.push(CsvSong {
            title,
            artist: get(Some(ai)),
            album: get(al),
            duration: get(du).parse::<f64>().map(|ms| ms / 1000.).unwrap_or(0.),
            isrc: Some(get(is)).filter(|s| !s.is_empty()),
        });
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(CsvPlaylist { name: stem.replace('_', " ").trim().to_string(), songs })
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Exportify lists every artist ("A;B", or "A, B" in older versions); the first is the main one.
fn main_artist(a: &str) -> &str {
    let first = if a.contains(';') { a.split(';').next() } else { a.split(", ").next() };
    first.unwrap_or(a).trim()
}

/// The whole library, indexed by ISRC and by normalised title, for matching many songs quickly.
pub struct LibraryIndex {
    by_title: HashMap<String, Vec<Track>>,
    by_isrc: HashMap<String, Track>,
}

impl LibraryIndex {
    pub fn load(client: &Client) -> Result<Self> {
        let (mut by_title, mut by_isrc) = (HashMap::<String, Vec<Track>>::new(), HashMap::new());
        for v in client.all_songs_raw()? {
            let t = Track::from_json(&v);
            for i in v["isrc"].as_array().into_iter().flatten().filter_map(|i| i.as_str()) {
                by_isrc.insert(i.to_uppercase(), t.clone());
            }
            by_title.entry(norm(&t.title)).or_default().push(t);
        }
        Ok(LibraryIndex { by_title, by_isrc })
    }

    /// The library song that is this Spotify song, if you have it. The ISRC (a recording's ID)
    /// finds it even when the title is in another language or spelled differently.
    pub fn find(&self, s: &CsvSong) -> Option<&Track> {
        if let Some(t) = s.isrc.as_ref().and_then(|i| self.by_isrc.get(&i.to_uppercase())) {
            return Some(t);
        }
        let cands = self.by_title.get(&norm(&s.title))?;
        cands
            .iter()
            .filter(|t| {
                let dd = (t.duration - s.duration).abs();
                (s.duration <= 0. || dd <= 8.) && (artist_overlap(&s.artist, &t.artist) || (s.duration > 0. && dd <= 2.))
            })
            .min_by(|a, b| (a.duration - s.duration).abs().total_cmp(&(b.duration - s.duration).abs()))
    }
}

/// What importing one file will do.
#[derive(Clone, Debug, Default)]
pub struct PlaylistPlan {
    pub name: String,
    /// library song for each CSV row (None = needs downloading)
    pub owned: Vec<Option<String>>,
    pub songs: Vec<CsvSong>,
    /// an existing Navidrome playlist with this name (songs are added to it, not duplicated)
    pub existing: Option<String>,
}

impl PlaylistPlan {
    pub fn have(&self) -> usize {
        self.owned.iter().filter(|o| o.is_some()).count()
    }
    pub fn missing(&self) -> usize {
        self.owned.len() - self.have()
    }
}

pub fn plan(client: &Client, idx: &LibraryIndex, pl: CsvPlaylist) -> Result<PlaylistPlan> {
    let existing = client.playlists()?.into_iter().find(|p| p.name.eq_ignore_ascii_case(&pl.name)).map(|p| p.id);
    let owned = pl.songs.iter().map(|s| idx.find(s).map(|t| t.id.clone())).collect();
    Ok(PlaylistPlan { name: pl.name, owned, songs: pl.songs, existing })
}

/// Look a Spotify song up in the catalog: by ISRC (the recording's barcode, exact) first, then by
/// name, artist and length.
pub fn catalog_match(dz: &Deezer, s: &CsvSong) -> Option<DzTrack> {
    if let Some(isrc) = &s.isrc {
        if let Ok(v) = dz.raw(&format!("/track/isrc:{isrc}")) {
            if v["id"].as_u64().is_some() {
                let t = track_from(&v, None);
                // trust the code only if the song it names looks like this one
                let close = s.duration <= 0. || (t.duration - s.duration).abs() <= 5.;
                if close && (artist_overlap(&s.artist, &t.artist) || norm(&t.title) == norm(&s.title)) {
                    return Some(t);
                }
            }
        }
    }
    let q = format!("artist:\"{}\" track:\"{}\"", main_artist(&s.artist), s.title);
    let mut found = dz.search_tracks(&q).unwrap_or_default();
    if found.is_empty() {
        found = dz.search_tracks(&format!("{} {}", main_artist(&s.artist), s.title)).unwrap_or_default();
    }
    let nt = norm(&s.title);
    found
        .into_iter()
        .filter(|t| norm(&t.title) == nt || norm(&t.title).starts_with(&nt))
        .filter(|t| artist_overlap(&s.artist, &t.artist))
        .filter(|t| s.duration <= 0. || (t.duration - s.duration).abs() <= 10.)
        .min_by(|a, b| (a.duration - s.duration).abs().total_cmp(&(b.duration - s.duration).abs()))
}

#[derive(Clone, Debug, Default)]
pub struct ImportResult {
    /// set when this playlist couldn't be made or added to (the others still import)
    pub error: Option<String>,
    pub playlist_id: String,
    pub added: usize,
    pub queued: Vec<DzTrack>,
    pub not_found: Vec<CsvSong>,
}

/// Make (or reuse) the playlist, add the songs you have in Spotify's order, and look up the rest.
/// The caller queues `queued` for download into `playlist_id`.
pub fn run(client: &Client, dz: &Deezer, p: &PlaylistPlan, progress: &dyn Fn(usize, usize)) -> Result<ImportResult> {
    let id = match &p.existing {
        Some(id) => id.clone(),
        None => client.create_playlist(&p.name)?,
    };
    let already: std::collections::HashSet<String> = match &p.existing {
        Some(id) => client.playlist(id)?.tracks.into_iter().map(|t| t.id).collect(),
        None => Default::default(),
    };
    let mut seen = already.clone();
    let ids: Vec<String> = p.owned.iter().flatten().filter(|id| seen.insert((*id).clone())).cloned().collect();
    if !ids.is_empty() {
        client.playlist_add_many(&id, &ids)?;
    }
    let mut res = ImportResult { playlist_id: id, added: ids.len(), ..Default::default() };
    let missing: Vec<&CsvSong> = p.songs.iter().zip(&p.owned).filter(|(_, o)| o.is_none()).map(|(s, _)| s).collect();
    for (i, s) in missing.iter().enumerate() {
        progress(i, missing.len());
        match catalog_match(dz, s) {
            Some(t) => res.queued.push(t),
            None => res.not_found.push((*s).clone()),
        }
        std::thread::sleep(std::time::Duration::from_millis(120)); // stay well under the catalog's rate limit
    }
    progress(missing.len(), missing.len());
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_exportify() {
        let dir = std::env::temp_dir().join(format!("crate-import-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("Road_Trip_Mix.csv");
        std::fs::write(
            &f,
            "Track URI,ISRC,Track Name,Album Name,Artist Name(s),Release Date,Duration (ms)\n\
             spotify:track:1,USAAA0000001,First Song,Album One,Artist A;Artist B,2020-01-01,201000\n\
             spotify:local:x,,,,,,0\n\
             spotify:track:2,,Second Song,Album Two,Artist C,2021,180500\n",
        )
        .unwrap();
        let p = read_csv(&f).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(p.name, "Road Trip Mix");
        assert_eq!(p.songs.len(), 2);
        assert_eq!(p.songs[0].artist, "Artist A;Artist B");
        assert_eq!(main_artist("Old A, Old B"), "Old A");
        assert_eq!(main_artist("TOMMY,Coda,JIN"), "TOMMY,Coda,JIN");
        assert_eq!(main_artist(&p.songs[0].artist), "Artist A");
        assert_eq!(p.songs[0].duration, 201.);
        assert_eq!(p.songs[0].isrc.as_deref(), Some("USAAA0000001"));
        assert_eq!(p.songs[1].isrc, None);
    }
}
