//! The artist page, laid out as Spotify's desktop app lays it out: a
//! full-width banner with the artist's picture, an action bar, Popular,
//! the discography, related artists, an About card, and a header that
//! stays at the top once the banner has scrolled away.

use std::sync::Arc;

use egui::{Align, Color32, Layout, Rect, Sense, UiBuilder, Vec2, pos2, vec2};

use crate::api::models::{Album, Artist, PlayableItem, Track, pick_image};
use crate::app::App;
use crate::i18n::{Locale, gettext, ngettext, pgettext};
use crate::model::{
    Action, Concert, Dialog, DiscographyFilter, Loadable, NpvArtist, Page, RowContext,
};
use crate::theme::{self, Icon, Palette};
use crate::util;

use super::collection::{HeroArt, hero_images};
use super::widgets::{self, CoverSources, TrackRow};

/// The banner's height at its narrowest and widest.
const BANNER_MIN: f32 = 280.0;
const BANNER_MAX: f32 = 400.0;
/// The largest size the artist's name is drawn at, Spotify's 6rem.
const NAME_MAX: f32 = 96.0;
const NAME_MIN: f32 = 32.0;
/// The header that takes the banner's place once it has scrolled away.
const STICKY_HEIGHT: f32 = 64.0;
/// How far the colour under the action bar reaches before it fades out.
const ACTION_WASH: f32 = 232.0;
/// Spotify's About card: as wide as 672 points of text and 40 of padding
/// on each side, and 516 high when it has a picture.
const ABOUT_WIDTH: f32 = 752.0;
const ABOUT_HEIGHT: f32 = 516.0;
const ABOUT_PADDING: f32 = 40.0;
/// The blue of Spotify's verified badge, which no theme recolours.
const VERIFIED_BLUE: Color32 = Color32::from_rgb(0x4c, 0xb3, 0xff);
/// How many popular songs show before "See more".
const POPULAR_SHOWN: usize = 5;

/// Draws the artist page and keeps discography loading placeholders in place.
pub fn show(app: &mut App, ui: &mut egui::Ui, id: &str) {
    if !app.artist_pages.contains_key(id) {
        app.ensure_loaded(Page::Artist(id.to_string()));
    }
    let Some(page) = app.artist_pages.remove(id) else {
        return;
    };
    let preview =
        super::loading_preview(ui.ctx(), id, &page.artist, || app.known_artist(id).cloned());
    let palette = app.palette;
    let locale = app.locale;
    match &page.artist {
        Loadable::Loaded(artist) => {
            let about = npv_artist(app, &artist.uri);
            let banner = artist_banner(app, ui, artist, preview.as_deref(), about.as_ref());
            action_wash(app, ui, artist, banner);
            artist_actions(app, ui, artist);
            // The same overview supplies both play counts and the upcoming release.
            app.actions
                .push(Action::RequestPlayCounts(artist.uri.clone()));
            popular(app, ui, id, artist, &page);
            ui.add_space(40.0);

            if let Some(upcoming) = app.artist_prereleases.get(&artist.uri).cloned()
                && upcoming.is_upcoming(jiff::Timestamp::now().as_second())
            {
                upcoming_release(app, ui, &upcoming);
                ui.add_space(40.0);
            }

            if !page.concerts.is_empty() {
                concerts_section(app, ui, id, &page.concerts);
                ui.add_space(40.0);
            }

            discography(app, ui, id, &page);
            ui.add_space(40.0);

            if let Loadable::Loaded(related) = &page.related
                && !related.is_empty()
            {
                fans_also_like(app, ui, related);
                ui.add_space(28.0);
            }

            request_appears_on(app, ui, id, &page);
            if let Some(list) = page.albums.get(DiscographyFilter::AppearsOn.groups())
                && !list.items.is_empty()
            {
                let title = gettext(locale, "Appears On");
                shelf(ui, &palette, "appears-on", &title, |ui| {
                    for album in &list.items {
                        album_card(app, ui, album, "appears-on-menu", false);
                    }
                });
                ui.add_space(28.0);
            }

            about_card(app, ui, artist, about.as_ref());
            sticky_header(app, ui, artist, banner);
        }
        Loadable::Loading | Loadable::NotLoaded => {
            if let Some(artist) = &preview {
                artist_banner(app, ui, artist, None, None);
            } else {
                ui.add_space(40.0);
            }
            if !app.network_reachable {
                widgets::empty_state(
                    ui,
                    &palette,
                    Icon::WifiOff,
                    &gettext(app.locale, "You're offline"),
                    &gettext(
                        app.locale,
                        "Spotifast will reconnect automatically when your connection returns.",
                    ),
                );
            } else {
                if let Some(artist) = &preview {
                    ui.add_enabled_ui(false, |ui| artist_actions(app, ui, artist));
                }
                widgets::skeleton_rows(ui, &palette, app.locale, POPULAR_SHOWN);
            }
        }
        Loadable::Failed(error) => {
            let error = error.clone();
            if let Some(artist) = &preview {
                artist_banner(app, ui, artist, None, None);
            } else {
                ui.add_space(40.0);
            }
            if !app.network_reachable
                || error.contains("network error")
                || error.contains("Network connection error")
            {
                widgets::empty_state(
                    ui,
                    &palette,
                    Icon::WifiOff,
                    &gettext(app.locale, "You're offline"),
                    &gettext(
                        app.locale,
                        "Spotifast will reconnect automatically when your connection returns.",
                    ),
                );
            } else {
                if let Some(artist) = &preview {
                    ui.add_enabled_ui(false, |ui| artist_actions(app, ui, artist));
                }
                widgets::error_row(ui, app, &error, Some(Page::Artist(id.to_string())));
            }
        }
    }
    app.artist_pages.insert(id.to_string(), page);
}

/// A section's title, Spotify's 24-point bold heading.
fn heading(ui: &mut egui::Ui, palette: &Palette, label: &str) -> egui::Response {
    theme::text(ui, label, theme::bold(24.0), palette.text)
}

/// A bold grey text button that brightens under the pointer, as Spotify's
/// "See more" is.
fn text_button(ui: &mut egui::Ui, palette: &Palette, label: &str) -> egui::Response {
    let font = theme::bold(14.0);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), font, palette.secondary);
    let (rect, response) = ui.allocate_exact_size(galley.size() + vec2(0.0, 8.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    if ui.is_rect_visible(rect) {
        let color = if response.hovered() || response.has_focus() {
            palette.text
        } else {
            palette.secondary
        };
        ui.painter().galley(
            rect.left_center() - vec2(0.0, galley.size().y / 2.0),
            galley,
            color,
        );
    }
    theme::focus_ring(ui, &response);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// What the Now Playing panel learnt about this artist from Spotify, when a
/// song of theirs was played: whether Spotify verified them, their
/// biography and a gallery picture.
fn npv_artist(app: &App, uri: &str) -> Option<NpvArtist> {
    app.now_playing_views
        .values()
        .filter_map(|read| read.view.get())
        .filter_map(|view| view.artist.as_ref())
        .find(|artist| artist.uri == uri)
        .cloned()
}

/// The banner's height for a page this wide.
fn banner_height(width: f32) -> f32 {
    (width * 0.3).clamp(BANNER_MIN, BANNER_MAX)
}

/// The largest size the name fits the banner's width at, shrinking from
/// Spotify's 96 points in steps.
fn name_size(width: f32, measure: impl Fn(f32) -> f32) -> f32 {
    let mut size = NAME_MAX;
    while size > NAME_MIN && measure(size) > width {
        size -= 8.0;
    }
    size.max(NAME_MIN)
}

/// The full-width banner: the artist's picture across the page, darkened
/// towards the bottom, with the verified badge, the name and the monthly
/// listeners over it. Clicking it shows the picture enlarged (#586).
/// Returns where it was drawn.
fn artist_banner(
    app: &mut App,
    ui: &mut egui::Ui,
    artist: &Artist,
    preview: Option<&Artist>,
    about: Option<&NpvArtist>,
) -> Rect {
    let palette = app.palette;
    let locale = app.locale;
    let viewport = ui.clip_rect();
    let height = banner_height(ui.available_width());
    let (content, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    // The banner bleeds to the page's edges, over its padding.
    let rect = Rect::from_min_max(
        pos2(viewport.left().min(content.left()), content.top() - 4.0),
        pos2(viewport.right().max(content.right()), content.bottom()),
    );
    let images = hero_images(
        &artist.images,
        preview.map(|artist| artist.images.as_slice()),
        false,
        HeroArt {
            hero: app.art_px(rect.width()),
            card: app.art_px(widgets::CARD_ART),
        },
    );
    let enlarged = images.enlarged.map(str::to_string);
    if let Some(url) = &enlarged {
        let label = gettext(locale, "Enlarge cover");
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label.as_ref())
        });
        if response
            .clone()
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            app.actions.push(Action::ShowDialog(Dialog::CoverArt {
                url: url.clone(),
                shown: images.image.map(str::to_string),
                title: artist.name.clone(),
            }));
        }
    }
    if !ui.is_rect_visible(rect) {
        return rect;
    }
    let art = app.backend.art().clone();
    let softened = images
        .thumbnail
        .and_then(|uri| app.softened_covers.texture(ui.ctx(), &art, uri));
    let pictured = images.image.is_some();
    if pictured {
        widgets::paint_cover_with_thumbnail(
            ui,
            &palette,
            CoverSources {
                requested: images.image,
                previous: images.previous,
                softened: softened.as_ref(),
                thumbnail: images.thumbnail,
                align_thumbnail: false,
            },
            rect,
            0.0,
            Icon::User,
            Some(&art),
        );
        // Spotify's header overlay, darker at the bottom where the name sits.
        widgets::paint_vertical_gradient(
            ui,
            rect,
            Color32::from_black_alpha(20),
            Color32::from_black_alpha(150),
        );
    } else {
        ui.painter().rect_filled(rect, 0.0, palette.surface);
    }
    // White over a picture in every theme, as the darkened picture asks for.
    let (text, subdued) = if pictured {
        (Color32::WHITE, Color32::from_white_alpha(230))
    } else {
        (palette.text, palette.secondary)
    };
    let width = content.width();
    let left = content.left();
    let mut bottom = content.bottom() - 24.0;
    let painter = ui.painter_at(rect);
    let listeners = app
        .play_counts
        .get(&artist.uri)
        .copied()
        .or_else(|| about.and_then(|about| about.monthly_listeners));
    if let Some(line) = artist_byline(locale, artist, listeners).into_iter().next() {
        let galley = widgets::ellipsized(ui, &line.0, theme::regular(16.0), subdued, width, 1);
        bottom -= galley.size().y;
        painter.galley(pos2(left, bottom), galley, subdued);
        bottom -= 8.0;
    }
    let size = name_size(width, |size| {
        ui.painter()
            .layout_no_wrap(artist.name.clone(), theme::bold(size), text)
            .size()
            .x
    });
    let name = widgets::ellipsized(ui, &artist.name, theme::bold(size), text, width, 1);
    bottom -= name.size().y;
    painter.galley(pos2(left, bottom), name, text);
    if about.is_some_and(|about| about.verified) {
        bottom -= 4.0;
        let label = gettext(locale, "Verified artist");
        let galley = widgets::ellipsized(ui, &label, theme::medium(14.0), text, width - 32.0, 1);
        let row = galley.size().y.max(24.0);
        bottom -= row;
        let badge = Rect::from_min_size(pos2(left, bottom), Vec2::splat(row));
        // Spotify's verified badge: a blue disc with a white check, the
        // same in every theme, as the real app draws it.
        ui.painter()
            .circle_filled(badge.center(), row * 0.42, VERIFIED_BLUE);
        theme::paint_icon(ui, Icon::Check, badge, row * 0.5, Color32::WHITE);
        painter.galley(
            pos2(badge.right() + 8.0, bottom + (row - galley.size().y) / 2.0),
            galley,
            text,
        );
    }
    rect
}

/// The artist's colour under the action bar, fading into the page, as
/// Spotify washes it from the banner's colour.
fn action_wash(app: &mut App, ui: &mut egui::Ui, artist: &Artist, banner: Rect) {
    let Some(tint) = artist_tint(app, artist) else {
        return;
    };
    let palette = app.palette;
    let rect = Rect::from_min_size(banner.left_bottom(), vec2(banner.width(), ACTION_WASH));
    widgets::paint_vertical_gradient(
        ui,
        rect,
        super::blend(palette.window, tint, 0.35),
        palette.window,
    );
}

/// The colour Spotifast draws from the artist's picture, when the
/// listener lets artwork colour the interface.
fn artist_tint(app: &mut App, artist: &Artist) -> Option<Color32> {
    if !app.settings.accent_from_art {
        return None;
    }
    let url = pick_image(&artist.images, 64)?.to_string();
    app.tint_for(Some(&url))
}

/// Whether this artist is the context playing now, and playing.
fn playing_here(app: &App, artist: &Artist) -> (bool, bool) {
    let context_here = app.playing_context_uri().as_deref() == Some(artist.uri.as_str());
    (context_here, context_here && app.believed_playing())
}

/// The green play button: it pauses and resumes the artist while they are
/// the context playing, and starts them otherwise.
fn play_button(app: &mut App, ui: &mut egui::Ui, artist: &Artist, diameter: f32) {
    let palette = app.palette;
    let locale = app.locale;
    let (context_here, playing) = playing_here(app, artist);
    if app.play_pending(&artist.uri) {
        theme::circle_spinner(
            ui,
            diameter,
            palette.accent,
            palette.on_accent,
            &gettext(locale, "Starting…"),
        );
    } else if theme::circle_button(
        ui,
        if playing {
            Icon::PauseFilled
        } else {
            Icon::PlayFilled
        },
        diameter,
        palette.accent,
        palette.accent_hover,
        palette.on_accent,
        &if playing {
            gettext(locale, "Pause")
        } else {
            gettext(locale, "Play")
        },
    )
    .clicked()
    {
        if context_here {
            app.actions.push(Action::TogglePlay);
        } else {
            app.actions.push(Action::PlayContext {
                uri: artist.uri.clone(),
                offset_uri: None,
                offset_index: None,
            });
        }
    }
}

/// Play, Shuffle, Follow and More, 24 points apart as in Spotify.
fn artist_actions(app: &mut App, ui: &mut egui::Ui, artist: &Artist) {
    let palette = app.palette;
    let locale = app.locale;
    let following = app.is_saved(&artist.uri).unwrap_or(false);
    ui.add_space(24.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 24.0;
        play_button(app, ui, artist, 56.0);
        let shuffle = app.playing_context_shuffle();
        if theme::icon_button(
            ui,
            Icon::Shuffle,
            28.0,
            if shuffle {
                palette.accent
            } else {
                palette.secondary
            },
            // Keep shuffle green under the pointer, or turning it on looks
            // like nothing happened until the pointer leaves.
            if shuffle {
                palette.accent_hover
            } else {
                palette.text
            },
            &if shuffle {
                gettext(locale, "Shuffle off")
            } else {
                gettext(locale, "Shuffle")
            },
        )
        .clicked()
        {
            app.actions.push(Action::SetShuffle(!shuffle));
        }
        if theme::pill_button(
            ui,
            &palette,
            &if following {
                pgettext(locale, "artist", "Following")
            } else {
                pgettext(locale, "artist", "Follow")
            },
            false,
        )
        .clicked()
        {
            app.actions.push(Action::ToggleSaved(artist.uri.clone()));
        }
        let more = theme::icon_button(
            ui,
            Icon::Ellipsis,
            28.0,
            palette.secondary,
            palette.text,
            &gettext(locale, "More"),
        );
        egui::Popup::menu(&more)
            .frame(widgets::menu_frame(&palette))
            .show(|ui| widgets::context_menu_items(ui, app, &artist.uri, &artist.name, None));
    });
    ui.add_space(24.0);
}

/// Popular: five numbered songs with their play counts, ten after "See more".
fn popular(
    app: &mut App,
    ui: &mut egui::Ui,
    id: &str,
    artist: &Artist,
    page: &crate::model::ArtistPage,
) {
    let palette = app.palette;
    let locale = app.locale;
    heading(ui, &palette, &gettext(locale, "Popular"));
    ui.add_space(12.0);
    match &page.top_tracks {
        Loadable::Loaded(tracks) if !tracks.is_empty() => {
            let uris: Arc<[String]> = tracks
                .iter()
                .map(|track| track.uri.clone())
                .collect::<Vec<_>>()
                .into();
            let context = RowContext::Uris(Arc::clone(&uris));
            let items: Vec<PlayableItem> =
                tracks.iter().cloned().map(PlayableItem::Track).collect();
            let limit = if page.show_all_top {
                items.len()
            } else {
                POPULAR_SHOWN
            };
            let show_plays = app.has_play_counts(&artist.uri);
            let render_top = |ui: &mut egui::Ui, app: &mut App| {
                for (index, item) in items.iter().take(limit).enumerate() {
                    widgets::track_row(
                        ui,
                        app,
                        TrackRow {
                            index,
                            number: Some(index + 1),
                            item,
                            context: &context,
                            show_cover: !app.settings.tracklist_compact,
                            show_album: false,
                            added_at: None,
                            added_by: None,
                            show_added_by: false,
                            show_plays,
                            compact: false,
                            thin: app.settings.tracklist_compact,
                            shift: 0.0,
                            picked: false,
                            picked_songs: &[],
                            picked_positions: &[],
                            position: None,
                        },
                    );
                }
            };
            if !app.network_reachable {
                ui.add_enabled_ui(false, |ui| {
                    ui.multiply_opacity(0.55);
                    render_top(ui, app);
                });
            } else {
                render_top(ui, app);
            }
            if items.len() > POPULAR_SHOWN {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    let label = if page.show_all_top {
                        gettext(locale, "Show less")
                    } else {
                        gettext(locale, "See more")
                    };
                    if text_button(ui, &palette, &label).clicked() {
                        app.actions.push(Action::ToggleShowAllTop(id.to_string()));
                    }
                });
            }
        }
        Loadable::Loaded(_) => {
            theme::subtle(ui, &palette, &gettext(locale, "No popular songs to show."));
        }
        Loadable::Loading | Loadable::NotLoaded => {
            widgets::skeleton_rows(ui, &palette, app.locale, POPULAR_SHOWN)
        }
        Loadable::Failed(error) => {
            let error = error.clone();
            widgets::error_row(ui, app, &error, None);
        }
    }
}

/// An album's card, playing or pausing it from its button and opening it
/// otherwise, with the album's menu on a right click.
fn album_card(app: &mut App, ui: &mut egui::Ui, album: &Album, menu: &str, latest: bool) {
    let palette = app.palette;
    let when = if latest {
        gettext(app.locale, "Latest release").into_owned()
    } else {
        album.year().unwrap_or("").to_string()
    };
    let subtitle = format!("{when} • {}", app.album_kind_label(album));
    let context_here = app.playing_context_uri().as_deref() == Some(album.uri.as_str());
    let playing_here = context_here && app.believed_playing();
    let card_art = app.art_px(widgets::CARD_ART);
    let card = widgets::card(
        ui,
        app,
        pick_image(&album.images, card_art),
        &album.name,
        subtitle.trim_start_matches(" • "),
        widgets::CardCover::square(playing_here),
    );
    if card.play {
        if context_here {
            app.actions.push(Action::TogglePlay);
        } else {
            app.actions.push(Action::PlayContext {
                uri: album.uri.clone(),
                offset_uri: None,
                offset_index: None,
            });
        }
    }
    if card.clicked {
        app.actions
            .push(Action::Open(Page::Album(album.id.clone())));
    }
    egui::Popup::context_menu(&card.response)
        .id(ui.make_persistent_id((menu, &album.uri)))
        .frame(widgets::menu_frame(&palette))
        .show(|ui| widgets::context_menu_items(ui, app, &album.uri, &album.name, None));
}

/// Discography: the release kinds as chips over their cards.
fn discography(app: &mut App, ui: &mut egui::Ui, id: &str, page: &crate::model::ArtistPage) {
    let palette = app.palette;
    let locale = app.locale;
    heading(ui, &palette, &gettext(locale, "Discography"));
    ui.add_space(12.0);
    let chosen = Chip::of(page.filter);
    let compilations = page.filter == DiscographyFilter::Compilations
        || page
            .albums
            .get(DiscographyFilter::All.groups())
            .is_some_and(|list| list.items.iter().any(is_compilation));
    let labels: Vec<_> = Chip::ALL
        .iter()
        .filter(|chip| compilations || **chip != Chip::Compilations)
        .map(|chip| (*chip, chip.label(locale)))
        .collect();
    let options: Vec<(Chip, &str)> = labels
        .iter()
        .map(|(chip, label)| (*chip, label.as_ref()))
        .collect();
    if let Some(chip) = widgets::chips(ui, &palette, &options, chosen) {
        app.actions.push(Action::SetDiscographyFilter {
            artist_id: id.to_string(),
            filter: chip.filter(),
        });
    }
    if chosen == Chip::Popular {
        ui.add_space(12.0);
        popular_releases_grid(app, ui, page);
        return;
    }
    ui.add_space(12.0);
    // Reserve the previous list's height while a new category loads.
    // Its cards disappear immediately so they always match the chip.
    let height_id = ui.make_persistent_id(("discography-height", id));
    let list = page.albums.get(page.filter.groups());
    let loading = list.is_none_or(|list| !list.loaded_once && list.error.is_none());
    let previous = ui
        .ctx()
        .data(|data| data.get_temp::<(f32, usize)>(height_id));
    let shown = ui.scope(|ui| {
        if loading {
            ui.set_min_height(previous.map_or(0.0, |(height, _)| height));
        }
        match list {
            Some(list) if !loading => {
                let mut seen = std::collections::HashSet::new();
                let albums: Vec<_> = list
                    .items
                    .iter()
                    .filter(|album| seen.insert(album.name.to_lowercase()))
                    .collect();
                widgets::grid(ui, |ui| {
                    for album in &albums {
                        album_card(app, ui, album, "discography-menu", false);
                    }
                });
                if list.loading {
                    widgets::loading_row(ui, &palette, app.locale);
                } else if let Some(error) = &list.error {
                    let error = error.clone();
                    widgets::error_row(ui, app, &error, None);
                } else if list.items.is_empty() {
                    theme::subtle(ui, &palette, &gettext(locale, "Nothing in this category."));
                } else if list.can_load_more() {
                    ui.add_space(8.0);
                    if theme::soft_button(ui, &palette, None, &gettext(locale, "Load more"), false)
                        .clicked()
                    {
                        app.actions
                            .push(Action::LoadMoreArtistAlbums(id.to_string()));
                    }
                }
                albums.len()
            }
            _ => {
                let count = previous.map_or(6, |(_, count)| count.max(1));
                widgets::grid(ui, |ui| {
                    widgets::skeleton_cards_filling(
                        ui,
                        &palette,
                        app.locale,
                        count,
                        false,
                        previous.map_or(0.0, |(height, _)| height),
                    )
                });
                count
            }
        }
    });
    if !loading {
        ui.ctx().data_mut(|data| {
            data.insert_temp(height_id, (shown.response.rect.height(), shown.inner));
        });
    }
}

/// The Discography chips, as Spotify shows them: the popular releases
/// first, then each kind of release. Appears On has its own shelf.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Chip {
    Popular,
    Albums,
    Singles,
    Compilations,
}

impl Chip {
    const ALL: [Chip; 4] = [
        Chip::Popular,
        Chip::Albums,
        Chip::Singles,
        Chip::Compilations,
    ];

    /// The chip showing a filter. Popular releases are drawn from every
    /// release, so they show the `All` filter, and Appears On, now a shelf
    /// of its own, falls back to them.
    fn of(filter: DiscographyFilter) -> Self {
        match filter {
            DiscographyFilter::Albums => Chip::Albums,
            DiscographyFilter::Singles => Chip::Singles,
            DiscographyFilter::Compilations => Chip::Compilations,
            DiscographyFilter::All | DiscographyFilter::AppearsOn => Chip::Popular,
        }
    }

    fn filter(self) -> DiscographyFilter {
        match self {
            Chip::Popular => DiscographyFilter::All,
            Chip::Albums => DiscographyFilter::Albums,
            Chip::Singles => DiscographyFilter::Singles,
            Chip::Compilations => DiscographyFilter::Compilations,
        }
    }

    fn label(self, locale: Locale) -> std::borrow::Cow<'static, str> {
        match self {
            Chip::Popular => gettext(locale, "Popular releases"),
            other => other.filter().label(locale),
        }
    }
}

fn is_compilation(album: &Album) -> bool {
    album.album_group.as_deref().or(album.album_type.as_deref()) == Some("compilation")
}

/// Spotify's popular releases: the newest release, then the releases that
/// hold the artist's popular songs, in the order of those songs. Each
/// release shows once. The flag marks the newest.
fn popular_releases<'a>(top: &'a [Track], all: &'a [Album]) -> Vec<(&'a Album, bool)> {
    let latest = all
        .iter()
        .filter(|album| album.release_date.is_some())
        .max_by(|a, b| a.release_date.cmp(&b.release_date));
    let mut seen = std::collections::HashSet::new();
    let mut releases = Vec::new();
    if let Some(latest) = latest {
        seen.insert(latest.name.to_lowercase());
        releases.push((latest, true));
    }
    for album in top.iter().filter_map(|track| track.album.as_ref()) {
        if !album.id.is_empty() && seen.insert(album.name.to_lowercase()) {
            releases.push((album, false));
        }
    }
    releases
}

/// The Popular releases chip's cards, or placeholders until the popular
/// songs they come from are in.
fn popular_releases_grid(app: &mut App, ui: &mut egui::Ui, page: &crate::model::ArtistPage) {
    let palette = app.palette;
    let all = page
        .albums
        .get(DiscographyFilter::All.groups())
        .map(|list| list.items.as_slice())
        .unwrap_or_default();
    match &page.top_tracks {
        Loadable::Loaded(top) => {
            let releases = popular_releases(top, all);
            if releases.is_empty() {
                theme::subtle(
                    ui,
                    &palette,
                    &gettext(app.locale, "Nothing in this category."),
                );
                return;
            }
            widgets::grid(ui, |ui| {
                for (album, latest) in releases {
                    album_card(app, ui, album, "discography-menu", latest);
                }
            });
        }
        Loadable::Failed(error) => {
            let error = error.clone();
            widgets::error_row(ui, app, &error, None);
        }
        Loadable::Loading | Loadable::NotLoaded => {
            widgets::grid(ui, |ui| {
                widgets::skeleton_cards(ui, &palette, app.locale, 6, false)
            });
        }
    }
}

/// Asks once for the releases the artist appears on, for their shelf.
/// Loading a release kind goes through choosing its filter, so the page
/// chooses Appears On and, right after, the filter it shows, which loads
/// nothing new.
fn request_appears_on(app: &mut App, ui: &egui::Ui, id: &str, page: &crate::model::ArtistPage) {
    let asked = egui::Id::new(("artist-appears-on-asked", id));
    if !app.network_reachable
        || page
            .albums
            .contains_key(DiscographyFilter::AppearsOn.groups())
        || ui.data(|data| data.get_temp::<bool>(asked).unwrap_or(false))
    {
        return;
    }
    ui.data_mut(|data| data.insert_temp(asked, true));
    for filter in [DiscographyFilter::AppearsOn, page.filter] {
        app.actions.push(Action::SetDiscographyFilter {
            artist_id: id.to_string(),
            filter,
        });
    }
}

/// A row of cards under a heading, scrolling sideways when it overflows.
fn shelf(
    ui: &mut egui::Ui,
    palette: &Palette,
    id: &str,
    title: &str,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    heading(ui, palette, title);
    ui.add_space(8.0);
    crate::autoscroll::show(
        ui,
        egui::ScrollArea::horizontal().id_salt(("artist-shelf", id)),
        egui::Vec2b::new(true, false),
        |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = widgets::CARD_GAP / 2.0;
                add_contents(ui);
            });
        },
    );
}

/// Fans also like: the related artists' round portraits.
fn fans_also_like(app: &mut App, ui: &mut egui::Ui, related: &[Artist]) {
    let palette = app.palette;
    let locale = app.locale;
    let artist_label = gettext(locale, "Artist");
    let title = gettext(locale, "Fans also like");
    shelf(ui, &palette, "related", &title, |ui| {
        for artist in related {
            let (context_here, playing) = playing_here(app, artist);
            let card_art = app.art_px(widgets::CARD_ART);
            let card = widgets::card(
                ui,
                app,
                pick_image(&artist.images, card_art),
                &artist.name,
                &artist_label,
                widgets::CardCover::portrait(playing),
            );
            if card.play {
                if context_here {
                    app.actions.push(Action::TogglePlay);
                } else {
                    app.actions.push(Action::PlayContext {
                        uri: artist.uri.clone(),
                        offset_uri: None,
                        offset_index: None,
                    });
                }
            }
            if card.clicked {
                app.actions
                    .push(Action::Open(Page::Artist(artist.id.clone())));
            }
            egui::Popup::context_menu(&card.response)
                .id(ui.make_persistent_id(("related-artist-menu", &artist.uri)))
                .frame(widgets::menu_frame(&palette))
                .show(|ui| widgets::context_menu_items(ui, app, &artist.uri, &artist.name, None));
        }
    });
}

/// The About card: the artist's picture filling a rounded card, with the
/// monthly listeners and the start of the biography over its bottom.
/// Clicking it shows the picture whole.
fn about_card(app: &mut App, ui: &mut egui::Ui, artist: &Artist, about: Option<&NpvArtist>) {
    let palette = app.palette;
    let locale = app.locale;
    let title = gettext(locale, "About");
    heading(ui, &palette, &title);
    ui.add_space(16.0);
    let image = about
        .and_then(|about| about.gallery_image.clone())
        .or_else(|| pick_image(&artist.images, u32::MAX).map(str::to_string));
    let width = ui.available_width().min(ABOUT_WIDTH);
    let height = if image.is_some() {
        (width * 0.7).clamp(320.0, ABOUT_HEIGHT)
    } else {
        200.0
    };
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            format!("{title}: {}", artist.name),
        )
    });
    if response.gained_focus() {
        response.scroll_to_me(None);
    }
    let hovered = response.hovered();
    if ui.is_rect_visible(rect) {
        // Spotify grows the card a hundredth under the pointer.
        let card = if hovered {
            rect.expand2(rect.size() * 0.005)
        } else {
            rect
        };
        let radius = theme::RADIUS as f32;
        let pictured = image.is_some();
        if pictured {
            widgets::paint_cover(
                ui,
                &palette,
                image.as_deref(),
                card,
                radius,
                Icon::User,
                Some(app.backend.art()),
            );
            let fade = Rect::from_min_max(pos2(card.left(), card.center().y), card.max);
            ui.painter()
                .add(egui::Shape::Rect(egui::epaint::RectShape::filled(
                    fade,
                    egui::CornerRadius {
                        nw: 0,
                        ne: 0,
                        sw: theme::RADIUS,
                        se: theme::RADIUS,
                    },
                    Color32::TRANSPARENT,
                )));
            ui.painter().add(widgets::vertical_gradient(
                fade,
                Color32::TRANSPARENT,
                Color32::from_black_alpha(190),
            ));
        } else {
            ui.painter().rect_filled(
                card,
                radius,
                if hovered {
                    palette.surface_hover
                } else {
                    palette.surface
                },
            );
        }
        let (text, subdued) = if pictured {
            (Color32::WHITE, Color32::from_white_alpha(220))
        } else {
            (palette.text, palette.secondary)
        };
        let left = card.left() + ABOUT_PADDING.min(card.width() / 8.0);
        let room = (card.width() - 2.0 * (left - card.left())).max(1.0);
        let biography = about
            .map(|about| about.biography.trim())
            .filter(|biography| !biography.is_empty());
        let listeners = app
            .play_counts
            .get(&artist.uri)
            .copied()
            .or_else(|| about.and_then(|about| about.monthly_listeners));
        let byline = artist_byline(locale, artist, listeners);
        let mut lines = Vec::new();
        if let Some((first, _)) = byline.first() {
            lines.push(widgets::ellipsized(
                ui,
                first,
                theme::bold(16.0),
                text,
                room,
                1,
            ));
        }
        match biography {
            Some(biography) => lines.push(widgets::ellipsized(
                ui,
                biography,
                theme::regular(15.0),
                subdued,
                room,
                3,
            )),
            None => {
                let rest = byline
                    .iter()
                    .skip(1)
                    .map(|(text, _)| text.as_str())
                    .collect::<Vec<_>>()
                    .join(" • ");
                if !rest.is_empty() {
                    lines.push(widgets::ellipsized(
                        ui,
                        &rest,
                        theme::regular(15.0),
                        subdued,
                        room,
                        2,
                    ));
                }
            }
        }
        let mut bottom = card.bottom() - ABOUT_PADDING.min(card.height() / 8.0);
        for galley in lines.into_iter().rev() {
            bottom -= galley.size().y;
            let x = if galley.job.halign == Align::RIGHT {
                left + room
            } else {
                left
            };
            ui.painter().galley(pos2(x, bottom), galley, text);
            bottom -= 8.0;
        }
    }
    theme::focus_ring(ui, &response);
    if response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
        && let Some(url) = image
    {
        app.actions.push(Action::ShowDialog(Dialog::CoverArt {
            url,
            shown: None,
            title: artist.name.clone(),
        }));
    }
}

/// The header that stays at the top of the page once the banner has
/// scrolled away: the artist's colour behind the play button and the
/// name, fading in as the banner's bottom passes under it.
fn sticky_header(app: &mut App, ui: &mut egui::Ui, artist: &Artist, banner: Rect) {
    let viewport = ui.clip_rect();
    let shown = sticky_opacity(viewport.top(), banner.bottom());
    if shown <= 0.0 {
        return;
    }
    let palette = app.palette;
    let fill = artist_tint(app, artist).map_or(palette.surface, |tint| {
        super::blend(palette.window, tint, 0.45)
    });
    let bar = Rect::from_min_size(viewport.min, vec2(viewport.width(), STICKY_HEIGHT));
    // Registered after the page, so the bar, not the rows under it, takes
    // the pointer.
    let _ = ui.interact(bar, ui.id().with("artist-sticky"), Sense::click());
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(bar.shrink2(vec2(widgets::PAGE_PADDING, 0.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    child.set_clip_rect(bar.intersect(viewport));
    child.multiply_opacity(shown);
    child.painter().rect_filled(bar, 0.0, fill);
    child.spacing_mut().item_spacing.x = 16.0;
    play_button(app, &mut child, artist, 48.0);
    let room = child.available_width().max(1.0);
    let name = widgets::ellipsized(
        &child,
        &artist.name,
        theme::bold(24.0),
        palette.text,
        room,
        1,
    );
    child.add(egui::Label::new(name).selectable(false));
}

/// How far the sticky header has faded in, from 0 while the banner's
/// bottom is still well below the top of the page to 1 once it has gone
/// under the header.
fn sticky_opacity(viewport_top: f32, banner_bottom: f32) -> f32 {
    ((viewport_top + 2.0 * STICKY_HEIGHT - banner_bottom) / STICKY_HEIGHT).clamp(0.0, 1.0)
}

/// How many concerts show before "Show all".
const CONCERTS_SHOWN: usize = 5;

/// The day and abbreviated month of a concert's ISO-8601 date.
fn concert_day(locale: Locale, iso: &str) -> Option<(String, String)> {
    let mut parts = iso.get(..10)?.split('-');
    let (_, _, day) = (parts.next()?, parts.next()?, parts.next()?);
    let day: u8 = day.parse().ok()?;
    let month = util::format_date(locale, &iso[..7]);
    // format_date gives "Mon YYYY"; the month is the part without digits.
    let month = month
        .split_whitespace()
        .find(|word| !word.chars().any(|c| c.is_ascii_digit()))
        .unwrap_or_default()
        .to_uppercase();
    Some((month, day.to_string()))
}

/// The "On tour" section: one row per concert, the first few unless the
/// viewer expands the list. Draws nothing for an empty list.
fn concerts_section(app: &mut App, ui: &mut egui::Ui, artist_id: &str, concerts: &[Concert]) {
    if concerts.is_empty() {
        return;
    }
    let palette = app.palette;
    let locale = app.locale;
    heading(ui, &palette, &gettext(locale, "On tour"));
    ui.add_space(12.0);
    let expanded_id = egui::Id::new(("artist-concerts-expanded", artist_id));
    let expanded = ui
        .data(|data| data.get_temp::<bool>(expanded_id))
        .unwrap_or(false);
    let shown = if expanded {
        concerts.len()
    } else {
        CONCERTS_SHOWN
    };
    let tickets = gettext(locale, "Tickets");
    for concert in concerts.iter().take(shown) {
        concert_row(app, ui, concert, &tickets);
    }
    if concerts.len() > CONCERTS_SHOWN {
        ui.add_space(8.0);
        let label = if expanded {
            gettext(locale, "Show less")
        } else {
            gettext(locale, "Show all")
        };
        if text_button(ui, &palette, &label).clicked() {
            ui.data_mut(|data| data.insert_temp(expanded_id, !expanded));
        }
    }
}

/// One concert as Spotify lists it: a date tile, the city over the venue
/// and the date, and the Tickets button. The whole row opens the concert.
fn concert_row(app: &mut App, ui: &mut egui::Ui, concert: &Concert, tickets: &str) {
    let palette = app.palette;
    let locale = app.locale;
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 72.0), Sense::click());
    let place = [concert.city.as_str(), concert.venue.as_str()]
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" \u{b7} ");
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Link, ui.is_enabled(), &place));
    let hovered = response.hovered();
    let mut open = response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked();
    if hovered {
        ui.painter()
            .rect_filled(rect, theme::RADIUS_SMALL as f32, palette.surface_hover);
    }
    let tile = Rect::from_min_size(rect.min + vec2(8.0, 8.0), Vec2::splat(56.0));
    ui.painter()
        .rect_filled(tile, theme::RADIUS_SMALL as f32, palette.surface_active);
    if let Some((month, day)) = concert_day(locale, &concert.date) {
        let month = ui
            .painter()
            .layout_no_wrap(month, theme::bold(12.0), palette.text);
        let day = ui
            .painter()
            .layout_no_wrap(day, theme::bold(22.0), palette.text);
        let total = month.size().y + day.size().y;
        let top = tile.center().y - total / 2.0;
        ui.painter().galley(
            pos2(tile.center().x - month.size().x / 2.0, top),
            month,
            palette.text,
        );
        ui.painter().galley(
            pos2(
                tile.center().x - day.size().x / 2.0,
                top + total - day.size().y,
            ),
            day,
            palette.text,
        );
    }
    let button_width = theme::soft_button_width(ui, tickets);
    let button = Rect::from_center_size(
        pos2(rect.right() - 8.0 - button_width / 2.0, rect.center().y),
        vec2(button_width, 32.0),
    );
    let left = tile.right() + 12.0;
    let room = (button.left() - 12.0 - left).max(1.0);
    let title = if concert.city.is_empty() {
        concert.venue.as_str()
    } else {
        concert.city.as_str()
    };
    let date = util::format_date(locale, &concert.date);
    let detail = if concert.city.is_empty() || concert.venue.is_empty() {
        date
    } else {
        format!("{date} \u{b7} {}", concert.venue)
    };
    let title = widgets::ellipsized(ui, title, theme::semibold(16.0), palette.text, room, 1);
    let detail = widgets::ellipsized(
        ui,
        &detail,
        theme::regular(14.0),
        palette.secondary,
        room,
        1,
    );
    let total = title.size().y + 4.0 + detail.size().y;
    let top = rect.center().y - total / 2.0;
    let x = |galley: &egui::Galley| {
        if galley.job.halign == Align::RIGHT {
            left + room
        } else {
            left
        }
    };
    let detail_top = top + title.size().y + 4.0;
    ui.painter()
        .galley(pos2(x(&title), top), title, palette.text);
    ui.painter()
        .galley(pos2(x(&detail), detail_top), detail, palette.secondary);
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(button)
            .layout(Layout::left_to_right(Align::Center)),
    );
    if theme::soft_button(&mut child, &palette, None, tickets, false)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        open = true;
    }
    if open {
        app.actions.push(Action::OpenUrl(concert.url()));
    }
}

fn upcoming_release(
    app: &mut App,
    ui: &mut egui::Ui,
    release: &crate::prerelease::UpcomingRelease,
) {
    let palette = app.palette;
    let heading_text = gettext(app.locale, "Release countdown");
    heading(ui, &palette, &heading_text);
    ui.add_space(12.0);
    let album_label = gettext(app.locale, "Album");
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().min(440.0), 96.0),
        egui::Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            format!("{heading_text}: {}", release.name),
        )
    });
    if response.gained_focus() {
        response.scroll_to_me(None);
    }
    ui.painter().rect_filled(
        rect,
        theme::RADIUS as f32,
        if response.hovered() || response.has_focus() {
            palette.surface_hover
        } else {
            egui::Color32::TRANSPARENT
        },
    );
    let cover = egui::Rect::from_min_size(rect.min + egui::vec2(0.0, 4.0), egui::Vec2::splat(88.0));
    widgets::paint_cover(
        ui,
        &palette,
        pick_image(&release.images, app.art_px(88.0)),
        cover,
        4.0,
        Icon::Music,
        Some(app.backend.art()),
    );
    let left = cover.right() + 12.0;
    let right = rect.right() - 12.0;
    for (text, y, font, color) in [
        (
            release.name.as_str(),
            28.0,
            theme::semibold(16.0),
            palette.text,
        ),
        (
            album_label.as_ref(),
            52.0,
            theme::regular(13.0),
            palette.secondary,
        ),
    ] {
        let galley = widgets::ellipsized(ui, text, font, color, (right - left).max(1.0), 1);
        let x = if galley.job.halign == egui::Align::RIGHT {
            right
        } else {
            left
        };
        ui.painter()
            .galley(egui::pos2(x, rect.top() + y), galley, color);
    }
    if response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(&release.name)
        .clicked()
    {
        app.actions.push(Action::OpenLink(release.uri.clone()));
    }
}

/// Monthly listeners, which Spotify's playback session reads with the
/// popular songs' play counts, then followers and genres.
fn artist_byline(
    locale: Locale,
    artist: &Artist,
    monthly_listeners: Option<u64>,
) -> Vec<(String, Option<Page>)> {
    let mut byline = Vec::new();
    if let Some(listeners) = monthly_listeners {
        byline.push((
            ngettext(
                locale,
                // Translators: {count} is the number of people who listened
                // to an artist in the last 28 days, as Spotify counts them.
                "{count} monthly listener",
                "{count} monthly listeners",
                u32::try_from(listeners).unwrap_or(u32::MAX),
            )
            .replace("{count}", &util::format_count(listeners)),
            None,
        ));
    }
    if let Some(followers) = &artist.followers {
        byline.push((
            ngettext(
                locale,
                // Translators: {count} is the number of people who follow an artist.
                "{count} follower",
                "{count} followers",
                u32::try_from(followers.total).unwrap_or(u32::MAX),
            )
            .replace("{count}", &util::format_count(followers.total)),
            None,
        ));
    }
    if !artist.genres.is_empty() {
        byline.push((
            artist
                .genres
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            None,
        ));
    }
    byline
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::Followers;

    #[test]
    fn the_byline_leads_with_monthly_listeners_once_spotify_gives_them() {
        let artist = Artist {
            followers: Some(Followers { total: 1_200 }),
            genres: vec!["indie".into()],
            ..Artist::default()
        };
        let texts = |listeners| {
            artist_byline(Locale::English, &artist, listeners)
                .into_iter()
                .map(|(text, _)| text)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            texts(Some(8_123_456)),
            ["8,123,456 monthly listeners", "1,200 followers", "indie"]
        );
        assert_eq!(texts(None), ["1,200 followers", "indie"]);
    }

    #[test]
    fn the_artist_banner_grows_with_the_page_between_spotify_bounds() {
        assert_eq!(banner_height(600.0), BANNER_MIN);
        assert_eq!(banner_height(1_100.0), 330.0);
        assert_eq!(banner_height(3_000.0), BANNER_MAX);
    }

    #[test]
    fn the_artist_name_shrinks_until_it_fits_the_banner() {
        // A name ten points wide per point of type.
        let measure = |size: f32| size * 10.0;
        assert_eq!(name_size(2_000.0, measure), NAME_MAX);
        assert_eq!(name_size(700.0, measure), 64.0);
        assert_eq!(name_size(10.0, measure), NAME_MIN);
    }

    #[test]
    fn the_artist_sticky_header_fades_in_as_the_banner_leaves() {
        // The banner's bottom well below the top: no header.
        assert_eq!(sticky_opacity(0.0, 400.0), 0.0);
        assert_eq!(sticky_opacity(0.0, 2.0 * STICKY_HEIGHT), 0.0);
        assert_eq!(sticky_opacity(0.0, 1.5 * STICKY_HEIGHT), 0.5);
        // Once the banner is under the header, fully shown.
        assert_eq!(sticky_opacity(0.0, STICKY_HEIGHT), 1.0);
        assert_eq!(sticky_opacity(100.0, 0.0), 1.0);
    }

    #[test]
    fn an_artist_concert_tile_shows_its_month_and_day() {
        assert_eq!(
            concert_day(Locale::English, "2027-02-05T20:00:00Z"),
            Some(("FEB".to_string(), "5".to_string()))
        );
        assert_eq!(
            concert_day(Locale::English, "2027-11-24"),
            Some(("NOV".to_string(), "24".to_string()))
        );
        assert_eq!(concert_day(Locale::English, "soon"), None);
    }

    fn release(id: &str, date: &str) -> Album {
        Album {
            id: id.into(),
            name: id.into(),
            release_date: Some(date.into()),
            ..Album::default()
        }
    }

    #[test]
    fn popular_releases_lead_with_the_newest_then_the_popular_songs_releases() {
        let all = [release("old", "2015-01-01"), release("new", "2026-03-01")];
        let top: Vec<Track> = ["hit", "new", "hit", "deep"]
            .iter()
            .map(|id| Track {
                album: Some(release(id, "2018-01-01")),
                ..Track::default()
            })
            .collect();
        let shown: Vec<_> = popular_releases(&top, &all)
            .into_iter()
            .map(|(album, latest)| (album.id.as_str(), latest))
            .collect();
        assert_eq!(
            shown,
            [("new", true), ("hit", false), ("deep", false)],
            "the newest first, then each popular song's release once"
        );
        assert!(popular_releases(&[], &[]).is_empty());
    }

    #[test]
    fn the_artist_discography_chips_follow_spotify() {
        assert_eq!(Chip::of(DiscographyFilter::All), Chip::Popular);
        // Appears On has its own shelf; its old filter shows Popular releases.
        assert_eq!(Chip::of(DiscographyFilter::AppearsOn), Chip::Popular);
        for chip in Chip::ALL {
            assert_eq!(Chip::of(chip.filter()), chip);
        }
        assert_eq!(Chip::Popular.label(Locale::English), "Popular releases");
        assert_eq!(Chip::Singles.label(Locale::English), "Singles & EPs");
    }
}
