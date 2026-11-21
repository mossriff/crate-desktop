//! Search (your library + the online catalog) and catalog album / artist pages.
use super::library::{CoverRef, Card};
use super::*;
use crate::catalog::{dz_album_route, dz_artist_route, Payload, Results, SearchKind};
use super::overlay::{drag_ghost, item, plus_button, DragItem, MenuItem};
use crate_core::deezer::{DzAlbum, DzArtist, DzTrack};
use crate_core::model::Track;

fn link(id: impl Into<gpui::ElementId>, label: impl Into<SharedString>, route: Option<Route>, shell: Entity<Shell>) -> gpui::Stateful<gpui::Div> {
    let has = route.is_some();
    div()
        .id(id)
        .min_w_0()
        .truncate()
        .when(has, |d| d.cursor_pointer().hover(|s| s.text_color(text()).underline()))
        .child(label.into())
        .on_click(move |_: &ClickEvent, _, cx| {
            if let Some(r) = route.clone() {
                cx.stop_propagation();
                shell.update(cx, |s, cx| s.go(r, cx));
            }
        })
}

fn section(title: &'static str) -> gpui::Div {
    div().pt_6().pb_3().text_xl().font_weight(FontWeight::BOLD).child(title)
}

fn green() -> gpui::Rgba {
    gpui::rgb(0x5cc98a)
}

fn dz_album_card(a: &DzAlbum) -> Card {
    let kind = if a.kind.is_empty() || a.kind == "album" { String::new() } else { capitalize(&a.kind) };
    let sub: Vec<&str> = [a.year.as_str(), kind.as_str(), a.artist.as_str()].into_iter().filter(|x| !x.is_empty()).collect();
    Card {
        key: format!("dz-{}", a.id).into(),
        title: a.title.clone().into(),
        sub: sub.join(" · ").into(),
        cover: CoverRef::Url(a.cover.clone()),
        round: false,
        open: Route::DzAlbum(a.id),
        play: None,
        payload: Some(Payload::DzAlbum(a.id)),
    }
}

fn dz_artist_card(a: &DzArtist) -> Card {
    Card {
        key: format!("dza-{}", a.id).into(),
        title: a.name.clone().into(),
        sub: format!("Artist · {} fans", group_thousands(a.fans)).into(),
        cover: CoverRef::Url(a.picture.clone()),
        round: true,
        open: Route::DzArtist(a.id),
        play: None,
        payload: None,
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn group_thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

impl Shell {
    /// Rows for catalog songs. Songs you own show a ✓, play on double-click, and get queue options;
    /// the rest can be dragged onto a playlist or added with "+" (which downloads them).
    fn dz_rows(&self, list: Arc<Vec<DzTrack>>, numbered: bool, lay: Layout, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let m = self.model.read(cx);
        let covers = m.covers.clone();
        let cur_id = m.current().map(|t| t.id.clone());
        let owned: Vec<Option<Track>> = list.iter().map(|t| m.owned.get(&t.id).cloned().flatten()).collect();
        let checked: Vec<bool> = list.iter().map(|t| m.owned.contains_key(&t.id)).collect();
        let show_album = lay.content_w >= 720.;
        let previewing = m.previewing;
        let shell = cx.entity();
        (0..list.len())
            .map(|i| {
                let t = &list[i];
                let lib = owned[i].clone();
                let pv = previewing == Some(t.id);
                let is_cur = lib.as_ref().map(|l| Some(&l.id) == cur_id.as_ref()).unwrap_or(false);
                let (s_play, s_menu, s_plus, s_dbl) = (shell.clone(), shell.clone(), shell.clone(), shell.clone());
                let (l1, l2, l3) = (list.clone(), list.clone(), list.clone());
                let t_menu = t.clone();
                let t_plus = t.clone();
                let lib_menu = lib.clone();
                let has = lib.is_some();
                div()
                    .id(SharedString::from(format!("dz-{}-{i}", t.id)))
                    .group("dzrow")
                    .w_full()
                    .h(px(56.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_3p5()
                    .px_3()
                    .rounded_lg()
                    .hover(|s| s.bg(hover()))
                    .on_drag(DragItem::new(Payload::Dz(vec![t.clone()])), |d, _, _, cx| drag_ghost(d, cx))
                    .child(
                        div()
                            .relative()
                            .w(px(if numbered { 36. } else { 40. }))
                            .h_full()
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .group_hover("dzrow", |s| s.invisible())
                                    .when(pv, |d| d.invisible())
                                    .text_color(if is_cur { accent() } else { faint() })
                                    .child(if numbered {
                                        format!("{}", i + 1).into_any_element()
                                    } else {
                                        cover_el(covers.get_url(t.cover_small.as_deref().or(t.cover.as_deref())), 40., 4.)
                                    }),
                            )
                            // hover: play for songs you own, 30-second preview (headphones) for the rest
                            .child(
                                div()
                                    .id(("dz-play", i))
                                    .absolute()
                                    .inset_0()
                                    .rounded(px(4.))
                                    .bg(veil(0x88))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .when(!pv, |d| d.invisible().group_hover("dzrow", |s| s.visible()))
                                    .tooltip(tip(if has { "Play" } else if pv { "Stop preview" } else { "30-second preview. This song isn\u{2019}t in your library yet." }))
                                    .child(if has {
                                        icon(IconName::Play, 16., text())
                                    } else {
                                        icon(IconName::Headphones, 16., if pv { accent() } else { text() })
                                    })
                                    .on_click(move |_: &ClickEvent, _, cx| {
                                        cx.stop_propagation();
                                        let l = l1.clone();
                                        s_play.update(cx, |s, cx| {
                                            s.model.update(cx, |m, cx| {
                                                if has {
                                                    m.preview.stop();
                                                    m.play_owned(&l, i, cx)
                                                } else {
                                                    m.toggle_preview(&l[i], cx)
                                                }
                                            })
                                        });
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().truncate().font_weight(FontWeight::MEDIUM).text_color(if is_cur || pv { accent() } else { text() }).child(t.title.clone()))
                            .child(div().flex().text_sm().text_color(dim()).child(link(("dz-artist", i), t.artist.clone(), dz_artist_route(t), shell.clone()))),
                    )
                    .when(show_album, |d| {
                        d.child(div().flex_1().min_w_0().flex().text_sm().text_color(dim()).child(link(("dz-album", i), t.album.clone(), dz_album_route(t), shell.clone())))
                    })
                    .child(div().w(px(48.)).flex_none().text_sm().text_color(faint()).flex().justify_end().child(fmt_time(t.duration)))
                    .child(
                        div()
                            .w(px(22.))
                            .flex_none()
                            .flex()
                            .justify_center()
                            .when(has, |d| d.child(div().id(("dz-own", i)).tooltip(tip("In your library")).child(icon(IconName::CircleCheck, 16., green()))))
                            .when(!checked[i], |d| d.child(div().size(px(6.)).rounded_full().bg(line()))),
                    )
                    .child(plus_button(("dz-plus", i)).on_click(move |ev: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        let pos = ev.position();
                        let t = t_plus.clone();
                        s_plus.update(cx, |s, cx| {
                            let items = s.add_items(Payload::Dz(vec![t]), has, cx);
                            s.open_menu(pos, items, cx);
                        });
                    }))
                    .on_mouse_down(MouseButton::Right, move |ev: &MouseDownEvent, _, cx| {
                        let (t, lib, l) = (t_menu.clone(), lib_menu.clone(), l2.clone());
                        s_menu.update(cx, |s, cx| {
                            let mut items = vec![];
                            if let Some(lt) = lib {
                                items.push(item(IconName::Play, "Play", move |s, _, cx| {
                                    let l = l.clone();
                                    s.model.update(cx, |m, cx| m.play_owned(&l, i, cx));
                                }));
                                items.extend(Shell::queue_items(vec![lt]));
                                items.push(MenuItem::Sep);
                            } else if t.preview.as_deref().is_some_and(|u| !u.is_empty()) {
                                let tp = t.clone();
                                items.push(item(IconName::Headphones, if pv { "Stop preview" } else { "Preview 30 s" }, move |s, _, cx| {
                                    let tp = tp.clone();
                                    s.model.update(cx, |m, cx| m.toggle_preview(&tp, cx));
                                }));
                                items.push(MenuItem::Sep);
                            }
                            items.extend(s.add_items(Payload::Dz(vec![t]), has, cx));
                            s.open_menu(ev.position, items, cx);
                        });
                    })
                    .on_click(move |ev: &ClickEvent, _, cx| {
                        if ev.click_count() >= 2 {
                            let l = l3.clone();
                            s_dbl.update(cx, |s, cx| {
                                s.model.update(cx, |m, cx| {
                                    if m.owned.get(&l[i].id).cloned().flatten().is_some() {
                                        m.preview.stop();
                                        m.play_owned(&l, i, cx);
                                    } else {
                                        m.toggle_preview(&l[i], cx);
                                    }
                                })
                            });
                        }
                    })
                    .into_any_element()
            })
            .collect()
    }

    /// Compact rows for library songs found by a search.
    fn lib_rows(&self, list: Arc<Vec<Track>>, lay: Layout, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let m = self.model.read(cx);
        let covers = m.covers.clone();
        let cur_id = m.current().map(|t| t.id.clone());
        let show_album = lay.content_w >= 720.;
        let shell = cx.entity();
        (0..list.len())
            .map(|i| {
                let t = &list[i];
                let cur = cur_id.as_deref() == Some(t.id.as_str());
                let (s1, s2, s3) = (shell.clone(), shell.clone(), shell.clone());
                let (l1, l2) = (list.clone(), list.clone());
                let album_route = (!t.album_id.is_empty()).then(|| Route::Album(t.album_id.clone()));
                let play = move |l: &Arc<Vec<Track>>, s: &mut Shell, cx: &mut Context<Shell>| {
                    let tracks = l.to_vec();
                    s.model.update(cx, |m, cx| m.play_tracks(tracks, i, None, cx));
                };
                div()
                    .id(SharedString::from(format!("lib-{}", t.id)))
                    .group("librow")
                    .w_full()
                    .h(px(56.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_3p5()
                    .px_3()
                    .rounded_lg()
                    .hover(|s| s.bg(hover()))
                    .on_drag(DragItem::new(Payload::Lib(vec![t.clone()])), |d, _, _, cx| drag_ghost(d, cx))
                    .child(
                        div()
                            .relative()
                            .flex_none()
                            .child(cover_el(covers.get(t.cover_art.as_deref(), SMALL), 40., 4.))
                            .child(
                                div()
                                    .id(("lib-play", i))
                                    .absolute()
                                    .inset_0()
                                    .rounded(px(4.))
                                    .bg(veil(0x88))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .invisible()
                                    .group_hover("librow", |s| s.visible())
                                    .child(icon(IconName::Play, 16., text()))
                                    .on_click(move |_: &ClickEvent, _, cx| {
                                        cx.stop_propagation();
                                        let l = l1.clone();
                                        s1.update(cx, |s, cx| play(&l, s, cx));
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().truncate().font_weight(FontWeight::MEDIUM).text_color(if cur { accent() } else { text() }).child(t.title.clone()))
                            .child(div().truncate().text_sm().text_color(dim()).child(t.artist.clone())),
                    )
                    .when(show_album, |d| d.child(div().flex_1().min_w_0().flex().text_sm().text_color(dim()).child(link(("lib-album", i), t.album.clone(), album_route, shell.clone()))))
                    .child(div().w(px(48.)).flex_none().text_sm().text_color(faint()).flex().justify_end().child(fmt_time(t.duration)))
                    .child(div().w(px(22.)).flex_none().flex().justify_center().child(icon(IconName::CircleCheck, 16., green())))
                    .child(div().w(px(28.)).flex_none())
                    .on_mouse_down(MouseButton::Right, {
                        let t = t.clone();
                        move |ev: &MouseDownEvent, _, cx| {
                            let t = t.clone();
                            s3.update(cx, |s, cx| {
                                let mut items = Shell::queue_items(vec![t.clone()]);
                                items.push(MenuItem::Sep);
                                items.extend(s.add_items(Payload::Lib(vec![t]), true, cx));
                                s.open_menu(ev.position, items, cx);
                            });
                        }
                    })
                    .on_click(move |ev: &ClickEvent, _, cx| {
                        if ev.click_count() >= 2 {
                            let l = l2.clone();
                            s2.update(cx, |s, cx| play(&l, s, cx));
                        }
                    })
                    .into_any_element()
            })
            .collect()
    }

    pub(super) fn search_page(&mut self, lay: Layout, cx: &mut Context<Self>) -> AnyElement {
        let pad = if lay.content_w < 640. { 20. } else { 32. };
        let avail = lay.content_w - pad * 2.;
        let m = self.model.read(cx);
        let (q, kind, searching) = (m.search_q.clone(), m.search_kind, m.searching);
        let mine = m.lib_matches.clone();
        let results = match &m.results {
            Some(Results::Tracks(t)) => Some(Results::Tracks(t.clone())),
            Some(Results::Albums(a)) => Some(Results::Albums(a.clone())),
            Some(Results::Artists(a)) => Some(Results::Artists(a.clone())),
            Some(Results::Failed(e)) => Some(Results::Failed(e.clone())),
            None => None,
        };
        let kinds = [(SearchKind::Track, "Songs"), (SearchKind::Album, "Albums"), (SearchKind::Artist, "Artists")];
        let top = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap_4()
            .px(px(pad))
            .pt_7()
            .pb_4()
            .child(div().max_w(px(560.)).child(gpui_component::Sizable::large(Input::new(&self.search_input)).cleanable(true).prefix(icon(IconName::Search, 18., dim()))))
            .child(div().flex().gap_2().children(kinds.map(|(k, label)| {
                div()
                    .id(SharedString::from(format!("kind-{}", k.as_str())))
                    .px_3p5()
                    .py_1p5()
                    .rounded_full()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .cursor_pointer()
                    .when(kind == k, |d| d.bg(text()).text_color(on_text()))
                    .when(kind != k, |d| d.bg(panel2()).text_color(text()).hover(|s| s.bg(hover())))
                    .child(label)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.set_search_kind(k, cx))))
            })));

        let mut body: Vec<AnyElement> = vec![];
        if q.is_empty() && !m.search_history.is_empty() {
            let history = m.search_history.clone();
            body.push(
                div()
                    .pt_6()
                    .max_w(px(560.))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .mb_2()
                            .child(div().flex_1().text_xl().font_weight(FontWeight::BOLD).child("Recent searches"))
                            .child(
                                div()
                                    .id("hist-clear")
                                    .px_3()
                                    .py_1()
                                    .rounded_full()
                                    .text_sm()
                                    .text_color(dim())
                                    .cursor_pointer()
                                    .hover(|s| s.bg(hover()).text_color(text()))
                                    .child("Clear all")
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.clear_search_history(cx)))),
                            ),
                    )
                    .children(history.into_iter().enumerate().map(|(i, h)| {
                        let (h1, h2) = (h.clone(), h.clone());
                        div()
                            .id(("hist", i))
                            .group("hist")
                            .flex()
                            .items_center()
                            .gap_3()
                            .px_3()
                            .py_2()
                            .rounded_lg()
                            .cursor_pointer()
                            .hover(|s| s.bg(hover()))
                            .child(icon(IconName::Clock, 16., faint()))
                            .child(div().flex_1().min_w_0().truncate().child(h.clone()))
                            .child(
                                div()
                                    .id(("hist-x", i))
                                    .p_1()
                                    .rounded_full()
                                    .invisible()
                                    .group_hover("hist", |s| s.visible())
                                    .hover(|s| s.bg(panel2()))
                                    .tooltip(tip("Remove from history"))
                                    .child(icon(IconName::X, 14., dim()))
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        cx.stop_propagation();
                                        let h = h2.clone();
                                        this.model.update(cx, |m, cx| m.forget_search(&h, cx));
                                    })),
                            )
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                let h = h1.clone();
                                this.search_input.update(cx, |i, cx| i.set_value(h.clone(), window, cx));
                                this.search_task = None;
                                this.model.update(cx, |m, cx| {
                                    m.remember_search(&h, cx);
                                    m.run_search(h, cx)
                                });
                            }))
                    }))
                    .into_any_element(),
            );
        } else if q.is_empty() {
            body.push(
                div()
                    .pt_10()
                    .max_w(px(520.))
                    .text_color(dim())
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(div().text_lg().text_color(text()).font_weight(FontWeight::SEMIBOLD).child("Search any song, album or artist."))
                    .child("Songs you already have are marked with a green check and play straight away.")
                    .child("Anything else: drag it onto a playlist in the sidebar (or click +) and crate downloads it for you.")
                    .into_any_element(),
            );
        } else {
            // what you already have
            let mut have_mine = false;
            if let Some(mine) = mine {
                match kind {
                    SearchKind::Track if !mine.songs.is_empty() => {
                        have_mine = true;
                        body.push(section("In your library").into_any_element());
                        let songs = Arc::new(mine.songs.iter().take(5).cloned().collect::<Vec<_>>());
                        body.extend(self.lib_rows(songs, lay, cx));
                    }
                    SearchKind::Album if !mine.albums.is_empty() => {
                        have_mine = true;
                        body.push(section("In your library").into_any_element());
                        let cards: Vec<Card> = mine
                            .albums
                            .iter()
                            .take(10)
                            .map(|a| Card {
                                key: a.id.clone().into(),
                                title: a.name.clone().into(),
                                sub: a.artist.clone().into(),
                                cover: CoverRef::Nav(a.cover_art.clone()),
                                round: false,
                                open: Route::Album(a.id.clone()),
                                play: Some(format!("album:{}", a.id)),
                                payload: None,
                            })
                            .collect();
                        body.push(self.cards_wrap(&cards, avail, cx).into_any_element());
                    }
                    SearchKind::Artist if !mine.artists.is_empty() => {
                        have_mine = true;
                        body.push(section("In your library").into_any_element());
                        let cards: Vec<Card> = mine
                            .artists
                            .iter()
                            .take(10)
                            .map(|a| Card {
                                key: a.id.clone().into(),
                                title: a.name.clone().into(),
                                sub: format!("{} album{}", a.album_count, if a.album_count == 1 { "" } else { "s" }).into(),
                                cover: CoverRef::Nav(a.cover_art.clone()),
                                round: true,
                                open: Route::Artist(a.id.clone()),
                                play: None,
                                payload: None,
                            })
                            .collect();
                        body.push(self.cards_wrap(&cards, avail, cx).into_any_element());
                    }
                    _ => {}
                }
            }
            if have_mine {
                body.push(section("Find more").into_any_element());
            } else {
                body.push(div().h(px(8.)).into_any_element());
            }
            match results {
                None if searching => body.push(div().py_6().text_color(dim()).child("Searching…").into_any_element()),
                None => {}
                Some(Results::Failed(e)) => body.push(div().py_6().text_color(dim()).child(format!("Search failed: {e}")).into_any_element()),
                Some(Results::Tracks(t)) if t.is_empty() => body.push(div().py_6().text_color(dim()).child("No results.").into_any_element()),
                Some(Results::Tracks(t)) => body.extend(self.dz_rows(t, false, lay, cx)),
                Some(Results::Albums(a)) => {
                    let cards: Vec<Card> = a.iter().map(dz_album_card).collect();
                    body.push(self.cards_wrap(&cards, avail, cx).into_any_element())
                }
                Some(Results::Artists(a)) => {
                    let cards: Vec<Card> = a.iter().map(dz_artist_card).collect();
                    body.push(self.cards_wrap(&cards, avail, cx).into_any_element())
                }
            }
        }
        let content = div()
            .id("search-body")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.home_scroll)
            .px(px(pad))
            .pb_8()
            .flex()
            .flex_col()
            .children(body);
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(top)
            .child(scroll_area(content, &self.home_scroll, &self.home_smooth, cx.entity_id()))
            .into_any_element()
    }

    fn pill_button(id: &'static str, ic: IconName, label: &'static str) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .rounded_full()
            .border_1()
            .border_color(line())
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer()
            .hover(|s| s.bg(hover()).border_color(dim()))
            .child(icon(ic, 15., text()))
            .child(label)
    }

    pub(super) fn dz_album_page(&mut self, id: u64, lay: Layout, cx: &mut Context<Self>) -> AnyElement {
        let m = self.model.read(cx);
        let Some(page) = m.dz_albums.get(&id).cloned() else { return loading() };
        let (album, tracks) = (&page.0, Arc::new(page.1.clone()));
        let all_owned = !tracks.is_empty() && tracks.iter().all(|t| m.owned.get(&t.id).map(|o| o.is_some()).unwrap_or(false));
        let cover = m.covers.get_url(album.cover.as_deref());
        let total: f64 = tracks.iter().map(|t| t.duration).sum();
        let pad = if lay.content_w < 640. { 20. } else { 32. };
        let shell = cx.entity();
        let artist_route = tracks.first().and_then(dz_artist_route);
        let mut details = vec![];
        if !album.year.is_empty() {
            details.push(album.year.clone());
        }
        details.push(songs(tracks.len()));
        details.push(fmt_long(total));
        let sub = div()
            .flex()
            .items_center()
            .gap_1p5()
            .min_w_0()
            .text_color(dim())
            .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).text_color(text()).child(link("dz-hero-artist", album.artist.clone(), artist_route, shell.clone())))
            .child(div().truncate().child(format!("· {}", details.join(" · "))))
            .into_any_element();
        let kind: &'static str = match album.kind.as_str() {
            "single" => "SINGLE",
            "ep" => "EP",
            "compile" => "COMPILATION",
            _ => "ALBUM",
        };
        let (t_play, t_add, t_dl) = (tracks.clone(), tracks.clone(), tracks.clone());
        let actions = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_3()
            .px(px(pad))
            .pb_4()
            .child(
                div()
                    .id("dz-album-play")
                    .size(px(54.))
                    .rounded_full()
                    .bg(accent())
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.9))
                    .tooltip(tip("Play the songs you have"))
                    .child(icon(IconName::Play, 24., accent_ink()))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let t = t_play.clone();
                        this.model.update(cx, |m, cx| m.play_owned(&t, 0, cx));
                    })),
            )
            .child(Self::pill_button("dz-album-add", IconName::Plus, "Add to playlist").on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                let items = this.add_items(Payload::Dz(t_add.to_vec()), all_owned, cx);
                this.open_menu(ev.position(), items, cx);
            })))
            .when(!all_owned, |d| {
                d.child(Self::pill_button("dz-album-dl", IconName::Download, "Download album").on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let t = t_dl.to_vec();
                    this.model.update(cx, |m, cx| m.add_to(None, Payload::Dz(t), cx));
                })))
            });
        let rows = self.dz_rows(tracks, true, lay, cx);
        let content = div()
            .id("dz-album")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.home_scroll)
            .child(hero(cover, false, kind, album.title.clone(), sub, lay))
            .child(actions)
            .child(div().px(px(pad - 8.)).pb_8().flex().flex_col().children(rows));
        div().size_full().flex().flex_col().child(scroll_area(content, &self.home_scroll, &self.home_smooth, cx.entity_id())).into_any_element()
    }

    pub(super) fn dz_artist_page(&mut self, id: u64, lay: Layout, cx: &mut Context<Self>) -> AnyElement {
        let m = self.model.read(cx);
        let Some(page) = m.dz_artists.get(&id).cloned() else { return loading() };
        let (artist, top, albums) = (&page.0, Arc::new(page.1.clone()), &page.2);
        let cover = m.covers.get_url(artist.picture.as_deref());
        let pad = if lay.content_w < 640. { 20. } else { 32. };
        let avail = lay.content_w - pad * 2.;
        let sub = div().text_color(dim()).child(format!("{} fans", group_thousands(artist.fans))).into_any_element();
        let full: Vec<Card> = albums.iter().filter(|a| a.kind == "album").map(dz_album_card).collect();
        let singles: Vec<Card> = albums.iter().filter(|a| a.kind != "album").map(dz_album_card).collect();
        let rows = self.dz_rows(top.clone(), false, lay, cx);
        let mut inner = div().px(px(pad)).pb_8().flex().flex_col();
        if !rows.is_empty() {
            inner = inner.child(section("Popular")).child(div().mx(px(-8.)).flex().flex_col().children(rows));
        }
        if !full.is_empty() {
            inner = inner.child(section("Albums")).child(self.cards_wrap(&full, avail, cx));
        }
        if !singles.is_empty() {
            inner = inner.child(section("Singles & EPs")).child(self.cards_wrap(&singles, avail, cx));
        }
        let content = div()
            .id("dz-artist")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.home_scroll)
            .child(hero(cover, true, "ARTIST", artist.name.clone(), sub, lay))
            .child(inner);
        div().size_full().flex().flex_col().child(scroll_area(content, &self.home_scroll, &self.home_smooth, cx.entity_id())).into_any_element()
    }
}
