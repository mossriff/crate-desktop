//! Cache + measure one MP3 and one Opus song; print timings and the gain they'd get.
use std::time::Instant;
fn main() -> anyhow::Result<()> {
    let c = crate_core::subsonic::Client::from_crate_config()?;
    let songs = c.all_songs()?;
    for suffix in ["mp3", "opus"] {
        let Some(t) = songs.iter().find(|t| t.suffix == suffix) else { continue };
        let t0 = Instant::now();
        let p = crate_core::audio_cache::fetch(&c, t).map_err(anyhow::Error::msg)?;
        let fetched = t0.elapsed();
        let t1 = Instant::now();
        crate_core::loudness::want_soon(&c, [t.clone()]);
        while crate_core::loudness::get(&t.id).is_none() && t1.elapsed().as_secs() < 30 {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        println!("{suffix}: fetch {:?} ({} KB on disk), measure {:?}, {:?}, gain {:+.1} dB",
            fetched, std::fs::metadata(&p)?.len() / 1024, t1.elapsed(), crate_core::loudness::get(&t.id), crate_core::loudness::gain_db(t));
    }
    Ok(())
}
