//! A size-capped image cache for the whole window.
//!
//! gpui's default keeps every decoded image forever, so scrolling an 800-song playlist or
//! browsing albums kept tens of MB of cover pixels alive for the rest of the session. This keeps
//! the most recently drawn covers up to a byte budget and frees the rest (from RAM and the GPU
//! atlas); a freed cover is decoded again from the disk cache if it comes back on screen.
use gpui::{App, AppContext, Entity, ImageCache, ImageCacheError, ImageCacheItem, RenderImage, Resource, Window};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Images drawn this recently are never freed, even over budget. Freeing one that's still on
/// screen makes it reload, flash, and get freed again next frame (seen with big artist photos
/// in search, and while resizing the window).
const IN_USE: Duration = Duration::from_millis(1500);

/// Decoded pixels kept around. One screen of large album covers is ~12 MB, so this holds about
/// two screens' worth; small list covers (36 KB each) are re-decoded in well under a millisecond.
const BUDGET: usize = 24 * 1024 * 1024;

struct Entry {
    item: ImageCacheItem,
    used: u64,
    used_at: Instant,
    bytes: usize,
}

pub struct LruImages {
    items: HashMap<Resource, Entry>,
    clock: u64,
    bytes: usize,
}

impl LruImages {
    pub fn new(cx: &mut App) -> Entity<Self> {
        cx.new(|_| LruImages { items: HashMap::new(), clock: 0, bytes: 0 })
    }

    fn evict(&mut self, keep: &Resource, window: &mut Window, cx: &mut App) {
        while self.bytes > BUDGET {
            let victim = self
                .items
                .iter()
                .filter(|(k, e)| e.bytes > 0 && *k != keep && e.used_at.elapsed() > IN_USE)
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| k.clone());
            let Some(k) = victim else { break };
            if let Some(e) = self.items.remove(&k) {
                self.bytes -= e.bytes;
                if let Some(Ok(img)) = e.item.get() {
                    cx.drop_image(img, Some(window));
                }
            }
        }
    }
}

fn pixel_bytes(img: &RenderImage) -> usize {
    let s = img.size(0);
    (s.width.0.max(0) as usize) * (s.height.0.max(0) as usize) * 4
}

impl ImageCache for LruImages {
    fn load(&mut self, resource: &Resource, window: &mut Window, cx: &mut App) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        self.clock += 1;
        let clock = self.clock;
        let e = self.items.entry(resource.clone()).or_insert_with(|| Entry { item: ImageCacheItem::new(resource, cx), used: 0, used_at: Instant::now(), bytes: 0 });
        e.used = clock;
        e.used_at = Instant::now();
        let res = e.item.use_image(window);
        if let (Some(Ok(img)), 0) = (&res, e.bytes) {
            e.bytes = pixel_bytes(img).max(1);
            self.bytes += e.bytes;
            if self.bytes > BUDGET {
                self.evict(resource, window, cx);
            }
        }
        res
    }
}
