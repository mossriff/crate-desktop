//! Navidrome setup in a sandbox: download it into <scratch>, run it there on a spare port, create
//! the first account and sign in, then stop it and delete everything. Never touches the real
//! config (set XDG_CONFIG_HOME to a scratch folder too) and installs no service.
//! usage: XDG_CONFIG_HOME=<scratch>/cfg setup_test <scratch>
use crate_core::setup;
use std::time::Duration;
fn main() -> anyhow::Result<()> {
    let scratch = std::path::PathBuf::from(std::env::args().nth(1).expect("scratch dir"));
    assert!(std::env::var("XDG_CONFIG_HOME").map(|c| c.starts_with(scratch.to_str().unwrap())).unwrap_or(false), "set XDG_CONFIG_HOME inside the scratch dir");
    let (dir, music) = (scratch.join("navidrome"), scratch.join("Music"));
    std::fs::create_dir_all(&music)?;
    let port = 4599;
    let bin = setup::prepare_navidrome(&dir, &music, port)?;
    println!("downloaded {}", bin.display());
    let mut child = std::process::Command::new(&bin).arg("--configfile").arg(dir.join("navidrome.toml")).current_dir(&dir)
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn()?;
    let url = format!("http://127.0.0.1:{port}");
    let r = (|| -> anyhow::Result<()> {
        for _ in 0..60 { if setup::server_alive(&url) { break } std::thread::sleep(Duration::from_millis(500)); }
        assert!(setup::server_alive(&url), "navidrome answers");
        setup::create_admin(&url, "tester", "test-password-1")?;
        assert!(setup::login(&url, "tester", "wrong", &music).is_err(), "wrong password refused");
        setup::login(&url, "tester", "test-password-1", &music)?;
        let c = crate_core::subsonic::Client::from_crate_config()?;
        println!("signed in; playlists: {}", c.playlists()?.len());
        assert_eq!(setup::current_login().map(|l| l.1), Some("tester".into()));
        // Spotify import into the empty sandbox library: playlist made, both songs looked up
        let csv = scratch.join("Two_Songs.csv");
        std::fs::write(&csv, "Track URI,ISRC,Track Name,Album Name,Artist Name(s),Release Date,Duration (ms)\n\
            spotify:track:a,GBDUW0000059,Clocks,A Rush of Blood to the Head,Coldplay,2002,307879\n\
            spotify:track:b,,Digital Love,Discovery,Daft Punk,2001,301373\n")?;
        let idx = crate_core::import::LibraryIndex::load(&c)?;
        let plan = crate_core::import::plan(&c, &idx, crate_core::import::read_csv(&csv)?)?;
        assert_eq!((plan.name.as_str(), plan.have(), plan.missing()), ("Two Songs", 0, 2));
        let dz = crate_core::deezer::Deezer::new();
        let r = crate_core::import::run(&c, &dz, &plan, &|_, _| {})?;
        println!("import: playlist {}, queued {:?}, not found {}", r.playlist_id, r.queued.iter().map(|t| format!("{} — {}", t.artist, t.title)).collect::<Vec<_>>(), r.not_found.len());
        assert!(c.playlists()?.iter().any(|p| p.name == "Two Songs"), "playlist created");
        assert_eq!(r.queued.len(), 2, "both found in the catalog");
        // importing again reuses the playlist instead of making a second one
        let again = crate_core::import::plan(&c, &idx, crate_core::import::read_csv(&csv)?)?;
        assert_eq!(again.existing.as_deref(), Some(r.playlist_id.as_str()));
        Ok(())
    })();
    child.kill()?;
    child.wait()?;
    std::fs::remove_dir_all(&scratch)?;
    r?;
    println!("ok");
    Ok(())
}
