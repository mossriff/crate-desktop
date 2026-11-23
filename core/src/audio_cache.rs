//! Songs are played from files in ~/.cache/crate/audio rather than from memory.
//!
//! The old engine downloaded each song into RAM (10 MB for an MP3, 50+ MB once Opus was decoded)
//! and kept a 48 MB cache on top. Now the download goes straight to disk and the decoder reads
//! the file as it plays, so audio costs almost no memory, and songs played recently start
//! instantly. The folder is trimmed to `LIMIT`, oldest first.
use crate::model::Track;
use crate::subsonic::Client;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

const LIMIT: u64 = 1024 * 1024 * 1024;

pub fn dir() -> PathBuf {
    crate::cache_dir().join("audio")
}

fn safe(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

/// The cached file for a song, if there is one (and mark it as recently used).
pub fn cached(id: &str) -> Option<PathBuf> {
    let base = safe(id);
    for ext in ["a", "flac"] {
        let p = dir().join(format!("{base}.{ext}"));
        if p.exists() {
            if let Ok(f) = std::fs::File::options().append(true).open(&p) {
                let _ = f.set_modified(SystemTime::now());
            }
            return Some(p);
        }
    }
    None
}

/// Whether rodio can play this file as is.
fn decodable(path: &Path) -> bool {
    std::fs::File::open(path)
        .ok()
        .map(|f| crate::decoder(std::io::BufReader::new(f)).is_ok())
        .unwrap_or(false)
}

/// Download a song into the cache (Opus and anything else rodio can't read is converted to
/// FLAC with ffmpeg on the way) and return its path.
pub fn fetch(client: &Client, t: &Track) -> Result<PathBuf, String> {
    if let Some(p) = cached(&t.id) {
        return Ok(p);
    }
    let d = dir();
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    let base = safe(&t.id);
    let tmp = d.join(format!("{base}.part{}", std::process::id()));
    client.stream_to(&t.id, &tmp).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{e:#}")
    })?;
    let needs_ffmpeg = matches!(t.suffix.as_str(), "opus" | "webm" | "weba") || !decodable(&tmp);
    let out = if needs_ffmpeg {
        let out = d.join(format!("{base}.flac"));
        let ok = Command::new("ffmpeg")
            .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"])
            .arg(&tmp)
            .args(["-vn", "-f", "flac"])
            .arg(&out)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        let _ = std::fs::remove_file(&tmp);
        if !ok {
            let _ = std::fs::remove_file(&out);
            return Err("couldn't decode this file (is ffmpeg installed?)".into());
        }
        out
    } else {
        let out = d.join(format!("{base}.a"));
        std::fs::rename(&tmp, &out).map_err(|e| e.to_string())?;
        out
    };
    std::thread::spawn(prune);
    Ok(out)
}

/// Delete the least recently played files until the folder fits in `LIMIT`.
pub fn prune() {
    let Ok(rd) = std::fs::read_dir(dir()) else { return };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort_by_key(|f| f.0);
    for (_, len, p) in files {
        if total <= LIMIT {
            break;
        }
        if std::fs::remove_file(&p).is_ok() {
            total -= len;
        }
    }
}

/// Size of the cache folder in bytes.
pub fn size() -> u64 {
    std::fs::read_dir(dir())
        .map(|rd| rd.filter_map(|e| e.ok()?.metadata().ok()).map(|m| m.len()).sum())
        .unwrap_or(0)
}

pub fn clear() {
    let _ = std::fs::remove_dir_all(dir());
}

#[cfg(test)]
mod tests {
    /// Apple Lossless in .m4a used to crash rodio's decoder (and with it, loading the song)
    /// instead of being sent to ffmpeg.
    #[test]
    fn alac_is_not_decodable_and_does_not_crash() {
        let dir = std::env::temp_dir().join(format!("crate-alac-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("t.m4a");
        let made = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=d=1", "-c:a", "alac"])
            .arg(&f)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if made {
            assert!(!super::decodable(&f));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
