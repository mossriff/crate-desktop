//! Playlist editing: rename, description, delete, reorder and remove songs.
//!
//! Edits show up instantly (the cached copy changes first) and are saved in the background.
//! If saving fails, the playlist is reloaded from Navidrome so the screen never lies. The last
//! edit can be undone with Ctrl+Z, deleting a playlist included.
use crate::model::{Model, Route};
use crate_core::model::Track;
use gpui::Context;
use std::sync::{Arc, Mutex};

/// Navidrome loses one of two playlist updates that arrive at the same moment (each reads the
/// playlist, changes its part and writes the whole thing back), so ours go one at a time.
static WRITE: Mutex<()> = Mutex::new(());

fn one_at_a_time<T>(f: impl FnOnce() -> T) -> T {
    let _g = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    f()
}

pub enum Undo {
    /// songs of `pid` were `before`; they are now `after` (ids), unless something else changed them
    Tracks { pid: String, before: Vec<Track>, after: Vec<String> },
    Deleted { name: String, comment: Option<String>, ids: Vec<String> },
}

fn ids(t: &[Track]) -> Vec<String> {
    t.iter().map(|t| t.id.clone()).collect()
}

fn songs(n: usize) -> String {
    if n == 1 { "1 song".into() } else { format!("{n} songs") }
}

impl Model {
    pub fn playlist_name_of(&self, pid: &str) -> String {
        self.playlists.iter().find(|p| p.id == pid).map(|p| p.name.clone()).unwrap_or_else(|| "playlist".into())
    }

    /// Replace a playlist's songs (shown at once, saved in the background).
    fn set_playlist_tracks(&mut self, pid: String, tracks: Vec<Track>, msg: String, undoable: bool, cx: &mut Context<Self>) {
        let Some(old) = self.cache.get(&pid).cloned() else { return };
        let expect = ids(&old.tracks);
        let new_ids = ids(&tracks);
        if new_ids == expect {
            return;
        }
        if let Some(s) = self.playlists.iter_mut().find(|p| p.id == pid) {
            s.song_count = tracks.len() as u64;
            s.duration = tracks.iter().map(|t| t.duration).sum();
        }
        let mut p = (*old).clone();
        p.tracks = tracks;
        self.cache.insert(pid.clone(), Arc::new(p));
        if undoable {
            self.undo = Some(Undo::Tracks { pid: pid.clone(), before: old.tracks.clone(), after: new_ids.clone() });
            self.toast(format!("{msg} \u{00b7} Ctrl+Z to undo"), false, cx);
        } else {
            self.toast(msg, false, cx);
        }
        let c = self.client.clone();
        cx.spawn(async move |this, cx| {
            let p2 = pid.clone();
            let r = cx.background_executor().spawn(async move { one_at_a_time(|| c.playlist_replace(&p2, &expect, &new_ids)) }).await;
            let _ = this.update(cx, |m, cx| {
                if let Err(e) = r {
                    m.undo = None;
                    m.toast(format!("Couldn't save the playlist: {e:#}"), true, cx);
                    m.load_list(pid, cx);
                }
                // the collage cover may have changed
                m.reload_playlists(cx);
            });
        })
        .detach();
    }

    /// Remove songs by their index in the playlist.
    pub fn remove_from_playlist(&mut self, pid: String, idx: &[usize], cx: &mut Context<Self>) {
        let Some(pl) = self.cache.get(&pid).cloned() else { return };
        let gone: std::collections::HashSet<usize> = idx.iter().copied().collect();
        let keep: Vec<Track> = pl.tracks.iter().enumerate().filter(|(i, _)| !gone.contains(i)).map(|(_, t)| t.clone()).collect();
        let n = pl.tracks.len() - keep.len();
        let what = if n == 1 {
            let t = idx.first().and_then(|&i| pl.tracks.get(i)).map(|t| t.title.clone()).unwrap_or_default();
            format!("Removed \u{201c}{t}\u{201d}")
        } else {
            format!("Removed {}", songs(n))
        };
        self.set_playlist_tracks(pid, keep, what, true, cx);
    }

    /// Move songs (indices into the playlist) next to `target`: after it when moving down,
    /// before it when moving up. Returns the moved songs' new indices.
    pub fn move_in_playlist(&mut self, pid: String, moved: &[usize], target: usize, cx: &mut Context<Self>) -> Option<Vec<usize>> {
        let pl = self.cache.get(&pid).cloned()?;
        let order = move_order(pl.tracks.len(), moved, target)?;
        let tracks: Vec<Track> = order.iter().map(|&k| pl.tracks[k].clone()).collect();
        let mut set: Vec<usize> = moved.to_vec();
        set.sort_unstable();
        set.dedup();
        let new_idx = order.iter().enumerate().filter(|(_, k)| set.binary_search(k).is_ok()).map(|(i, _)| i).collect();
        let what = if set.len() == 1 { format!("Moved \u{201c}{}\u{201d}", pl.tracks[set[0]].title) } else { format!("Moved {}", songs(set.len())) };
        self.set_playlist_tracks(pid, tracks, what, true, cx);
        Some(new_idx)
    }

    pub fn rename_playlist(&mut self, pid: String, name: String, cx: &mut Context<Self>) {
        let name = name.trim().to_string();
        if name.is_empty() || self.playlist_name_of(&pid) == name {
            return;
        }
        if let Some(s) = self.playlists.iter_mut().find(|p| p.id == pid) {
            s.name = name.clone();
        }
        if let Some(p) = self.cache.get(&pid).cloned() {
            let mut p = (*p).clone();
            p.name = name.clone();
            self.cache.insert(pid.clone(), Arc::new(p));
        }
        cx.notify();
        self.save_meta(pid, name, true, cx);
    }

    pub fn set_playlist_comment(&mut self, pid: String, comment: String, cx: &mut Context<Self>) {
        let comment = comment.trim().to_string();
        if let Some(p) = self.cache.get(&pid).cloned() {
            if p.comment.as_deref().unwrap_or("") == comment {
                return;
            }
            let mut p = (*p).clone();
            p.comment = (!comment.is_empty()).then(|| comment.clone());
            self.cache.insert(pid.clone(), Arc::new(p));
        }
        cx.notify();
        self.save_meta(pid, comment, false, cx);
    }

    fn save_meta(&mut self, pid: String, value: String, is_name: bool, cx: &mut Context<Self>) {
        let c = self.client.clone();
        cx.spawn(async move |this, cx| {
            let p2 = pid.clone();
            let r = cx
                .background_executor()
                .spawn(async move { one_at_a_time(|| if is_name { c.rename_playlist(&p2, &value) } else { c.set_playlist_comment(&p2, &value) }) })
                .await;
            let _ = this.update(cx, |m, cx| {
                if let Err(e) = r {
                    m.toast(format!("Couldn't save: {e:#}"), true, cx);
                    m.reload_playlists(cx);
                    m.load_list(pid, cx);
                }
            });
        })
        .detach();
    }

    pub fn delete_playlist(&mut self, pid: String, cx: &mut Context<Self>) {
        let name = self.playlist_name_of(&pid);
        let cached = self.cache.get(&pid).cloned();
        let c = self.client.clone();
        cx.spawn(async move |this, cx| {
            let p2 = pid.clone();
            // keep what's needed to undo: the songs (fetched fresh if not loaded) and description
            let r = cx
                .background_executor()
                .spawn(async move {
                    let pl = match cached {
                        Some(p) => (*p).clone(),
                        None => c.playlist(&p2)?,
                    };
                    one_at_a_time(|| c.delete_playlist(&p2))?;
                    anyhow::Ok(pl)
                })
                .await;
            let _ = this.update(cx, |m, cx| match r {
                Ok(pl) => {
                    m.playlists.retain(|p| p.id != pid);
                    m.cache.remove(&pid);
                    let gone = Route::Playlist(pid.clone());
                    m.history.retain(|r| *r != gone);
                    m.future.retain(|r| *r != gone);
                    if m.route == gone {
                        m.route = Route::Home;
                        m.after_route_pub(cx);
                    }
                    if m.queue_source.as_deref() == Some(pid.as_str()) {
                        m.queue_source = None;
                    }
                    m.undo = Some(Undo::Deleted { name: pl.name, comment: pl.comment, ids: ids(&pl.tracks) });
                    m.toast(format!("Deleted \u{201c}{name}\u{201d} \u{00b7} Ctrl+Z to undo"), false, cx);
                }
                Err(e) => m.toast(format!("Couldn't delete: {e:#}"), true, cx),
            });
        })
        .detach();
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        match self.undo.take() {
            None => {}
            Some(Undo::Tracks { pid, before, after }) => {
                let now = self.cache.get(&pid).map(|p| ids(&p.tracks));
                if now.as_ref() != Some(&after) {
                    return self.toast("Can't undo: the playlist has changed since", true, cx);
                }
                self.set_playlist_tracks(pid, before, "Undone".into(), false, cx);
            }
            Some(Undo::Deleted { name, comment, ids }) => {
                let c = self.client.clone();
                cx.spawn(async move |this, cx| {
                    let n2 = name.clone();
                    let r = cx
                        .background_executor()
                        .spawn(async move {
                            let id = c.create_playlist(&n2)?;
                            if !ids.is_empty() {
                                c.playlist_replace(&id, &[], &ids)?;
                            }
                            if let Some(cm) = comment.filter(|s| !s.is_empty()) {
                                c.set_playlist_comment(&id, &cm)?;
                            }
                            anyhow::Ok(id)
                        })
                        .await;
                    let _ = this.update(cx, |m, cx| match r {
                        Ok(id) => {
                            m.toast(format!("Restored \u{201c}{name}\u{201d}"), false, cx);
                            m.reload_playlists(cx);
                            m.go(Route::Playlist(id), cx);
                        }
                        Err(e) => m.toast(format!("Couldn't restore: {e:#}"), true, cx),
                    });
                })
                .detach();
            }
        }
    }
}

/// New order (as old indices) after moving `moved` next to `target`. None if nothing moves.
pub fn move_order(len: usize, moved: &[usize], target: usize) -> Option<Vec<usize>> {
    let mut set: Vec<usize> = moved.iter().copied().filter(|&k| k < len).collect();
    set.sort_unstable();
    set.dedup();
    if set.is_empty() || target >= len || set.binary_search(&target).is_ok() {
        return None;
    }
    let down = target > set[0];
    let rest: Vec<usize> = (0..len).filter(|k| set.binary_search(k).is_err()).collect();
    let at = rest.iter().position(|&k| k == target)? + usize::from(down);
    let mut order = rest[..at].to_vec();
    order.extend(&set);
    order.extend(&rest[at..]);
    (order != (0..len).collect::<Vec<_>>()).then_some(order)
}

#[cfg(test)]
mod tests {
    use super::move_order;
    #[test]
    fn moves() {
        // drag 0 onto 2 (down): lands after 2
        assert_eq!(move_order(4, &[0], 2), Some(vec![1, 2, 0, 3]));
        // drag 3 onto 1 (up): lands before 1
        assert_eq!(move_order(4, &[3], 1), Some(vec![0, 3, 1, 2]));
        // several, keeping their order
        assert_eq!(move_order(5, &[0, 2], 4), Some(vec![1, 3, 4, 0, 2]));
        assert_eq!(move_order(5, &[3, 4], 0), Some(vec![3, 4, 0, 1, 2]));
        // onto itself / no-op
        assert_eq!(move_order(3, &[1], 1), None);
        assert_eq!(move_order(3, &[0], 1).unwrap(), vec![1, 0, 2]);
    }
}
