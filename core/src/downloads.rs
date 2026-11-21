//! Download manager: queue catalog songs, fetch them with sockseek (Soulseek, falling back to
//! YouTube), convert/tag/file them into the library, then add them to Navidrome playlists.
use crate::deezer::{Deezer, DzTrack, TagMeta};
use crate::library::find_song;
use crate::subsonic::Client;
use anyhow::{Context, Result};
use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::{ItemKey, Tag};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Soulseek bans accounts for 30 minutes after too many searches. sockseek's own limiter
/// (34 per 220 s) resets with every process, so the count is kept here across batches.
pub const SEARCH_LIMIT: usize = 30;
pub const SEARCH_WINDOW: f64 = 220.0;
const LOSSLESS: [&str; 4] = ["flac", "wav", "aiff", "aif"];

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Queued,
    Searching,
    Downloading,
    Processing,
    Scanning,
    Done,
    Failed,
}
impl Status {
    pub fn active(self) -> bool {
        !matches!(self, Status::Done | Status::Failed)
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Soulseek,
    Youtube,
    Library,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Job {
    pub id: String,
    pub track: DzTrack,
    pub playlists: Vec<String>,
    pub status: Status,
    pub source: Option<Source>,
    pub progress: Option<f64>,
    pub message: Option<String>,
    pub song_id: Option<String>,
    pub path: Option<String>,
    pub playlist_fail: Vec<String>,
    pub created: f64,
    pub updated: f64,
}
impl Default for Job {
    fn default() -> Self {
        Job {
            id: String::new(),
            track: DzTrack::default(),
            playlists: vec![],
            status: Status::Queued,
            source: None,
            progress: None,
            message: None,
            song_id: None,
            path: None,
            playlist_fail: vec![],
            created: 0.0,
            updated: 0.0,
        }
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Limiter {
    pub limit: usize,
    pub window: f64,
    pub used: usize,
    pub next_free: Option<f64>,
    pub paused_until: Option<f64>,
    pub now: f64,
}

pub trait DownloadEvents: Send + Sync + 'static {
    fn changed(&self, jobs: &[Job], limiter: &Limiter);
    /// A song finished and is in the library (and its playlists).
    fn finished(&self, job: &Job);
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("crate")
}

#[derive(Clone)]
pub struct Downloads {
    inner: Arc<Inner>,
}

struct Inner {
    client: Client,
    dz: Deezer,
    library: PathBuf,
    events: Box<dyn DownloadEvents>,
    jobs: Mutex<Vec<Job>>,
    searches: Mutex<VecDeque<f64>>,
    wake: (Mutex<bool>, Condvar),
    dirty: AtomicBool,
    last_emit: Mutex<Instant>,
    /// one `add` at a time, so the same song asked for twice at once (e.g. two imported
    /// playlists) becomes one download for both playlists, not two
    adding: Mutex<()>,
}

impl Downloads {
    pub fn start(client: Client, dz: Deezer, library: PathBuf, events: impl DownloadEvents) -> Self {
        let mut jobs: Vec<Job> = std::fs::read(data_dir().join("desktop-jobs.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let mut resume_scan = vec![];
        for j in jobs.iter_mut() {
            match j.status {
                Status::Scanning => resume_scan.push(j.id.clone()),
                s if s.active() => {
                    j.status = Status::Queued; // interrupted by a restart: try again
                    j.progress = None;
                }
                _ => {}
            }
        }
        let searches: VecDeque<f64> = std::fs::read(data_dir().join("searches.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<f64>>(&b).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|t| now() - t < SEARCH_WINDOW)
            .collect();
        let d = Downloads {
            inner: Arc::new(Inner {
                client,
                dz,
                library,
                events: Box::new(events),
                jobs: Mutex::new(jobs),
                searches: Mutex::new(searches),
                wake: (Mutex::new(true), Condvar::new()),
                dirty: AtomicBool::new(true),
                adding: Mutex::new(()),
                last_emit: Mutex::new(Instant::now()),
            }),
        };
        for id in resume_scan {
            let d2 = d.clone();
            thread::spawn(move || d2.attach_when_scanned(&id));
        }
        let d2 = d.clone();
        thread::Builder::new().name("crate-downloads".into()).spawn(move || d2.worker()).expect("spawn worker");
        d
    }

    // ------------------------------------------------------------ public API
    pub fn snapshot(&self) -> (Vec<Job>, Limiter) {
        // clone first and release the lock: limiter() takes the jobs lock too
        let jobs = self.inner.jobs.lock().unwrap().clone();
        (jobs, self.limiter())
    }

    /// Queue catalog songs for a playlist (or just the library when `playlist` is None).
    /// Songs already in the library are added straight away.
    pub fn add(&self, tracks: Vec<DzTrack>, playlist: Option<String>) {
        let d = self.clone();
        thread::spawn(move || {
            let _one_at_a_time = d.inner.adding.lock().unwrap();
            for t in tracks {
                // already queued? just attach the playlist
                {
                    let mut jobs = d.inner.jobs.lock().unwrap();
                    if let Some(j) = jobs.iter_mut().find(|j| j.track.id == t.id && j.status.active()) {
                        if let Some(p) = &playlist {
                            if !j.playlists.contains(p) {
                                j.playlists.push(p.clone());
                            }
                        }
                        continue;
                    }
                }
                let mut job = Job {
                    id: format!("{:012x}", rand::random::<u64>() & 0xffff_ffff_ffff),
                    track: t.clone(),
                    playlists: playlist.iter().cloned().collect(),
                    created: now(),
                    updated: now(),
                    ..Default::default()
                };
                if let Some(song) = find_song(&d.inner.client, &t.title, &t.artist, t.duration) {
                    job.status = Status::Done;
                    job.source = Some(Source::Library);
                    job.song_id = Some(song.id);
                    job.message = Some("Already in your library".into());
                    d.add_to_playlists(&mut job);
                    d.inner.jobs.lock().unwrap().push(job.clone());
                    d.inner.events.finished(&job);
                } else {
                    d.inner.jobs.lock().unwrap().push(job);
                }
                d.changed(true);
            }
            d.wake();
        });
    }

    pub fn retry(&self, id: &str) {
        self.update(id, |j| {
            if j.status == Status::Failed {
                j.status = Status::Queued;
                j.message = None;
                j.source = None;
                j.progress = None;
            }
        });
        self.wake();
    }

    pub fn dismiss(&self, id: &str) {
        self.inner.jobs.lock().unwrap().retain(|j| j.id != id || j.status.active());
        self.changed(true);
    }

    pub fn clear_finished(&self) {
        self.inner.jobs.lock().unwrap().retain(|j| j.status.active());
        self.changed(true);
    }

    // ------------------------------------------------------------ internals
    fn wake(&self) {
        let (m, c) = &self.inner.wake;
        *m.lock().unwrap() = true;
        c.notify_all();
    }

    fn limiter(&self) -> Limiter {
        let mut s = self.inner.searches.lock().unwrap();
        let t = now();
        while s.front().map(|x| t - x >= SEARCH_WINDOW).unwrap_or(false) {
            s.pop_front();
        }
        let waiting = self.inner.jobs.lock().unwrap().iter().any(|j| j.status == Status::Queued);
        let next_free = s.front().map(|x| x + SEARCH_WINDOW);
        Limiter {
            limit: SEARCH_LIMIT,
            window: SEARCH_WINDOW,
            used: s.len(),
            next_free,
            paused_until: if s.len() >= SEARCH_LIMIT && waiting { next_free } else { None },
            now: t,
        }
    }

    fn note_search(&self) {
        let mut s = self.inner.searches.lock().unwrap();
        s.push_back(now());
        let _ = std::fs::create_dir_all(data_dir());
        let _ = std::fs::write(data_dir().join("searches.json"), serde_json::to_vec(&s.iter().collect::<Vec<_>>()).unwrap_or_default());
    }

    /// Mark state changed; emits immediately when `force`, otherwise at most ~3x/second.
    fn changed(&self, force: bool) {
        self.inner.dirty.store(true, Ordering::SeqCst);
        let mut last = self.inner.last_emit.lock().unwrap();
        if force || last.elapsed() > Duration::from_millis(300) {
            *last = Instant::now();
            drop(last);
            self.emit();
        }
    }

    fn emit(&self) {
        self.inner.dirty.store(false, Ordering::SeqCst);
        let jobs = self.inner.jobs.lock().unwrap().clone();
        let _ = std::fs::create_dir_all(data_dir());
        let _ = std::fs::write(data_dir().join("desktop-jobs.json"), serde_json::to_vec(&jobs).unwrap_or_default());
        self.inner.events.changed(&jobs, &self.limiter());
    }

    fn update(&self, id: &str, f: impl FnOnce(&mut Job)) {
        let status_changed;
        {
            let mut jobs = self.inner.jobs.lock().unwrap();
            let Some(j) = jobs.iter_mut().find(|j| j.id == id) else { return };
            let before = j.status;
            f(j);
            j.updated = now();
            status_changed = before != j.status;
        }
        self.changed(status_changed);
    }

    fn job(&self, id: &str) -> Option<Job> {
        self.inner.jobs.lock().unwrap().iter().find(|j| j.id == id).cloned()
    }

    fn add_to_playlists(&self, job: &mut Job) {
        let Some(song) = job.song_id.clone() else { return };
        let mut failed = vec![];
        for pid in &job.playlists {
            if let Err(e) = self.inner.client.playlist_add(pid, &song) {
                failed.push(pid.clone());
                let e = format!("{e:#}");
                job.message = Some(if e.to_lowercase().contains("not authorized") {
                    "In your library, but this playlist is auto-imported from a file, so Navidrome won't let it be edited. \
                     Turn off Auto-import for it in Navidrome; it'll be added then."
                        .into()
                } else {
                    format!("In your library, but adding to the playlist failed: {e}")
                });
            }
        }
        if failed.is_empty() && job.message.as_deref().map(|m| m.contains("playlist")).unwrap_or(false) {
            job.message = None;
        }
        job.playlist_fail = failed;
    }

    fn retry_playlist_adds(&self) {
        let todo: Vec<Job> = self
            .inner
            .jobs
            .lock()
            .unwrap()
            .iter()
            .filter(|j| j.status == Status::Done && j.song_id.is_some() && !j.playlist_fail.is_empty())
            .cloned()
            .collect();
        for mut j in todo {
            self.add_to_playlists(&mut j);
            let (fail, msg) = (j.playlist_fail.clone(), j.message.clone());
            self.update(&j.id, |x| {
                x.playlist_fail = fail;
                x.message = msg;
            });
        }
    }

    fn worker(&self) {
        let mut last_retry = Instant::now() - Duration::from_secs(60);
        loop {
            {
                let (m, c) = &self.inner.wake;
                let g = m.lock().unwrap();
                let (mut g, _) = c.wait_timeout_while(g, Duration::from_secs(1), |w| !*w).unwrap();
                *g = false;
            }
            if self.inner.dirty.load(Ordering::SeqCst) {
                self.emit();
            }
            if last_retry.elapsed() > Duration::from_secs(30) {
                last_retry = Instant::now();
                self.retry_playlist_adds();
            }
            let budget = SEARCH_LIMIT.saturating_sub(self.limiter().used);
            if budget == 0 {
                self.changed(false); // keeps the countdown fresh in the UI
                continue;
            }
            let batch: Vec<Job> = self
                .inner
                .jobs
                .lock()
                .unwrap()
                .iter()
                .filter(|j| j.status == Status::Queued)
                .take(budget.min(25))
                .cloned()
                .collect();
            if batch.is_empty() {
                continue;
            }
            if let Err(e) = self.run_batch(&batch, budget) {
                for j in &batch {
                    self.update(&j.id, |x| {
                        if matches!(x.status, Status::Queued | Status::Searching | Status::Downloading) {
                            x.status = Status::Failed;
                            x.message = Some(format!("Error: {e:#}"));
                        }
                    });
                }
            }
            self.wake(); // look for more right away
        }
    }

    fn run_batch(&self, batch: &[Job], budget: usize) -> Result<()> {
        let stamp = format!("{}-{:04x}", now() as u64, rand::random::<u16>());
        let dir = data_dir().join("incoming").join(stamp);
        std::fs::create_dir_all(&dir)?;
        let mut keys: HashMap<(String, String), String> = HashMap::new();
        {
            let mut w = csv::Writer::from_path(dir.join("in.csv"))?;
            w.write_record(["Artist", "Title", "Album", "Length"])?;
            for j in batch {
                let t = &j.track;
                w.write_record([&t.artist, &t.title, &t.album, &format!("{}", t.duration as u64)])?;
                keys.insert((t.artist.to_lowercase(), t.title.to_lowercase()), j.id.clone());
                self.update(&j.id, |x| {
                    x.status = Status::Searching;
                    x.message = None;
                    x.progress = None;
                });
            }
            w.flush()?;
        }
        let out = dir.join("out");
        let mut child = Command::new(crate::setup::tool_path("sockseek").unwrap_or_else(|| "sockseek".into()))
            .env("PATH", crate::setup::child_path())
            .current_dir(&dir)
            .args(["in.csv", "-o"])
            .arg(&out)
            .arg("--index-path")
            .arg(dir.join("index.csv"))
            .arg("--playlist-path")
            .arg(dir.join("batch.m3u8"))
            .args(["--yt-dlp", "--progress-json", "--no-skip-existing", "--name-format", "{snum} - {stitle}"])
            .args(["--searches-per-time", &budget.max(1).to_string(), "--searches-renew-time", &(SEARCH_WINDOW as u64).to_string()])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("couldn't start sockseek")?;
        let stderr = child.stderr.take().unwrap();
        let errlog = dir.join("sockseek.err.log");
        thread::spawn(move || {
            if let Ok(mut f) = std::fs::File::create(errlog) {
                let _ = std::io::copy(&mut BufReader::new(stderr), &mut f);
            }
        });
        let mut log = std::fs::File::create(dir.join("sockseek.log"))?;
        let job_for = |d: &Value| -> Option<String> {
            let a = d["artist"].as_str().unwrap_or_default().to_lowercase();
            let t = d["title"].as_str().unwrap_or_default().to_lowercase();
            keys.get(&(a, t)).cloned()
        };
        let mut pending: Vec<(Option<u64>, String)> = vec![]; // downloads started, waiting for their progress jobId
        let mut by_dl: HashMap<String, String> = HashMap::new();
        let mut finished: Vec<String> = vec![];

        for line in BufReader::new(child.stdout.take().unwrap()).lines() {
            let Ok(line) = line else { break };
            let _ = writeln!(log, "{line}");
            let low = line.to_lowercase();
            if low.contains("yt-dlp") || low.contains("running fallback") {
                for j in batch {
                    if !finished.contains(&j.id) && low.contains(&j.track.title.to_lowercase()) {
                        self.update(&j.id, |x| {
                            x.source = Some(Source::Youtube);
                            x.status = Status::Downloading;
                            x.message = Some("Not on Soulseek, trying YouTube".into());
                        });
                    }
                }
            }
            if !line.starts_with('{') {
                continue;
            }
            let Ok(ev) = serde_json::from_str::<Value>(&line) else { continue };
            let d = &ev["data"];
            match ev["type"].as_str().unwrap_or_default() {
                "search_start" => {
                    self.note_search();
                    if let Some(id) = job_for(d) {
                        self.update(&id, |x| x.status = Status::Searching);
                    }
                }
                "download_start" => {
                    if let Some(id) = job_for(d) {
                        let ext = d["extension"].as_str().unwrap_or_default().to_lowercase();
                        let user = d["username"].as_str().unwrap_or_default().to_string();
                        let lossless = LOSSLESS.contains(&ext.as_str());
                        self.update(&id, |x| {
                            x.status = Status::Downloading;
                            x.source = Some(Source::Soulseek);
                            x.progress = Some(0.0);
                            x.message = Some(format!(
                                "{} from {}{}",
                                ext.to_uppercase(),
                                user,
                                if lossless { " (will be converted to 320k MP3)" } else { "" }
                            ));
                        });
                        pending.push((d["size"].as_u64(), id));
                    }
                }
                "download_progress" => {
                    let dl = d["jobId"].as_str().unwrap_or_default().to_string();
                    if !by_dl.contains_key(&dl) && !pending.is_empty() {
                        // progress events only carry a jobId: match it to its download by file size
                        let total = d["totalBytes"].as_u64();
                        let k = pending
                            .iter()
                            .position(|(size, _)| size.is_some() && *size == total)
                            .or(if pending.len() == 1 { Some(0) } else { None });
                        if let Some(k) = k {
                            by_dl.insert(dl.clone(), pending.remove(k).1);
                        }
                    }
                    if let Some(id) = by_dl.get(&dl) {
                        let pct = d["percent"].as_f64();
                        self.update(id, |x| x.progress = pct);
                    }
                }
                "track_state" if d["lifecycleState"] == "Terminal" => {
                    let Some(id) = job_for(d) else { continue };
                    finished.push(id.clone());
                    match d["terminalOutcome"].as_str().unwrap_or_default() {
                        "Succeeded" => {
                            if let Some(path) = d["downloadPath"].as_str() {
                                self.update(&id, |x| {
                                    if x.source.is_none() {
                                        x.source = Some(Source::Youtube);
                                    }
                                });
                                let (me, path) = (self.clone(), PathBuf::from(path));
                                thread::spawn(move || me.postprocess(&id, path));
                            }
                        }
                        "Skipped" => {
                            self.update(&id, |x| {
                                x.status = Status::Scanning;
                                x.message = Some("Already in your library".into());
                            });
                            let me = self.clone();
                            thread::spawn(move || me.attach_when_scanned(&id));
                        }
                        other => {
                            let reason = d["failureReason"].as_str().unwrap_or(other).to_string();
                            let pretty = if reason == "NoSearchResults" { "Not found on Soulseek or YouTube".to_string() } else { reason };
                            self.update(&id, |x| {
                                x.status = Status::Failed;
                                x.message = Some(pretty);
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        let status = child.wait()?;
        // anything without a terminal event: fall back to sockseek's index file
        let rows: Vec<csv::StringRecord> = csv::Reader::from_path(dir.join("index.csv"))
            .map(|mut r| r.records().filter_map(|x| x.ok()).collect())
            .unwrap_or_default();
        for j in batch {
            if finished.contains(&j.id) {
                continue;
            }
            let row = rows.iter().find(|r| {
                r.get(1).map(|a| a.eq_ignore_ascii_case(&j.track.artist)).unwrap_or(false)
                    && r.get(3).map(|t| t.eq_ignore_ascii_case(&j.track.title)).unwrap_or(false)
            });
            match row {
                Some(r) if r.get(6) == Some("1") && !r.get(0).unwrap_or_default().is_empty() => {
                    let path = dir.join(r.get(0).unwrap());
                    let (me, id) = (self.clone(), j.id.clone());
                    thread::spawn(move || me.postprocess(&id, path));
                }
                _ => self.update(&j.id, |x| {
                    x.status = Status::Failed;
                    x.message = Some(format!("sockseek stopped ({status}); see {}", dir.join("sockseek.log").display()));
                }),
            }
        }
        Ok(())
    }

    fn postprocess(&self, id: &str, path: PathBuf) {
        if let Err(e) = self.try_postprocess(id, path) {
            self.update(id, |x| {
                x.status = Status::Failed;
                x.message = Some(format!("Processing failed: {e:#}"));
            });
        }
    }

    fn try_postprocess(&self, id: &str, mut path: PathBuf) -> Result<()> {
        self.update(id, |x| {
            x.status = Status::Processing;
            x.progress = None;
        });
        let job = self.job(id).context("job vanished")?;
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_lowercase();
        if LOSSLESS.contains(&ext.as_str()) {
            path = ffmpeg(&path, "mp3", &["-map", "0:a:0", "-map", "0:v?", "-c:a", "libmp3lame", "-b:a", "320k", "-c:v", "copy",
                "-disposition:v", "attached_pic", "-map_metadata", "0", "-id3v2_version", "3"])?;
        } else if ext == "webm" {
            path = ffmpeg(&path, "opus", &["-vn", "-c:a", "copy"])?;
        }
        let mut meta = self.inner.dz.tag_meta(&job.track)?;
        write_tags(&path, &meta)?;
        if crate::fixer::auto() {
            match crate::fixer::on_download(&self.inner.library, &path, &meta.album_artist, &meta.album) {
                // already in the library (maybe under another spelling): use that copy
                crate::fixer::Arrival::Duplicate(rel) => {
                    let _ = std::fs::remove_file(&path);
                    let existing = self.inner.library.join(rel).to_string_lossy().to_string();
                    self.update(id, |x| {
                        x.path = Some(existing);
                        x.status = Status::Scanning;
                        x.message = Some("Already in your library".into());
                    });
                    self.attach_when_scanned(id);
                    return Ok(());
                }
                // file it with the spelling its album already has, so the album isn't split
                crate::fixer::Arrival::File { album_artist, album } => {
                    if album_artist != meta.album_artist || album != meta.album {
                        meta.album_artist = album_artist;
                        meta.album = album;
                        write_tags(&path, &meta)?;
                    }
                }
            }
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("mp3").to_lowercase();
        let prefix = match (meta.track, meta.disc) {
            (Some(n), Some(d)) if d > 1 => format!("{d}-{n:02}. "),
            (Some(n), _) => format!("{n:02}. "),
            _ => String::new(),
        };
        let album = if meta.album.is_empty() { "Singles".to_string() } else { meta.album.clone() };
        let dest = self.inner.library.join(safe(&meta.album_artist)).join(safe(&album)).join(format!("{}.{ext}", safe(&format!("{prefix}{}", meta.title))));
        if dest.exists() {
            let _ = std::fs::remove_file(&path); // same song already filed there
        } else {
            std::fs::create_dir_all(dest.parent().unwrap())?;
            if std::fs::rename(&path, &dest).is_err() {
                std::fs::copy(&path, &dest)?;
                let _ = std::fs::remove_file(&path);
            }
        }
        let dest_s = dest.to_string_lossy().to_string();
        self.update(id, |x| {
            x.path = Some(dest_s);
            x.status = Status::Scanning;
            x.message = Some("Waiting for Navidrome to pick it up".into());
        });
        self.attach_when_scanned(id);
        Ok(())
    }

    fn attach_when_scanned(&self, id: &str) {
        let Some(job) = self.job(id) else { return };
        self.inner.client.start_scan();
        let deadline = Instant::now() + Duration::from_secs(600);
        while Instant::now() < deadline {
            let t = &job.track;
            let mut song = find_song(&self.inner.client, &t.title, &t.artist, t.duration);
            if song.is_none() {
                // match on the tags actually written, in case the catalog title differs a little
                if let Some(p) = &job.path {
                    if let Ok(f) = lofty::read_from_path(p) {
                        if let Some(tag) = f.primary_tag() {
                            let (title, artist) = (tag.title().unwrap_or_default().to_string(), tag.artist().unwrap_or_default().to_string());
                            song = find_song(&self.inner.client, &title, &artist, t.duration);
                        }
                    }
                }
            }
            if let Some(s) = song {
                let mut j = self.job(id).unwrap_or(job.clone());
                j.song_id = Some(s.id);
                j.message = None;
                self.add_to_playlists(&mut j);
                let (sid, fail, msg) = (j.song_id.clone(), j.playlist_fail.clone(), j.message.clone());
                self.update(id, |x| {
                    x.song_id = sid;
                    x.playlist_fail = fail;
                    x.message = msg;
                    x.status = Status::Done;
                });
                if let Some(done) = self.job(id) {
                    self.inner.events.finished(&done);
                }
                return;
            }
            thread::sleep(Duration::from_secs(4));
        }
        self.update(id, |x| {
            x.status = Status::Done;
            x.message = Some("Saved to your library, but Navidrome hasn't shown it yet. Add it to the playlist once it appears.".into());
        });
    }
}

fn ffmpeg(input: &Path, ext: &str, args: &[&str]) -> Result<PathBuf> {
    let out = input.with_extension(ext);
    let part = input.with_extension(format!("{ext}.part"));
    let st = Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(input)
        .args(args)
        .args(["-f", if ext == "mp3" { "mp3" } else { ext }])
        .arg(&part)
        .status()
        .context("ffmpeg not available")?;
    anyhow::ensure!(st.success(), "ffmpeg failed converting to {ext}");
    std::fs::rename(&part, &out)?;
    std::fs::remove_file(input)?;
    Ok(out)
}

fn write_tags(path: &Path, meta: &TagMeta) -> Result<()> {
    let mut file = Probe::open(path)?.read()?;
    if file.primary_tag().is_none() {
        let tt = file.primary_tag_type();
        file.insert_tag(Tag::new(tt));
    }
    let tag = file.primary_tag_mut().context("no tag")?;
    tag.set_title(meta.title.clone());
    tag.set_artist(meta.artist.clone());
    tag.set_album(meta.album.clone());
    tag.insert_text(ItemKey::AlbumArtist, meta.album_artist.clone());
    if let Some(n) = meta.track {
        tag.set_track(n);
    }
    if let Some(d) = meta.disc {
        tag.set_disk(d);
    }
    if !meta.date.is_empty() {
        tag.insert_text(ItemKey::RecordingDate, meta.date.clone());
    }
    if !meta.genre.is_empty() {
        tag.set_genre(meta.genre.clone());
    }
    if tag.pictures().is_empty() {
        if let Some(url) = &meta.cover_url {
            if let Ok(r) = reqwest::blocking::get(url) {
                if let Ok(bytes) = r.bytes() {
                    tag.push_picture(Picture::new_unchecked(PictureType::CoverFront, Some(MimeType::Jpeg), None, bytes.to_vec()));
                }
            }
        }
    }
    tag.save_to_path(path, WriteOptions::default())?;
    Ok(())
}

/// Make a string safe as a file/folder name (same rules as the original Crate script).
pub fn safe(s: &str) -> String {
    let cleaned: String = s.chars().map(|c| if "/\\:*?\"<>|".contains(c) || c.is_control() { ' ' } else { c }).collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_matches(|c: char| c == '.' || c.is_whitespace());
    let out: String = trimmed.chars().take(120).collect();
    let out = out.trim().to_string();
    if out.is_empty() { "_".into() } else { out }
}
