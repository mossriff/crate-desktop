//! Lyrics: time-synced lyrics from LRCLIB (free, open database), falling back to lyrics
//! embedded in the file (via Navidrome). Japanese lines get a romaji reading.
use crate::model::Track;
use crate::romaji;
use crate::subsonic::Client;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LyricLine {
    /// Start time in seconds (None for unsynced lyrics).
    pub time: Option<f64>,
    pub text: String,
    pub romaji: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Lyrics {
    pub synced: bool,
    pub japanese: bool,
    pub source: String,
    pub lines: Vec<LyricLine>,
}

fn cache_path(song_id: &str) -> Option<PathBuf> {
    let safe: String = song_id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    dirs::cache_dir().map(|d| d.join("crate/lyrics").join(format!("{safe}.json")))
}

/// Parse LRC text ("[01:02.34]line"), supporting several timestamps per line.
pub fn parse_lrc(text: &str) -> Vec<LyricLine> {
    let mut out = vec![];
    for raw in text.lines() {
        let mut rest = raw.trim();
        let mut times = vec![];
        while let Some(stripped) = rest.strip_prefix('[') {
            let Some(end) = stripped.find(']') else { break };
            let tag = &stripped[..end];
            let mut parts = tag.splitn(2, ':');
            let (m, s) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            match (m.trim().parse::<f64>(), s.trim().parse::<f64>()) {
                (Ok(m), Ok(s)) => times.push(m * 60.0 + s),
                _ => {} // metadata tag like [ar:...] — skip
            }
            rest = stripped[end + 1..].trim_start();
        }
        for t in times {
            out.push(LyricLine { time: Some(t), text: rest.to_string(), romaji: None });
        }
    }
    out.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
    out
}

fn plain(text: &str) -> Vec<LyricLine> {
    text.lines().map(|l| LyricLine { time: None, text: l.trim_end().to_string(), romaji: None }).collect()
}

fn finish(mut lines: Vec<LyricLine>, synced: bool, source: &str) -> Lyrics {
    // kana anywhere means Japanese (kanji alone could be Chinese, which this would misread)
    let japanese = lines.iter().any(|l| romaji::has_kana(&l.text));
    if japanese {
        for l in lines.iter_mut() {
            if romaji::has_japanese(&l.text) {
                l.romaji = Some(romaji::romanize(&l.text));
            }
        }
    }
    Lyrics { synced, japanese, source: source.into(), lines }
}

fn first_artist(a: &str) -> &str {
    a.split([',', '&', ';', '/']).next().unwrap_or(a).trim()
}

fn http() -> &'static reqwest::blocking::Client {
    static HTTP: std::sync::OnceLock<reqwest::blocking::Client> = std::sync::OnceLock::new();
    HTTP.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent("Crate/0.1 (desktop music player)")
            .build()
            .expect("http client")
    })
}

fn lrclib(t: &Track) -> Result<Option<Lyrics>> {
    let http = http();
    let dur = format!("{}", t.duration.round() as i64);
    let pick = |v: &Value| -> Option<Lyrics> {
        if let Some(s) = v["syncedLyrics"].as_str().filter(|s| !s.trim().is_empty()) {
            let lines = parse_lrc(s);
            if !lines.is_empty() {
                return Some(finish(lines, true, "LRCLIB"));
            }
        }
        v["plainLyrics"].as_str().filter(|s| !s.trim().is_empty()).map(|s| finish(plain(s), false, "LRCLIB"))
    };
    // exact lookups first (with and without album, full and first artist)
    for artist in [t.artist.as_str(), first_artist(&t.artist)] {
        for album in [Some(t.album.as_str()), None] {
            let mut q = vec![("artist_name", artist), ("track_name", t.title.as_str()), ("duration", dur.as_str())];
            if let Some(a) = album {
                q.push(("album_name", a));
            }
            let r = http.get("https://lrclib.net/api/get").query(&q).send()?;
            if r.status().is_success() {
                if let Some(l) = pick(&r.json()?) {
                    return Ok(Some(l));
                }
            }
        }
    }
    // fuzzy search: closest duration, prefer synced
    let r: Value = http.get("https://lrclib.net/api/search").query(&[("track_name", t.title.as_str()), ("artist_name", first_artist(&t.artist))]).send()?.json()?;
    let mut best: Option<(f64, Lyrics)> = None;
    for v in r.as_array().cloned().unwrap_or_default() {
        let dd = (v["duration"].as_f64().unwrap_or(0.0) - t.duration).abs();
        if t.duration > 0.0 && dd > 10.0 {
            continue;
        }
        if let Some(l) = pick(&v) {
            let score = dd - if l.synced { 100.0 } else { 0.0 };
            if best.as_ref().map(|(b, _)| score < *b).unwrap_or(true) {
                best = Some((score, l));
            }
        }
    }
    Ok(best.map(|(_, l)| l))
}

/// Lyrics embedded in the audio file, via Navidrome's OpenSubsonic endpoint.
fn embedded(client: &Client, t: &Track) -> Option<Lyrics> {
    let b = client.call("getLyricsBySongId", &[("id", &t.id)]).ok()?;
    let list = b["lyricsList"]["structuredLyrics"].as_array()?.clone();
    let best = list.iter().find(|l| l["synced"].as_bool() == Some(true)).or(list.first())?;
    let synced = best["synced"].as_bool().unwrap_or(false);
    let lines: Vec<LyricLine> = best["line"]
        .as_array()?
        .iter()
        .map(|l| LyricLine {
            time: if synced { l["start"].as_f64().map(|ms| ms / 1000.0) } else { None },
            text: l["value"].as_str().unwrap_or_default().to_string(),
            romaji: None,
        })
        .collect();
    if lines.iter().all(|l| l.text.trim().is_empty()) {
        return None;
    }
    Some(finish(lines, synced, "embedded"))
}

/// Find lyrics for a library song. Results (including "none found") are cached on disk.
pub fn get(client: &Client, t: &Track) -> Result<Option<Lyrics>> {
    let cache = cache_path(&t.id);
    if let Some(p) = &cache {
        if let Ok(meta) = std::fs::metadata(p) {
            let age = meta.modified().ok().and_then(|m| SystemTime::now().duration_since(m).ok()).unwrap_or_default();
            if let Ok(b) = std::fs::read(p) {
                match serde_json::from_slice::<Option<Lyrics>>(&b) {
                    // romaji is recomputed rather than trusted from the cache, so improvements to
                    // the romanizer reach songs looked up before them
                    Ok(Some(l)) => return Ok(Some(finish(l.lines.into_iter().map(|x| LyricLine { romaji: None, ..x }).collect(), l.synced, &l.source))),
                    // remember misses for 3 days, then look again
                    Ok(None) if age < Duration::from_secs(3 * 86400) => return Ok(None),
                    _ => {}
                }
            }
        }
    }
    let found = match lrclib(t) {
        Ok(Some(l)) => Some(l),
        _ => embedded(client, t),
    };
    if let Some(p) = &cache {
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = std::fs::write(p, serde_json::to_vec(&found).unwrap_or_default());
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lrc() {
        let l = parse_lrc("[ar:x]\n[00:19.85] One two\n[00:20.91][01:20.00] 紅茶\n[00:30.00]");
        assert_eq!(l.len(), 4);
        assert_eq!(l[0].text, "One two");
        assert!((l[1].time.unwrap() - 20.91).abs() < 1e-6);
        assert_eq!(l[3].text, "紅茶");
    }
}
