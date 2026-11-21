//! Smooth mouse-wheel scrolling.
//!
//! gpui jumps a fixed distance per wheel notch, which looks choppy on a 144 Hz screen. This layer
//! catches wheel notches before the list sees them and eases the list towards the target offset
//! over a few frames instead. Touchpads already send pixel-precise deltas, so those pass straight
//! through untouched (any wheel animation in flight is cancelled so the two never fight).
use gpui::{
    canvas, point, px, DispatchPhase, EntityId, HitboxBehavior, IntoElement, ScrollDelta, ScrollEvent, Styled,
};
use gpui_component::scroll::ScrollbarHandle;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

/// Pixels per wheel line (gpui reports 3 lines per notch → ~110 px per notch).
const LINE: f32 = 36.;
/// Easing time constant: ~63% of the remaining distance is covered every TAU seconds.
const TAU: f32 = 0.07;

#[derive(Default)]
struct State {
    target: Option<f32>,
    last: Option<Instant>,
}

#[derive(Clone, Default)]
pub struct SmoothScroll(Rc<RefCell<State>>);

fn max_scroll<H: ScrollbarHandle>(h: &H) -> f32 {
    let content = f32::from(h.content_size().height);
    let view = f32::from(h.viewport_bounds().size.height);
    (content - view).max(0.)
}

impl SmoothScroll {
    /// An invisible layer to put (as the first child, absolutely positioned) inside the
    /// scroll container's relative wrapper. `view` is the entity that renders the list.
    pub fn layer<H: ScrollbarHandle + Clone>(&self, handle: H, view: EntityId) -> impl IntoElement {
        let st = self.clone();
        canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |_, hitbox, window, _| {
                window.on_mouse_event(move |ev: &ScrollEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture || !hitbox.should_handle_scroll(window) {
                        return;
                    }
                    let ScrollDelta::Lines(lines) = ev.delta else {
                        st.0.borrow_mut().target = None; // touchpad: let gpui handle it natively
                        return;
                    };
                    if lines.y == 0. {
                        return;
                    }
                    cx.stop_propagation();
                    let max = max_scroll(&handle);
                    let cur = f32::from(handle.offset().y);
                    let animating = st.0.borrow().target.is_some();
                    let base = st.0.borrow().target.unwrap_or(cur);
                    let target = (base + lines.y * LINE).clamp(-max, 0.);
                    st.0.borrow_mut().target = Some(target);
                    if !animating {
                        st.0.borrow_mut().last = Some(Instant::now());
                        Self::step(st.clone(), handle.clone(), view, window);
                    }
                });
            },
        )
        .absolute()
        .size_full()
    }

    fn step<H: ScrollbarHandle + Clone>(st: SmoothScroll, handle: H, view: EntityId, window: &mut gpui::Window) {
        window.on_next_frame(move |window, cx| {
            let Some(target) = st.0.borrow().target else { return };
            let now = Instant::now();
            let dt = st.0.borrow().last.map(|l| (now - l).as_secs_f32()).unwrap_or(0.007).min(0.05);
            st.0.borrow_mut().last = Some(now);
            let cur = f32::from(handle.offset().y);
            let next = cur + (target - cur) * (1. - (-dt / TAU).exp());
            let done = (target - next).abs() < 0.5;
            let y = if done { target } else { next };
            handle.set_offset(point(handle.offset().x, px(y)));
            cx.notify(view);
            if done {
                st.0.borrow_mut().target = None;
            } else {
                Self::step(st.clone(), handle.clone(), view, window);
            }
        });
    }
}
