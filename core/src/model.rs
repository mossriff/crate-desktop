use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A song in the library (a Navidrome/Subsonic "child" entry).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_id: String,
    pub duration: f64,
    pub cover_art: Option<String>,
    pub suffix: String,
    pub year: Option<i64>,
    pub track: Option<i64>,
    pub created: Option<String>,
    /// ReplayGain from the file's tags (dB, relative to -18 LUFS) and its peak (linear).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rg_gain: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rg_peak: Option<f32>,
}

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_string()
}

impl Track {
    pub fn from_json(v: &Value) -> Self {
        Track {
            id: s(v, "id"),
            title: s(v, "title"),
            artist: v["displayArtist"].as_str().map(str::to_string).unwrap_or_else(|| s(v, "artist")),
            album: s(v, "album"),
            album_id: s(v, "albumId"),
            duration: v["duration"].as_f64().unwrap_or(0.0),
            cover_art: v["coverArt"].as_str().map(str::to_string),
            suffix: s(v, "suffix").to_lowercase(),
            year: v["year"].as_i64(),
            track: v["track"].as_i64(),
            created: v["created"].as_str().map(str::to_string),
            rg_gain: v["replayGain"]["trackGain"].as_f64().map(|x| x as f32),
            rg_peak: v["replayGain"]["trackPeak"].as_f64().map(|x| x as f32).filter(|p| *p > 0.0),
        }
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistSummary {
    pub id: String,
    pub name: String,
    pub song_count: u64,
    pub duration: f64,
    pub cover_art: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub comment: Option<String>,
    pub cover_art: Option<String>,
    pub tracks: Vec<Track>,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct AlbumSummary {
    pub id: String,
    pub name: String,
    pub artist: String,
    pub artist_id: Option<String>,
    pub cover_art: Option<String>,
    pub song_count: u64,
    pub duration: f64,
    pub year: Option<i64>,
    pub created: Option<String>,
}

impl AlbumSummary {
    pub fn from_json(v: &Value) -> Self {
        AlbumSummary {
            id: s(v, "id"),
            name: v["name"].as_str().or(v["title"].as_str()).unwrap_or_default().to_string(),
            artist: v["displayArtist"].as_str().map(str::to_string).unwrap_or_else(|| s(v, "artist")),
            artist_id: v["artistId"].as_str().map(str::to_string),
            cover_art: v["coverArt"].as_str().map(str::to_string),
            song_count: v["songCount"].as_u64().unwrap_or(0),
            duration: v["duration"].as_f64().unwrap_or(0.0),
            year: v["year"].as_i64(),
            created: v["created"].as_str().map(str::to_string),
        }
    }
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ArtistSummary {
    pub id: String,
    pub name: String,
    pub album_count: u64,
    pub cover_art: Option<String>,
}

impl ArtistSummary {
    pub fn from_json(v: &Value) -> Self {
        ArtistSummary {
            id: s(v, "id"),
            name: s(v, "name"),
            album_count: v["albumCount"].as_u64().unwrap_or(0),
            cover_art: v["coverArt"].as_str().map(str::to_string),
        }
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LibrarySearch {
    pub songs: Vec<Track>,
    pub albums: Vec<AlbumSummary>,
    pub artists: Vec<ArtistSummary>,
}
