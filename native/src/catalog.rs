//! Search, the online catalog (Deezer), "do I already own this?", downloads and adding songs to
//! playlists. UI-free: these are `Model` methods the pages call.
use crate::model::{list_key, Model, Panel, Route};
use crate_core::deezer::{DzAlbum, DzArtist, DzTrack};
use crate_core::downloads::{Job, Limiter, Status};
use crate_core::library::find_song;
use crate_core::model::Track;
use crate_core::player::Action;
use gpui::{Context, Task};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SearchKind {
    Track,
    Album,
    Artist,
}

impl SearchKind {
    pub fn parse(s: &str) -> Self {
        match s {
            "album" => SearchKind::Album,
            "artist" => SearchKind::Artist,
            _ => SearchKind::Track,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            SearchKind::Track => "track",
            SearchKind::Album => "album",
            SearchKind::Artist => "artist",
        }
    }
}

pub enum Results {
    Tracks(Arc<Vec<DzTrack>>),
    Albums(Arc<Vec<DzAlbum>>),
    Artists(Arc<Vec<DzArtist>>),
    Failed(String),
}

/// Something being added to a playlist / downloaded (from a menu or a drag).
#[derive(Clone, Debug)]
pub enum Payload {
    /// Catalog songs: downloaded if needed, then added.
    Dz(Vec<DzTrack>),
    /// A whole catalog album.
    DzAlbum(u64),
    /// Songs already in the library.
    Lib(Vec<Track>),
}

impl Payload {
    pub fn label(&self) -> String {
        match self {
            Payload::Dz(t) if t.len() == 1 => t[0].title.clone(),
            Payload::Lib(t) if t.len() == 1 => t[0].title.clone(),
            Payload::Dz(t) => format!("{} songs", t.len()),
            Payload::Lib(t) => format!("{} songs", t.len()),
            Payload::DzAlbum(_) => "Album".into(),
        }
    }
}

#[derive(Clone)]
pub struct Toast {
    pub text: String,
    pub bad: bool,
}

pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.)
}

/// Redraw once a second while the downloads panel shows a live countdown or progress.
pub fn ticker(cx: &mut Context<Model>) -> Task<()> {
    cx.spawn(async move |this, cx| loop {
        cx.background_executor().timer(Duration::from_secs(1)).await;
        let r = this.update(cx, |m, cx| {
            let live = m.jobs.iter().any(|j| j.status.active()) || m.limiter.as_ref().and_then(|l| l.paused_until).is_some();
            if m.panel == Panel::Downloads && live {
                cx.notify();
            }
            // Settings shows the loudness scan's progress
            if m.route == crate::model::Route::Settings && (crate_core::loudness::progress().1 > 0 || matches!(m.fix, crate::prefs::Fix::Working)) {
                cx.notify();
            }
            if m.route == crate::model::Route::Import && m.import.working() {
                cx.notify();
            }
        });
        if r.is_err() {
            break;
        }
    })
}

impl Model {
    pub fn toast(&mut self, text: impl Into<String>, bad: bool, cx: &mut Context<Self>) {
        self.toast_seq += 1;
        let seq = self.toast_seq;
        self.toast = Some(Toast { text: text.into(), bad });
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(3200)).await;
            let _ = this.update(cx, |m, cx| {
                if m.toast_seq == seq {
                    m.toast = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn active_downloads(&self) -> usize {
        self.jobs.iter().filter(|j| j.status.active()).count()
    }

    pub(crate) fn set_jobs(&mut self, mut jobs: Vec<Job>, limiter: Limiter, cx: &mut Context<Self>) {
        jobs.sort_by(|a, b| b.created.partial_cmp(&a.created).unwrap_or(std::cmp::Ordering::Equal));
        self.skew = now() - limiter.now;
        self.jobs = jobs;
        self.limiter = Some(limiter);
        cx.notify();
    }

    /// A download landed in the library: refresh whatever shows it.
    pub(crate) fn download_finished(&mut self, job: Job, cx: &mut Context<Self>) {
        self.owned.remove(&job.track.id);
        self.cache.remove("__library");
        self.albums.clear();
        self.artists = None;
        self.reload_playlists(cx);
        for pid in &job.playlists {
            if self.cache.contains_key(pid) {
                self.load_list(pid.clone(), cx);
            }
        }
        if list_key(&self.route, self.lib_tab).as_deref() == Some("__library") {
            self.load_route(cx);
        }
        let mut recheck = vec![job.track.clone()];
        if let Some(Results::Tracks(t)) = &self.results {
            recheck.extend(t.iter().cloned());
        }
        self.check_owned(recheck, cx);
    }

    // ------------------------------------------------------------ search
    pub fn set_search_kind(&mut self, k: SearchKind, cx: &mut Context<Self>) {
        self.search_kind = k;
        self.persist("searchKind", json!(k.as_str()));
        let q = self.search_q.clone();
        self.run_search(q, cx);
    }

    /// Keep a search in the recent list (newest first, no repeats, at most 12).
    pub fn remember_search(&mut self, q: &str, cx: &mut Context<Self>) {
        let q = q.trim();
        if q.chars().count() < 2 {
            return;
        }
        let low = q.to_lowercase();
        self.search_history.retain(|h| h.to_lowercase() != low);
        self.search_history.insert(0, q.to_string());
        self.search_history.truncate(12);
        self.persist("searchHistory", serde_json::json!(self.search_history));
        cx.notify();
    }

    pub fn forget_search(&mut self, q: &str, cx: &mut Context<Self>) {
        self.search_history.retain(|h| h != q);
        self.persist("searchHistory", serde_json::json!(self.search_history));
        cx.notify();
    }

    pub fn clear_search_history(&mut self, cx: &mut Context<Self>) {
        self.search_history.clear();
        self.persist("searchHistory", serde_json::json!(self.search_history));
        cx.notify();
    }

    pub fn run_search(&mut self, q: String, cx: &mut Context<Self>) {
        let q = q.trim().to_string();
        self.persist("searchQ", json!(q));
        self.search_q = q.clone();
        self.search_seq += 1;
        let seq = self.search_seq;
        if q.is_empty() {
            self.results = None;
            self.lib_matches = None;
            self.searching = false;
            cx.notify();
            return;
        }
        self.searching = true;
        cx.notify();
        let (dz, c, kind) = (self.dz.clone(), self.client.clone(), self.search_kind);
        cx.spawn(async move |this, cx| {
            let (res, mine) = cx
                .background_executor()
                .spawn(async move {
                    let res = match kind {
                        SearchKind::Track => dz.search_tracks(&q).map(|v| Results::Tracks(Arc::new(v))),
                        SearchKind::Album => dz.search_albums(&q).map(|v| Results::Albums(Arc::new(v))),
                        SearchKind::Artist => dz.search_artists(&q).map(|v| Results::Artists(Arc::new(v))),
                    }
                    .unwrap_or_else(|e| Results::Failed(format!("{e:#}")));
                    (res, c.search_library(&q).ok())
                })
                .await;
            let _ = this.update(cx, |m, cx| {
                if m.search_seq != seq {
                    return; // a newer search is on its way
                }
                if let Results::Tracks(t) = &res {
                    m.check_owned(t.to_vec(), cx);
                }
                m.results = Some(res);
                m.lib_matches = mine;
                m.searching = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// Work out which catalog songs are already in the library (8 lookups at a time).
    pub fn check_owned(&mut self, tracks: Vec<DzTrack>, cx: &mut Context<Self>) {
        let need: Vec<DzTrack> = tracks.into_iter().filter(|t| !self.owned.contains_key(&t.id)).collect();
        if need.is_empty() {
            return;
        }
        let c = self.client.clone();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    let mut out = vec![];
                    for chunk in need.chunks(8) {
                        let part: Vec<(u64, Option<Track>)> = std::thread::scope(|s| {
                            let hs: Vec<_> = chunk.iter().map(|t| s.spawn(|| (t.id, find_song(&c, &t.title, &t.artist, t.duration)))).collect();
                            hs.into_iter().filter_map(|h| h.join().ok()).collect()
                        });
                        out.extend(part);
                    }
                    out
                })
                .await;
            let _ = this.update(cx, |m, cx| {
                for (id, t) in found {
                    m.owned.insert(id, t);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Play the owned songs of a catalog list, starting from `start` (if it's owned).
    pub fn play_owned(&mut self, list: &[DzTrack], start: usize, cx: &mut Context<Self>) {
        if self.route == crate::model::Route::Search {
            let q = self.search_q.clone();
            self.remember_search(&q, cx);
        }
        let libs: Vec<Track> = list.iter().filter_map(|t| self.owned.get(&t.id).cloned().flatten()).collect();
        if libs.is_empty() {
            return self.toast("You don't have any of these yet. Add them to a playlist or download them first.", false, cx);
        }
        let first = list.get(start).and_then(|t| self.owned.get(&t.id).cloned().flatten()).and_then(|lt| libs.iter().position(|x| x.id == lt.id)).unwrap_or(0);
        self.send(Action::SetShuffle { on: false });
        self.play_tracks(libs, first, None, cx);
    }

    pub fn load_dz_album(&mut self, id: u64, cx: &mut Context<Self>) {
        let dz = self.dz.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { dz.album(id) }).await;
            let _ = this.update(cx, |m, cx| {
                match r {
                    Ok(a) => {
                        m.check_owned(a.1.clone(), cx);
                        m.dz_albums.insert(id, Arc::new(a));
                    }
                    Err(e) => m.toast(format!("Couldn't load album: {e:#}"), true, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn load_dz_artist(&mut self, id: u64, cx: &mut Context<Self>) {
        let dz = self.dz.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { dz.artist(id) }).await;
            let _ = this.update(cx, |m, cx| {
                match r {
                    Ok(a) => {
                        m.check_owned(a.1.clone(), cx);
                        m.dz_artists.insert(id, Arc::new(a));
                    }
                    Err(e) => m.toast(format!("Couldn't load artist: {e:#}"), true, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Play (or stop) the 30-second preview of a catalog song. Pauses the main player.
    pub fn toggle_preview(&mut self, t: &DzTrack, cx: &mut Context<Self>) {
        if self.previewing == Some(t.id) {
            self.preview.stop();
            return;
        }
        let Some(url) = t.preview.clone().filter(|u| !u.is_empty()) else {
            return self.toast("No preview available for this one", false, cx);
        };
        if self.player_state.playing {
            self.send(Action::Pause);
        }
        let v = if self.player_state.muted { 0.0 } else { self.player_state.volume };
        self.preview.play(t.id, &url, (v * v).max(0.05)); // same loudness curve as the player
        self.previewing = Some(t.id);
        self.toast(format!("Preview: {} (30 s)", t.title), false, cx);
    }

    // ------------------------------------------------------------ adding & downloading
    fn playlist_name(&self, pid: &Option<String>) -> String {
        match pid {
            None => "your library".into(),
            Some(id) => self.playlists.iter().find(|p| &p.id == id).map(|p| p.name.clone()).unwrap_or_else(|| "playlist".into()),
        }
    }

    /// Add songs to a playlist (`None` = just get them into the library), downloading what's missing.
    pub fn add_to(&mut self, pid: Option<String>, payload: Payload, cx: &mut Context<Self>) {
        let name = self.playlist_name(&pid);
        match payload {
            Payload::Lib(tracks) => {
                let Some(pid) = pid else { return self.toast("Already in your library", false, cx) };
                let c = self.client.clone();
                let ids: Vec<String> = tracks.iter().map(|t| t.id.clone()).collect();
                let one = (tracks.len() == 1).then(|| tracks[0].title.clone());
                cx.spawn(async move |this, cx| {
                    let p2 = pid.clone();
                    let r = cx.background_executor().spawn(async move { c.playlist_add_many(&p2, &ids) }).await;
                    let _ = this.update(cx, |m, cx| match r {
                        Ok(0) => m.toast(format!("Already in {name}"), false, cx),
                        Ok(n) => {
                            let what = match (&one, n) {
                                (Some(t), 1) => format!("\u{201c}{t}\u{201d}"),
                                _ => format!("{n} songs"),
                            };
                            m.toast(format!("{what} \u{2192} {name}"), false, cx);
                            m.reload_playlists(cx);
                            m.load_list(pid, cx);
                        }
                        Err(e) => m.toast(format!("Couldn't add: {e:#}"), true, cx),
                    });
                })
                .detach();
            }
            Payload::DzAlbum(id) => {
                let dz = self.dz.clone();
                cx.spawn(async move |this, cx| {
                    let r = cx.background_executor().spawn(async move { dz.album(id) }).await;
                    let _ = this.update(cx, |m, cx| match r {
                        Ok((_, tracks)) => m.add_to(pid, Payload::Dz(tracks), cx),
                        Err(e) => m.toast(format!("Couldn't load album: {e:#}"), true, cx),
                    });
                })
                .detach();
            }
            Payload::Dz(tracks) => {
                if tracks.is_empty() {
                    return;
                }
                let Some(d) = self.downloads.clone() else { return self.toast("Downloads aren't available", true, cx) };
                let what = if tracks.len() == 1 { format!("\u{201c}{}\u{201d}", tracks[0].title) } else { format!("{} songs", tracks.len()) };
                d.add(tracks, pid);
                self.toast(format!("{what} \u{2192} {name}"), false, cx);
            }
        }
    }

    pub fn create_playlist(&mut self, name: String, then: Option<Payload>, cx: &mut Context<Self>) {
        let c = self.client.clone();
        cx.spawn(async move |this, cx| {
            let n2 = name.clone();
            let r = cx.background_executor().spawn(async move { c.create_playlist(&n2) }).await;
            let _ = this.update(cx, |m, cx| match r {
                Ok(id) => {
                    m.toast(format!("Created \u{201c}{name}\u{201d}"), false, cx);
                    // make the new name known before add_to uses it in its message
                    m.playlists.push(crate_core::model::PlaylistSummary { id: id.clone(), name, song_count: 0, duration: 0., cover_art: None });
                    m.reload_playlists(cx);
                    if let Some(p) = then {
                        m.add_to(Some(id), p, cx);
                    }
                }
                Err(e) => m.toast(format!("Couldn't create playlist: {e:#}"), true, cx),
            });
        })
        .detach();
    }

    pub fn retry_download(&self, id: &str) {
        if let Some(d) = &self.downloads {
            d.retry(id);
        }
    }
    pub fn dismiss_download(&self, id: &str) {
        if let Some(d) = &self.downloads {
            d.dismiss(id);
        }
    }
    pub fn clear_downloads(&self) {
        if let Some(d) = &self.downloads {
            d.clear_finished();
        }
    }
}

/// Human status line for a download.
pub fn status_text(j: &Job, paused: bool) -> String {
    match j.status {
        Status::Queued if paused => "Waiting for search limit".into(),
        Status::Queued => "Queued".into(),
        Status::Searching => "Searching…".into(),
        Status::Downloading => match j.progress {
            Some(p) => format!("Downloading {}%", p.round()),
            None => "Downloading…".into(),
        },
        Status::Processing => "Tagging & filing…".into(),
        Status::Scanning => "Adding to library…".into(),
        Status::Done => "Done".into(),
        Status::Failed => "Failed".into(),
    }
}

/// Route for a catalog track's album/artist links.
pub fn dz_album_route(t: &DzTrack) -> Option<Route> {
    t.album_id.map(Route::DzAlbum)
}
pub fn dz_artist_route(t: &DzTrack) -> Option<Route> {
    t.artist_id.map(Route::DzArtist)
}
