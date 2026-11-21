//! Crate — native (gpui) interface. All audio, library, download and lyrics logic lives in
//! crate-core; this crate only draws the window.
mod catalog;
mod covers;
mod edit;
mod imgcache;
mod importer;
mod onboarding;
mod lyrics_view;
mod model;
mod player_bar;
mod prefs;
mod shell;
mod smooth;
mod theme;

use crate_core::subsonic::Client;
use gpui::{prelude::*, px, size, App, AssetSource, Bounds, Entity, SharedString, WindowBounds, WindowOptions};
use gpui_component::{Root, Theme, ThemeMode};
use std::borrow::Cow;

// Only the icons Crate uses are compiled in (keeps the binary small).
gpui_kit_assets::icon_assets!(CrateIcons, [
    Play, Pause, Shuffle, Repeat, Repeat1, SkipBack, SkipForward, ListMusic, House, Library, Search, Mic,
    Download, Plus, X, ChevronLeft, ChevronRight, ArrowUp, ArrowDown, Music, Volume1, Volume2, VolumeX,
    LoaderCircle, Check, ListPlus, ListEnd, ListStart, RotateCw, CircleCheck, CircleAlert, Headphones,
    Ellipsis, Pencil, Trash, Undo2, ListMinus, Settings, Clock
]);

/// Our icons first, then gpui-component's own defaults (input clear buttons etc.).
struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        match CrateIcons.load(path)? {
            Some(b) => Ok(Some(b)),
            None => gpui_kit_assets::Assets.load(path),
        }
    }
    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        gpui_kit_assets::Assets.list(path)
    }
}

fn main() {
    // glibc gives every thread that allocates its own memory pool ("arena") and rarely hands
    // freed memory back. Short-lived worker threads (downloads, cover fetches, loudness scans)
    // left ~75 MB stranded that way while playing; two shared arenas keep it to a few MB.
    #[cfg(target_env = "gnu")]
    unsafe {
        libc::mallopt(libc::M_ARENA_MAX, 2);
    }
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,lofty=error")).init();
    // not set up yet (or CRATE_SETUP=1): the window opens on the setup screens instead
    let client = Client::from_crate_config().ok().filter(|_| std::env::var("CRATE_SETUP").is_err());
    gpui_platform::application().with_assets(Assets).run(move |cx: &mut App| {
        gpui_component::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        {
            let t = Theme::global_mut(cx);
            t.font_family = "Inter".into(); // installed with all weights; gpui's default name doesn't exist on Linux
            t.mono_font_family = "Noto Sans Mono".into();
        }
        shell::bind_keys(cx);
        // CRATE_WINDOW=WxH opens at a specific size (handy for testing narrow layouts)
        let (w, h) = std::env::var("CRATE_WINDOW")
            .ok()
            .and_then(|v| v.split_once('x').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?))))
            .unwrap_or((1320., 840.));
        let bounds = Bounds::centered(None, size(px(w), px(h)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                focus: true,
                window_min_size: Some(size(px(560.), px(420.))),
                app_id: Some("crate-desktop".into()), // matches crate-desktop.desktop (icon, taskbar grouping)
                titlebar: Some(gpui::TitlebarOptions { title: Some("Crate".into()), ..Default::default() }),
                ..Default::default()
            },
            move |window, cx| {
                let stage = match client {
                    Some(c) => Stage::App(start_app(c, window, cx)),
                    None => {
                        // CRATE_SETUP=tools|navidrome|soulseek|finish opens on that step (testing)
                        let step = match std::env::var("CRATE_SETUP").as_deref() {
                            Ok("tools") => onboarding::Step::Tools,
                            Ok("navidrome") => onboarding::Step::Navidrome,
                            Ok("soulseek") => onboarding::Step::Soulseek,
                            Ok("finish") => onboarding::Step::Finish,
                            _ => onboarding::Step::Welcome,
                        };
                        Stage::Setup(cx.new(|cx| onboarding::Onboarding::new(step, false, window, cx)))
                    }
                };
                let top = cx.new(|cx| Top::new(stage, window, cx));
                cx.new(|cx| Root::new(top, window, cx))
            },
        )
        .expect("open window");
    });
}

enum Stage {
    Setup(Entity<onboarding::Onboarding>),
    App(Entity<shell::Shell>),
}

/// The window's content: the setup screens until Crate is connected, then the app.
struct Top {
    stage: Stage,
    _sub: Option<gpui::Subscription>,
}

impl Top {
    fn new(stage: Stage, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> Self {
        let sub = match &stage {
            Stage::Setup(ob) => Some(cx.subscribe_in(ob, window, |top: &mut Top, _, ev: &onboarding::SetupDone, window, cx| {
                match Client::from_crate_config() {
                    Ok(c) => {
                        let shell = start_app(c, window, cx);
                        if ev.import {
                            let model = shell.read(cx).model().clone();
                            model.update(cx, |m, cx| m.go(model::Route::Import, cx));
                        }
                        top.stage = Stage::App(shell);
                        top._sub = None;
                        cx.notify();
                    }
                    Err(e) => {
                        eprintln!("setup finished but Crate still can't connect: {e:#}");
                        if onboarding::demo() {
                            cx.quit(); // a demo without a library to open afterwards just ends
                        }
                    }
                }
            })),
            Stage::App(_) => None,
        };
        Top { stage, _sub: sub }
    }
}

impl Render for Top {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        match &self.stage {
            Stage::Setup(ob) => ob.clone().into_any_element(),
            Stage::App(shell) => shell.clone().into_any_element(),
        }
    }
}

/// Start the player, library and downloads, and build the main window content.
fn start_app(client: Client, window: &mut gpui::Window, cx: &mut App) -> Entity<shell::Shell> {
    let model = cx.new(|cx| model::Model::new(client, cx));
    let shell = cx.new(|cx| shell::Shell::new(model.clone(), window, cx));
    if let Some(secs) = std::env::var("CRATE_DEMO_SEEK").ok().and_then(|v| v.parse::<f64>().ok()) {
        // test hook: jump into the current song (still paused) to check the lyrics highlight
        let m = model.clone();
        window
            .spawn(cx, async move |cx| {
                cx.background_executor().timer(std::time::Duration::from_secs(4)).await;
                let _ = cx.update(|_, cx| {
                    let clock = m.read(cx).clock.clone();
                    clock.update(cx, |c, cx| {
                        c.progress.position = secs;
                        c.at = std::time::Instant::now();
                        cx.notify();
                    })
                });
            })
            .detach();
    }
    if std::env::var("CRATE_DEMO_MENU").is_ok() {
        // test hook: show the "add to playlist" menu and the new-playlist prompt
        let s = shell.clone();
        window
            .spawn(cx, async move |cx| {
                cx.background_executor().timer(std::time::Duration::from_secs(6)).await;
                let _ = cx.update(|window, cx| s.update(cx, |sh, cx| sh.demo_overlays(window, cx)));
            })
            .detach();
    }
    if let Some(y) = std::env::var("CRATE_DEMO_SCROLL").ok().and_then(|v| v.parse::<f32>().ok()) {
        // test hook: scroll the song list to y px
        let s = shell.clone();
        window
            .spawn(cx, async move |cx| {
                cx.background_executor().timer(std::time::Duration::from_secs(4)).await;
                let _ = cx.update(|_, cx| s.update(cx, |sh, cx| sh.demo_scroll(y, cx)));
            })
            .detach();
    }
    if std::env::var("CRATE_DEMO_FIX").is_ok() {
        // test hook: run "Check library" (read-only) to show its summary
        let m = model.clone();
        window
            .spawn(cx, async move |cx| {
                cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
                let _ = cx.update(|_, cx| m.update(cx, |m, cx| m.check_library(cx)));
            })
            .detach();
    }
    if std::env::var("CRATE_DEMO_PLAY").is_ok() {
        // test hook: start playing (the harness mutes the player and turns off scrobbling)
        let m = model.clone();
        window
            .spawn(cx, async move |cx| {
                cx.background_executor().timer(std::time::Duration::from_secs(3)).await;
                let _ = cx.update(|_, cx| m.read(cx).send(crate_core::player::Action::Play));
            })
            .detach();
    }
    if let Ok(files) = std::env::var("CRATE_DEMO_IMPORT") {
        // test hook: read these CSVs on the Import page (shows the preview; imports nothing)
        let m = model.clone();
        let files: Vec<std::path::PathBuf> = std::env::split_paths(&files).collect();
        window
            .spawn(cx, async move |cx| {
                cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
                let _ = cx.update(|_, cx| m.update(cx, |m, cx| m.import_read(files, cx)));
            })
            .detach();
    }
    if std::env::var("CRATE_DEMO_EDIT").is_ok() {
        shell::demo_edit(shell.clone(), window, cx);
    }
    if let Ok(out) = std::env::var("CRATE_BENCH") {
        let s = shell.clone();
        window
            .spawn(cx, async move |cx| {
                cx.background_executor().timer(std::time::Duration::from_secs(5)).await;
                let _ = cx.update(|window, cx| s.update(cx, |sh, cx| sh.bench_scroll(out, window, cx)));
            })
            .detach();
    }
    shell
}
