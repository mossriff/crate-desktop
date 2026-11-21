//! Shared app state. Views observe `Model` (rarely changes) and `Clock` (playback position,
//! changes several times a second) separately, so position updates only redraw the player bar.
use crate::covers::Covers;
use crate::catalog::{Results, SearchKind, Toast};
use crate_core::deezer::{Deezer, DzAlbum, DzArtist, DzTrack};
use crate_core::downloads::{DownloadEvents, Downloads, Job, Limiter};
use crate_core::model::{AlbumSummary, ArtistSummary, LibrarySearch, Playlist, PlaylistSummary, Track};
use crate_core::player::{Action, Events, Player, PlayerState, Progress};
use crate_core::settings::Settings;
use crate_core::subsonic::Client;
use gpui::{AppContext, Context, Entity, Task};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Debug, PartialEq)]
pub enum Route {
    Home,
    Playlist(String),
    Library,
    Album(String),
    Artist(String),
    Search,
    DzAlbum(u64),
    DzArtist(u64),
    Lyrics,
    Settings,
    /// one setup step, opened from Settings
    Setup(crate::onboarding::Step),
    /// bring Spotify playlists over (Exportify CSV files)
    Import,
}

/// Lyrics display: original text, original + romaji underneath, or romaji only.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LyrMode {
    Original,
    Both,
    Romaji,
}

impl LyrMode {
    pub fn as_str(self) -> &'static str {
        match self {
            LyrMode::Original => "original",
            LyrMode::Both => "both",
            LyrMode::Romaji => "romaji",
        }
    }
}

/// Lyrics for one song: still loading, none found, or found.
#[derive(Clone)]
pub enum LyricsState {
    Loading,
    Missing,
    Found(Arc<crate_core::lyrics::Lyrics>),
}

/// The right-hand panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Panel {
    None,
    Queue,
    Downloads,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LibTab {
    Songs,
    Albums,
    Artists,
}

impl LibTab {
    pub fn as_str(self) -> &'static str {
        match self {
            LibTab::Songs => "songs",
            LibTab::Albums => "albums",
            LibTab::Artists => "artists",
        }
    }
}

/// Album sort orders offered on the Library › Albums tab (Subsonic getAlbumList2 types).
pub const ALBUM_SORTS: [(&str, &str); 5] = [
    ("alphabeticalByName", "A–Z"),
    ("alphabeticalByArtist", "Artist"),
    ("newest", "Recently added"),
    ("frequent", "Most played"),
    ("recent", "Recently played"),
];

/// Key of the song list a route shows (playlist id, "__library", or "album:<id>").
pub fn list_key(r: &Route, tab: LibTab) -> Option<String> {
    match r {
        Route::Playlist(id) => Some(id.clone()),
        Route::Library if tab == LibTab::Songs => Some("__library".into()),
        Route::Album(id) => Some(format!("album:{id}")),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SortKey {
    Title,
    Album,
    Created,
    Duration,
}

impl SortKey {
    pub fn as_str(self) -> &'static str {
        match self {
            SortKey::Title => "title",
            SortKey::Album => "album",
            SortKey::Created => "created",
            SortKey::Duration => "duration",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "title" => SortKey::Title,
            "album" => SortKey::Album,
            "created" => SortKey::Created,
            "duration" => SortKey::Duration,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sort {
    pub key: Option<SortKey>,
    pub desc: bool,
}

/// Playback position, interpolated between engine updates.
pub struct Clock {
    pub progress: Progress,
    pub at: Instant,
}

impl Clock {
    pub fn position(&self) -> f64 {
        let mut p = self.progress.position;
        if self.progress.playing {
            p += self.at.elapsed().as_secs_f64();
        }
        p.min(self.progress.duration.max(0.0))
    }
}

pub struct Model {
    pub client: Client,
    pub player: Player,
    pub settings: Arc<Settings>,
    pub covers: Covers,
    pub clock: Entity<Clock>,
    pub playlists: Vec<PlaylistSummary>,
    pub route: Route,
    pub history: Vec<Route>,
    pub future: Vec<Route>,
    pub player_state: PlayerState,
    /// Which playlist the current queue was started from (for the sidebar/play button).
    pub queue_source: Option<String>,
    /// Song lists by `list_key`: playlists, the whole library, albums.
    pub cache: HashMap<String, Arc<Playlist>>,
    pub album_meta: HashMap<String, AlbumSummary>,
    /// Album grids by sort order.
    pub albums: HashMap<String, Arc<Vec<AlbumSummary>>>,
    pub artists: Option<Arc<Vec<ArtistSummary>>>,
    pub artist_pages: HashMap<String, Arc<(ArtistSummary, Vec<AlbumSummary>)>>,
    pub lib_tab: LibTab,
    pub album_sort: String,
    pub panel: Panel,
    // ---- catalog / downloads (see catalog.rs)
    pub dz: Deezer,
    pub downloads: Option<Downloads>,
    pub jobs: Vec<Job>,
    pub limiter: Option<Limiter>,
    /// local clock minus the limiter's clock, for countdowns
    pub skew: f64,
    /// catalog track id -> the library copy (None = not in library)
    pub owned: HashMap<u64, Option<Track>>,
    pub search_q: String,
    /// recent searches, newest first (deletable on the Search page)
    pub search_history: Vec<String>,
    pub search_kind: SearchKind,
    pub results: Option<Results>,
    pub lib_matches: Option<LibrarySearch>,
    pub searching: bool,
    pub(crate) search_seq: u64,
    pub dz_albums: HashMap<u64, Arc<(DzAlbum, Vec<DzTrack>)>>,
    pub dz_artists: HashMap<u64, Arc<(DzArtist, Vec<DzTrack>, Vec<DzAlbum>)>>,
    pub toast: Option<Toast>,
    /// Navidrome's own-API session (for playlist covers); in memory only, it was made from the password
    pub nd_session: Option<String>,
    /// a cover change that needs the password first: playlist, picture (None = automatic cover)
    pub cover_wants_password: Option<(String, Option<std::path::PathBuf>)>,
    /// Lyrics of the current song (fetched as soon as a song starts, so the view opens instantly).
    pub lyrics: Option<(String, LyricsState)>,
    pub lyr_mode: LyrMode,
    pub preview: crate_core::preview::Preview,
    /// catalog id of the song being previewed
    pub previewing: Option<u64>,
    pub(crate) toast_seq: u64,
    /// Settings page: audio cache size, and how many library songs have a loudness measurement
    pub storage: Option<crate::prefs::Storage>,
    pub(crate) loud_scan_started: bool,
    pub fix: crate::prefs::Fix,
    /// shown song-list columns: album, date added
    pub show_cols: [bool; 2],
    /// shown Home rows: recently played, most played
    pub home_rows: [bool; 2],
    /// song the album-art accent was last taken from
    pub(crate) art_accent_for: Option<String>,
    pub fix_progress: crate::prefs::FixProgress,
    pub import: crate::importer::Importing,
    pub import_progress: crate::importer::ImportProgress,
    pub(crate) fix_plan: Arc<std::sync::Mutex<Option<(Arc<crate_core::fixer::Plan>, crate_core::fixer::Index)>>>,
    /// last playlist edit, for Ctrl+Z
    pub undo: Option<crate::edit::Undo>,
    pub error: Option<String>,
    _tasks: Vec<Task<()>>,
}

enum Ev {
    State(PlayerState),
    Progress(Progress),
    Jobs(Vec<Job>, Limiter),
    Finished(Job),
    Previewing(Option<u64>),
    PreviewFailed(String),
}

#[derive(Clone)]
struct Bridge(async_channel::Sender<Ev>);
impl crate_core::preview::PreviewEvents for Bridge {
    fn previewing(&self, id: Option<u64>) {
        let _ = self.0.send_blocking(Ev::Previewing(id));
    }
    fn failed(&self, _id: u64, error: String) {
        let _ = self.0.send_blocking(Ev::PreviewFailed(error));
    }
}
impl DownloadEvents for Bridge {
    fn changed(&self, jobs: &[Job], limiter: &Limiter) {
        let _ = self.0.send_blocking(Ev::Jobs(jobs.to_vec(), limiter.clone()));
    }
    fn finished(&self, job: &Job) {
        let _ = self.0.send_blocking(Ev::Finished(job.clone()));
    }
}
impl Events for Bridge {
    fn state(&self, s: &PlayerState) {
        let _ = self.0.send_blocking(Ev::State(s.clone()));
    }
    fn progress(&self, p: &Progress) {
        let _ = self.0.send_blocking(Ev::Progress(p.clone()));
    }
}

impl Model {
    pub fn new(client: Client, cx: &mut Context<Self>) -> Self {
        let (tx, rx) = async_channel::unbounded();
        let player = Player::spawn(client.clone(), Bridge(tx.clone()));
        let dz = Deezer::new();
        let preview = crate_core::preview::Preview::spawn(Bridge(tx.clone()));
        let downloads = Downloads::start(client.clone(), dz.clone(), crate_core::subsonic::library_dir(), Bridge(tx));
        let settings = Arc::new(Settings::load());
        let covers = Covers::start(client.clone());
        let clock = cx.new(|_| Clock {
            progress: Progress { position: 0.0, duration: 0.0, playing: false, index: None },
            at: Instant::now(),
        });
        let route = settings
            .get("lastView")
            .and_then(|v| match (v["type"].as_str(), v["id"].as_str()) {
                (Some("playlist"), Some(id)) => Some(Route::Playlist(id.to_string())),
                (Some("library"), _) => Some(Route::Library),
                (Some("libAlbum"), Some(id)) => Some(Route::Album(id.to_string())),
                (Some("libArtist"), Some(id)) => Some(Route::Artist(id.to_string())),
                (Some("search"), _) => Some(Route::Search),
                (Some("settings"), _) => Some(Route::Settings),
                (Some("lyrics"), _) => Some(Route::Lyrics), // only via test hooks; never saved
                (Some("import"), _) => Some(Route::Import),
                (Some("setup"), _) => Some(Route::Setup(crate::onboarding::Step::Tools)),
                (Some("dzAlbum"), _) => v["id"].as_u64().map(Route::DzAlbum),
                (Some("dzArtist"), _) => v["id"].as_u64().map(Route::DzArtist),
                _ => None,
            })
            .unwrap_or(Route::Home);
        crate_core::fixer::set_auto(settings.get("autoFix").and_then(|v| v.as_bool()).unwrap_or(true));
        crate::theme::set_accent(settings.get("accent").as_ref().and_then(|v| v.as_str()).unwrap_or("amber"));
        let lyr_mode = match settings.get("lyrMode").as_ref().and_then(|v| v.as_str()) {
            Some("original") => LyrMode::Original,
            Some("romaji") => LyrMode::Romaji,
            _ => LyrMode::Both,
        };
        let lib_tab = match settings.get("libTab").as_ref().and_then(|v| v.as_str()) {
            Some("albums") => LibTab::Albums,
            Some("artists") => LibTab::Artists,
            _ => LibTab::Songs,
        };
        let album_sort = settings.get("libAlbumSort").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| "alphabeticalByName".into());
        let panel = match settings.get("panel").as_ref().and_then(|v| v.as_str()) {
            Some("queue") => Panel::Queue,
            Some("downloads") => Panel::Downloads,
            _ => Panel::None,
        };
        let search_history: Vec<String> = settings.get("searchHistory").and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
        let search_q = settings.get("searchQ").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        let search_kind = SearchKind::parse(settings.get("searchKind").as_ref().and_then(|v| v.as_str()).unwrap_or("track"));

        // engine events -> UI thread
        let clock2 = clock.clone();
        let events = cx.spawn(async move |this, cx| {
            while let Ok(ev) = rx.recv().await {
                match ev {
                    Ev::State(s) => {
                        if this.update(cx, |m, cx| m.set_player_state(s, cx)).is_err() {
                            break;
                        }
                    }
                    Ev::Jobs(jobs, limiter) => {
                        if this.update(cx, |m, cx| m.set_jobs(jobs, limiter, cx)).is_err() {
                            break;
                        }
                    }
                    Ev::Previewing(id) => {
                        let _ = this.update(cx, |m, cx| {
                            m.previewing = id;
                            cx.notify();
                        });
                    }
                    Ev::PreviewFailed(e) => {
                        let _ = this.update(cx, |m, cx| m.toast(format!("Preview failed: {e}"), true, cx));
                    }
                    Ev::Finished(job) => {
                        let _ = this.update(cx, |m, cx| m.download_finished(job, cx));
                    }
                    Ev::Progress(p) => {
                        let _ = clock2.update(cx, |c, cx| {
                            c.progress = p;
                            c.at = Instant::now();
                            cx.notify();
                        });
                    }
                }
            }
        });
        // redraw when covers arrive
        let dirty = covers.dirty.clone();
        let cover_poll = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(std::time::Duration::from_millis(120)).await;
            if dirty.swap(false, std::sync::atomic::Ordering::Relaxed) && this.update(cx, |_, cx| cx.notify()).is_err() {
                break;
            }
        });

        let mut m = Model {
            client,
            player,
            settings,
            covers,
            clock,
            playlists: vec![],
            route,
            history: vec![],
            future: vec![],
            player_state: PlayerState::default(),
            queue_source: None,
            cache: HashMap::new(),
            album_meta: HashMap::new(),
            albums: HashMap::new(),
            artists: None,
            artist_pages: HashMap::new(),
            lib_tab,
            album_sort,
            panel,
            dz,
            downloads: Some(downloads),
            jobs: vec![],
            limiter: None,
            skew: 0.0,
            owned: HashMap::new(),
            search_q,
            search_history,
            search_kind,
            results: None,
            lib_matches: None,
            searching: false,
            search_seq: 0,
            dz_albums: HashMap::new(),
            dz_artists: HashMap::new(),
            toast: None,
            nd_session: None,
            cover_wants_password: None,
            lyrics: None,
            lyr_mode,
            preview,
            previewing: None,
            toast_seq: 0,
            storage: None,
            loud_scan_started: false,
            fix: crate::prefs::Fix::Idle,
            show_cols: [true; 2],
            home_rows: [true; 2],
            art_accent_for: None,
            fix_progress: Default::default(),
            import: Default::default(),
            import_progress: Default::default(),
            fix_plan: Default::default(),
            undo: None,
            error: None,
            _tasks: vec![events, cover_poll],
        };
        m._tasks.push(crate::catalog::ticker(cx));
        m.player_state = m.player.state();
        m.ensure_lyrics(cx);
        if let Some(d) = &m.downloads {
            let (jobs, limiter) = d.snapshot();
            m.set_jobs(jobs, limiter, cx);
        }
        m.reload_playlists(cx);
        m.load_route(cx);
        m.start_loudness_scan(cx);
        m.load_appearance(cx);
        m.update_art_accent(cx);
        m
    }

    fn set_player_state(&mut self, s: PlayerState, cx: &mut Context<Self>) {
        if let Some(e) = &s.error {
            if self.player_state.error.as_ref() != Some(e) {
                self.error = Some(e.clone());
            }
        }
        // starting real playback ends any preview
        if s.playing && !self.player_state.playing && self.previewing.is_some() {
            self.preview.stop();
        }
        self.player_state = s;
        self.ensure_lyrics(cx);
        self.update_art_accent(cx);
        cx.notify();
    }

    pub fn current(&self) -> Option<&Track> {
        self.player_state.index.and_then(|i| self.player_state.queue.get(i))
    }

    pub fn send(&self, a: Action) {
        self.player.send(a);
    }

    pub fn play_tracks(&mut self, tracks: Vec<Track>, start: usize, source: Option<String>, cx: &mut Context<Self>) {
        if tracks.is_empty() {
            return;
        }
        self.queue_source = source;
        self.send(Action::SetQueue { tracks, start });
        cx.notify();
    }

    /// Volume changes aren't broadcast by the engine (too chatty while dragging), so keep our copy in step.
    pub fn set_volume(&mut self, value: f32, cx: &mut Context<Self>) {
        let value = value.clamp(0.0, 1.0);
        self.player_state.volume = value;
        self.player_state.muted = false;
        self.send(Action::SetVolume { value });
        cx.notify();
    }

    /// Play a whole song list (playlist id / "album:<id>") in its saved sort order, fetching it first if needed.
    pub fn play_playlist(&mut self, id: String, shuffle: bool, cx: &mut Context<Self>) {
        let start = move |m: &mut Model, pl: &Playlist, cx: &mut Context<Model>| {
            let rows = sorted_rows(pl, m.sort_for(&pl.id), "");
            let tracks: Vec<Track> = rows.iter().map(|&k| pl.tracks[k].clone()).collect();
            if tracks.is_empty() {
                return;
            }
            m.send(Action::SetShuffle { on: shuffle });
            let first = if shuffle { random_index(tracks.len()) } else { 0 };
            m.play_tracks(tracks, first, Some(pl.id.clone()), cx);
        };
        if let Some(pl) = self.cache.get(&id).cloned() {
            return start(self, &pl, cx);
        }
        let c = self.client.clone();
        cx.spawn(async move |this, cx| {
            let id2 = id.clone();
            let r = cx.background_executor().spawn(async move { fetch_list(&c, &id2) }).await;
            let _ = this.update(cx, |m, cx| match r {
                Ok((pl, meta)) => {
                    if let Some(a) = meta {
                        m.album_meta.insert(a.id.clone(), a);
                    }
                    start(m, &pl, cx);
                    m.cache.insert(id, Arc::new(pl));
                }
                Err(e) => {
                    m.error = Some(format!("{e:#}"));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    // ------------------------------------------------------------ navigation
    pub fn go(&mut self, r: Route, cx: &mut Context<Self>) {
        if r == self.route {
            return;
        }
        // opening something you searched for: remember the search
        if self.route == Route::Search && !self.search_q.trim().is_empty() {
            let q = self.search_q.clone();
            self.remember_search(&q, cx);
        }
        let prev = std::mem::replace(&mut self.route, r.clone());
        self.history.push(prev);
        self.future.clear();
        self.after_route(cx);
    }

    pub fn back(&mut self, cx: &mut Context<Self>) {
        if let Some(r) = self.history.pop() {
            let cur = std::mem::replace(&mut self.route, r);
            self.future.push(cur);
            self.after_route(cx);
        }
    }

    pub fn forward(&mut self, cx: &mut Context<Self>) {
        if let Some(r) = self.future.pop() {
            let cur = std::mem::replace(&mut self.route, r);
            self.history.push(cur);
            self.after_route(cx);
        }
    }

    pub(crate) fn after_route_pub(&mut self, cx: &mut Context<Self>) {
        self.after_route(cx)
    }

    fn after_route(&mut self, cx: &mut Context<Self>) {
        let v = match &self.route {
            Route::Home => json!({ "type": "home" }),
            Route::Playlist(id) => json!({ "type": "playlist", "id": id }),
            Route::Library => json!({ "type": "library" }),
            Route::Album(id) => json!({ "type": "libAlbum", "id": id }),
            Route::Artist(id) => json!({ "type": "libArtist", "id": id }),
            Route::Search => json!({ "type": "search" }),
            Route::DzAlbum(id) => json!({ "type": "dzAlbum", "id": id }),
            Route::DzArtist(id) => json!({ "type": "dzArtist", "id": id }),
            Route::Lyrics | Route::Setup(_) | Route::Import => Value::Null,
            Route::Settings => json!({ "type": "settings" }),
        };
        if !v.is_null() {
            self.persist("lastView", v);
        }
        self.load_route(cx);
        cx.notify();
    }

    /// Open the lyrics page, or go back to where you were if it's already open.
    pub fn toggle_lyrics(&mut self, cx: &mut Context<Self>) {
        if self.route == Route::Lyrics {
            self.back(cx)
        } else {
            self.go(Route::Lyrics, cx)
        }
    }

    pub fn set_lyr_mode(&mut self, mode: LyrMode, cx: &mut Context<Self>) {
        self.lyr_mode = mode;
        self.persist("lyrMode", json!(mode.as_str()));
        cx.notify();
    }

    /// Fetch lyrics for the current song if we don't have them yet.
    fn ensure_lyrics(&mut self, cx: &mut Context<Self>) {
        let Some(t) = self.current().cloned() else { return };
        if self.lyrics.as_ref().map(|(id, _)| id == &t.id).unwrap_or(false) {
            return;
        }
        self.lyrics = Some((t.id.clone(), LyricsState::Loading));
        let c = self.client.clone();
        cx.spawn(async move |this, cx| {
            let t2 = t.clone();
            let r = cx.background_executor().spawn(async move { crate_core::lyrics::get(&c, &t2) }).await;
            let _ = this.update(cx, |m, cx| {
                if m.lyrics.as_ref().map(|(id, _)| id == &t.id).unwrap_or(false) {
                    let st = match r {
                        Ok(Some(l)) => LyricsState::Found(Arc::new(l)),
                        _ => LyricsState::Missing,
                    };
                    m.lyrics = Some((t.id.clone(), st));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn set_lib_tab(&mut self, t: LibTab, cx: &mut Context<Self>) {
        self.lib_tab = t;
        self.persist("libTab", json!(t.as_str()));
        self.load_route(cx);
        cx.notify();
    }

    pub fn set_album_sort(&mut self, kind: &str, cx: &mut Context<Self>) {
        self.album_sort = kind.to_string();
        self.persist("libAlbumSort", json!(kind));
        self.load_route(cx);
        cx.notify();
    }

    /// Fetch whatever the current page needs (cached data shows immediately and is refreshed).
    pub(crate) fn load_route(&mut self, cx: &mut Context<Self>) {
        if let Some(key) = list_key(&self.route, self.lib_tab) {
            return self.load_list(key, cx);
        }
        match self.route.clone() {
            Route::Library if self.lib_tab == LibTab::Albums => self.load_albums(cx),
            Route::Library if self.lib_tab == LibTab::Artists => self.load_artists(cx),
            Route::Artist(id) => self.load_artist(id, cx),
            Route::Search if self.results.is_none() && !self.search_q.is_empty() => {
                let q = self.search_q.clone();
                self.run_search(q, cx)
            }
            Route::DzAlbum(id) => self.load_dz_album(id, cx),
            Route::DzArtist(id) => self.load_dz_artist(id, cx),
            Route::Home => {
                self.load_album_kind("recent".into(), cx);
                self.load_album_kind("frequent".into(), cx);
            }
            Route::Settings => self.refresh_storage(cx),
            _ => {}
        }
    }

    // ------------------------------------------------------------ data
    pub fn reload_playlists(&mut self, cx: &mut Context<Self>) {
        let c = self.client.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { c.playlists() }).await;
            let _ = this.update(cx, |m, cx| {
                match r {
                    Ok(p) => m.playlists = p,
                    Err(e) => m.error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Fetch a song list by `list_key` (cached copy, if any, stays on screen meanwhile).
    pub fn load_list(&mut self, id: String, cx: &mut Context<Self>) {
        let c = self.client.clone();
        cx.spawn(async move |this, cx| {
            let id2 = id.clone();
            let r = cx.background_executor().spawn(async move { fetch_list(&c, &id2) }).await;
            let _ = this.update(cx, |m, cx| {
                match r {
                    Ok((p, meta)) => {
                        if let Some(a) = meta {
                            m.album_meta.insert(a.id.clone(), a);
                        }
                        m.covers.prefetch(p.tracks.iter().filter_map(|t| t.cover_art.as_deref()), crate::covers::SMALL);
                        m.cache.insert(id, Arc::new(p));
                    }
                    Err(e) => m.error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_albums(&mut self, cx: &mut Context<Self>) {
        self.load_album_kind(self.album_sort.clone(), cx);
    }

    pub(crate) fn load_album_kind(&mut self, kind: String, cx: &mut Context<Self>) {
        let c = self.client.clone();
        cx.spawn(async move |this, cx| {
            let k2 = kind.clone();
            let r = cx
                .background_executor()
                .spawn(async move { if k2 == "alphabeticalByName" { c.all_albums() } else { c.albums(&k2, 500, 0) } })
                .await;
            let _ = this.update(cx, |m, cx| {
                match r {
                    Ok(a) => {
                        m.covers.prefetch(a.iter().filter_map(|x| x.cover_art.as_deref()), crate::covers::LARGE);
                        m.albums.insert(kind, Arc::new(a));
                    }
                    Err(e) => m.error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_artists(&mut self, cx: &mut Context<Self>) {
        let c = self.client.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { c.artists() }).await;
            let _ = this.update(cx, |m, cx| {
                match r {
                    Ok(a) => m.artists = Some(Arc::new(a)),
                    Err(e) => m.error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_artist(&mut self, id: String, cx: &mut Context<Self>) {
        let c = self.client.clone();
        cx.spawn(async move |this, cx| {
            let id2 = id.clone();
            let r = cx.background_executor().spawn(async move { c.artist(&id2) }).await;
            let _ = this.update(cx, |m, cx| {
                match r {
                    Ok(a) => {
                        m.artist_pages.insert(id, Arc::new(a));
                    }
                    Err(e) => m.error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Play every album by an artist, in album order.
    pub fn play_artist(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(page) = self.artist_pages.get(&id).cloned() else { return };
        let c = self.client.clone();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(async move {
                    let mut all = vec![];
                    for a in &page.1 {
                        if let Ok((_, t)) = c.album(&a.id) {
                            all.extend(t);
                        }
                    }
                    all
                })
                .await;
            let _ = this.update(cx, |m, cx| {
                m.send(Action::SetShuffle { on: false });
                m.play_tracks(r, 0, None, cx);
            });
        })
        .detach();
    }

    // ------------------------------------------------------------ settings
    pub(crate) fn persist(&self, key: &str, v: Value) {
        let s = self.settings.clone();
        let key = key.to_string();
        std::thread::spawn(move || s.set(&key, v)); // never block the UI thread on disk
    }

    pub fn sort_for(&self, id: &str) -> Sort {
        let v = self.settings.get("sorts").and_then(|s| s.get(id).cloned()).unwrap_or(Value::Null);
        let default = if id == "__library" { Sort { key: Some(SortKey::Created), desc: true } } else { Sort { key: None, desc: false } };
        if v.is_null() {
            return default;
        }
        Sort { key: v["key"].as_str().and_then(SortKey::parse), desc: v["dir"].as_i64() == Some(-1) }
    }

    pub fn set_sort(&mut self, id: &str, s: Sort, cx: &mut Context<Self>) {
        let mut all = self.settings.get("sorts").unwrap_or_else(|| json!({}));
        all[id] = json!({ "key": s.key.map(|k| k.as_str()), "dir": if s.desc { -1 } else { 1 } });
        self.persist("sorts", all);
        cx.notify();
    }

    pub fn toggle_panel(&mut self, p: Panel, cx: &mut Context<Self>) {
        self.panel = if self.panel == p { Panel::None } else { p };
        let v = match self.panel {
            Panel::None => Value::Null,
            Panel::Queue => json!("queue"),
            Panel::Downloads => json!("downloads"),
        };
        self.persist("panel", v);
        cx.notify();
    }

    pub fn toggle_queue(&mut self, cx: &mut Context<Self>) {
        self.toggle_panel(Panel::Queue, cx)
    }
}

/// Fetch a song list by key: a playlist id, "__library" (every song), or "album:<id>".
fn fetch_list(c: &Client, key: &str) -> anyhow::Result<(Playlist, Option<AlbumSummary>)> {
    if key == "__library" {
        let tracks = c.all_songs()?;
        return Ok((Playlist { id: key.into(), name: "Your library".into(), comment: None, cover_art: None, tracks }, None));
    }
    if let Some(id) = key.strip_prefix("album:") {
        let (a, tracks) = c.album(id)?;
        let pl = Playlist { id: key.into(), name: a.name.clone(), comment: None, cover_art: a.cover_art.clone(), tracks };
        return Ok((pl, Some(a)));
    }
    Ok((c.playlist(key)?, None))
}

pub fn random_index(n: usize) -> usize {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0) as usize;
    t % n.max(1)
}

pub fn sorted_rows(pl: &Playlist, sort: Sort, filter: &str) -> Vec<usize> {
    let f = filter.to_lowercase();
    let mut rows: Vec<usize> = (0..pl.tracks.len())
        .filter(|&i| {
            if f.is_empty() {
                return true;
            }
            let t = &pl.tracks[i];
            t.title.to_lowercase().contains(&f) || t.artist.to_lowercase().contains(&f) || t.album.to_lowercase().contains(&f)
        })
        .collect();
    if let Some(k) = sort.key {
        rows.sort_by(|&a, &b| {
            let (x, y) = (&pl.tracks[a], &pl.tracks[b]);
            let o = match k {
                SortKey::Title => x.title.to_lowercase().cmp(&y.title.to_lowercase()),
                SortKey::Album => x.album.to_lowercase().cmp(&y.album.to_lowercase()),
                SortKey::Created => x.created.cmp(&y.created),
                SortKey::Duration => x.duration.partial_cmp(&y.duration).unwrap_or(std::cmp::Ordering::Equal),
            };
            if sort.desc { o.reverse() } else { o }
        });
    }
    rows
}

