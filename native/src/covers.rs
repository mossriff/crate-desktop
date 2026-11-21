//! Album covers: fetched from Navidrome by background threads into a disk cache
//! (~/.cache/crate/covers/<id>_<size>.jpg) and drawn from there.
use crate_core::subsonic::Client;
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

pub const SMALL: u32 = 96;
pub const LARGE: u32 = 300;

/// A cover to fetch: from Navidrome by cover id + size, or a plain URL (catalog art).
#[derive(Clone)]
enum Req {
    Nav(String, u32),
    Url(String),
}

fn url_key(url: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    format!("u{:016x}", h.finish())
}

#[derive(Clone)]
pub struct Covers {
    dir: PathBuf,
    ready: Arc<Mutex<HashSet<String>>>,
    queued: Arc<Mutex<HashSet<String>>>,
    queue: Arc<Mutex<VecDeque<Req>>>,
    /// wakes the fetch threads when something is queued (they sleep otherwise)
    wake: Arc<Condvar>,
    /// Set when new covers arrived, so the UI knows to redraw.
    pub dirty: Arc<AtomicBool>,
}

fn key(id: &str, size: u32) -> String {
    let safe: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    format!("{safe}_{size}")
}

impl Covers {
    pub fn start(client: Client) -> Self {
        let dir = crate_core::cache_dir().join("covers");
        let _ = std::fs::create_dir_all(&dir);
        let ready: HashSet<String> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().map(|x| x == "jpg").unwrap_or(false))
                    .filter_map(|e| e.path().file_stem().map(|s| s.to_string_lossy().to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let c = Covers {
            dir,
            ready: Arc::new(Mutex::new(ready)),
            queued: Default::default(),
            queue: Default::default(),
            wake: Default::default(),
            dirty: Arc::new(AtomicBool::new(false)),
        };
        for _ in 0..4 {
            let (c2, client) = (c.clone(), client.clone());
            std::thread::Builder::new()
                .name("covers".into())
                .spawn(move || loop {
                    // newest request first: that's what is on screen right now
                    let req = {
                        let mut q = c2.queue.lock().unwrap();
                        loop {
                            match q.pop_back() {
                                Some(r) => break r,
                                None => q = c2.wake.wait(q).unwrap(),
                            }
                        }
                    };
                    let (fetched, k) = match &req {
                        Req::Nav(id, size) => (client.cover(id, *size), key(id, *size)),
                        Req::Url(u) => (crate_core::http_get(u), url_key(u)),
                    };
                    if let Ok(bytes) = fetched {
                        let tmp = c2.dir.join(format!("{k}.tmp"));
                        if std::fs::write(&tmp, bytes).is_ok() && std::fs::rename(&tmp, c2.dir.join(format!("{k}.jpg"))).is_ok() {
                            c2.ready.lock().unwrap().insert(k);
                            c2.dirty.store(true, Ordering::Relaxed);
                        }
                    }
                })
                .expect("cover thread");
        }
        c
    }

    /// Queue covers that aren't on screen yet at the lowest priority, so they're already on disk
    /// by the time the user scrolls to them (avoids pop-in and first-scroll hitches).
    pub fn prefetch<'a>(&self, ids: impl IntoIterator<Item = &'a str>, size: u32) {
        let ready = self.ready.lock().unwrap();
        let mut queued = self.queued.lock().unwrap();
        let mut q = self.queue.lock().unwrap();
        for id in ids {
            let k = key(id, size);
            if !ready.contains(&k) && queued.insert(k) {
                q.push_front(Req::Nav(id.to_string(), size));
            }
        }
        self.wake.notify_all();
    }

    /// Path to the cached cover, or None (and a fetch is scheduled).
    pub fn get(&self, id: Option<&str>, size: u32) -> Option<PathBuf> {
        let id = id?;
        let k = key(id, size);
        if self.ready.lock().unwrap().contains(&k) {
            return Some(self.dir.join(format!("{k}.jpg")));
        }
        if self.queued.lock().unwrap().insert(k) {
            self.queue.lock().unwrap().push_back(Req::Nav(id.to_string(), size));
            self.wake.notify_one();
        }
        None
    }

    /// Same as `get`, for an image URL (Deezer covers, artist pictures).
    pub fn get_url(&self, url: Option<&str>) -> Option<PathBuf> {
        let url = url.filter(|u| !u.is_empty())?;
        let k = url_key(url);
        if self.ready.lock().unwrap().contains(&k) {
            return Some(self.dir.join(format!("{k}.jpg")));
        }
        if self.queued.lock().unwrap().insert(k) {
            self.queue.lock().unwrap().push_back(Req::Url(url.to_string()));
            self.wake.notify_one();
        }
        None
    }
}
