//! Library pages: Songs / Albums / Artists tabs, album pages and artist pages.
//! A child module of `shell`, so it shares the Shell's private state and helpers.
use super::*;
use crate::catalog::Payload;
use crate::covers::Covers;
use crate::model::ALBUM_SORTS;
use super::overlay::{drag_ghost, DragItem};

/// Where a card's picture comes from.
pub(super) enum CoverRef {
    /// Navidrome cover id
    Nav(Option<String>),
    /// image URL (online catalog)
    Url(Option<String>),
}

/// One tile in a cover grid.
pub(super) struct Card {
    pub key: SharedString,
    pub title: SharedString,
    pub sub: SharedString,
    pub cover: CoverRef,
    pub round: bool,
    pub open: Route,
    /// Song-list key to play from the hover button (library albums).
    pub play: Option<String>,
    /// What dragging the card / its right-click menu adds to playlists.
    pub payload: Option<Payload>,
}

/// Card width that fills `avail` evenly with cards of at least 184px.
pub(super) fn card_metrics(avail: f32) -> (usize, f32) {
    let gap = 20.;
    let per_row = ((avail + gap) / (184. + gap)).floor().max(1.) as usize;
    let card_w = ((avail - gap * (per_row as f32 - 1.)) / per_row as f32).max(120.);
    (per_row, card_w)
}

pub(super) fn card_el(c: &Card, card_w: f32, covers: &Covers, playing_src: &Option<String>, shell: &Entity<Shell>) -> gpui::Stateful<gpui::Div> {
    let cover_w = card_w - 28.;
    let (open, play) = (c.open.clone(), c.play.clone());
    let is_playing = play.is_some() && *playing_src == play;
    let (s1, s2, s3) = (shell.clone(), shell.clone(), shell.clone());
    let path = match &c.cover {
        CoverRef::Nav(id) => covers.get(id.as_deref(), LARGE),
        CoverRef::Url(u) => covers.get_url(u.as_deref()),
    };
    let payload = c.payload.clone();
    div()
        .id(gpui::ElementId::Name(format!("card-{}", c.key).into()))
        .group("gcard")
        .w(px(card_w))
        .flex_none()
        .p_3p5()
        .rounded_xl()
        .bg(panel())
        .hover(|s| s.bg(panel2()))
        .cursor_pointer()
        .flex()
        .flex_col()
        .gap_3()
        .when_some(c.payload.clone(), |d, p| d.on_drag(DragItem::new(p), |d, _, _, cx| drag_ghost(d, cx)))
        .when_some(payload, |d, p| {
            d.on_mouse_down(MouseButton::Right, move |ev: &MouseDownEvent, _, cx| {
                let p = p.clone();
                s3.update(cx, |s, cx| {
                    let items = s.add_items(p, false, cx);
                    s.open_menu(ev.position, items, cx);
                });
            })
        })
        .child(
            div()
                .relative()
                .child(cover_el(path, cover_w, if c.round { cover_w / 2. } else { 8. }))
                .when_some(play, |d, key| {
                    d.child(
                        div()
                            .id(gpui::ElementId::Name(format!("gplay-{}", c.key).into()))
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
                            .when(!is_playing, |d| d.invisible().group_hover("gcard", |s| s.visible()))
                            .hover(|s| s.opacity(0.9))
                            .child(icon(if is_playing { IconName::Pause } else { IconName::Play }, 20., accent_ink()))
                            .on_click(move |_: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                let key = key.clone();
                                s2.update(cx, |s, cx| {
                                    s.model.update(cx, |m, cx| {
                                        if m.queue_source.as_deref() == Some(key.as_str()) && m.player_state.index.is_some() {
                                            m.send(Action::Toggle);
                                        } else {
                                            m.play_playlist(key, false, cx);
                                        }
                                    })
                                });
                            }),
                    )
                }),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .when(c.round, |d| d.items_center())
                .child(div().max_w_full().truncate().font_weight(FontWeight::SEMIBOLD).text_color(if is_playing { accent() } else { text() }).child(c.title.clone()))
                .child(div().max_w_full().truncate().text_sm().text_color(dim()).child(c.sub.clone())),
        )
        .on_click(move |_: &ClickEvent, _, cx| {
            let r = open.clone();
            s1.update(cx, |s, cx| s.go(r, cx));
        })
}

fn chip(id: impl Into<SharedString>, label: &'static str, on: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(gpui::ElementId::Name(id.into()))
        .px_3p5()
        .py_1p5()
        .rounded_full()
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .when(on, |d| d.bg(text()).text_color(on_text()))
        .when(!on, |d| d.bg(panel2()).text_color(text()).hover(|s| s.bg(hover())))
        .child(label)
}

impl Shell {
    /// "Your library" title with the Songs/Albums/Artists switch (and album sort options).
    fn library_header(&self, lay: Layout, cx: &mut Context<Self>) -> AnyElement {
        let (tab, album_sort) = {
            let m = self.model.read(cx);
            (m.lib_tab, m.album_sort.clone())
        };
        let pad = if lay.content_w < 640. { 20. } else { 32. };
        let tabs = [(LibTab::Songs, "Songs"), (LibTab::Albums, "Albums"), (LibTab::Artists, "Artists")];
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap_4()
            .px(px(pad))
            .pt_7()
            .pb_5()
            .child(div().text_3xl().font_weight(FontWeight::BOLD).child("Your library"))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .children(tabs.map(|(t, label)| {
                        chip(format!("tab-{}", t.as_str()), label, tab == t)
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.model.update(cx, |m, cx| m.set_lib_tab(t, cx))))
                    }))
                    .when(tab == LibTab::Albums, |d| {
                        // one compact "Sort" menu instead of five chips that wrap onto a second line
                        let cur = ALBUM_SORTS.iter().find(|s| s.0 == album_sort).map(|s| s.1).unwrap_or("A–Z");
                        d.child(div().flex_1()).child(
                            div()
                                .id("album-sort")
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .px_3()
                                .py_1p5()
                                .rounded_full()
                                .text_sm()
                                .cursor_pointer()
                                .text_color(dim())
                                .hover(|s| s.bg(hover()).text_color(text()))
                                .child(format!("Sort: {cur}"))
                                .child(icon(IconName::ArrowDown, 12., dim()))
                                .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                                    let cur = this.model.read(cx).album_sort.clone();
                                    let items = ALBUM_SORTS
                                        .iter()
                                        .map(|&(kind, label)| super::overlay::choice(label, kind == cur, move |s, _, cx| s.model.update(cx, |m, cx| m.set_album_sort(kind, cx))))
                                        .collect();
                                    this.open_menu(ev.position(), items, cx);
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    pub(super) fn library_page(&mut self, lay: Layout, cx: &mut Context<Self>) -> AnyElement {
        let header = self.library_header(lay, cx);
        let m = self.model.read(cx);
        match m.lib_tab {
            LibTab::Songs => match m.cache.get("__library").cloned() {
                Some(pl) => self.song_page(pl, Some((header, LIB_HEADER_H, false)), lay, cx),
                None => div().size_full().flex().flex_col().child(header).child(loading()).into_any_element(),
            },
            LibTab::Albums => {
                let Some(albums) = m.albums.get(&m.album_sort).cloned() else {
                    return div().size_full().flex().flex_col().child(header).child(loading()).into_any_element();
                };
                let cards = albums.iter().map(album_card).collect();
                self.grid_page(Some(header), Rc::new(cards), true, lay, cx)
            }
            LibTab::Artists => {
                let Some(artists) = m.artists.clone() else {
                    return div().size_full().flex().flex_col().child(header).child(loading()).into_any_element();
                };
                let cards = artists
                    .iter()
                    .map(|a| Card {
                        key: a.id.clone().into(),
                        title: a.name.clone().into(),
                        sub: match a.album_count {
                            0 => "Artist".to_string(),
                            1 => "1 album".to_string(),
                            n => format!("{n} albums"),
                        }
                        .into(),
                        cover: CoverRef::Nav(a.cover_art.clone()),
                        round: true,
                        open: Route::Artist(a.id.clone()),
                        play: None,
                        payload: None,
                    })
                    .collect();
                self.grid_page(Some(header), Rc::new(cards), true, lay, cx)
            }
        }
    }

    pub(super) fn album_page(&mut self, id: &str, lay: Layout, cx: &mut Context<Self>) -> AnyElement {
        let m = self.model.read(cx);
        let Some(pl) = m.cache.get(&format!("album:{id}")).cloned() else { return loading() };
        let meta = m.album_meta.get(id).cloned().unwrap_or_default();
        let cover = m.covers.get(pl.cover_art.as_deref(), LARGE);
        let total: f64 = pl.tracks.iter().map(|t| t.duration).sum();
        let mut details = vec![songs(pl.tracks.len()), fmt_long(total)];
        if let Some(y) = meta.year {
            details.insert(0, y.to_string());
        }
        let artist_id = meta.artist_id.clone();
        let sub = div()
            .flex()
            .items_center()
            .gap_1p5()
            .text_color(dim())
            .min_w_0()
            .child(
                div()
                    .id("album-artist")
                    .flex_none()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(text())
                    .when(artist_id.is_some(), |d| d.cursor_pointer().hover(|s| s.underline()))
                    .child(meta.artist.clone())
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if let Some(a) = artist_id.clone() {
                            this.go(Route::Artist(a), cx);
                        }
                    })),
            )
            .child(div().truncate().child(format!("· {}", details.join(" · "))))
            .into_any_element();
        let hero = hero(cover, false, "ALBUM", pl.name.clone(), sub, lay);
        self.song_page(pl, Some((hero, super::hero_height(lay), true)), lay, cx)
    }

    pub(super) fn artist_page(&mut self, id: &str, lay: Layout, cx: &mut Context<Self>) -> AnyElement {
        let m = self.model.read(cx);
        let Some(page) = m.artist_pages.get(id).cloned() else { return loading() };
        let (artist, albums) = (&page.0, &page.1);
        let cover = m.covers.get(artist.cover_art.as_deref(), LARGE);
        let sub = div().text_color(dim()).child(format!("{} album{} in your library", albums.len(), if albums.len() == 1 { "" } else { "s" })).into_any_element();
        let pad = if lay.content_w < 640. { 20. } else { 32. };
        let artist_id = artist.id.clone();
        let top = div()
            .flex_none()
            .flex()
            .flex_col()
            .child(hero(cover, true, "ARTIST", artist.name.clone(), sub, lay))
            .child(
                div().flex().items_center().gap_4().px(px(pad)).pb_2().child(
                    div()
                        .id("artist-play")
                        .size(px(54.))
                        .rounded_full()
                        .bg(accent())
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|s| s.opacity(0.9))
                        .tooltip(tip("Play all"))
                        .child(icon(IconName::Play, 24., accent_ink()))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            let id = artist_id.clone();
                            this.model.update(cx, |m, cx| m.play_artist(id, cx));
                        })),
                ),
            )
            .child(div().px(px(pad)).pt_4().pb_3().text_xl().font_weight(FontWeight::BOLD).child("Albums"))
            .into_any_element();
        let cards = albums.iter().map(album_card).collect();
        self.grid_page(Some(top), Rc::new(cards), false, lay, cx)
    }

    /// A plain (non-virtualised) block of cards, for short grids inside a scrolling page.
    pub(super) fn cards_wrap(&self, cards: &[Card], avail: f32, cx: &mut Context<Self>) -> gpui::Div {
        let (per_row, card_w) = card_metrics(avail);
        let (covers, playing_src) = {
            let m = self.model.read(cx);
            (m.covers.clone(), if m.player_state.playing { m.queue_source.clone() } else { None })
        };
        let shell = cx.entity();
        div().flex().flex_col().gap_5().children(cards.chunks(per_row).map(|row| {
            div().flex().gap_5().children(row.iter().map(|c| card_el(c, card_w, &covers, &playing_src, &shell)))
        }))
    }

    /// A header and a grid of cover cards.
    /// `pinned_header`: the header stays put and the grid is virtualised (rows rendered on demand,
    /// so thousands of albums scroll smoothly). Otherwise header and grid scroll together — for
    /// short grids under a tall header, like an artist's albums.
    fn grid_page(&mut self, top: Option<AnyElement>, cards: Rc<Vec<Card>>, pinned_header: bool, lay: Layout, cx: &mut Context<Self>) -> AnyElement {
        let pad = if lay.content_w < 640. { 20. } else { 32. };
        let avail = lay.content_w - pad * 2.;
        if !pinned_header {
            let grid = self.cards_wrap(&cards, avail, cx);
            let content = div()
                .id("grid-page")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&self.home_scroll)
                .children(top)
                .child(div().px(px(pad)).pb_6().child(grid));
            return div().size_full().flex().flex_col().child(scroll_area(content, &self.home_scroll, &self.home_smooth, cx.entity_id())).into_any_element();
        }
        let (per_row, card_w) = card_metrics(avail);
        let gap = 20.;
        let row_h = card_w - 28. + 28. + 12. + 44. + gap;
        let rows = cards.len().div_ceil(per_row);
        let (covers, playing_src) = {
            let m = self.model.read(cx);
            (m.covers.clone(), if m.player_state.playing { m.queue_source.clone() } else { None })
        };
        let shell = cx.entity();
        let list = uniform_list("grid", rows, move |range, _, _| {
            range
                .map(|r| {
                    let row = (r * per_row..((r + 1) * per_row).min(cards.len())).map(|i| card_el(&cards[i], card_w, &covers, &playing_src, &shell));
                    // padding inside the rows, so the scrollbar sits in the page margin, not on the cards
                    div().h(px(row_h)).w_full().px(px(pad)).flex().gap(px(gap)).children(row)
                })
                .collect()
        })
        .flex_1()
        .track_scroll(&self.grid_scroll);
        let body = if rows == 0 {
            div().flex_1().flex().justify_center().pt_10().text_color(dim()).child("Nothing here yet.").into_any_element()
        } else {
            div().flex_1().min_h_0().flex().flex_col().child(scroll_area(list, &self.grid_scroll, &self.grid_smooth, cx.entity_id())).into_any_element()
        };
        div().size_full().flex().flex_col().children(top).child(body).into_any_element()
    }
}

/// Height of the "Your library" header (title + tabs), so the songs tab can scroll it away.
const LIB_HEADER_H: f32 = 140.;

pub(super) fn album_card(a: &crate_core::model::AlbumSummary) -> Card {
    let sub = match a.year {
        Some(y) if y > 0 => format!("{} · {}", y, a.artist),
        _ => a.artist.clone(),
    };
    Card {
        key: a.id.clone().into(),
        title: a.name.clone().into(),
        sub: sub.into(),
        cover: CoverRef::Nav(a.cover_art.clone()),
        round: false,
        open: Route::Album(a.id.clone()),
        play: Some(format!("album:{}", a.id)),
        payload: None,
    }
}
