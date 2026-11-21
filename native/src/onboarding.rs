//! First-run setup, also reachable later from Settings: helper programs, Navidrome (install one
//! or sign in to yours), and a Soulseek account. Ends by offering the Spotify import.
use crate::shell::icon;
use crate::theme::*;
use crate_core::setup::{self, SlskLogin, Tool};
use gpui::{div, img, prelude::*, px, AnyElement, ClickEvent, Context, Entity, EventEmitter, FontWeight, SharedString, Window};
use gpui_component::input::{Input, InputState};
use gpui_kit_assets::IconName;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    Welcome,
    Tools,
    Navidrome,
    Soulseek,
    Finish,
}

/// Sent when the person is done: `import` = they want to bring Spotify playlists over now;
/// `server_changed` = a different Navidrome was set up (the app has to restart to use it).
pub struct SetupDone {
    pub import: bool,
    pub server_changed: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum NdMode {
    Install,
    Existing,
}

pub struct Onboarding {
    step: Step,
    /// opened from Settings for one step (finishing returns there)
    embedded: bool,
    tools: Vec<Tool>,
    music: Entity<InputState>,
    nd_mode: NdMode,
    nd_url: Entity<InputState>,
    nd_user: Entity<InputState>,
    nd_pass: Entity<InputState>,
    sl_user: Entity<InputState>,
    sl_pass: Entity<InputState>,
    /// Soulseek name sockseek already has (keep it, or replace it)
    sl_existing: Option<String>,
    sl_replace: bool,
    /// what's happening in the background, shown instead of the buttons
    busy: Option<Arc<Mutex<String>>>,
    error: Option<String>,
    server_changed: bool,
}

impl EventEmitter<SetupDone> for Onboarding {}

/// Demo mode (CRATE_SETUP_DEMO, see tools/try-setup.sh): the screens behave as usual but every
/// action is pretend. Nothing is downloaded, installed, signed in to or saved.
/// CRATE_SETUP_DEMO=missing also pretends some helper programs are missing.
pub fn demo() -> bool {
    std::env::var_os("CRATE_SETUP_DEMO").is_some()
}

fn demo_tools() -> Vec<Tool> {
    let missing = std::env::var("CRATE_SETUP_DEMO").as_deref() == Ok("missing");
    let mut t = setup::tools();
    for x in &mut t {
        x.found = !(missing && matches!(x.name, "ffmpeg" | "fpcalc" | "sockseek" | "yt-dlp"));
        x.system = !x.auto;
    }
    t
}

/// Pretend work for demo mode: show each status line for a moment.
fn demo_steps(status: &Mutex<String>, steps: &[&str]) {
    for s in steps {
        *status.lock().unwrap() = s.to_string();
        std::thread::sleep(Duration::from_millis(1100));
    }
}

fn home_music() -> String {
    dirs::audio_dir().or_else(|| dirs::home_dir().map(|h| h.join("Music"))).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

impl Onboarding {
    pub fn new(start: Step, embedded: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // demo mode always looks like a brand-new computer
        let login = if demo() { None } else { setup::current_login() };
        let input = |ph: &str, value: String, masked: bool, window: &mut Window, cx: &mut Context<Self>| {
            let ph = ph.to_string();
            cx.new(|cx| InputState::new(window, cx).placeholder(ph).masked(masked).default_value(value))
        };
        let music_default = login.as_ref().map(|l| l.2.clone()).filter(|m| !m.is_empty()).unwrap_or_else(home_music);
        let url_default = login.as_ref().map(|l| l.0.clone()).unwrap_or_else(|| format!("http://127.0.0.1:{}", setup::NAVIDROME_PORT));
        let user_default = login.as_ref().map(|l| l.1.clone()).unwrap_or_default();
        let sl_existing = if demo() { None } else { setup::soulseek_account() };
        let v = Onboarding {
            step: start,
            embedded,
            tools: if demo() { demo_tools() } else { setup::tools() },
            music: input("Music folder", music_default, false, window, cx),
            // a Navidrome that's already running (or a saved login) means "sign in", not "install"
            nd_mode: if login.is_some() { NdMode::Existing } else { NdMode::Install },
            nd_url: input("Server address", url_default, false, window, cx),
            nd_user: input("Username", user_default, false, window, cx),
            nd_pass: input("Password", String::new(), true, window, cx),
            sl_user: input("Soulseek username", String::new(), false, window, cx),
            sl_pass: input("Soulseek password", String::new(), true, window, cx),
            sl_replace: sl_existing.is_none(),
            sl_existing,
            busy: None,
            error: None,
            server_changed: false,
        };
        if login.is_none() && !demo() {
            // checked off the UI thread: is a Navidrome already running here?
            cx.spawn(async move |this, cx| {
                let url = format!("http://127.0.0.1:{}", setup::NAVIDROME_PORT);
                let alive = cx.background_executor().spawn(async move { setup::server_alive(&url) }).await;
                let _ = this.update(cx, |v, cx| {
                    if alive {
                        v.nd_mode = NdMode::Existing;
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        v
    }

    fn val(i: &Entity<InputState>, cx: &gpui::App) -> String {
        i.read(cx).value().trim().to_string()
    }

    fn go(&mut self, step: Step, cx: &mut Context<Self>) {
        self.error = None;
        if step == Step::Tools && !demo() {
            self.tools = setup::tools();
        }
        self.step = step;
        cx.notify();
    }

    fn finish(&mut self, import: bool, cx: &mut Context<Self>) {
        cx.emit(SetupDone { import, server_changed: self.server_changed });
    }

    /// After a step done from Settings, go back there; during setup, on to the next one.
    fn next(&mut self, after: Step, cx: &mut Context<Self>) {
        if self.embedded {
            self.finish(false, cx);
        } else {
            self.go(after, cx);
        }
    }

    /// Run `work` off the UI thread with a status line; `done` gets its result.
    fn run<T: Send + 'static>(
        &mut self,
        first: &str,
        work: impl FnOnce(Arc<Mutex<String>>) -> anyhow::Result<T> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        let status = Arc::new(Mutex::new(first.to_string()));
        self.busy = Some(status.clone());
        self.error = None;
        cx.notify();
        // keep the status line current while it works
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(250)).await;
            let live = this.update(cx, |v, cx| {
                cx.notify();
                v.busy.is_some()
            });
            if !matches!(live, Ok(true)) {
                break;
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { work(status) }).await;
            let _ = this.update(cx, |v, cx| {
                v.busy = None;
                match r {
                    Ok(t) => done(v, t, cx),
                    Err(e) => v.error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn install_tool(&mut self, name: &'static str, cx: &mut Context<Self>) {
        let system = self.tools.iter().any(|t| t.name == name && t.system);
        let status = if system {
            format!("Installing {name}\u{2026} type your password in the window that popped up.")
        } else {
            format!("Downloading {name}\u{2026}")
        };
        if demo() {
            return self.run(
                &status,
                move |s| Ok(demo_steps(&s, if system { &["Installing\u{2026} type your password in the window that popped up. (Demo: no window, nothing installed.)", "Installing\u{2026}"] } else { &[] })),
                move |v, _, _| v.tools.iter_mut().filter(|t| t.name == name).for_each(|t| t.found = true),
                cx,
            );
        }
        self.run(
            &status,
            move |_| setup::install_tool(name),
            |v, _, _| v.tools = setup::tools(),
            cx,
        );
    }

    fn choose_music(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Use this folder".into()) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if let Some(p) = paths.into_iter().next() {
                    let _ = this.update_in(cx, |v, window, cx| v.music.update(cx, |i, cx| i.set_value(p.to_string_lossy().into_owned(), window, cx)));
                }
            }
        })
        .detach();
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        let music = PathBuf::from(shellexpand(&Self::val(&self.music, cx)));
        let (url, user, pass) = (Self::val(&self.nd_url, cx), Self::val(&self.nd_user, cx), self.nd_pass.read(cx).value().to_string());
        if music.as_os_str().is_empty() || user.is_empty() || pass.is_empty() {
            self.error = Some("Fill in the music folder, a username and a password.".into());
            return cx.notify();
        }
        let mode = self.nd_mode;
        if demo() {
            // type "wrong" as the password to see what a failed sign-in looks like
            return self.run(
                "Starting\u{2026}",
                move |s| {
                    match mode {
                        NdMode::Install => demo_steps(&s, &["Downloading Navidrome\u{2026}", "Setting it up\u{2026}", "Starting Navidrome\u{2026}", "Creating your account\u{2026}"]),
                        NdMode::Existing => demo_steps(&s, &["Signing in\u{2026}"]),
                    }
                    if pass == "wrong" {
                        anyhow::bail!("Wrong username or password");
                    }
                    Ok(())
                },
                |v, _, cx| v.next(Step::Soulseek, cx),
                cx,
            );
        }
        self.run(
            if mode == NdMode::Install { "Starting\u{2026}" } else { "Signing in\u{2026}" },
            move |status| {
                std::fs::create_dir_all(&music).map_err(|e| anyhow::anyhow!("couldn't use {}: {e}", music.display()))?;
                match mode {
                    NdMode::Install => setup::install_navidrome(&music, &user, &pass, &|s| *status.lock().unwrap() = s.to_string()).map(|_| ()),
                    NdMode::Existing => setup::login(&url, &user, &pass, &music),
                }
            },
            |v, _, cx| {
                v.server_changed = true;
                v.next(Step::Soulseek, cx)
            },
            cx,
        );
    }

    fn soulseek(&mut self, cx: &mut Context<Self>) {
        let (user, pass) = (Self::val(&self.sl_user, cx), self.sl_pass.read(cx).value().to_string());
        if user.is_empty() || pass.is_empty() {
            self.error = Some("Pick a username and a password.".into());
            return cx.notify();
        }
        if demo() {
            // the name "taken" shows the "name already taken" message
            return self.run(
                "Signing in to Soulseek\u{2026}",
                move |s| {
                    demo_steps(&s, &["Signing in to Soulseek\u{2026}"]);
                    Ok(if user == "taken" { SlskLogin::Taken } else { SlskLogin::Ok })
                },
                |v, r, cx| match r {
                    SlskLogin::Ok => v.next(Step::Finish, cx),
                    _ => v.error = Some("That name is already taken, or the password is wrong. Pick another name, or type the right password.".into()),
                },
                cx,
            );
        }
        self.run(
            "Signing in to Soulseek\u{2026}",
            move |_| {
                let r = setup::soulseek_login(&user, &pass)?;
                if r == SlskLogin::Ok {
                    setup::save_soulseek(&user, &pass)?;
                }
                Ok(r)
            },
            |v, r, cx| match r {
                SlskLogin::Ok => v.next(Step::Finish, cx),
                SlskLogin::Taken => v.error = Some("That name is already taken, or the password is wrong. Pick another name, or type the right password.".into()),
                SlskLogin::Refused(why) => v.error = Some(why),
            },
            cx,
        );
    }

    // ------------------------------------------------------------ drawing

    fn page(&self, title: &str, body: impl IntoElement, actions: AnyElement) -> gpui::Div {
        div()
            .w_full()
            .max_w(px(600.))
            .flex()
            .flex_col()
            .gap_5()
            .child(div().text_3xl().font_weight(FontWeight::EXTRA_BOLD).child(title.to_string()))
            .child(body)
            .when_some(self.error.clone(), |d, e| {
                d.child(
                    div()
                        .flex()
                        .gap_2()
                        .p_3()
                        .rounded_lg()
                        .bg(panel2())
                        .child(div().flex_none().pt_0p5().child(icon(IconName::CircleAlert, 16., gpui::red())))
                        .child(div().flex_1().min_w_0().text_sm().child(e)),
                )
            })
            .child(match &self.busy {
                Some(s) => div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .h(px(40.))
                    .child(icon(IconName::LoaderCircle, 18., accent()))
                    .child(div().text_color(dim()).child(s.lock().unwrap().clone()))
                    .into_any_element(),
                None => div().flex().child(actions).into_any_element(),
            })
    }

    fn welcome(&self, cx: &mut Context<Self>) -> gpui::Div {
        let body = div()
            .flex()
            .flex_col()
            .gap_3()
            .text_color(dim())
            .child("Crate plays your own music, kept on your own computer by Navidrome, and can fetch songs you don\u{2019}t have yet from Soulseek (or YouTube, when Soulseek doesn\u{2019}t have them).")
            .child("Setting up takes a couple of minutes: a few helper programs, Navidrome, and a Soulseek account. After that you can bring your Spotify playlists over.");
        div()
            .flex()
            .flex_col()
            .items_start()
            .gap_6()
            .child(img(crate::shell::app_icon()).size(px(72.)))
            .child(self.page(
                "Welcome to Crate",
                body,
                primary("ob-start", "Get started").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.go(Step::Tools, cx))).into_any_element(),
            ))
    }

    fn tools_step(&self, cx: &mut Context<Self>) -> gpui::Div {
        let rows = self.tools.iter().map(|t| {
            let name = t.name;
            div()
                .flex()
                .items_center()
                .gap_4()
                .py_3()
                .border_b_1()
                .border_color(line())
                .child(if t.found { icon(IconName::CircleCheck, 20., accent()) } else { icon(IconName::CircleAlert, 20., faint()) })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(name))
                        .child(div().text_sm().text_color(dim()).child(t.what))
                        .when(!t.found && t.system, |d| {
                            d.child(div().text_xs().text_color(faint()).child("A window will pop up asking for your password, because installing system programs needs it."))
                        })
                        .when(!t.found && !t.auto && !t.system, |d| {
                            // no way to ask for the password from here: show the command to run
                            d.child(div().mt_1().px_2().py_1().rounded_md().bg(panel2()).text_sm().font_family("Noto Sans Mono").child(t.install.clone().unwrap_or_default()))
                        }),
                )
                .when(!t.found && (t.auto || t.system), |d| {
                    d.child(secondary(SharedString::from(format!("ob-get-{name}")), "Install").on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.install_tool(name, cx))))
                })
        });
        let missing = self.tools.iter().filter(|t| !t.found).count();
        let body = div()
            .flex()
            .flex_col()
            .child(div().text_color(dim()).mb_2().child(if missing == 0 {
                "Everything Crate needs is installed."
            } else {
                "Crate uses these programs. Click Install next to any that are missing. Anything missing only turns off what it\u{2019}s for, so you can also carry on without them."
            }))
            .children(rows);
        let actions = div()
            .flex()
            .gap_2()
            .child(primary("ob-tools-next", if self.embedded { "Done" } else { "Continue" }).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.next(Step::Navidrome, cx))))
            .when(missing > 0, |d| d.child(secondary("ob-tools-again", "Check again").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.go(Step::Tools, cx)))))
            .into_any_element();
        self.page("Helper programs", body, actions)
    }

    fn navidrome_step(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mode = self.nd_mode;
        let seg = |id: &'static str, label: &'static str, m: NdMode, cx: &mut Context<Self>| {
            let on = mode == m;
            div()
                .id(id)
                .px_4()
                .py_1p5()
                .rounded_full()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .cursor_pointer()
                .when(on, |d| d.bg(text()).text_color(on_text()))
                .when(!on, |d| d.text_color(dim()).hover(|s| s.text_color(text())))
                .child(label)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.nd_mode = m;
                    this.error = None;
                    cx.notify();
                }))
        };
        let explain = match mode {
            NdMode::Install => "Crate downloads Navidrome, points it at your music folder and keeps it running in the background whenever you\u{2019}re logged in. Only this computer can reach it. Choose a username and password for it.",
            NdMode::Existing => "Sign in to the Navidrome you already run. The music folder is where Crate saves new downloads; it should be the folder (or inside the folder) Navidrome scans.",
        };
        let body = div()
            .flex()
            .flex_col()
            .gap_4()
            .child(div().text_color(dim()).child("Navidrome keeps your music library and plays it to Crate (and to apps on your phone, if you like)."))
            .child(div().flex().p_1().rounded_full().bg(panel2()).self_start().child(seg("ob-nd-install", "Set it up for me", NdMode::Install, cx)).child(seg("ob-nd-existing", "I already have one", NdMode::Existing, cx)))
            .child(div().text_sm().text_color(dim()).child(explain))
            .child(field(
                "Music folder",
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.music)))
                    .child(secondary("ob-browse", "Browse\u{2026}").py_1().text_sm().on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.choose_music(window, cx)))),
            ))
            .when(mode == NdMode::Existing, |d| d.child(field("Server address", Input::new(&self.nd_url))))
            .child(field("Username", Input::new(&self.nd_user)))
            .child(field("Password", Input::new(&self.nd_pass).mask_toggle()));
        let actions = div()
            .flex()
            .gap_2()
            .child(primary("ob-nd-go", if mode == NdMode::Install { "Set up Navidrome" } else { "Sign in" }).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.connect(cx))))
            .when(!self.embedded, |d| d.child(secondary("ob-nd-back", "Back").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.go(Step::Tools, cx)))))
            .into_any_element();
        self.page("Your music library", body, actions)
    }

    fn soulseek_step(&self, cx: &mut Context<Self>) -> gpui::Div {
        let keep = self.sl_existing.clone().filter(|_| !self.sl_replace);
        let body = div()
            .flex()
            .flex_col()
            .gap_4()
            .child(div().text_color(dim()).child("Soulseek is a long-running file-sharing network where people share their music collections. Crate searches it for songs you ask for."))
            .child(match &keep {
                Some(name) => div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .p_4()
                    .rounded_lg()
                    .bg(panel())
                    .child(icon(IconName::CircleCheck, 18., accent()))
                    .child(div().flex_1().child(format!("Signed in as {name}")))
                    .child(secondary("ob-sl-change", "Use another account").on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.sl_replace = true;
                        cx.notify();
                    })))
                    .into_any_element(),
                None => div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(div().text_sm().text_color(dim()).child("There\u{2019}s no sign-up page: pick a name and password, and if the name is free the account is created the first time you sign in. Already have one? Just sign in with it."))
                    .child(field("Username", Input::new(&self.sl_user)))
                    .child(field("Password", Input::new(&self.sl_pass).mask_toggle()))
                    .child(div().text_xs().text_color(faint()).child("Saved where the downloader (sockseek) reads it, readable only by you."))
                    .into_any_element(),
            });
        let actions = div()
            .flex()
            .gap_2()
            .child(match keep {
                Some(_) => primary("ob-sl-keep", if self.embedded { "Done" } else { "Continue" }).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.next(Step::Finish, cx))),
                None => primary("ob-sl-go", "Sign in").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.soulseek(cx))),
            })
            .child(secondary("ob-sl-skip", if self.embedded { "Cancel" } else { "Skip for now" }).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.next(Step::Finish, cx))))
            .into_any_element();
        self.page("Soulseek account", body, actions)
    }

    fn finish_step(&self, cx: &mut Context<Self>) -> gpui::Div {
        let body = div()
            .flex()
            .flex_col()
            .gap_3()
            .text_color(dim())
            .child("Coming from Spotify? Crate can rebuild your playlists: songs you already have go straight in, and the rest are downloaded into them.")
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_4()
                    .rounded_lg()
                    .bg(panel())
                    .text_sm()
                    .child("1.  Open exportify.net and log in with Spotify.")
                    .child("2.  Click \u{201c}Export All\u{201d}. You get a .zip with one .csv file per playlist.")
                    .child("3.  Unzip it, then pick the .csv files in Crate."),
            )
            .child(div().text_sm().child("You can also do this later in Settings \u{2192} Accounts."));
        let actions = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .child(primary("ob-import", "Import Spotify playlists").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.finish(true, cx))))
            .child(secondary("ob-exportify", "Open exportify.net").on_click(|_, _, cx| cx.open_url("https://exportify.net")))
            .child(secondary("ob-done", "Start listening").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.finish(false, cx))))
            .into_any_element();
        self.page("You\u{2019}re all set", body, actions)
    }
}

/// `~/x` → home folder.
fn shellexpand(p: &str) -> String {
    match (p.strip_prefix("~/"), dirs::home_dir()) {
        (Some(rest), Some(h)) => h.join(rest).to_string_lossy().into_owned(),
        _ => p.to_string(),
    }
}

fn field(label: &'static str, control: impl IntoElement) -> gpui::Div {
    div().flex().flex_col().gap_1p5().child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(label)).child(control)
}

pub(crate) fn primary(id: impl Into<gpui::ElementId>, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_none()
        .px_5()
        .py_2()
        .rounded_full()
        .cursor_pointer()
        .bg(accent())
        .text_color(accent_ink())
        .font_weight(FontWeight::SEMIBOLD)
        .hover(|s| s.opacity(0.9))
        .child(label)
}

pub(crate) fn secondary(id: impl Into<gpui::ElementId>, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_none()
        .px_5()
        .py_2()
        .rounded_full()
        .border_1()
        .border_color(line())
        .cursor_pointer()
        .font_weight(FontWeight::SEMIBOLD)
        .hover(|s| s.bg(hover()))
        .child(label)
}

impl Render for Onboarding {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let steps = [Step::Tools, Step::Navidrome, Step::Soulseek, Step::Finish];
        let at = steps.iter().position(|s| *s == self.step);
        let content = match self.step {
            Step::Welcome => self.welcome(cx),
            Step::Tools => self.tools_step(cx),
            Step::Navidrome => self.navidrome_step(cx),
            Step::Soulseek => self.soulseek_step(cx),
            Step::Finish => self.finish_step(cx),
        };
        div()
            .id("onboarding")
            .size_full()
            .overflow_y_scroll()
            .bg(bg())
            .text_color(text())
            .font_family("Inter")
            .flex()
            .flex_col()
            .items_center()
            .px_8()
            .py_12()
            .when(demo(), |d| {
                d.child(
                    div()
                        .w_full()
                        .max_w(px(600.))
                        .mb_6()
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .bg(panel2())
                        .text_sm()
                        .text_color(dim())
                        .child("Demo: nothing here is really downloaded, installed or saved. Try the password \u{201c}wrong\u{201d} or the Soulseek name \u{201c}taken\u{201d} to see the error messages."),
                )
            })
            .when(!self.embedded, |d| {
                // progress dots
                d.child(div().w_full().max_w(px(600.)).flex().gap_2().mb_10().children(steps.iter().enumerate().map(|(i, _)| {
                    let done = at.map(|a| i <= a).unwrap_or(false);
                    div().h(px(4.)).flex_1().rounded_full().bg(if done { accent() } else { hover() })
                })))
            })
            .child(content)
    }
}
