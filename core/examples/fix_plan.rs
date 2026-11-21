//! Dry run of "Fix library": prints what it would do, changes nothing.
use crate_core::fixer::*;
fn main() -> anyhow::Result<()> {
    let root = crate_core::subsonic::library_dir();
    let dz = crate_core::deezer::Deezer::new();
    let t = std::time::Instant::now();
    let (plan, idx) = plan(&root, &dz, None, Progress(&|what, d, n| if d == 0 { eprintln!("{what} ({n})") }))?;
    println!("took {:?}; {} files", t.elapsed(), plan.files);
    println!("duplicate groups: {} ({} files to move out)", plan.dupes.len(), plan.removed());
    for g in &plan.dupes {
        let fp = |r: &str| idx.files[r].fp.clone().unwrap_or_default();
        println!("  KEEP {}", g.keep);
        for r in &g.remove { println!("   drop {}  (sim {:.2})", r, similarity(&fp(&g.keep), &fp(r))); }
    }
    println!("tag edits: {}", plan.edits.len());
    for e in &plan.edits { println!("  {} -> {}  {:?} {:?} {:?} {:?} {}", e.rel, e.why, e.album_artist, e.album, e.track, e.year, e.cover_from.is_some()); }
    println!("unsure: {:?}", plan.unsure);
    println!("mismatched: {:?}", plan.mismatched);
    // near misses: highest similarity among pairs that weren't grouped
    let mut v: Vec<_> = idx.files.iter().filter(|(_, f)| f.duration > 5.0).collect();
    v.sort_by(|a, b| a.1.duration.total_cmp(&b.1.duration));
    let grouped: std::collections::HashSet<&String> = plan.dupes.iter().flat_map(|g| std::iter::once(&g.keep).chain(g.remove.iter())).collect();
    let mut miss = vec![];
    for i in 0..v.len() { for j in i+1..v.len() {
        if v[j].1.duration - v[i].1.duration > 2.0 { break }
        if grouped.contains(v[i].0) && grouped.contains(v[j].0) { continue }
        let s = similarity(v[i].1.fp.as_deref().unwrap_or(&[]), v[j].1.fp.as_deref().unwrap_or(&[]));
        miss.push((s, v[i].0.clone(), v[j].0.clone()));
    }}
    miss.sort_by(|a, b| b.0.total_cmp(&a.0));
    println!("closest non-duplicates:");
    for m in miss.iter().take(6) { println!("  {:.2} {} | {}", m.0, m.1, m.2); }
    Ok(())
}
