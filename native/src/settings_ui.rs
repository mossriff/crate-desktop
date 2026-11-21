//! The Settings page: playback, library, accounts, appearance, storage and the keyboard shortcuts.
use super::*;
use crate::theme::{accent_name, ACCENTS, DENSITIES, TEXT_SIZES, THEMES};

fn mb(b: u64) -> String {
    if b >= 1 << 30 { format!("{:.1} GB", b as f64 / (1u64 << 30) as f64) } else { format!("{} MB", b >> 20) }
}

fn section(title: &'static str) -> gpui::Div {
    div().flex().flex_col().gap_1().mb_8().child(div().text_xl().font_weight(FontWeight::BOLD).mb_3().child(title))
}

fn row(title: impl Into<SharedString>, sub: impl Into<SharedString>, control: AnyElement) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap_6()
        .py_3()
        .border_b_1()
        .border_color(line())
        .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5().child(div().font_weight(FontWeight::MEDIUM).child(title.into())).child(div().text_sm().text_color(dim()).child(sub.into())))
        .child(control)
}

fn switch(id: &'static str, on: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_none()
        .w(px(42.))
        .h(px(24.))
        .p(px(3.))
        .rounded_full()
        .cursor_pointer()
        .bg(if on { accent() } else { hover() })
        .flex()
        .when(on, |d| d.justify_end())
        .child(div().size(px(18.)).rounded_full().bg(if on { accent_ink() } else { dim() }))
}

fn button(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_none()
        .px_4()
        .py_1p5()
        .rounded_full()
        .border_1()
        .border_color(line())
        .cursor_pointer()
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .hover(|s| s.bg(hover()))
        .child(label)
}

const SHORTCUTS: [(&str, &str); 16] = [
    ("Space", "Play / pause"),
    ("Ctrl + → / ←", "Next / previous song"),
    ("Shift + → / ←", "Skip 5 seconds"),
    ("Ctrl + ↑ / ↓", "Volume"),
    ("↑ / ↓, Enter", "Pick a song, play it"),
    ("Ctrl/Shift + click", "Select several songs"),
    ("Ctrl + A", "Select all songs"),
    ("Alt + ↑ / ↓", "Move selected songs (playlist)"),
    ("Delete", "Remove from playlist"),
    ("Ctrl + Z", "Undo a playlist change"),
    ("Ctrl + F", "Filter / search"),
    ("Ctrl + L", "Lyrics"),
    ("R", "Lyrics: original / both / romaji"),
    ("Ctrl + Q", "Queue"),
    ("Alt + ← / →", "Back / forward"),
    ("Ctrl + ,", "Settings"),
];

impl Shell {
    pub(super) fn settings_page(&self, lay: Layout, cx: &mut Context<Self>) -> AnyElement {
        let m = self.model.read(cx);
        let (normalize, scrobble) = (m.player_state.normalize, m.player_state.scrobble);
        let storage = m.storage;
        let (measured, waiting) = crate_core::loudness::progress();
        let norm_sub = if !normalize {
            "Off: songs play exactly as loud as they were mastered.".to_string()
        } else if waiting > 0 {
            format!("Plays every song at about the same loudness. Measuring your library in the background: {measured} songs done, {waiting} to go.")
        } else {
            format!("Plays every song at about the same loudness ({measured} songs measured).")
        };
        let pad = if lay.content_w < 640. { 20. } else { 40. };

        let playback = section("Playback")
            .child(row(
                "Normalize volume",
                norm_sub,
                switch("sw-norm", normalize).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.set_normalize(!normalize, cx)))).into_any_element(),
            ))
            .child(row(
                "Report plays to Navidrome",
                "Keeps play counts and \u{201c}Recently played\u{201d} up to date (and Last.fm, if Navidrome is set up for it).",
                switch("sw-scrobble", scrobble).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.set_scrobble(!scrobble, cx)))).into_any_element(),
            ));

        let appearance = self.appearance_section(cx);

        let library = self.library_section(cx);
        let accounts = self.accounts_section(cx);
        let storage_sec = section("Storage").child(row(
            "Downloaded audio",
            match storage {
                Some(s) => format!("{} of songs you played recently, so they start instantly. Kept under 1 GB, oldest first. Album art: {}.", mb(s.audio_bytes), mb(s.cover_bytes)),
                None => "Checking…".to_string(),
            },
            button("clear-audio", "Clear").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.clear_audio_cache(cx)))).into_any_element(),
        ));

        let keys = section("Keyboard shortcuts").child(div().flex().flex_wrap().children(SHORTCUTS.iter().map(|(k, what)| {
            div()
                .w(px(((lay.content_w - pad * 2.) / 2.).max(260.)))
                .flex()
                .items_center()
                .gap_3()
                .py_1p5()
                .child(div().w(px(150.)).flex_none().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(text()).child(*k))
                .child(div().text_sm().text_color(dim()).child(*what))
        })));

        let content = div()
            .id("settings")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.home_scroll)
            .px(px(pad))
            .py_7()
            .child(div().max_w(px(820.)).flex().flex_col().child(div().text_3xl().font_weight(FontWeight::BOLD).mb_8().child("Settings")).child(playback).child(library).child(accounts).child(appearance).child(storage_sec).child(keys));
        div().size_full().flex().flex_col().child(scroll_area(content, &self.home_scroll, &self.home_smooth, cx.entity_id())).into_any_element()
    }

    fn accounts_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        use crate::onboarding::Step;
        // reading these means files and a search of PATH: do it when the page opens (and every
        // few seconds while it stays open), not on every redraw
        let fresh = self.accounts.borrow().as_ref().is_some_and(|(t, _)| t.elapsed().as_secs() < 5);
        if !fresh {
            let missing = crate_core::setup::tools().into_iter().filter(|t| !t.found).map(|t| t.name).collect();
            *self.accounts.borrow_mut() = Some((std::time::Instant::now(), (crate_core::setup::current_login(), crate_core::setup::soulseek_account(), missing)));
        }
        let (login, slsk, missing) = self.accounts.borrow().as_ref().map(|(_, a)| a.clone()).unwrap_or_default();
        section("Accounts")
            .child(row(
                "Navidrome",
                match &login {
                    Some((url, user, _)) => format!("Signed in to {url} as {user}."),
                    None => "Not signed in.".into(),
                },
                button("acc-nd", "Change").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.go(Route::Setup(Step::Navidrome), cx))).into_any_element(),
            ))
            .child(row(
                "Soulseek",
                match &slsk {
                    Some(name) => format!("Downloads sign in as {name}."),
                    None => "No account yet, so songs can\u{2019}t be downloaded.".into(),
                },
                button("acc-sl", if slsk.is_some() { "Change" } else { "Set up" }).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.go(Route::Setup(Step::Soulseek), cx))).into_any_element(),
            ))
            .child(row(
                "Helper programs",
                if missing.is_empty() { "All installed.".to_string() } else { format!("Missing: {}.", missing.join(", ")) },
                button("acc-tools", "Check").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.go(Route::Setup(Step::Tools), cx))).into_any_element(),
            ))
            .child(row(
                "Import from Spotify",
                "Bring playlists over from Exportify (exportify.net) files.",
                button("acc-import", "Import\u{2026}").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.go(Route::Import, cx))).into_any_element(),
            ))
    }

    fn library_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        use crate::prefs::Fix;
        let m = self.model.read(cx);
        let fix = m.fix.clone();
        let (what, done, total) = m.fix_progress.lock().unwrap().clone();
        let auto = crate_core::fixer::auto();
        let last = crate_core::fixer::last_fix();
        let accent_btn = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .flex_none()
                .px_4()
                .py_1p5()
                .rounded_full()
                .cursor_pointer()
                .bg(accent())
                .text_color(accent_ink())
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .hover(|s| s.opacity(0.9))
                .child(label)
        };
        let intro = "Finds songs you have twice (by how they sound, so different spellings like 宇多田ヒカル / Hikaru Utada don't hide them), merges albums split across two names, and fills in missing covers and details. Shows you what it found before changing anything.";
        let main: AnyElement = match &fix {
            Fix::Idle => row("Fix library", intro, accent_btn("fix-check", "Check library").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.check_library(cx)))).into_any_element()).into_any_element(),
            Fix::Working => {
                let pct = if total > 0 { done as f32 / total as f32 } else { 0. };
                row(
                    "Fix library",
                    if total > 0 { format!("{what}\u{2026} {done} of {total}") } else { format!("{what}\u{2026}") },
                    div().w(px(160.)).h(px(6.)).rounded_full().bg(hover()).child(div().h_full().rounded_full().bg(accent()).w(px(160. * pct.clamp(0.03, 1.)))).into_any_element(),
                )
                .into_any_element()
            }
            Fix::Ready(plan) => self.fix_summary(plan, cx),
            Fix::Done(msg) | Fix::Failed(msg) => {
                let failed = matches!(fix, Fix::Failed(_));
                row(
                    "Fix library",
                    msg.clone(),
                    div()
                        .flex()
                        .gap_2()
                        .when(!failed && last.is_some(), |d| {
                            d.child(button("fix-undo", "Undo").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.undo_fix(cx)))))
                        })
                        .child(button("fix-again", "Check again").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.check_library(cx)))))
                        .into_any_element(),
                )
                .into_any_element()
            }
        };
        section("Library")
            .child(main)
            .when(matches!(fix, Fix::Idle) && last.is_some(), |d| {
                let when = last.map(|(_, t)| t).unwrap_or(0);
                let ago = crate::catalog::now() as u64 - when;
                let ago = if ago < 3600 { format!("{} min ago", ago / 60) } else if ago < 86400 { format!("{} h ago", ago / 3600) } else { format!("{} days ago", ago / 86400) };
                d.child(row(
                    "Undo the last fix",
                    format!("From {ago}. Puts moved songs, tags and playlists back."),
                    button("fix-undo2", "Undo").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.undo_fix(cx)))).into_any_element(),
                ))
            })
            .child(row(
                "Tidy new downloads",
                "Each new song is checked as it arrives: no second copy if you already have it, and it joins its album's existing spelling instead of starting a new one.",
                switch("sw-autofix", auto).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.set_auto_fix(!auto, cx)))).into_any_element(),
            ))
    }

    /// What "Fix library" found, with the button to go ahead.
    fn fix_summary(&self, plan: &crate_core::fixer::Plan, cx: &mut Context<Self>) -> AnyElement {
        let name = |rel: &str| -> String {
            let p = std::path::Path::new(rel);
            let file = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let artist = p.components().next().map(|c| c.as_os_str().to_string_lossy().to_string()).unwrap_or_default();
            format!("{file} \u{2014} {artist}")
        };
        let mut lines: Vec<(IconName, String)> = vec![];
        if !plan.dupes.is_empty() {
            lines.push((IconName::Music, format!(
                "{} song{} you have twice. The best-sounding copy stays, playlists switch to it, and the other moves to ~/music-dupes-review (not deleted).",
                plan.removed(), if plan.removed() == 1 { "" } else { "s" }
            )));
        }
        let merged = plan.count("merged split album");
        if merged > 0 {
            lines.push((IconName::Library, format!("{merged} song{} to join the rest of their album (it was split across two names).", if merged == 1 { "" } else { "s" })));
        }
        let covers = plan.count("cover");
        if covers > 0 {
            lines.push((IconName::Check, format!("{covers} missing cover{} to add.", if covers == 1 { "" } else { "s" })));
        }
        let details = plan.edits.iter().filter(|e| !e.why.contains("cover") && !e.why.contains("merged")).count();
        if details > 0 {
            lines.push((IconName::Pencil, format!("{details} song{} with missing details (album artist, track number, year).", if details == 1 { "" } else { "s" })));
        }
        for (a, b) in &plan.mismatched {
            lines.push((IconName::CircleAlert, format!("\u{201c}{}\u{201d} sounds exactly like \u{201c}{}\u{201d}: probably a wrong download. Left alone; you may want to download it again.", name(a), name(b))));
        }
        if !plan.repeats.is_empty() {
            let (pl, song) = &plan.repeats[0];
            let more = if plan.repeats.len() > 1 { format!(" and {} more", plan.repeats.len() - 1) } else { String::new() };
            lines.push((IconName::ListMinus, format!("{} song{} listed twice in the same playlist (\u{201c}{song}\u{201d} in {pl}{more}): the extra entry goes.", plan.repeats.len(), if plan.repeats.len() == 1 { "" } else { "s" })));
        }
        if !plan.unsure.is_empty() {
            lines.push((IconName::CircleAlert, format!("{} song{} without a cover that couldn't be matched online (left as is).", plan.unsure.len(), if plan.unsure.len() == 1 { "" } else { "s" })));
        }
        let details_open = self.fix_details;
        let dupes: Vec<AnyElement> = if details_open {
            plan.dupes
                .iter()
                .map(|g| {
                    div()
                        .flex()
                        .flex_col()
                        .py_1p5()
                        .border_b_1()
                        .border_color(line())
                        .child(div().text_sm().child(format!("Keep  {}", g.keep)))
                        .children(g.remove.iter().map(|r| div().text_sm().text_color(faint()).child(format!("Move  {r}"))))
                        .into_any_element()
                })
                .collect()
        } else {
            vec![]
        };
        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_5()
            .rounded_xl()
            .bg(panel())
            .border_1()
            .border_color(line())
            .child(div().font_weight(FontWeight::BOLD).text_lg().child(format!("Checked {} songs. Here\u{2019}s what can be fixed:", plan.files)))
            .children(lines.into_iter().map(|(ic, t)| div().w_full().flex().gap_3().child(div().flex_none().pt_0p5().child(icon(ic, 16., accent()))).child(div().flex_1().min_w_0().text_sm().child(t))))
            .when(!plan.dupes.is_empty(), |d| {
                d.child(
                    div()
                        .id("fix-details")
                        .text_sm()
                        .text_color(accent())
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .child(if details_open { "Hide the duplicate list" } else { "Show which copies stay and which move" })
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.fix_details = !this.fix_details;
                            cx.notify();
                        })),
                )
            })
            .when(details_open, |d| d.child(div().max_h(px(320.)).id("fix-list").overflow_y_scroll().children(dupes)))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .pt_1()
                    .child(
                        div()
                            .id("fix-apply")
                            .px_5()
                            .py_2()
                            .rounded_full()
                            .cursor_pointer()
                            .bg(accent())
                            .text_color(accent_ink())
                            .font_weight(FontWeight::SEMIBOLD)
                            .hover(|s| s.opacity(0.9))
                            .child(if plan.is_empty() { "OK" } else { "Fix it" })
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.apply_fix(cx)))),
                    )
                    .child(button("fix-cancel", "Cancel").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.cancel_fix(cx)))))
                    .child(div().flex_1())
                    .child(div().text_xs().text_color(faint()).self_center().child("Everything can be undone afterwards.")),
            )
            .into_any_element()
    }

    fn appearance_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let (cols, home) = {
            let m = self.model.read(cx);
            (m.show_cols, m.home_rows)
        };
        // theme tiles: a little preview of each scheme
        let cur_theme = crate::theme::theme_key();
        let themes = div().flex().flex_wrap().gap_3().children(THEMES.iter().map(|t| {
            let (key, on) = (t.key, t.key == cur_theme);
            let (bgc, sidec, textc) = t.preview();
            div()
                .id(SharedString::from(format!("theme-{key}")))
                .w(px(92.))
                .flex()
                .flex_col()
                .gap_1p5()
                .cursor_pointer()
                .child(
                    div()
                        .h(px(56.))
                        .rounded_lg()
                        .overflow_hidden()
                        .border_2()
                        .border_color(if on { accent() } else { line() })
                        .bg(gpui::rgb(bgc))
                        .flex()
                        .child(div().w(px(22.)).h_full().bg(gpui::rgb(sidec)))
                        .child(
                            div()
                                .flex_1()
                                .p_2()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(div().h(px(5.)).w(px(36.)).rounded_full().bg(gpui::rgb(textc)))
                                .child(div().h(px(5.)).w(px(24.)).rounded_full().bg(accent())),
                        ),
                )
                .child(div().text_xs().text_color(if on { text() } else { dim() }).child(t.name))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.set_theme(key, cx))))
        }));

        let cur = accent_name();
        let swatch = |id: String, fill: gpui::Div, on: bool, name: &'static str| {
            fill.id(SharedString::from(id))
                .size(px(32.))
                .rounded_full()
                .cursor_pointer()
                .border_2()
                .border_color(if on { text() } else { gpui::rgba(0) })
                .tooltip(tip(name))
                .flex()
                .items_center()
                .justify_center()
        };
        let mut swatches: Vec<AnyElement> = ACCENTS
            .iter()
            .map(|&(key, name, rgb)| {
                let on = key == cur;
                swatch(format!("acc-{key}"), div().bg(gpui::rgb(rgb)), on, name)
                    .when(on, |d| d.child(icon(IconName::Check, 15., accent_ink())))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.set_accent(key, cx))))
                    .into_any_element()
            })
            .collect();
        let art_on = cur == "art";
        swatches.push(
            swatch(
                "acc-art".into(),
                div().bg(gpui::linear_gradient(135., gpui::linear_color_stop(gpui::rgb(0xff6b9a), 0.), gpui::linear_color_stop(gpui::rgb(0x5b9dff), 1.))),
                art_on,
                "From the album art",
            )
            .child(icon(if art_on { IconName::Check } else { IconName::Music }, 15., gpui::white()))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.set_accent("art", cx))))
            .into_any_element(),
        );

        let segs = |id: &'static str, options: Vec<(String, String)>, cur: String, cx: &mut Context<Self>, act: Rc<dyn Fn(&mut Shell, &str, &mut Context<Shell>)>| {
            div().flex().p_1().rounded_full().bg(panel2()).children(options.into_iter().map(|(key, label)| {
                let on = key == cur;
                let act = act.clone();
                div()
                    .id(SharedString::from(format!("{id}-{key}")))
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .cursor_pointer()
                    .when(on, |d| d.bg(text()).text_color(on_text()))
                    .when(!on, |d| d.text_color(dim()).hover(|s| s.text_color(text())))
                    .child(label)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| act(this, &key, cx)))
            }))
        };
        let sizes = segs(
            "txt",
            TEXT_SIZES.iter().map(|p| (p.to_string(), format!("{p}%"))).collect(),
            crate::theme::text_pct().to_string(),
            cx,
            Rc::new(|this, k, cx| {
                let p = k.parse().unwrap_or(100);
                this.model.update(cx, |m, cx| m.set_text_size(p, cx));
            }),
        );
        let density = segs(
            "dens",
            DENSITIES.iter().map(|d| (d.0.to_string(), d.1.to_string())).collect(),
            crate::theme::density_key().to_string(),
            cx,
            Rc::new(|this, k, cx| {
                let k = k.to_string();
                this.model.update(cx, |m, cx| m.set_density(&k, cx));
            }),
        );
        let anim = crate::theme::animations();
        section("Appearance")
            .child(row("Theme", "Colours of the window.", themes.into_any_element()))
            .child(row(
                "Accent colour",
                if art_on { "Follows the cover of the song that's playing." } else { "Buttons, highlights and the playing song. The last one follows the album art." },
                div().flex().flex_wrap().justify_end().max_w(px(330.)).gap_2p5().children(swatches).into_any_element(),
            ))
            .child(row("Text size", "Makes all text bigger or smaller.", sizes.into_any_element()))
            .child(row("Song rows", "How tightly lists of songs are packed.", density.into_any_element()))
            .child(row(
                "Album column",
                "In playlists and your library (shown when the window is wide enough).",
                switch("sw-col-album", cols[0]).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.set_col(0, !cols[0], cx)))).into_any_element(),
            ))
            .child(row(
                "Date added column",
                "When each song joined the playlist or library.",
                switch("sw-col-date", cols[1]).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.set_col(1, !cols[1], cx)))).into_any_element(),
            ))
            .child(row(
                "Home: Jump back in",
                "Albums you played recently.",
                switch("sw-home-recent", home[0]).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.set_home_row(0, !home[0], cx)))).into_any_element(),
            ))
            .child(row(
                "Home: Your most played",
                "Albums you play the most.",
                switch("sw-home-freq", home[1]).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.set_home_row(1, !home[1], cx)))).into_any_element(),
            ))
            .child(row(
                "Animations",
                "Pages fade in, panels slide, covers cross-fade, the playing song gets moving bars, lyrics glow in. Off: everything changes instantly and uses a little less CPU.",
                switch("sw-anim", anim).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.set_animations(!anim, cx)))).into_any_element(),
            ))
    }
}
