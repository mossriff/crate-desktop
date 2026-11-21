//! Colours, look-and-feel settings, and small shared formatting helpers.
//!
//! Everything here is read on every frame, so it lives in atomics: changing a setting is just a
//! store plus a redraw.
use gpui::{rgb, rgba, Rgba};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

/// One colour scheme for the window chrome (the accent is separate).
pub struct Palette {
    pub key: &'static str,
    pub name: &'static str,
    pub light: bool,
    bg: u32,
    side: u32,
    panel: u32,
    panel2: u32,
    hover: u32,
    line: u32,
    text: u32,
    dim: u32,
    faint: u32,
    bar: u32,
}

pub const THEMES: [Palette; 4] = [
    Palette { key: "dark", name: "Dark", light: false, bg: 0x0f0f11, side: 0x141417, panel: 0x18181c, panel2: 0x1f1f24, hover: 0x26262c, line: 0x2a2a31, text: 0xededf2, dim: 0xa0a0ab, faint: 0x6c6c78, bar: 0x0b0b0d },
    Palette { key: "oled", name: "OLED black", light: false, bg: 0x000000, side: 0x000000, panel: 0x0e0e10, panel2: 0x16161a, hover: 0x1d1d22, line: 0x202026, text: 0xf2f2f5, dim: 0x9c9ca6, faint: 0x66666f, bar: 0x000000 },
    Palette { key: "dim", name: "Dim", light: false, bg: 0x1d1f24, side: 0x22252b, panel: 0x272a31, panel2: 0x2e3139, hover: 0x353945, line: 0x3a3e48, text: 0xe8eaf0, dim: 0xa4a9b6, faint: 0x737987, bar: 0x191b1f },
    Palette { key: "light", name: "Light", light: true, bg: 0xf7f6f3, side: 0xefede8, panel: 0xffffff, panel2: 0xeae8e2, hover: 0xe2dfd8, line: 0xdcd9d1, text: 0x1d1c1a, dim: 0x5f5c56, faint: 0x8e8a82, bar: 0xefede8 },
];

impl Palette {
    /// (background, sidebar, text) for the little preview tiles in Settings.
    pub fn preview(&self) -> (u32, u32, u32) {
        (self.bg, self.side, self.text)
    }
}

static THEME: AtomicUsize = AtomicUsize::new(0);

fn p() -> &'static Palette {
    &THEMES[THEME.load(Ordering::Relaxed).min(THEMES.len() - 1)]
}

pub fn set_theme(key: &str) {
    if let Some(i) = THEMES.iter().position(|t| t.key == key) {
        THEME.store(i, Ordering::Relaxed);
    }
}
pub fn theme_key() -> &'static str {
    p().key
}
pub fn is_light() -> bool {
    p().light
}

pub fn bg() -> Rgba { rgb(p().bg) }
pub fn side() -> Rgba { rgb(p().side) }
pub fn panel() -> Rgba { rgb(p().panel) }
pub fn panel2() -> Rgba { rgb(p().panel2) }
pub fn hover() -> Rgba { rgb(p().hover) }
pub fn line() -> Rgba { rgb(p().line) }
pub fn text() -> Rgba { rgb(p().text) }
pub fn dim() -> Rgba { rgb(p().dim) }
pub fn faint() -> Rgba { rgb(p().faint) }
pub fn bar() -> Rgba { rgb(p().bar) }
/// Main text colour at some opacity (0–255), e.g. lyrics that are sung / still to come.
pub fn text_a(alpha: u8) -> Rgba { rgba((p().text << 8) | alpha as u32) }
/// Text on a `text()`-coloured background (selected chips, the big play button).
pub fn on_text() -> Rgba { rgb(p().bg) }
/// Dark veil over cover art (hover play buttons) and behind dialogs.
pub fn veil(alpha: u8) -> Rgba { rgba(alpha as u32) }

// ------------------------------------------------------------------ accent

/// Accent colours to pick from in Settings: (setting value, name, colour).
pub const ACCENTS: [(&str, &str, u32); 8] = [
    ("amber", "Amber", 0xe8a33d),
    ("green", "Green", 0x3ecf7a),
    ("teal", "Teal", 0x2ec4b6),
    ("blue", "Blue", 0x5b9dff),
    ("violet", "Violet", 0xa78bfa),
    ("pink", "Pink", 0xff6b9a),
    ("red", "Red", 0xff6257),
    ("lime", "Lime", 0xb5e04b),
];

static ACCENT: AtomicU32 = AtomicU32::new(0xe8a33d);
/// "art": the accent follows the playing song's cover
static FROM_ART: AtomicBool = AtomicBool::new(false);

/// Pick a preset by name, or "art" to follow album covers.
pub fn set_accent(name: &str) {
    FROM_ART.store(name == "art", Ordering::Relaxed);
    if let Some(a) = ACCENTS.iter().find(|a| a.0 == name) {
        ACCENT.store(a.2, Ordering::Relaxed);
    }
}

/// Set the colour directly (used by "from album art").
pub fn set_accent_rgb(c: u32) {
    ACCENT.store(c, Ordering::Relaxed);
}

pub fn accent_from_art() -> bool {
    FROM_ART.load(Ordering::Relaxed)
}

pub fn accent_name() -> &'static str {
    if accent_from_art() {
        return "art";
    }
    let c = ACCENT.load(Ordering::Relaxed);
    ACCENTS.iter().find(|a| a.2 == c).map(|a| a.0).unwrap_or("amber")
}

pub fn accent() -> Rgba { rgb(ACCENT.load(Ordering::Relaxed)) }

fn luminance(c: u32) -> f32 {
    let ch = |sh: u32| ((c >> sh) & 0xff) as f32 / 255.;
    0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0)
}

/// Text/icons drawn on the accent colour: dark on bright accents, white on deep ones.
pub fn accent_ink() -> Rgba {
    if luminance(ACCENT.load(Ordering::Relaxed)) > 0.45 { rgb(0x14100a) } else { rgb(0xffffff) }
}

fn mix(a: u32, b: u32, pct_a: u32) -> u32 {
    let m = |sh: u32| (((a >> sh) & 0xff) * pct_a + ((b >> sh) & 0xff) * (100 - pct_a)) / 100;
    (m(16) << 16) | (m(8) << 8) | m(0)
}

/// A wash of the accent over the background, for the top of page headers.
pub fn accent_wash() -> Rgba {
    rgb(mix(ACCENT.load(Ordering::Relaxed), p().bg, if is_light() { 22 } else { 18 }))
}

/// Highlight for a playlist while songs are dragged over it.
pub fn accent_drop() -> Rgba { rgba((ACCENT.load(Ordering::Relaxed) << 8) | 0x40) }

/// A pleasant accent from a cover image: the most common vivid hue, brightened or darkened to
/// read well on the current theme. None for greyscale covers.
pub fn accent_from_image(img: &image::RgbaImage) -> Option<u32> {
    let mut buckets = [(0f32, 0f32, 0f32, 0f32); 24]; // weight, r, g, b per 15° of hue
    for px in img.pixels() {
        let [r, g, b, a] = px.0;
        if a < 128 {
            continue;
        }
        let (rf, gf, bf) = (r as f32 / 255., g as f32 / 255., b as f32 / 255.);
        let (mx, mn) = (rf.max(gf).max(bf), rf.min(gf).min(bf));
        let (sat, val) = (if mx > 0. { (mx - mn) / mx } else { 0. }, mx);
        if sat < 0.3 || !(0.2..=0.98).contains(&val) {
            continue;
        }
        let h = hue(rf, gf, bf);
        let w = sat * sat;
        let bk = &mut buckets[((h / 15.) as usize).min(23)];
        bk.0 += w;
        bk.1 += rf * w;
        bk.2 += gf * w;
        bk.3 += bf * w;
    }
    let total: f32 = buckets.iter().map(|b| b.0).sum();
    let best = buckets.iter().max_by(|a, b| a.0.total_cmp(&b.0))?;
    if best.0 < 0.02 * img.pixels().len() as f32 || best.0 < total * 0.08 {
        return None;
    }
    let (r, g, b) = (best.1 / best.0, best.2 / best.0, best.3 / best.0);
    // settle on a fixed brightness/saturation so text on it and it on the background both read
    let h = hue(r, g, b);
    let (s, l) = if is_light() { (0.65, 0.42) } else { (0.7, 0.62) };
    Some(hsl(h, s, l))
}

fn hue(r: f32, g: f32, b: f32) -> f32 {
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    let d = mx - mn;
    if d <= 0. {
        return 0.;
    }
    let h = if mx == r { ((g - b) / d).rem_euclid(6.) } else if mx == g { (b - r) / d + 2. } else { (r - g) / d + 4. };
    h * 60.
}

fn hsl(h: f32, s: f32, l: f32) -> u32 {
    let c = (1. - (2. * l - 1.).abs()) * s;
    let x = c * (1. - ((h / 60.).rem_euclid(2.) - 1.).abs());
    let m = l - c / 2.;
    let (r, g, b) = match (h / 60.) as u32 {
        0 => (c, x, 0.),
        1 => (x, c, 0.),
        2 => (0., c, x),
        3 => (0., x, c),
        4 => (x, 0., c),
        _ => (c, 0., x),
    };
    let to = |v: f32| (((v + m) * 255.).round().clamp(0., 255.)) as u32;
    (to(r) << 16) | (to(g) << 8) | to(b)
}

// ------------------------------------------------------------------ size, density, motion

/// Text size, percent of normal.
static TEXT_PCT: AtomicU32 = AtomicU32::new(100);
pub const TEXT_SIZES: [u32; 4] = [90, 100, 110, 120];
pub fn set_text_pct(p: u32) {
    TEXT_PCT.store(p.clamp(80, 130), Ordering::Relaxed);
}
pub fn text_pct() -> u32 {
    TEXT_PCT.load(Ordering::Relaxed)
}

/// Song row height: compact / normal / comfortable.
static ROW_H: AtomicU32 = AtomicU32::new(56);
pub const DENSITIES: [(&str, &str, u32); 3] = [("compact", "Compact", 46), ("normal", "Normal", 56), ("comfy", "Comfortable", 66)];
pub fn set_density(key: &str) {
    if let Some(d) = DENSITIES.iter().find(|d| d.0 == key) {
        ROW_H.store(d.2, Ordering::Relaxed);
    }
}
pub fn density_key() -> &'static str {
    let h = ROW_H.load(Ordering::Relaxed);
    DENSITIES.iter().find(|d| d.2 == h).map(|d| d.0).unwrap_or("normal")
}
pub fn row_h() -> f32 {
    ROW_H.load(Ordering::Relaxed) as f32
}
/// Cover size in song rows (scales with the row).
pub fn row_cover() -> f32 {
    (row_h() - 16.).max(28.)
}

static ANIMATIONS: AtomicBool = AtomicBool::new(true);
pub fn set_animations(on: bool) {
    ANIMATIONS.store(on, Ordering::Relaxed);
}
pub fn animations() -> bool {
    ANIMATIONS.load(Ordering::Relaxed)
}

// ------------------------------------------------------------------ formatting

pub fn fmt_time(s: f64) -> String {
    let s = s.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

pub fn fmt_long(s: f64) -> String {
    let (h, m) = ((s / 3600.0) as u64, ((s % 3600.0) / 60.0).round() as u64);
    if h > 0 { format!("{h} hr {m} min") } else { format!("{m} min") }
}

pub fn songs(n: usize) -> String {
    format!("{n} song{}", if n == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn art_accent() {
        let mut img = image::RgbaImage::new(8, 8);
        for p in img.pixels_mut() {
            *p = image::Rgba([30, 60, 200, 255]); // blue cover
        }
        let c = accent_from_image(&img).unwrap();
        let (r, g, b) = (c >> 16, (c >> 8) & 0xff, c & 0xff);
        assert!(b > r && b > g, "{c:06x}");
        let grey = image::RgbaImage::from_pixel(8, 8, image::Rgba([120, 120, 120, 255]));
        assert!(accent_from_image(&grey).is_none());
    }
}
