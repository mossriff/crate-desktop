//! Headless engine test (volume 0): cargo run -p crate-core --example smoke
use crate_core::{player::*, subsonic::Client};
use std::{sync::{Arc, Mutex}, thread::sleep, time::Duration};

#[derive(Clone, Default)]
struct Log(Arc<Mutex<(Option<PlayerState>, Option<Progress>)>>);
impl Events for Log {
    fn state(&self, s: &PlayerState) { self.0.lock().unwrap().0 = Some(s.clone()); }
    fn progress(&self, p: &Progress) { self.0.lock().unwrap().1 = Some(p.clone()); }
}
fn main() {
    let c = Client::from_crate_config().unwrap();
    let pls = c.playlists().unwrap();
    let pl = c.playlist(&pls.iter().find(|p| p.song_count >= 3).expect("a playlist with 3+ songs").id).unwrap();
    let mut tracks = pl.tracks[..3].to_vec();
    let log = Log::default();
    let p = Player::spawn(c.clone(), log.clone());
    p.send(Action::SetVolume { value: 0.0 });
    p.send(Action::SetRepeat { mode: Repeat::Off });
    p.send(Action::SetShuffle { on: false });
    let show = |l: &str| { let g = log.0.lock().unwrap(); let (s, pr) = (g.0.clone().unwrap(), g.1.clone());
        println!("{l:28} idx={:?} playing={} loading={} pos={:.1}/{:.0} err={:?} title={:?}", s.index, s.playing, s.loading,
            pr.as_ref().map(|p| p.position).unwrap_or(0.0), pr.as_ref().map(|p| p.duration).unwrap_or(0.0), s.error,
            s.index.and_then(|i| s.queue.get(i)).map(|t| &t.title)); };
    p.send(Action::SetQueue { tracks: tracks.clone(), start: 0 });
    sleep(Duration::from_secs(2)); show("started");
    let dur = tracks[0].duration;
    p.send(Action::Seek { secs: dur - 4.0 }); sleep(Duration::from_secs(1)); show("seeked near end");
    sleep(Duration::from_secs(6)); show("after end (gapless next?)");
    p.send(Action::Pause); sleep(Duration::from_millis(600)); show("paused");
    p.send(Action::Next); sleep(Duration::from_secs(2)); show("next");
    // opus file
    if let Some(t) = c.all_songs().unwrap().into_iter().find(|t| t.suffix == "opus") {
        tracks = vec![t];
        p.send(Action::SetQueue { tracks, start: 0 }); sleep(Duration::from_secs(3)); show("opus file");
    } else { println!("no opus test file found"); }
    p.send(Action::Pause); sleep(Duration::from_millis(300));
}
