//! Window layout: panels, overlays, keyboard shortcuts.

use std::sync::Arc;

pub mod artist;
pub mod collection;
pub mod corners;
pub(crate) mod devices;
mod dialogs;
pub(crate) mod friends;
pub mod home;
mod keys;
pub mod library;
pub mod login;
mod lyrics;
pub mod new_releases;
pub mod now_playing;
pub mod player_bar;
mod prerelease;
pub mod queue;
pub mod radio;
pub mod search;
pub mod settings;
pub mod show;
pub mod sidebar;
mod soloist;
pub mod topbar;
mod update;
pub mod whats_new;
pub mod widgets;

use egui::{Align2, Color32, Context, CornerRadius, Frame, Id, Margin, Rect, Stroke, vec2};

use crate::api::models::pick_image;
use crate::app::App;
use crate::backend::AuthStatus;
use crate::model::{Action, Loadable, Page, ToastKind};
use crate::theme::{self, Icon};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let ctx = &ctx;
    keys::handle(app, ctx);
    let signed_in = app.is_connected() && app.user.is_some();
    let connecting = matches!(app.auth, AuthStatus::Connecting | AuthStatus::Starting)
        || (app.is_connected() && app.user.is_none());
    if !signed_in {
        player_bar::end_tint_session(ctx);
        login::show(app, ui, connecting);
        update::show(app, ctx);
        toasts(app, ctx, 20.0);
        caption_buttons(ui, &app.palette, app.locale);
        fastframe_titlebar::resize(ui);
        corners::paint(ctx, &app.palette);
        return;
    }
    let soloist_view = app.soloist_view();
    if soloist_view != crate::app::SoloistView::App {
        player_bar::end_tint_session(ctx);
        if soloist_view == crate::app::SoloistView::Loading {
            login::show(app, ui, true);
        } else {
            soloist::onboarding(app, ui);
        }
        toasts(app, ctx, 20.0);
        caption_buttons(ui, &app.palette, app.locale);
        fastframe_titlebar::resize(ui);
        corners::paint(ctx, &app.palette);
        return;
    }
    if titlebar_spans_window(cfg!(target_os = "macos"), fastframe_titlebar::enabled()) {
        fastframe_titlebar::drag(
            ui,
            Rect::from_min_size(
                ui.max_rect().min,
                vec2(
                    ui.max_rect().width(),
                    topbar::height(ui.ctx(), ui.max_rect().width()),
                ),
            ),
        );
    }
    // Spotify's shell: the window's own colour shows in the gaps between
    // the rounded panels, around them, and under the top and player bars.
    ui.painter()
        .rect_filled(ui.max_rect(), CornerRadius::ZERO, app.palette.window);
    player_bar::show(app, ui);
    if app.lyrics_fullscreen.is_some() {
        lyrics::fullscreen(app, ui);
    } else {
        top_bar(app, ui);
        if app.settings.sidebar_visible {
            sidebar::show(app, ui);
        }
        if app.show_queue_panel {
            queue::side_panel(app, ui);
        }
        if app.show_lyrics_panel {
            lyrics::side_panel(app, ui);
        }
        if app.show_friends_panel {
            friends::side_panel(app, ui);
        }
        if app.now_playing_panel_shown() {
            now_playing::side_panel(app, ui);
        }
        central(app, ui);
    }
    keep_room_for_panels(app, ctx);
    devices::popup(app, ctx);
    dialogs::show(app, ctx);
    update::show(app, ctx);
    widgets::drag_ghost(ctx, &app.palette, app.locale);
    toasts(app, ctx, theme::PLAYER_BAR_HEIGHT + 16.0);
    caption_buttons(ui, &app.palette, app.locale);
    fastframe_titlebar::resize(ui);
    corners::paint(ctx, &app.palette);
}

/// The main window's narrowest width with these panels open: their least
/// widths and the page's.
fn main_min_width(page: f32, sidebar: bool, right_panel: bool) -> f32 {
    let sidebar = if sidebar { SIDEBAR_MIN_WIDTH } else { 0.0 };
    let right = if right_panel {
        theme::SIDE_PANEL_MIN_WIDTH
    } else {
        0.0
    };
    (sidebar + right + page).max(crate::window::MAIN_MIN_SIZE[0])
}

/// The sidebar's narrowest width.
pub(crate) const SIDEBAR_MIN_WIDTH: f32 = 210.0;

/// Raise the window's minimum width while the Queue or Lyrics panel is open,
/// so even at its narrowest the page beside the panels keeps the room its
/// top bar needs (#624), and lower it again once they close. Window managers
/// that tile ignore the minimum; the panels still give way there.
///
/// Full-screen lyrics draw no sidebar or side panel (an open queue becomes a
/// column inside them), so they ask for no room for them. A screen narrower
/// than the panels' minimum, such as a portrait monitor, then holds the
/// full-screen window without a minimum wider than itself (#783).
fn keep_room_for_panels(app: &App, ctx: &Context) {
    if crate::window::fixed_size() {
        return;
    }
    let panels = app.lyrics_fullscreen.is_none();
    let width = main_min_width(
        topbar::least_width(ctx),
        panels && app.settings.sidebar_visible,
        panels && app.right_panel_open(),
    )
    .round();
    let id = Id::new("main-min-width");
    let sent = ctx.data(|data| data.get_temp::<f32>(id));
    // The window starts at MAIN_MIN_SIZE, so there is nothing to send until
    // the panels need more.
    if sent.unwrap_or(crate::window::MAIN_MIN_SIZE[0]) != width {
        ctx.data_mut(|data| data.insert_temp(id, width));
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(vec2(
            width,
            crate::window::MAIN_MIN_SIZE[1],
        )));
    }
}

/// How a resizable side panel fits beside the page.
pub(crate) struct PanelFit {
    /// The widths the panel may take this frame.
    pub range: std::ops::RangeInclusive<f32>,
    /// Whether the room the page needs holds the panel under the width the
    /// person chose for it.
    pub yielding: bool,
}

/// Fit a side panel between `range` and the page beside it. The panel gives
/// up width, down to its minimum, so the page keeps `room_for_page`, the
/// least its top bar needs (#624). The width chosen for the panel stays in
/// the settings and comes back once the window widens again.
///
/// `room` is what the panel and everything after it share.
pub(crate) fn yielding_panel(
    ctx: &Context,
    id: &str,
    range: std::ops::RangeInclusive<f32>,
    chosen: f32,
    room: f32,
) -> PanelFit {
    let (min, max) = range.into_inner();
    let max = max.min(room).max(min);
    let width = chosen.clamp(min, max);
    // egui remembers the width the panel last drew at. While nobody drags
    // its handle, hand it back the chosen width, either way: a resize made
    // elsewhere (the fullscreen queue column shares the queue width) shows
    // here too, and the panel never writes a stale width back over it.
    let id = Id::new(id);
    if !ctx.is_being_dragged(id.with("__resize"))
        && let Some(mut state) = egui::containers::panel::PanelState::load(ctx, id)
        && (state.outer_rect.width() - width).abs() > 0.5
    {
        state.outer_rect.set_width(width);
        ctx.data_mut(|data| data.insert_persisted(id, state));
    }
    PanelFit {
        range: min..=max,
        yielding: max < chosen - 0.5,
    }
}

/// Whether the person is dragging this panel's resize handle right now.
pub(crate) fn panel_dragged(ctx: &Context, id: &str) -> bool {
    ctx.is_being_dragged(Id::new(id).with("__resize"))
}

/// Whether a panel's new width is one the person chose: it moved while not
/// held back by the page, or by their own drag.
pub(crate) fn panel_width_chosen(ctx: &Context, id: &str, fit: &PanelFit) -> bool {
    !fit.yielding || panel_dragged(ctx, id)
}

/// Keeps the most recent loading preview of each metadata type available to
/// the loaded hero as an artwork fallback. The fixed typed slot bounds this to
/// one playlist, album, artist, and show instead of scanning known metadata on
/// every loaded frame.
fn loading_preview<T>(
    ctx: &Context,
    key: &str,
    details: &Loadable<T>,
    known: impl FnOnce() -> Option<T>,
) -> Option<Arc<T>>
where
    T: Send + Sync + 'static,
{
    let memory_id = Id::new("collection-loading-preview");
    let key = Id::new(key);
    if details.get().is_some() {
        ctx.data(|data| data.get_temp::<(Id, Arc<T>)>(memory_id))
            .filter(|(stored_key, _)| *stored_key == key)
            .map(|(_, preview)| preview)
    } else {
        let preview = known().map(Arc::new);
        ctx.data_mut(|data| match &preview {
            Some(preview) => {
                data.insert_temp(memory_id, (key, Arc::clone(preview)));
            }
            None => data.remove::<(Id, Arc<T>)>(memory_id),
        });
        preview
    }
}

fn page_tint(app: &mut App) -> Option<Color32> {
    let page = app.page().clone();
    let image = match &page {
        Page::Playlist(id) => app
            .playlist_pages
            .get(id)
            .and_then(|page| page.playlist.get())
            .or_else(|| app.known_playlist(id))
            .and_then(|playlist| pick_image(&playlist.images, 64))
            .map(str::to_string),
        Page::Album(id) => app
            .album_pages
            .get(id)
            .and_then(|page| page.album.get())
            .or_else(|| app.known_album(id))
            .and_then(|album| pick_image(&album.images, 64))
            .map(str::to_string),
        Page::Artist(id) => app
            .artist_pages
            .get(id)
            .and_then(|page| page.artist.get())
            .or_else(|| app.known_artist(id))
            .and_then(|artist| pick_image(&artist.images, 64))
            .map(str::to_string),
        Page::Show(id) => app
            .show_pages
            .get(id)
            .and_then(|page| page.show.get())
            .or_else(|| app.known_show(id))
            .and_then(|show| pick_image(&show.images, 64))
            .map(str::to_string),
        Page::Radio(seed) => pick_image(&app.radio_images(seed), 64).map(str::to_string),
        Page::LikedSongs => {
            return app
                .settings
                .accent_from_art
                .then_some(Color32::from_rgb(0x50, 0x38, 0xc8));
        }
        _ => None,
    };
    if !app.settings.accent_from_art && image.is_some() {
        return None;
    }
    match image {
        Some(url) => app.tint_for(Some(&url)).or_else(|| app.now_playing_tint()),
        None => app.now_playing_tint(),
    }
}

fn central(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let tint = page_tint(app);
    // The main view is the middle of the three cards. A panel beside it
    // keeps the gap between them; without one, the card keeps the window's
    // margin itself.
    let margin = Margin {
        left: if app.settings.sidebar_visible {
            0
        } else {
            PANEL_GAP as i8
        },
        right: if app.right_panel_open() {
            0
        } else {
            PANEL_GAP as i8
        },
        top: 0,
        bottom: PANEL_GAP as i8,
    };
    egui::CentralPanel::default()
        .frame(card_frame(&palette, margin, Margin::ZERO))
        .show(ui, |ui| {
            let rect = ui.max_rect();
            let page = app.page().clone();
            let overview = matches!(
                page,
                Page::Home | Page::Search | Page::Settings | Page::Queue
            );
            // The wash belongs to the page's hero, so it scrolls away with
            // it. The page's offset is known once it has been laid out, so
            // the previous frame's serves.
            let scrolled_id = Id::new(("page-scrolled", page.encode()));
            let scrolled = ui
                .ctx()
                .data(|data| data.get_temp::<f32>(scrolled_id))
                .unwrap_or(0.0);
            let countdown = matches!(&page, Page::Album(id)
                if app.album_pages.get(id).is_some_and(|page| page.prerelease.is_some()));
            // Countdown pages own their hero gradient. The generic artwork wash
            // otherwise leaves an unrelated color band behind the search bar.
            if !countdown && let Some((top, bottom)) = header_wash(&palette, overview, tint) {
                widgets::paint_vertical_gradient(ui, header_rect(rect, scrolled), top, bottom);
            }
            ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
            // egui fades a scrolled page's edge into the panel's plain
            // colour, which shows as a pale band over a cover's tint; the
            // page casts a shadow under the header instead.
            ui.spacing_mut().scroll.fade.strength = 0.0;
            // One banner at a time: a song that will not play matters more
            // than a slow app, which waits its turn.
            if app.dialog.is_none() && page != Page::Settings {
                if app.soloist_waiting() {
                    soloist::banner(app, ui);
                } else if app.audio_key_banner {
                    audio_key_banner(app, ui);
                } else if app.slow_banner {
                    slow_banner(app, ui);
                }
            }
            let page_key = page.encode();
            let mut area = egui::ScrollArea::vertical()
                .id_salt(("page", &page_key))
                .auto_shrink([false, false]);
            if let Some((_, offset)) = app.restore_scroll.take().filter(|(at, _)| *at == page_key) {
                area = area.vertical_scroll_offset(offset);
            }
            let scroll = crate::autoscroll::show(ui, area, egui::Vec2b::new(false, true), |ui| {
                Frame::new()
                    .inner_margin(Margin {
                        left: widgets::PAGE_PADDING as i8,
                        right: widgets::PAGE_PADDING as i8,
                        top: widgets::PAGE_TOP as i8,
                        bottom: 48,
                    })
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        match page {
                            Page::Home => home::show(app, ui),
                            Page::TopSongs => collection::top_songs(app, ui),
                            Page::NewReleases => new_releases::page(app, ui),
                            Page::WhatsNew => whats_new::page(app, ui),
                            Page::Search => search::show(app, ui),
                            Page::LikedSongs => collection::liked(app, ui),
                            Page::Albums | Page::Artists | Page::Podcasts | Page::Episodes => {
                                library::show(app, ui, page)
                            }
                            Page::Playlist(id) => collection::playlist(app, ui, &id),
                            Page::Album(id) => collection::album(app, ui, &id),
                            Page::Artist(id) => artist::show(app, ui, &id),
                            Page::Show(id) => show::show(app, ui, &id),
                            Page::Radio(seed) => radio::radio(app, ui, &seed),
                            Page::Category(id) => search::category(app, ui, &id),
                            Page::Queue => queue::page(app, ui),
                            Page::Settings => settings::show(app, ui),
                        }
                    });
                if app.dialog.is_none() && !app.show_devices {
                    keys::scroll_page(ui);
                }
            });
            app.page_scroll = Some((page_key, scroll.state.offset.y));
            ui.ctx()
                .data_mut(|data| data.insert_temp(scrolled_id, scroll.state.offset.y));
            header_shadow(ui, scroll.inner_rect, scroll.state.offset.y, palette.dark);
            // The wash, a full-bleed banner and the shadow reach the card's
            // edges; its corners stay round over them.
            round_card_corners(ui, rect, palette.window);
        });
}

/// The gap between the shell's rounded panels, and the margin they keep
/// from the window's edges, as Spotify's `--panel-gap`.
pub(crate) const PANEL_GAP: f32 = 8.0;
/// The corner radius of the shell's panels.
pub(crate) const PANEL_RADIUS: u8 = 8;

/// The frame of one of the shell's panels: a card in the panel colour with
/// rounded corners, `outer` away from its neighbours and the window's
/// edges, holding its contents `inner` in from its own edges.
pub(crate) fn card_frame(palette: &theme::Palette, outer: Margin, inner: Margin) -> Frame {
    Frame::new()
        .outer_margin(outer)
        .fill(palette.panel)
        .corner_radius(CornerRadius::same(PANEL_RADIUS))
        .inner_margin(inner)
}

/// The outer margin of a panel on the right: the gap to the main view on
/// its left, the window's margin on its right and under it.
pub(crate) const RIGHT_PANEL_MARGIN: Margin = Margin {
    left: PANEL_GAP as i8,
    right: PANEL_GAP as i8,
    top: 0,
    bottom: PANEL_GAP as i8,
};

/// The padding inside a panel on the right: its scroll bar sits 4 points
/// in from the card's edge, as the sidebar's does, and its rows keep
/// `RIGHT_PANEL_SCROLL` clear of it.
pub(crate) const RIGHT_PANEL_INNER: Margin = Margin {
    left: 12,
    right: 4,
    top: 12,
    bottom: 0,
};
/// The size of a right panel's title: Now Playing's context, Lyrics,
/// Listening activity.
pub(crate) const RIGHT_PANEL_TITLE: f32 = 16.0;
/// The margin inside a right panel's scrolling list: clear of its scroll
/// bar, and room after the last row.
pub(crate) const RIGHT_PANEL_SCROLL: Margin = Margin {
    left: 0,
    right: 8,
    top: 0,
    bottom: 16,
};

/// Paints the window's colour over the four corners of the card `rect`,
/// outside its rounded outline, so whatever the card's contents drew up to
/// its edges is clipped to the round corners. egui clips to rectangles
/// only.
pub(crate) fn round_card_corners(ui: &egui::Ui, rect: Rect, window: Color32) {
    let painter = ui.painter().with_clip_rect(rect);
    for shape in corner_masks(rect, f32::from(PANEL_RADIUS), window) {
        painter.add(shape);
    }
}

/// The shapes `round_card_corners` paints: per corner, a fan from the
/// rectangle's corner along the arc, then the arc's own line, whose
/// feathered edge smooths the fan's.
fn corner_masks(rect: Rect, radius: f32, color: Color32) -> Vec<egui::Shape> {
    const STEPS: usize = 8;
    let mut shapes = Vec::with_capacity(8);
    // Each corner: the corner point, the arc's centre, and the arc's start
    // angle; the arc sweeps a quarter turn.
    let corners = [
        (rect.left_top(), vec2(radius, radius), std::f32::consts::PI),
        (
            rect.right_top(),
            vec2(-radius, radius),
            1.5 * std::f32::consts::PI,
        ),
        (rect.right_bottom(), vec2(-radius, -radius), 0.0),
        (
            rect.left_bottom(),
            vec2(radius, -radius),
            0.5 * std::f32::consts::PI,
        ),
    ];
    for (corner, offset, start) in corners {
        let centre = corner + offset;
        let arc: Vec<egui::Pos2> = (0..=STEPS)
            .map(|step| {
                let angle = start + step as f32 / STEPS as f32 * std::f32::consts::FRAC_PI_2;
                centre + radius * vec2(angle.cos(), angle.sin())
            })
            .collect();
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(corner, color);
        for point in &arc {
            mesh.colored_vertex(*point, color);
        }
        for index in 1..=STEPS as u32 {
            mesh.add_triangle(0, index, index + 1);
        }
        shapes.push(egui::Shape::mesh(mesh));
        shapes.push(egui::Shape::line(arc, Stroke::new(1.0, color)));
    }
    shapes
}

/// Where the "Why?" link of the slow-app banner leads.
const SLOW_BANNER_GUIDE: &str = "https://spotifast.rocks/troubleshooting/#rate-limits";
/// Where the "Why?" link of the audio-key banner leads.
pub const AUDIO_KEY_GUIDE: &str = "https://spotifast.rocks/troubleshooting/#audio-key";

/// A strip at the top of the main view suggesting a personal app, after Spotify
/// told the shared app, which only listeners signed in before 0.14.2 still
/// use, that it sent too many requests. It stays put while the
/// page scrolls, and goes when it is closed.
fn slow_banner(app: &mut App, ui: &mut egui::Ui) {
    use crate::i18n::gettext;
    let locale = app.locale;
    banner(
        app,
        ui,
        &gettext(locale, "Spotify is being slow on the shared sign-in."),
        &gettext(
            locale,
            "Spotifast no longer offers the shared sign-in, and Spotify can stop accepting it at any time. Register Spotifast and you won't have to wait.",
        ),
        false,
        vec![
            (
                gettext(locale, "Register Spotifast"),
                Action::OpenPersonalAppSetup,
            ),
            (
                // Translators: a button that opens a page explaining why
                // the banner it sits in appeared.
                gettext(locale, "Why?"),
                Action::OpenUrl(SLOW_BANNER_GUIDE.into()),
            ),
        ],
        Some(Action::CloseSlowBanner),
    );
}

/// A strip at the top of the main view after Spotify refused the key that decrypts a
/// song on this computer, offering another Spotify device, which fetches
/// its own audio, and the guide's explanation.
fn audio_key_banner(app: &mut App, ui: &mut egui::Ui) {
    use crate::i18n::gettext;
    let locale = app.locale;
    banner(
        app,
        ui,
        &gettext(
            locale,
            "Spotify won't let this computer play this song right now.",
        ),
        &gettext(
            locale,
            "Spotify refused the key that unlocks the audio. Spotifast can't fix that, but it sometimes clears up on its own. You can still play on your phone or another Spotify device.",
        ),
        false,
        vec![
            (
                gettext(locale, "Play on another device"),
                Action::PlayOnAnotherDevice,
            ),
            (
                gettext(locale, "Why?"),
                Action::OpenUrl(AUDIO_KEY_GUIDE.into()),
            ),
        ],
        Some(Action::CloseAudioKeyBanner),
    );
}

/// A notice at the top of the main view: a bold line, with a spinner while `busy`, a
/// sentence or two, its buttons with the main one first, and a close button
/// unless the notice goes by itself.
fn banner(
    app: &mut App,
    ui: &mut egui::Ui,
    title: &str,
    text: &str,
    busy: bool,
    buttons: Vec<(std::borrow::Cow<'static, str>, Action)>,
    close: Option<Action>,
) {
    use crate::i18n::gettext;
    let palette = app.palette;
    let locale = app.locale;
    Frame::new()
        .outer_margin(Margin {
            left: widgets::PAGE_PADDING as i8,
            right: widgets::PAGE_PADDING as i8,
            top: widgets::PAGE_TOP as i8,
            bottom: 0,
        })
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(Margin {
            left: 16,
            right: 6,
            top: 12,
            bottom: 12,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if let Some(close) = close
                    && theme::icon_button(
                        ui,
                        Icon::X,
                        16.0,
                        palette.secondary,
                        palette.text,
                        &gettext(locale, "Close banner"),
                    )
                    .clicked()
                {
                    app.actions.push(close);
                }
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.spacing_mut().item_spacing = vec2(8.0, 4.0);
                    ui.horizontal(|ui| {
                        if busy {
                            theme::spinner(ui, 14.0, palette.accent);
                        }
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(title)
                                    .font(theme::bold(14.0))
                                    .color(palette.text),
                            )
                            .wrap(),
                        );
                    });
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(text)
                                .font(theme::regular(13.0))
                                .color(palette.secondary),
                        )
                        .wrap(),
                    );
                    if !buttons.is_empty() {
                        ui.add_space(6.0);
                        ui.horizontal_wrapped(|ui| {
                            for (index, (label, action)) in buttons.into_iter().enumerate() {
                                if theme::pill_button(ui, &palette, &label, index == 0).clicked() {
                                    app.actions.push(action);
                                }
                            }
                        });
                    }
                });
            });
        });
}

/// The gradient behind a page's header, top to bottom: the art's colour
/// blended into the main view's card, stronger on a cover's own page than on an
/// overview, or else the palette's `tint` fading out, whose alpha is its
/// strength. `None` leaves the plain card.
fn header_wash(
    palette: &theme::Palette,
    overview: bool,
    art: Option<Color32>,
) -> Option<(Color32, Color32)> {
    if let Some(art) = art {
        let strength = if overview { 0.45 } else { 0.85 };
        return Some((blend(palette.panel, art, strength), palette.panel));
    }
    (palette.tint.a() > 0).then_some((palette.tint, Color32::TRANSPARENT))
}

/// Where the header's wash is painted: the top of the panel, moved up by
/// how far the page has scrolled, so it leaves with the hero.
fn header_rect(panel: Rect, scrolled: f32) -> Rect {
    Rect::from_min_size(
        panel.min - vec2(0.0, scrolled.max(0.0)),
        vec2(panel.width(), 340.0),
    )
}

/// The shadow the header casts on a page scrolled under it, deepening over
/// the first few points of scrolling. Dark in both themes, lighter over a
/// light page.
fn header_shadow(ui: &egui::Ui, page: Rect, scrolled: f32, dark: bool) {
    let depth = (scrolled / 24.0).clamp(0.0, 1.0);
    if depth <= 0.0 {
        return;
    }
    let strength = if dark { 110.0 } else { 36.0 };
    let rect = Rect::from_min_size(page.min, vec2(page.width(), 14.0));
    widgets::paint_vertical_gradient(
        ui,
        rect,
        egui::Color32::from_black_alpha((strength * depth) as u8),
        egui::Color32::TRANSPARENT,
    );
}

/// Without a native title bar, the top bar's whole strip drags the window,
/// not only the room between its controls.
const fn titlebar_spans_window(on_macos: bool, own_titlebar: bool) -> bool {
    on_macos || own_titlebar
}

/// The room the top bar's own controls need beside the caption buttons.
/// The bar spans the window, whose 760-point minimum always holds both.
const TOPBAR_MIN_CONTENT: f32 = 510.0;

/// Where the window's own buttons sit in the top bar, which spans the
/// whole window: beside its controls at either end, or above them when the
/// window is too narrow for both.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct WindowControlsReservation {
    /// The width left free at the bar's left end, for GNOME layouts that
    /// put buttons there. macOS's traffic lights sit above the bar instead
    /// (`theme::titlebar_inset`).
    pub left: f32,
    /// The width left free at the bar's right end.
    pub right: f32,
    /// How far the bar moves down, under the caption row.
    pub top: f32,
}

fn controls_reservation(
    footprint: fastframe_titlebar::Footprint,
    linux: bool,
    width: f32,
) -> WindowControlsReservation {
    let reserve = footprint.reserve(width, TOPBAR_MIN_CONTENT);
    let pushed = reserve.push_down > 0.0;
    WindowControlsReservation {
        left: if linux && footprint.left > 0.0 && !pushed {
            footprint.left
        } else {
            0.0
        },
        right: reserve.inline_width,
        top: reserve.push_down,
    }
}

/// The caption row's height: the top bar's, with its gap under it.
fn caption_header_height() -> f32 {
    theme::CUSTOM_TOP_BAR_HEIGHT + PANEL_GAP
}

pub(super) fn window_controls_reservation(
    ctx: &egui::Context,
    width: f32,
) -> WindowControlsReservation {
    controls_reservation(
        fastframe_titlebar::footprint(ctx, caption_header_height()),
        cfg!(target_os = "linux"),
        width,
    )
}

/// The top bar across the whole window, on the window's colour: nothing
/// else reaches the window's top edge.
fn top_bar(app: &mut App, ui: &mut egui::Ui) {
    let height = topbar::height(ui.ctx(), ui.available_width());
    egui::Panel::top("top-bar")
        .exact_size(height)
        .resizable(false)
        .show_separator_line(false)
        .frame(Frame::new().fill(app.palette.window))
        .show(ui, |ui| topbar::show(app, ui));
}

/// Draws the caption buttons over the window's top corners in the palette's
/// colours: Windows' glyphs in the secondary text colour, GNOME's round
/// buttons in the text colour on the hover surface.
fn caption_buttons(ui: &mut egui::Ui, palette: &theme::Palette, locale: crate::i18n::Locale) {
    use crate::i18n::gettext;
    let style = fastframe_titlebar::Style {
        icon: if cfg!(target_os = "linux") {
            palette.text
        } else {
            palette.secondary
        },
        button: palette.surface_hover,
        header_height: caption_header_height(),
    };
    // Literal calls, so xgettext extracts each label.
    let minimize = gettext(locale, "Minimize");
    let maximize = gettext(locale, "Maximize");
    let restore = gettext(locale, "Restore");
    let close = gettext(locale, "Close");
    let labels = fastframe_titlebar::Labels {
        minimize: &minimize,
        maximize: &maximize,
        restore: &restore,
        close: &close,
    };
    fastframe_titlebar::controls(ui, &style, &labels);
}

pub fn blend(base: Color32, tint: Color32, amount: f32) -> Color32 {
    let a = egui::Rgba::from(base);
    let b = egui::Rgba::from(tint);
    let mixed = a * (1.0 - amount) + b * amount;
    let mut color = Color32::from(mixed);
    color[3] = 255;
    color
}

fn toasts(app: &mut App, ctx: &egui::Context, bottom_offset: f32) {
    if app.toasts.is_empty() {
        return;
    }
    let palette = app.palette;
    egui::Area::new(egui::Id::new("toasts"))
        .anchor(Align2::RIGHT_BOTTOM, vec2(-20.0, -bottom_offset))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            for toast in &app.toasts {
                let age = toast.created.elapsed().as_secs_f32();
                let alpha = if age < 0.15 {
                    age / 0.15
                } else if age > 2.8 {
                    ((3.2 - age) / 0.4).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                ui.set_opacity(alpha);
                Frame::new()
                    .fill(palette.overlay)
                    .stroke(Stroke::new(1.0, palette.outline))
                    .corner_radius(CornerRadius::same(theme::RADIUS))
                    .inner_margin(Margin::symmetric(14, 10))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 4],
                        blur: 16,
                        spread: 0,
                        color: palette.shadow,
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (icon, color) = match toast.kind {
                                ToastKind::Info => (Icon::CircleCheck, palette.accent),
                                ToastKind::Error => (Icon::CircleAlert, palette.danger),
                            };
                            theme::icon(ui, icon, 16.0, color);
                            // Laid out at its own width. The area
                            // remembers its size, so after a short toast a
                            // label left to wrap at the area's width broke
                            // long messages on every word.
                            let galley = ui.painter().layout(
                                toast.message.clone(),
                                theme::medium(13.5),
                                palette.text,
                                280.0,
                            );
                            ui.add(egui::Label::new(galley));
                        });
                    });
            }
        });
}

#[cfg(test)]
mod window_chrome_tests {
    use super::*;
    use fastframe_titlebar::Footprint;

    /// Windows' three buttons in its 48-point header.
    const BUTTONS: Footprint = Footprint {
        left: 0.0,
        right: 3.0 * 36.0 + 5.0,
        height: theme::CUSTOM_TOP_BAR_HEIGHT,
    };

    #[test]
    fn the_top_strip_drags_a_window_without_a_native_title_bar() {
        assert!(titlebar_spans_window(true, false));
        assert!(titlebar_spans_window(false, true));
        assert!(!titlebar_spans_window(false, false));
    }

    /// The bar spans the window, so the caption buttons always sit at its
    /// right end, whatever panel is open beside the page.
    #[test]
    fn caption_space_belongs_to_the_top_bar() {
        let space = controls_reservation(BUTTONS, false, 1240.0);
        assert_eq!(
            space,
            WindowControlsReservation {
                left: 0.0,
                right: BUTTONS.right,
                top: 0.0,
            }
        );
        // No buttons on the right: full screen, the native frame, or
        // macOS's traffic lights, which the bar leaves room above itself
        // for.
        let lights = Footprint {
            left: 50.0,
            ..Footprint::default()
        };
        assert_eq!(
            controls_reservation(lights, false, 1240.0),
            WindowControlsReservation {
                left: 0.0,
                right: 0.0,
                top: 0.0,
            }
        );
        // A GNOME layout with buttons at both ends keeps both ends free.
        let gnome = Footprint {
            left: 40.0,
            right: 80.0,
            height: 44.0,
        };
        let space = controls_reservation(gnome, true, 1240.0);
        assert_eq!((space.left, space.right, space.top), (40.0, 80.0, 0.0));
    }

    #[test]
    fn a_window_too_narrow_for_both_stacks_caption_space_above_the_bar() {
        let space = controls_reservation(BUTTONS, false, TOPBAR_MIN_CONTENT);
        assert_eq!(space.right, 0.0);
        assert_eq!(space.top, BUTTONS.height);

        let inline = controls_reservation(BUTTONS, false, crate::window::MAIN_MIN_SIZE[0]);
        assert_eq!(inline.right, BUTTONS.right);
        assert_eq!(inline.top, 0.0);
    }

    /// Each card's corners are masked in the window's colour, outside the
    /// rounded outline only: the card's middle and edges' middles stay
    /// clear.
    #[test]
    fn card_corner_masks_stay_outside_the_rounded_outline() {
        let rect = Rect::from_min_size(egui::pos2(10.0, 20.0), vec2(300.0, 200.0));
        let radius = f32::from(PANEL_RADIUS);
        let shapes = corner_masks(rect, radius, Color32::BLACK);
        let meshes: Vec<_> = shapes
            .iter()
            .filter_map(|shape| match shape {
                egui::Shape::Mesh(mesh) => Some(mesh),
                _ => None,
            })
            .collect();
        assert_eq!(meshes.len(), 4, "one per corner");
        for mesh in meshes {
            for vertex in &mesh.vertices {
                let near_x = (vertex.pos.x - rect.left())
                    .abs()
                    .min((vertex.pos.x - rect.right()).abs());
                let near_y = (vertex.pos.y - rect.top())
                    .abs()
                    .min((vertex.pos.y - rect.bottom()).abs());
                assert!(near_x <= radius + 0.01 && near_y <= radius + 0.01);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page fades from its cover's colour while art tints the interface,
    /// otherwise from the palette's own wash, and from nothing when the
    /// palette has none.
    #[test]
    fn the_header_fades_from_the_art_or_the_palettes_wash() {
        let mut palette = theme::Palette::dark();
        assert_eq!(header_wash(&palette, false, None), None);

        let art = Color32::from_rgb(0x50, 0x38, 0xc8);
        let (page, _) = header_wash(&palette, false, Some(art)).unwrap();
        let (overview, _) = header_wash(&palette, true, Some(art)).unwrap();
        assert_eq!(page, blend(palette.panel, art, 0.85));
        assert_eq!(overview, blend(palette.panel, art, 0.45));

        palette.tint = Color32::from_black_alpha(24);
        assert_eq!(
            header_wash(&palette, false, None),
            Some((palette.tint, Color32::TRANSPARENT))
        );
        assert_eq!(
            header_wash(&palette, false, Some(art)).unwrap().0,
            page,
            "art wins over the wash"
        );
    }

    /// The wash sits at the top of an unscrolled page and leaves with the
    /// hero as the page scrolls, instead of staying behind the page.
    #[test]
    fn the_header_wash_scrolls_away_with_the_page() {
        let panel = Rect::from_min_size(egui::pos2(0.0, 80.0), vec2(800.0, 600.0));
        assert_eq!(header_rect(panel, 0.0).min, panel.min);
        assert_eq!(header_rect(panel, 0.0).width(), 800.0);
        assert_eq!(header_rect(panel, 120.0).min.y, panel.min.y - 120.0);
        assert_eq!(header_rect(panel, 120.0).max.y, panel.min.y + 220.0);
        assert!(
            header_rect(panel, 400.0).max.y < panel.min.y,
            "gone once scrolled past"
        );
        assert_eq!(
            header_rect(panel, -5.0).min,
            panel.min,
            "no overscroll bounce"
        );
    }

    /// The header casts a shadow only on a page scrolled under it, and it
    /// is black in both themes, never the page's own colour.
    #[test]
    fn the_header_shadow_appears_once_the_page_scrolls() {
        let ctx = egui::Context::default();
        let page = Rect::from_min_size(egui::pos2(0.0, 80.0), vec2(800.0, 600.0));
        let shadows = |scrolled: f32, dark: bool| {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                header_shadow(ui, page, scrolled, dark);
            });
            output.textures_delta.clear();
            output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Mesh(mesh) => Some(mesh.vertices.clone()),
                    _ => None,
                })
                .flatten()
                .collect::<Vec<_>>()
        };
        assert!(
            shadows(0.0, true).is_empty(),
            "nothing at the top of the page"
        );
        for dark in [true, false] {
            let vertices = shadows(40.0, dark);
            let top = vertices
                .iter()
                .find(|vertex| vertex.pos.y == page.top())
                .expect("a shadow along the page's top edge");
            assert!(top.color.a() > 0);
            assert_eq!((top.color.r(), top.color.g(), top.color.b()), (0, 0, 0));
        }
    }
}
