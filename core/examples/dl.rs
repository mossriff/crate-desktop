//! Download pipeline test into a scratch library: cargo run -p crate-core --example dl -- <scratch-dir>
use crate_core::{deezer::Deezer, downloads::*, subsonic::Client};
use std::{sync::{Arc, Mutex}, time::{Duration, Instant}};
struct Ev(Arc<Mutex<Vec<Job>>>);
impl DownloadEvents for Ev {
    fn changed(&self, jobs: &[Job], l: &Limiter) {
        for j in jobs { println!("  [{:?}] {:?} src={:?} prog={:?} msg={:?} | searches {}/{}", j.track.title, j.status, j.source, j.progress.map(|p| p as u32), j.message, l.used, l.limit); }
        *self.0.lock().unwrap() = jobs.to_vec();
    }
    fn finished(&self, j: &Job) { println!("  FINISHED {:?}", j.track.title); }
}
fn main() {
    let lib = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let (c, dz) = (Client::from_crate_config().unwrap(), Deezer::new());
    let t = dz.search_tracks("daft punk digital love").unwrap().into_iter().next().unwrap();
    println!("catalog: {} - {} ({})", t.artist, t.title, t.album);
    let seen = Arc::new(Mutex::new(vec![]));
    let d = Downloads::start(c, dz, lib, Ev(seen.clone()));
    d.clear_finished();
    d.add(vec![t], None);
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(240) {
        std::thread::sleep(Duration::from_secs(1));
        if seen.lock().unwrap().iter().any(|j| matches!(j.status, Status::Scanning | Status::Done | Status::Failed)) { break; }
    }
}
