//! A throwaway Navidrome full of the fake library from native/tools/demo/make-library.py, with
//! some playlists, play history and a queue, for screenshots. Run by native/tools/demo-shots.sh.
//! Keeps running until stdin closes, then stops the server.
//! usage: XDG_CONFIG_HOME=<dir>/cfg XDG_DATA_HOME=<dir>/data demo_server <dir> <music>
use crate_core::model::Track;
use crate_core::{player::PlayerState, setup, subsonic::Client};
use std::time::Duration;

const PORT: u16 = 4598;

const PLAYLISTS: &[(&str, &[&str])] = &[
    ("late night drive", &["Weird Fishes", "Teardrop", "Roads", "Something About Us", "Let Down", "Reckoner", "Jóga", "Kid A", "Nude", "Angel", "Glory Box", "Digital Love", "Lullaby", "Heaven or Las Vegas", "No Surprises"]),
    ("rainy day", &["Skinny Love", "Re: Stacks", "Bunyan", "Sour Times", "Exit Music", "How to Disappear Completely", "Hunter", "Pearly-Dewdrops", "Dissolved Girl", "Videotape"]),
    ("focus", &["Roygbiv", "Turquoise Hexagon Sun", "Aquarius", "Idioteque", "Treefingers", "An Eagle in Your Mind", "Olson", "Rue the Whirl", "Telephasic Workshop"]),
    ("dance in the kitchen", &["One More Time", "Aerodynamic", "Harder, Better", "Nobody", "Once in a Lifetime", "Crosseyed and Painless", "Born Under Punches", "Superheroes", "Face to Face", "Washing Machine Heart"]),
    ("sad girl autumn", &["Nobody", "Two Slow Dancers", "Geyser", "A Pearl", "Blackbird", "Bachelorette", "Unravel", "House of Cards"]),
    ("90s", &["Paranoid Android", "Karma Police", "Angel", "Teardrop", "Wandering Star", "Bachelorette", "Cherry-coloured Funk"]),
];

fn main() -> anyhow::Result<()> {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("demo dir"));
    let music = std::path::PathBuf::from(std::env::args().nth(2).expect("music dir"));
    let cfg = std::env::var("XDG_CONFIG_HOME").unwrap_or_default();
    assert!(cfg.starts_with(dir.to_str().unwrap()), "set XDG_CONFIG_HOME inside the demo dir");
    let nd = dir.join("navidrome");
    let _ = std::fs::remove_dir_all(nd.join("data")); // a fresh server every time
    let bin = setup::prepare_navidrome(&nd, &music, PORT)?;
    let mut child = std::process::Command::new(&bin).arg("--configfile").arg(nd.join("navidrome.toml")).current_dir(&nd)
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn()?;
    let r = fill(&music, &dir);
    if r.is_ok() {
        println!("ready");
        std::io::copy(&mut std::io::stdin(), &mut std::io::sink())?;
    }
    child.kill()?;
    child.wait()?;
    r
}

fn fill(music: &std::path::Path, dir: &std::path::Path) -> anyhow::Result<()> {
    let url = format!("http://127.0.0.1:{PORT}");
    for _ in 0..60 {
        if setup::server_alive(&url) { break }
        std::thread::sleep(Duration::from_millis(500));
    }
    setup::create_admin(&url, "demo", "demo-password")?;
    setup::login(&url, "demo", "demo-password", music)?;
    let c = Client::from_crate_config()?;
    c.start_scan();
    let mut songs = vec![];
    for _ in 0..120 {
        std::thread::sleep(Duration::from_millis(500));
        let s = c.call("getScanStatus", &[])?;
        if s["scanStatus"]["scanning"] == false {
            songs = c.all_songs()?;
            if !songs.is_empty() { break }
        }
    }
    anyhow::ensure!(!songs.is_empty(), "the scan found nothing");
    let find = |t: &str| songs.iter().find(|s| s.title.to_lowercase().starts_with(&t.to_lowercase())).cloned();
    let mut first: Vec<Track> = vec![];
    let mut ids = serde_json::json!({ "playlists": {}, "albums": {} });
    for s in &songs { ids["albums"][&s.album] = s.album_id.clone().into(); }
    for (name, titles) in PLAYLISTS {
        let picked: Vec<Track> = titles.iter().filter_map(|t| find(t)).collect();
        let id = c.create_playlist(name)?;
        c.playlist_add_many(&id, &picked.iter().map(|t| t.id.clone()).collect::<Vec<_>>())?;
        ids["playlists"][*name] = id.clone().into();
        if first.is_empty() { first = picked }
    }
    // play history, so Home has "Jump back in" and "Most played" rows
    let albums = ["In Rainbows", "Dummy", "OK Computer", "Discovery", "Be the Cowboy", "Homogenic", "Kid A", "Heaven or Las Vegas"];
    for (k, a) in albums.iter().enumerate() {
        for s in songs.iter().filter(|s| s.album == *a).take(8 - k) {
            for _ in 0..(albums.len() - k) { c.scrobble(&s.id, true)?; }
        }
    }
    let st = PlayerState { queue: first, index: Some(2), volume: 0.7, muted: true, scrobble: false, ..Default::default() };
    let p = dirs::data_dir().unwrap().join("crate/player.json");
    std::fs::create_dir_all(p.parent().unwrap())?;
    std::fs::write(p, serde_json::to_vec(&st)?)?;
    std::fs::write(dir.join("ids.json"), serde_json::to_vec_pretty(&ids)?)?;
    println!("{} songs, {} playlists", songs.len(), PLAYLISTS.len());
    Ok(())
}
