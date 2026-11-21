//! Minimal Navidrome (Subsonic API) client. The login (a salted token, not the password) is
//! saved in ~/.config/crate/config.json by the setup screens (see `setup`).
use crate::model::{AlbumSummary, ArtistSummary, LibrarySearch, Playlist, PlaylistSummary, Track};
use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

#[derive(Deserialize)]
struct CrateConfig {
    navidrome_url: String,
    library: Option<String>,
    user: Option<String>,
    token: Option<String>,
    salt: Option<String>,
}

/// Where downloaded music is filed (Crate config `library`, default ~/Music/Library).
pub fn library_dir() -> std::path::PathBuf {
    let from_conf = dirs::config_dir()
        .and_then(|d| std::fs::read_to_string(d.join("crate/config.json")).ok())
        .and_then(|raw| serde_json::from_str::<CrateConfig>(&raw).ok())
        .and_then(|c| c.library);
    from_conf.map(Into::into).unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Music/Library"))
}

#[derive(Clone)]
pub struct Client {
    base: String,
    auth: Vec<(String, String)>,
    http: reqwest::blocking::Client,
}

impl Client {
    pub fn from_crate_config() -> Result<Self> {
        let path = dirs::config_dir().context("no config dir")?.join("crate/config.json");
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("couldn't read {} (not set up yet)", path.display()))?;
        let c: CrateConfig = serde_json::from_str(&raw)?;
        let (Some(user), Some(token), Some(salt)) = (c.user, c.token, c.salt) else {
            bail!("not signed in to Navidrome yet");
        };
        let auth = vec![
            ("u".into(), user),
            ("t".into(), token),
            ("s".into(), salt),
            ("v".into(), "1.16.1".into()),
            ("c".into(), "crate-desktop".into()),
        ];
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()?;
        Ok(Client { base: c.navidrome_url.trim_end_matches('/').to_string(), auth, http })
    }

    fn url(&self, method: &str) -> String {
        format!("{}/rest/{}", self.base, method)
    }

    pub fn call(&self, method: &str, params: &[(&str, &str)]) -> Result<Value> {
        let v: Value = self
            .http
            .get(self.url(method))
            .query(&self.auth)
            .query(params)
            .query(&[("f", "json")])
            .send()?
            .json()?;
        let body = v["subsonic-response"].clone();
        if body["status"] != "ok" {
            bail!("Navidrome: {}", body["error"]["message"].as_str().unwrap_or("unknown error"));
        }
        Ok(body)
    }

    pub fn playlists(&self) -> Result<Vec<PlaylistSummary>> {
        let b = self.call("getPlaylists", &[])?;
        let empty = vec![];
        Ok(b["playlists"]["playlist"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .map(|p| PlaylistSummary {
                id: p["id"].as_str().unwrap_or_default().into(),
                name: p["name"].as_str().unwrap_or_default().into(),
                song_count: p["songCount"].as_u64().unwrap_or(0),
                duration: p["duration"].as_f64().unwrap_or(0.0),
                cover_art: p["coverArt"].as_str().map(str::to_string),
            })
            .collect())
    }

    pub fn playlist(&self, id: &str) -> Result<Playlist> {
        let b = self.call("getPlaylist", &[("id", id)])?;
        let p = &b["playlist"];
        let empty = vec![];
        Ok(Playlist {
            id: p["id"].as_str().unwrap_or_default().into(),
            name: p["name"].as_str().unwrap_or_default().into(),
            comment: p["comment"].as_str().map(str::to_string),
            cover_art: p["coverArt"].as_str().map(str::to_string),
            tracks: p["entry"].as_array().unwrap_or(&empty).iter().map(Track::from_json).collect(),
        })
    }

    pub fn search_songs(&self, query: &str, count: u32) -> Result<Vec<Track>> {
        let n = count.to_string();
        let b = self.call("search3", &[("query", query), ("songCount", &n), ("albumCount", "0"), ("artistCount", "0")])?;
        let empty = vec![];
        Ok(b["searchResult3"]["song"].as_array().unwrap_or(&empty).iter().map(Track::from_json).collect())
    }

    fn arr(v: &Value) -> Vec<Value> {
        v.as_array().cloned().unwrap_or_default()
    }

    /// Every song in the library (paged through search3 with an empty query).
    pub fn all_songs(&self) -> Result<Vec<Track>> {
        Ok(self.all_songs_raw()?.iter().map(Track::from_json).collect())
    }

    /// Every song as Navidrome describes it (all fields, e.g. `isrc`).
    pub fn all_songs_raw(&self) -> Result<Vec<Value>> {
        let mut out = vec![];
        loop {
            let off = out.len().to_string();
            let b = self.call("search3", &[("query", ""), ("songCount", "500"), ("songOffset", &off), ("albumCount", "0"), ("artistCount", "0")])?;
            let page = Self::arr(&b["searchResult3"]["song"]);
            let n = page.len();
            out.extend(page);
            if n < 500 {
                break;
            }
        }
        Ok(out)
    }

    /// kind: alphabeticalByName | newest | recent | frequent | random | alphabeticalByArtist
    pub fn albums(&self, kind: &str, size: u32, offset: u32) -> Result<Vec<AlbumSummary>> {
        let (sz, off) = (size.to_string(), offset.to_string());
        let b = self.call("getAlbumList2", &[("type", kind), ("size", &sz), ("offset", &off)])?;
        Ok(Self::arr(&b["albumList2"]["album"]).iter().map(AlbumSummary::from_json).collect())
    }

    pub fn all_albums(&self) -> Result<Vec<AlbumSummary>> {
        let mut out = vec![];
        loop {
            let page = self.albums("alphabeticalByName", 500, out.len() as u32)?;
            let n = page.len();
            out.extend(page);
            if n < 500 {
                break;
            }
        }
        Ok(out)
    }

    pub fn album(&self, id: &str) -> Result<(AlbumSummary, Vec<Track>)> {
        let b = self.call("getAlbum", &[("id", id)])?;
        let a = &b["album"];
        Ok((AlbumSummary::from_json(a), Self::arr(&a["song"]).iter().map(Track::from_json).collect()))
    }

    pub fn artists(&self) -> Result<Vec<ArtistSummary>> {
        let b = self.call("getArtists", &[])?;
        let mut out = vec![];
        for idx in Self::arr(&b["artists"]["index"]) {
            out.extend(Self::arr(&idx["artist"]).iter().map(ArtistSummary::from_json));
        }
        Ok(out)
    }

    pub fn artist(&self, id: &str) -> Result<(ArtistSummary, Vec<AlbumSummary>)> {
        let b = self.call("getArtist", &[("id", id)])?;
        let a = &b["artist"];
        Ok((ArtistSummary::from_json(a), Self::arr(&a["album"]).iter().map(AlbumSummary::from_json).collect()))
    }

    pub fn search_library(&self, q: &str) -> Result<LibrarySearch> {
        let b = self.call("search3", &[("query", q), ("songCount", "50"), ("albumCount", "20"), ("artistCount", "12")])?;
        let r = &b["searchResult3"];
        Ok(LibrarySearch {
            songs: Self::arr(&r["song"]).iter().map(Track::from_json).collect(),
            albums: Self::arr(&r["album"]).iter().map(AlbumSummary::from_json).collect(),
            artists: Self::arr(&r["artist"]).iter().map(ArtistSummary::from_json).collect(),
        })
    }

    /// Add several library songs to a playlist, skipping ones already in it.
    pub fn playlist_add_many(&self, playlist_id: &str, song_ids: &[String]) -> Result<usize> {
        let have: std::collections::HashSet<String> = self.playlist(playlist_id)?.tracks.into_iter().map(|t| t.id).collect();
        let new: Vec<&String> = song_ids.iter().filter(|id| !have.contains(*id)).collect();
        for chunk in new.chunks(100) {
            let mut q: Vec<(&str, &str)> = vec![("playlistId", playlist_id)];
            for id in chunk {
                q.push(("songIdToAdd", id.as_str()));
            }
            self.call("updatePlaylist", &q)?;
        }
        Ok(new.len())
    }

    /// Add a song to a playlist unless it's already in it.
    pub fn playlist_add(&self, playlist_id: &str, song_id: &str) -> Result<()> {
        let p = self.playlist(playlist_id)?;
        if p.tracks.iter().any(|t| t.id == song_id) {
            return Ok(());
        }
        self.call("updatePlaylist", &[("playlistId", playlist_id), ("songIdToAdd", song_id)])?;
        Ok(())
    }

    /// Same as `call` but sends the parameters as a form POST (OpenSubsonic `formPost`), for
    /// requests too long for a URL, like rewriting an 800-song playlist.
    pub fn call_post(&self, method: &str, params: &[(&str, &str)]) -> Result<Value> {
        let mut form: Vec<(&str, &str)> = self.auth.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        form.extend_from_slice(params);
        form.push(("f", "json"));
        let v: Value = self.http.post(self.url(method)).form(&form).send()?.json()?;
        let body = v["subsonic-response"].clone();
        if body["status"] != "ok" {
            bail!("Navidrome: {}", body["error"]["message"].as_str().unwrap_or("unknown error"));
        }
        Ok(body)
    }

    pub fn rename_playlist(&self, id: &str, name: &str) -> Result<()> {
        self.call_post("updatePlaylist", &[("playlistId", id), ("name", name)])?;
        Ok(())
    }

    pub fn set_playlist_comment(&self, id: &str, comment: &str) -> Result<()> {
        self.call_post("updatePlaylist", &[("playlistId", id), ("comment", comment)])?;
        Ok(())
    }

    pub fn delete_playlist(&self, id: &str) -> Result<()> {
        self.call("deletePlaylist", &[("id", id)])?;
        Ok(())
    }

    /// Replace a playlist's songs with `song_ids` (in that order), but only if it still holds
    /// exactly `expect` — so a song a download added a moment ago is never silently dropped.
    pub fn playlist_replace(&self, id: &str, expect: &[String], song_ids: &[String]) -> Result<()> {
        let now: Vec<String> = self.playlist(id)?.tracks.into_iter().map(|t| t.id).collect();
        if now != expect {
            bail!("the playlist changed in the meantime, try again");
        }
        let mut q: Vec<(&str, &str)> = vec![("playlistId", id)];
        for s in song_ids {
            q.push(("songId", s.as_str()));
        }
        self.call_post("createPlaylist", &q)?;
        Ok(())
    }

    pub fn create_playlist(&self, name: &str) -> Result<String> {
        let b = self.call("createPlaylist", &[("name", name)])?;
        Ok(b["playlist"]["id"].as_str().unwrap_or_default().to_string())
    }

    /// Ask Navidrome to rescan (needs admin; its file watcher usually catches new files anyway).
    pub fn start_scan(&self) {
        let _ = self.call("startScan", &[]);
    }

    /// Original file, written to `path` as it downloads (never held in memory whole).
    pub fn stream_to(&self, id: &str, path: &std::path::Path) -> Result<()> {
        let mut r = self
            .http
            .get(self.url("stream"))
            .query(&self.auth)
            .query(&[("id", id), ("format", "raw")])
            .send()?;
        if !r.status().is_success() {
            return Err(anyhow!("stream failed: HTTP {}", r.status()));
        }
        let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
        r.copy_to(&mut f)?;
        use std::io::Write;
        f.flush()?;
        Ok(())
    }

    /// Tell Navidrome a song is playing (`submission` false) or was listened to (true), for
    /// play counts, "recently played" and anything scrobbling on from Navidrome (Last.fm etc.).
    pub fn scrobble(&self, id: &str, submission: bool) -> Result<()> {
        let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0).to_string();
        self.call("scrobble", &[("id", id), ("submission", if submission { "true" } else { "false" }), ("time", &ms)])?;
        Ok(())
    }

    /// Original file bytes (no transcoding).
    pub fn stream(&self, id: &str) -> Result<Vec<u8>> {
        let r = self
            .http
            .get(self.url("stream"))
            .query(&self.auth)
            .query(&[("id", id), ("format", "raw")])
            .send()?;
        if !r.status().is_success() {
            return Err(anyhow!("stream failed: HTTP {}", r.status()));
        }
        Ok(r.bytes()?.to_vec())
    }

    pub fn cover(&self, id: &str, size: u32) -> Result<Vec<u8>> {
        let r = self
            .http
            .get(self.url("getCoverArt"))
            .query(&self.auth)
            .query(&[("id", id), ("size", &size.to_string())])
            .send()?;
        if !r.status().is_success() {
            bail!("cover failed: HTTP {}", r.status());
        }
        Ok(r.bytes()?.to_vec())
    }

    /// Cover URL prefix for the UI (append `&id=..&size=..`). Only used inside the app window.
    pub fn cover_base(&self) -> String {
        reqwest::Url::parse_with_params(&self.url("getCoverArt"), &self.auth)
            .map(|u| u.to_string())
            .unwrap_or_default()
    }
}
