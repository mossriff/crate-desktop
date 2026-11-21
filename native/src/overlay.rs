//! Things drawn on top of the page: right-click / "+" menus, the new-playlist prompt, toasts,
//! and the label that follows the cursor while dragging songs.
use super::*;
use crate::catalog::Payload;
use gpui::{anchored, deferred, AnimationExt, Point, Pixels};

pub(super) type MenuAction = Rc<dyn Fn(&mut Shell, &mut Window, &mut Context<Shell>)>;

pub(super) enum MenuItem {
    Header(SharedString),
    Sep,
    Item { icon: IconName, label: SharedString, act: MenuAction },
    /// one of several options; the current one is ticked
    Choice { label: SharedString, on: bool, act: MenuAction },
}

pub(super) fn choice(label: impl Into<SharedString>, on: bool, act: impl Fn(&mut Shell, &mut Window, &mut Context<Shell>) + 'static) -> MenuItem {
    MenuItem::Choice { label: label.into(), on, act: Rc::new(act) }
}

pub(super) fn item(icon: IconName, label: impl Into<SharedString>, act: impl Fn(&mut Shell, &mut Window, &mut Context<Shell>) + 'static) -> MenuItem {
    MenuItem::Item { icon, label: label.into(), act: Rc::new(act) }
}

pub(super) struct Menu {
    pub pos: Point<Pixels>,
    pub items: Vec<MenuItem>,
}

pub(super) enum PromptKind {
    /// create a playlist, then add these songs to it
    NewPlaylist(Option<Payload>),
    Rename(String),
    Describe(String),
    Delete(String),
}

pub(super) struct Prompt {
    /// None for a plain yes/no question
    pub input: Option<Entity<InputState>>,
    pub kind: PromptKind,
    pub _sub: Option<Subscription>,
}

/// What's being dragged (songs or an album), plus its label. Shared (`Rc`) because every
/// selected row carries the whole selection and is rebuilt each frame.
#[derive(Clone)]
pub(super) struct DragItem {
    pub payload: Rc<Payload>,
    pub label: SharedString,
    /// dragged out of this playlist, at these indices (so dropping on its rows reorders)
    pub from: Option<(String, Rc<Vec<usize>>)>,
}

impl DragItem {
    pub fn new(payload: Payload) -> Self {
        Self::shared(Rc::new(payload), None)
    }
    pub fn shared(payload: Rc<Payload>, from: Option<(String, Rc<Vec<usize>>)>) -> Self {
        let label = payload.label().into();
        DragItem { payload, label, from }
    }
}

pub(super) struct DragGhost(SharedString);

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_1p5()
            .rounded_lg()
            .bg(accent())
            .text_color(accent_ink())
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .font_family("Inter")
            .shadow_lg()
            .child(self.0.clone())
    }
}

pub(super) fn drag_ghost(d: &DragItem, cx: &mut App) -> Entity<DragGhost> {
    cx.new(|_| DragGhost(d.label.clone()))
}

impl Shell {
    /// CRATE_DEMO_MENU test hook.
    pub fn demo_overlays(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut items = Shell::queue_items(vec![]);
        items.push(MenuItem::Sep);
        items.extend(self.add_items(Payload::Lib(vec![]), true, cx));
        self.open_menu(gpui::point(px(600.), px(300.)), items, cx);
        self.model.update(cx, |m, cx| m.toast("\u{201c}Song title\u{201d} \u{2192} Playlist", false, cx));
        if std::env::var("CRATE_DEMO_MENU").as_deref() == Ok("prompt") {
            self.menu = None;
            self.ask_playlist_name(None, window, cx);
        }
    }

    pub(super) fn open_menu(&mut self, pos: Point<Pixels>, items: Vec<MenuItem>, cx: &mut Context<Self>) {
        self.menu = Some(Menu { pos, items });
        cx.notify();
    }

    /// "Add to playlist" entries (plus "New playlist…" and download-only) for a payload.
    pub(super) fn add_items(&self, payload: Payload, all_owned: bool, cx: &App) -> Vec<MenuItem> {
        let mut v = vec![MenuItem::Header("Add to playlist".into())];
        for p in &self.model.read(cx).playlists {
            let (pid, pl) = (p.id.clone(), payload.clone());
            v.push(item(IconName::ListPlus, p.name.clone(), move |s, _, cx| {
                let (pid, pl) = (pid.clone(), pl.clone());
                s.model.update(cx, |m, cx| m.add_to(Some(pid), pl, cx));
            }));
        }
        let pl = payload.clone();
        v.push(item(IconName::Plus, "New playlist…", move |s, w, cx| s.ask_playlist_name(Some(pl.clone()), w, cx)));
        if !matches!(payload, Payload::Lib(_)) {
            v.push(MenuItem::Sep);
            let label = if all_owned { "Already in your library" } else { "Download only (no playlist)" };
            v.push(item(IconName::Download, label, move |s, _, cx| {
                let pl = payload.clone();
                s.model.update(cx, |m, cx| m.add_to(None, pl, cx));
            }));
        }
        v
    }

    /// Play next / Add to queue entries for library songs.
    pub(super) fn queue_items(tracks: Vec<crate_core::model::Track>) -> Vec<MenuItem> {
        let t2 = tracks.clone();
        vec![
            item(IconName::ListStart, "Play next", move |s, _, cx| {
                let n = tracks.len();
                s.model.update(cx, |m, cx| {
                    m.send(Action::PlayNext { tracks: tracks.clone() });
                    m.toast(if n == 1 { "Will play next".to_string() } else { format!("{n} songs will play next") }, false, cx);
                });
            }),
            item(IconName::ListEnd, "Add to queue", move |s, _, cx| {
                let n = t2.len();
                s.model.update(cx, |m, cx| {
                    m.send(Action::Enqueue { tracks: t2.clone() });
                    m.toast(if n == 1 { "Added to queue".to_string() } else { format!("Added {n} songs to queue") }, false, cx);
                });
            }),
        ]
    }

    pub(super) fn ask_playlist_name(&mut self, then: Option<Payload>, window: &mut Window, cx: &mut Context<Self>) {
        self.open_prompt(PromptKind::NewPlaylist(then), window, cx);
    }

    pub(super) fn open_prompt(&mut self, kind: PromptKind, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        let (placeholder, initial) = {
            let m = self.model.read(cx);
            match &kind {
                PromptKind::NewPlaylist(_) => (Some("Playlist name"), String::new()),
                PromptKind::Rename(pid) => (Some("Playlist name"), m.playlist_name_of(pid)),
                PromptKind::Describe(pid) => (Some("Add an optional description"), m.cache.get(pid).and_then(|p| p.comment.clone()).unwrap_or_default()),
                PromptKind::Delete(_) => (None, String::new()),
            }
        };
        let (input, sub) = match placeholder {
            None => {
                // nothing to type: let Enter / Escape answer the question
                window.focus(&self.focus, cx);
                (None, None)
            }
            Some(ph) => {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder(ph));
                input.update(cx, |i, cx| {
                    i.set_value(initial, window, cx);
                    i.focus(window, cx);
                });
                let sub = cx.subscribe_in(&input, window, |this: &mut Self, _, ev: &InputEvent, window, cx| {
                    if matches!(ev, InputEvent::PressEnter { .. }) {
                        this.submit_prompt(window, cx);
                    }
                });
                (Some(input), Some(sub))
            }
        };
        self.prompt = Some(Prompt { input, kind, _sub: sub });
        cx.notify();
    }

    pub(super) fn close_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(super) fn submit_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.prompt.take() else { return };
        let value = p.input.as_ref().map(|i| i.read(cx).value().trim().to_string()).unwrap_or_default();
        window.focus(&self.focus, cx);
        self.model.update(cx, |m, cx| match p.kind {
            PromptKind::NewPlaylist(then) if !value.is_empty() => m.create_playlist(value, then, cx),
            PromptKind::Rename(pid) if !value.is_empty() => m.rename_playlist(pid, value, cx),
            PromptKind::Describe(pid) => m.set_playlist_comment(pid, value, cx),
            PromptKind::Delete(pid) => m.delete_playlist(pid, cx),
            _ => {}
        });
        cx.notify();
    }

    /// Rename / describe / delete entries for a playlist.
    pub(super) fn playlist_items(pid: &str) -> Vec<MenuItem> {
        let (a, b, c) = (pid.to_string(), pid.to_string(), pid.to_string());
        vec![
            item(IconName::Pencil, "Rename…", move |s, w, cx| s.open_prompt(PromptKind::Rename(a.clone()), w, cx)),
            item(IconName::Pencil, "Edit description…", move |s, w, cx| s.open_prompt(PromptKind::Describe(b.clone()), w, cx)),
            MenuItem::Sep,
            item(IconName::Trash, "Delete playlist…", move |s, w, cx| s.open_prompt(PromptKind::Delete(c.clone()), w, cx)),
        ]
    }

    pub(super) fn overlays(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = vec![];
        if let Some(menu) = &self.menu {
            let items: Vec<AnyElement> = menu
                .items
                .iter()
                .enumerate()
                .map(|(i, it)| match it {
                    MenuItem::Header(t) => div().px_3().pt_2().pb_1().text_xs().font_weight(FontWeight::BOLD).text_color(faint()).child(t.clone()).into_any_element(),
                    MenuItem::Sep => div().my_1().h(px(1.)).bg(line()).into_any_element(),
                    MenuItem::Item { .. } | MenuItem::Choice { .. } => {
                        let (ic, label, act, on) = match it {
                            MenuItem::Item { icon, label, act } => (Some(*icon), label, act.clone(), false),
                            MenuItem::Choice { label, on, act } => (if *on { Some(IconName::Check) } else { None }, label, act.clone(), *on),
                            _ => unreachable!(),
                        };
                        div()
                            .id(("menu-item", i))
                            .flex()
                            .items_center()
                            .gap_2p5()
                            .px_3()
                            .py_1p5()
                            .rounded_md()
                            .text_sm()
                            .cursor_pointer()
                            .hover(|s| s.bg(hover()))
                            .child(match ic {
                                Some(ic) => icon(ic, 15., if on { accent() } else { dim() }).into_any_element(),
                                None => div().size(px(15.)).flex_none().into_any_element(),
                            })
                            .child(div().truncate().when(on, |d| d.text_color(accent())).child(label.clone()))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.menu = None;
                                act(this, window, cx);
                                cx.notify();
                            }))
                            .into_any_element()
                    }
                })
                .collect();
            out.push(
                deferred(
                    anchored().position(menu.pos).snap_to_window_with_margin(px(8.)).child(
                        div()
                            .id("menu")
                            .occlude()
                            .w(px(240.))
                            .max_h(px(420.))
                            .overflow_y_scroll()
                            .p_1()
                            .rounded_lg()
                            .bg(panel2())
                            .border_1()
                            .border_color(line())
                            .shadow_2xl()
                            .text_color(text())
                            .font_family("Inter")
                            .children(items)
                            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                this.menu = None;
                                cx.notify();
                            }))
                            .with_animation(
                                SharedString::from(format!("menu-{}-{}", f32::from(menu.pos.x), f32::from(menu.pos.y))),
                                gpui::Animation::new(std::time::Duration::from_millis(140)).with_easing(gpui::ease_out_quint()),
                                |d, t| d.opacity(t).mt(px((1. - t) * -6.)),
                            ),
                    ),
                )
                .with_priority(2)
                .into_any_element(),
            );
        }
        if let Some(p) = &self.prompt {
            let (title, ok, danger): (SharedString, &'static str, bool) = match &p.kind {
                PromptKind::NewPlaylist(_) => ("New playlist".into(), "Create", false),
                PromptKind::Rename(_) => ("Rename playlist".into(), "Save", false),
                PromptKind::Describe(_) => ("Playlist description".into(), "Save", false),
                PromptKind::Delete(pid) => (format!("Delete \u{201c}{}\u{201d}?", self.model.read(cx).playlist_name_of(pid)).into(), "Delete", true),
            };
            let body: AnyElement = match &p.input {
                Some(i) => Input::new(i).into_any_element(),
                None => div()
                    .text_sm()
                    .text_color(dim())
                    .child("The songs stay in your library. Changed your mind? Ctrl+Z brings it back.")
                    .into_any_element(),
            };
            out.push(
                deferred(
                    div()
                        .id("prompt-backdrop")
                        .occlude()
                        .absolute()
                        .inset_0()
                        .bg(veil(0x99))
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.close_prompt(window, cx)))
                        .child(
                            div()
                                .id("prompt")
                                .w(px(380.))
                                .p_5()
                                .rounded_xl()
                                .bg(panel())
                                .border_1()
                                .border_color(line())
                                .shadow_2xl()
                                .flex()
                                .flex_col()
                                .gap_4()
                                .on_click(|_: &ClickEvent, _, cx| cx.stop_propagation())
                                .child(div().text_lg().font_weight(FontWeight::BOLD).child(title))
                                .child(body)
                                .child(
                                    div()
                                        .flex()
                                        .justify_end()
                                        .gap_2()
                                        .child(
                                            div()
                                                .id("prompt-cancel")
                                                .px_4()
                                                .py_2()
                                                .rounded_full()
                                                .cursor_pointer()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .hover(|s| s.bg(hover()))
                                                .child("Cancel")
                                                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.close_prompt(window, cx))),
                                        )
                                        .child(
                                            div()
                                                .id("prompt-ok")
                                                .px_4()
                                                .py_2()
                                                .rounded_full()
                                                .cursor_pointer()
                                                .when(!danger, |d| d.bg(accent()).text_color(accent_ink()))
                                                .when(danger, |d| d.bg(gpui::rgb(0xd9534f)).text_color(gpui::white()))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .hover(|s| s.opacity(0.9))
                                                .child(ok)
                                                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.submit_prompt(window, cx))),
                                        ),
                                ),
                        ),
                )
                .with_priority(3)
                .into_any_element(),
            );
        }
        let toast_seq = self.model.read(cx).toast_seq;
        if let Some(t) = self.model.read(cx).toast.clone() {
            out.push(
                div()
                    .absolute()
                    .bottom(px(16.))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .max_w(px(520.))
                            .px_4()
                            .py_2p5()
                            .rounded_lg()
                            .bg(if t.bad { gpui::rgb(0x7a2a2a) } else { panel2() })
                            .border_1()
                            .border_color(line())
                            .shadow_lg()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(t.text)
                            .with_animation(
                                ("toast", toast_seq as usize),
                                gpui::Animation::new(std::time::Duration::from_millis(220)).with_easing(gpui::ease_out_quint()),
                                |d, t| d.opacity(t).mb(px((1. - t) * -14.)),
                            ),
                    )
                    .into_any_element(),
            );
        }
        out
    }
}

/// Convenience: a menu-opening "+" button for catalog rows / album pages.
pub(super) fn plus_button(id: impl Into<gpui::ElementId>) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .p_1p5()
        .rounded_full()
        .cursor_pointer()
        .hover(|s| s.bg(hover()))
        .tooltip(tip("Add to playlist"))
        .child(icon(IconName::Plus, 16., dim()))
}

/// CRATE_DEMO_EDIT test hook: drives playlist editing through the real UI code on a throwaway
/// playlist ("zz crate ui test", deleted again at the end) and checks Navidrome after each step.
pub fn demo_edit(shell: Entity<Shell>, window: &mut Window, cx: &mut App) {
    let client = shell.read(cx).model.read(cx).client.clone();
    window
        .spawn(cx, async move |cx| {
            let ex = cx.background_executor().clone();
            let wait = |s: f32| ex.timer(std::time::Duration::from_secs_f32(s));
            let c = client.clone();
            let server = move |id: String| {
                let c = c.clone();
                async move { c.playlist(&id).map(|p| (p.name, p.comment, p.tracks.into_iter().map(|t| t.id).collect::<Vec<_>>())) }
            };
            let check = |what: &str, ok: bool| eprintln!("EDIT {} {what}", if ok { "ok  " } else { "FAIL" });
            wait(4.).await;
            let c = client.clone();
            let (id, ids) = cx
                .background_executor()
                .spawn(async move {
                    let ids: Vec<String> = c.search_songs("", 5)?.into_iter().map(|t| t.id).collect();
                    let id = c.create_playlist("zz crate ui test")?;
                    c.playlist_replace(&id, &[], &ids)?;
                    anyhow::Ok((id, ids))
                })
                .await
                .expect("setup");
            let pick = |v: &[usize]| v.iter().map(|&i| ids[i].clone()).collect::<Vec<_>>();
            let _ = cx.update(|_, cx| shell.update(cx, |s, cx| s.model.update(cx, |m, cx| {
                m.reload_playlists(cx);
                m.go(Route::Playlist(id.clone()), cx);
            })));
            wait(2.5).await;
            // drag rows 1-2 onto row 4
            let _ = cx.update(|_, cx| shell.update(cx, |s, cx| {
                s.select_only(Some(0));
                s.click_row(1, gpui::Modifiers { shift: true, ..Default::default() }, cx);
                let sel = s.selection();
                s.move_songs(sel, 3, cx);
            }));
            wait(2.).await;
            let r = cx.background_executor().spawn(server(id.clone())).await.unwrap();
            check("move 2 songs down", r.2 == pick(&[2, 3, 0, 1, 4]));
            let _ = cx.update(|_, cx| shell.update(cx, |s, cx| s.remove_selected(cx)));
            wait(2.).await;
            let r = cx.background_executor().spawn(server(id.clone())).await.unwrap();
            check("remove the 2 selected", r.2 == pick(&[2, 3, 4]));
            let _ = cx.update(|_, cx| shell.update(cx, |s, cx| s.model.update(cx, |m, cx| m.undo(cx))));
            wait(2.).await;
            let r = cx.background_executor().spawn(server(id.clone())).await.unwrap();
            check("undo remove", r.2 == pick(&[2, 3, 0, 1, 4]));
            let _ = cx.update(|_, cx| shell.update(cx, |s, cx| {
                s.select_only(Some(4));
                s.nudge(-1, cx); // Alt+Up
            }));
            wait(2.).await;
            let r = cx.background_executor().spawn(server(id.clone())).await.unwrap();
            check("alt+up", r.2 == pick(&[2, 3, 0, 4, 1]));
            let _ = cx.update(|_, cx| shell.update(cx, |s, cx| s.model.update(cx, |m, cx| {
                m.rename_playlist(id.clone(), "zz crate ui test renamed".into(), cx);
                m.set_playlist_comment(id.clone(), "test description".into(), cx);
            })));
            wait(2.).await;
            let r = cx.background_executor().spawn(server(id.clone())).await.unwrap();
            check("rename + description", r.0 == "zz crate ui test renamed" && r.1.as_deref() == Some("test description"));
            // leave the delete question on screen for the screenshot
            let _ = cx.update(|window, cx| shell.update(cx, |s, cx| {
                s.picked = Rc::new([0, 1, 2].into_iter().collect());
                s.open_prompt(PromptKind::Delete(id.clone()), window, cx);
            }));
            wait(3.).await;
            let _ = cx.update(|window, cx| shell.update(cx, |s, cx| s.submit_prompt(window, cx)));
            wait(2.).await;
            let c = client.clone();
            let id2 = id.clone();
            let gone = cx.background_executor().spawn(async move { c.playlists().map(|p| p.iter().all(|p| p.id != id2)) }).await.unwrap();
            let on_home = cx.update(|_, cx| shell.read(cx).model.read(cx).route == Route::Home).unwrap_or(false);
            check("delete (and leave its page)", gone && on_home);
            let _ = cx.update(|_, cx| shell.update(cx, |s, cx| s.model.update(cx, |m, cx| m.undo(cx))));
            wait(3.).await;
            let c = client.clone();
            let restored = cx
                .background_executor()
                .spawn(async move {
                    let mine: Vec<_> = c.playlists()?.into_iter().filter(|p| p.name.starts_with("zz crate ui test")).collect();
                    let r = match mine.first() {
                        Some(p) => {
                            let pl = c.playlist(&p.id)?;
                            Some((pl.tracks.into_iter().map(|t| t.id).collect::<Vec<_>>(), pl.comment))
                        }
                        None => None,
                    };
                    for p in &mine {
                        c.delete_playlist(&p.id)?; // clean up
                    }
                    anyhow::Ok(r)
                })
                .await
                .unwrap();
            check("undo delete", restored == Some((pick(&[2, 3, 0, 4, 1]), Some("test description".into()))));
            eprintln!("EDIT done");
        })
        .detach();
}
