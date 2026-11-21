//! Apply + undo "Fix library" on a scratch copy of three songs (never touches the real library
//! or Navidrome). usage: fix_apply_test <scratch dir>
use crate_core::fixer::*;
use lofty::prelude::*;
use lofty::tag::ItemKey;
use std::path::Path;
fn tags(p: &Path) -> (Option<String>, Option<u32>, usize) {
    let f = lofty::read_from_path(p).unwrap();
    let t = f.primary_tag().unwrap();
    (t.get_string(&ItemKey::AlbumArtist).map(str::to_string), t.track(), t.pictures().len())
}
fn main() -> anyhow::Result<()> {
    let scratch = std::path::PathBuf::from(std::env::args().nth(1).expect("scratch dir"));
    let root = scratch.join("Library");
    let _ = std::fs::remove_dir_all(&scratch);
    let real = crate_core::subsonic::library_dir();
    // any album folder in your library with three or more mp3s that have covers
    let src = find_album(&real).expect("an album folder with 3+ mp3 files");
    let a = root.join("Artist/Alpha/01. One.mp3");
    let c = root.join("Artist/Alpha/03. Three.mp3");
    let b = root.join("Someone Else/Elsewhere/01. One.mp3");
    for (from, to) in [(&src[0], &a), (&src[0], &b), (&src[2], &c)] {
        std::fs::create_dir_all(to.parent().unwrap())?;
        std::fs::copy(from, to)?;
    }
    // B: same audio, different artist spelling + no cover (the worse copy); C: no cover, no track
    for (p, artist) in [(&b, Some("Someone Else")), (&c, None)] {
        let mut f = lofty::read_from_path(p)?;
        let t = f.primary_tag_mut().unwrap();
        t.remove_picture_type(lofty::picture::PictureType::CoverFront);
        while !t.pictures().is_empty() { t.remove_picture(0); }
        if let Some(ar) = artist { t.set_artist(ar.into()); t.insert_text(ItemKey::AlbumArtist, ar.into()); } else { t.remove_track(); }
        t.save_to_path(p, lofty::config::WriteOptions::default())?;
    }
    let before_c = tags(&c);
    let dz = crate_core::deezer::Deezer::new();
    let (plan, idx) = plan(&root, &dz, None, Progress(&|_, _, _| {}))?;
    println!("plan: dupes {:?}", plan.dupes.iter().map(|g| (&g.keep, &g.remove)).collect::<Vec<_>>());
    println!("plan: edits {:?}", plan.edits.iter().map(|e| (&e.rel, &e.why)).collect::<Vec<_>>());
    assert_eq!(plan.dupes.len(), 1, "one duplicate");
    assert_eq!(plan.dupes[0].keep, "Artist/Alpha/01. One.mp3", "keeps the covered copy");
    let rep = apply(&plan, &idx, None, Progress(&|_, _, _| {}))?;
    println!("applied: moved {}, edited {}, failed {:?}", rep.removed, rep.edited, rep.failed);
    assert!(!b.exists(), "duplicate moved out");
    let after_c = tags(&c);
    println!("C before {before_c:?} after {after_c:?}");
    assert_eq!(after_c.1, Some(3), "track number from file name");
    assert_eq!(after_c.2, 1, "cover copied from the album");
    let n = undo(&root, None)?;
    println!("undo restored {n} things");
    assert!(b.exists(), "duplicate back");
    assert_eq!(tags(&c), before_c, "tags back");
    std::fs::remove_dir_all(&scratch)?;
    println!("ok");
    Ok(())
}

fn find_album(dir: &Path) -> Option<Vec<std::path::PathBuf>> {
    let mut files = vec![];
    let mut subdirs = vec![];
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            subdirs.push(p);
        } else if p.extension().is_some_and(|x| x == "mp3") {
            files.push(p);
        }
    }
    files.sort();
    if files.len() >= 3 && files.iter().all(|f| tags(f).2 > 0) {
        return Some(files);
    }
    subdirs.sort();
    subdirs.iter().find_map(|d| find_album(d))
}
