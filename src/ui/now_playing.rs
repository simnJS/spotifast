//! The playing song's Now Playing panel, on the right, after Spotify's own
//! "Now Playing View": the playing context as a sticky header, the big
//! cover, the title with its artists and saved check, then a Lyrics
//! preview, Related music videos, About the artist, Credits, On tour and
//! Next in queue. Each section without data is left out.

use egui::{
    Align, Color32, CornerRadius, Frame, Layout, Margin, Rect, Sense, UiBuilder, Vec2, pos2, vec2,
};

use crate::app::{App, CanvasRead, NowPlaying};
use crate::i18n::{gettext, ngettext, pgettext};
use crate::model::{Action, Concert, Credit, Loadable, NowPlayingView, NpvArtist, Page};
use crate::theme::{self, Icon, Palette};
use crate::util;

use super::widgets;

/// Spotify's `--section-padding`, `--section-border-radius` and the gap
/// between sections.
const SECTION_PADDING: i8 = 16;
const SECTION_RADIUS: u8 = 8;
const SECTION_GAP: f32 = 16.0;
/// The header's shadow is fully drawn after this much scroll.
const HEADER_SHADOW_SCROLL: f32 = 32.0;
/// The Lyrics preview card's height in Spotify's static mode.
const LYRICS_PREVIEW_HEIGHT: f32 = 177.0;
/// The width of a Related music videos card.
const VIDEO_CARD_WIDTH: f32 = 200.0;
/// How many skeleton sections show while the panel's data loads.
const SKELETONS: usize = 3;

pub fn side_panel(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let Some(now) = app.now_playing() else {
        return;
    };
    let fit = super::yielding_panel(
        ui.ctx(),
        "now-playing-panel",
        theme::SIDE_PANEL_MIN_WIDTH..=560.0,
        app.settings.queue_width,
        ui.available_width() - super::topbar::least_width(ui.ctx()),
    );
    let panel = egui::Panel::right("now-playing-panel")
        .resizable(true)
        .default_size(app.settings.queue_width)
        .size_range(fit.range.clone())
        .show_separator_line(false)
        .frame(Frame::new().fill(palette.panel).inner_margin(Margin {
            left: 12,
            right: 4,
            top: 12,
            bottom: 0,
        }));
    let response = panel.show(ui, |ui| {
        // The panel sits where the Queue would, under the same controls.
        let window_controls =
            super::window_controls_reservation(ui.ctx(), true, false, ui.available_width());
        ui.add_space(window_controls.queue_top);
        header(app, ui, &now);
        let header_bottom = ui.cursor().top();
        ui.add_space(4.0);
        // Back to the top when the song changes, as Spotify does.
        let shown_id = egui::Id::new("now-playing-panel-uri");
        let changed = ui
            .ctx()
            .data(|data| data.get_temp::<String>(shown_id))
            .is_none_or(|shown| shown != now.uri);
        if changed {
            ui.ctx()
                .data_mut(|data| data.insert_temp(shown_id, now.uri.clone()));
        }
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("now-playing-panel-scroll")
            .auto_shrink([false, false])
            .content_margin(Margin {
                left: 0,
                right: 8,
                top: 0,
                bottom: 16,
            });
        if changed {
            scroll = scroll.vertical_scroll_offset(0.0);
        }
        let output = scroll.show(ui, |ui| contents(app, ui, &now));
        header_shadow(ui, header_bottom, output.state.offset.y);
    });
    // The panel shares the Queue panel's width, so switching between them
    // never moves the page.
    let width = response.response.rect.width();
    if (width - app.settings.queue_width).abs() > 1.0
        && super::panel_width_chosen(ui.ctx(), "now-playing-panel", &fit)
    {
        app.settings.queue_width = width;
        app.actions.push(Action::SettingsChanged);
    }
}

/// The sticky header: the playing context, which opens on click, then the
/// song's menu and the close button.
fn header(app: &mut App, ui: &mut egui::Ui, now: &NowPlaying) {
    let palette = app.palette;
    let locale = app.locale;
    let from = app.playing_from();
    let mut close = false;
    let mut open = None;
    egui::Sides::new().shrink_left().show(
        ui,
        |ui| {
            ui.add_space(4.0);
            match from {
                Some(crate::model::PlayingFrom {
                    name,
                    page: Some(page),
                }) => {
                    if theme::link(ui, name, theme::bold(16.0), palette.text).clicked() {
                        open = Some(page);
                    }
                }
                Some(from) => {
                    theme::text(ui, from.name, theme::bold(16.0), palette.text);
                }
                None => {
                    let title = if now.album_name.is_empty() {
                        gettext(locale, "Now playing").into_owned()
                    } else {
                        now.album_name.clone()
                    };
                    theme::text(ui, title, theme::bold(16.0), palette.text);
                }
            }
        },
        |ui| {
            close = theme::icon_button(
                ui,
                Icon::X,
                18.0,
                palette.secondary,
                palette.text,
                &gettext(locale, "Close"),
            )
            .clicked();
            let more = theme::icon_button(
                ui,
                Icon::Ellipsis,
                18.0,
                palette.secondary,
                palette.text,
                &gettext(locale, "More"),
            );
            egui::Popup::menu(&more)
                .frame(widgets::menu_frame(&palette))
                .show(|ui| widgets::context_menu_items(ui, app, &now.uri, &now.title, None));
        },
    );
    if let Some(page) = open {
        app.actions.push(Action::Open(page));
    }
    if close {
        app.actions.push(Action::ToggleNowPlayingPanel);
    }
    ui.add_space(8.0);
}

/// Spotify fades the header's shadow in over the first 32 points of
/// scroll.
fn header_shadow(ui: &egui::Ui, top: f32, offset: f32) {
    let strength = (offset / HEADER_SHADOW_SCROLL).clamp(0.0, 1.0);
    if strength <= 0.0 {
        return;
    }
    let rect = Rect::from_min_max(
        pos2(ui.max_rect().left() - 12.0, top),
        pos2(ui.max_rect().right() + 4.0, top + 10.0),
    );
    // Spotify's `0 6px 10px #0009`.
    let shadow = Color32::from_black_alpha((153.0 * strength) as u8);
    fade(ui.painter(), rect, shadow, Color32::TRANSPARENT);
}

/// A vertical gradient from `top` to `bottom` over `rect`.
fn fade(painter: &egui::Painter, rect: Rect, top: Color32, bottom: Color32) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

fn contents(app: &mut App, ui: &mut egui::Ui, now: &NowPlaying) {
    ui.spacing_mut().item_spacing.y = 0.0;
    let (cover, over_canvas) = header_media(app, ui, now);
    if let Some(id) = now.album_id.clone() {
        let cover = cover.on_hover_cursor(egui::CursorIcon::PointingHand);
        if cover.clicked() {
            app.actions.push(Action::Open(Page::Album(id)));
        }
    }
    ui.add_space(SECTION_GAP);
    title_row(app, ui, now, over_canvas);
    let view = app
        .now_playing_views
        .get(&now.uri)
        .map(|read| read.view.clone());
    // Asked after drawing; the request waits for the session itself.
    if view.is_none() && !now.is_episode {
        app.actions.push(Action::RequestNowPlayingView);
    }
    lyrics_preview(app, ui, now);
    match view {
        Some(Loadable::Loading) => {
            for _ in 0..SKELETONS {
                ui.add_space(SECTION_GAP);
                skeleton(ui, &app.palette);
            }
        }
        Some(Loadable::Loaded(view)) => sections(app, ui, &view),
        // Without Spotify's answer, the panel keeps to what playback knows.
        _ => {}
    }
    next_in_queue(app, ui);
}

/// Draws the panel's header media: the cover, or the song's Canvas looping
/// in a tall box as Spotify's own panel does. This is the one place the
/// header's picture is drawn. Returns the media's response and whether a
/// Canvas shows, which turns the text below it to the full text colour.
pub fn header_media(app: &mut App, ui: &mut egui::Ui, now: &NowPlaying) -> (egui::Response, bool) {
    let ctx = ui.ctx().clone();
    let width = ui.available_width();
    let texture = canvas_frame(app, &ctx, now);
    // The box stays tall while the next song's Canvas is asked for, so
    // going from one Canvas to another never bounces the panel.
    let tall_id = egui::Id::new("now-playing-canvas-tall");
    let was_tall = ctx
        .data(|data| data.get_temp::<bool>(tall_id))
        .unwrap_or(false);
    let failed = app.canvas.as_ref().is_some_and(|canvas| {
        canvas.track_uri == now.uri && canvas.player.error().is_some() && texture.is_none()
    });
    let tall = !failed
        && match app.canvases.get(&now.uri) {
            Some(CanvasRead::Video) => true,
            Some(CanvasRead::Loading) => was_tall,
            _ => false,
        };
    ctx.data_mut(|data| data.insert_temp(tall_id, tall));
    let grow = ctx.animate_bool_with_time(
        egui::Id::new("now-playing-canvas-height"),
        tall,
        crate::motion::seconds(&ctx, CANVAS_FADE),
    );
    // Spotify's 9/14.5, or 9/11 in a panel under 608 points tall.
    let ratio = if ui.clip_rect().height() < 608.0 {
        11.0 / 9.0
    } else {
        14.5 / 9.0
    };
    let height = egui::lerp(width..=width * ratio, grow);
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    // Keyed by song, so a new song's video fades in over its own cover and
    // the last song's never shows.
    let shown = ctx.animate_bool_with_time(
        egui::Id::new(("now-playing-canvas-fade", &now.uri)),
        texture.is_some(),
        crate::motion::seconds(&ctx, CANVAS_FADE),
    );
    let radius = f32::from(SECTION_RADIUS);
    if shown < 1.0 {
        let side = width.min(height);
        widgets::playing_cover(
            ui,
            &app.palette,
            app.backend.art(),
            "now-playing-panel",
            now.art_url.as_deref().or(now.art_small.as_deref()),
            Rect::from_center_size(rect.center(), Vec2::splat(side)),
            radius,
        );
    }
    if let Some(texture) = texture
        && shown > 0.0
    {
        egui::Image::from_texture(&texture)
            .uv(fill_uv(texture.size_vec2(), rect.size()))
            .corner_radius(radius)
            .tint(Color32::WHITE.gamma_multiply(shown))
            .paint_at(ui, rect);
        canvas_gradient(ui.painter(), rect, app.palette.panel, shown);
    }
    (response, shown >= 0.5)
}

/// How long the header takes to grow for a Canvas, and the video to fade in
/// over the cover: Spotify's `.6s`.
const CANVAS_FADE: f32 = 0.6;

/// The playing song's Canvas frame, if it has one decoded. It plays with
/// the music and holds its frame while paused or when less motion is asked
/// for.
fn canvas_frame(
    app: &mut App,
    ctx: &egui::Context,
    now: &NowPlaying,
) -> Option<egui::TextureHandle> {
    let canvas = app
        .canvas
        .as_mut()
        .filter(|canvas| canvas.track_uri == now.uri)?;
    let at = std::time::Instant::now();
    if now.playing && !crate::motion::reduced(ctx) {
        canvas.player.resume();
    } else {
        canvas.player.pause(at);
    }
    canvas.player.frame(ctx, at)
}

/// The part of a `texture` sized picture that fills `size` without
/// stretching, centred, as CSS's `object-fit: cover`.
fn fill_uv(texture: Vec2, size: Vec2) -> Rect {
    if texture.x <= 0.0 || texture.y <= 0.0 || size.x <= 0.0 || size.y <= 0.0 {
        return Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    }
    let picture = texture.x / texture.y;
    let target = size.x / size.y;
    if picture > target {
        let keep = target / picture;
        Rect::from_min_max(pos2((1.0 - keep) / 2.0, 0.0), pos2((1.0 + keep) / 2.0, 1.0))
    } else {
        let keep = picture / target;
        Rect::from_min_max(pos2(0.0, (1.0 - keep) / 2.0), pos2(1.0, (1.0 + keep) / 2.0))
    }
}

/// Spotify's gradient over a Canvas, in the panel's colour:
/// `linear-gradient(#121212d9 0%, #12121200 30% 80%, #121212cc 93.25%,
/// #121212 100%)`, scaled by `strength` while the video fades in.
fn canvas_gradient(painter: &egui::Painter, rect: Rect, panel: Color32, strength: f32) {
    let color = |alpha: f32| panel.gamma_multiply(alpha * strength);
    let band = |from: f32, to: f32| {
        Rect::from_min_max(
            pos2(rect.left(), rect.top() + rect.height() * from),
            pos2(rect.right(), rect.top() + rect.height() * to),
        )
    };
    fade(painter, band(0.0, 0.3), color(0.85), color(0.0));
    fade(painter, band(0.8, 0.9325), color(0.0), color(0.8));
    fade(painter, band(0.9325, 1.0), color(0.8), color(1.0));
}

/// The title, its artists as links, and the saved check.
fn title_row(app: &mut App, ui: &mut egui::Ui, now: &NowPlaying, over_canvas: bool) {
    let palette = app.palette;
    // Over a Canvas the artists take the full text colour, as Spotify's do.
    let subdued = if over_canvas {
        palette.text
    } else {
        palette.secondary
    };
    let locale = app.locale;
    let mut open = None;
    let saved = app.is_saved(&now.uri).unwrap_or(false);
    let mut toggle_saved = false;
    egui::Sides::new().shrink_left().show(
        ui,
        |ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                let title = ui.add(
                    egui::Label::new(
                        egui::RichText::new(&now.title)
                            .font(theme::bold(24.0))
                            .color(palette.text),
                    )
                    .wrap()
                    .selectable(false)
                    .sense(Sense::click()),
                );
                if let Some(id) = now.album_id.clone()
                    && title
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                {
                    open = Some(Page::Album(id));
                }
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    if now.artists.is_empty() {
                        theme::text(ui, &now.subtitle, theme::regular(15.0), subdued);
                    }
                    for (index, artist) in now.artists.iter().enumerate() {
                        if index > 0 {
                            theme::text(ui, ", ", theme::regular(15.0), subdued);
                        }
                        let link = theme::link(ui, &artist.name, theme::regular(15.0), subdued);
                        if let Some(id) = artist.id.clone()
                            && link.clicked()
                        {
                            open = Some(Page::Artist(id));
                        }
                    }
                });
            });
        },
        |ui| {
            if now.is_episode {
                return;
            }
            let (icon, color, hover, tooltip) = if saved {
                (
                    Icon::CircleCheck,
                    palette.accent,
                    palette.accent_hover,
                    gettext(locale, "Remove from Liked Songs"),
                )
            } else {
                (
                    Icon::CirclePlus,
                    subdued,
                    palette.text,
                    gettext(locale, "Save to Liked Songs"),
                )
            };
            toggle_saved = theme::icon_button(ui, icon, 20.0, color, hover, &tooltip).clicked();
        },
    );
    if let Some(page) = open {
        app.actions.push(Action::Open(page));
    }
    if toggle_saved {
        app.actions.push(Action::ToggleSaved(now.uri.clone()));
    }
}

/// A section's box: Spotify's elevated background, padding and corners.
fn section<R>(
    ui: &mut egui::Ui,
    fill: Color32,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    ui.add_space(SECTION_GAP);
    Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(SECTION_RADIUS))
        .inner_margin(Margin::same(SECTION_PADDING))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            add_contents(ui)
        })
}

/// A section's title, with an optional trailing link button. Says whether
/// the link was clicked.
fn section_title(ui: &mut egui::Ui, palette: &Palette, title: &str, action: Option<&str>) -> bool {
    let mut clicked = false;
    egui::Sides::new().shrink_left().show(
        ui,
        |ui| {
            theme::text(ui, title, theme::bold(16.0), palette.text);
        },
        |ui| {
            if let Some(action) = action {
                clicked =
                    theme::link(ui, action, theme::semibold(13.0), palette.secondary).clicked();
            }
        },
    );
    clicked
}

/// The colour behind the Lyrics preview: the cover's own, darkened enough
/// for white words, as Spotify's dynamic colours are.
fn lyrics_color(app: &mut App, now: &NowPlaying) -> Color32 {
    let url = now.art_small.as_deref().or(now.art_url.as_deref());
    match app.tint_for(url) {
        Some(color) => super::blend(color, Color32::BLACK, 0.35),
        None => app.palette.surface_active,
    }
}

/// A few lines of the song's lyrics from where it is, on the cover's
/// colour. A click opens the full Lyrics panel.
fn lyrics_preview(app: &mut App, ui: &mut egui::Ui, now: &NowPlaying) {
    if app.lyrics_uri.as_deref() != Some(now.uri.as_str()) {
        return;
    }
    let Loadable::Loaded(Some(lyrics)) = &app.lyrics else {
        return;
    };
    if lyrics.instrumental || lyrics.lines.is_empty() {
        return;
    }
    let first = lyrics.active_line(now.position_ms).unwrap_or(0);
    let lines: Vec<String> = lyrics
        .lines
        .iter()
        .skip(first)
        .take(4)
        .map(|line| line.text.clone())
        .collect();
    let locale = app.locale;
    let fill = lyrics_color(app, now);
    let card = section(ui, fill, |ui| {
        theme::text(
            ui,
            gettext(locale, "Lyrics preview"),
            theme::bold(16.0),
            Color32::WHITE,
        );
        ui.add_space(SECTION_GAP);
        let width = ui.available_width();
        let height = LYRICS_PREVIEW_HEIGHT - 2.0 * f32::from(SECTION_PADDING) - 16.0 - SECTION_GAP;
        let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        let mut text = ui.new_child(
            UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::top_down(Align::Min)),
        );
        text.set_clip_rect(rect.intersect(ui.clip_rect()));
        text.spacing_mut().item_spacing.y = 4.0;
        for line in &lines {
            let galley = crate::bidi::layout(
                text.painter(),
                line,
                theme::bold(22.0),
                Color32::WHITE,
                width,
                usize::MAX,
                None,
            );
            text.add(egui::Label::new(galley).selectable(false));
        }
        // The words fade out at the card's foot, as Spotify's mask does.
        let foot = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 28.0), rect.max);
        fade(text.painter(), foot, Color32::TRANSPARENT, fill);
        rect
    });
    let response = ui
        .interact(
            card.response.rect,
            card.response.id.with("open"),
            Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.hovered() {
        // Spotify's "Show more" pill, bottom right.
        let label = gettext(locale, "Show more");
        let galley =
            ui.painter()
                .layout_no_wrap(label.into_owned(), theme::semibold(12.0), Color32::BLACK);
        let size = galley.size() + vec2(24.0, 12.0);
        let pill = Rect::from_min_size(card.response.rect.max - size - Vec2::splat(12.0), size);
        ui.painter()
            .rect_filled(pill, pill.height() / 2.0, Color32::WHITE);
        ui.painter()
            .galley(pill.center() - galley.size() / 2.0, galley, Color32::BLACK);
    }
    if response.clicked() {
        app.actions.push(Action::ToggleLyricsPanel);
    }
}

/// The sections Spotify's `queryNpvArtist` answer fills, in its order.
fn sections(app: &mut App, ui: &mut egui::Ui, view: &NowPlayingView) {
    related_videos(app, ui, view);
    if let Some(artist) = view
        .artist
        .as_ref()
        .filter(|artist| !artist.biography.is_empty())
    {
        about_artist(app, ui, artist);
    }
    if !view.credits.is_empty() {
        credits(app, ui, &view.credits);
    }
    if !view.concerts.is_empty() {
        on_tour(app, ui, view);
    }
}

/// Related music videos. Spotifast cannot play protected video, so a card
/// plays its song's audio.
fn related_videos(app: &mut App, ui: &mut egui::Ui, view: &NowPlayingView) {
    if view.related_videos.is_empty() {
        return;
    }
    let palette = app.palette;
    ui.add_space(SECTION_GAP);
    section_title(
        ui,
        &palette,
        &gettext(app.locale, "Related music videos"),
        None,
    );
    ui.add_space(8.0);
    egui::ScrollArea::horizontal()
        .id_salt("now-playing-videos")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                for video in &view.related_videos {
                    let thumb = vec2(VIDEO_CARD_WIDTH, VIDEO_CARD_WIDTH * 9.0 / 16.0);
                    let (rect, response) =
                        ui.allocate_exact_size(vec2(thumb.x, thumb.y + 46.0), Sense::click());
                    let response = response
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text(gettext(app.locale, "Play the song"));
                    if response.hovered() {
                        ui.painter()
                            .rect_filled(rect.expand(4.0), 6.0, palette.surface_hover);
                    }
                    let picture = Rect::from_min_size(rect.min, thumb);
                    let back = video
                        .color
                        .map_or(palette.surface, |[r, g, b]| Color32::from_rgb(r, g, b));
                    ui.painter().rect_filled(picture, 6.0, back);
                    widgets::paint_cover(
                        ui,
                        &palette,
                        video.image.as_deref(),
                        picture,
                        6.0,
                        Icon::ListVideo,
                        Some(app.backend.art()),
                    );
                    let mut words = ui.new_child(
                        UiBuilder::new()
                            .max_rect(Rect::from_min_max(
                                pos2(rect.left(), picture.bottom() + 6.0),
                                rect.max,
                            ))
                            .layout(Layout::top_down(Align::Min)),
                    );
                    words.spacing_mut().item_spacing.y = 2.0;
                    theme::text(&mut words, &video.name, theme::semibold(14.0), palette.text);
                    theme::text(
                        &mut words,
                        video.artists.join(", "),
                        theme::regular(13.0),
                        palette.secondary,
                    );
                    if response.clicked() {
                        app.actions.push(Action::PlayUris {
                            uris: vec![video.uri.clone()],
                            index: 0,
                        });
                    }
                }
            });
        });
}

/// About the artist: a gallery picture or the avatar, the name, monthly
/// listeners, Follow, and the start of the biography.
fn about_artist(app: &mut App, ui: &mut egui::Ui, artist: &NpvArtist) {
    let palette = app.palette;
    let locale = app.locale;
    let artist_page = util::uri_id(&artist.uri).map(|id| Page::Artist(id.to_string()));
    let title = gettext(locale, "About the artist");
    ui.add_space(SECTION_GAP);
    Frame::new()
        .fill(palette.surface)
        .corner_radius(CornerRadius::same(SECTION_RADIUS))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            let width = ui.available_width();
            if let Some(image) = &artist.gallery_image {
                // Spotify's 3:2 header, its title over the picture.
                let (rect, picture) =
                    ui.allocate_exact_size(vec2(width, width * 2.0 / 3.0), Sense::click());
                widgets::paint_cover(
                    ui,
                    &palette,
                    Some(image),
                    rect,
                    f32::from(SECTION_RADIUS),
                    Icon::User,
                    Some(app.backend.art()),
                );
                let top = Rect::from_min_size(rect.min, vec2(width, 56.0));
                fade(
                    ui.painter(),
                    top,
                    Color32::from_black_alpha(140),
                    Color32::TRANSPARENT,
                );
                ui.painter().text(
                    rect.min + Vec2::splat(f32::from(SECTION_PADDING)),
                    egui::Align2::LEFT_TOP,
                    &title,
                    theme::bold(16.0),
                    Color32::WHITE,
                );
                if let Some(page) = artist_page.clone()
                    && picture
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                {
                    app.actions.push(Action::Open(page));
                }
            } else {
                Frame::new()
                    .inner_margin(Margin {
                        left: SECTION_PADDING,
                        right: SECTION_PADDING,
                        top: SECTION_PADDING,
                        bottom: 0,
                    })
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        theme::text(ui, title.as_ref(), theme::bold(16.0), palette.text);
                        if let Some(avatar) = &artist.avatar {
                            ui.add_space(12.0);
                            let (rect, picture) =
                                ui.allocate_exact_size(Vec2::splat(80.0), Sense::click());
                            widgets::paint_cover(
                                ui,
                                &palette,
                                Some(avatar),
                                rect,
                                40.0,
                                Icon::User,
                                Some(app.backend.art()),
                            );
                            if let Some(page) = artist_page.clone()
                                && picture
                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                    .clicked()
                            {
                                app.actions.push(Action::Open(page));
                            }
                        }
                    });
            }
            Frame::new()
                .inner_margin(Margin::same(SECTION_PADDING))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let name = theme::link(ui, &artist.name, theme::bold(16.0), palette.text);
                        if let Some(page) = artist_page.clone()
                            && name.clicked()
                        {
                            app.actions.push(Action::Open(page));
                        }
                        if artist.verified {
                            theme::icon(ui, Icon::BadgeCheck, 16.0, palette.accent)
                                .on_hover_text(gettext(locale, "Verified artist"));
                        }
                    });
                    ui.add_space(8.0);
                    // Shown once Spotify says whether the artist is followed.
                    let following = app.is_saved(&artist.uri);
                    let mut follow = false;
                    egui::Sides::new().shrink_left().show(
                        ui,
                        |ui| {
                            if let Some(listeners) = artist.monthly_listeners {
                                let label = ngettext(
                                    locale,
                                    // Translators: {count} is the number of people who listened
                                    // to an artist in the last 28 days, as Spotify counts them.
                                    "{count} monthly listener",
                                    "{count} monthly listeners",
                                    u32::try_from(listeners).unwrap_or(u32::MAX),
                                )
                                .replace("{count}", &util::format_count(listeners));
                                theme::text(ui, label, theme::regular(14.0), palette.secondary);
                            }
                        },
                        |ui| {
                            let Some(following) = following else {
                                return;
                            };
                            let label = if following {
                                pgettext(locale, "artist", "Following")
                            } else {
                                pgettext(locale, "artist", "Follow")
                            };
                            follow = theme::pill_button(ui, &palette, &label, false).clicked();
                        },
                    );
                    if follow {
                        app.actions.push(Action::ToggleSaved(artist.uri.clone()));
                    }
                    ui.add_space(12.0);
                    let galley = crate::bidi::layout(
                        ui.painter(),
                        &artist.biography,
                        theme::regular(14.0),
                        palette.secondary,
                        ui.available_width(),
                        3,
                        Some(crate::bidi::ELLIPSIS),
                    );
                    ui.add(egui::Label::new(galley).selectable(false));
                });
        });
}

/// Credits: each contributor with their roles. An artist opens its page,
/// a web credit its link.
fn credits(app: &mut App, ui: &mut egui::Ui, credits: &[Credit]) {
    let palette = app.palette;
    let locale = app.locale;
    section(ui, palette.surface, |ui| {
        section_title(ui, &palette, &gettext(locale, "Credits"), None);
        for credit in credits {
            ui.add_space(12.0);
            let target = credit
                .uri
                .as_deref()
                .and_then(util::uri_id)
                .map(|id| Action::Open(Page::Artist(id.to_string())))
                .or_else(|| credit.url.clone().map(Action::OpenUrl));
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    let name = if target.is_some() {
                        theme::link(ui, &credit.name, theme::regular(15.0), palette.text)
                    } else {
                        theme::text(ui, &credit.name, theme::regular(15.0), palette.text)
                    };
                    if let Some(action) = target.clone()
                        && name.clicked()
                    {
                        app.actions.push(action);
                    }
                    if !credit.roles.is_empty() {
                        theme::text(
                            ui,
                            credit.roles.join(" \u{2022} "),
                            theme::regular(13.0),
                            palette.secondary,
                        );
                    }
                });
                if credit.uri.is_none() && credit.url.is_some() {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        theme::icon(ui, Icon::ExternalLink, 14.0, palette.secondary);
                    });
                }
            });
        }
    });
}

/// The month's short name, as dates elsewhere write it.
fn month(locale: crate::i18n::Locale, date: &str) -> String {
    let name = match date.get(5..7) {
        Some("01") => pgettext(locale, "month", "Jan"),
        Some("02") => pgettext(locale, "month", "Feb"),
        Some("03") => pgettext(locale, "month", "Mar"),
        Some("04") => pgettext(locale, "month", "Apr"),
        Some("05") => pgettext(locale, "month", "May"),
        Some("06") => pgettext(locale, "month", "Jun"),
        Some("07") => pgettext(locale, "month", "Jul"),
        Some("08") => pgettext(locale, "month", "Aug"),
        Some("09") => pgettext(locale, "month", "Sep"),
        Some("10") => pgettext(locale, "month", "Oct"),
        Some("11") => pgettext(locale, "month", "Nov"),
        Some("12") => pgettext(locale, "month", "Dec"),
        _ => return String::new(),
    };
    name.into_owned()
}

/// On tour: a date tile, the venue and the city for each concert. A click
/// opens the concert on Spotify's site; "Show all" opens the artist, whose
/// page lists every one.
fn on_tour(app: &mut App, ui: &mut egui::Ui, view: &NowPlayingView) {
    let palette = app.palette;
    let locale = app.locale;
    let artist_page = view
        .artist
        .as_ref()
        .and_then(|artist| util::uri_id(&artist.uri))
        .map(|id| Page::Artist(id.to_string()));
    let more = view
        .concerts_total
        .is_some_and(|total| total != view.concerts.len() as u64)
        && artist_page.is_some();
    let show_all = gettext(locale, "Show all");
    section(ui, palette.surface, |ui| {
        if section_title(
            ui,
            &palette,
            &gettext(locale, "On tour"),
            more.then_some(show_all.as_ref()),
        ) && let Some(page) = artist_page.clone()
        {
            app.actions.push(Action::Open(page));
        }
        ui.add_space(8.0);
        for concert in &view.concerts {
            concert_row(app, ui, concert);
        }
    });
}

fn concert_row(app: &mut App, ui: &mut egui::Ui, concert: &Concert) {
    let palette = app.palette;
    let locale = app.locale;
    let width = ui.available_width() + 16.0;
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::click());
    let row = Rect::from_min_size(pos2(rect.left() - 8.0, rect.top()), vec2(width, 64.0));
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.is_pointer_button_down_on() {
        ui.painter().rect_filled(row, 6.0, palette.surface_active);
    } else if response.hovered() {
        ui.painter().rect_filled(row, 6.0, palette.surface_hover);
    }
    let tile = Rect::from_min_size(rect.min + vec2(0.0, 8.0), Vec2::splat(48.0));
    ui.painter().rect_filled(tile, 6.0, palette.surface_hover);
    ui.painter().text(
        pos2(tile.center().x, tile.top() + 8.0),
        egui::Align2::CENTER_TOP,
        month(locale, &concert.date),
        theme::semibold(11.0),
        palette.secondary,
    );
    let day = concert
        .date
        .get(8..10)
        .map(|day| day.trim_start_matches('0').to_string())
        .unwrap_or_default();
    ui.painter().text(
        pos2(tile.center().x, tile.bottom() - 6.0),
        egui::Align2::CENTER_BOTTOM,
        day,
        theme::bold(18.0),
        palette.text,
    );
    let words = Rect::from_min_max(pos2(tile.right() + 12.0, rect.top() + 12.0), rect.max);
    let mut text = ui.new_child(
        UiBuilder::new()
            .max_rect(words)
            .layout(Layout::top_down(Align::Min)),
    );
    text.spacing_mut().item_spacing.y = 2.0;
    let (first, second) = if concert.venue.is_empty() {
        (concert.city.as_str(), "")
    } else {
        (concert.venue.as_str(), concert.city.as_str())
    };
    theme::text(&mut text, first, theme::regular(15.0), palette.text);
    if !second.is_empty() {
        theme::text(&mut text, second, theme::regular(13.0), palette.secondary);
    }
    if response.clicked() {
        app.actions.push(Action::OpenUrl(concert.url()));
    }
}

/// Next in queue: the queue's head, which plays on click, and a way to the
/// whole queue.
fn next_in_queue(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let locale = app.locale;
    let loaded = matches!(app.queue, Loadable::Loaded(_));
    let empty = loaded && super::queue::queue_is_empty(app);
    section(ui, palette.surface, |ui| {
        if empty {
            section_title(ui, &palette, &gettext(locale, "Your queue is empty"), None);
            ui.add_space(12.0);
            if theme::pill_button(
                ui,
                &palette,
                &gettext(locale, "Search for something new"),
                false,
            )
            .clicked()
            {
                app.actions.push(Action::Open(Page::Search));
            }
            return;
        }
        if section_title(
            ui,
            &palette,
            &gettext(locale, "Next in queue"),
            Some(&gettext(locale, "Open queue")),
        ) {
            app.actions.push(Action::ToggleQueuePanel);
        }
        if loaded {
            ui.add_space(8.0);
            super::queue::queue_row(app, ui, 0, true, 0.0);
        }
    });
}

/// Spotify's loading section: a box with one bold bar and three thinner
/// ones.
fn skeleton(ui: &mut egui::Ui, palette: &Palette) {
    Frame::new()
        .fill(palette.surface)
        .corner_radius(CornerRadius::same(SECTION_RADIUS))
        .inner_margin(Margin::same(SECTION_PADDING))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let width = ui.available_width();
            let bar = if palette.dark {
                palette.surface_active
            } else {
                palette.surface_hover
            };
            for (index, share) in [0.25_f32, 0.6, 0.95, 0.45].into_iter().enumerate() {
                let height = if index == 0 { 16.0 } else { 12.0 };
                let (rect, _) = ui.allocate_exact_size(vec2(width * share, height), Sense::hover());
                ui.painter().rect_filled(rect, 4.0, bar);
                ui.add_space(if index == 0 { 14.0 } else { 8.0 });
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_playing_month_reads_the_iso_date() {
        let locale = crate::i18n::Locale::default();
        assert_eq!(month(locale, "2027-03-14T20:00:00+01:00"), "Mar");
        assert_eq!(month(locale, "2027"), "");
    }

    #[test]
    fn now_playing_header_shadow_grows_over_the_first_32_points() {
        let ctx = egui::Context::default();
        let mut shapes = Vec::new();
        for offset in [0.0, 16.0, 64.0] {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                header_shadow(ui, 0.0, offset);
            });
            output.textures_delta.clear();
            shapes.push(output.shapes.len());
        }
        assert_eq!(shapes[0], 0, "no shadow at rest");
        assert!(shapes[1] > 0 && shapes[2] > 0);
    }
}
