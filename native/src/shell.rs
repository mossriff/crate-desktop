//! The main window: sidebar, page area, queue panel, and the player bar along the bottom.
//!
//! Layout adapts to the window width: the sidebar collapses to covers only, the queue floats
//! over the page instead of squeezing it, and the song list drops its Album/Date columns.
use crate::covers::{LARGE, SMALL};
use crate::model::{list_key, random_index, sorted_rows, LibTab, Model, Route, Sort, SortKey};
use crate_core::model::Playlist;
use crate::player_bar::PlayerBar;
use crate::smooth::SmoothScroll;
use crate::theme::*;
use crate_core::player::Action;
use gpui::AnimationExt;
use gpui::{
    actions, div, img, prelude::*, px, uniform_list, AnyElement, App, ClickEvent, Context, Entity, FocusHandle,
    FontWeight, KeyBinding, MouseButton, MouseDownEvent, NavigationDirection, ObjectFit, SharedString, Subscription,
    UniformListScrollHandle, Window, EntityId, ScrollHandle,
};
use gpui_component::scroll::{Scrollbar, ScrollbarHandle, ScrollbarMode};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::tooltip::Tooltip;
use gpui_component::Icon;
use gpui_kit_assets::IconName;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

#[path = "downloads_ui.rs"]
mod downloads_ui;
#[path = "library.rs"]
mod library;
#[path = "overlay.rs"]
mod overlay;
#[path = "search.rs"]
mod search;
#[path = "settings_ui.rs"]
mod settings_ui;
#[path = "import_ui.rs"]
mod import_ui;

use crate::catalog::Payload;
use crate::model::Panel;
pub use overlay::demo_edit;
use overlay::{drag_ghost, DragItem, Menu, MenuItem, Prompt};
use std::collections::BTreeSet;

actions!(crate_app, [TogglePlay, NextTrack, PrevTrack, GoBack, GoForward, FindInPage, VolumeUp, VolumeDown, SeekForward, SeekBack, ToggleQueue, PlaySelected, SelectUp, SelectDown, ToggleLyrics, CycleLyrMode, RemoveSelected, UndoEdit, SelectAll, Dismiss, MoveUp, MoveDown, OpenSettings]);

/// Keyboard shortcuts. Ones that would clash with typing are disabled while a text box has focus.
pub fn bind_keys(cx: &mut App) {
    let free = Some("Shell && !Input");
    cx.bind_keys([
        KeyBinding::new("space", TogglePlay, free),
        KeyBinding::new("ctrl-right", NextTrack, free),
        KeyBinding::new("ctrl-left", PrevTrack, free),
        KeyBinding::new("shift-right", SeekForward, free),
        KeyBinding::new("shift-left", SeekBack, free),
        KeyBinding::new("ctrl-up", VolumeUp, free),
        KeyBinding::new("ctrl-down", VolumeDown, free),
        KeyBinding::new("enter", PlaySelected, free),
        KeyBinding::new("up", SelectUp, free),
        KeyBinding::new("down", SelectDown, free),
        KeyBinding::new("alt-left", GoBack, Some("Shell")),
        KeyBinding::new("alt-right", GoForward, Some("Shell")),
        KeyBinding::new("ctrl-f", FindInPage, Some("Shell")),
        KeyBinding::new("ctrl-q", ToggleQueue, Some("Shell")),
        KeyBinding::new("ctrl-l", ToggleLyrics, Some("Shell")),
        KeyBinding::new("r", CycleLyrMode, free),
        KeyBinding::new("delete", RemoveSelected, free),
        KeyBinding::new("ctrl-z", UndoEdit, free),
        KeyBinding::new("ctrl-a", SelectAll, free),
        KeyBinding::new("alt-up", MoveUp, free),
        KeyBinding::new("alt-down", MoveDown, free),
        KeyBinding::new("escape", Dismiss, Some("Shell")),
        KeyBinding::new("ctrl-,", OpenSettings, Some("Shell")),
    ]);
}

pub struct Shell {
    model: Entity<Model>,
    bar: Entity<PlayerBar>,
    filter: Entity<InputState>,
    focus: FocusHandle,
    list_scroll: UniformListScrollHandle,
    list_smooth: SmoothScroll,
    home_scroll: ScrollHandle,
    home_smooth: SmoothScroll,
    queue_scroll: ScrollHandle,
    queue_smooth: SmoothScroll,
    side_scroll: ScrollHandle,
    side_smooth: SmoothScroll,
    grid_scroll: UniformListScrollHandle,
    grid_smooth: SmoothScroll,
    /// Page shown last frame; when it changes, scroll positions/selection/filter reset.
    last_page: Option<(Route, LibTab)>,
    /// Selected song on the current playlist page (index into the playlist's tracks).
    selected: Option<usize>,
    /// All selected songs (Ctrl/Shift+click); `selected` is the one the keyboard moves from.
    picked: Rc<BTreeSet<usize>>,
    /// While dragging songs near the top/bottom of the list it scrolls by itself (px/s, + = up).
    drag_speed: f32,
    /// blank rows before the songs (room for the page header), see `song_page`
    list_lead: usize,
    /// Settings: duplicate list of "Fix library" expanded
    fix_details: bool,
    drag_last: std::time::Instant,
    /// Rows as currently shown (sorted + filtered), for keyboard navigation.
    visible_rows: Rc<Vec<usize>>,
    /// What `visible_rows` was computed from, so sorting/filtering only reruns when one changes
    /// (not on every scroll frame).
    rows_key: Option<(usize, Sort, String)>,
    lyrics_view: Entity<crate::lyrics_view::LyricsView>,
    search_input: Entity<InputState>,
    search_task: Option<gpui::Task<()>>,
    menu: Option<Menu>,
    /// decoded covers, capped (see imgcache.rs)
    images: Entity<crate::imgcache::LruImages>,
    prompt: Option<Prompt>,
    /// a setup step opened from Settings, and its "done" subscription
    setup: Option<(crate::onboarding::Step, Entity<crate::onboarding::Onboarding>, Subscription)>,
    /// Import page: list of songs not found, expanded
    import_details: bool,
    /// Settings → Accounts: (Navidrome login, Soulseek name, missing programs), and when read
    accounts: std::cell::RefCell<Option<(std::time::Instant, AccountsInfo)>>,
    _subs: Vec<Subscription>,
}

pub fn icon(name: IconName, size: f32, color: impl Into<gpui::Hsla>) -> Icon {
    Icon::new(name).size(px(size)).text_color(color.into())
}

fn tip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView + 'static {
    let text: SharedString = text.into();
    move |w, cx| Tooltip::new(text.clone()).build(w, cx)
}

fn cover_el(path: Option<PathBuf>, size: f32, radius: f32) -> AnyElement {
    match path {
        Some(p) => img(p).size(px(size)).rounded(px(radius)).object_fit(ObjectFit::Cover).flex_none().into_any_element(),
        None => div()
            .size(px(size))
            .rounded(px(radius))
            .bg(panel2())
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .child(icon(IconName::Music, (size * 0.4).max(12.), faint()))
            .into_any_element(),
    }
}

/// Crate's icon (assets/crate.svg), decoded once.
pub(crate) fn app_icon() -> Arc<gpui::Image> {
    static ICON: std::sync::OnceLock<Arc<gpui::Image>> = std::sync::OnceLock::new();
    ICON.get_or_init(|| Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Svg, include_bytes!("../assets/crate.svg").to_vec()))).clone()
}

/// Marker for the playing song in lists: three bouncing bars while it plays (a speaker icon
/// when paused or with animations off).
fn now_playing_icon(playing: bool) -> AnyElement {
    if !playing || !crate::theme::animations() {
        return icon(IconName::Volume2, 16., accent()).into_any_element();
    }
    let bar = |i: usize| {
        div().w(px(3.)).rounded_sm().bg(accent()).h(px(6.)).with_animation(
            ("eq-bar", i),
            // repeating, capped at 30 fps: it redraws the window, so keep it cheap
            gpui::Animation::new(std::time::Duration::from_millis(700 + 160 * i as u64)).repeat().with_max_fps(30.).with_easing(gpui::pulsating_between(0., 1.)),
            move |d, t| d.h(px(4. + 10. * t)),
        )
    };
    div().h(px(16.)).flex().items_end().gap(px(2.)).child(bar(0)).child(bar(1)).child(bar(2)).into_any_element()
}

fn loading() -> AnyElement {
    div().size_full().flex().items_center().justify_center().text_color(dim()).child("Loading…").into_any_element()
}

/// Natural height of `hero` (its cover plus padding).
fn hero_height(lay: Layout) -> f32 {
    // 26 px on top (not 32) so header + bar + column titles (222 + 114) is exactly six song rows
    if lay.content_w < 640. { 20. + 112. + 20. } else { 26. + 176. + 20. }
}

/// Big header with cover art, a kind label ("PLAYLIST"/"ALBUM"/"ARTIST"), title and details.
fn hero(cover: Option<PathBuf>, round: bool, kind: &'static str, title: impl Into<SharedString>, sub: AnyElement, lay: Layout) -> AnyElement {
    let narrow = lay.content_w < 640.;
    let pad = if narrow { 20. } else { 32. };
    let size = if narrow { 112. } else { 176. };
    div()
        .flex()
        .flex_none()
        .items_end()
        .gap(px(if narrow { 18. } else { 28. }))
        .px(px(pad))
        .pt(px(if narrow { 20. } else { 26. }))
        .pb_5()
        .bg(gpui::linear_gradient(180., gpui::linear_color_stop(accent_wash(), 0.), gpui::linear_color_stop(bg(), 1.)))
        .child(cover_el(cover, size, if round { size / 2. } else { 12. }))
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .min_w_0()
                .gap_2()
                .child(div().text_xs().font_weight(FontWeight::BOLD).text_color(dim()).child(kind))
                .child(div().when(narrow, |d| d.text_2xl()).when(!narrow, |d| d.text_3xl()).font_weight(FontWeight::EXTRA_BOLD).truncate().child(title.into()))
                .child(sub),
        )
        .into_any_element()
}

type AccountsInfo = (Option<(String, String, String)>, Option<String>, Vec<&'static str>);

/// Width of the scrollbar lane; content keeps clear of it.
pub(crate) const GUTTER: f32 = 16.;

/// A scrolling region with smooth wheel scrolling and a visible scrollbar on the right.
/// The returned wrapper is `relative` and fills its parent's flex column; size it further as needed.
pub(crate) fn scroll_area<H: ScrollbarHandle + Clone>(content: impl IntoElement, handle: &H, smooth: &SmoothScroll, view: EntityId) -> gpui::Div {
    div()
        .relative()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(smooth.layer(handle.clone(), view))
        .child(content)
        .child(Scrollbar::vertical(handle).mode(ScrollbarMode::Always))
}

/// Like `scroll_area`, without the scrollbar (the caller places it, e.g. above an overlay).
pub(crate) fn scroll_area_bare<H: ScrollbarHandle + Clone>(content: impl IntoElement, handle: &H, smooth: &SmoothScroll, view: EntityId) -> gpui::Div {
    div().relative().flex_1().min_h_0().flex().flex_col().child(smooth.layer(handle.clone(), view)).child(content)
}

/// Window-width dependent layout decisions, computed once per frame.
#[derive(Clone, Copy)]
struct Layout {
    compact_sidebar: bool,
    sidebar_w: f32,
    queue_w: f32,
    queue_overlay: bool,
    /// Width available to the page itself.
    content_w: f32,
}

impl Layout {
    fn new(win_w: f32, queue_open: bool) -> Self {
        let compact_sidebar = win_w < 980.;
        let sidebar_w = if compact_sidebar { 76. } else { 248. };
        let queue_w = if win_w < 1200. { 300. } else { 340. };
        let queue_overlay = queue_open && win_w - sidebar_w - queue_w < 560.;
        let content_w = win_w - sidebar_w - if queue_open && !queue_overlay { queue_w } else { 0. };
        Layout { compact_sidebar, sidebar_w, queue_w, queue_overlay, content_w }
    }
}

impl Shell {
    pub fn new(model: Entity<Model>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let bar = cx.new(|cx| PlayerBar::new(model.clone(), window, cx));
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter songs"));
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let q0 = model.read(cx).search_q.clone();
        let search_input = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("What do you want to listen to?");
            s.set_value(q0, window, cx);
            s
        });
        let subs = vec![
            // search as you type (debounced), or straight away on Enter
            cx.subscribe_in(&search_input, window, |this: &mut Self, input, ev: &InputEvent, _, cx| {
                let q = input.read(cx).value().to_string();
                match ev {
                    InputEvent::Change => {
                        this.search_task = Some(cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(std::time::Duration::from_millis(300)).await;
                            let _ = this.update(cx, |s, cx| s.model.update(cx, |m, cx| m.run_search(q, cx)));
                        }));
                    }
                    InputEvent::PressEnter { .. } => {
                        this.search_task = None;
                        this.model.update(cx, |m, cx| {
                            m.remember_search(&q, cx);
                            m.run_search(q, cx)
                        });
                    }
                    _ => {}
                }
            }),
            cx.observe(&model, |_, _, cx| cx.notify()),
            cx.subscribe_in(&filter, window, |this: &mut Self, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.select_only(None);
                    cx.notify();
                }
            }),
        ];
        let lyrics_view = cx.new(|cx| crate::lyrics_view::LyricsView::new(model.clone(), cx));
        Shell {
            lyrics_view,
            model,
            bar,
            filter,
            focus,
            list_scroll: UniformListScrollHandle::new(),
            list_smooth: SmoothScroll::default(),
            home_scroll: ScrollHandle::new(),
            home_smooth: SmoothScroll::default(),
            queue_scroll: ScrollHandle::new(),
            queue_smooth: SmoothScroll::default(),
            side_scroll: ScrollHandle::new(),
            side_smooth: SmoothScroll::default(),
            grid_scroll: UniformListScrollHandle::new(),
            grid_smooth: SmoothScroll::default(),
            last_page: None,
            selected: None,
            picked: Rc::new(BTreeSet::new()),
            drag_speed: 0.,
            list_lead: 0,
            fix_details: false,
            drag_last: std::time::Instant::now(),
            visible_rows: Rc::new(vec![]),
            rows_key: None,
            search_input,
            search_task: None,
            menu: None,
            images: crate::imgcache::LruImages::new(cx),
            prompt: None,
            setup: None,
            import_details: false,
            accounts: Default::default(),
            _subs: subs,
        }
    }

    /// CRATE_DEMO_SCROLL test hook.
    pub fn demo_scroll(&mut self, y: f32, cx: &mut Context<Self>) {
        ScrollbarHandle::set_offset(&self.list_scroll, gpui::point(px(0.), px(-y)));
        cx.notify();
    }

    pub(crate) fn model(&self) -> &Entity<Model> {
        &self.model
    }

    pub(crate) fn go(&mut self, r: Route, cx: &mut Context<Self>) {
        self.model.update(cx, |m, cx| m.go(r, cx));
    }

    fn current_playlist_id(&self, cx: &App) -> Option<String> {
        let m = self.model.read(cx);
        list_key(&m.route, m.lib_tab)
    }

    /// Play the shown list starting at `track_index` (an index into the playlist's tracks).
    fn play_from(&mut self, track_index: usize, cx: &mut Context<Self>) {
        let Some(id) = self.current_playlist_id(cx) else { return };
        let Some(pl) = self.model.read(cx).cache.get(&id).cloned() else { return };
        let rows = self.visible_rows.clone();
        let Some(pos) = rows.iter().position(|&k| k == track_index) else { return };
        let tracks = rows.iter().map(|&k| pl.tracks[k].clone()).collect();
        self.select_only(Some(track_index));
        self.model.update(cx, |m, cx| m.play_tracks(tracks, pos, Some(id), cx));
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.visible_rows.is_empty() {
            return;
        }
        let pos = self.selected.and_then(|s| self.visible_rows.iter().position(|&k| k == s));
        let next = match pos {
            None => 0,
            Some(p) => (p as isize + delta).clamp(0, self.visible_rows.len() as isize - 1) as usize,
        };
        self.select_only(Some(self.visible_rows[next]));
        // keep it clear of the pinned play/filter bar and column titles (~2 rows tall)
        self.list_scroll.scroll_to_item_with_offset(next + self.list_lead, gpui::ScrollStrategy::Nearest, 3);
        cx.notify();
    }

    /// Drag near the list's top or bottom edge: scroll that way, faster the closer you get.
    fn drag_edge(&mut self, ev: &gpui::DragMoveEvent<DragItem>, cx: &mut Context<Self>) {
        const ZONE: f32 = 64.;
        let (b, p) = (ev.bounds, ev.event.position);
        let (top, bottom, y) = (f32::from(b.top()), f32::from(b.bottom()), f32::from(p.y));
        let inside_x = p.x >= b.left() && p.x <= b.right();
        let speed = if !inside_x || y < top - ZONE || y > bottom + ZONE {
            0.
        } else if y < top + ZONE {
            ((top + ZONE - y) / ZONE).min(1.5) * 900.
        } else if y > bottom - ZONE {
            -((y - (bottom - ZONE)) / ZONE).min(1.5) * 900.
        } else {
            0.
        };
        if speed != 0. && self.drag_speed == 0. {
            self.drag_last = std::time::Instant::now();
            cx.notify();
        }
        self.drag_speed = speed;
    }

    fn drag_scroll_step(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.drag_speed == 0. {
            return;
        }
        if !cx.has_active_drag() {
            self.drag_speed = 0.;
            return;
        }
        let now = std::time::Instant::now();
        let dt = (now - self.drag_last).as_secs_f32().min(0.05);
        self.drag_last = now;
        let h = &self.list_scroll;
        let max = (f32::from(ScrollbarHandle::content_size(h).height) - f32::from(ScrollbarHandle::viewport_bounds(h).size.height)).max(0.);
        let cur = ScrollbarHandle::offset(h);
        let y = (f32::from(cur.y) + self.drag_speed * dt).clamp(-max, 0.);
        ScrollbarHandle::set_offset(h, gpui::point(cur.x, px(y)));
        window.request_animation_frame();
    }

    fn select_only(&mut self, k: Option<usize>) {
        self.selected = k;
        let set = Rc::make_mut(&mut self.picked);
        set.clear();
        set.extend(k);
    }

    /// Click on a song: select it; Ctrl adds/removes it, Shift selects everything in between.
    fn click_row(&mut self, k: usize, mods: gpui::Modifiers, cx: &mut Context<Self>) {
        if mods.control || mods.platform {
            let set = Rc::make_mut(&mut self.picked);
            if !set.insert(k) {
                set.remove(&k);
            }
            self.selected = Some(k);
        } else if let (true, Some(a)) = (mods.shift, self.selected) {
            let pos = |x: usize| self.visible_rows.iter().position(|&r| r == x);
            if let (Some(pa), Some(pk)) = (pos(a), pos(k)) {
                let (lo, hi) = (pa.min(pk), pa.max(pk));
                let range: Vec<usize> = self.visible_rows[lo..=hi].to_vec();
                let set = Rc::make_mut(&mut self.picked);
                set.clear();
                set.extend(range);
            }
        } else {
            self.select_only(Some(k));
        }
        cx.notify();
    }

    /// Selected songs, in the order they're shown.
    fn selection(&self) -> Vec<usize> {
        self.visible_rows.iter().copied().filter(|k| self.picked.contains(k)).collect()
    }

    /// The playlist on screen, if it's one you can edit (not the library or an album).
    fn editable_pid(&self, cx: &App) -> Option<String> {
        match &self.model.read(cx).route {
            Route::Playlist(id) => Some(id.clone()),
            _ => None,
        }
    }

    fn remove_selected(&mut self, cx: &mut Context<Self>) {
        let Some(pid) = self.editable_pid(cx) else { return };
        let idx = self.selection();
        if idx.is_empty() {
            return;
        }
        self.select_only(None);
        self.model.update(cx, |m, cx| m.remove_from_playlist(pid, &idx, cx));
    }

    /// Move songs within the playlist (drag and drop, Alt+↑/↓). Only in the playlist's own order.
    fn move_songs(&mut self, moved: Vec<usize>, target: usize, cx: &mut Context<Self>) {
        let Some(pid) = self.editable_pid(cx) else { return };
        if self.model.read(cx).sort_for(&pid).key.is_some() {
            return self.model.update(cx, |m, cx| m.toast("Sorted by a column \u{2014} click it until the arrow goes away to reorder", false, cx));
        }
        let anchor = self.selected.and_then(|a| moved.iter().position(|&k| k == a));
        if let Some(new) = self.model.update(cx, |m, cx| m.move_in_playlist(pid, &moved, target, cx)) {
            self.selected = anchor.and_then(|i| new.get(i).copied()).or(new.first().copied());
            self.picked = Rc::new(new.into_iter().collect());
            if let Some(pos) = self.selected.and_then(|s| self.visible_rows_after_move(s)) {
                self.list_scroll.scroll_to_item_with_offset(pos + self.list_lead, gpui::ScrollStrategy::Nearest, 3);
            }
        }
    }

    /// Where a playlist index will be shown once rows are recomputed (no sort: same order,
    /// minus whatever the filter hides).
    fn visible_rows_after_move(&self, k: usize) -> Option<usize> {
        self.rows_key.as_ref().is_some_and(|r| r.2.is_empty()).then_some(k)
    }

    fn nudge(&mut self, delta: isize, cx: &mut Context<Self>) {
        let sel = self.selection();
        let (Some(&first), Some(&last)) = (sel.iter().min(), sel.iter().max()) else { return };
        let target = if delta < 0 { first.checked_sub(1) } else { Some(last + 1) };
        let len = self.editable_pid(cx).and_then(|p| self.model.read(cx).cache.get(&p).map(|p| p.tracks.len())).unwrap_or(0);
        if let Some(t) = target.filter(|&t| t < len) {
            self.move_songs(sel, t, cx);
        }
    }

    // ------------------------------------------------------------ sidebar
    fn sidebar(&self, lay: Layout, cx: &mut Context<Self>) -> impl IntoElement {
        let compact = lay.compact_sidebar;
        let m = self.model.read(cx);
        let route = m.route.clone();
        let playing_src = if m.player_state.playing { m.queue_source.clone() } else { None };
        let can_back = !m.history.is_empty();
        let can_fwd = !m.future.is_empty();

        let arrow = |id: &'static str, ic: IconName, label: &'static str, enabled: bool| {
            div()
                .id(id)
                .p_1()
                .rounded_md()
                .tooltip(tip(label))
                .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(hover())))
                .child(icon(ic, 16., if enabled { dim() } else { line() }))
        };
        let back = arrow("back", IconName::ChevronLeft, "Back (Alt+←)", can_back)
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.back(cx))));
        let fwd = arrow("fwd", IconName::ChevronRight, "Forward (Alt+→)", can_fwd)
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.forward(cx))));
        // the same picture as the launcher / taskbar icon
        let logo = img(app_icon()).size(px(30.)).flex_none();

        let lib_on = matches!(route, Route::Library | Route::Album(_) | Route::Artist(_));
        let nav = |id: &'static str, ic: IconName, label: &'static str, on: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_3()
                .px_3()
                .py_2()
                .rounded_lg()
                .when(compact, |d| d.justify_center().px_0())
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if on { text() } else { dim() })
                .when(on, |d| d.bg(panel2()))
                .hover(|s| s.text_color(text()).bg(hover()))
                .cursor_pointer()
                .when(compact, |d| d.tooltip(tip(label)))
                .child(icon(ic, 18., if on { text() } else { dim() }))
                .when(!compact, |d| d.child(label))
        };
        let library = nav("nav-lib", IconName::Library, "Your library", lib_on)
            .drag_over::<DragItem>(|s, _, _, _| s.bg(accent()).text_color(accent_ink()))
            .on_drop(cx.listener(|this, d: &DragItem, _, cx| {
                let p = (*d.payload).clone();
                this.model.update(cx, |m, cx| m.add_to(None, p, cx));
            }))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.go(Route::Library, cx)));
        let search_on = matches!(route, Route::Search | Route::DzAlbum(_) | Route::DzArtist(_));
        let search = nav("nav-search", IconName::Search, "Search", search_on).on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
            this.go(Route::Search, cx);
            this.search_input.update(cx, |i, cx| i.focus(window, cx));
        }));
        let home_on = route == Route::Home;
        let home = div()
            .id("nav-home")
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .py_2()
            .rounded_lg()
            .when(compact, |d| d.justify_center().px_0())
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(if home_on { text() } else { dim() })
            .when(home_on, |d| d.bg(panel2()))
            .hover(|s| s.text_color(text()).bg(hover()))
            .cursor_pointer()
            .when(compact, |d| d.tooltip(tip("Home")))
            .child(icon(IconName::House, 18., if home_on { text() } else { dim() }))
            .when(!compact, |d| d.child("Home"))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.go(Route::Home, cx)));

        let pls: Vec<AnyElement> = m
            .playlists
            .iter()
            .map(|p| {
                let on = route == Route::Playlist(p.id.clone());
                let playing = playing_src.as_deref() == Some(p.id.as_str());
                let id = p.id.clone();
                div()
                    .id(SharedString::from(format!("pl-{}", p.id)))
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_2()
                    .py_1p5()
                    .rounded_lg()
                    .cursor_pointer()
                    .when(compact, |d| d.justify_center().tooltip(tip(p.name.clone())))
                    .when(on, |d| d.bg(panel2()))
                    .hover(|s| s.bg(hover()))
                    .child(cover_el(m.covers.get(p.cover_art.as_deref().or(Some(&p.id)), SMALL), if compact { 44. } else { 42. }, 6.))
                    .when(!compact, |d| {
                        d.child(
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .min_w_0()
                                .child(div().truncate().font_weight(FontWeight::MEDIUM).text_color(if on || playing { accent() } else { text() }).child(p.name.clone()))
                                .child(div().truncate().text_xs().text_color(faint()).child(format!("Playlist · {}", songs(p.song_count as usize)))),
                        )
                        .when(playing, |d| d.child(icon(IconName::Volume2, 14., accent())))
                    })
                    .drag_over::<DragItem>(|s, _, _, _| s.bg(accent_drop()))
                    .on_drop(cx.listener({
                        let pid = p.id.clone();
                        move |this, d: &DragItem, _, cx| {
                            let (pid, pl) = (pid.clone(), (*d.payload).clone());
                            this.model.update(cx, |m, cx| m.add_to(Some(pid), pl, cx));
                        }
                    }))
                    .on_mouse_down(MouseButton::Right, cx.listener({
                        let pid = p.id.clone();
                        move |this, ev: &MouseDownEvent, _, cx| {
                            let (a, b) = (pid.clone(), pid.clone());
                            let mut items = vec![
                                overlay::item(IconName::Play, "Play", move |s, _, cx| s.model.update(cx, |m, cx| m.play_playlist(a.clone(), false, cx))),
                                overlay::item(IconName::Shuffle, "Shuffle play", move |s, _, cx| s.model.update(cx, |m, cx| m.play_playlist(b.clone(), true, cx))),
                                MenuItem::Sep,
                            ];
                            items.extend(Shell::playlist_items(&pid));
                            this.open_menu(ev.position, items, cx);
                        }
                    }))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.go(Route::Playlist(id.clone()), cx)))
                    .into_any_element()
            })
            .collect();

        let top = if compact {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .pt_4()
                .pb_2()
                .child(logo)
                .child(div().flex().child(back).child(fwd))
        } else {
            div()
                .flex()
                .items_center()
                .gap_2p5()
                .px_5()
                .pt_5()
                .pb_3()
                .child(logo)
                .child(div().text_lg().font_weight(FontWeight::BOLD).child("Crate"))
                .child(div().flex_1())
                .child(back)
                .child(fwd)
        };

        div()
            .w(px(lay.sidebar_w))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(side())
            .border_r_1()
            .border_color(line())
            .child(top)
            .child(div().px_2p5().flex().flex_col().gap_0p5().child(home).child(search).child(library))
            .child(if compact {
                div().mx_4().my_3().h(px(1.)).bg(line()).into_any_element()
            } else {
                div().px_5().pt_5().pb_2().text_xs().font_weight(FontWeight::BOLD).text_color(faint()).child("PLAYLISTS").into_any_element()
            })
            .child(scroll_area(
                div().id("pls").size_full().overflow_y_scroll().track_scroll(&self.side_scroll).pl_2p5().pr(px(GUTTER)).pb_3().flex().flex_col().children(pls),
                &self.side_scroll,
                &self.side_smooth,
                cx.entity_id(),
            ))
            .child(
                div().px_2p5().py_2().border_t_1().border_color(line()).child(
                    nav("nav-settings", IconName::Settings, "Settings", route == Route::Settings)
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.go(Route::Settings, cx))),
                ),
            )
    }

    // ------------------------------------------------------------ pages
    fn home(&self, lay: Layout, cx: &mut Context<Self>) -> AnyElement {
        // a row each of recently played and most played albums (once there are plays)
        let album_rows: Vec<AnyElement> = {
            let m = self.model.read(cx);
            let lists: Vec<(&'static str, Option<Arc<Vec<crate_core::model::AlbumSummary>>>)> =
                vec![
                    ("Jump back in", m.albums.get("recent").cloned().filter(|_| m.home_rows[0])),
                    ("Your most played", m.albums.get("frequent").cloned().filter(|_| m.home_rows[1])),
                ];
            let avail = lay.content_w - 64.;
            let (per_row, _) = library::card_metrics(avail);
            lists
                .into_iter()
                .filter_map(|(title, l)| {
                    let l = l.filter(|l| !l.is_empty())?;
                    let cards: Vec<library::Card> = l.iter().take(per_row).map(library::album_card).collect();
                    Some(
                        div()
                            .mt_10()
                            .child(div().text_xl().font_weight(FontWeight::BOLD).mb_4().child(title))
                            .child(self.cards_wrap(&cards, avail, cx))
                            .into_any_element(),
                    )
                })
                .collect()
        };
        let m = self.model.read(cx);
        // cards fill the row evenly instead of leaving a ragged gap on the right
        let avail = lay.content_w - 64.;
        let per_row = ((avail + 20.) / (184. + 20.)).floor().max(1.);
        let card_w = ((avail - 20. * (per_row - 1.)) / per_row).clamp(150., 240.);
        let cover_w = card_w - 28.;
        let playing_src = if m.player_state.playing { m.queue_source.clone() } else { None };
        let cards: Vec<AnyElement> = m
            .playlists
            .iter()
            .map(|p| {
                let id = p.id.clone();
                let id_play = p.id.clone();
                let is_playing = playing_src.as_deref() == Some(p.id.as_str());
                div()
                    .id(SharedString::from(format!("card-{}", p.id)))
                    .group("card")
                    .w(px(card_w))
                    .p_3p5()
                    .rounded_xl()
                    .bg(panel())
                    .hover(|s| s.bg(panel2()))
                    .cursor_pointer()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .relative()
                            .child(cover_el(m.covers.get(p.cover_art.as_deref().or(Some(&p.id)), LARGE), cover_w, 8.))
                            .child(
                                div()
                                    .id(SharedString::from(format!("card-play-{}", p.id)))
                                    .absolute()
                                    .right(px(8.))
                                    .bottom(px(8.))
                                    .size(px(46.))
                                    .rounded_full()
                                    .bg(accent())
                                    .shadow_lg()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .when(!is_playing, |d| d.invisible().group_hover("card", |s| s.visible()))
                                    .hover(|s| s.opacity(0.9))
                                    .tooltip(tip(if is_playing { "Pause" } else { "Play" }))
                                    .child(icon(if is_playing { IconName::Pause } else { IconName::Play }, 20., accent_ink()))
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        cx.stop_propagation();
                                        let id = id_play.clone();
                                        this.model.update(cx, |m, cx| {
                                            if m.queue_source.as_deref() == Some(id.as_str()) && m.player_state.index.is_some() {
                                                m.send(Action::Toggle);
                                            } else {
                                                m.play_playlist(id, false, cx);
                                            }
                                        });
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(div().truncate().font_weight(FontWeight::SEMIBOLD).text_color(if is_playing { accent() } else { text() }).child(p.name.clone()))
                            .child(div().truncate().text_sm().text_color(dim()).child(format!("{} · {}", songs(p.song_count as usize), fmt_long(p.duration)))),
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.go(Route::Playlist(id.clone()), cx)))
                    .into_any_element()
            })
            .collect();
        let content = div()
            .id("home")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.home_scroll)
            .px_8()
            .py_7()
            .child(div().text_3xl().font_weight(FontWeight::BOLD).mb_6().child("Welcome back"))
            .child(div().text_xl().font_weight(FontWeight::BOLD).mb_4().child("Your playlists"))
            .child(div().flex().flex_wrap().gap_5().children(cards))
            .children(album_rows);
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(scroll_area(content, &self.home_scroll, &self.home_smooth, cx.entity_id()))
            .into_any_element()
    }

    fn playlist_page(&mut self, id: &str, lay: Layout, cx: &mut Context<Self>) -> AnyElement {
        let m = self.model.read(cx);
        let Some(pl) = m.cache.get(id).cloned() else { return loading() };
        let total: f64 = pl.tracks.iter().map(|t| t.duration).sum();
        let cover = m.covers.get(pl.cover_art.as_deref().or(Some(&pl.id)), LARGE);
        let sub = div()
            .flex()
            .flex_col()
            .gap_2()
            .when_some(pl.comment.clone(), |d, c| d.child(div().truncate().text_color(dim()).child(c)))
            .child(div().text_color(dim()).child(format!("{} · {}", songs(pl.tracks.len()), fmt_long(total))))
            .into_any_element();
        let hero = hero(cover, false, "PLAYLIST", pl.name.clone(), sub, lay);
        self.song_page(pl, Some((hero, hero_height(lay), true)), lay, cx)
    }

    /// A song list with play/shuffle/filter, sortable columns, and a virtualised table.
    /// Used for playlists, the whole library, and albums.
    /// `top`: the page header shown above the list (element, its natural height, and whether its
    /// background is accent-tinted). The header scrolls away with the songs, smoothly, while the
    /// play/filter bar and column titles stay pinned at the top.
    fn song_page(&mut self, pl: Arc<Playlist>, top: Option<(AnyElement, f32, bool)>, lay: Layout, cx: &mut Context<Self>) -> AnyElement {
        // gpui's virtual list can't scroll padding, so the header's space is reserved with blank
        // rows at the start of the list and the header is drawn over them, moved up as the list
        // scrolls. Heights are fixed so the blank rows add up exactly.
        let row = crate::theme::row_h();
        let cov = crate::theme::row_cover();
        const BAR_H: f32 = 70.;
        const COLS_H: f32 = 36.;
        const GAP: f32 = 8.;
        let (top_el, top_nat, tinted) = match top {
            Some((e, h, t)) => (Some(e), h, t),
            None => (None, 12., false),
        };
        let stack = top_nat + BAR_H + COLS_H + GAP;
        let lead = (stack / row).ceil() as usize;
        let head_h = top_nat + (lead as f32 * row - stack);
        self.list_lead = lead;
        let scrolled = -f32::from(ScrollbarHandle::offset(&self.list_scroll).y);
        let shift = scrolled.clamp(0., head_h);
        let title_fade = ((scrolled - (head_h - 70.)) / 70.).clamp(0., 1.);
        let m = self.model.read(cx);
        let id = pl.id.as_str();
        let sort = m.sort_for(id);
        let filter = self.filter.read(cx).value().to_string();
        let key = (Arc::as_ptr(&pl) as usize, sort, filter.clone());
        if self.rows_key.as_ref() != Some(&key) {
            self.visible_rows = Rc::new(sorted_rows(&pl, sort, &filter));
            self.rows_key = Some(key);
        }
        let rows = self.visible_rows.clone();
        let cur_id = m.current().map(|t| t.id.clone());
        let is_source_playing = m.queue_source.as_deref() == Some(id) && m.player_state.playing;
        let covers = m.covers.clone();
        let total: f64 = pl.tracks.iter().map(|t| t.duration).sum();
        let is_library = id == "__library";

        let w = lay.content_w;
        let narrow = w < 640.;
        // on an album's own page every row would repeat its name
        let show_album = w >= 720. && !matches!(m.route, Route::Album(_)) && m.show_cols[0];
        let show_date = w >= 900. && m.show_cols[1];
        let playing = m.player_state.playing;
        let pad = if narrow { 20. } else { 32. };

        let header_btn = |key: SortKey, label: &'static str, width: Option<f32>, cx: &mut Context<Self>| {
            let on = sort.key == Some(key);
            let pid = pl.id.clone();
            let hint = match (on, sort.desc) {
                (false, _) => format!("Sort by {}", label.to_lowercase()),
                (true, false) => "Click to reverse".to_string(),
                (true, true) => "Click to go back to the original order".to_string(),
            };
            let d = div()
                .id(SharedString::from(format!("sort-{}", key.as_str())))
                .flex()
                .items_center()
                .gap_1()
                .cursor_pointer()
                .text_color(if on { accent() } else { faint() })
                .hover(|s| s.text_color(text()))
                .tooltip(tip(hint))
                .child(label)
                .when(on, |d| d.child(icon(if sort.desc { IconName::ArrowDown } else { IconName::ArrowUp }, 12., accent())))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let next = match (sort.key == Some(key), sort.desc) {
                        (false, _) => Sort { key: Some(key), desc: false },
                        (true, false) => Sort { key: Some(key), desc: true },
                        (true, true) => Sort { key: None, desc: false },
                    };
                    let pid = pid.clone();
                    this.model.update(cx, |m, cx| m.set_sort(&pid, next, cx));
                }));
            match width {
                Some(wd) => d.w(px(wd)).flex_none(),
                None => d.flex_1().min_w_0(),
            }
        };

        let rows_for_list = rows.clone();
        let pl_for_list = pl.clone();
        let picked = self.picked.clone();
        // a playlist in its own order can be rearranged by dragging songs onto each other
        let editable = matches!(&m.route, Route::Playlist(p) if *p == pl.id);
        let reorder = (editable && sort.key.is_none()).then(|| pl.id.clone());
        // every selected row drags the whole selection; built once per frame, shared by all
        let sel_drag = (picked.len() > 1).then(|| {
            let sel: Vec<usize> = rows.iter().copied().filter(|k| picked.contains(k)).collect();
            let tracks = sel.iter().map(|&k| pl.tracks[k].clone()).collect();
            DragItem::shared(Rc::new(Payload::Lib(tracks)), reorder.clone().map(|p| (p, Rc::new(sel))))
        });
        let shell = cx.entity();
        let list = uniform_list("songs", rows.len() + lead, move |range, _window, _cx| {
            range
                .map(|i| {
                    if i < lead {
                        return div().w_full().h(px(row)).child(div().id(i));
                    }
                    let n = i - lead; // position among the shown songs
                    let k = rows_for_list[n];
                    let t = &pl_for_list.tracks[k];
                    let cur = cur_id.as_deref() == Some(t.id.as_str());
                    let sel = picked.contains(&k);
                    let (shell, shell2, shell3, shell4) = (shell.clone(), shell.clone(), shell.clone(), shell.clone());
                    let drag = match (&sel_drag, sel) {
                        (Some(d), true) => d.clone(),
                        _ => DragItem::shared(Rc::new(Payload::Lib(vec![t.clone()])), reorder.clone().map(|p| (p, Rc::new(vec![k])))),
                    };
                    let t_menu = t.clone();
                    // the row stops short of the scrollbar's lane on the right
                    div().w_full().h(px(row)).pr(px(GUTTER)).child(div()
                        .id(i)
                        .group("row")
                        .w_full()
                        .h(px(row))
                        .flex()
                        .items_center()
                        .gap_3p5()
                        .px_3()
                        .rounded_lg()
                        .when(sel, |d| d.bg(panel2()))
                        .when(!sel, |d| d.hover(|s| s.bg(hover())))
                        .on_drag(drag, |d, _, _, cx| drag_ghost(d, cx))
                        .when_some(reorder.clone(), |d, pid| {
                            let (pid2, shell5) = (pid.clone(), shell.clone());
                            // a line shows where the songs will land: below this row when moving down
                            d.drag_over::<DragItem>(move |st, d, _, _| match &d.from {
                                Some((p, idx)) if *p == pid && !idx.contains(&k) => {
                                    let st = st.rounded_none().border_color(accent());
                                    if idx.first().is_some_and(|&f| f < k) { st.border_b_2() } else { st.border_t_2() }
                                }
                                _ => st,
                            })
                            .on_drop(move |d: &DragItem, _, cx| {
                                if let Some((p, idx)) = &d.from {
                                    if *p == pid2 {
                                        let moved = idx.to_vec();
                                        shell5.update(cx, |s, cx| s.move_songs(moved, k, cx));
                                    }
                                }
                            })
                        })
                        .on_mouse_down(MouseButton::Right, move |ev: &MouseDownEvent, _, cx| {
                            let t = t_menu.clone();
                            shell4.update(cx, |s, cx| {
                                if !s.picked.contains(&k) {
                                    s.select_only(Some(k));
                                }
                                let sel = s.selection();
                                let tracks: Vec<crate_core::model::Track> = match s.model.read(cx).cache.get(&s.current_playlist_id(cx).unwrap_or_default()) {
                                    Some(pl) if sel.len() > 1 => sel.iter().filter_map(|&i| pl.tracks.get(i).cloned()).collect(),
                                    _ => vec![t.clone()],
                                };
                                let n = tracks.len();
                                let mut items = vec![];
                                if n == 1 {
                                    items.push(overlay::item(IconName::Play, "Play", move |s, _, cx| s.play_from(k, cx)));
                                } else {
                                    items.push(MenuItem::Header(format!("{n} songs selected").into()));
                                }
                                items.extend(Shell::queue_items(tracks.clone()));
                                items.push(MenuItem::Sep);
                                items.extend(s.add_items(Payload::Lib(tracks), true, cx));
                                if s.editable_pid(cx).is_some() {
                                    items.push(MenuItem::Sep);
                                    let label = if n == 1 { "Remove from this playlist".to_string() } else { format!("Remove {n} songs from this playlist") };
                                    items.push(overlay::item(IconName::ListMinus, label, |s, _, cx| s.remove_selected(cx)));
                                }
                                s.open_menu(ev.position, items, cx);
                            });
                        })
                        .child(
                            // number (or now-playing icon); turns into a play button on hover
                            div()
                                .relative()
                                .w(px(36.))
                                .h_full()
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    div()
                                        .group_hover("row", |s| s.invisible())
                                        .text_color(if cur { accent() } else { faint() })
                                        .child(if cur { now_playing_icon(playing).into_any_element() } else { format!("{}", n + 1).into_any_element() }),
                                )
                                .child(
                                    div()
                                        .id(("play-row", i))
                                        .absolute()
                                        .inset_0()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .invisible()
                                        .group_hover("row", |s| s.visible())
                                        .child(icon(IconName::Play, 16., text()))
                                        .on_click(move |_: &ClickEvent, _, cx| {
                                            cx.stop_propagation();
                                            shell2.update(cx, |s, cx| s.play_from(k, cx));
                                        }),
                                ),
                        )
                        .child(cover_el(covers.get(t.cover_art.as_deref(), SMALL), cov, 4.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(div().truncate().font_weight(FontWeight::MEDIUM).text_color(if cur { accent() } else { text() }).child(t.title.clone()))
                                .child(div().truncate().text_sm().text_color(dim()).child(t.artist.clone())),
                        )
                        .when(show_album, |d| {
                            let (album_id, shell3) = (t.album_id.clone(), shell3.clone());
                            d.child(
                                div().flex_1().min_w_0().flex().child(
                                    div()
                                        .id(("album-link", i))
                                        .min_w_0()
                                        .truncate()
                                        .text_sm()
                                        .text_color(dim())
                                        .when(!album_id.is_empty(), |d| d.cursor_pointer().hover(|s| s.text_color(text()).underline()))
                                        .child(t.album.clone())
                                        .on_click(move |_: &ClickEvent, _, cx| {
                                            if !album_id.is_empty() {
                                                cx.stop_propagation();
                                                let r = Route::Album(album_id.clone());
                                                shell3.update(cx, |s, cx| s.go(r, cx));
                                            }
                                        }),
                                ),
                            )
                        })
                        .when(show_date, |d| {
                            d.child(div().w(px(120.)).flex_none().text_sm().text_color(dim()).child(t.created.as_deref().map(|c| c.chars().take(10).collect::<String>()).unwrap_or_default()))
                        })
                        .child(div().w(px(56.)).flex_none().text_sm().text_color(faint()).flex().justify_end().child(fmt_time(t.duration)))
                        .on_click(move |ev: &ClickEvent, _, cx| {
                            shell.update(cx, |s, cx| {
                                if ev.click_count() >= 2 {
                                    s.play_from(k, cx);
                                } else {
                                    s.click_row(k, ev.modifiers(), cx);
                                }
                            });
                        }))
                })
                .collect()
        })
        .flex_1()
        .track_scroll(&self.list_scroll);

        let pl_id = pl.id.clone();
        let pl_id2 = pl.id.clone();
        let filter_w = (w * 0.32).clamp(150., 240.);
        let page_title: SharedString = if is_library { "Songs".into() } else { pl.name.clone().into() };
        let toolbar = div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .px(px(pad))
                    .h(px(BAR_H))
                    .flex_none()
                    .bg(bg())
                    .child(
                        div()
                            .id("pl-play")
                            .size(px(54.))
                            .flex_none()
                            .rounded_full()
                            .bg(accent())
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.opacity(0.9))
                            .tooltip(tip(if is_source_playing { "Pause" } else { "Play" }))
                            .child(icon(if is_source_playing { IconName::Pause } else { IconName::Play }, 24., accent_ink()))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                let pid = pl_id.clone();
                                let resume = {
                                    let m = this.model.read(cx);
                                    m.queue_source.as_deref() == Some(pid.as_str()) && m.player_state.index.is_some()
                                };
                                if resume {
                                    this.model.read(cx).send(Action::Toggle);
                                } else if let Some(&first) = this.visible_rows.first() {
                                    this.model.read(cx).send(Action::SetShuffle { on: false });
                                    this.play_from(first, cx);
                                }
                            })),
                    )
                    .child(
                        div()
                            .id("pl-shuffle")
                            .p_2()
                            .flex_none()
                            .rounded_full()
                            .cursor_pointer()
                            .hover(|s| s.bg(hover()))
                            .tooltip(tip("Shuffle play"))
                            .child(icon(IconName::Shuffle, 22., dim()))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                let _ = &pl_id2;
                                if this.visible_rows.is_empty() {
                                    return;
                                }
                                this.model.read(cx).send(Action::SetShuffle { on: true });
                                let k = this.visible_rows[random_index(this.visible_rows.len())];
                                this.play_from(k, cx);
                            })),
                    )
                    .when(editable, |d| {
                        let pid = pl.id.clone();
                        d.child(
                            div()
                                .id("pl-more")
                                .p_2()
                                .flex_none()
                                .rounded_full()
                                .cursor_pointer()
                                .hover(|s| s.bg(hover()))
                                .tooltip(tip("More options"))
                                .child(icon(IconName::Ellipsis, 22., dim()))
                                .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                                    let pos = ev.position();
                                    this.open_menu(pos, Shell::playlist_items(&pid), cx);
                                })),
                        )
                    })
                    .when(title_fade > 0., |d| {
                        // the page's name fades in here once its header has scrolled away
                        d.child(div().min_w_0().opacity(title_fade).text_xl().font_weight(FontWeight::BOLD).truncate().child(page_title.clone()))
                    })
                    .when(is_library, |d| d.child(div().text_color(dim()).truncate().child(format!("{} · {}", songs(pl.tracks.len()), fmt_long(total)))))
                    .child(div().flex_1())
                    .child(div().w(px(filter_w)).child(Input::new(&self.filter).cleanable(true).prefix(icon(IconName::Search, 14., faint()))));
        let cols = div()
                    .flex()
                    .items_center()
                    .gap_3p5()
                    .ml(px(pad - 8.))
                    .mr(px((pad - 8.).max(GUTTER)))
                    .px_3()
                    .h(px(36.))
                    .flex_none()
                    .border_b_1()
                    .border_color(line())
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(div().w(px(36.)).flex_none().flex().justify_center().text_color(faint()).child("#"))
                    .child(div().w(px(crate::theme::row_cover())).flex_none())
                    .child(header_btn(SortKey::Title, "TITLE", None, cx))
                    .when(show_album, |d| d.child(header_btn(SortKey::Album, "ALBUM", None, cx)))
                    .when(show_date, |d| d.child(header_btn(SortKey::Created, "DATE ADDED", Some(120.), cx)))
                    .child(div().w(px(56.)).flex_none().flex().justify_end().child(header_btn(SortKey::Duration, "TIME", None, cx)));
        if rows.is_empty() {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .children(top_el)
                .child(toolbar)
                .child(div().bg(bg()).child(cols))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .justify_center()
                        .pt_10()
                        .text_color(dim())
                        .child(if filter.is_empty() { "Nothing here yet.".to_string() } else { format!("No songs match \u{201c}{filter}\u{201d}.") }),
                )
                .into_any_element();
        }
        let header = div()
            .absolute()
            .top(px(-shift))
            .left_0()
            .right_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(head_h))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .justify_end()
                    .overflow_hidden()
                    .bg(if tinted { accent_wash() } else { bg() })
                    .children(top_el),
            )
            .child(toolbar)
            .child(div().bg(bg()).child(cols))
            .child(div().h(px(GAP)).bg(bg()));
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .pl(px(pad - 8.))
                    .pr(px((pad - 8. - GUTTER).max(0.)))
                    .flex()
                    .flex_col()
                    .on_drag_move::<DragItem>(cx.listener(|this, ev: &gpui::DragMoveEvent<DragItem>, _, cx| {
                        if ev.drag(cx).from.is_some() {
                            this.drag_edge(ev, cx);
                        }
                    }))
                    .child(scroll_area_bare(list, &self.list_scroll, &self.list_smooth, cx.entity_id())),
            )
            .child(header)
            // drawn last so it sits above the header, at the page's right edge
            .child(Scrollbar::vertical(&self.list_scroll).mode(ScrollbarMode::Always))
            .into_any_element()
    }

    // ------------------------------------------------------------ queue panel
    fn queue_panel(&self, lay: Layout, cx: &mut Context<Self>) -> impl IntoElement {
        // only copy the part of the queue that's shown (it can hold thousands of songs)
        let (covers, idx, queue_empty, now, upcoming) = {
            let m = self.model.read(cx);
            let q = &m.player_state.queue;
            let idx = m.player_state.index;
            let from = idx.map(|i| i + 1).unwrap_or(0);
            let upcoming: Vec<(usize, crate_core::model::Track)> = q.iter().enumerate().skip(from).take(200).map(|(k, t)| (k, t.clone())).collect();
            let now = idx.and_then(|i| q.get(i).map(|t| (i, t.clone())));
            (m.covers.clone(), idx, q.is_empty(), now, upcoming)
        };
        let _ = idx;
        let item = |k: usize, t: &crate_core::model::Track, cur: bool, cx: &mut Context<Self>| {
            div()
                .id(SharedString::from(format!("q-{k}")))
                .group("qrow")
                .flex()
                .items_center()
                .gap_2p5()
                .px_2()
                .py_1p5()
                .rounded_lg()
                .when(!cur, |d| d.cursor_pointer())
                .hover(|s| s.bg(hover()))
                .when(!cur, |d| d.tooltip(tip("Click to play")))
                .child(
                    div()
                        .relative()
                        .flex_none()
                        .child(cover_el(covers.get(t.cover_art.as_deref(), SMALL), 40., 4.))
                        .when(!cur, |d| {
                            d.child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .rounded(px(4.))
                                    .bg(veil(0x99))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .invisible()
                                    .group_hover("qrow", |s| s.visible())
                                    .child(icon(IconName::Play, 16., text())),
                            )
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().truncate().text_sm().font_weight(FontWeight::MEDIUM).text_color(if cur { accent() } else { text() }).child(t.title.clone()))
                        .child(div().truncate().text_xs().text_color(dim()).child(t.artist.clone())),
                )
                .when(!cur, |d| {
                    d.child(
                        div()
                            .id(SharedString::from(format!("qx-{k}")))
                            .p_1()
                            .rounded_md()
                            .cursor_pointer()
                            .invisible()
                            .group_hover("qrow", |s| s.visible())
                            .hover(|s| s.bg(panel2()))
                            .tooltip(tip("Remove from queue"))
                            .child(icon(IconName::X, 14., dim()))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                this.model.read(cx).send(Action::Remove { index: k });
                            })),
                    )
                })
                .when(!cur, |d| d.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.model.read(cx).send(Action::Jump { index: k }))))
                .into_any_element()
        };
        let mut children: Vec<AnyElement> = vec![];
        if let Some((i, t)) = &now {
            children.push(div().px_2().pt_3().pb_1p5().text_xs().font_weight(FontWeight::BOLD).text_color(faint()).child("NOW PLAYING").into_any_element());
            children.push(item(*i, t, true, cx));
        }
        children.push(div().px_2().pt_4().pb_1p5().text_xs().font_weight(FontWeight::BOLD).text_color(faint()).child("NEXT UP").into_any_element());
        if upcoming.is_empty() {
            children.push(div().px_2().py_4().text_sm().text_color(dim()).child(if queue_empty { "Nothing queued. Double-click a song to start." } else { "End of queue." }).into_any_element());
        }
        for (k, t) in &upcoming {
            children.push(item(*k, t, false, cx));
        }
        let panel = self.model.read(cx).panel;
        let (n_active, any_finished) = {
            let m = self.model.read(cx);
            (m.active_downloads(), m.jobs.iter().any(|j| !j.status.active()))
        };
        let has_upcoming = if panel == Panel::Downloads { any_finished } else { !upcoming.is_empty() };
        if panel == Panel::Downloads {
            children = self.downloads_body(cx);
        }
        let tab = |id: &'static str, label: String, p: Panel, cx: &mut Context<Self>| {
            let on = panel == p;
            div()
                .id(id)
                .px_2p5()
                .py_1()
                .rounded_full()
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .cursor_pointer()
                .when(on, |d| d.bg(panel2()).text_color(text()))
                .when(!on, |d| d.text_color(dim()).hover(|s| s.text_color(text())))
                .child(label)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.model.update(cx, |m, cx| {
                        if m.panel != p {
                            m.toggle_panel(p, cx)
                        }
                    })
                }))
        };
        let tabs = div()
            .flex_1()
            .flex()
            .gap_1()
            .child(tab("tab-queue", "Queue".into(), Panel::Queue, cx))
            .child(tab("tab-dl", if n_active > 0 { format!("Downloads ({n_active})") } else { "Downloads".into() }, Panel::Downloads, cx));
        div()
            .w(px(lay.queue_w))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(side())
            .border_l_1()
            .border_color(line())
            .when(lay.queue_overlay, |d| d.absolute().top_0().right_0().bottom_0().shadow_2xl().occlude())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .pt_4()
                    .pb_2()
                    .child(tabs)
                    .when(has_upcoming, |d| {
                        d.child(
                            div()
                                .id("q-clear")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(dim())
                                .cursor_pointer()
                                .hover(|s| s.text_color(text()).bg(hover()))
                                .tooltip(tip(if panel == Panel::Downloads { "Remove finished and failed downloads" } else { "Remove everything after the current song" }))
                                .child("Clear")
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    if panel == Panel::Downloads {
                                        this.model.read(cx).clear_downloads()
                                    } else {
                                        this.model.read(cx).send(Action::ClearQueue)
                                    }
                                })),
                        )
                    })
                    .child(
                        div()
                            .id("q-close")
                            .p_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(hover()))
                            .tooltip(tip("Close"))
                            .child(icon(IconName::X, 16., dim()))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.toggle_panel(panel, cx)))),
                    ),
            )
            .child(scroll_area(
                div().id("q-list").size_full().overflow_y_scroll().track_scroll(&self.queue_scroll).pl_2p5().pr(px(GUTTER)).pb_4().flex().flex_col().children(children),
                &self.queue_scroll,
                &self.queue_smooth,
                cx.entity_id(),
            ))
    }
}

struct BenchTimer(std::time::Instant);
impl Drop for BenchTimer {
    fn drop(&mut self) {
        let ms = self.0.elapsed().as_secs_f64() * 1000.;
        BENCH.with(|b| b.borrow_mut().0.push(ms));
    }
}

thread_local! {
    /// CRATE_BENCH: (render time, frame interval) samples in milliseconds.
    static BENCH: std::cell::RefCell<(Vec<f64>, Vec<f64>)> = Default::default();
}

impl Shell {
    /// CRATE_BENCH=<file>: scroll the song list down continuously, like a fast wheel flick,
    /// and write render-time / frame-interval stats to <file>.
    pub fn bench_scroll(&mut self, out: String, window: &mut Window, cx: &mut Context<Self>) {
        fn tick(h: UniformListScrollHandle, view: EntityId, out: String, start: std::time::Instant, last: std::time::Instant, window: &mut Window) {
            window.on_next_frame(move |window, cx| {
                let now = std::time::Instant::now();
                BENCH.with(|b| b.borrow_mut().1.push((now - last).as_secs_f64() * 1000.));
                let o = ScrollbarHandle::offset(&h);
                ScrollbarHandle::set_offset(&h, gpui::point(o.x, o.y - px(14.)));
                cx.notify(view);
                if (now - start).as_secs_f64() < 6.0 {
                    tick(h, view, out, start, now, window);
                } else {
                    BENCH.with(|b| {
                        let (r, f) = &mut *b.borrow_mut();
                        let stat = |v: &mut Vec<f64>| {
                            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
                            let p = |q: f64| v.get(((v.len() as f64 - 1.) * q) as usize).copied().unwrap_or(0.);
                            format!("n={} p50={:.2} p90={:.2} p99={:.2} max={:.2}", v.len(), p(0.5), p(0.9), p(0.99), p(1.0))
                        };
                        let txt = format!("render_ms {}\nframe_ms  {}\n", stat(r), stat(f));
                        let _ = std::fs::write(&out, txt);
                    });
                    if std::env::var("CRATE_BENCH_STAY").is_err() {
                        cx.quit();
                    }
                }
            });
        }
        BENCH.with(|b| *b.borrow_mut() = Default::default());
        let now = std::time::Instant::now();
        tick(self.list_scroll.clone(), cx.entity_id(), out, now, now, window);
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _t = BenchTimer(std::time::Instant::now());
        self.drag_scroll_step(window, cx);
        let route = self.model.read(cx).route.clone();
        let queue_open = self.model.read(cx).panel != Panel::None;
        window.set_rem_size(px(16. * crate::theme::text_pct() as f32 / 100.));
        let lay = Layout::new(f32::from(window.viewport_size().width), queue_open);

        let page_now = (route.clone(), self.model.read(cx).lib_tab);
        if self.last_page.as_ref() != Some(&page_now) {
            if self.last_page.is_some() {
                self.filter.update(cx, |f, cx| f.set_value("", window, cx));
            }
            self.last_page = Some(page_now);
            self.select_only(None);
            for h in [&self.list_scroll, &self.grid_scroll] {
                ScrollbarHandle::set_offset(h, gpui::point(px(0.), px(0.)));
            }
            self.home_scroll.set_offset(gpui::point(px(0.), px(0.)));
        }
        if !matches!(route, Route::Setup(_)) {
            // a setup step left halfway starts fresh next time
            self.setup = None;
        }
        if route != Route::Settings {
            *self.accounts.borrow_mut() = None; // re-read when Settings opens again
        }
        let page = match &route {
            Route::Home => self.home(lay, cx),
            Route::Playlist(id) => {
                let id = id.clone();
                self.playlist_page(&id, lay, cx)
            }
            Route::Library => self.library_page(lay, cx),
            Route::Album(id) => {
                let id = id.clone();
                self.album_page(&id, lay, cx)
            }
            Route::Artist(id) => {
                let id = id.clone();
                self.artist_page(&id, lay, cx)
            }
            Route::Search => self.search_page(lay, cx),
            Route::DzAlbum(id) => self.dz_album_page(*id, lay, cx),
            Route::DzArtist(id) => self.dz_artist_page(*id, lay, cx),
            Route::Lyrics => self.lyrics_view.clone().into_any_element(),
            Route::Settings => self.settings_page(lay, cx),
            Route::Setup(step) => self.setup_page(*step, window, cx),
            Route::Import => self.import_page(lay, cx),
        };
        let overlays = self.overlays(cx);
        let filter_focus = gpui::Focusable::focus_handle(self.filter.read(cx), cx);
        let search_focus = gpui::Focusable::focus_handle(self.search_input.read(cx), cx);
        let prompt_focus = self.prompt.as_ref().and_then(|p| p.input.as_ref()).map(|i| gpui::Focusable::focus_handle(i.read(cx), cx));
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(bg())
            .text_color(text())
            .font_family("Inter")
            .image_cache(self.images.clone())
            .key_context("Shell")
            .track_focus(&self.focus)
            // clicking anywhere outside the text box hands keyboard shortcuts back to the app
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                let typing = filter_focus.is_focused(window) || search_focus.is_focused(window) || prompt_focus.as_ref().map(|f| f.is_focused(window)).unwrap_or(false);
                if !typing {
                    window.focus(&this.focus, cx);
                }
            }))
            .on_mouse_down(MouseButton::Navigate(NavigationDirection::Back), cx.listener(|this, _: &MouseDownEvent, _, cx| this.model.update(cx, |m, cx| m.back(cx))))
            .on_mouse_down(MouseButton::Navigate(NavigationDirection::Forward), cx.listener(|this, _: &MouseDownEvent, _, cx| this.model.update(cx, |m, cx| m.forward(cx))))
            .on_action(cx.listener(|this, _: &TogglePlay, _, cx| this.model.read(cx).send(Action::Toggle)))
            .on_action(cx.listener(|this, _: &NextTrack, _, cx| this.model.read(cx).send(Action::Next)))
            .on_action(cx.listener(|this, _: &PrevTrack, _, cx| this.model.read(cx).send(Action::Prev)))
            .on_action(cx.listener(|this, _: &SeekForward, _, cx| this.model.read(cx).send(Action::SeekBy { delta: 5.0 })))
            .on_action(cx.listener(|this, _: &SeekBack, _, cx| this.model.read(cx).send(Action::SeekBy { delta: -5.0 })))
            .on_action(cx.listener(|this, _: &VolumeUp, _, cx| this.model.update(cx, |m, cx| m.set_volume(m.player_state.volume + 0.05, cx))))
            .on_action(cx.listener(|this, _: &VolumeDown, _, cx| this.model.update(cx, |m, cx| m.set_volume(m.player_state.volume - 0.05, cx))))
            .on_action(cx.listener(|this, _: &GoBack, _, cx| this.model.update(cx, |m, cx| m.back(cx))))
            .on_action(cx.listener(|this, _: &GoForward, _, cx| this.model.update(cx, |m, cx| m.forward(cx))))
            .on_action(cx.listener(|this, _: &ToggleQueue, _, cx| this.model.update(cx, |m, cx| m.toggle_queue(cx))))
            .on_action(cx.listener(|this, _: &ToggleLyrics, _, cx| this.model.update(cx, |m, cx| m.toggle_lyrics(cx))))
            .on_action(cx.listener(|this, _: &CycleLyrMode, _, cx| {
                this.model.update(cx, |m, cx| {
                    use crate::model::LyrMode::*;
                    if m.route == Route::Lyrics {
                        let next = match m.lyr_mode {
                            Original => Both,
                            Both => Romaji,
                            Romaji => Original,
                        };
                        m.set_lyr_mode(next, cx);
                    }
                })
            }))
            .on_action(cx.listener(|this, _: &SelectUp, _, cx| this.move_selection(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectDown, _, cx| this.move_selection(1, cx)))
            .on_action(cx.listener(|this, _: &RemoveSelected, _, cx| this.remove_selected(cx)))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| this.go(Route::Settings, cx)))
            .on_action(cx.listener(|this, _: &UndoEdit, _, cx| this.model.update(cx, |m, cx| m.undo(cx))))
            .on_action(cx.listener(|this, _: &MoveUp, _, cx| this.nudge(-1, cx)))
            .on_action(cx.listener(|this, _: &MoveDown, _, cx| this.nudge(1, cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
                if this.current_playlist_id(cx).is_some() {
                    this.picked = Rc::new(this.visible_rows.iter().copied().collect());
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &Dismiss, window, cx| {
                if this.prompt.is_some() {
                    this.close_prompt(window, cx);
                } else if this.menu.is_some() {
                    this.menu = None;
                    cx.notify();
                } else {
                    this.select_only(None);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &PlaySelected, window, cx| {
                if this.prompt.is_some() {
                    return this.submit_prompt(window, cx);
                }
                if let Some(k) = this.selected {
                    this.play_from(k, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &FindInPage, window, cx| {
                if matches!(this.model.read(cx).route, Route::Search) {
                    this.search_input.update(cx, |f, cx| f.focus(window, cx));
                } else if this.current_playlist_id(cx).is_some() {
                    this.filter.update(cx, |f, cx| f.focus(window, cx));
                }
            }))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.sidebar(lay, cx))
                    .child(
                        // a quick fade/rise as each page opens (off with Settings → Animations)
                        div().flex_1().min_w_0().h_full().child(page).with_animation(
                            SharedString::from(format!("page-{route:?}")),
                            gpui::Animation::new(std::time::Duration::from_millis(200)).with_easing(gpui::ease_out_quint()),
                            |d, t| d.opacity(0.35 + 0.65 * t).mt(px((1. - t) * 10.)),
                        ),
                    )
                    .when(queue_open, |d| {
                        let which = format!("panel-{:?}", self.model.read(cx).panel);
                        d.child(div().h_full().flex_none().flex().child(self.queue_panel(lay, cx)).with_animation(
                            SharedString::from(which),
                            gpui::Animation::new(std::time::Duration::from_millis(220)).with_easing(gpui::ease_out_quint()),
                            |d, t| d.opacity(t).ml(px((1. - t) * 24.)),
                        ))
                    })
                    .children(overlays),
            )
            .child(self.bar.clone())
    }
}
