//! Settings: volume normalization, play reporting, accent colour, and the caches on disk.
use crate::model::Model;
use crate_core::player::Action;
use gpui::Context;
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use crate_core::fixer::{self, Plan};

/// Where "Fix library" is at.
#[derive(Clone)]
pub enum Fix {
    Idle,
    /// scanning or applying; progress is in `Model::fix_progress`
    Working,
    /// checked: this is what it would do
    Ready(Arc<Plan>),
    Done(String),
    Failed(String),
}

pub type FixProgress = Arc<Mutex<(String, usize, usize)>>;

/// Light/dark mode and fonts of the ready-made components (text boxes, sliders, tooltips).
fn apply_component_theme(cx: &mut gpui::App) {
    use gpui_component::{Theme, ThemeMode};
    Theme::change(if crate::theme::is_light() { ThemeMode::Light } else { ThemeMode::Dark }, None, cx);
    let t = Theme::global_mut(cx);
    t.font_family = "Inter".into();
    t.mono_font_family = "Noto Sans Mono".into();
}

#[derive(Clone, Copy)]
pub struct Storage {
    pub audio_bytes: u64,
    pub cover_bytes: u64,
}

fn dir_size(d: std::path::PathBuf) -> u64 {
    std::fs::read_dir(d).map(|rd| rd.filter_map(|e| e.ok()?.metadata().ok()).map(|m| m.len()).sum()).unwrap_or(0)
}

impl Model {
    pub fn set_normalize(&mut self, on: bool, cx: &mut Context<Self>) {
        self.player_state.normalize = on;
        self.send(Action::SetNormalize { on });
        if on {
            self.start_loudness_scan(cx);
        }
        cx.notify();
    }

    pub fn set_scrobble(&mut self, on: bool, cx: &mut Context<Self>) {
        self.player_state.scrobble = on;
        self.send(Action::SetScrobble { on });
        cx.notify();
    }

    pub fn set_accent(&mut self, name: &str, cx: &mut Context<Self>) {
        crate::theme::set_accent(name);
        self.persist("accent", json!(name));
        self.art_accent_for = None;
        self.update_art_accent(cx);
        cx.refresh_windows();
        cx.notify();
    }

    // ------------------------------------------------------------ appearance
    /// Apply the saved look at startup.
    pub(crate) fn load_appearance(&mut self, cx: &mut Context<Self>) {
        let s = self.settings.clone();
        let get_str = |k: &str| s.get(k).and_then(|v| v.as_str().map(str::to_string));
        crate::theme::set_theme(&get_str("theme").unwrap_or_else(|| "dark".into()));
        crate::theme::set_text_pct(s.get("textSize").and_then(|v| v.as_u64()).unwrap_or(100) as u32);
        crate::theme::set_density(&get_str("density").unwrap_or_else(|| "normal".into()));
        let anim = s.get("animations").and_then(|v| v.as_bool()).unwrap_or(true);
        crate::theme::set_animations(anim);
        cx.set_reduce_motion(!anim);
        let flag = |k: &str, d: bool| s.get(k).and_then(|v| v.as_bool()).unwrap_or(d);
        self.show_cols = [flag("colAlbum", true), flag("colDate", true)];
        self.home_rows = [flag("homeRecent", true), flag("homeFrequent", true)];
        apply_component_theme(cx);
    }

    pub fn set_theme(&mut self, key: &str, cx: &mut Context<Self>) {
        crate::theme::set_theme(key);
        self.persist("theme", json!(key));
        apply_component_theme(cx);
        // album-art accents are tuned to the background, so pick again
        self.art_accent_for = None;
        self.update_art_accent(cx);
        cx.refresh_windows();
        cx.notify();
    }

    pub fn set_text_size(&mut self, pct: u32, cx: &mut Context<Self>) {
        crate::theme::set_text_pct(pct);
        self.persist("textSize", json!(pct));
        cx.refresh_windows();
        cx.notify();
    }

    pub fn set_density(&mut self, key: &str, cx: &mut Context<Self>) {
        crate::theme::set_density(key);
        self.persist("density", json!(key));
        cx.refresh_windows();
        cx.notify();
    }

    pub fn set_animations(&mut self, on: bool, cx: &mut Context<Self>) {
        crate::theme::set_animations(on);
        cx.set_reduce_motion(!on);
        self.persist("animations", json!(on));
        cx.notify();
    }

    /// 0 = album column, 1 = date added column
    pub fn set_col(&mut self, i: usize, on: bool, cx: &mut Context<Self>) {
        self.show_cols[i] = on;
        self.persist(["colAlbum", "colDate"][i], json!(on));
        cx.notify();
    }

    /// 0 = "Jump back in", 1 = "Your most played"
    pub fn set_home_row(&mut self, i: usize, on: bool, cx: &mut Context<Self>) {
        self.home_rows[i] = on;
        self.persist(["homeRecent", "homeFrequent"][i], json!(on));
        cx.notify();
    }

    /// With the "from album art" accent: take the colour from the playing song's cover.
    pub(crate) fn update_art_accent(&mut self, cx: &mut Context<Self>) {
        if !crate::theme::accent_from_art() {
            return;
        }
        let Some(t) = self.current() else { return };
        let (id, cover) = (t.id.clone(), t.cover_art.clone());
        if self.art_accent_for.as_deref() == Some(id.as_str()) {
            return;
        }
        let Some(path) = self.covers.get(cover.as_deref(), crate::covers::SMALL) else {
            // not downloaded yet: try again shortly
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_millis(1500)).await;
                let _ = this.update(cx, |m, cx| m.update_art_accent(cx));
            })
            .detach();
            return;
        };
        self.art_accent_for = Some(id);
        cx.spawn(async move |_, cx| {
            let c = cx
                .background_executor()
                .spawn(async move { image::open(&path).ok().and_then(|i| crate::theme::accent_from_image(&i.thumbnail(48, 48).to_rgba8())) })
                .await;
            crate::theme::set_accent_rgb(c.unwrap_or(0xe8a33d));
            let _ = cx.update(|cx| cx.refresh_windows());
        })
        .detach();
    }

    /// Measure the whole library's loudness, a little at a time in the background (only once;
    /// measured songs are remembered), so normalization works for songs you haven't played yet.
    pub fn start_loudness_scan(&mut self, cx: &mut Context<Self>) {
        if !self.player_state.normalize || self.loud_scan_started {
            return;
        }
        self.loud_scan_started = true;
        let c = self.client.clone();
        cx.spawn(async move |_, cx| {
            // let startup and whatever you open first have the network to themselves
            cx.background_executor().timer(Duration::from_secs(30)).await;
            cx.background_executor()
                .spawn(async move {
                    if let Ok(all) = c.all_songs() {
                        crate_core::loudness::want_eventually(&c, all);
                    }
                })
                .await;
        })
        .detach();
    }

    pub fn refresh_storage(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let st = cx
                .background_executor()
                .spawn(async move {
                    Storage { audio_bytes: crate_core::audio_cache::size(), cover_bytes: dir_size(crate_core::cache_dir().join("covers")) }
                })
                .await;
            let _ = this.update(cx, |m, cx| {
                m.storage = Some(st);
                cx.notify();
            });
        })
        .detach();
    }

    pub fn clear_audio_cache(&mut self, cx: &mut Context<Self>) {
        crate_core::audio_cache::clear();
        self.toast("Cleared downloaded audio (songs will be fetched again as they play)", false, cx);
        self.refresh_storage(cx);
    }

    // ------------------------------------------------------------ fix library
    pub fn set_auto_fix(&mut self, on: bool, cx: &mut Context<Self>) {
        fixer::set_auto(on);
        self.persist("autoFix", json!(on));
        cx.notify();
    }

    /// Look the library over and show what fixing would change (changes nothing yet).
    pub fn check_library(&mut self, cx: &mut Context<Self>) {
        if matches!(self.fix, Fix::Working) {
            return;
        }
        self.fix = Fix::Working;
        *self.fix_progress.lock().unwrap() = ("Starting".into(), 0, 0);
        let (dz, prog, slot, client) = (self.dz.clone(), self.fix_progress.clone(), self.fix_plan.clone(), self.client.clone());
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(async move {
                    let root = crate_core::subsonic::library_dir();
                    let p2 = prog.clone();
                    let (plan, idx) = fixer::plan(&root, &dz, Some(&client), fixer::Progress(&move |what, d, n| *p2.lock().unwrap() = (what.to_string(), d, n)))?;
                    let plan = Arc::new(plan);
                    *slot.lock().unwrap() = Some((plan.clone(), idx));
                    anyhow::Ok(plan)
                })
                .await;
            let _ = this.update(cx, |m, cx| {
                m.fix = match r {
                    Ok(p) if p.is_empty() && p.mismatched.is_empty() => Fix::Done("Everything looks tidy: no duplicates or missing details found.".into()),
                    Ok(p) => Fix::Ready(p),
                    Err(e) => Fix::Failed(format!("{e:#}")),
                };
                cx.notify();
            });
        })
        .detach();
    }

    pub fn cancel_fix(&mut self, cx: &mut Context<Self>) {
        self.fix = Fix::Idle;
        *self.fix_plan.lock().unwrap() = None;
        cx.notify();
    }

    pub fn apply_fix(&mut self, cx: &mut Context<Self>) {
        let Some((plan, idx)) = self.fix_plan.lock().unwrap().take() else { return };
        self.fix = Fix::Working;
        *self.fix_progress.lock().unwrap() = ("Starting".into(), 0, 0);
        let (client, prog) = (self.client.clone(), self.fix_progress.clone());
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(async move {
                    let p2 = prog.clone();
                    fixer::apply(&plan, &idx, Some(&client), fixer::Progress(&move |what, d, n| *p2.lock().unwrap() = (what.to_string(), d, n)))
                })
                .await;
            let _ = this.update(cx, |m, cx| {
                m.fix = match r {
                    Ok(rep) => {
                        let mut parts = vec![];
                        if rep.removed > 0 {
                            parts.push(format!("moved {} duplicate{} to ~/music-dupes-review", rep.removed, if rep.removed == 1 { "" } else { "s" }));
                        }
                        if rep.edited > 0 {
                            parts.push(format!("fixed {} song{}", rep.edited, if rep.edited == 1 { "" } else { "s" }));
                        }
                        if rep.playlists > 0 {
                            parts.push(format!("updated {} playlist{}", rep.playlists, if rep.playlists == 1 { "" } else { "s" }));
                        }
                        let mut msg = format!("Done: {}.", if parts.is_empty() { "nothing needed changing".to_string() } else { parts.join(", ") });
                        if !rep.failed.is_empty() {
                            msg += &format!(" {} couldn't be changed (file in use or read-only).", rep.failed.len());
                        }
                        Fix::Done(msg)
                    }
                    Err(e) => Fix::Failed(format!("{e:#}")),
                };
                m.after_library_change(cx);
            });
        })
        .detach();
    }

    pub fn undo_fix(&mut self, cx: &mut Context<Self>) {
        self.fix = Fix::Working;
        *self.fix_progress.lock().unwrap() = ("Putting everything back".into(), 0, 0);
        let c = self.client.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { fixer::undo(&crate_core::subsonic::library_dir(), Some(&c)) }).await;
            let _ = this.update(cx, |m, cx| {
                m.fix = match r {
                    Ok(_) => Fix::Done("Undone: files, tags and playlists are back the way they were.".into()),
                    Err(e) => Fix::Failed(format!("Couldn't undo: {e:#}")),
                };
                m.after_library_change(cx);
            });
        })
        .detach();
    }

    /// Songs moved or retagged: forget cached lists, and look again once Navidrome has rescanned.
    fn after_library_change(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        cx.spawn(async move |this, cx| {
            for wait in [2u64, 10] {
                cx.background_executor().timer(Duration::from_secs(wait)).await;
                let ok = this.update(cx, |m, cx| {
                    m.cache.clear();
                    m.albums.clear();
                    m.artists = None;
                    m.artist_pages.clear();
                    m.reload_playlists(cx);
                    m.load_route(cx);
                });
                if ok.is_err() {
                    break;
                }
            }
        })
        .detach();
    }
}
