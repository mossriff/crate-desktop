//! "Import from Spotify" page: pick Exportify CSV files, see what each playlist needs, import.
use super::*;
use crate::importer::Importing;
use crate::onboarding::{primary, secondary};

impl Shell {
    fn choose_csvs(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("Import".into()) });
        let model = self.model.clone();
        cx.spawn(async move |_, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                // a folder's worth of exports may include other files; only CSVs are playlists
                let csvs: Vec<PathBuf> = paths.into_iter().filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("csv"))).collect();
                let _ = model.update(cx, |m, cx| {
                    if csvs.is_empty() {
                        m.toast("Pick the .csv files from Exportify", true, cx);
                    } else {
                        m.import_read(csvs, cx);
                    }
                });
            }
        })
        .detach();
    }

    pub(super) fn import_page(&mut self, lay: Layout, cx: &mut Context<Self>) -> AnyElement {
        let (state, progress) = {
            let m = self.model.read(cx);
            (m.import.clone(), m.import_progress.lock().unwrap().clone())
        };
        let pad = if lay.content_w < 640. { 20. } else { 40. };
        let busy = |text: String| div().flex().items_center().gap_3().py_4().child(icon(IconName::LoaderCircle, 18., accent())).child(div().text_color(dim()).child(text));
        let body: AnyElement = match state {
            Importing::Idle => div()
                .flex()
                .flex_col()
                .gap_4()
                .child(div().text_color(dim()).child("Rebuild your Spotify playlists here. Songs you already have go straight in; the rest are found in the catalog and downloaded into the playlist (they join the end as they finish)."))
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
                        .child("3.  Unzip it, then choose the .csv files below (as many as you like)."),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(primary("imp-choose", "Choose CSV files\u{2026}").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.choose_csvs(cx))))
                        .child(secondary("imp-exportify", "Open exportify.net").on_click(|_, _, cx| cx.open_url("https://exportify.net"))),
                )
                .into_any_element(),
            Importing::Reading | Importing::Running => busy(progress).into_any_element(),
            Importing::Failed(e) => div()
                .flex()
                .flex_col()
                .gap_4()
                .child(div().flex().gap_2().child(icon(IconName::CircleAlert, 18., gpui::red())).child(div().flex_1().child(e)))
                .child(div().flex().child(secondary("imp-again", "Try other files").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.import_reset(cx))))))
                .into_any_element(),
            Importing::Preview(plans, picks, skipped) => {
                let chosen = picks.iter().filter(|p| **p).count();
                let (have, missing): (usize, usize) = plans.iter().zip(&picks).filter(|(_, on)| **on).fold((0, 0), |(h, mi), (p, _)| (h + p.have(), mi + p.missing()));
                let no_slsk = crate_core::setup::soulseek_account().is_none();
                let rows = plans.iter().enumerate().map(|(i, p)| {
                    let on = picks[i];
                    div()
                        .id(("imp-row", i))
                        .flex()
                        .items_center()
                        .gap_4()
                        .py_3()
                        .px_2()
                        .rounded_lg()
                        .cursor_pointer()
                        .hover(|s| s.bg(hover()))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.import_toggle(i, cx))))
                        .child(
                            div()
                                .size(px(20.))
                                .flex_none()
                                .rounded_md()
                                .border_2()
                                .border_color(if on { accent() } else { faint() })
                                .when(on, |d| d.bg(accent()))
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(on, |d| d.child(icon(IconName::Check, 14., accent_ink()))),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(div().truncate().font_weight(FontWeight::SEMIBOLD).child(p.name.clone()))
                                .child(div().text_sm().text_color(dim()).truncate().child(format!(
                                    "{} songs \u{00b7} {} in your library \u{00b7} {} to download{}",
                                    p.songs.len(),
                                    p.have(),
                                    p.missing(),
                                    if p.existing.is_some() { " \u{00b7} adds to your playlist with this name" } else { "" }
                                ))),
                        )
                });
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_color(dim()).mb_2().child(format!("{} playlist{} found. Untick any you don\u{2019}t want.", plans.len(), if plans.len() == 1 { "" } else { "s" })))
                    .children(rows)
                    .when(!skipped.is_empty(), |d| {
                        d.child(div().mt_2().text_sm().text_color(faint()).child(format!("Skipped: {}", skipped.join("; "))))
                    })
                    .when(no_slsk && missing > 0, |d| {
                        d.child(
                            div()
                                .mt_2()
                                .flex()
                                .items_center()
                                .gap_3()
                                .p_3()
                                .rounded_lg()
                                .bg(panel2())
                                .child(icon(IconName::CircleAlert, 16., accent()))
                                .child(div().flex_1().min_w_0().text_sm().child("Downloading needs a Soulseek account. Songs you have are added now; set up Soulseek and the rest can download."))
                                .child(secondary("imp-slsk", "Set up").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.go(Route::Setup(crate::onboarding::Step::Soulseek), cx)))),
                        )
                    })
                    .child(
                        div()
                            .mt_4()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                primary("imp-go", "Import")
                                    .when(chosen == 0, |d| d.opacity(0.4))
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.import_run(cx)))),
                            )
                            .child(secondary("imp-cancel", "Cancel").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.import_reset(cx)))))
                            .child(div().flex_1())
                            .child(div().text_sm().text_color(dim()).child(format!("{have} in your library, {missing} to download"))),
                    )
                    .into_any_element()
            }
            Importing::Done(results) => {
                let open = self.import_details;
                let not_found: usize = results.iter().map(|(_, r)| r.not_found.len()).sum();
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children(results.iter().map(|(name, r)| {
                        div()
                            .flex()
                            .flex_col()
                            .py_2()
                            .border_b_1()
                            .border_color(line())
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(name.clone()))
                            .child(div().text_sm().text_color(dim()).child(match &r.error {
                                Some(e) => format!("Not imported: {e}"),
                                None => format!(
                                    "{} added \u{00b7} {} downloading{}",
                                    r.added,
                                    r.queued.len(),
                                    if r.not_found.is_empty() { String::new() } else { format!(" \u{00b7} {} not found", r.not_found.len()) }
                                ),
                            }))
                            .when(open, |d| {
                                d.children(r.not_found.iter().map(|s| div().text_sm().text_color(faint()).child(format!("{} \u{2014} {}", s.title, s.artist))))
                            })
                    }))
                    .when(not_found > 0, |d| {
                        d.child(
                            div()
                                .id("imp-details")
                                .text_sm()
                                .text_color(accent())
                                .cursor_pointer()
                                .hover(|s| s.underline())
                                .child(if open { "Hide the songs that weren\u{2019}t found" } else { "Show the songs that weren\u{2019}t found (you can search for them by hand)" })
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.import_details = !this.import_details;
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        div()
                            .mt_2()
                            .flex()
                            .gap_2()
                            .child(primary("imp-dl", "Show downloads").on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.model.update(cx, |m, cx| {
                                    if m.panel != Panel::Downloads {
                                        m.toggle_panel(Panel::Downloads, cx)
                                    }
                                })
                            })))
                            .child(secondary("imp-more", "Import more").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.import_reset(cx))))),
                    )
                    .into_any_element()
            }
        };
        let content = div()
            .id("import")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.home_scroll)
            .px(px(pad))
            .py_7()
            .child(div().max_w(px(760.)).flex().flex_col().child(div().text_3xl().font_weight(FontWeight::BOLD).mb_6().child("Import from Spotify")).child(body));
        div().size_full().flex().flex_col().child(scroll_area(content, &self.home_scroll, &self.home_smooth, cx.entity_id())).into_any_element()
    }

    /// A setup step opened from Settings (helpers, Navidrome, Soulseek).
    pub(super) fn setup_page(&mut self, step: crate::onboarding::Step, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.setup.as_ref().map(|s| s.0) != Some(step) {
            let ob = cx.new(|cx| crate::onboarding::Onboarding::new(step, true, window, cx));
            let sub = cx.subscribe(&ob, |this, _, ev: &crate::onboarding::SetupDone, cx| {
                let changed = ev.server_changed;
                this.setup = None;
                this.model.update(cx, |m, cx| {
                    m.back(cx);
                    if changed {
                        m.toast("Saved. Restart Crate to use the new server.", false, cx);
                    }
                });
            });
            self.setup = Some((step, ob, sub));
        }
        self.setup.as_ref().map(|s| s.1.clone().into_any_element()).unwrap_or_else(|| div().into_any_element())
    }
}
