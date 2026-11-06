//! The Home page, laid out as Spotify's desktop Home: All / Music /
//! Podcasts chips, the grid of shortcuts to what was played, a wash tinted
//! by the hovered shortcut, the newest release from a followed artist, and
//! shelves of cards with Show all.

use std::sync::Arc;

use egui::{Color32, CornerRadius, Id, Rect, Sense, Ui, UiBuilder, Vec2, pos2, vec2};

use crate::api::models::{Album, Episode, PlayableItem, Playlist, Show, pick_image};
use crate::app::App;
use crate::i18n::{Locale, gettext};
use crate::model::{Action, Loadable, Page, RowContext, TopPeriod};
use crate::theme::{self, Icon, Palette};

use super::widgets::{self, CardResponse, TrackRow};

/// Which shelves Home shows, as Spotify's chips above the shortcuts pick
/// them. The choice lasts while the app runs, as Spotify's does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Filter {
    #[default]
    All,
    Music,
    Podcasts,
}

impl Filter {
    /// Whether a shortcut or shelf for a podcast belongs under this chip.
    fn admits(self, podcast: bool) -> bool {
        match self {
            Self::All => true,
            Self::Music => !podcast,
            Self::Podcasts => podcast,
        }
    }
}

fn filter_id() -> Id {
    Id::new("home-filter")
}

/// The space above each section, as Spotify's Home leaves between shelves.
const SECTION_GAP: f32 = 24.0;
/// The size of a section's title, Spotify's 24 point bold.
const SECTION_TITLE: f32 = 24.0;

pub fn show(app: &mut App, ui: &mut Ui) {
    let palette = app.palette;
    if !app.network_reachable {
        let space = ((ui.available_height() - 160.0) / 2.0).max(48.0);
        ui.add_space(space);
        ui.vertical_centered(|ui| {
            theme::icon(ui, Icon::WifiOff, 54.0, palette.dim);
            ui.add_space(16.0);
            theme::text(
                ui,
                gettext(app.locale, "You're offline"),
                theme::bold(28.0),
                palette.text,
            );
            ui.add_space(8.0);
            theme::text(
                ui,
                gettext(
                    app.locale,
                    "Spotifast will reconnect automatically when your connection returns.",
                ),
                theme::regular(14.0),
                palette.secondary,
            );
        });
        return;
    }
    // The wash lies under everything; its colour is known once the
    // shortcuts know which one is hovered.
    let wash = ui.painter().add(egui::Shape::Noop);
    let top = ui.max_rect().top();
    ui.add_space(8.0);
    let filter = filter_chips(app, ui);
    ui.add_space(16.0);
    let hovered = quick_access(app, ui, filter);
    paint_wash(app, ui, wash, top, hovered);

    if filter == Filter::Podcasts {
        if !podcasts(app, ui) {
            widgets::empty_state(
                ui,
                &palette,
                Icon::Mic,
                &gettext(app.locale, "No podcasts yet"),
                &gettext(
                    app.locale,
                    "Episodes of the podcasts you follow will appear here.",
                ),
            );
        }
        return;
    }
    if app.settings.home.made_for_you.visible {
        made_for_you(app, ui);
        top_mixes(app, ui);
    }
    new_releases(app, ui);
    recently_played(app, ui);
    if filter.admits(true) {
        podcasts(app, ui);
    }
    top_artists(app, ui);
    top_tracks(app, ui);
    if app.settings.home.recommendations.visible {
        recommendations(app, ui);
    }
}

/// The All / Music / Podcasts chips; returns the chosen one.
fn filter_chips(app: &App, ui: &mut Ui) -> Filter {
    let current = ui
        .data(|data| data.get_temp::<Filter>(filter_id()))
        .unwrap_or_default();
    let labels = [
        (Filter::All, gettext(app.locale, "All")),
        (Filter::Music, gettext(app.locale, "Music")),
        (Filter::Podcasts, gettext(app.locale, "Podcasts")),
    ];
    let options: Vec<(Filter, &str)> = labels
        .iter()
        .map(|(filter, label)| (*filter, label.as_ref()))
        .collect();
    match widgets::chips(ui, &app.palette, &options, current) {
        Some(chosen) if chosen != current => {
            ui.data_mut(|data| data.insert_temp(filter_id(), chosen));
            chosen
        }
        _ => current,
    }
}

/// How far down the wash reaches, as Spotify's Home gradient.
const WASH_HEIGHT: f32 = 332.0;
/// How much of the art's colour the wash starts with. Spotify darkens the
/// colour by 60%; this also meets the window's own header wash, which has
/// faded to about this much where the page starts.
const WASH_STRENGTH: f32 = 0.4;
/// How quickly the wash turns to a newly hovered shortcut's colour.
const WASH_EASE_SECONDS: f32 = 0.1;
/// Liked Songs' colour, as the window tints its page with.
const LIKED_TINT: Color32 = Color32::from_rgb(0x50, 0x38, 0xc8);

/// The wash's colour at the top: the hovered shortcut's art, or else the
/// playing song's, darkened into the window; transparent with neither.
fn wash_target(palette: &Palette, art: Option<Color32>) -> egui::Rgba {
    art.map_or(egui::Rgba::TRANSPARENT, |art| {
        egui::Rgba::from(super::blend(palette.window, art, WASH_STRENGTH))
    })
}

/// Moves `current` toward `target` by the share of the way a frame `dt`
/// long covers; `None` once there.
fn ease_toward(current: egui::Rgba, target: egui::Rgba, dt: f32) -> Option<egui::Rgba> {
    let share = 1.0 - (-dt / WASH_EASE_SECONDS).exp();
    let channel = |from: f32, to: f32| from + (to - from) * share;
    let next = egui::Rgba::from_rgba_premultiplied(
        channel(current.r(), target.r()),
        channel(current.g(), target.g()),
        channel(current.b(), target.b()),
        channel(current.a(), target.a()),
    );
    let apart = [
        next.r() - target.r(),
        next.g() - target.g(),
        next.b() - target.b(),
        next.a() - target.a(),
    ]
    .into_iter()
    .any(|difference| difference.abs() > 0.002);
    apart.then_some(next)
}

/// Paints the wash into the slot `show` kept under the page, turning to
/// the hovered shortcut's colour as Spotify's Home does.
fn paint_wash(
    app: &App,
    ui: &Ui,
    slot: egui::layers::ShapeIdx,
    top: f32,
    hovered: Option<Color32>,
) {
    if !app.settings.accent_from_art {
        return;
    }
    let target = wash_target(&app.palette, hovered.or_else(|| app.now_playing_tint()));
    let id = Id::new("home-wash");
    let dt = ui.input(|input| input.stable_dt).min(0.1);
    let current = ui
        .data(|data| data.get_temp::<egui::Rgba>(id))
        .unwrap_or(target);
    let color = match ease_toward(current, target, dt) {
        Some(next) => {
            ui.ctx().request_repaint();
            next
        }
        None => target,
    };
    ui.data_mut(|data| data.insert_temp(id, color));
    if color.a() <= 0.0 {
        return;
    }
    let page = ui.max_rect();
    let rect = Rect::from_min_max(
        pos2(page.left() - widgets::PAGE_PADDING, top - 4.0),
        pos2(
            page.right() + widgets::PAGE_PADDING,
            top - 4.0 + WASH_HEIGHT,
        ),
    );
    ui.painter().set(
        slot,
        widgets::vertical_gradient(rect, Color32::from(color), Color32::TRANSPARENT),
    );
}

struct Tile {
    image: Option<String>,
    name: String,
    page: Page,
    uri: Option<String>,
    liked: bool,
    owned_playlist: Option<Playlist>,
    /// A song played on its own, which the tile plays and whose menu it
    /// offers, rather than a playlist, album or artist.
    song: Option<crate::api::models::Track>,
}

impl Tile {
    fn podcast(&self) -> bool {
        matches!(self.page, Page::Show(_))
    }
}

/// The height of a shortcut, and so the side of its cover, as Spotify's.
const TILE_HEIGHT: f32 = 48.0;
/// The space between shortcuts.
const TILE_GAP: f32 = 8.0;
/// How many quick-access tiles Home shows, as Spotify does.
const TILES: usize = 8;

/// What was played most recently, as Spotify's Home shows it: the playlists,
/// albums, artists and podcasts songs were played from, Liked Songs, and
/// songs played on their own, newest first (#942). Until Spotify's play
/// history arrives, or when it holds fewer, Liked Songs and the first
/// playlists in the Library fill the rest. Under the Podcasts chip only
/// podcasts are shortcuts, and saved ones fill the rest; under Music, no
/// podcast is.
fn quick_access_tiles(app: &App, filter: Filter) -> Vec<Tile> {
    let tile_art = app.art_px(TILE_HEIGHT);
    let collection = app
        .user
        .as_ref()
        .map(|user| format!("spotify:user:{}:collection", user.id));
    let liked_tile = || Tile {
        image: None,
        name: gettext(app.locale, "Liked Songs").into_owned(),
        page: Page::LikedSongs,
        uri: collection.clone(),
        liked: true,
        owned_playlist: None,
        song: None,
    };
    let playlist_tile = |playlist: &Playlist| Tile {
        image: pick_image(&playlist.images, tile_art).map(str::to_string),
        name: playlist.name.clone(),
        page: Page::Playlist(playlist.id.clone()),
        uri: Some(playlist.uri.clone()),
        liked: false,
        owned_playlist: app
            .user_id()
            .is_some_and(|id| playlist.owned_by(id))
            .then(|| playlist.clone()),
        song: None,
    };
    let mut tiles: Vec<Tile> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for play in app.home.recently_played.get().into_iter().flatten() {
        if tiles.len() == TILES {
            break;
        }
        let track = &play.track;
        let context = play.context.as_ref().map(|context| context.uri.as_str());
        let tile = match context.map(|uri| (uri, uri.rsplit(':').next().unwrap_or(""))) {
            Some((uri, _)) if uri.ends_with(":collection") => Some(liked_tile()),
            Some((uri, id)) if uri.starts_with("spotify:playlist:") => {
                app.known_playlist(id).map(playlist_tile)
            }
            Some((uri, id)) if uri.starts_with("spotify:album:") => app
                .known_album(id)
                .or(track.album.as_ref().filter(|album| album.id == id))
                .map(|album| Tile {
                    image: pick_image(&album.images, tile_art).map(str::to_string),
                    name: album.name.clone(),
                    page: Page::Album(album.id.clone()),
                    uri: Some(uri.to_string()),
                    liked: false,
                    owned_playlist: None,
                    song: None,
                }),
            Some((uri, id)) if uri.starts_with("spotify:artist:") => {
                app.known_artist(id).map(|artist| Tile {
                    image: pick_image(&artist.images, tile_art).map(str::to_string),
                    name: artist.name.clone(),
                    page: Page::Artist(artist.id.clone()),
                    uri: Some(uri.to_string()),
                    liked: false,
                    owned_playlist: None,
                    song: None,
                })
            }
            Some((uri, id)) if uri.starts_with("spotify:show:") => {
                app.known_show(id).map(|show| Tile {
                    image: pick_image(&show.images, tile_art).map(str::to_string),
                    name: show.name.clone(),
                    page: Page::Show(show.id.clone()),
                    uri: Some(uri.to_string()),
                    liked: false,
                    owned_playlist: None,
                    song: None,
                })
            }
            // A song played on its own, or from somewhere Home cannot name.
            _ => track
                .album
                .as_ref()
                .filter(|album| !album.id.is_empty())
                .map(|album| Tile {
                    image: track.image(tile_art).map(str::to_string),
                    name: track.name.clone(),
                    page: Page::Album(album.id.clone()),
                    uri: Some(track.uri.clone()),
                    liked: false,
                    owned_playlist: None,
                    song: Some(track.clone()),
                }),
        };
        if let Some(tile) = tile
            && filter.admits(tile.podcast())
            && seen.insert(tile.page.clone())
        {
            tiles.push(tile);
        }
    }
    let fill: Vec<Tile> = if filter == Filter::Podcasts {
        app.home
            .podcasts
            .iter()
            .map(|(show, _)| Tile {
                image: pick_image(&show.images, tile_art).map(str::to_string),
                name: show.name.clone(),
                page: Page::Show(show.id.clone()),
                uri: Some(show.uri.clone()),
                liked: false,
                owned_playlist: None,
                song: None,
            })
            .collect()
    } else {
        let library = app.library.playlists.get().into_iter().flatten();
        std::iter::once(liked_tile())
            .chain(library.map(playlist_tile))
            .collect()
    };
    for tile in fill {
        if tiles.len() == TILES {
            break;
        }
        if seen.insert(tile.page.clone()) {
            tiles.push(tile);
        }
    }
    tiles
}

/// How many shortcuts share a row: four, as Spotify's, until a row of four
/// would leave too little room for their titles.
fn shortcut_columns(width: f32) -> usize {
    ((width / 220.0).floor() as usize).clamp(2, 4)
}

/// Paints `galley` in `rect`, aligned as its text asks.
fn paint_galley(ui: &Ui, rect: Rect, galley: Arc<egui::Galley>, color: Color32) {
    let pos = match galley.job.halign {
        egui::Align::RIGHT => pos2(rect.right(), rect.top()),
        egui::Align::Center => pos2(rect.center().x, rect.top()),
        _ => rect.min,
    };
    ui.painter().galley(pos, galley, color);
}

/// A shortcut's play control: shown while the shortcut is hovered, fading
/// in as Spotify's does. True when it was pressed.
fn hover_play(
    app: &App,
    ui: &mut Ui,
    center: egui::Pos2,
    size: f32,
    shown: f32,
    playing: bool,
) -> bool {
    let palette = app.palette;
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(Rect::from_center_size(center, Vec2::splat(size)))
            .layout(egui::Layout::centered_and_justified(
                egui::Direction::LeftToRight,
            )),
    );
    child.multiply_opacity(shown);
    theme::circle_button(
        &mut child,
        if playing {
            Icon::PauseFilled
        } else {
            Icon::PlayFilled
        },
        size,
        palette.accent,
        palette.accent_hover,
        palette.on_accent,
        &gettext(app.locale, if playing { "Pause" } else { "Play" }),
    )
    .clicked()
}

/// The shortcut grid; returns the colour of the hovered shortcut's art, for
/// the wash behind it.
fn quick_access(app: &mut App, ui: &mut Ui, filter: Filter) -> Option<Color32> {
    let tiles = quick_access_tiles(app, filter);
    let available = ui.available_width();
    let columns = shortcut_columns(available);
    let tile_width = (available - TILE_GAP * (columns as f32 - 1.0)) / columns as f32;
    let rows = tiles.len().div_ceil(columns);
    let playing_uri = app.playing_context_uri();
    let believed_playing = app.believed_playing();
    let mut hovered_tint = None;
    for row in 0..rows {
        if row > 0 {
            ui.add_space(TILE_GAP - ui.spacing().item_spacing.y);
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = TILE_GAP;
            for column in 0..columns {
                let Some(tile) = tiles.get(row * columns + column) else {
                    break;
                };
                // Asked for every shortcut, so the wash's colour is there
                // by the time one is hovered.
                let tint = if tile.liked {
                    Some(LIKED_TINT)
                } else {
                    app.tint_for(tile.image.as_deref())
                };
                if shortcut(app, ui, tile, tile_width, &playing_uri, believed_playing) {
                    hovered_tint = tint;
                }
            }
        });
    }
    hovered_tint
}

/// One shortcut: its cover, its title on up to two lines, Play on hover
/// and Spotify's green bars while it plays. True while it is hovered.
fn shortcut(
    app: &mut App,
    ui: &mut Ui,
    tile: &Tile,
    width: f32,
    playing_uri: &Option<String>,
    believed_playing: bool,
) -> bool {
    let palette = app.palette;
    let Tile {
        image,
        name,
        page,
        uri,
        liked,
        owned_playlist,
        song,
    } = tile;
    let (rect, response) = ui.allocate_exact_size(vec2(width, TILE_HEIGHT), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
    let hovered = ui.rect_contains_pointer(rect);
    let context_here = uri.is_some() && playing_uri.as_deref() == uri.as_deref();
    let playing_here = context_here && believed_playing;
    let shown = ui.ctx().animate_bool_with_time_and_easing(
        response.id.with("play"),
        hovered && uri.is_some(),
        0.2,
        egui::emath::easing::cubic_out,
    );
    if ui.is_rect_visible(rect) {
        // Spotify's shortcuts are translucent, so the wash shows through.
        let fill = palette.text.gamma_multiply(if hovered { 0.2 } else { 0.1 });
        let corner = CornerRadius::same(theme::RADIUS_SMALL);
        ui.painter().rect_filled(rect, corner, fill);
        let cover = Rect::from_min_size(rect.min, Vec2::splat(TILE_HEIGHT));
        if *liked {
            super::sidebar::liked_cover(ui, cover, f32::from(theme::RADIUS_SMALL));
        } else {
            widgets::paint_cover(
                ui,
                &palette,
                image.as_deref(),
                cover,
                f32::from(theme::RADIUS_SMALL),
                if tile.podcast() {
                    Icon::Mic
                } else {
                    Icon::Music
                },
                Some(app.backend.art()),
            );
        }
        let end_room = if shown > 0.0 {
            48.0
        } else if playing_here {
            40.0
        } else {
            8.0
        };
        let text_left = cover.right() + 8.0;
        let text_width = (rect.right() - end_room - text_left).max(0.0);
        let galley = widgets::ellipsized(ui, name, theme::bold(14.0), palette.text, text_width, 2);
        let text_rect = Rect::from_min_size(
            pos2(text_left, rect.center().y - galley.size().y / 2.0),
            vec2(text_width, galley.size().y),
        );
        paint_galley(ui, text_rect, galley, palette.text);
        if playing_here && shown < 1.0 {
            let bars = Rect::from_center_size(
                pos2(rect.right() - 24.0, rect.center().y),
                Vec2::splat(16.0),
            );
            theme::paint_icon(
                ui,
                Icon::AudioLines,
                bars,
                16.0,
                palette.accent.gamma_multiply(1.0 - shown),
            );
        }
    }
    if shown > 0.0
        && let Some(uri) = uri
        && hover_play(
            app,
            ui,
            pos2(rect.right() - 24.0, rect.center().y),
            32.0,
            shown,
            playing_here,
        )
    {
        if context_here {
            app.actions.push(Action::TogglePlay);
        } else if song.is_some() {
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
    } else if response.clicked() {
        app.actions.push(Action::Open(page.clone()));
    }
    theme::focus_ring(ui, &response);
    if !liked && let Some(uri) = uri {
        egui::Popup::context_menu(&response)
            .id(ui.make_persistent_id(("quick-access-menu", uri)))
            .frame(widgets::menu_frame(&palette))
            .show(|ui| match song {
                Some(track) => {
                    widgets::item_menu(ui, app, &PlayableItem::Track(track.clone()), None, None);
                }
                None => widgets::context_menu_items(ui, app, uri, name, owned_playlist.as_ref()),
            });
    }
    hovered
}

/// A Home card's cover, inside its 12 point padding, as Spotify's.
const CARD_ART: f32 = 160.0;
const CARD_PAD: f32 = 12.0;
const CARD_WIDTH: f32 = CARD_ART + 2.0 * CARD_PAD;
/// The play control on a hovered card's cover.
const CARD_PLAY: f32 = 48.0;

/// What a Home [`card`] shows.
struct Card<'a> {
    image: Option<&'a str>,
    title: &'a str,
    subtitle: &'a str,
    /// A round cover, as an artist's portrait.
    round: bool,
    /// The card's context is the one playing: its control stays in sight
    /// and pauses.
    playing: bool,
}

/// A card in Spotify's style: the cover, the title on one line and the
/// subtitle on two, a lighter background on hover, and the green play
/// button sliding up into the cover's corner.
fn card(ui: &mut Ui, app: &mut App, card: &Card<'_>) -> CardResponse {
    let palette = app.palette;
    let title_font = theme::regular(15.0);
    let subtitle_font = theme::regular(13.5);
    let (title_row, subtitle_row) = ui.fonts_mut(|fonts| {
        (
            fonts.row_height(&title_font),
            fonts.row_height(&subtitle_font),
        )
    });
    let height = CARD_PAD + CARD_ART + 8.0 + title_row + 2.0 + 2.0 * subtitle_row + CARD_PAD;
    let (rect, response) = ui.allocate_exact_size(vec2(CARD_WIDTH, height), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            format!("{}, {}", card.title, card.subtitle),
        )
    });
    if response.gained_focus() {
        response.scroll_to_me(None);
    }
    let hovered = ui.rect_contains_pointer(rect);
    let shown = ui.ctx().animate_bool_with_time_and_easing(
        response.id.with("play"),
        hovered || response.has_focus() || card.playing,
        0.2,
        egui::emath::easing::cubic_out,
    );
    let art = Rect::from_min_size(rect.min + Vec2::splat(CARD_PAD), Vec2::splat(CARD_ART));
    if ui.is_rect_visible(rect) {
        if hovered {
            ui.painter().rect_filled(
                rect,
                CornerRadius::same(theme::RADIUS_SMALL + 2),
                palette.surface,
            );
        }
        let radius = if card.round { CARD_ART / 2.0 } else { 6.0 };
        widgets::paint_shadow(ui, &palette, art, radius);
        widgets::paint_cover(
            ui,
            &palette,
            card.image,
            art,
            radius,
            if card.round { Icon::User } else { Icon::Music },
            Some(app.backend.art()),
        );
        let title = widgets::ellipsized(ui, card.title, title_font, palette.text, CARD_ART, 1);
        let title_rect = Rect::from_min_size(
            pos2(art.left(), art.bottom() + 8.0),
            vec2(CARD_ART, title_row),
        );
        paint_galley(ui, title_rect, title, palette.text);
        let subtitle = widgets::ellipsized(
            ui,
            card.subtitle,
            subtitle_font,
            palette.secondary,
            CARD_ART,
            2,
        );
        let subtitle_rect = Rect::from_min_size(
            pos2(art.left(), title_rect.bottom() + 2.0),
            vec2(CARD_ART, 2.0 * subtitle_row),
        );
        paint_galley(ui, subtitle_rect, subtitle, palette.secondary);
    }
    let mut play = false;
    if shown > 0.0 && ui.is_rect_visible(art) {
        // Spotify's button rises 8 points into place as it fades in.
        let center = pos2(
            art.right() - 8.0 - CARD_PLAY / 2.0,
            art.bottom() - 8.0 - CARD_PLAY / 2.0 + 8.0 * (1.0 - shown),
        );
        let shadow = egui::epaint::Shadow {
            offset: [0, 8],
            blur: 8,
            spread: 0,
            color: Color32::from_black_alpha((77.0 * shown) as u8),
        };
        ui.painter().add(shadow.as_shape(
            Rect::from_center_size(center, Vec2::splat(CARD_PLAY)),
            CornerRadius::same((CARD_PLAY / 2.0) as u8),
        ));
        play = hover_play(app, ui, center, CARD_PLAY, shown, card.playing);
    }
    crate::autoscroll::row(ui, &response);
    theme::focus_ring(ui, &response);
    CardResponse {
        clicked: response.clicked() && !play,
        response,
        play,
    }
}

/// What a shelf's header was asked to do.
struct ShelfClicks {
    /// The title, which opens the shelf's page when it has one.
    title: bool,
    /// Show all, which opens that page, or else lays every card out.
    show_all: bool,
    /// Where the title was drawn, for a badge beside it.
    title_rect: Rect,
}

/// A shelf's place on Home.
struct ShelfSpec<'a> {
    id: &'a str,
    title: &'a str,
    /// The shelf has a page of its own, which its title and Show all open.
    linked: bool,
    /// How many cards it holds, to know whether they overflow a row.
    cards: usize,
    /// The least height of its header, to line it up with a neighbour's.
    header_height: f32,
}

impl<'a> ShelfSpec<'a> {
    fn new(id: &'a str, title: &'a str, cards: usize) -> Self {
        Self {
            id,
            title,
            linked: false,
            cards,
            header_height: 0.0,
        }
    }

    fn linked(self) -> Self {
        Self {
            linked: true,
            ..self
        }
    }
}

/// A section's header, as Spotify's: the title in 24 point bold, underlined
/// on hover when it opens a page, and an action such as Show all at the
/// end of the row.
fn section_header(
    ui: &mut Ui,
    palette: &Palette,
    id: &str,
    title: &str,
    linked: bool,
    action: Option<&str>,
    min_height: f32,
) -> ShelfClicks {
    let painter = ui.painter().clone();
    let width = ui.available_width();
    let action_galley = action.map(|label| {
        crate::bidi::layout_line(&painter, label, theme::bold(14.0), palette.secondary)
    });
    let action_width = action_galley
        .as_ref()
        .map_or(0.0, |galley| galley.size().x + 16.0);
    let title_galley = widgets::ellipsized(
        ui,
        title,
        theme::bold(SECTION_TITLE),
        palette.text,
        (width - action_width).max(0.0),
        1,
    );
    let height = title_galley.size().y.max(min_height);
    let (row, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let title_rect = Rect::from_min_size(
        pos2(row.left(), row.bottom() - title_galley.size().y),
        title_galley.size(),
    );
    let mut clicks = ShelfClicks {
        title: false,
        show_all: false,
        title_rect,
    };
    if linked {
        let response = ui.interact(
            title_rect,
            ui.make_persistent_id(("home-shelf-title", id)),
            Sense::click(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Link, ui.is_enabled(), title)
        });
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            underline(&painter, title_rect, palette.text);
        }
        theme::focus_ring(ui, &response);
        clicks.title = response.clicked();
    }
    painter.galley(title_rect.min, title_galley, palette.text);
    if let (Some(label), Some(galley)) = (action, action_galley) {
        let rect = Rect::from_min_size(
            pos2(
                row.right() - galley.size().x,
                title_rect.center().y - galley.size().y / 2.0,
            ),
            galley.size(),
        );
        let response = ui.interact(
            rect,
            ui.make_persistent_id(("home-shelf-action", id)),
            Sense::click(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
        });
        let color = if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            underline(&painter, rect, palette.text);
            palette.text
        } else {
            palette.secondary
        };
        painter.galley_with_override_text_color(rect.min, galley, color);
        theme::focus_ring(ui, &response);
        clicks.show_all = response.clicked();
    }
    clicks
}

fn underline(painter: &egui::Painter, rect: Rect, color: Color32) {
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        egui::Stroke::new(1.0, color),
    );
}

/// A shelf of cards under its header. Its cards stay on one row that
/// scrolls sideways; Show all opens the shelf's page, or, for a shelf
/// without one, lays every card out in rows until Show less.
fn shelf(
    ui: &mut Ui,
    palette: &Palette,
    locale: Locale,
    spec: &ShelfSpec<'_>,
    add_contents: impl FnOnce(&mut Ui),
) -> ShelfClicks {
    ui.add_space(SECTION_GAP);
    let expanded_id = Id::new(("home-shelf-expanded", spec.id));
    let expanded = !spec.linked && ui.data(|data| data.get_temp::<bool>(expanded_id)) == Some(true);
    let overflows = spec.cards as f32 * CARD_WIDTH > ui.available_width() + 2.0 * CARD_PAD;
    let show_all = gettext(locale, "Show all");
    let show_less = gettext(locale, "Show less");
    let action = if expanded {
        Some(show_less.as_ref())
    } else if spec.linked || overflows {
        Some(show_all.as_ref())
    } else {
        None
    };
    let clicks = section_header(
        ui,
        palette,
        spec.id,
        spec.title,
        spec.linked,
        action,
        spec.header_height,
    );
    if clicks.show_all && !spec.linked {
        ui.data_mut(|data| data.insert_temp(expanded_id, !expanded));
    }
    ui.add_space(8.0);
    // The cards' padding sits outside the page's edge, so their covers line
    // up with the title, as Spotify's do.
    let body = ui.available_rect_before_wrap();
    let body = Rect::from_min_max(
        pos2(body.left() - CARD_PAD, body.top()),
        pos2(body.right() + CARD_PAD, body.bottom()),
    );
    let mut body = ui.new_child(
        UiBuilder::new()
            .max_rect(body)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    {
        let ui = &mut body;
        if expanded {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                add_contents(ui);
            });
        } else {
            crate::autoscroll::show(
                ui,
                egui::ScrollArea::horizontal().id_salt(spec.id),
                egui::Vec2b::new(true, false),
                |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        add_contents(ui);
                    });
                },
            );
        }
    }
    // Only the page's own width is taken, so the page does not widen.
    let used = body.min_rect();
    let page = ui.max_rect();
    ui.advance_cursor_after_rect(Rect::from_min_max(
        pos2(page.left(), used.top()),
        pos2(page.right(), used.bottom()),
    ));
    clicks
}

/// Renders session Home cards or approximate fallback cards in the same shelf.
fn made_for_you(app: &mut App, ui: &mut Ui) {
    let palette = app.palette;
    let (playlists, loading, failure) = match &app.home.made_for_you {
        Loadable::Loaded(playlists) => (playlists.clone(), false, None),
        Loadable::Loading | Loadable::NotLoaded => (Vec::new(), true, None),
        Loadable::Failed(error) => (Vec::new(), false, Some(error.clone())),
    };
    if playlists.is_empty() && !loading && failure.is_none() {
        return;
    }
    let title = gettext(app.locale, "Made for you");
    let cards = if loading { 7 } else { playlists.len() };
    shelf(
        ui,
        &palette,
        app.locale,
        &ShelfSpec::new("made-for-you", &title, cards),
        |ui| {
            if playlists.is_empty() && loading {
                widgets::skeleton_cards(ui, &palette, app.locale, 7, false);
            } else if playlists.is_empty()
                && let Some(message) = &failure
            {
                widgets::error_row(ui, app, message, Some(Page::Home));
            }
            playlist_cards(app, ui, "home-made_for_you-menu", &playlists);
        },
    );
}

fn top_mixes(app: &mut App, ui: &mut Ui) {
    let palette = app.palette;
    let Some(playlists) = app
        .home
        .top_mixes
        .get()
        .filter(|playlists| !playlists.is_empty())
        .cloned()
    else {
        return;
    };
    let title = gettext(app.locale, "Your top mixes");
    shelf(
        ui,
        &palette,
        app.locale,
        &ShelfSpec::new("top-mixes", &title, playlists.len()),
        |ui| playlist_cards(app, ui, "home-top_mixes-menu", &playlists),
    );
}

/// Where the hero for the newest release sits beside the New releases
/// shelf instead of above it.
const HERO_BESIDE_WIDTH: f32 = 860.0;
/// The space between the hero and the shelf beside it.
const HERO_GAP: f32 = 24.0;
/// The height of the hero's card, and so the side of its cover.
const HERO_HEIGHT: f32 = 200.0;
/// The height of the hero's header: the artist's portrait.
const HERO_HEADER: f32 = 48.0;

/// The latest releases from followed artists: the newest in Spotify's
/// "New release from" block, then the shelf of them all. A card opens its
/// release; the title and Show all open the page that plays all their
/// songs, newest first.
fn new_releases(app: &mut App, ui: &mut Ui) {
    let (albums, loading, failure) = match &app.home.new_releases {
        Loadable::Loaded(albums) => (albums.clone(), false, None),
        Loadable::Loading | Loadable::NotLoaded => (Vec::new(), true, None),
        Loadable::Failed(error) => (Vec::new(), false, Some(error.clone())),
    };
    if albums.is_empty() && !loading && failure.is_none() {
        return;
    }
    let Some(newest) = albums.first().cloned() else {
        release_shelf(app, ui, &albums, loading, failure.as_deref(), 0.0);
        return;
    };
    if ui.available_width() >= HERO_BESIDE_WIDTH {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = HERO_GAP;
            let half = (ui.available_width() - HERO_GAP) / 2.0;
            let column = |ui: &mut Ui, add: &mut dyn FnMut(&mut Ui)| {
                ui.allocate_ui_with_layout(
                    vec2(half, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(half);
                        add(ui);
                    },
                );
            };
            column(ui, &mut |ui| release_hero(app, ui, &newest));
            column(ui, &mut |ui| {
                release_shelf(app, ui, &albums, loading, failure.as_deref(), HERO_HEADER);
            });
        });
    } else {
        release_hero(app, ui, &newest);
        release_shelf(app, ui, &albums, loading, failure.as_deref(), 0.0);
    }
}

fn release_shelf(
    app: &mut App,
    ui: &mut Ui,
    albums: &[Album],
    loading: bool,
    failure: Option<&str>,
    header_height: f32,
) {
    let palette = app.palette;
    let title = gettext(app.locale, "New releases");
    let spec = ShelfSpec {
        header_height,
        ..ShelfSpec::new("new-releases", &title, albums.len()).linked()
    };
    let clicks = shelf(ui, &palette, app.locale, &spec, |ui| {
        if albums.is_empty() && loading {
            widgets::loading_row(ui, &palette, app.locale);
        } else if albums.is_empty()
            && let Some(message) = failure
        {
            widgets::error_row(ui, app, message, Some(Page::Home));
        }
        for album in albums {
            release_card(app, ui, album);
        }
    });
    super::new_releases::shelf_badge(app, ui, clicks.title_rect);
    if clicks.title || clicks.show_all {
        app.actions.push(Action::Open(Page::NewReleases));
    }
}

/// "Kind • Artists", as a release's card and hero say what it is.
fn release_subtitle(locale: Locale, album: &Album) -> String {
    let artists =
        crate::api::models::join_names(album.artists.iter().map(|artist| artist.name.as_str()));
    if artists.is_empty() {
        album.kind_label(locale).into_owned()
    } else {
        format!("{} • {artists}", album.kind_label(locale))
    }
}

/// Spotify's "New release from" block: the artist's portrait and name over
/// a card with the release's cover, what it is, its title, Play and Save.
fn release_hero(app: &mut App, ui: &mut Ui, album: &Album) {
    let palette = app.palette;
    let locale = app.locale;
    ui.add_space(SECTION_GAP);
    let artist = album.artists.first();
    let artist_id = artist.and_then(|artist| artist.id.clone());
    let portrait = artist_id
        .as_deref()
        .and_then(|id| app.known_artist(id))
        .and_then(|artist| pick_image(&artist.images, app.art_px(HERO_HEADER)))
        .map(str::to_string);
    let width = ui.available_width();
    let (header, _) = ui.allocate_exact_size(vec2(width, HERO_HEADER), Sense::hover());
    let face = Rect::from_min_size(header.min, Vec2::splat(HERO_HEADER));
    if artist.is_some() {
        widgets::paint_cover(
            ui,
            &palette,
            portrait.as_deref(),
            face,
            HERO_HEADER / 2.0,
            Icon::User,
            Some(app.backend.art()),
        );
    }
    let text_left = if artist.is_some() {
        face.right() + 12.0
    } else {
        header.left()
    };
    let text_width = (header.right() - text_left).max(0.0);
    let from = widgets::ellipsized(
        ui,
        &gettext(locale, "New release from"),
        theme::regular(13.0),
        palette.secondary,
        text_width,
        1,
    );
    let name = widgets::ellipsized(
        ui,
        artist.map_or("", |artist| artist.name.as_str()),
        theme::bold(SECTION_TITLE),
        palette.text,
        text_width,
        1,
    );
    let block = from.size().y + name.size().y;
    let from_rect = Rect::from_min_size(
        pos2(text_left, header.center().y - block / 2.0),
        vec2(text_width, from.size().y),
    );
    let name_rect = Rect::from_min_size(
        pos2(text_left, from_rect.bottom()),
        vec2(name.size().x.min(text_width), name.size().y),
    );
    paint_galley(ui, from_rect, from, palette.secondary);
    if let Some(id) = &artist_id {
        let response = ui.interact(
            face.union(name_rect),
            ui.make_persistent_id(("home-hero-artist", id)),
            Sense::click(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Link,
                true,
                artist.map_or("", |artist| artist.name.as_str()),
            )
        });
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            underline(ui.painter(), name_rect, palette.text);
        }
        theme::focus_ring(ui, &response);
        if response.clicked() {
            app.actions.push(Action::Open(Page::Artist(id.clone())));
        }
    }
    paint_galley(ui, name_rect, name, palette.text);
    ui.add_space(8.0);

    // The card itself.
    let side = HERO_HEIGHT.min(width * 0.45);
    let (rect, response) = ui.allocate_exact_size(vec2(width, side), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &album.name));
    let hovered = ui.rect_contains_pointer(rect);
    let context_here = app.playing_context_uri().as_deref() == Some(album.uri.as_str());
    let playing_here = context_here && app.believed_playing();
    let cover = Rect::from_min_size(rect.min, Vec2::splat(side));
    let info_left = cover.right() + 16.0;
    let info_width = (rect.right() - 16.0 - info_left).max(0.0);
    let kind = widgets::ellipsized(
        ui,
        &release_subtitle(locale, album),
        theme::regular(13.5),
        palette.secondary,
        info_width,
        1,
    );
    let title = widgets::ellipsized(
        ui,
        &album.name,
        theme::bold(if side < HERO_HEIGHT { 18.0 } else { 22.0 }),
        palette.text,
        info_width,
        2,
    );
    const BUTTONS: f32 = 32.0;
    let block = kind.size().y + 4.0 + title.size().y + 16.0 + BUTTONS;
    let kind_rect = Rect::from_min_size(
        pos2(info_left, rect.center().y - block / 2.0),
        vec2(info_width, kind.size().y),
    );
    let title_rect = Rect::from_min_size(
        pos2(info_left, kind_rect.bottom() + 4.0),
        vec2(info_width, title.size().y),
    );
    let buttons_top = title_rect.bottom() + 16.0;
    if ui.is_rect_visible(rect) {
        let fill = if hovered {
            palette.surface_hover
        } else {
            palette.surface
        };
        ui.painter()
            .rect_filled(rect, CornerRadius::same(theme::RADIUS), fill);
        widgets::paint_cover(
            ui,
            &palette,
            pick_image(&album.images, app.art_px(side)),
            cover,
            f32::from(theme::RADIUS),
            Icon::Disc,
            Some(app.backend.art()),
        );
        paint_galley(ui, kind_rect, kind, palette.secondary);
        paint_galley(ui, title_rect, title, palette.text);
    }
    // Spotify's hero plays with a white button, not the green one.
    let mut play = ui.new_child(
        UiBuilder::new()
            .max_rect(Rect::from_min_size(
                pos2(info_left, buttons_top),
                Vec2::splat(BUTTONS),
            ))
            .layout(egui::Layout::centered_and_justified(
                egui::Direction::LeftToRight,
            )),
    );
    let played = theme::circle_button(
        &mut play,
        if playing_here {
            Icon::PauseFilled
        } else {
            Icon::PlayFilled
        },
        BUTTONS,
        palette.text,
        palette.text,
        palette.window,
        &gettext(locale, if playing_here { "Pause" } else { "Play" }),
    )
    .clicked();
    let saved = app.is_saved(&album.uri) == Some(true);
    let mut save = ui.new_child(
        UiBuilder::new()
            .max_rect(Rect::from_min_size(
                pos2(info_left + BUTTONS + 12.0, buttons_top),
                Vec2::splat(BUTTONS),
            ))
            .layout(egui::Layout::centered_and_justified(
                egui::Direction::LeftToRight,
            )),
    );
    let toggled = theme::icon_button(
        &mut save,
        if saved {
            Icon::CircleCheck
        } else {
            Icon::CirclePlus
        },
        22.0,
        if saved {
            palette.accent
        } else {
            palette.secondary
        },
        if saved {
            palette.accent_hover
        } else {
            palette.text
        },
        &gettext(
            locale,
            if saved {
                "Remove from Your Library"
            } else {
                "Save to Your Library"
            },
        ),
    )
    .clicked();
    if played {
        if context_here {
            app.actions.push(Action::TogglePlay);
        } else {
            app.actions.push(Action::PlayContext {
                uri: album.uri.clone(),
                offset_uri: None,
                offset_index: None,
            });
        }
    } else if toggled {
        app.actions.push(Action::ToggleSaved(album.uri.clone()));
    } else if response.clicked() {
        app.actions
            .push(Action::Open(Page::Album(album.id.clone())));
    }
    theme::focus_ring(ui, &response);
    egui::Popup::context_menu(&response)
        .id(ui.make_persistent_id(("home-new-release-hero-menu", &album.uri)))
        .frame(widgets::menu_frame(&palette))
        .show(|ui| {
            widgets::context_menu_items(ui, app, &album.uri, &album.name, None);
        });
}

fn release_card(app: &mut App, ui: &mut Ui, album: &Album) {
    let palette = app.palette;
    let subtitle = release_subtitle(app.locale, album);
    let context_here = app.playing_context_uri().as_deref() == Some(album.uri.as_str());
    let playing_here = context_here && app.believed_playing();
    let card_art = app.art_px(CARD_ART);
    let card = card(
        ui,
        app,
        &Card {
            image: pick_image(&album.images, card_art),
            title: &album.name,
            subtitle: &subtitle,
            round: false,
            playing: playing_here,
        },
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
        .id(ui.make_persistent_id(("home-new-releases-menu", &album.uri)))
        .frame(widgets::menu_frame(&palette))
        .show(|ui| {
            widgets::context_menu_items(ui, app, &album.uri, &album.name, None);
        });
}

fn playlist_cards(app: &mut App, ui: &mut Ui, menu: &str, playlists: &[Playlist]) {
    let palette = app.palette;
    for playlist in playlists {
        let subtitle = playlist
            .description
            .as_deref()
            .map(crate::util::strip_html)
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| {
                // Translators: {owner} is the name of the playlist's owner.
                gettext(app.locale, "By {owner}").replace("{owner}", playlist.owner_name())
            });
        let context_here = app.playing_context_uri().as_deref() == Some(playlist.uri.as_str());
        let playing_here = context_here && app.believed_playing();
        let card_art = app.art_px(CARD_ART);
        let card = card(
            ui,
            app,
            &Card {
                image: pick_image(&playlist.images, card_art),
                title: &playlist.name,
                subtitle: &subtitle,
                round: false,
                playing: playing_here,
            },
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
            .id(ui.make_persistent_id((menu, &playlist.uri)))
            .frame(widgets::menu_frame(&palette))
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
}

fn recently_played(app: &mut App, ui: &mut Ui) {
    let palette = app.palette;
    let title = gettext(app.locale, "Recently played");
    let history = match app.home.recently_played.clone() {
        Loadable::Loaded(history) => history,
        Loadable::Loading | Loadable::NotLoaded => {
            shelf(
                ui,
                &palette,
                app.locale,
                &ShelfSpec::new("recent", &title, 0),
                |ui| widgets::skeleton_cards(ui, &palette, app.locale, 7, false),
            );
            return;
        }
        Loadable::Failed(message) => {
            shelf(
                ui,
                &palette,
                app.locale,
                &ShelfSpec::new("recent", &title, 0),
                |ui| {
                    widgets::error_row(ui, app, &message, Some(Page::Home));
                },
            );
            return;
        }
    };
    let mut seen = std::collections::HashSet::new();
    let tracks: Vec<_> = history
        .into_iter()
        .filter(|entry| {
            entry
                .track
                .id
                .as_ref()
                .is_some_and(|id| seen.insert(id.clone()))
        })
        .take(16)
        .collect();
    if tracks.is_empty() {
        return;
    }
    shelf(
        ui,
        &palette,
        app.locale,
        &ShelfSpec::new("recent", &title, tracks.len()),
        |ui| {
            for entry in &tracks {
                let track = &entry.track;
                let card_art = app.art_px(CARD_ART);
                let artists = track.artist_names();
                let card = card(
                    ui,
                    app,
                    &Card {
                        image: track.image(card_art),
                        title: &track.name,
                        subtitle: &artists,
                        round: false,
                        playing: false,
                    },
                );
                if card.play {
                    app.actions.push(Action::PlayUris {
                        uris: vec![track.uri.clone()],
                        index: 0,
                    });
                }
                if card.clicked
                    && let Some(album) = &track.album
                    && !album.id.is_empty()
                {
                    app.actions
                        .push(Action::Open(Page::Album(album.id.clone())));
                }
                egui::Popup::context_menu(&card.response)
                    .id(ui.make_persistent_id(("home-recently_played-menu", &track.uri)))
                    .frame(widgets::menu_frame(&palette))
                    .show(|ui| {
                        widgets::item_menu(
                            ui,
                            app,
                            &PlayableItem::Track(track.clone()),
                            None,
                            None,
                        );
                    });
            }
        },
    );
}

/// Why an episode is on the podcast shelf.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EpisodeReason {
    /// Started and not finished, with this much left.
    Continue { left_ms: u32 },
    /// The show's newest episode, recently released and not yet started.
    New,
}

/// How many days after its release an unplayed episode still counts as new.
const NEW_EPISODE_DAYS: i64 = 30;
/// The podcast shelf's card limit, as for Recently played.
const PODCAST_CARDS: usize = 16;

/// The episodes on the podcast shelf: those to continue first, then each
/// show's newest unstarted episode from the last month, each group newest
/// first. `skip` leaves out shows that are audiobooks or no longer saved.
pub(crate) fn podcast_episodes(
    podcasts: &[(Show, Vec<Episode>)],
    skip: impl Fn(&Show) -> bool,
    today: jiff::civil::Date,
) -> Vec<(Show, Episode, EpisodeReason)> {
    let oldest_new = today
        .checked_sub(jiff::Span::new().days(NEW_EPISODE_DAYS))
        .unwrap_or(today);
    let released = |episode: &Episode| {
        episode
            .release_date
            .as_deref()
            .and_then(|date| date.get(..10))
            .and_then(|date| date.parse::<jiff::civil::Date>().ok())
    };
    let mut continuing = Vec::new();
    let mut new = Vec::new();
    for (show, episodes) in podcasts {
        if skip(show) {
            continue;
        }
        for episode in episodes {
            if let Some(resume) = &episode.resume_point
                && !resume.fully_played
                && resume.resume_position_ms > 0
            {
                let left_ms = episode
                    .duration_ms
                    .saturating_sub(resume.resume_position_ms);
                continuing.push((show, episode, EpisodeReason::Continue { left_ms }));
            }
        }
        // Spotify lists a show's episodes newest first.
        if let Some(newest) = episodes.first()
            && newest
                .resume_point
                .as_ref()
                .is_none_or(|resume| !resume.fully_played && resume.resume_position_ms == 0)
            && released(newest).is_some_and(|date| date >= oldest_new)
        {
            new.push((show, newest, EpisodeReason::New));
        }
    }
    for group in [&mut continuing, &mut new] {
        group.sort_by_key(|(_, episode, _)| std::cmp::Reverse(released(episode)));
    }
    let mut seen = std::collections::HashSet::new();
    continuing
        .into_iter()
        .chain(new)
        .filter(|(_, episode, _)| !episode.uri.is_empty() && seen.insert(&episode.uri))
        .take(PODCAST_CARDS)
        .map(|(show, episode, reason)| {
            let mut episode = episode.clone();
            // A show's episode list leaves out the show; menus need it.
            if episode.show.is_none() {
                episode.show = Some(show.clone());
            }
            (show.clone(), episode, reason)
        })
        .collect()
}

/// The podcast shelf; false when it has no episode to show.
fn podcasts(app: &mut App, ui: &mut Ui) -> bool {
    let palette = app.palette;
    let episodes = podcast_episodes(
        &app.home.podcasts,
        |show| app.audiobook_shows.contains(&show.uri) || app.is_saved(&show.uri) == Some(false),
        jiff::Zoned::now().date(),
    );
    if episodes.is_empty() {
        return false;
    }
    let title = gettext(app.locale, "Your podcasts");
    shelf(
        ui,
        &palette,
        app.locale,
        &ShelfSpec::new("podcasts", &title, episodes.len()),
        |ui| {
            for (show, episode, reason) in &episodes {
                let subtitle = match reason {
                    EpisodeReason::Continue { left_ms } => {
                        // Translators: {time} is the time left in an episode, such as 12 min; {show} is the podcast's name.
                        gettext(app.locale, "{time} left • {show}")
                            .replace(
                                "{time}",
                                &crate::util::format_episode_ms(app.locale, *left_ms),
                            )
                            .replace("{show}", &show.name)
                    }
                    EpisodeReason::New => {
                        // Translators: {show} is the podcast's name.
                        gettext(app.locale, "New • {show}").replace("{show}", &show.name)
                    }
                };
                let card_art = app.art_px(CARD_ART);
                let card = card(
                    ui,
                    app,
                    &Card {
                        image: pick_image(&episode.images, card_art)
                            .or_else(|| pick_image(&show.images, card_art)),
                        title: &episode.name,
                        subtitle: &subtitle,
                        round: false,
                        playing: false,
                    },
                );
                if card.play {
                    app.actions.push(Action::PlayEpisode {
                        uri: episode.uri.clone(),
                        resume_ms: episode.resume_ms(),
                    });
                }
                if card.clicked && !show.id.is_empty() {
                    app.actions.push(Action::Open(Page::Show(show.id.clone())));
                }
                egui::Popup::context_menu(&card.response)
                    .id(ui.make_persistent_id(("home-podcasts-menu", &episode.uri)))
                    .frame(widgets::menu_frame(&palette))
                    .show(|ui| {
                        widgets::item_menu(
                            ui,
                            app,
                            &PlayableItem::Episode(episode.clone()),
                            None,
                            None,
                        );
                    });
            }
        },
    );
    true
}

fn top_artists(app: &mut App, ui: &mut Ui) {
    let palette = app.palette;
    let title = gettext(app.locale, "Your top artists");
    let artists = match app.home.top_artists.clone() {
        Loadable::Loaded(artists) => artists,
        Loadable::Loading | Loadable::NotLoaded => {
            shelf(
                ui,
                &palette,
                app.locale,
                &ShelfSpec::new("top-artists", &title, 0),
                |ui| widgets::skeleton_cards(ui, &palette, app.locale, 7, true),
            );
            return;
        }
        Loadable::Failed(message) => {
            shelf(
                ui,
                &palette,
                app.locale,
                &ShelfSpec::new("top-artists", &title, 0),
                |ui| {
                    widgets::error_row(ui, app, &message, Some(Page::Home));
                },
            );
            return;
        }
    };
    if artists.is_empty() {
        return;
    }
    shelf(
        ui,
        &palette,
        app.locale,
        &ShelfSpec::new("top-artists", &title, artists.len()),
        |ui| {
            for artist in &artists {
                let context_here =
                    app.playing_context_uri().as_deref() == Some(artist.uri.as_str());
                let playing_here = context_here && app.believed_playing();
                let card_art = app.art_px(CARD_ART);
                let kind = gettext(app.locale, "Artist");
                let card = card(
                    ui,
                    app,
                    &Card {
                        image: pick_image(&artist.images, card_art),
                        title: &artist.name,
                        subtitle: &kind,
                        round: true,
                        playing: playing_here,
                    },
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
                    .id(ui.make_persistent_id(("home-top_artists-menu", &artist.uri)))
                    .frame(widgets::menu_frame(&palette))
                    .show(|ui| {
                        widgets::context_menu_items(ui, app, &artist.uri, &artist.name, None);
                    });
            }
        },
    );
}

/// The rows of a song list on Home, played as one list.
fn song_rows(app: &mut App, ui: &mut Ui, tracks: &[crate::api::models::Track], limit: usize) {
    let uris: Arc<[String]> = tracks
        .iter()
        .map(|track| track.uri.clone())
        .collect::<Vec<_>>()
        .into();
    let context = RowContext::Uris(Arc::clone(&uris));
    for (index, track) in tracks.iter().take(limit).enumerate() {
        let item = PlayableItem::Track(track.clone());
        widgets::track_row(
            ui,
            app,
            TrackRow {
                index,
                number: None,
                item: &item,
                context: &context,
                show_cover: !app.settings.tracklist_compact,
                show_album: true,
                added_at: None,
                added_by: None,
                show_added_by: false,
                show_plays: false,
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
}

/// How many of the top songs Home lists; Show all lists the rest.
const TOP_SONGS: usize = 10;

fn top_tracks(app: &mut App, ui: &mut Ui) {
    let palette = app.palette;
    let title = gettext(app.locale, "Your top songs");
    let show_all = gettext(app.locale, "Show all");
    let tracks = app.home.top_tracks.clone();

    ui.add_space(SECTION_GAP);
    let clicks = section_header(
        ui,
        &palette,
        "top-songs",
        &title,
        true,
        Some(&show_all),
        0.0,
    );
    if clicks.title || clicks.show_all {
        app.actions.push(Action::Open(Page::TopSongs));
    }
    ui.add_space(8.0);

    let labels: Vec<_> = TopPeriod::ALL
        .iter()
        .map(|p| (*p, p.label(app.locale)))
        .collect();
    let options: Vec<(TopPeriod, &str)> = labels
        .iter()
        .map(|(period, label)| (*period, label.as_ref()))
        .collect();
    if let Some(period) = widgets::chips(ui, &palette, &options, app.home.top_period) {
        app.actions.push(Action::SetTopPeriod(period));
    }
    ui.add_space(8.0);

    match tracks {
        Loadable::Loaded(tracks) => song_rows(app, ui, &tracks, TOP_SONGS),
        Loadable::Loading | Loadable::NotLoaded => {
            widgets::loading_row(ui, &palette, app.locale);
        }
        Loadable::Failed(message) => {
            widgets::error_row(ui, app, &message, Some(Page::TopSongs));
        }
    }
}

fn recommendations(app: &mut App, ui: &mut Ui) {
    let palette = app.palette;
    let title = gettext(app.locale, "Recommended for you");
    let tracks = match app.home.recommendations.clone() {
        Loadable::Loaded(tracks) if tracks.is_empty() => return,
        tracks => tracks,
    };
    ui.add_space(SECTION_GAP);
    section_header(ui, &palette, "recommendations", &title, false, None, 0.0);
    ui.add_space(8.0);
    match tracks {
        Loadable::Loaded(tracks) => song_rows(app, ui, &tracks, 20),
        Loadable::Loading | Loadable::NotLoaded => {
            widgets::skeleton_rows(ui, &palette, app.locale, 4);
        }
        Loadable::Failed(message) => {
            widgets::error_row(ui, app, &message, Some(Page::Home));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::ResumePoint;

    fn show(id: &str) -> Show {
        Show {
            id: id.into(),
            uri: format!("spotify:show:{id}"),
            name: id.to_uppercase(),
            ..Show::default()
        }
    }

    fn episode(id: &str, date: &str, played: Option<u32>, finished: bool) -> Episode {
        Episode {
            id: id.into(),
            uri: format!("spotify:episode:{id}"),
            duration_ms: 3_600_000,
            release_date: Some(date.into()),
            resume_point: Some(ResumePoint {
                fully_played: finished,
                resume_position_ms: played.unwrap_or(0),
            }),
            ..Episode::default()
        }
    }

    fn ids(shelf: &[(Show, Episode, EpisodeReason)]) -> Vec<&str> {
        shelf
            .iter()
            .map(|(_, episode, _)| episode.id.as_str())
            .collect()
    }

    /// A tile's cover is 60 points. Spotify's 64 pixel rendition would be
    /// blurry even at 100%, so the tile takes the 300 pixel one and scales
    /// it down; at 300% the 300 pixel one still has a quarter more pixels
    /// than the tile shows.
    #[cfg(feature = "demo")]
    #[test]
    fn quick_access_tiles_ask_for_art_the_display_can_show() {
        let root = std::env::temp_dir().join(format!("spotifast-tiles-{}", std::process::id()));
        let mut app = App::new(
            &crate::backend::Waker::default(),
            crate::paths::AppDirs {
                config: root.join("config"),
                state: root.join("state"),
                cache: root.join("cache"),
            },
            crate::settings::Settings::default(),
            crate::app::AppOptions {
                media_controls: false,
                restore_sign_in: false,
                tray: false,
                autostart: false,
            },
        );
        crate::demo::populate(&mut app);
        // Only the Library's playlists, so the second tile is the first one.
        app.home.recently_played = Loadable::Loaded(Vec::new());
        let first = app.library.playlists.get().unwrap()[0].images.clone();
        let width = |url: Option<&str>| {
            first
                .iter()
                .find(|image| Some(image.url.as_str()) == url)
                .and_then(|image| image.width)
        };
        app.pixels_per_point = 1.0;
        assert_eq!(
            width(quick_access_tiles(&app, Filter::All)[1].image.as_deref()),
            Some(300)
        );
        app.pixels_per_point = 3.0;
        assert_eq!(
            width(quick_access_tiles(&app, Filter::All)[1].image.as_deref()),
            Some(300)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// Home's tiles follow what was played, newest first: the playlist,
    /// album or Liked Songs a song came from, or the song itself, each once.
    /// The Library's playlists fill what history leaves, and stand in until
    /// it arrives (#942).
    #[cfg(feature = "demo")]
    #[test]
    fn quick_access_tiles_follow_what_was_played() {
        let root =
            std::env::temp_dir().join(format!("spotifast-recent-tiles-{}", std::process::id()));
        let mut app = App::new(
            &crate::backend::Waker::default(),
            crate::paths::AppDirs {
                config: root.join("config"),
                state: root.join("state"),
                cache: root.join("cache"),
            },
            crate::settings::Settings::default(),
            crate::app::AppOptions {
                media_controls: false,
                restore_sign_in: false,
                tray: false,
                autostart: false,
            },
        );
        crate::demo::populate(&mut app);
        let history = app.home.recently_played.get().unwrap().clone();
        let tiles = quick_access_tiles(&app, Filter::All);
        assert_eq!(tiles.len(), TILES);
        assert_eq!(
            tiles[0].page,
            Page::Playlist("pl1".into()),
            "played from it"
        );
        let song = &history[2].track;
        assert_eq!(
            tiles[1].uri.as_deref(),
            Some(song.uri.as_str()),
            "played on its own"
        );
        assert!(tiles[1].song.is_some());
        let pages: Vec<_> = tiles.iter().map(|tile| tile.page.clone()).collect();
        assert_eq!(
            pages
                .iter()
                .filter(|page| **page == Page::LikedSongs)
                .count(),
            1
        );
        let unique: std::collections::HashSet<_> = pages.iter().collect();
        assert_eq!(unique.len(), pages.len(), "each once");

        app.home.recently_played = Loadable::Loading;
        let tiles = quick_access_tiles(&app, Filter::All);
        assert_eq!(
            tiles[0].page,
            Page::LikedSongs,
            "the Library until history arrives"
        );
        let first = app.library.playlists.get().unwrap()[0].id.clone();
        assert_eq!(tiles[1].page, Page::Playlist(first));
        assert_eq!(tiles.len(), TILES);
        app.backend.shutdown();
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(feature = "demo")]
    fn demo_app(name: &str) -> (App, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("spotifast-{name}-{}", std::process::id()));
        let mut app = App::new(
            &crate::backend::Waker::default(),
            crate::paths::AppDirs {
                config: root.join("config"),
                state: root.join("state"),
                cache: root.join("cache"),
            },
            crate::settings::Settings::default(),
            crate::app::AppOptions {
                media_controls: false,
                restore_sign_in: false,
                tray: false,
                autostart: false,
            },
        );
        crate::demo::populate(&mut app);
        (app, root)
    }

    /// Draws Home at `width` and returns the text it painted.
    #[cfg(feature = "demo")]
    fn painted(ctx: &egui::Context, app: &mut App, width: f32) -> Vec<String> {
        fn walk(shape: &egui::epaint::Shape, text: &mut Vec<String>) {
            match shape {
                egui::epaint::Shape::Text(shape) => text.push(shape.galley.job.text.clone()),
                egui::epaint::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, text)),
                _ => {}
            }
        }
        let mut text = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(width, 3200.0))),
                    ..Default::default()
                },
                |ui| super::show(app, ui),
            );
            output.textures_delta.clear();
            text.clear();
            for shape in &output.shapes {
                walk(&shape.shape, &mut text);
            }
        }
        text
    }

    /// The chips sort the shortcuts as Spotify's do: Music leaves podcasts
    /// out, Podcasts shows only podcasts, filled from the saved ones.
    #[cfg(feature = "demo")]
    #[test]
    fn home_chips_filter_the_shortcuts() {
        let (mut app, root) = demo_app("home-chip-tiles");
        assert!(
            quick_access_tiles(&app, Filter::Music)
                .iter()
                .all(|tile| !tile.podcast())
        );
        let podcasts = quick_access_tiles(&app, Filter::Podcasts);
        assert!(!podcasts.is_empty());
        assert!(podcasts.iter().all(Tile::podcast));
        app.backend.shutdown();
        let _ = std::fs::remove_dir_all(root);
    }

    /// Under the Podcasts chip Home shows only the podcast shelf; All brings
    /// the music shelves back, with the newest release's hero and Show all.
    #[cfg(feature = "demo")]
    #[test]
    fn home_podcasts_chip_keeps_only_podcasts() {
        let (mut app, root) = demo_app("home-chip-shelves");
        let ctx = egui::Context::default();
        app.attach(&ctx);
        app.reveal_theme_changes = false;
        let text = painted(&ctx, &mut app, 1280.0);
        for label in [
            "All",
            "Music",
            "Podcasts",
            "Made for you",
            "New release from",
            "New releases",
            "Your podcasts",
            "Show all",
        ] {
            assert!(text.iter().any(|text| text == label), "{label}");
        }
        ctx.data_mut(|data| data.insert_temp(filter_id(), Filter::Podcasts));
        let text = painted(&ctx, &mut app, 1280.0);
        assert!(text.iter().any(|text| text == "Your podcasts"));
        for label in ["Made for you", "New releases", "Your top artists"] {
            assert!(!text.iter().any(|text| text == label), "{label}");
        }
        ctx.data_mut(|data| data.insert_temp(filter_id(), Filter::Music));
        let text = painted(&ctx, &mut app, 1280.0);
        assert!(text.iter().any(|text| text == "Made for you"));
        assert!(!text.iter().any(|text| text == "Your podcasts"));
        app.backend.shutdown();
        let _ = std::fs::remove_dir_all(root);
    }

    /// Show all on a shelf without a page of its own lays all its cards out,
    /// and Show less puts them back on one row.
    #[cfg(feature = "demo")]
    #[test]
    fn home_show_all_expands_a_shelf_without_a_page() {
        let (mut app, root) = demo_app("home-show-all");
        let ctx = egui::Context::default();
        app.attach(&ctx);
        app.reveal_theme_changes = false;
        let expanded = Id::new(("home-shelf-expanded", "made-for-you"));
        let row = painted(&ctx, &mut app, 900.0);
        assert!(!row.iter().any(|text| text == "Show less"));
        ctx.data_mut(|data| data.insert_temp(expanded, true));
        let all = painted(&ctx, &mut app, 900.0);
        assert!(all.iter().any(|text| text == "Show less"));
        let count = |text: &[String]| {
            app.home
                .made_for_you
                .get()
                .unwrap()
                .iter()
                .filter(|playlist| text.contains(&playlist.name))
                .count()
        };
        assert!(count(&all) > count(&row), "every card in sight");
        app.backend.shutdown();
        let _ = std::fs::remove_dir_all(root);
    }

    /// The wash eases toward the hovered shortcut's colour and settles there.
    #[test]
    fn the_wash_settles_on_its_colour() {
        let target = egui::Rgba::from(Color32::from_rgb(120, 40, 40));
        let mut color = egui::Rgba::TRANSPARENT;
        let mut frames = 0;
        while let Some(next) = ease_toward(color, target, 1.0 / 60.0) {
            assert!(next.a() >= color.a(), "it only moves toward the colour");
            color = next;
            frames += 1;
            assert!(frames < 120, "settled within two seconds");
        }
        assert!(frames > 3, "eased, not cut");
    }

    fn today() -> jiff::civil::Date {
        "2026-09-23".parse().unwrap()
    }

    #[test]
    fn episodes_to_continue_come_before_new_ones() {
        let podcasts = vec![
            (
                show("a"),
                vec![
                    episode("a-new", "2026-09-20", None, false),
                    episode("a-started", "2026-09-01", Some(600_000), false),
                    episode("a-done", "2026-08-25", Some(0), true),
                ],
            ),
            (
                show("b"),
                vec![
                    episode("b-started", "2026-09-10", Some(60_000), false),
                    episode("b-old", "2026-09-03", None, false),
                ],
            ),
            (show("c"), vec![episode("c-new", "2026-09-22", None, false)]),
        ];
        let shelf = podcast_episodes(&podcasts, |_| false, today());
        assert_eq!(ids(&shelf), ["b-started", "a-started", "c-new", "a-new"]);
        assert_eq!(shelf[1].2, EpisodeReason::Continue { left_ms: 3_000_000 });
        assert_eq!(shelf[2].2, EpisodeReason::New);
        assert_eq!(
            shelf[0].1.show.as_ref().map(|show| show.id.as_str()),
            Some("b"),
            "the episode menu can go to its podcast"
        );
    }

    #[test]
    fn only_a_recent_unstarted_newest_episode_is_new() {
        let podcasts = vec![
            (show("old"), vec![episode("old", "2026-07-01", None, false)]),
            (
                show("finished"),
                vec![episode("finished", "2026-09-20", Some(0), true)],
            ),
            (
                show("undated"),
                vec![episode("undated", "2026", None, false)],
            ),
            (
                show("second"),
                vec![
                    episode("second-done", "2026-09-21", Some(0), true),
                    episode("second-new", "2026-09-20", None, false),
                ],
            ),
        ];
        assert!(podcast_episodes(&podcasts, |_| false, today()).is_empty());
    }

    #[test]
    fn skipped_shows_leave_the_shelf_and_it_stays_bounded() {
        let podcasts: Vec<_> = (0..20)
            .map(|index| {
                let id = format!("s{index}");
                let started = episode(&format!("{id}-e"), "2026-09-01", Some(1), false);
                (show(&id), vec![started])
            })
            .collect();
        let shelf = podcast_episodes(&podcasts, |show| show.id == "s0", today());
        assert_eq!(shelf.len(), PODCAST_CARDS);
        assert!(!ids(&shelf).contains(&"s0-e"));
    }
}
