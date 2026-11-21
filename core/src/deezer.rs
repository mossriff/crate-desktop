//! Deezer's free public catalog API: search, albums, artists and full tag data.
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct DzTrack {
    pub id: u64,
    pub title: String,
    pub duration: f64,
    pub preview: Option<String>,
    pub artist: String,
    pub artist_id: Option<u64>,
    pub album: String,
    pub album_id: Option<u64>,
    pub cover: Option<String>,
    pub cover_small: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct DzAlbum {
    pub id: u64,
    pub title: String,
    pub artist: String,
    pub cover: Option<String>,
    pub tracks: Option<u64>,
    #[serde(rename = "type")]
    pub kind: String,
    pub year: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct DzArtist {
    pub id: u64,
    pub name: String,
    pub picture: Option<String>,
    pub fans: u64,
}

/// Everything written into a downloaded file's tags.
#[derive(Clone, Debug, Default)]
pub struct TagMeta {
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub track: Option<u32>,
    pub disc: Option<u32>,
    pub date: String,
    pub genre: String,
    pub cover_url: Option<String>,
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}
fn os(v: &Value) -> Option<String> {
    v.as_str().filter(|x| !x.is_empty()).map(str::to_string)
}

pub fn track_from(t: &Value, album: Option<&Value>) -> DzTrack {
    let al = album.unwrap_or(&t["album"]);
    DzTrack {
        id: t["id"].as_u64().unwrap_or(0),
        title: s(&t["title"]),
        duration: t["duration"].as_f64().unwrap_or(0.0),
        preview: os(&t["preview"]),
        artist: s(&t["artist"]["name"]),
        artist_id: t["artist"]["id"].as_u64(),
        album: s(&al["title"]),
        album_id: al["id"].as_u64(),
        cover: os(&al["cover_medium"]),
        cover_small: os(&al["cover_small"]),
    }
}

pub fn album_from(a: &Value, artist_name: Option<&str>) -> DzAlbum {
    DzAlbum {
        id: a["id"].as_u64().unwrap_or(0),
        title: s(&a["title"]),
        artist: artist_name.map(str::to_string).unwrap_or_else(|| s(&a["artist"]["name"])),
        cover: os(&a["cover_medium"]).or_else(|| os(&a["cover_big"])),
        tracks: a["nb_tracks"].as_u64(),
        kind: s(&a["record_type"]),
        year: s(&a["release_date"]).chars().take(4).collect(),
    }
}

#[derive(Clone)]
pub struct Deezer {
    http: reqwest::blocking::Client,
    cache: Arc<Mutex<HashMap<String, (Instant, Value)>>>,
}

impl Default for Deezer {
    fn default() -> Self {
        Self::new()
    }
}

impl Deezer {
    pub fn new() -> Self {
        Deezer {
            http: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(20))
                .user_agent("crate/1.0")
                .build()
                .expect("http client"),
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn get(&self, path: &str, params: &[(&str, &str)]) -> Result<Value> {
        let key = format!("{path}?{params:?}");
        if let Some((t, v)) = self.cache.lock().unwrap().get(&key) {
            if t.elapsed() < Duration::from_secs(3600) {
                return Ok(v.clone());
            }
        }
        let mut v = Value::Null;
        for _ in 0..3 {
            v = self.http.get(format!("https://api.deezer.com{path}")).query(params).send()?.json()?;
            if v["error"]["code"].as_i64() == Some(4) {
                std::thread::sleep(Duration::from_millis(1500)); // rate limited; back off
                continue;
            }
            break;
        }
        if v.get("error").is_some() {
            bail!("Deezer: {}", v["error"]["message"].as_str().unwrap_or("error"));
        }
        let mut c = self.cache.lock().unwrap();
        if c.len() > 2000 {
            c.clear();
        }
        c.insert(key, (Instant::now(), v.clone()));
        Ok(v)
    }

    fn data(v: &Value) -> Vec<Value> {
        v["data"].as_array().cloned().unwrap_or_default()
    }

    /// Any catalog endpoint, e.g. `/track/isrc:…`.
    pub fn raw(&self, path: &str) -> Result<Value> {
        self.get(path, &[])
    }

    pub fn search_tracks(&self, q: &str) -> Result<Vec<DzTrack>> {
        Ok(Self::data(&self.get("/search", &[("q", q), ("limit", "40")])?).iter().map(|t| track_from(t, None)).collect())
    }

    pub fn search_albums(&self, q: &str) -> Result<Vec<DzAlbum>> {
        Ok(Self::data(&self.get("/search/album", &[("q", q), ("limit", "30")])?).iter().map(|a| album_from(a, None)).collect())
    }

    pub fn search_artists(&self, q: &str) -> Result<Vec<DzArtist>> {
        Ok(Self::data(&self.get("/search/artist", &[("q", q), ("limit", "30")])?)
            .iter()
            .map(|a| DzArtist {
                id: a["id"].as_u64().unwrap_or(0),
                name: s(&a["name"]),
                picture: os(&a["picture_medium"]).or_else(|| os(&a["picture_big"])),
                fans: a["nb_fan"].as_u64().unwrap_or(0),
            })
            .collect())
    }

    pub fn album(&self, id: u64) -> Result<(DzAlbum, Vec<DzTrack>)> {
        let a = self.get(&format!("/album/{id}"), &[])?;
        let tracks = a["tracks"]["data"].as_array().cloned().unwrap_or_default().iter().map(|t| track_from(t, Some(&a))).collect();
        Ok((album_from(&a, None), tracks))
    }

    pub fn artist(&self, id: u64) -> Result<(DzArtist, Vec<DzTrack>, Vec<DzAlbum>)> {
        let a = self.get(&format!("/artist/{id}"), &[])?;
        let name = s(&a["name"]);
        let top = Self::data(&self.get(&format!("/artist/{id}/top"), &[("limit", "10")])?).iter().map(|t| track_from(t, None)).collect();
        let albums = Self::data(&self.get(&format!("/artist/{id}/albums"), &[("limit", "100")])?)
            .iter()
            .map(|x| album_from(x, Some(&name)))
            .collect();
        Ok((
            DzArtist { id, name, picture: os(&a["picture_medium"]).or_else(|| os(&a["picture_big"])), fans: a["nb_fan"].as_u64().unwrap_or(0) },
            top,
            albums,
        ))
    }

    /// Full tag data for a track (contributors, track/disc number, date, genre, big cover).
    pub fn tag_meta(&self, t: &DzTrack) -> Result<TagMeta> {
        let full = self.get(&format!("/track/{}", t.id), &[])?;
        let album = match full["album"]["id"].as_u64() {
            Some(aid) => self.get(&format!("/album/{aid}"), &[])?,
            None => Value::Null,
        };
        let mut artists: Vec<String> = vec![];
        for c in full["contributors"].as_array().cloned().unwrap_or_default() {
            let n = s(&c["name"]);
            if !n.is_empty() && !artists.contains(&n) {
                artists.push(n);
            }
        }
        if artists.is_empty() {
            artists.push(t.artist.clone());
        }
        let genres: Vec<String> = album["genres"]["data"].as_array().cloned().unwrap_or_default().iter().map(|g| s(&g["name"])).collect();
        Ok(TagMeta {
            title: os(&full["title"]).unwrap_or_else(|| t.title.clone()),
            artist: artists.join(" & "),
            album_artist: os(&album["artist"]["name"]).unwrap_or_else(|| t.artist.clone()),
            album: os(&album["title"]).unwrap_or_else(|| t.album.clone()),
            track: full["track_position"].as_u64().map(|n| n as u32),
            disc: full["disk_number"].as_u64().map(|n| n as u32),
            date: os(&full["release_date"]).or_else(|| os(&album["release_date"])).unwrap_or_default().chars().take(10).collect(),
            genre: genres.join(", "),
            cover_url: os(&album["cover_xl"]).or_else(|| os(&album["cover_big"])),
        })
    }
}
