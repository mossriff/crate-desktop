//! Live lyrics: the current line lights up in time with the song and glides to just above the
//! middle of the screen. Japanese lyrics can show the original, romaji under each line, or romaji
//! only. Its own view, so the 20 checks a second for "which line is it?" never redraw the rest of
//! the window.
use crate::covers::LARGE;
use crate::model::{LyrMode, LyricsState, Model};
use crate::shell::{icon, scroll_area};
use crate::smooth::SmoothScroll;
use crate::theme::*;
use crate_core::player::Action;
use gpui::{div, img, AnimationExt, point, prelude::*, px, ClickEvent, Context, Entity, FontWeight, ObjectFit, ScrollHandle, Subscription, Task, Window};
use gpui_kit_assets::IconName;
use std::time::{Duration, Instant};

/// Where the current line settles, as a fraction of the visible height.
const ANCHOR: f32 = 0.38;
/// Show a line slightly before it's sung, like karaoke.
const EARLY: f64 = 0.15;
/// After you scroll the lyrics yourself, leave them alone this long.
const HANDS_OFF: Duration = Duration::from_secs(4);

pub struct LyricsView {
    model: Entity<Model>,
    scroll: ScrollHandle,
    smooth: SmoothScroll,
    /// highlighted line and the song it belongs to
    cur: Option<usize>,
    track: Option<String>,
    /// auto-scroll animation target (scroll offset y) and the offset we last set
    target: Option<f32>,
    last_set: Option<f32>,
    last_frame: Instant,
    user_scrolled: Option<Instant>,
    _subs: Vec<Subscription>,
    _ticker: Task<()>,
}

impl LyricsView {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let subs = vec![cx.observe(&model, |_, _, cx| cx.notify())];
        // which line are we on? (cheap: a binary-search-sized loop, 20x a second, only while visible)
        let ticker = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(50)).await;
            if this.update(cx, |v, cx| v.tick(cx)).is_err() {
                break;
            }
        });
        LyricsView {
            model,
            scroll: ScrollHandle::new(),
            smooth: SmoothScroll::default(),
            cur: None,
            track: None,
            target: None,
            last_set: None,
            last_frame: Instant::now(),
            user_scrolled: None,
            _subs: subs,
            _ticker: ticker,
        }
    }

    fn lyrics(&self, cx: &gpui::App) -> Option<(String, LyricsState)> {
        self.model.read(cx).lyrics.clone()
    }

    /// Index of the line being sung now.
    fn line_now(&self, cx: &gpui::App) -> Option<usize> {
        let m = self.model.read(cx);
        if m.route != crate::model::Route::Lyrics {
            return None;
        }
        let Some((_, LyricsState::Found(l))) = &m.lyrics else { return None };
        if !l.synced {
            return None;
        }
        let pos = m.clock.read(cx).position() + EARLY;
        let mut idx = None;
        for (i, line) in l.lines.iter().enumerate() {
            match line.time {
                Some(t) if t <= pos => idx = Some(i),
                Some(_) => break,
                None => {}
            }
        }
        idx
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        // notice the user scrolling by hand (the offset moved and it wasn't us)
        if let Some(set) = self.last_set {
            if self.target.is_none() && (f32::from(self.scroll.offset().y) - set).abs() > 2. {
                self.user_scrolled = Some(Instant::now());
                self.last_set = None;
            }
        }
        let track = self.lyrics(cx).map(|(id, _)| id);
        let now = self.line_now(cx);
        if track != self.track {
            self.track = track;
            self.cur = None;
            self.user_scrolled = None;
            self.scroll.set_offset(point(px(0.), px(0.)));
            cx.notify();
        }
        if now != self.cur {
            self.cur = now;
            cx.notify();
            let hands_off = self.user_scrolled.map(|t| t.elapsed() < HANDS_OFF).unwrap_or(false);
            if !hands_off {
                if let Some(i) = now {
                    self.scroll_to_line(i, cx);
                }
            }
        }
    }

    fn scroll_to_line(&mut self, i: usize, cx: &mut Context<Self>) {
        let (Some(item), view) = (self.scroll.bounds_for_item(i), self.scroll.bounds()) else { return };
        // gpui reports child positions as laid out, *before* scrolling, so the offset that puts
        // the line at the anchor doesn't depend on where we are scrolled to now. (Treating it as
        // already scrolled subtracted the current offset twice, so the further into the song,
        // the further the lyrics overshot.)
        let item_top = f32::from(item.origin.y - view.origin.y);
        let max = f32::from(self.scroll.max_offset().y);
        let want = (f32::from(view.size.height) * ANCHOR - item_top).clamp(-max, 0.);
        let animating = self.target.is_some();
        self.target = Some(want);
        if !animating {
            self.last_frame = Instant::now();
            cx.notify(); // the frame loop below starts from render
        }
    }

    /// One step of the glide towards `target`; keeps requesting frames until it arrives.
    fn animate(&mut self, window: &mut Window) {
        let Some(target) = self.target else { return };
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.05);
        self.last_frame = now;
        let cur = f32::from(self.scroll.offset().y);
        let next = cur + (target - cur) * (1. - (-dt / 0.12).exp());
        let done = (target - next).abs() < 0.5;
        let y = if done { target } else { next };
        self.scroll.set_offset(point(px(0.), px(y)));
        self.last_set = Some(y);
        if done {
            self.target = None;
        } else {
            window.request_animation_frame();
        }
    }
}

impl Render for LyricsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.animate(window);
        let m = self.model.read(cx);
        let Some(t) = m.current().cloned() else {
            return div().size_full().flex().items_center().justify_center().text_color(dim()).text_lg().child("Play something to see its lyrics.").into_any_element();
        };
        let cover = m.covers.get(t.cover_art.as_deref(), LARGE);
        let mode = m.lyr_mode;
        let state = m.lyrics.as_ref().filter(|(id, _)| id == &t.id).map(|(_, s)| s.clone()).unwrap_or(LyricsState::Loading);
        let found = match &state {
            LyricsState::Found(l) => Some(l.clone()),
            _ => None,
        };
        let japanese = found.as_ref().map(|l| l.japanese).unwrap_or(false);

        let seg = |m: LyrMode, label: &'static str, cx: &mut Context<Self>| {
            let on = mode == m;
            div()
                .id(SharedStr::from(m.as_str()))
                .px_3()
                .py_1()
                .rounded_full()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .cursor_pointer()
                .when(on, |d| d.bg(text()).text_color(on_text()))
                .when(!on, |d| d.text_color(dim()).hover(|s| s.text_color(text())))
                .child(label)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.model.update(cx, |md, cx| md.set_lyr_mode(m, cx))))
        };
        let top = div()
            .flex_none()
            .flex()
            .items_center()
            .gap_4()
            .px_8()
            .pt_6()
            .pb_4()
            .child(match cover {
                Some(p) => img(p).size(px(64.)).rounded(px(8.)).object_fit(ObjectFit::Cover).flex_none().into_any_element(),
                None => div().size(px(64.)).rounded(px(8.)).bg(panel2()).flex_none().into_any_element(),
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().truncate().text_xl().font_weight(FontWeight::BOLD).child(t.title.clone()))
                    .child(div().truncate().text_color(dim()).child(t.artist.clone())),
            )
            .when(japanese, |d| {
                d.child(
                    div()
                        .flex()
                        .p_1()
                        .rounded_full()
                        .bg(panel2())
                        .child(seg(LyrMode::Original, "日本語", cx))
                        .child(seg(LyrMode::Both, "Both", cx))
                        .child(seg(LyrMode::Romaji, "Romaji", cx)),
                )
            })
            .when_some(found.as_ref(), |d, l| d.child(div().flex_none().text_xs().text_color(faint()).child(format!("{} \u{00b7} {}", if l.synced { "Synced" } else { "Unsynced" }, l.source))));

        let body = match state {
            LyricsState::Loading => div().flex_1().flex().items_center().justify_center().text_color(dim()).child("Loading lyrics…").into_any_element(),
            LyricsState::Missing => div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(icon(IconName::Mic, 28., faint()))
                .child(div().text_lg().text_color(dim()).child("No lyrics found for this song."))
                .child(div().text_sm().text_color(faint()).child("Looked on LRCLIB and in the file itself."))
                .into_any_element(),
            LyricsState::Found(l) => {
                let cur = self.cur;
                let synced = l.synced;
                let view_h = f32::from(window.viewport_size().height);
                let lines = l.lines.iter().enumerate().map(|(i, line)| {
                    let text_s = line.text.trim().to_string();
                    if text_s.is_empty() {
                        return div().id(("ly", i)).h(px(28.)).into_any_element();
                    }
                    let (main, sub) = match (mode, &line.romaji) {
                        (LyrMode::Romaji, Some(r)) => (r.clone(), None),
                        (LyrMode::Both, Some(r)) => (text_s, Some(r.clone())),
                        _ => (text_s, None),
                    };
                    let color = match (synced, cur) {
                        (false, _) => text(),
                        (true, Some(c)) if c == i => text(),
                        (true, Some(c)) if i < c => text_a(0x66),
                        _ => text_a(0x38),
                    };
                    let time = line.time;
                    div()
                        .id(("ly", i))
                        .py_2()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .when(synced && time.is_some(), |d| d.cursor_pointer().hover(|s| s.opacity(0.85)))
                        .child({
                            let line = div().text_3xl().font_weight(FontWeight::BOLD).text_color(color).child(main);
                            if synced && cur == Some(i) {
                                // the line being sung brightens in rather than switching instantly
                                line.with_animation(
                                    ("ly-now", i),
                                    gpui::Animation::new(Duration::from_millis(260)).with_easing(gpui::ease_out_quint()),
                                    |d, t| d.opacity(0.45 + 0.55 * t),
                                )
                                .into_any_element()
                            } else {
                                line.into_any_element()
                            }
                        })
                        .when_some(sub, |d, r| d.child(div().text_lg().font_weight(FontWeight::MEDIUM).text_color(color).opacity(0.75).child(r)))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            if let (true, Some(secs)) = (synced, time) {
                                this.user_scrolled = None;
                                let model = this.model.clone();
                                model.update(cx, |m, cx| {
                                    m.send(Action::Seek { secs });
                                    m.clock.update(cx, |c, _| {
                                        c.progress.position = secs;
                                        c.at = Instant::now();
                                    });
                                });
                                this.tick(cx);
                            }
                        }))
                        .into_any_element()
                });
                let content = div()
                    .id("lyr-lines")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .px_8()
                    // room above/below so the first and last lines can reach the anchor point
                    .pt(px(if synced { view_h * 0.12 } else { 16. }))
                    .pb(px(view_h * 0.5))
                    .children(lines);
                scroll_area(content, &self.scroll, &self.smooth, cx.entity_id()).into_any_element()
            }
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(gpui::linear_gradient(180., gpui::linear_color_stop(accent_wash(), 0.), gpui::linear_color_stop(bg(), 0.6)))
            .child(top)
            .child(body)
            .into_any_element()
    }
}

type SharedStr = gpui::SharedString;
