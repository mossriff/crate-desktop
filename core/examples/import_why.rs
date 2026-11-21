//! For each song an import would treat as missing, show the closest library titles (dry run).
use crate_core::import::*;
fn main() -> anyhow::Result<()> {
    let c = crate_core::subsonic::Client::from_crate_config()?;
    let idx = LibraryIndex::load(&c)?;
    let p = plan(&c, &idx, read_csv(std::env::args().nth(1).unwrap().as_ref())?)?;
    for s in p.songs.iter().zip(&p.owned).filter(|(_, o)| o.is_none()).map(|(s, _)| s) {
        let word = s.title.split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() > 2).next().unwrap_or(&s.title);
        let hits: Vec<String> = c.search_songs(word, 5)?.into_iter().map(|t| format!("{} — {} ({:.0}s)", t.artist, t.title, t.duration)).collect();
        println!("{} — {} ({:.0}s)\n    lib: {:?}", s.artist, s.title, s.duration, hits);
    }
    Ok(())
}
