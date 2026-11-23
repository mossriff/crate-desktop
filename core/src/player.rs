//! The audio engine. Runs on its own thread and owns the output device; the UI talks to it
//! with `Action`s and hears back through `Events`. Nothing here knows about Tauri.
use crate::media::Media;
use crate::model::Track;
use crate::subsonic::Client;
use rand::seq::SliceRandom;
use rodio::{OutputStream, Sink, Source};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const PRELOAD_SECS: f64 = 20.0; // start fetching the next song this long before the current one ends
/// A song counts as listened to (scrobbled) after half of it or this many seconds.
const SCROBBLE_AFTER: f64 = 240.0;
/// Loudness measured after a song started still gets applied (smoothly) this early in.
const LATE_GAIN_SECS: f64 = 8.0;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct PlayerState {
    pub queue: Vec<Track>,
    pub index: Option<usize>,
    pub playing: bool,
    pub loading: bool,
    pub volume: f32,
    pub muted: bool,
    pub shuffle: bool,
    pub repeat: Repeat,
    pub error: Option<String>,
    /// Queue order before shuffling, so turning shuffle off restores it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unshuffled: Option<Vec<Track>>,
    /// Play every song at about the same loudness.
    pub normalize: bool,
    /// Report plays to Navidrome (play counts, recently played).
    pub scrobble: bool,
}

impl Default for PlayerState {
    fn default() -> Self {
        PlayerState {
            queue: vec![],
            index: None,
            playing: false,
            loading: false,
            volume: 0.8,
            muted: false,
            shuffle: false,
            repeat: Repeat::Off,
            error: None,
            unshuffled: None,
            normalize: true,
            scrobble: true,
        }
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub position: f64,
    pub duration: f64,
    pub playing: bool,
    pub index: Option<usize>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Action {
    SetQueue { tracks: Vec<Track>, start: usize },
    Toggle,
    Play,
    Pause,
    Next,
    Prev,
    Seek { secs: f64 },
    SeekBy { delta: f64 },
    SetVolume { value: f32 },
    SetMuted { muted: bool },
    SetShuffle { on: bool },
    SetRepeat { mode: Repeat },
    Jump { index: usize },
    Enqueue { tracks: Vec<Track> },
    PlayNext { tracks: Vec<Track> },
    Remove { index: usize },
    ClearQueue,
    SetNormalize { on: bool },
    SetScrobble { on: bool },
}

pub enum Msg {
    Act(Action),
    Loaded { gen: u64, idx: usize, preload: bool, data: Result<PathBuf, String> },
    Cover { track_id: String, url: String },
}

pub trait Events: Send + 'static {
    /// Queue/flags changed (sent rarely; contains the whole queue).
    fn state(&self, s: &PlayerState);
    /// Position update (sent ~4x/second while playing).
    fn progress(&self, p: &Progress);
}

/// A song's volume adjustment (linear factor, as f32 bits), shared with its audio source so it
/// can change while playing; the source glides to a new value instead of jumping.
#[derive(Clone)]
struct Gain(Arc<AtomicU32>);
impl Gain {
    fn new(f: f32) -> Self {
        Gain(Arc::new(AtomicU32::new(f.to_bits())))
    }
    fn set(&self, f: f32) {
        self.0.store(f.to_bits(), Ordering::Relaxed);
    }
    fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
}

/// The song's gain, given whether normalization is on.
fn gain_for(t: &Track, normalize: bool) -> f32 {
    if normalize { 10f32.powf(crate::loudness::gain_db(t) / 20.0) } else { 1.0 }
}

/// Open a cached song file as a source with an adjustable gain.
fn open_source(path: &std::path::Path, gain: &Gain) -> Result<impl Source<Item = f32> + Send + 'static, String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let dec = crate::decoder(std::io::BufReader::with_capacity(64 * 1024, f))?;
    let g = gain.clone();
    let mut cur = g.get();
    Ok(dec.convert_samples::<f32>().amplify(cur).periodic_access(Duration::from_millis(50), move |s| {
        let want = g.get();
        if (want - cur).abs() > 1e-4 {
            cur += (want - cur) * 0.25;
            s.set_factor(cur);
        }
    }))
}

pub struct Player {
    tx: Sender<Msg>,
    shared: Arc<Mutex<PlayerState>>,
}

impl Player {
    pub fn spawn<E: Events>(client: Client, events: E) -> Self {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Mutex::new(PlayerState::default()));
        let (tx2, shared2) = (tx.clone(), shared.clone());
        thread::Builder::new()
            .name("crate-player".into())
            .spawn(move || run(client, events, tx2, rx, shared2))
            .expect("spawn player thread");
        Player { tx, shared }
    }

    pub fn send(&self, a: Action) {
        let _ = self.tx.send(Msg::Act(a));
    }

    pub fn state(&self) -> PlayerState {
        self.shared.lock().unwrap().clone()
    }
}

fn state_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("crate/player.json"))
}

fn run<E: Events>(client: Client, events: E, tx: Sender<Msg>, rx: Receiver<Msg>, shared: Arc<Mutex<PlayerState>>) {
    let mut st: PlayerState = state_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    st.playing = false;
    st.loading = false;
    st.error = None;

    let (_stream, handle) = match OutputStream::try_default() {
        Ok(x) => x,
        Err(e) => {
            st.error = Some(format!("No audio output: {e}"));
            *shared.lock().unwrap() = st.clone();
            events.state(&st);
            return;
        }
    };
    let sink = Sink::try_new(&handle).expect("audio sink");
    sink.pause();
    let media = match Media::new(tx.clone()) {
        Ok(m) => Some(m),
        Err(e) => {
            eprintln!("MPRIS unavailable: {e}");
            None
        }
    };
    let mut e = Engine {
        client,
        events,
        tx,
        shared,
        st,
        sink,
        media,
        gen: 0,
        preloaded: None,
        preload_requested: false,
        pending_seek: None,
        cur_gain: None,
        cur_gain_known: false,
        next_gain: None,
        scrobbled: false,
        cover_url: None,
        loaded: false,
        last_media_sync: Instant::now(),
    };
    e.apply_volume();
    e.publish();
    e.progress();
    e.sync_media(true);

    loop {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(m) => e.handle(m),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        while let Ok(m) = rx.try_recv() {
            e.handle(m);
        }
        e.tick();
    }
}

struct Engine<E: Events> {
    client: Client,
    events: E,
    tx: Sender<Msg>,
    shared: Arc<Mutex<PlayerState>>,
    st: PlayerState,
    sink: Sink,
    media: Option<Media>,
    /// Bumped on every hard load so stale downloads are ignored.
    gen: u64,
    /// The next song, already appended to the sink for a gapless switch: (queue index, track id).
    preloaded: Option<(usize, String)>,
    preload_requested: bool,
    pending_seek: Option<f64>,
    /// gains of the playing song and the preloaded one; `cur_gain_known` is false when the
    /// song started before its loudness was measured
    cur_gain: Option<Gain>,
    cur_gain_known: bool,
    next_gain: Option<Gain>,
    /// whether this play of the current song has been reported as listened to
    scrobbled: bool,
    cover_url: Option<String>,
    /// Whether the current song's audio is in the sink.
    loaded: bool,
    last_media_sync: Instant,
}

impl<E: Events> Engine<E> {
    // ------------------------------------------------------------ helpers
    fn current(&self) -> Option<&Track> {
        self.st.index.and_then(|i| self.st.queue.get(i))
    }

    fn position(&self) -> f64 {
        if self.loaded { self.sink.get_pos().as_secs_f64() } else { 0.0 }
    }

    fn duration(&self) -> f64 {
        self.current().map(|t| t.duration).unwrap_or(0.0)
    }

    fn apply_volume(&self) {
        let v = if self.st.muted { 0.0 } else { self.st.volume.clamp(0.0, 1.0) };
        self.sink.set_volume(v * v); // perceptual curve
    }

    /// Next index when a song ends by itself.
    fn next_auto(&self) -> Option<usize> {
        let i = self.st.index?;
        let n = self.st.queue.len();
        match self.st.repeat {
            Repeat::One => Some(i),
            _ if i + 1 < n => Some(i + 1),
            Repeat::All if n > 0 => Some(0),
            _ => None,
        }
    }

    /// Next index when the user presses "next" (repeat-one doesn't trap you).
    fn next_manual(&self) -> Option<usize> {
        let i = self.st.index?;
        let n = self.st.queue.len();
        if i + 1 < n {
            Some(i + 1)
        } else if self.st.repeat != Repeat::Off && n > 0 {
            Some(0)
        } else {
            None
        }
    }

    fn publish(&mut self) {
        *self.shared.lock().unwrap() = self.st.clone();
        self.events.state(&self.st);
        if let Some(p) = state_path() {
            let mut saved = self.st.clone();
            saved.playing = false;
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(b) = serde_json::to_vec(&saved) {
                let _ = std::fs::write(p, b);
            }
        }
    }

    fn progress(&self) {
        self.events.progress(&Progress {
            position: self.position(),
            duration: self.duration(),
            playing: self.st.playing,
            index: self.st.index,
        });
    }

    fn sync_media(&mut self, with_track: bool) {
        self.last_media_sync = Instant::now();
        let pos = self.position();
        let (playing, has) = (self.st.playing, self.current().is_some());
        let track = self.current().cloned();
        let cover = self.cover_url.clone();
        if let Some(m) = self.media.as_mut() {
            if with_track {
                m.set_track(track.as_ref(), cover.as_deref());
            }
            m.set_status(playing, has, pos);
        }
    }

    // ------------------------------------------------------------ loading
    fn fetch(&mut self, idx: usize, preload: bool) {
        let Some(t) = self.st.queue.get(idx).cloned() else { return };
        let (gen, tx) = (self.gen, self.tx.clone());
        // measure what's coming up so it plays at the right loudness
        if self.st.normalize {
            let soon: Vec<Track> = self.st.queue.iter().skip(idx).take(3).cloned().collect();
            crate::loudness::want_soon(&self.client, soon);
        }
        let client = self.client.clone();
        thread::spawn(move || {
            let data = crate::audio_cache::fetch(&client, &t);
            let _ = tx.send(Msg::Loaded { gen, idx, preload, data });
        });
    }

    /// Report the current song to Navidrome: playing now, or listened to.
    fn scrobble(&self, submission: bool) {
        if !self.st.scrobble {
            return;
        }
        let Some(id) = self.current().map(|t| t.id.clone()) else { return };
        let c = self.client.clone();
        thread::spawn(move || {
            let _ = c.scrobble(&id, submission);
        });
    }

    fn fetch_cover(&self) {
        let Some(t) = self.current().cloned() else { return };
        let Some(art) = t.cover_art.clone() else { return };
        let (client, tx) = (self.client.clone(), self.tx.clone());
        thread::spawn(move || {
            let Some(dir) = dirs::cache_dir().map(|d| d.join("crate/covers")) else { return };
            let safe: String = art.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
            let path = dir.join(format!("{safe}.jpg"));
            if !path.exists() {
                let Ok(b) = client.cover(&art, 512) else { return };
                let _ = std::fs::create_dir_all(&dir);
                if std::fs::write(&path, b).is_err() {
                    return;
                }
            }
            let _ = tx.send(Msg::Cover { track_id: t.id, url: format!("file://{}", path.display()) });
        });
    }

    /// Hard-load a queue entry (clears whatever is playing).
    fn load(&mut self, idx: usize, play: bool) {
        if idx >= self.st.queue.len() {
            return;
        }
        self.gen += 1;
        self.sink.clear();
        self.loaded = false;
        self.preloaded = None;
        self.preload_requested = false;
        self.next_gain = None;
        self.scrobbled = false;
        self.st.index = Some(idx);
        self.st.loading = true;
        self.st.playing = play;
        self.st.error = None;
        self.cover_url = None;
        self.fetch(idx, false);
        self.fetch_cover();
        self.publish();
        self.progress();
        self.sync_media(true);
    }

    fn on_loaded(&mut self, gen: u64, idx: usize, preload: bool, data: Result<PathBuf, String>) {
        if gen != self.gen {
            return;
        }
        let Some(track) = self.st.queue.get(idx).cloned() else { return };
        let id = track.id.clone();
        let path = match data {
            Ok(p) => p,
            Err(err) => {
                if preload {
                    self.preload_requested = false; // try again later
                } else {
                    self.fail(err);
                }
                return;
            }
        };
        let gain = Gain::new(gain_for(&track, self.st.normalize));
        let source = match open_source(&path, &gain) {
            Ok(s) => s,
            Err(err) => {
                if !preload {
                    self.fail(format!("Can't decode this file: {err}"));
                }
                return;
            }
        };
        if preload {
            // only keep it if it's still what comes next
            if self.next_auto() == Some(idx) && self.preloaded.is_none() {
                self.sink.append(source);
                self.preloaded = Some((idx, id));
                self.next_gain = Some(gain);
            }
            return;
        }
        self.sink.append(source);
        self.cur_gain_known = !self.st.normalize || crate::loudness::get(&id).is_some();
        self.cur_gain = Some(gain);
        self.loaded = true;
        self.st.loading = false;
        if let Some(s) = self.pending_seek.take() {
            let _ = self.sink.try_seek(Duration::from_secs_f64(s));
        }
        if self.st.playing {
            self.sink.play();
            self.scrobble(false);
        } else {
            self.sink.pause();
        }
        self.publish();
        self.progress();
        self.sync_media(false);
    }

    /// Point the playing/preloaded songs at their current gains (normalization toggled, or a
    /// measurement just came in).
    fn refresh_gains(&mut self) {
        let normalize = self.st.normalize;
        if let (Some(g), Some(t)) = (&self.cur_gain, self.current()) {
            g.set(gain_for(t, normalize));
        }
        if let (Some(g), Some((i, _))) = (&self.next_gain, &self.preloaded) {
            if let Some(t) = self.st.queue.get(*i) {
                g.set(gain_for(t, normalize));
            }
        }
    }

    fn fail(&mut self, err: String) {
        let title = self.current().map(|t| t.title.clone()).unwrap_or_default();
        self.st.loading = false;
        self.st.playing = false;
        self.loaded = false;
        self.st.error = Some(format!("Couldn't play “{title}”: {err}"));
        self.publish();
        self.sync_media(false);
    }

    /// After the queue or repeat mode changes, make sure the preloaded song is still the right one.
    fn revalidate_preload(&mut self) {
        let Some((_, id)) = self.preloaded.clone() else {
            self.preload_requested = false;
            return;
        };
        let next = self.next_auto();
        match next.and_then(|n| self.st.queue.get(n)).map(|t| t.id.clone()) {
            Some(nid) if nid == id => self.preloaded = Some((next.unwrap(), id)),
            _ => {
                // the sink already holds the wrong next song; reload the current one where it was
                if let Some(i) = self.st.index {
                    self.pending_seek = Some(self.position());
                    let playing = self.st.playing;
                    self.load(i, playing);
                }
            }
        }
    }

    // ------------------------------------------------------------ events
    fn handle(&mut self, m: Msg) {
        match m {
            Msg::Act(a) => self.act(a),
            Msg::Loaded { gen, idx, preload, data } => self.on_loaded(gen, idx, preload, data),
            Msg::Cover { track_id, url } => {
                if self.current().map(|t| t.id == track_id).unwrap_or(false) {
                    self.cover_url = Some(url);
                    self.sync_media(true);
                }
            }
        }
    }

    fn play(&mut self) {
        match self.st.index {
            None if !self.st.queue.is_empty() => self.load(0, true),
            Some(i) if !self.loaded && !self.st.loading => self.load(i, true),
            Some(_) => {
                self.st.playing = true;
                if self.sink.empty() {
                    let i = self.st.index.unwrap();
                    self.load(i, true);
                    return;
                }
                self.sink.play();
                self.publish();
                self.progress();
                self.sync_media(false);
            }
            None => {}
        }
    }

    fn pause(&mut self) {
        self.st.playing = false;
        self.sink.pause();
        self.publish();
        self.progress();
        self.sync_media(false);
    }

    fn seek(&mut self, secs: f64) {
        let secs = secs.clamp(0.0, (self.duration() - 0.5).max(0.0));
        if self.loaded {
            let _ = self.sink.try_seek(Duration::from_secs_f64(secs));
        } else {
            self.pending_seek = Some(secs);
        }
        self.progress();
        self.sync_media(false);
    }

    fn act(&mut self, a: Action) {
        match a {
            Action::SetQueue { tracks, start } => {
                if tracks.is_empty() {
                    return;
                }
                let start = start.min(tracks.len() - 1);
                if self.st.shuffle {
                    let mut rest: Vec<Track> = tracks.iter().enumerate().filter(|(i, _)| *i != start).map(|(_, t)| t.clone()).collect();
                    rest.shuffle(&mut rand::thread_rng());
                    let mut q = vec![tracks[start].clone()];
                    q.extend(rest);
                    self.st.unshuffled = Some(tracks);
                    self.st.queue = q;
                    self.load(0, true);
                } else {
                    self.st.unshuffled = None;
                    self.st.queue = tracks;
                    self.load(start, true);
                }
            }
            Action::Toggle => {
                if self.st.playing { self.pause() } else { self.play() }
            }
            Action::Play => self.play(),
            Action::Pause => self.pause(),
            Action::Next => {
                if let Some(n) = self.next_manual() {
                    self.load(n, true);
                }
            }
            Action::Prev => {
                if self.position() > 3.0 {
                    self.seek(0.0);
                } else if let Some(i) = self.st.index {
                    let n = self.st.queue.len();
                    if i > 0 {
                        self.load(i - 1, true);
                    } else if self.st.repeat == Repeat::All && n > 0 {
                        self.load(n - 1, true);
                    } else {
                        self.seek(0.0);
                    }
                }
            }
            Action::Seek { secs } => self.seek(secs),
            Action::SeekBy { delta } => {
                let p = self.position();
                self.seek(p + delta);
            }
            Action::SetVolume { value } => {
                self.st.volume = value.clamp(0.0, 1.0);
                self.st.muted = false;
                self.apply_volume();
                *self.shared.lock().unwrap() = self.st.clone();
            }
            Action::SetMuted { muted } => {
                self.st.muted = muted;
                self.apply_volume();
                self.publish();
            }
            Action::SetShuffle { on } => {
                if on == self.st.shuffle {
                    return;
                }
                self.st.shuffle = on;
                let cur = self.current().cloned();
                if on {
                    self.st.unshuffled = Some(self.st.queue.clone());
                    let mut rest: Vec<Track> = self.st.queue.clone();
                    if let Some(i) = self.st.index {
                        rest.remove(i);
                    }
                    rest.shuffle(&mut rand::thread_rng());
                    let mut q = Vec::new();
                    if let Some(c) = cur {
                        q.push(c);
                        self.st.index = Some(0);
                    }
                    q.extend(rest);
                    self.st.queue = q;
                } else if let Some(orig) = self.st.unshuffled.take() {
                    self.st.index = cur.and_then(|c| orig.iter().position(|t| t.id == c.id)).or(self.st.index.map(|_| 0));
                    self.st.queue = orig;
                }
                self.revalidate_preload();
                self.publish();
            }
            Action::SetRepeat { mode } => {
                self.st.repeat = mode;
                self.revalidate_preload();
                self.publish();
            }
            Action::Jump { index } => self.load(index, true),
            Action::Enqueue { tracks } => {
                if let Some(o) = self.st.unshuffled.as_mut() {
                    o.extend(tracks.iter().cloned());
                }
                self.st.queue.extend(tracks);
                self.revalidate_preload();
                self.publish();
            }
            Action::PlayNext { tracks } => {
                let at = self.st.index.map(|i| i + 1).unwrap_or(0);
                if let (Some(o), Some(cur)) = (self.st.unshuffled.as_mut(), self.st.index.and_then(|i| self.st.queue.get(i))) {
                    let p = o.iter().position(|t| t.id == cur.id).map(|p| p + 1).unwrap_or(o.len());
                    for (k, t) in tracks.iter().enumerate() {
                        o.insert(p + k, t.clone());
                    }
                }
                for (k, t) in tracks.into_iter().enumerate() {
                    self.st.queue.insert(at + k, t);
                }
                self.revalidate_preload();
                self.publish();
            }
            Action::Remove { index } => {
                if index >= self.st.queue.len() {
                    return;
                }
                let removed = self.st.queue.remove(index);
                if let Some(o) = self.st.unshuffled.as_mut() {
                    if let Some(p) = o.iter().position(|t| t.id == removed.id) {
                        o.remove(p);
                    }
                }
                match self.st.index {
                    Some(i) if index < i => {
                        self.st.index = Some(i - 1);
                        self.revalidate_preload();
                    }
                    Some(i) if index == i => {
                        if self.st.queue.is_empty() {
                            self.clear();
                        } else {
                            let playing = self.st.playing;
                            self.load(i.min(self.st.queue.len() - 1), playing);
                        }
                    }
                    _ => self.revalidate_preload(),
                }
                self.publish();
            }
            Action::ClearQueue => self.clear(),
            Action::SetNormalize { on } => {
                self.st.normalize = on;
                if on {
                    let soon: Vec<Track> = self.st.index.map(|i| self.st.queue.iter().skip(i).take(3).cloned().collect()).unwrap_or_default();
                    crate::loudness::want_soon(&self.client, soon);
                }
                self.refresh_gains();
                self.publish();
            }
            Action::SetScrobble { on } => {
                self.st.scrobble = on;
                self.publish();
            }
        }
    }

    fn clear(&mut self) {
        self.gen += 1;
        self.sink.clear();
        self.loaded = false;
        self.preloaded = None;
        self.preload_requested = false;
        self.st.queue.clear();
        self.st.unshuffled = None;
        self.st.index = None;
        self.st.playing = false;
        self.st.loading = false;
        self.publish();
        self.progress();
        self.sync_media(true);
    }

    fn tick(&mut self) {
        if self.st.index.is_none() || self.st.loading || !self.loaded {
            return;
        }
        // gapless switch: the preloaded song has started
        if let Some((p, _)) = self.preloaded.clone() {
            if self.sink.len() <= 1 && !self.sink.empty() {
                self.preloaded = None;
                self.preload_requested = false;
                self.st.index = Some(p);
                self.cur_gain = self.next_gain.take();
                self.cur_gain_known = true;
                self.scrobbled = false;
                self.scrobble(false);
                self.cover_url = None;
                self.fetch_cover();
                self.publish();
                self.sync_media(true);
            }
        }
        // song ended with nothing preloaded
        if self.sink.empty() && self.st.playing {
            match self.next_auto() {
                Some(n) => self.load(n, true),
                None => {
                    self.st.playing = false;
                    self.loaded = false;
                    self.publish();
                    self.sync_media(false);
                }
            }
            self.progress();
            return;
        }
        let (pos, dur) = (self.position(), self.duration());
        if self.st.playing && !self.scrobbled && dur > 0.0 && (pos >= dur / 2.0 || pos >= SCROBBLE_AFTER) {
            self.scrobbled = true;
            self.scrobble(true);
        }
        // the current song's loudness was measured just after it started: glide to it
        if !self.cur_gain_known && self.st.normalize {
            if pos > LATE_GAIN_SECS {
                self.cur_gain_known = true;
            } else if let Some(t) = self.current() {
                if crate::loudness::get(&t.id).is_some() {
                    self.cur_gain_known = true;
                    self.refresh_gains();
                }
            }
        }
        if self.st.playing && !self.preload_requested && self.preloaded.is_none() && dur > 0.0 && dur - pos < PRELOAD_SECS {
            if let Some(n) = self.next_auto() {
                self.preload_requested = true;
                self.fetch(n, true);
            }
        }
        if self.st.playing {
            self.progress();
            if self.last_media_sync.elapsed() > Duration::from_secs(10) {
                self.sync_media(false);
            }
        }
    }
}
