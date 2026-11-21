//! The player bar. It's its own view so position updates (≈10/s while playing) only redraw
//! this strip, not the whole window.
use crate::covers::SMALL;
use crate::model::{Model, Panel};
use crate::shell::icon;
use crate::theme::*;
use crate_core::player::{Action, Repeat};
use gpui::AnimationExt;
use gpui::{div, img, prelude::*, px, ClickEvent, Context, Entity, FontWeight, ObjectFit, SharedString, Subscription, Task, Window};
use gpui_component::tooltip::Tooltip;
use gpui_component::slider::{Slider, SliderEvent, SliderState, SliderValue};
use gpui_kit_assets::IconName;

pub struct PlayerBar {
    model: Entity<Model>,
    seek: Entity<SliderState>,
    volume: Entity<SliderState>,
    /// Position being dragged to (seconds), while the user holds the seek slider.
    seeking: Option<f64>,
    _subs: Vec<Subscription>,
    _ticker: Task<()>,
}

struct Flags {
    playing: bool,
    volume: f32,
    muted: bool,
    shuffle: bool,
    repeat: Repeat,
}

fn single(v: SliderValue) -> f32 {
    match v {
        SliderValue::Single(x) => x,
        SliderValue::Range(_, x) => x,
    }
}

impl PlayerBar {
    pub fn new(model: Entity<Model>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let vol0 = model.read(cx).player_state.volume;
        let seek = cx.new(|_| SliderState::new().min(0.).max(1.).step(0.0005).default_value(0.));
        let volume = cx.new(|_| SliderState::new().min(0.).max(1.).step(0.01).default_value(vol0));
        let clock = model.read(cx).clock.clone();
        let subs = vec![
            cx.observe(&model, |_, _, cx| cx.notify()),
            cx.observe(&clock, |_, _, cx| cx.notify()),
            cx.subscribe(&seek, |this: &mut Self, _, ev: &SliderEvent, cx| {
                let dur = this.model.read(cx).clock.read(cx).progress.duration;
                match ev {
                    SliderEvent::Change(v) => this.seeking = Some(single(*v) as f64 * dur),
                    SliderEvent::Release(v) => {
                        let secs = single(*v) as f64 * dur;
                        this.model.read(cx).send(Action::Seek { secs });
                        this.model.read(cx).clock.clone().update(cx, |c, _| {
                            c.progress.position = secs;
                            c.at = std::time::Instant::now();
                        });
                        this.seeking = None;
                    }
                }
                cx.notify();
            }),
            cx.subscribe(&volume, |this: &mut Self, _, ev: &SliderEvent, cx| {
                let (SliderEvent::Change(v) | SliderEvent::Release(v)) = ev;
                let value = single(*v);
                // The engine doesn't broadcast volume changes (they'd flood the UI while dragging),
                // so update our copy directly — otherwise the next redraw snaps the slider back.
                this.model.update(cx, |m, _| {
                    m.player_state.volume = value;
                    m.player_state.muted = false;
                    m.send(Action::SetVolume { value });
                });
                cx.notify();
            }),
        ];
        // No timer of its own: the engine's position updates (4 a second) redraw the bar. Every
        // redraw rebuilds the whole window in gpui, so an extra 10/s timer cost ~2% CPU while playing.
        let ticker = Task::ready(());
        PlayerBar { model, seek, volume, seeking: None, _subs: subs, _ticker: ticker }
    }
}

fn ctl(id: &'static str, tip: impl Into<SharedString>, ic: IconName, size: f32, color: gpui::Rgba) -> gpui::Stateful<gpui::Div> {
    let tip: SharedString = tip.into();
    div()
        .id(id)
        .tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
        .size(px(36.))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|s| s.bg(hover()))
        .child(icon(ic, size, color))
}

impl Render for PlayerBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = self.model.read(cx);
        // copy just the flags; cloning PlayerState would copy the whole queue on every tick
        let ps = &m.player_state;
        let st = Flags { playing: ps.playing, volume: ps.volume, muted: ps.muted, shuffle: ps.shuffle, repeat: ps.repeat };
        let cur = m.current().cloned();
        let clock = m.clock.read(cx);
        let dur = if clock.progress.duration > 0.0 { clock.progress.duration } else { cur.as_ref().map(|t| t.duration).unwrap_or(0.0) };
        let pos = self.seeking.unwrap_or_else(|| clock.position());
        let cover = cur.as_ref().and_then(|t| m.covers.get(t.cover_art.as_deref(), SMALL));
        let queue_open = m.panel == Panel::Queue;
        let dl_open = m.panel == Panel::Downloads;
        let lyrics_open = m.route == crate::model::Route::Lyrics;
        let n_active = m.active_downloads();

        // keep sliders in sync with the engine (but don't fight the user while dragging)
        if self.seeking.is_none() {
            let f = if dur > 0.0 { (pos / dur) as f32 } else { 0.0 };
            if (single(self.seek.read(cx).value()) - f).abs() > 0.0005 {
                self.seek.update(cx, |s, cx| s.set_value(f, window, cx));
            }
        }
        let vol = if st.muted { 0.0 } else { st.volume };
        if (single(self.volume.read(cx).value()) - vol).abs() > 0.005 {
            self.volume.update(cx, |s, cx| s.set_value(vol, window, cx));
        }

        let repeat_icon = if st.repeat == Repeat::One { IconName::Repeat1 } else { IconName::Repeat };
        let next_repeat = match st.repeat {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        };
        let shuffle_on = st.shuffle;
        let muted = st.muted || st.volume == 0.0;
        // squish gracefully: side columns shrink with the window, volume slider goes first
        let win_w = f32::from(window.viewport_size().width);
        let side_w = (win_w * 0.24).clamp(150., 300.);
        let show_vol = win_w >= 820.;
        let show_cover = win_w >= 640.;
        let vol_icon = if muted { IconName::VolumeX } else if st.volume < 0.5 { IconName::Volume1 } else { IconName::Volume2 };

        div()
            .h(px(92.))
            .flex_none()
            .flex()
            .items_center()
            .gap_4()
            .px_4()
            .bg(bar())
            .border_t_1()
            .border_color(line())
            // now playing
            .child(
                div()
                    .w(px(side_w))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_3p5()
                    .min_w_0()
                    .children(cur.as_ref().map(|t| {
                        div()
                            .flex()
                            .items_center()
                            .gap_3p5()
                            .min_w_0()
                            .when(show_cover, |d| d.child(match cover {
                                // new song: its cover fades in
                                Some(p) => img(p).size(px(58.)).rounded(px(6.)).object_fit(ObjectFit::Cover).flex_none().with_animation(
                                    gpui::SharedString::from(format!("barcover-{}", cur.as_ref().map(|t| t.id.as_str()).unwrap_or(""))),
                                    gpui::Animation::new(std::time::Duration::from_millis(280)).with_easing(gpui::ease_out_quint()),
                                    |d, t| d.opacity(t),
                                ).into_any_element(),
                                None => div().size(px(58.)).rounded(px(6.)).bg(panel2()).flex_none().into_any_element(),
                            }))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .min_w_0()
                                    .child(div().truncate().font_weight(FontWeight::SEMIBOLD).child(t.title.clone()))
                                    .child(div().truncate().text_sm().text_color(dim()).child(t.artist.clone())),
                            )
                    })),
            )
            // transport + seek
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1p5()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3p5()
                            .child(ctl("shuffle", if shuffle_on { "Shuffle: on" } else { "Shuffle: off" }, IconName::Shuffle, 18., if shuffle_on { accent() } else { dim() }).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.model.read(cx).send(Action::SetShuffle { on: !shuffle_on })
                            })))
                            .child(ctl("prev", "Previous (Ctrl+←)", IconName::SkipBack, 20., text()).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.model.read(cx).send(Action::Prev))))
                            .child(
                                div()
                                    .id("play")
                                    .tooltip(|w, cx| Tooltip::new("Play / Pause (Space)").build(w, cx))
                                    .size(px(40.))
                                    .rounded_full()
                                    .bg(text())
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .hover(|s| s.opacity(0.9))
                                    .child(icon(if st.playing { IconName::Pause } else { IconName::Play }, 18., on_text()))
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.model.read(cx).send(Action::Toggle))),
                            )
                            .child(ctl("next", "Next (Ctrl+→)", IconName::SkipForward, 20., text()).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.model.read(cx).send(Action::Next))))
                            .child(ctl("repeat", match st.repeat { Repeat::Off => "Repeat: off", Repeat::All => "Repeat: all", Repeat::One => "Repeat: this song" }, repeat_icon, 18., if st.repeat != Repeat::Off { accent() } else { dim() }).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.model.read(cx).send(Action::SetRepeat { mode: next_repeat })
                            }))),
                    )
                    .child(
                        div()
                            .w_full()
                            .max_w(px(640.))
                            .flex()
                            .items_center()
                            .gap_2p5()
                            .child(div().w(px(42.)).flex().justify_end().text_xs().text_color(faint()).child(fmt_time(pos)))
                            .child(div().flex_1().child(Slider::new(&self.seek)))
                            .child(div().w(px(42.)).text_xs().text_color(faint()).child(fmt_time(dur))),
                    ),
            )
            // queue + volume
            .child(
                div()
                    .w(px(side_w))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_1()
                    .child(ctl("lyrics", "Lyrics (Ctrl+L)", IconName::Mic, 18., if lyrics_open { accent() } else { dim() }).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.model.update(cx, |m, cx| m.toggle_lyrics(cx))
                    })))
                    .child(
                        div()
                            .relative()
                            .child(ctl("downloads", "Downloads", IconName::Download, 18., if dl_open { accent() } else { dim() }).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.model.update(cx, |m, cx| m.toggle_panel(Panel::Downloads, cx))
                            })))
                            .when(n_active > 0, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .top(px(2.))
                                        .right(px(0.))
                                        .min_w(px(16.))
                                        .h(px(16.))
                                        .px_1()
                                        .rounded_full()
                                        .bg(accent())
                                        .text_color(accent_ink())
                                        .text_xs()
                                        .font_weight(FontWeight::BOLD)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(n_active.to_string()),
                                )
                            }),
                    )
                    .child(ctl("queue", "Queue (Ctrl+Q)", IconName::ListMusic, 18., if queue_open { accent() } else { dim() }).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.model.update(cx, |m, cx| m.toggle_queue(cx))
                    })))
                    .child(ctl("mute", if muted { "Unmute" } else { "Mute" }, vol_icon, 18., dim()).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.model.read(cx).send(Action::SetMuted { muted: !muted })
                    })))
                    .when(show_vol, |d| d.child(div().w(px(110.)).child(Slider::new(&self.volume)))),
            )
    }
}
