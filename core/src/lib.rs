//! crate-core: everything Crate does that isn't drawing the window.
//! Kept free of any UI framework.
pub mod audio_cache;
pub mod loudness;
pub mod media;
pub mod model;
pub mod player;
pub mod preview;
pub mod subsonic;
pub mod deezer;
pub mod fixer;
pub mod library;
pub mod downloads;
pub mod lyrics;
pub mod romaji;
pub mod settings;
pub mod setup;
pub mod import;

/// Crate's cache folder (~/.cache/crate).
pub fn cache_dir() -> std::path::PathBuf {
    dirs::cache_dir().unwrap_or_else(|| std::path::PathBuf::from(".cache")).join("crate")
}

/// GET a URL (catalog cover art etc.) with a shared client.
pub fn http_get(url: &str) -> anyhow::Result<Vec<u8>> {
    static HTTP: std::sync::OnceLock<reqwest::blocking::Client> = std::sync::OnceLock::new();
    let c = HTTP.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .user_agent("Crate/0.1 (desktop music player)")
            .build()
            .expect("http client")
    });
    Ok(c.get(url).send()?.error_for_status()?.bytes()?.to_vec())
}
