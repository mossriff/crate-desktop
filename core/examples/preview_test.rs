//! Silent check of the preview engine: fetch, decode and play a real catalog preview at volume 0.
use crate_core::deezer::Deezer;
use crate_core::preview::{Preview, PreviewEvents};
use std::sync::mpsc::{channel, Sender};
use std::sync::Mutex;

struct Ev(Mutex<Sender<String>>);
impl PreviewEvents for Ev {
    fn previewing(&self, id: Option<u64>) {
        let _ = self.0.lock().unwrap().send(format!("{:?} previewing {id:?}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() % 100000));
    }
    fn failed(&self, id: u64, e: String) {
        let _ = self.0.lock().unwrap().send(format!("FAILED {id}: {e}"));
    }
}

fn main() {
    let t = Deezer::new().search_tracks("daft punk one more time").unwrap().into_iter().find(|t| t.preview.is_some()).expect("a track with a preview");
    println!("track: {} — preview url present: {}", t.title, t.preview.as_deref().map(|u| !u.is_empty()).unwrap_or(false));
    let (tx, rx) = channel();
    let p = Preview::spawn(Ev(Mutex::new(tx)));
    p.play(t.id, t.preview.as_deref().unwrap(), 0.0);
    std::thread::sleep(std::time::Duration::from_secs(4));
    println!("stop at {:?}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() % 100000);
    p.stop();
    std::thread::sleep(std::time::Duration::from_millis(400));
    while let Ok(m) = rx.try_recv() {
        println!("{m}");
    }
}
