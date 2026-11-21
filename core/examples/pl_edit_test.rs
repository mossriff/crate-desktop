//! Exercise playlist editing on a throwaway playlist (created and deleted again).
fn main() -> anyhow::Result<()> {
    let c = crate_core::subsonic::Client::from_crate_config()?;
    let songs: Vec<String> = c.search_songs("", 3)?.into_iter().map(|t| t.id).collect();
    assert_eq!(songs.len(), 3);
    let id = c.create_playlist("zz crate edit test")?;
    let r = (|| -> anyhow::Result<()> {
        c.playlist_add_many(&id, &songs)?;
        let rev: Vec<String> = songs.iter().rev().cloned().collect();
        c.playlist_replace(&id, &songs, &rev)?;
        let got: Vec<String> = c.playlist(&id)?.tracks.into_iter().map(|t| t.id).collect();
        assert_eq!(got, rev, "reorder");
        assert!(c.playlist_replace(&id, &songs, &songs).is_err(), "stale expect must fail");
        c.playlist_replace(&id, &rev, &rev[1..])?;
        assert_eq!(c.playlist(&id)?.tracks.len(), 2, "remove");
        c.rename_playlist(&id, "zz crate edit test renamed")?;
        c.set_playlist_comment(&id, "hello")?;
        let p = c.playlist(&id)?;
        assert_eq!(p.name, "zz crate edit test renamed");
        assert_eq!(p.comment.as_deref(), Some("hello"));
        Ok(())
    })();
    c.delete_playlist(&id)?;
    assert!(c.playlists()?.iter().all(|p| p.id != id), "deleted");
    r?;
    println!("ok");
    Ok(())
}
