//! Dry run of the Spotify (Exportify) import: how many songs of each CSV you already have, and
//! what the catalog finds for a few missing ones. Changes nothing. usage: import_plan <csv>...
use crate_core::import::*;
fn main() -> anyhow::Result<()> {
    let c = crate_core::subsonic::Client::from_crate_config()?;
    let dz = crate_core::deezer::Deezer::new();
    let idx = LibraryIndex::load(&c)?;
    for f in std::env::args().skip(1) {
        let p = plan(&c, &idx, read_csv(f.as_ref())?)?;
        println!("{}: {} songs, have {}, missing {}, existing playlist: {}", p.name, p.songs.len(), p.have(), p.missing(), p.existing.is_some());
        for s in p.songs.iter().zip(&p.owned).filter(|(_, o)| o.is_none()).map(|(s, _)| s).take(3) {
            let m = catalog_match(&dz, s);
            println!("   missing: {} — {}  -> catalog: {:?}", s.artist, s.title, m.map(|t| format!("{} — {} ({:.0}s vs {:.0}s)", t.artist, t.title, t.duration, s.duration)));
        }
    }
    Ok(())
}
