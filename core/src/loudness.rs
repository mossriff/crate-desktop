//! Volume normalization: how loud each song really is (EBU R128), so every song plays at about
//! the same loudness instead of you reaching for the volume between a quiet 70s track and a
//! loud modern master.
//!
//! Most of the library has no ReplayGain tags, so songs are measured with ffmpeg on one
//! background thread at the lowest CPU priority: upcoming songs first, then the rest of the
//! library little by little. Results live in ~/.cache/crate/loudness.json.
use crate::model::Track;
use crate::subsonic::Client;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{Condvar, Mutex, OnceLock};

/// Loudness everything is brought to (the usual streaming level).
pub const TARGET_LUFS: f32 = -14.0;
/// Never boost a song so far that its peaks would clip; keep this much room (dB).
const HEADROOM: f32 = 1.0;

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize, Debug)]
pub struct Measure {
    /// integrated loudness, LUFS
    pub lufs: f32,
    /// true peak, dBFS
    pub peak: f32,
}

struct State {
    known: HashMap<String, Measure>,
    /// songs that couldn't be measured (don't retry every launch)
    failed: HashSet<String>,
    urgent: VecDeque<Track>,
    background: VecDeque<Track>,
    queued: HashSet<String>,
    started: bool,
    dirty: usize,
}

fn store() -> &'static (Mutex<State>, Condvar) {
    static S: OnceLock<(Mutex<State>, Condvar)> = OnceLock::new();
    S.get_or_init(|| {
        let known = std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let st = State { known, failed: HashSet::new(), urgent: VecDeque::new(), background: VecDeque::new(), queued: HashSet::new(), started: false, dirty: 0 };
        (Mutex::new(st), Condvar::new())
    })
}

fn path() -> std::path::PathBuf {
    crate::cache_dir().join("loudness.json")
}

pub fn get(id: &str) -> Option<Measure> {
    store().0.lock().unwrap().known.get(id).copied()
}

/// Gain to apply to a song, in dB (0 when nothing is known about it).
pub fn gain_db(t: &Track) -> f32 {
    let (want, peak) = match get(&t.id) {
        Some(m) => (TARGET_LUFS - m.lufs, m.peak),
        // ReplayGain is relative to -18 LUFS
        None => match t.rg_gain {
            Some(g) => (g + (TARGET_LUFS + 18.0), t.rg_peak.map(|p| 20.0 * p.log10()).unwrap_or(-HEADROOM)),
            None => return 0.0,
        },
    };
    want.min(-peak - HEADROOM).clamp(-20.0, 12.0)
}

/// Measure these soon (they're about to play).
pub fn want_soon(client: &Client, tracks: impl IntoIterator<Item = Track>) {
    enqueue(client, tracks, true);
}

/// Measure these whenever there's nothing more pressing.
pub fn want_eventually(client: &Client, tracks: impl IntoIterator<Item = Track>) {
    enqueue(client, tracks, false);
}

fn enqueue(client: &Client, tracks: impl IntoIterator<Item = Track>, urgent: bool) {
    let (m, cv) = store();
    let mut st = m.lock().unwrap();
    for t in tracks {
        if st.known.contains_key(&t.id) || st.failed.contains(&t.id) || t.id.is_empty() {
            continue;
        }
        if urgent {
            // jump the queue even if it's already waiting in the background list
            st.urgent.push_back(t.clone());
            st.queued.insert(t.id);
        } else if st.queued.insert(t.id.clone()) {
            st.background.push_back(t);
        }
    }
    if !st.started {
        st.started = true;
        let c = client.clone();
        std::thread::Builder::new().name("loudness".into()).spawn(move || worker(c)).expect("loudness thread");
    }
    cv.notify_one();
}

fn worker(client: Client) {
    let (m, cv) = store();
    loop {
        let t = {
            let mut st = m.lock().unwrap();
            loop {
                let next = st.urgent.pop_front().or_else(|| st.background.pop_front());
                match next {
                    Some(t) if st.known.contains_key(&t.id) => continue,
                    Some(t) => break t,
                    None => {
                        save(&mut st);
                        st = cv.wait(st).unwrap();
                    }
                }
            }
        };
        let r = measure(&client, &t);
        let mut st = m.lock().unwrap();
        st.queued.remove(&t.id);
        match r {
            Some(x) => {
                st.known.insert(t.id, x);
                st.dirty += 1;
                if st.dirty >= 10 {
                    save(&mut st);
                }
            }
            None => {
                st.failed.insert(t.id);
            }
        }
    }
}

fn save(st: &mut State) {
    if st.dirty == 0 {
        return;
    }
    st.dirty = 0;
    if let Ok(b) = serde_json::to_vec(&st.known) {
        let p = path();
        let tmp = p.with_extension("json.tmp");
        if std::fs::write(&tmp, b).is_ok() {
            let _ = std::fs::rename(&tmp, p);
        }
    }
}

/// Run ffmpeg's EBU R128 meter over the song (from the audio cache if it's there, otherwise
/// streamed from Navidrome through a pipe), at the lowest CPU priority.
fn measure(client: &Client, t: &Track) -> Option<Measure> {
    let cached = crate::audio_cache::cached(&t.id);
    let mut cmd = Command::new("nice");
    cmd.args(["-n", "19", "ffmpeg", "-nostdin", "-hide_banner", "-nostats", "-threads", "1", "-i"]);
    match &cached {
        Some(p) => cmd.arg(p),
        None => cmd.arg("pipe:0"),
    };
    cmd.args(["-vn", "-af", "ebur128=peak=true", "-f", "null", "-"])
        .stdin(if cached.is_some() { Stdio::null() } else { Stdio::piped() })
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().ok()?;
    let feeder = if cached.is_none() {
        let mut stdin = child.stdin.take()?;
        let (c, id) = (client.clone(), t.id.clone());
        Some(std::thread::spawn(move || {
            if let Ok(b) = c.stream(&id) {
                let _ = stdin.write_all(&b);
            }
        }))
    } else {
        None
    };
    let out = child.wait_with_output().ok()?;
    if let Some(f) = feeder {
        let _ = f.join();
    }
    parse(&String::from_utf8_lossy(&out.stderr))
}

/// Pull "I: -9.6 LUFS" and the true "Peak: 0.4 dBFS" out of ffmpeg's summary.
fn parse(log: &str) -> Option<Measure> {
    let summary = &log[log.rfind("Summary:")?..];
    let num = |key: &str| -> Option<f32> {
        let line = summary.lines().find(|l| l.trim_start().starts_with(key))?;
        line.split_whitespace().nth(1)?.parse().ok()
    };
    let lufs = num("I:")?;
    let peak = num("Peak:").unwrap_or(0.0);
    (lufs.is_finite() && lufs > -70.0).then_some(Measure { lufs, peak })
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_summary() {
        let log = "[Parsed_ebur128_0 @ 0x1] Summary:\n\n  Integrated loudness:\n    I:          -9.6 LUFS\n    Threshold: -19.8 LUFS\n\n  Loudness range:\n    LRA:         4.1 LU\n\n  True peak:\n    Peak:        0.4 dBFS\n";
        let m = super::parse(log).unwrap();
        assert_eq!((m.lufs, m.peak), (-9.6, 0.4));
    }
}

/// (songs measured, songs waiting to be measured)
pub fn progress() -> (usize, usize) {
    let st = store().0.lock().unwrap();
    (st.known.len(), st.urgent.len() + st.background.len())
}
