//! crate-core: everything crate does that isn't drawing the window.
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

/// crate's cache folder (~/.cache/crate).
pub fn cache_dir() -> std::path::PathBuf {
    dirs::cache_dir().unwrap_or_else(|| std::path::PathBuf::from(".cache")).join("crate")
}

/// GET a URL (catalog cover art etc.) with a shared client.
pub fn http_get(url: &str) -> anyhow::Result<Vec<u8>> {
    static HTTP: std::sync::OnceLock<reqwest::blocking::Client> = std::sync::OnceLock::new();
    let c = HTTP.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .user_agent("crate/0.1 (desktop music player)")
            .build()
            .expect("http client")
    });
    Ok(c.get(url).send()?.error_for_status()?.bytes()?.to_vec())
}

/// Open audio for playing. rodio's decoder crashes on some files (Apple Lossless in .m4a, for
/// one) instead of saying it can't read them; this turns that into an error, so the caller can
/// convert the file or skip it.
pub fn decoder<R: std::io::Read + std::io::Seek + Send + Sync + 'static>(r: R) -> Result<rodio::Decoder<R>, String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rodio::Decoder::new(r))) {
        Ok(d) => d.map_err(|e| e.to_string()),
        Err(_) => Err("unsupported audio format".into()),
    }
}
