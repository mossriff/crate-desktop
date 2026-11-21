//! The Downloads tab of the right-hand panel: Soulseek's search allowance (with the break
//! countdown) and every download's progress.
use super::*;
use crate::catalog::{now, status_text};
use crate_core::downloads::{Source, Status};

fn mmss(s: f64) -> String {
    let s = s.max(0.).ceil() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

impl Shell {
    pub(super) fn downloads_body(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let m = self.model.read(cx);
        let t = now() - m.skew;
        let mut out = vec![];
        let paused = m.limiter.as_ref().and_then(|l| l.paused_until).map(|p| p > t).unwrap_or(false);
        if let Some(l) = &m.limiter {
            let pct = (l.used as f32 / l.limit.max(1) as f32).min(1.);
            let meter = div().mt_2().h(px(4.)).w_full().rounded_full().bg(line()).child(div().h_full().w(gpui::relative(pct)).rounded_full().bg(if paused { accent() } else { dim() }));
            let text_el = if paused {
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(text()).child("Taking a break so Soulseek doesn't ban you"))
                    .child(format!("Resuming in {} · {}/{} searches used", mmss(l.paused_until.unwrap_or(t) - t), l.used, l.limit))
            } else {
                let mut s = format!("Soulseek searches: {}/{} in the last {:.1} min", l.used, l.limit, l.window / 60.);
                if let Some(nf) = l.next_free.filter(|nf| l.used > 0 && *nf > t) {
                    s.push_str(&format!(" · next frees up in {}", mmss(nf - t)));
                }
                div().child(s)
            };
            out.push(
                div()
                    .mx_2()
                    .mt_2()
                    .mb_2()
                    .p_3()
                    .rounded_lg()
                    .bg(if paused { accent_wash() } else { panel() })
                    .text_xs()
                    .text_color(dim())
                    .child(text_el)
                    .child(meter)
                    .into_any_element(),
            );
        }
        if m.jobs.is_empty() {
            out.push(
                div()
                    .px_2()
                    .py_4()
                    .text_sm()
                    .text_color(dim())
                    .child("No downloads yet. Search for something and drag it onto a playlist.")
                    .into_any_element(),
            );
            return out;
        }
        let names: std::collections::HashMap<&str, &str> = m.playlists.iter().map(|p| (p.id.as_str(), p.name.as_str())).collect();
        for j in &m.jobs {
            let failed = j.status == Status::Failed;
            let busy = j.status.active();
            let pls: Vec<&str> = j.playlists.iter().filter_map(|id| names.get(id.as_str()).copied()).collect();
            let src = j.source.map(|s| match s {
                Source::Soulseek => ("Soulseek", gpui::rgb(0x6aa7ff)),
                Source::Youtube => ("YouTube", gpui::rgb(0xff6b6b)),
                Source::Library => ("Owned", gpui::rgb(0x5cc98a)),
            });
            let show_msg = j.message.is_some() && !(j.status == Status::Done && j.source == Some(Source::Library));
            let (id_r, id_d) = (j.id.clone(), j.id.clone());
            out.push(
                div()
                    .id(SharedString::from(format!("job-{}", j.id)))
                    .group("job")
                    .flex()
                    .items_start()
                    .gap_2p5()
                    .px_2()
                    .py_2()
                    .rounded_lg()
                    .hover(|s| s.bg(hover()))
                    .child(cover_el(m.covers.get_url(j.track.cover_small.as_deref().or(j.track.cover.as_deref())), 40., 4.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .child(div().truncate().text_sm().font_weight(FontWeight::MEDIUM).child(format!("{} \u{00b7} {}", j.track.title, j.track.artist)))
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap_1p5()
                                    .text_xs()
                                    .text_color(if failed { gpui::rgb(0xff7b7b) } else { dim() })
                                    .child(status_text(j, paused))
                                    .when_some(src, |d, (label, color)| d.child(div().px_1p5().rounded(px(4.)).border_1().border_color(color).text_color(color).child(label)))
                                    .when(!pls.is_empty(), |d| d.child(div().truncate().child(format!("\u{2192} {}", pls.join(", "))))),
                            )
                            .when(show_msg, |d| d.child(div().text_xs().text_color(faint()).child(j.message.clone().unwrap_or_default())))
                            .when(busy, |d| {
                                let pct = j.progress.map(|p| (p / 100.) as f32);
                                d.child(
                                    div().mt_1().h(px(3.)).w_full().rounded_full().bg(line()).child(
                                        div()
                                            .h_full()
                                            .rounded_full()
                                            .bg(accent())
                                            .w(gpui::relative(pct.unwrap_or(0.25)))
                                            .when(pct.is_none(), |d| d.opacity(0.5)),
                                    ),
                                )
                            }),
                    )
                    .when(failed, |d| {
                        d.child(
                            div()
                                .id(SharedString::from(format!("retry-{}", j.id)))
                                .p_1()
                                .rounded_md()
                                .cursor_pointer()
                                .hover(|s| s.bg(panel2()))
                                .tooltip(tip("Retry"))
                                .child(icon(IconName::RotateCw, 14., dim()))
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.model.read(cx).retry_download(&id_r))),
                        )
                    })
                    .when(!busy, |d| {
                        d.child(
                            div()
                                .id(SharedString::from(format!("dismiss-{}", j.id)))
                                .p_1()
                                .rounded_md()
                                .cursor_pointer()
                                .invisible()
                                .group_hover("job", |s| s.visible())
                                .hover(|s| s.bg(panel2()))
                                .tooltip(tip("Dismiss"))
                                .child(icon(IconName::X, 14., dim()))
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.model.read(cx).dismiss_download(&id_d))),
                        )
                    })
                    .into_any_element(),
            );
        }
        out
    }
}
