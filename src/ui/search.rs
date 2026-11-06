//! The Search page.

use std::sync::Arc;

use egui::{Align, CornerRadius, Layout, Pos2, Rect, Sense, Vec2, pos2, vec2};

use crate::api::models::{Artist, ArtistRef, PlayableItem, SearchResults, pick_image};
use crate::app::App;
use crate::i18n::gettext;
use crate::model::{
    Action, BrowseCategory, BrowseItem, BrowseSection, Loadable, Page, RowContext, SearchFilter,
};
use crate::theme::{self, Icon};

use super::widgets::{self, TrackRow};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    if app.search.committed.is_empty() && app.search.typed_at.is_none() {
        recent(app, ui);
        return;
    }
    ui.add_space(4.0);
    let labels: Vec<_> = SearchFilter::ALL
        .iter()
        .map(|f| (*f, f.label(app.locale)))
        .collect();
    let options: Vec<(SearchFilter, &str)> = labels
        .iter()
        .map(|(filter, label)| (*filter, label.as_ref()))
        .collect();
    if let Some(filter) = widgets::chips(ui, &palette, &options, app.search.filter) {
        app.actions.push(Action::SetSearchFilter(filter));
    }
    ui.add_space(12.0);
    let pending = app.search.catalogue_pending || app.search.playlists_pending;
    if pending {
        widgets::loading_row(ui, &palette, app.locale);
    }
    if let Some(error) = app.search.error.clone() {
        widgets::error_row(ui, app, &error, None);
    }
    let results = match &app.search.results {
        Loadable::Loaded(results) => results.clone(),
        Loadable::Loading | Loadable::NotLoaded => {
            if !pending {
                widgets::skeleton_rows(ui, &palette, app.locale, 6);
            }
            return;
        }
        Loadable::Failed(error) => {
            let error = error.clone();
            if app.search.error.is_none() {
                widgets::error_row(ui, app, &error, None);
            }
            return;
        }
    };
    if results.is_empty() && (pending || app.search.error.is_some()) {
        return;
    }
    if results.is_empty() {
        widgets::empty_state(
            ui,
            &palette,
            Icon::Search,
            // Translators: {query} is the text the user searched for.
            &gettext(app.locale, "No results for “{query}”")
                .replace("{query}", &app.search.committed),
            &gettext(app.locale, "Check the spelling, or try fewer words."),
        );
        return;
    }
    match app.search.filter {
        SearchFilter::All => all(app, ui, &results),
        SearchFilter::Songs => songs(app, ui, &results, usize::MAX),
        SearchFilter::Artists => artists_grid(app, ui, &results),
        SearchFilter::Albums => albums_grid(app, ui, &results),
        SearchFilter::Playlists => playlists_grid(app, ui, &results),
        SearchFilter::Podcasts => shows_grid(app, ui, &results),
        SearchFilter::Episodes => episodes(app, ui, &results, usize::MAX),
    }
}

fn recent(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    ui.add_space(6.0);
    if !app.network_reachable && app.settings.search_history.is_empty() {
        widgets::empty_state(
            ui,
            &palette,
            Icon::WifiOff,
            &gettext(app.locale, "You're offline"),
            &gettext(
                app.locale,
                "Connect to the internet to search songs, artists, and playlists.",
            ),
        );
        return;
    }
    if !app.settings.search_history.is_empty() {
        theme::section_title(ui, &palette, &gettext(app.locale, "Recent searches"));
        ui.add_space(6.0);
        let history = app.settings.search_history.clone();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            for query in &history {
                let (response, forget) =
                    theme::soft_button_dismiss(ui, &palette, Icon::Clock, query);
                if forget {
                    app.actions.push(Action::ForgetSearch(query.clone()));
                } else if response.clicked() {
                    app.actions.push(Action::Search(query.clone()));
                }
            }
        });
        ui.add_space(16.0);
    }
    browse_all(app, ui);
}

/// Spotify's Browse all grid, under the recent searches, as Spotify's own
/// Search page shows it while nothing is searched.
fn browse_all(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let locale = app.locale;
    match &app.search.browse {
        Loadable::Loaded(sections) => {
            let sections: Vec<BrowseSection> = sections
                .iter()
                .filter(|section| section.is_grid())
                .cloned()
                .collect();
            for section in &sections {
                let title = if section.title.is_empty() {
                    gettext(locale, "Browse all").into_owned()
                } else {
                    section.title.clone()
                };
                theme::section_title(ui, &palette, &title);
                ui.add_space(4.0);
                tile_grid(app, ui, &section.items);
                ui.add_space(16.0);
            }
        }
        Loadable::Failed(error) => {
            let error = error.clone();
            if app.settings.search_history.is_empty() {
                search_spotify(app, ui);
                ui.add_space(16.0);
            }
            widgets::error_row(ui, app, &error, Some(Page::Search));
        }
        Loadable::Loading | Loadable::NotLoaded if app.network_reachable => {
            theme::section_title(ui, &palette, &gettext(locale, "Browse all"));
            ui.add_space(4.0);
            skeleton_tiles(ui, &palette, locale, SKELETON_TILES);
        }
        Loadable::Loading | Loadable::NotLoaded => {
            if app.settings.search_history.is_empty() {
                search_spotify(app, ui);
            }
        }
    }
}

/// The page's empty state when it has no grid to show.
fn search_spotify(app: &App, ui: &mut egui::Ui) {
    widgets::empty_state(
        ui,
        &app.palette,
        Icon::Search,
        &gettext(app.locale, "Search Spotify"),
        &gettext(
            app.locale,
            "Find songs, artists, albums, playlists, and podcasts.",
        ),
    );
}

/// The narrowest a tile gets before the grid drops a column, and the room
/// between tiles: Spotify pads each tile by 12 points.
const TILE_MIN_WIDTH: f32 = 180.0;
const TILE_GAP: f32 = 24.0;
/// Spotify's tiles are 16:9.
const TILE_ASPECT: f32 = 0.5625;
const TILE_RADIUS: f32 = 8.0;
/// The tilted cover: 45% of the tile's width, turned 25 degrees and pushed
/// 18% right and 2% up from the tile's bottom right corner.
const TILE_ART_SHARE: f32 = 0.45;
const TILE_ART_TURN: f32 = 25.0;
const TILE_ART_SHIFT: Vec2 = vec2(0.18, -0.02);
const TILE_ART_RADIUS: f32 = 4.0;
/// Placeholder tiles while the grid loads.
const SKELETON_TILES: usize = 12;

/// How many tiles a row of `width` holds, and how wide each is.
pub(crate) fn tile_layout(width: f32) -> (usize, f32) {
    let columns = ((width + TILE_GAP) / (TILE_MIN_WIDTH + TILE_GAP))
        .floor()
        .max(2.0) as usize;
    let tile = ((width - TILE_GAP * (columns - 1) as f32) / columns as f32).max(1.0);
    (columns, tile)
}

/// Category tiles in rows that fill the page's width. Items that are not
/// categories are left to the shelves.
fn tile_grid(app: &mut App, ui: &mut egui::Ui, items: &[BrowseItem]) {
    let tiles: Vec<&BrowseCategory> = items
        .iter()
        .filter_map(|item| match item {
            BrowseItem::Category(category) => Some(category),
            _ => None,
        })
        .collect();
    // Spotify's titles shrink when the whole grid is 520 points or narrower.
    let narrow = ui.available_width() <= 520.0;
    let (columns, width) = tile_layout(ui.available_width());
    let size = vec2(width, (width * TILE_ASPECT).round());
    for row in tiles.chunks(columns) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = TILE_GAP;
            for category in row {
                if category_tile(app, ui, category, size, narrow).clicked() {
                    app.actions
                        .push(Action::Open(Page::Category(category.id.clone())));
                }
            }
        });
        ui.add_space(TILE_GAP - ui.spacing().item_spacing.y);
    }
}

/// The colour Spotify gives a tile, or the theme's surface without one.
fn tile_color(app: &App, color: Option<[u8; 3]>) -> egui::Color32 {
    color.map_or(app.palette.surface_hover, |[r, g, b]| {
        egui::Color32::from_rgb(r, g, b)
    })
}

/// One coloured tile: its title at the top left and its cover tilted at the
/// bottom right, cut to the tile's rounded corners.
fn category_tile(
    app: &App,
    ui: &mut egui::Ui,
    category: &BrowseCategory,
    size: Vec2,
    narrow: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let title = category.title.clone();
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Link, true, &title));
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter();
    let mut fill = tile_color(app, category.color);
    if response.hovered() {
        fill = fill.gamma_multiply(0.92);
    }
    painter.rect_filled(rect, CornerRadius::same(TILE_RADIUS as u8), fill);
    if let Some(url) = &category.image {
        paint_tilted_art(ui, url, rect);
    }
    // Spotify writes every tile's title in white over its own colour, which
    // no theme chooses, so the title does not follow the theme either.
    let font = theme::bold(if narrow { 16.0 } else { 24.0 });
    let wrap = (size.x - 16.0 - size.x * 0.3).max(16.0);
    let mut job =
        egui::text::LayoutJob::simple(category.title.clone(), font, egui::Color32::WHITE, wrap);
    job.wrap.max_rows = 3;
    job.wrap.break_anywhere = false;
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    ui.painter_at(rect)
        .galley(rect.min + vec2(16.0, 16.0), galley, egui::Color32::WHITE);
    response
}

/// Paints `url`'s artwork turned as on Spotify's tiles, cut where it leaves
/// the tile's rounded outline. egui clips only to rectangles, so the cover
/// is a mesh of the outline both shapes share.
fn paint_tilted_art(ui: &egui::Ui, url: &str, tile: Rect) {
    let side = tile.width() * TILE_ART_SHARE;
    let pixels = crate::images::cover_pixels(side * ui.ctx().pixels_per_point());
    let uri = crate::images::scaled_uri(url, pixels);
    let image = egui::Image::new(uri.as_str())
        .texture_options(crate::images::COVER_TEXTURE)
        .show_loading_spinner(false);
    let Ok(egui::load::TexturePoll::Ready { texture }) =
        image.load_for_size(ui.ctx(), Vec2::splat(side))
    else {
        return;
    };
    let shape = tilted_art_outline(tile);
    if shape.len() < 3 {
        return;
    }
    let (center, turn) = tilted_art_frame(tile);
    let back = egui::emath::Rot2::from_angle(-turn);
    let mut mesh = egui::Mesh::with_texture(texture.id);
    for point in &shape {
        let local = back * (*point - center);
        let uv = pos2(local.x / side + 0.5, local.y / side + 0.5);
        mesh.vertices.push(egui::epaint::Vertex {
            pos: *point,
            uv,
            color: egui::Color32::WHITE,
        });
    }
    for index in 1..shape.len() as u32 - 1 {
        mesh.add_triangle(0, index, index + 1);
    }
    ui.painter_at(tile).add(mesh);
}

/// Where the tilted cover's centre is, and how far it is turned.
fn tilted_art_frame(tile: Rect) -> (Pos2, f32) {
    let side = tile.width() * TILE_ART_SHARE;
    let turn = TILE_ART_TURN.to_radians();
    let resting = pos2(tile.right() - side / 2.0, tile.bottom() - side / 2.0);
    let shift = egui::emath::Rot2::from_angle(turn) * (TILE_ART_SHIFT * side);
    (resting + shift, turn)
}

/// The outline of the tilted cover inside the tile's rounded outline.
pub(crate) fn tilted_art_outline(tile: Rect) -> Vec<Pos2> {
    let side = tile.width() * TILE_ART_SHARE;
    let (center, turn) = tilted_art_frame(tile);
    let rotate = egui::emath::Rot2::from_angle(turn);
    let art: Vec<Pos2> = rounded_outline(
        Rect::from_center_size(Pos2::ZERO, Vec2::splat(side)),
        TILE_ART_RADIUS,
    )
    .into_iter()
    .map(|point| center + rotate * point.to_vec2())
    .collect();
    clip_convex(&art, &rounded_outline(tile, TILE_RADIUS))
}

/// A rounded rectangle's outline, clockwise on screen, as a polygon.
fn rounded_outline(rect: Rect, radius: f32) -> Vec<Pos2> {
    const STEPS: usize = 6;
    let radius = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let corners = [
        (pos2(rect.right() - radius, rect.top() + radius), -90.0_f32),
        (pos2(rect.right() - radius, rect.bottom() - radius), 0.0),
        (pos2(rect.left() + radius, rect.bottom() - radius), 90.0),
        (pos2(rect.left() + radius, rect.top() + radius), 180.0),
    ];
    let mut points = Vec::with_capacity(4 * (STEPS + 1));
    for (center, start) in corners {
        for step in 0..=STEPS {
            let angle = (start + 90.0 * step as f32 / STEPS as f32).to_radians();
            points.push(center + radius * vec2(angle.cos(), angle.sin()));
        }
    }
    points
}

/// The part of convex polygon `subject` inside convex polygon `clip`, both
/// wound the same way (Sutherland and Hodgman).
fn clip_convex(subject: &[Pos2], clip: &[Pos2]) -> Vec<Pos2> {
    let mut output = subject.to_vec();
    for (index, &from) in clip.iter().enumerate() {
        let to = clip[(index + 1) % clip.len()];
        let edge = to - from;
        let inside = |point: Pos2| edge.x * (point.y - from.y) - edge.y * (point.x - from.x) >= 0.0;
        let input = std::mem::take(&mut output);
        for (at, &current) in input.iter().enumerate() {
            let previous = input[(at + input.len() - 1) % input.len()];
            let crossing = || {
                let direction = current - previous;
                let denominator = edge.x * direction.y - edge.y * direction.x;
                if denominator.abs() < f32::EPSILON {
                    return current;
                }
                let t =
                    (edge.y * (previous.x - from.x) - edge.x * (previous.y - from.y)) / denominator;
                previous + direction * t
            };
            match (inside(previous), inside(current)) {
                (true, true) => output.push(current),
                (true, false) => output.push(crossing()),
                (false, true) => {
                    output.push(crossing());
                    output.push(current);
                }
                (false, false) => {}
            }
        }
        if output.is_empty() {
            break;
        }
    }
    output
}

/// Placeholder tiles the shape of the grid that is loading. Like the other
/// skeletons, they do not move.
fn skeleton_tiles(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    locale: crate::i18n::Locale,
    count: usize,
) {
    let (columns, width) = tile_layout(ui.available_width());
    let size = vec2(width, (width * TILE_ASPECT).round());
    let fill = palette.surface_hover;
    let mut shown = 0;
    while shown < count {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = TILE_GAP;
            for _ in 0..columns.min(count - shown) {
                let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
                if shown == 0 {
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Label,
                            true,
                            gettext(locale, "Loading…"),
                        )
                    });
                }
                ui.painter()
                    .rect_filled(rect, CornerRadius::same(TILE_RADIUS as u8), fill);
                shown += 1;
            }
        });
        ui.add_space(TILE_GAP - ui.spacing().item_spacing.y);
    }
}

/// A browse page opened from a Browse all tile: its title over its colour,
/// then its grids of tiles and shelves of playlists, albums and artists.
pub fn category(app: &mut App, ui: &mut egui::Ui, id: &str) {
    let palette = app.palette;
    let locale = app.locale;
    let tile = app
        .search
        .browse
        .get()
        .into_iter()
        .flatten()
        .flat_map(|section| &section.items)
        .chain(
            app.search
                .categories
                .values()
                .filter_map(Loadable::get)
                .flat_map(|page| &page.sections)
                .flat_map(|section| &section.items),
        )
        .find_map(|item| match item {
            BrowseItem::Category(category) if category.id == id => Some(category.clone()),
            _ => None,
        });
    let page = app.search.categories.get(id).cloned().unwrap_or_default();
    let title = page
        .get()
        .map(|page| page.title.clone())
        .filter(|title| !title.is_empty())
        .or_else(|| tile.as_ref().map(|tile| tile.title.clone()));
    let color = page
        .get()
        .and_then(|page| page.color)
        .or_else(|| tile.as_ref().and_then(|tile| tile.color));
    category_header(app, ui, title.as_deref(), color);
    match page {
        Loadable::Loaded(page) => {
            if page.sections.is_empty() {
                widgets::empty_state(
                    ui,
                    &palette,
                    Icon::Music,
                    &gettext(locale, "Nothing to browse here"),
                    &gettext(locale, "Spotify lists nothing on this page yet."),
                );
            }
            for (index, section) in page.sections.iter().enumerate() {
                browse_section(app, ui, id, index, section);
            }
        }
        Loadable::Failed(error) => {
            widgets::error_row(ui, app, &error, Some(Page::Category(id.to_owned())));
        }
        Loadable::Loading | Loadable::NotLoaded => {
            if app.network_reachable {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = widgets::CARD_GAP / 2.0;
                    widgets::skeleton_cards(ui, &palette, locale, 6, false);
                });
            } else {
                widgets::empty_state(
                    ui,
                    &palette,
                    Icon::WifiOff,
                    &gettext(locale, "You're offline"),
                    &gettext(locale, "Connect to the internet to browse Spotify."),
                );
            }
        }
    }
}

/// The page's title over a band of its colour, as Spotify heads a browse
/// page.
fn category_header(app: &App, ui: &mut egui::Ui, title: Option<&str>, color: Option<[u8; 3]>) {
    let palette = app.palette;
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 160.0), Sense::hover());
    let fill = tile_color(app, color);
    ui.painter()
        .rect_filled(rect, CornerRadius::same(TILE_RADIUS as u8), fill);
    if let Some(title) = title {
        let size = if width < 520.0 { 40.0 } else { 64.0 };
        let galley = ui.fonts_mut(|fonts| {
            fonts.layout(
                title.to_owned(),
                theme::bold(size),
                // White over Spotify's own colour, as on the tiles.
                egui::Color32::WHITE,
                (width - 48.0).max(16.0),
            )
        });
        let at = pos2(rect.left() + 24.0, rect.bottom() - 20.0 - galley.size().y);
        ui.painter_at(rect).galley(at, galley, egui::Color32::WHITE);
    } else {
        let bar = Rect::from_min_size(
            pos2(rect.left() + 24.0, rect.bottom() - 76.0),
            vec2(width * 0.35, 48.0),
        );
        ui.painter()
            .rect_filled(bar, CornerRadius::same(6), palette.surface_active);
    }
    ui.add_space(20.0);
}

/// One section of a browse page: a grid of tiles, or a shelf of cards.
fn browse_section(
    app: &mut App,
    ui: &mut egui::Ui,
    page: &str,
    index: usize,
    section: &BrowseSection,
) {
    let palette = app.palette;
    if section.is_grid() {
        if !section.title.is_empty() {
            theme::section_title(ui, &palette, &section.title);
            ui.add_space(4.0);
        }
        tile_grid(app, ui, &section.items);
        ui.add_space(8.0);
        return;
    }
    let id = format!("browse-{page}-{index}");
    widgets::shelf(ui, &palette, &id, &section.title, |ui| {
        for item in &section.items {
            match item {
                BrowseItem::Playlist(playlist) => playlist_card(app, ui, playlist),
                BrowseItem::Album(album) => album_card(app, ui, album),
                BrowseItem::Artist(artist) => artist_card(app, ui, artist),
                BrowseItem::Category(category) => {
                    let width = widgets::CARD_WIDTH;
                    let size = vec2(width, (width * TILE_ASPECT).round());
                    if category_tile(app, ui, category, size, true).clicked() {
                        app.actions
                            .push(Action::Open(Page::Category(category.id.clone())));
                    }
                }
            }
        }
    });
}

fn all(app: &mut App, ui: &mut egui::Ui, results: &SearchResults) {
    let palette = app.palette;
    let locale = app.locale;
    let query = app.search.committed.to_lowercase();
    let top_artist = results
        .artists
        .as_ref()
        .and_then(|page| page.items.first())
        .filter(|artist| {
            artist.name.to_lowercase() == query
                || results.tracks.as_ref().is_none_or(|t| t.items.is_empty())
        });
    let top_art = app.art_px(TOP_RESULT_ART);
    let wide = ui.available_width() > 720.0;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 24.0;
        let top_width = if wide {
            (ui.available_width() * 0.4).clamp(260.0, 460.0)
        } else {
            ui.available_width()
        };
        ui.vertical(|ui| {
            ui.set_width(top_width);
            theme::section_title(ui, &palette, &gettext(locale, "Top result"));
            ui.add_space(4.0);
            if let Some(artist) = top_artist {
                top_result(
                    app,
                    ui,
                    pick_image(&artist.images, top_art),
                    &artist.name,
                    TopResultSubtitle::Text(&gettext(locale, "Artist")),
                    true,
                    Some(artist.uri.clone()),
                    Page::Artist(artist.id.clone()),
                    |ui, app| {
                        widgets::context_menu_items(ui, app, &artist.uri, &artist.name, None);
                    },
                );
            } else if let Some(track) = results.tracks.as_ref().and_then(|page| page.items.first())
            {
                let page = track
                    .album
                    .as_ref()
                    .map(|album| Page::Album(album.id.clone()))
                    .unwrap_or(Page::Search);
                top_result(
                    app,
                    ui,
                    track.image(top_art),
                    &track.name,
                    TopResultSubtitle::SongArtists(&track.artists),
                    false,
                    Some(track.uri.clone()),
                    page,
                    |ui, app| {
                        widgets::item_menu(
                            ui,
                            app,
                            &PlayableItem::Track(track.clone()),
                            None,
                            None,
                        );
                    },
                );
            } else if let Some(album) = results.albums.as_ref().and_then(|page| page.items.first())
            {
                top_result(
                    app,
                    ui,
                    pick_image(&album.images, top_art),
                    &album.name,
                    TopResultSubtitle::Text(
                        // Translators: {artists} is the album's artist names.
                        &gettext(locale, "Album • {artists}").replace(
                            "{artists}",
                            &crate::api::models::join_names(
                                album.artists.iter().map(|a| a.name.as_str()),
                            ),
                        ),
                    ),
                    false,
                    Some(album.uri.clone()),
                    Page::Album(album.id.clone()),
                    |ui, app| {
                        widgets::context_menu_items(ui, app, &album.uri, &album.name, None);
                    },
                );
            } else if let Some(playlist) = results
                .playlists
                .as_ref()
                .and_then(|page| page.items.first())
            {
                top_result(
                    app,
                    ui,
                    pick_image(&playlist.images, top_art),
                    &playlist.name,
                    TopResultSubtitle::Text(
                        // Translators: {owner} is the name of the playlist's owner.
                        &gettext(locale, "Playlist • {owner}")
                            .replace("{owner}", playlist.owner_name()),
                    ),
                    false,
                    Some(playlist.uri.clone()),
                    Page::Playlist(playlist.id.clone()),
                    |ui, app| {
                        let owned = app.user_id().is_some_and(|id| playlist.owned_by(id));
                        widgets::context_menu_items(
                            ui,
                            app,
                            &playlist.uri,
                            &playlist.name,
                            owned.then_some(playlist),
                        );
                    },
                );
            } else if let Some(show) = results.shows.as_ref().and_then(|page| page.items.first()) {
                top_result(
                    app,
                    ui,
                    pick_image(&show.images, top_art),
                    &show.name,
                    TopResultSubtitle::Text(
                        // Translators: {publisher} is the podcast's publisher.
                        &gettext(locale, "Podcast • {publisher}")
                            .replace("{publisher}", &show.publisher),
                    ),
                    false,
                    Some(show.uri.clone()),
                    Page::Show(show.id.clone()),
                    |ui, app| {
                        widgets::context_menu_items(ui, app, &show.uri, &show.name, None);
                    },
                );
            }
        });
        if wide {
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                songs(app, ui, results, 4);
            });
        }
    });
    if !wide {
        ui.add_space(12.0);
        songs(app, ui, results, 4);
    }
    ui.add_space(8.0);
    shelf_artists(app, ui, results);
    shelf_albums(app, ui, results);
    shelf_playlists(app, ui, results);
    shelf_shows(app, ui, results);
    if results
        .episodes
        .as_ref()
        .is_some_and(|page| !page.items.is_empty())
    {
        theme::section_title(ui, &palette, &gettext(locale, "Episodes"));
        ui.add_space(4.0);
        episodes(app, ui, results, 4);
    }
}

enum TopResultSubtitle<'a> {
    Text(&'a str),
    SongArtists(&'a [ArtistRef]),
}

/// The top result card, as Spotify draws it: a 92 px cover 20 px in, the
/// name in 32 px bold under it, on an 8 px rounded card.
const TOP_RESULT_ART: f32 = 92.0;
const TOP_RESULT_PADDING: f32 = 20.0;
const TOP_RESULT_HEIGHT: f32 = 228.0;
const TOP_RESULT_RADIUS: u8 = 8;
/// The green play button the card shows under the pointer.
const TOP_RESULT_PLAY: f32 = 48.0;

#[allow(clippy::too_many_arguments)]
fn top_result(
    app: &mut App,
    ui: &mut egui::Ui,
    image: Option<&str>,
    title: &str,
    subtitle: TopResultSubtitle<'_>,
    round: bool,
    play_uri: Option<String>,
    page: Page,
    menu: impl FnOnce(&mut egui::Ui, &mut App),
) {
    let palette = app.palette;
    let mut subtitle_clicked = false;
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width(), TOP_RESULT_HEIGHT),
        Sense::click(),
    );
    if ui.is_rect_visible(rect) {
        let hovered = ui.rect_contains_pointer(rect);
        let fill = if hovered {
            palette.surface_hover
        } else {
            palette.surface
        };
        ui.painter()
            .rect_filled(rect, CornerRadius::same(TOP_RESULT_RADIUS), fill);
        let image_rect = Rect::from_min_size(
            rect.min + Vec2::splat(TOP_RESULT_PADDING),
            Vec2::splat(TOP_RESULT_ART),
        );
        let art_radius = if round { TOP_RESULT_ART / 2.0 } else { 4.0 };
        widgets::paint_shadow(ui, &palette, image_rect, art_radius);
        widgets::paint_cover(
            ui,
            &palette,
            image,
            image_rect,
            art_radius,
            if round { Icon::User } else { Icon::Music },
            Some(app.backend.art()),
        );
        let text_clip = Rect::from_min_max(
            pos2(rect.left() + TOP_RESULT_PADDING, image_rect.bottom() + 16.0),
            pos2(rect.right() - TOP_RESULT_PADDING, rect.bottom()),
        );
        let painter = ui.painter().with_clip_rect(text_clip);
        crate::bidi::paint_line(
            &painter,
            text_clip.left(),
            text_clip.right(),
            text_clip.top() + 20.0,
            title,
            theme::bold(32.0),
            palette.text,
        );
        match subtitle {
            TopResultSubtitle::SongArtists(artists) => {
                let subtitle_rect = Rect::from_min_max(
                    pos2(text_clip.left(), text_clip.top() + 46.0),
                    pos2(text_clip.right(), text_clip.top() + 66.0),
                );
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(subtitle_rect)
                        .layout(Layout::left_to_right(Align::Center)),
                );
                child.set_clip_rect(subtitle_rect.intersect(ui.clip_rect()));
                child.spacing_mut().item_spacing.x = 0.0;
                theme::text(
                    &mut child,
                    // Translators: Precedes the song's artist names, which follow as links.
                    gettext(app.locale, "Song • ").as_ref(),
                    theme::regular(14.0),
                    palette.secondary,
                );
                subtitle_clicked = widgets::artist_links(
                    &mut child,
                    app,
                    artists,
                    theme::regular(14.0),
                    palette.secondary,
                );
            }
            TopResultSubtitle::Text(subtitle) => {
                crate::bidi::paint_line(
                    &painter,
                    text_clip.left(),
                    text_clip.right(),
                    text_clip.top() + 56.0,
                    subtitle,
                    theme::regular(14.0),
                    palette.secondary,
                );
            }
        }
        if hovered && let Some(uri) = &play_uri {
            let inset = TOP_RESULT_PADDING + TOP_RESULT_PLAY / 2.0;
            let button = Rect::from_center_size(
                pos2(rect.right() - inset, rect.bottom() - inset),
                Vec2::splat(TOP_RESULT_PLAY),
            );
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(button)
                    .layout(Layout::centered_and_justified(egui::Direction::LeftToRight)),
            );
            if theme::circle_button(
                &mut child,
                Icon::PlayFilled,
                TOP_RESULT_PLAY,
                palette.accent,
                palette.accent_hover,
                palette.on_accent,
                &gettext(app.locale, "Play"),
            )
            .clicked()
            {
                if uri.starts_with("spotify:track:") {
                    app.actions.push(Action::PlayUris {
                        uris: vec![uri.clone()],
                        index: 0,
                    });
                } else {
                    app.actions.push(Action::PlayContext {
                        uri: uri.clone(),
                        offset_uri: None,
                        offset_index: None,
                    });
                }
            }
        }
    }
    if response.clicked() && !subtitle_clicked && page != Page::Search {
        app.actions.push(Action::Open(page));
    }
    egui::Popup::context_menu(&response)
        .frame(widgets::menu_frame(&palette))
        .show(|ui| menu(ui, app));
}

fn songs(app: &mut App, ui: &mut egui::Ui, results: &SearchResults, limit: usize) {
    let palette = app.palette;
    let Some(page) = &results.tracks else {
        return;
    };
    if page.items.is_empty() {
        return;
    }
    theme::section_title(ui, &palette, &gettext(app.locale, "Songs"));
    ui.add_space(4.0);
    let items: Vec<PlayableItem> = page
        .items
        .iter()
        .cloned()
        .map(PlayableItem::Track)
        .collect();
    for (index, item) in items.iter().take(limit).enumerate() {
        // A search result plays on its own, and Spotify continues with what
        // follows it (autoplay), as the Spotify apps do; the other results are
        // not a queue. A one-song list loads as that song's own context.
        let context = RowContext::Uris(Arc::from([item.uri().to_string()]));
        widgets::track_row(
            ui,
            app,
            TrackRow {
                index,
                number: None,
                item,
                context: &context,
                show_cover: true,
                show_album: limit == usize::MAX,
                added_at: None,
                added_by: None,
                show_added_by: false,
                show_plays: false,
                compact: limit != usize::MAX,
                thin: false,
                shift: 0.0,
                picked: false,
                picked_songs: &[],
                picked_positions: &[],
                position: None,
            },
        );
    }
}

fn artist_card(app: &mut App, ui: &mut egui::Ui, artist: &Artist) {
    let context_here = app.playing_context_uri().as_deref() == Some(artist.uri.as_str());
    let playing_here = context_here && app.believed_playing();
    let card_art = app.art_px(widgets::CARD_ART);
    let card = widgets::card(
        ui,
        app,
        pick_image(&artist.images, card_art),
        &artist.name,
        &gettext(app.locale, "Artist"),
        widgets::CardCover::portrait(playing_here),
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
        .id(ui.make_persistent_id(("search-artist-menu", &artist.uri)))
        .frame(widgets::menu_frame(&app.palette))
        .show(|ui| {
            widgets::context_menu_items(ui, app, &artist.uri, &artist.name, None);
        });
}

fn shelf_artists(app: &mut App, ui: &mut egui::Ui, results: &SearchResults) {
    let palette = app.palette;
    let Some(page) = &results.artists else { return };
    if page.items.is_empty() {
        return;
    }
    widgets::shelf(
        ui,
        &palette,
        "search-artists",
        &gettext(app.locale, "Artists"),
        |ui| {
            for artist in &page.items {
                artist_card(app, ui, artist);
            }
        },
    );
}

fn artists_grid(app: &mut App, ui: &mut egui::Ui, results: &SearchResults) {
    let Some(page) = &results.artists else { return };
    widgets::grid(ui, |ui| {
        for artist in &page.items {
            artist_card(app, ui, artist);
        }
    });
}

fn album_card(app: &mut App, ui: &mut egui::Ui, album: &crate::api::models::Album) {
    let subtitle = format!(
        "{} • {}",
        album.year().unwrap_or(""),
        crate::api::models::join_names(album.artists.iter().map(|a| a.name.as_str()))
    );
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
        .id(ui.make_persistent_id(("search-album-menu", &album.uri)))
        .frame(widgets::menu_frame(&app.palette))
        .show(|ui| {
            widgets::context_menu_items(ui, app, &album.uri, &album.name, None);
        });
}

fn shelf_albums(app: &mut App, ui: &mut egui::Ui, results: &SearchResults) {
    let palette = app.palette;
    let Some(page) = &results.albums else { return };
    if page.items.is_empty() {
        return;
    }
    widgets::shelf(
        ui,
        &palette,
        "search-albums",
        &gettext(app.locale, "Albums"),
        |ui| {
            for album in &page.items {
                album_card(app, ui, album);
            }
        },
    );
}

fn albums_grid(app: &mut App, ui: &mut egui::Ui, results: &SearchResults) {
    let Some(page) = &results.albums else { return };
    widgets::grid(ui, |ui| {
        for album in &page.items {
            album_card(app, ui, album);
        }
    });
}

fn playlist_card(app: &mut App, ui: &mut egui::Ui, playlist: &crate::api::models::Playlist) {
    let context_here = app.playing_context_uri().as_deref() == Some(playlist.uri.as_str());
    let playing_here = context_here && app.believed_playing();
    let card_art = app.art_px(widgets::CARD_ART);
    let card = widgets::card(
        ui,
        app,
        pick_image(&playlist.images, card_art),
        &playlist.name,
        // Translators: {owner} is the name of the playlist's owner.
        &gettext(app.locale, "By {owner}").replace("{owner}", playlist.owner_name()),
        widgets::CardCover::square(playing_here),
    );
    if card.play {
        if context_here {
            app.actions.push(Action::TogglePlay);
        } else {
            app.actions.push(Action::PlayContext {
                uri: playlist.uri.clone(),
                offset_uri: None,
                offset_index: None,
            });
        }
    }
    if card.clicked {
        app.actions
            .push(Action::Open(Page::Playlist(playlist.id.clone())));
    }
    egui::Popup::context_menu(&card.response)
        .id(ui.make_persistent_id(("search-playlist-menu", &playlist.uri)))
        .frame(widgets::menu_frame(&app.palette))
        .show(|ui| {
            let owned = app.user_id().is_some_and(|id| playlist.owned_by(id));
            widgets::context_menu_items(
                ui,
                app,
                &playlist.uri,
                &playlist.name,
                owned.then_some(playlist),
            );
        });
}

fn shelf_playlists(app: &mut App, ui: &mut egui::Ui, results: &SearchResults) {
    let palette = app.palette;
    let Some(page) = &results.playlists else {
        return;
    };
    if page.items.is_empty() {
        return;
    }
    widgets::shelf(
        ui,
        &palette,
        "search-playlists",
        &gettext(app.locale, "Playlists"),
        |ui| {
            for playlist in &page.items {
                playlist_card(app, ui, playlist);
            }
        },
    );
}

fn playlists_grid(app: &mut App, ui: &mut egui::Ui, results: &SearchResults) {
    let Some(page) = &results.playlists else {
        return;
    };
    widgets::grid(ui, |ui| {
        for playlist in &page.items {
            playlist_card(app, ui, playlist);
        }
    });
}

fn show_card(app: &mut App, ui: &mut egui::Ui, show: &crate::api::models::Show) {
    let card_art = app.art_px(widgets::CARD_ART);
    let card = widgets::card(
        ui,
        app,
        pick_image(&show.images, card_art),
        &show.name,
        &show.publisher,
        widgets::CardCover::default(),
    );
    if card.clicked {
        app.actions.push(Action::Open(Page::Show(show.id.clone())));
    }
    egui::Popup::context_menu(&card.response)
        .id(ui.make_persistent_id(("search-show-menu", &show.uri)))
        .frame(widgets::menu_frame(&app.palette))
        .show(|ui| {
            widgets::context_menu_items(ui, app, &show.uri, &show.name, None);
        });
}

fn shelf_shows(app: &mut App, ui: &mut egui::Ui, results: &SearchResults) {
    let palette = app.palette;
    let Some(page) = &results.shows else { return };
    if page.items.is_empty() {
        return;
    }
    widgets::shelf(
        ui,
        &palette,
        "search-shows",
        &gettext(app.locale, "Podcasts"),
        |ui| {
            for show in &page.items {
                show_card(app, ui, show);
            }
        },
    );
}

fn shows_grid(app: &mut App, ui: &mut egui::Ui, results: &SearchResults) {
    let Some(page) = &results.shows else { return };
    widgets::grid(ui, |ui| {
        for show in &page.items {
            show_card(app, ui, show);
        }
    });
}

fn episodes(app: &mut App, ui: &mut egui::Ui, results: &SearchResults, limit: usize) {
    let Some(page) = &results.episodes else {
        return;
    };
    for episode in page.items.iter().take(limit) {
        super::show::episode_row(app, ui, episode, None);
    }
}

#[allow(dead_code)]
fn align_right(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    ui.with_layout(Layout::right_to_left(Align::Center), add);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_browse_tiles_fill_the_row() {
        for width in [300.0, 520.0, 700.0, 942.0, 1600.0] {
            let (columns, tile) = tile_layout(width);
            assert!(columns >= 2, "{width}");
            assert!(tile >= TILE_MIN_WIDTH || columns == 2, "{width}: {tile}");
            let used = tile * columns as f32 + TILE_GAP * (columns - 1) as f32;
            assert!((used - width).abs() < 0.5, "{width}: {used}");
        }
        assert_eq!(tile_layout(942.0).0, 4);
    }

    #[test]
    fn search_browse_tilted_cover_stays_inside_the_tile() {
        let tile = Rect::from_min_size(pos2(10.0, 20.0), vec2(217.0, 122.0));
        let outline = tilted_art_outline(tile);
        assert!(outline.len() >= 3);
        for point in &outline {
            assert!(tile.expand(0.01).contains(*point), "{point:?}");
        }
        // The cover sits at the bottom right and leaves the title's corner.
        let bounds = Rect::from_points(&outline);
        assert!(bounds.left() > tile.center().x);
        assert!((bounds.right() - tile.right()).abs() < 0.5);
        assert!((bounds.bottom() - tile.bottom()).abs() < 0.5);
        // The tile's rounded bottom right corner cuts the cover.
        let corner = tile.right_bottom();
        assert!(outline.iter().all(|point| point.distance(corner) > 1.0));
    }

    #[test]
    fn search_browse_clipping_keeps_a_square_inside_and_drops_one_outside() {
        let square = |rect: Rect| {
            vec![
                rect.left_top(),
                rect.right_top(),
                rect.right_bottom(),
                rect.left_bottom(),
            ]
        };
        let clip = square(Rect::from_min_size(Pos2::ZERO, Vec2::splat(10.0)));
        let inside = square(Rect::from_min_size(pos2(2.0, 2.0), Vec2::splat(3.0)));
        assert_eq!(clip_convex(&inside, &clip), inside);
        let outside = square(Rect::from_min_size(pos2(20.0, 20.0), Vec2::splat(3.0)));
        assert!(clip_convex(&outside, &clip).is_empty());
        let half = square(Rect::from_min_size(pos2(5.0, 2.0), Vec2::splat(10.0)));
        let cut = Rect::from_points(&clip_convex(&half, &clip));
        assert_eq!(cut, Rect::from_min_max(pos2(5.0, 2.0), pos2(10.0, 10.0)));
    }
}
