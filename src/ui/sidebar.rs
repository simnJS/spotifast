//! The left panel: navigation and Your Library.

use egui::{Align, CornerRadius, Frame, Layout, Margin, Rect, Sense, Vec2, pos2, vec2};

use crate::api::models::pick_image;
use crate::app::App;
use crate::i18n::{Locale, gettext};
use crate::model::{Action, Dialog, DragEntry, DragTrack, Loadable, Page};
use crate::settings::{LIKED_SONGS_KEY, LibraryShelf as Filter, LibrarySort};
use crate::theme::{self, Icon, Palette};

const DEFAULT_ROW_HEIGHT: f32 = ROW_ART + 2.0 * LIBRARY_ITEM_PADDING;
const COMPACT_ROW_HEIGHT: f32 = 32.0;

struct Entry {
    image: Option<String>,
    grid_image: Option<String>,
    name: String,
    subtitle: String,
    grid_subtitle: String,
    page: Page,
    uri: String,
    round: bool,
    liked: bool,
    /// The account's own playlist: the one it may rename and delete.
    owned: bool,
    /// A playlist the account may drop songs on.
    editable: bool,
    playlist_index: Option<usize>,
    /// A folder row: its rootlist id, whether it is rolled up, and how
    /// many playlists it holds.
    folder: Option<(String, bool, usize)>,
    /// How deep inside folders the row sits, for the indent.
    depth: u8,
    added_at: Option<i64>,
}

impl Entry {
    fn ordering_key(&self) -> &str {
        if self.liked {
            LIKED_SONGS_KEY
        } else {
            &self.uri
        }
    }
}

/// The context a library row plays, matching the cover play button and the
/// right-click menu. Liked Songs has no URI of its own and plays the
/// account's collection instead.
fn entry_play_uri(app: &App, entry: &Entry) -> Option<String> {
    if entry.liked {
        app.user
            .as_ref()
            .map(|user| format!("spotify:user:{}:collection", user.id))
    } else if entry.uri.is_empty() {
        None
    } else {
        Some(entry.uri.clone())
    }
}

fn entry_is_playing_context(entry: &Entry, context: Option<&str>) -> bool {
    if entry.liked {
        context.is_some_and(|context| context.ends_with(":collection"))
    } else {
        !entry.uri.is_empty() && context == Some(entry.uri.as_str())
    }
}

fn cover_play_button(
    app: &mut App,
    ui: &mut egui::Ui,
    entry: &Entry,
    index: usize,
    cover_rect: Rect,
    parent: &egui::Response,
    current_context: bool,
) -> bool {
    let Some(uri) = entry_play_uri(app, entry) else {
        return false;
    };
    // The control pauses only while this context is actually playing; a
    // paused one keeps the play glyph, but pressing it resumes in place
    // instead of restarting the context.
    let playing = current_context && app.believed_playing();
    let play = ui.interact(
        cover_rect,
        ui.id().with(("library-cover-play", index)),
        Sense::click(),
    );
    let play_hover = play.hovered();
    if play_hover || parent.hovered() {
        let radius = if entry.round {
            (cover_rect.width() / 2.0).min(127.0) as u8
        } else {
            6
        };
        ui.painter().rect_filled(
            cover_rect,
            CornerRadius::same(radius),
            egui::Color32::from_black_alpha(120),
        );
        let icon = if playing {
            Icon::PauseFilled
        } else {
            Icon::PlayFilled
        };
        let icon_size = (cover_rect.width() * 0.24).clamp(18.0, 26.0);
        icon.image(
            if play_hover {
                app.palette.accent
            } else {
                egui::Color32::WHITE
            },
            icon_size,
        )
        .paint_at(
            ui,
            Rect::from_center_size(
                cover_rect.center() + theme::play_glyph_offset(icon, icon_size),
                Vec2::splat(icon_size),
            ),
        );
    }
    if !play.clicked() {
        return false;
    }
    // The first click of a double click plays; the second must not play again.
    if !play.double_clicked() {
        if current_context {
            app.actions.push(Action::TogglePlay);
        } else {
            app.actions.push(Action::PlayContext {
                uri,
                offset_uri: None,
                offset_index: None,
            });
        }
    }
    true
}

fn grid_play_rect(cover_rect: Rect) -> Rect {
    let size = (cover_rect.width() * 0.3).clamp(32.0, 44.0);
    let inset = LIBRARY_ITEM_PADDING + size / 2.0;
    Rect::from_center_size(
        pos2(cover_rect.right() - inset, cover_rect.bottom() - inset),
        Vec2::splat(size),
    )
}

fn grid_pin_rect(cover_rect: Rect) -> Rect {
    Rect::from_center_size(cover_rect.left_top() + Vec2::splat(12.0), Vec2::splat(20.0))
}

fn grid_play_button(
    app: &mut App,
    ui: &mut egui::Ui,
    entry: &Entry,
    cover_rect: Rect,
    parent: &egui::Response,
    playing_here: bool,
) -> bool {
    let Some(uri) = entry_play_uri(app, entry) else {
        return false;
    };
    let rect = grid_play_rect(cover_rect);
    let size = rect.width();
    let button = ui.interact(rect, ui.id().with("library-grid-play"), Sense::click());
    let playing = playing_here && app.believed_playing();
    let label = if playing {
        // Translators: The play button on a Library card. {name} is the playlist, album, artist, or podcast.
        gettext(app.locale, "Pause {name}")
    } else {
        // Translators: The play button on a Library card. {name} is the playlist, album, artist, or podcast.
        gettext(app.locale, "Play {name}")
    }
    .replace("{name}", &entry.name);
    button.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label.clone())
    });
    if parent.hovered() || playing_here || button.has_focus() {
        let fill = if button.hovered() {
            app.palette.accent_hover
        } else {
            app.palette.accent
        };
        ui.painter().add(
            egui::epaint::Shadow {
                offset: [0, 4],
                blur: 12,
                spread: 0,
                color: egui::Color32::from_black_alpha(90),
            }
            .as_shape(rect, egui::CornerRadius::same(127)),
        );
        ui.painter().circle_filled(rect.center(), size / 2.0, fill);
        let icon = if playing {
            Icon::PauseFilled
        } else {
            Icon::PlayFilled
        };
        let icon_size = size * 0.42;
        icon.image(app.palette.on_accent, icon_size).paint_at(
            ui,
            Rect::from_center_size(
                rect.center() + theme::play_glyph_offset(icon, icon_size),
                Vec2::splat(icon_size),
            ),
        );
    }
    if !button.clicked() {
        return false;
    }
    if playing_here {
        app.actions.push(Action::TogglePlay);
    } else {
        app.actions.push(Action::PlayContext {
            uri,
            offset_uri: None,
            offset_index: None,
        });
    }
    true
}

fn finish_entry_interaction(
    app: &mut App,
    ui: &mut egui::Ui,
    response: &egui::Response,
    entry: &Entry,
    cover_took_click: bool,
    custom_order: bool,
    drop_allowed: bool,
) {
    if response.hovered()
        && let Some(image) = &entry.image
    {
        app.actions.push(Action::PrepareTint(image.clone()));
    }
    if !entry.ordering_key().is_empty() && response.drag_started_by(egui::PointerButton::Primary) {
        egui::DragAndDrop::set_payload(
            ui.ctx(),
            DragEntry {
                uri: entry.ordering_key().to_string(),
                title: entry.name.clone(),
                image: entry.image.clone(),
            },
        );
    }
    if drop_allowed
        && (entry.liked || entry.editable)
        && egui::DragAndDrop::has_payload_of_type::<DragTrack>(ui.ctx())
        && let Some(track) = response.dnd_release_payload::<DragTrack>()
    {
        if entry.liked {
            app.actions.push(Action::SetSavedMany {
                uris: track
                    .items
                    .iter()
                    .map(|item| item.uri().to_string())
                    .collect(),
                saved: true,
            });
        } else if let Page::Playlist(id) = &entry.page {
            app.actions.push(Action::AddToPlaylist {
                playlist_id: id.clone(),
                playlist_name: entry.name.clone(),
                items: track.items.clone(),
            });
        }
    }
    theme::focus_ring(ui, response);
    if response.clicked() && !cover_took_click {
        if let Some((folder_id, _, _)) = &entry.folder {
            app.actions
                .push(Action::ToggleLibraryFolder(folder_id.clone()));
        } else {
            app.actions.push(Action::Open(entry.page.clone()));
        }
    }
    if !app.settings.sidebar_grid
        && response.double_clicked()
        && entry.folder.is_none()
        && !cover_took_click
        && let Some(uri) = entry_play_uri(app, entry)
    {
        app.actions.push(Action::PlayContext {
            uri,
            offset_uri: None,
            offset_index: None,
        });
    }
    entry_menu(app, response, entry, custom_order);
    crate::autoscroll::row(ui, response);
}

fn liked_entry(app: &App) -> Entry {
    let subtitle = match app.library.liked.total {
        Some(total) => app.locale.liked_song_count(total),
        None => gettext(app.locale, "Playlist").into_owned(),
    };
    let grid_subtitle = app
        .library
        .liked
        .total
        .map_or_else(String::new, |total| app.locale.song_count(total));
    Entry {
        image: None,
        grid_image: None,
        name: gettext(app.locale, "Liked Songs").into_owned(),
        subtitle,
        grid_subtitle,
        page: Page::LikedSongs,
        uri: String::new(),
        round: false,
        liked: true,
        owned: false,
        editable: false,
        playlist_index: None,
        folder: None,
        depth: 0,
        added_at: None,
    }
}

pub(crate) fn selected_sort(app: &App, shelf: Filter) -> LibrarySort {
    if let Some(sort) = app.settings.library_sort.get(&shelf).copied()
        && sort.supports(shelf)
    {
        return sort;
    }
    if shelf != Filter::Playlists {
        LibrarySort::Library
    } else if !app.settings.sidebar_order.is_empty() {
        LibrarySort::Local
    } else if app
        .rootlist
        .iter()
        .any(|row| matches!(row, crate::player::RootlistEntry::FolderStart { .. }))
    {
        LibrarySort::Spotify
    } else {
        LibrarySort::RecentlyPlayed
    }
}

fn sort_labels(locale: Locale) -> [(LibrarySort, std::borrow::Cow<'static, str>); 6] {
    [
        (LibrarySort::Library, gettext(locale, "Library order")),
        (
            LibrarySort::RecentlyPlayed,
            gettext(locale, "Recently played"),
        ),
        (LibrarySort::Name, gettext(locale, "Name")),
        (
            LibrarySort::RecentlyAdded,
            gettext(locale, "Recently added"),
        ),
        (LibrarySort::Local, gettext(locale, "Local custom order")),
        (
            LibrarySort::Spotify,
            gettext(locale, "Spotify custom order"),
        ),
    ]
}

fn sort_label(locale: Locale, sort: LibrarySort) -> std::borrow::Cow<'static, str> {
    sort_labels(locale)
        .into_iter()
        .find(|(held, _)| *held == sort)
        .expect("sort label")
        .1
}

const SORT_FONT_SIZE: f32 = 14.0;

/// The width the sort menu's button takes, for the search field beside it.
fn sort_menu_width(app: &App, ui: &egui::Ui, sort: LibrarySort) -> f32 {
    ui.painter()
        .layout_no_wrap(
            sort_label(app.locale, sort).into_owned(),
            theme::regular(SORT_FONT_SIZE),
            app.palette.text,
        )
        .size()
        .x
        + 16.0
}

/// Spotify's sort button: the selected order's name, quiet until pointed
/// at. Its menu offers only the sort modes this shelf supports, then the
/// row density, and queues the selected change.
fn sort_menu(app: &mut App, ui: &mut egui::Ui, shelf: Filter, selected: LibrarySort) {
    let locale = app.locale;
    let palette = app.palette;
    let labels = sort_labels(locale);
    let label = sort_label(locale, selected);
    let room = (ui.available_width() - 8.0).max(0.0);
    let galley = crate::bidi::layout(
        ui.painter(),
        &label,
        theme::regular(SORT_FONT_SIZE),
        palette.secondary,
        room - 16.0,
        1,
        Some(crate::bidi::ELLIPSIS),
    );
    let (rect, response) = ui.allocate_exact_size(
        vec2((galley.size().x + 16.0).min(room), PILL_HEIGHT),
        Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label.as_ref())
    });
    if ui.is_rect_visible(rect) {
        let color = if response.hovered() || response.has_focus() {
            palette.text
        } else {
            palette.secondary
        };
        ui.painter().galley(
            pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0),
            galley,
            color,
        );
    }
    theme::focus_ring(ui, &response);
    egui::Popup::menu(&response)
        .frame(super::widgets::menu_frame(&palette))
        .show(|ui| {
            let width = labels
                .iter()
                .map(|(_, label)| {
                    ui.painter()
                        .layout_no_wrap(label.to_string(), theme::regular(13.5), palette.text)
                        .size()
                        .x
                })
                .fold(140.0_f32, f32::max)
                + 52.0;
            ui.set_width(width.min(ui.ctx().content_rect().width() - 24.0));
            menu_heading(ui, &palette, &gettext(locale, "Sort by"));
            for (sort, label) in &labels {
                if !sort.supports(shelf)
                    || (*sort == LibrarySort::Library && shelf == Filter::Playlists)
                    || (*sort == LibrarySort::Local && app.settings.sidebar_order.is_empty())
                {
                    continue;
                }
                if super::widgets::menu_item(
                    ui,
                    &palette,
                    (*sort == selected).then_some(Icon::Check),
                    label,
                ) {
                    app.actions
                        .push(Action::SetLibrarySort { shelf, sort: *sort });
                }
            }
            ui.add_space(4.0);
            ui.separator();
            menu_heading(ui, &palette, &gettext(locale, "View as"));
            let compact = app.settings.sidebar_compact;
            let grid = app.settings.sidebar_grid;
            for (view, label) in [
                (LibraryView::Compact, gettext(locale, "Compact")),
                (LibraryView::List, gettext(locale, "List")),
                (LibraryView::Grid, gettext(locale, "Grid")),
            ] {
                let current = LibraryView::of(compact, grid) == view;
                if super::widgets::menu_item(ui, &palette, current.then_some(Icon::Check), &label)
                    && !current
                {
                    let (compact, grid) = view.flags(compact);
                    if grid != app.settings.sidebar_grid {
                        app.actions.push(Action::SetLibraryGrid(grid));
                    }
                    if compact != app.settings.sidebar_compact {
                        app.settings.sidebar_compact = compact;
                        app.actions.push(Action::SettingsChanged);
                    }
                }
            }
        });
}

/// How Your Library draws its rows, as the sort menu offers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LibraryView {
    Compact,
    List,
    Grid,
}

impl LibraryView {
    fn of(compact: bool, grid: bool) -> Self {
        if grid {
            Self::Grid
        } else if compact {
            Self::Compact
        } else {
            Self::List
        }
    }

    /// The compact and grid settings that give this view. The grid keeps
    /// the saved density, which its cards do not use.
    fn flags(self, compact: bool) -> (bool, bool) {
        match self {
            Self::Compact => (true, false),
            Self::List => (false, false),
            Self::Grid => (compact, true),
        }
    }
}

/// A small subdued heading inside a menu.
fn menu_heading(ui: &mut egui::Ui, palette: &Palette, text: &str) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        theme::text(ui, text, theme::bold(11.0), palette.secondary);
    });
    ui.add_space(2.0);
}

fn saved_time(value: Option<&str>) -> Option<i64> {
    value
        .and_then(|text| text.parse::<jiff::Timestamp>().ok())
        .map(|time| time.as_millisecond())
}

/// Build the ranks once per frame, keeping the first occurrence just as
/// `position` did. Searching the whole order from a sort comparator makes
/// scrolling a large library quadratic even when only visible rows are drawn.
fn first_ranks<'a>(
    keys: impl Iterator<Item = &'a str>,
) -> std::collections::HashMap<&'a str, usize> {
    let mut ranks = std::collections::HashMap::new();
    for (rank, key) in keys.enumerate() {
        ranks.entry(key).or_insert(rank);
    }
    ranks
}

/// Applies the selected library order, then places pins first in their saved
/// order. New playlists precede a saved local arrangement, and pinned playlists
/// are shown at the top level rather than inside their folders.
fn order_entries(app: &App, shelf: Filter, sort: LibrarySort, entries: &mut [Entry]) {
    match sort {
        LibrarySort::Name => {
            // Each row sorts under the folders that hold it: its key is the
            // path of names down to it, so a folder's rows follow the folder
            // in name order and the folders sort among their siblings.
            let mut folders: Vec<(String, String)> = Vec::new();
            let paths: Vec<Vec<(String, String)>> = entries
                .iter()
                .map(|entry| {
                    folders.truncate(entry.depth as usize);
                    let key = (
                        entry.name.to_lowercase(),
                        match &entry.folder {
                            Some((id, _, _)) => id.clone(),
                            None => entry.uri.clone(),
                        },
                    );
                    let mut path = folders.clone();
                    path.push(key.clone());
                    if entry.folder.is_some() {
                        folders.push(key);
                    }
                    path
                })
                .collect();
            let mut order: Vec<usize> = (0..entries.len()).collect();
            order.sort_by(|a, b| paths[*a].cmp(&paths[*b]));
            permute(entries, &order);
        }
        LibrarySort::RecentlyPlayed => {
            let ranks = first_ranks(app.recent_contexts.iter().map(String::as_str));
            let liked_uri = app
                .user_id()
                .map(|id| format!("spotify:user:{id}:collection"));
            entries.sort_by_cached_key(|entry| {
                let key = if entry.liked {
                    liked_uri.as_deref()
                } else {
                    Some(entry.uri.as_str())
                };
                key.and_then(|key| ranks.get(key))
                    .copied()
                    .unwrap_or(usize::MAX)
            });
        }
        LibrarySort::RecentlyAdded => entries
            .sort_by_key(|entry| (entry.added_at.is_none(), std::cmp::Reverse(entry.added_at))),
        LibrarySort::Local => {
            let ranks = first_ranks(app.settings.sidebar_order.iter().map(String::as_str));
            entries.sort_by_cached_key(|entry| match ranks.get(entry.ordering_key()) {
                Some(&rank) => (1, rank),
                None => (0, entry.playlist_index.unwrap_or(0)),
            });
        }
        LibrarySort::Spotify if !entries.iter().any(|entry| entry.folder.is_some()) => {
            let ranks = first_ranks(app.rootlist.iter().filter_map(|row| match row {
                crate::player::RootlistEntry::Playlist(uri) => Some(uri.as_str()),
                _ => None,
            }));
            entries.sort_by_cached_key(|entry| {
                (
                    entry.liked,
                    ranks.get(entry.uri.as_str()).copied().unwrap_or(usize::MAX),
                )
            });
        }
        LibrarySort::Spotify => entries.sort_by_key(|entry| entry.liked),
        LibrarySort::Library => {}
    }
    let pins = app.settings.library_pins();
    let ranks = first_ranks(pins.iter().map(String::as_str));
    entries.sort_by_cached_key(|entry| {
        ranks
            .get(entry.ordering_key())
            .copied()
            .unwrap_or(usize::MAX)
    });
    if shelf == Filter::Playlists {
        for entry in entries {
            if app.settings.pinned_contexts.contains(&entry.uri) {
                entry.depth = 0;
            }
        }
    }
}

/// Puts `entries[order[i]]` at position `i`.
fn permute<T>(entries: &mut [T], order: &[usize]) {
    let mut place: Vec<usize> = vec![0; order.len()];
    for (to, &from) in order.iter().enumerate() {
        place[from] = to;
    }
    for start in 0..entries.len() {
        while place[start] != start {
            let to = place[start];
            entries.swap(start, to);
            place.swap(start, to);
        }
    }
}

/// The gap between the window's rounded panels, and their corner radius.
const PANEL_GAP: f32 = 8.0;
const PANEL_RADIUS: u8 = 8;
/// The collapsed sidebar: a column of 48px covers in Spotify's 72px card.
const COLLAPSED_CARD_WIDTH: f32 = 72.0;
const COLLAPSED_WIDTH: f32 = COLLAPSED_CARD_WIDTH + 2.0 * PANEL_GAP;
/// A drag that ends narrower than this collapses the sidebar; anything
/// wider snaps to at least the sidebar's narrowest expanded width.
const COLLAPSE_BELOW: f32 = (COLLAPSED_WIDTH + super::SIDEBAR_MIN_WIDTH) / 2.0;
/// The width Your Library opens at when Show more widens it.
const WIDE_WIDTH: f32 = 600.0;
/// The expanded width remembered while the sidebar is collapsed or widened.
const RESTORE_WIDTH_ID: &str = "sidebar-restore-width";

/// Whether the saved width is the collapsed column of covers.
fn is_collapsed_width(width: f32) -> bool {
    width < COLLAPSE_BELOW
}

/// The width a released drag settles on: collapsed below the midpoint,
/// otherwise never narrower than the sidebar's narrowest expanded width.
fn settled_width(width: f32) -> f32 {
    if is_collapsed_width(width) {
        COLLAPSED_WIDTH
    } else {
        width.max(super::SIDEBAR_MIN_WIDTH)
    }
}

/// Change the sidebar's width from one of its own buttons, remembering the
/// width it leaves so the opposite button brings it back.
fn set_sidebar_width(app: &mut App, ctx: &egui::Context, width: f32) {
    let current = app.settings.sidebar_width;
    if !is_collapsed_width(current) {
        ctx.data_mut(|data| data.insert_temp(egui::Id::new(RESTORE_WIDTH_ID), current));
    }
    app.settings.sidebar_width = width;
    app.actions.push(Action::SettingsChanged);
}

/// The expanded width to return to from the collapsed or widened sidebar.
fn restore_width(ctx: &egui::Context) -> f32 {
    ctx.data(|data| data.get_temp::<f32>(egui::Id::new(RESTORE_WIDTH_ID)))
        .filter(|width| !is_collapsed_width(*width) && *width < WIDE_WIDTH)
        .unwrap_or(crate::settings::Settings::default().sidebar_width)
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let expanded_art = has_expanded_art(app);
    // The traffic lights float over the top-left of the sidebar now, so the
    // first nav row has to start below them.
    let top = PANEL_GAP as i8
        + theme::titlebar_inset(ui.ctx()) as i8
        + super::linux_left_controls_inset(ui.ctx()) as i8;
    let beside = if app.right_panel_open() {
        theme::SIDE_PANEL_MIN_WIDTH
    } else {
        0.0
    };
    // The collapsed column, and a drag on its way there, may go narrower
    // than the expanded sidebar ever does.
    let dragging = super::panel_dragged(ui.ctx(), "sidebar");
    let least = if dragging || is_collapsed_width(app.settings.sidebar_width) {
        COLLAPSED_WIDTH
    } else {
        super::SIDEBAR_MIN_WIDTH
    };
    let fit = super::yielding_panel(
        ui.ctx(),
        "sidebar",
        least..=WIDE_WIDTH,
        app.settings.sidebar_width,
        ui.available_width() - super::topbar::least_width(ui.ctx()) - beside,
    );
    let panel = egui::Panel::left("sidebar")
        .resizable(true)
        .default_size(app.settings.sidebar_width)
        .size_range(fit.range.clone())
        .show_separator_line(false)
        .frame(Frame::new().fill(palette.window).inner_margin(Margin {
            left: PANEL_GAP as i8,
            right: PANEL_GAP as i8,
            top,
            bottom: PANEL_GAP as i8,
        }));
    let chosen = app.settings.sidebar_width;
    let response = panel.show(ui, |ui| {
        let collapsed = is_collapsed_width(ui.max_rect().width() + 2.0 * PANEL_GAP);
        nav_card(app, ui, collapsed);
        ui.add_space(PANEL_GAP);
        let card = Rect::from_min_max(
            pos2(ui.max_rect().left(), ui.cursor().top()),
            ui.max_rect().max,
        );
        ui.painter()
            .rect_filled(card, CornerRadius::same(PANEL_RADIUS), palette.panel);
        // The list scrolls right to the card's bottom edge, so a drag held
        // there still scrolls it; its last row keeps a gap of its own.
        let inner = Rect::from_min_max(
            card.min + vec2(8.0, 8.0),
            card.max - vec2(if collapsed { 8.0 } else { 4.0 }, 0.0),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
            ui.set_clip_rect(card.intersect(ui.clip_rect()));
            if collapsed {
                contents(app, ui, None, true);
                return;
            }
            let floating_art = app.settings.sidebar_grid && expanded_art;
            let art_rect = expanded_art.then(|| expanded_art_rect(ui));
            if let Some(rect) = art_rect.filter(|_| !floating_art) {
                reserve_expanded_art(ui, rect);
            }
            contents(app, ui, art_rect.filter(|_| floating_art), false);
            if let Some(rect) = art_rect {
                if floating_art {
                    paint_grid_art_mask(app, ui, rect);
                }
                paint_expanded_art(app, ui, rect);
            }
        });
    });
    let width = response.response.rect.width();
    // While the handle is held the panel follows the pointer; once it is
    // let go the width settles on the collapsed column or an expanded one.
    let settled = if dragging {
        width
    } else {
        settled_width(width)
    };
    // A button that just chose a width wins over the one drawn this frame.
    if app.settings.sidebar_width == chosen
        && (settled - chosen).abs() > 1.0
        && super::panel_width_chosen(ui.ctx(), "sidebar", &fit)
    {
        if !dragging
            && !is_collapsed_width(app.settings.sidebar_width)
            && is_collapsed_width(settled)
        {
            let current = app.settings.sidebar_width;
            ui.ctx()
                .data_mut(|data| data.insert_temp(egui::Id::new(RESTORE_WIDTH_ID), current));
        }
        app.settings.sidebar_width = settled;
        app.actions.push(Action::SettingsChanged);
    }
}

/// Home and Search, in their own rounded card above Your Library. The
/// collapsed sidebar keeps only their icons.
fn nav_card(app: &mut App, ui: &mut egui::Ui, collapsed: bool) {
    let palette = app.palette;
    let locale = app.locale;
    let page = app.page().clone();
    let height = 2.0 * NAV_ROW_HEIGHT + 2.0 * NAV_CARD_PADDING;
    let card = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), height));
    ui.painter()
        .rect_filled(card, CornerRadius::same(PANEL_RADIUS), palette.panel);
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(card.shrink2(vec2(if collapsed { 0.0 } else { 12.0 }, NAV_CARD_PADDING))),
        |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if nav_row(
                ui,
                &palette,
                Icon::House,
                &gettext(locale, "Home"),
                page == Page::Home,
                collapsed,
            )
            .clicked()
            {
                app.actions.push(Action::Open(Page::Home));
            }
            if nav_row(
                ui,
                &palette,
                Icon::Search,
                &gettext(locale, "Search"),
                page == Page::Search,
                collapsed,
            )
            .clicked()
            {
                app.actions.push(Action::FocusSearch);
            }
        },
    );
    ui.advance_cursor_after_rect(card);
}

const NAV_ROW_HEIGHT: f32 = 40.0;
const NAV_CARD_PADDING: f32 = 8.0;

fn expanded_art_rect(ui: &egui::Ui) -> Rect {
    let side = expanded_art_side(ui);
    Rect::from_min_size(
        pos2(
            ui.max_rect().left(),
            ui.max_rect().bottom() - EXPANDED_ART_GAP - side,
        ),
        Vec2::splat(side),
    )
}

fn reserve_expanded_art(ui: &mut egui::Ui, rect: Rect) {
    egui::Panel::bottom("sidebar-art-space")
        .exact_size(rect.height() + EXPANDED_ART_GAP)
        .resizable(false)
        .show_separator_line(false)
        .frame(Frame::new())
        .show(ui, |_| {});
}

/// In grid mode content continues behind the shared fixed artwork.
fn paint_grid_art_mask(app: &App, ui: &egui::Ui, rect: Rect) {
    let bottom_fade_rect = Rect::from_min_max(
        pos2(ui.max_rect().left(), rect.bottom() - 64.0),
        pos2(ui.max_rect().right(), rect.bottom()),
    );
    super::widgets::paint_vertical_gradient(
        ui,
        bottom_fade_rect,
        egui::Color32::TRANSPARENT,
        app.palette.panel,
    );
    ui.painter().rect_filled(
        Rect::from_min_max(
            pos2(ui.max_rect().left(), rect.bottom()),
            ui.max_rect().right_bottom(),
        ),
        0.0,
        app.palette.panel,
    );
}

fn has_expanded_art(app: &App) -> bool {
    app.settings.art_expanded
        && app
            .now_playing()
            .is_some_and(|now| now.art_url.is_some() || now.art_small.is_some())
}

fn expanded_art_side(ui: &egui::Ui) -> f32 {
    ui.max_rect()
        .width()
        .min(ui.max_rect().height() * 0.45)
        .max(80.0)
}

fn paint_expanded_art(app: &mut App, ui: &mut egui::Ui, rect: Rect) {
    let Some(now) = app.now_playing() else {
        return;
    };
    let Some(url) = now.art_url.clone().or_else(|| now.art_small.clone()) else {
        return;
    };
    let album_id = now.album_id.clone();
    let show_id = now.show_id.clone();
    let palette = app.palette;
    ui.painter().add(
        egui::epaint::Shadow {
            offset: [0, 0],
            blur: 28,
            spread: 0,
            color: egui::Color32::from_black_alpha(if palette.dark { 120 } else { 40 }),
        }
        .as_shape(rect, egui::CornerRadius::same(8)),
    );
    super::widgets::playing_cover(
        ui,
        &palette,
        app.backend.art(),
        "sidebar-art",
        Some(&url),
        rect,
        8.0,
    );
    let art = ui.interact(rect, egui::Id::new("sidebar-art"), Sense::click());
    let chevron_rect = Rect::from_center_size(
        pos2(rect.right() - 16.0, rect.top() + 16.0),
        Vec2::splat(20.0),
    );
    let over_chevron = ui.rect_contains_pointer(chevron_rect);
    if art.hovered() || over_chevron {
        let chevron = ui.interact(
            chevron_rect,
            egui::Id::new("sidebar-art-collapse"),
            Sense::click(),
        );
        ui.painter().circle_filled(
            chevron_rect.center(),
            10.0,
            palette.panel.gamma_multiply(0.9),
        );
        Icon::ChevronDown.image(palette.text, 14.0).paint_at(
            ui,
            Rect::from_center_size(chevron_rect.center(), Vec2::splat(14.0)),
        );
        if chevron.clicked() {
            app.settings.art_expanded = false;
            app.actions.push(Action::SettingsChanged);
        }
    }
    if art.clicked() && !over_chevron {
        if let Some(id) = album_id {
            app.actions.push(Action::Open(Page::Album(id)));
        } else if let Some(id) = show_id {
            app.actions.push(Action::Open(Page::Show(id)));
        }
    }
}

/// Which owners' playlists the playlist shelf shows: the saved By You
/// filter, or Spotify's own playlists for this session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Ownership {
    #[default]
    Anyone,
    You,
    Spotify,
}

/// The account Spotify's own playlists belong to.
const SPOTIFY_OWNER: &str = "spotify";

impl Ownership {
    fn of(app: &App, by_spotify: bool) -> Self {
        if app.settings.library_by_you {
            Self::You
        } else if by_spotify {
            Self::Spotify
        } else {
            Self::Anyone
        }
    }

    fn admits(self, playlist: &crate::api::models::Playlist, user_id: Option<&str>) -> bool {
        match self {
            Self::Anyone => true,
            Self::You => user_id.is_some_and(|id| playlist.owned_by(id)),
            Self::Spotify => playlist.owned_by(SPOTIFY_OWNER),
        }
    }
}

/// Project the playlist shelf without changing its saved order or pins.
/// The Spotify order keeps the folder tree; ownership filtering then hides
/// playlists the account does not own and folders left empty, with counts
/// reflecting the visible rows. Text search stays flat so matches in
/// collapsed folders remain reachable. Liked Songs belongs to the account.
#[cfg(all(test, feature = "demo"))]
fn playlist_rows(app: &App, sort: LibrarySort, needle: &str, entries: &mut Vec<Entry>) {
    playlist_rows_of(app, sort, needle, Ownership::of(app, false), entries);
}

fn playlist_rows_of(
    app: &App,
    sort: LibrarySort,
    needle: &str,
    owner: Ownership,
    entries: &mut Vec<Entry>,
) {
    let liked = liked_entry(app);
    if owner != Ownership::Spotify
        && (needle.is_empty() || liked.name.to_lowercase().contains(needle))
    {
        entries.push(liked);
    }
    let user_id = app.user_id().filter(|id| !id.is_empty());
    // Name keeps playlists inside their folders, as Spotify does, and sorts
    // each folder on its own (#651).
    let folders = match sort {
        LibrarySort::Spotify => true,
        LibrarySort::Name => app
            .rootlist
            .iter()
            .any(|row| matches!(row, crate::player::RootlistEntry::FolderStart { .. })),
        _ => false,
    };
    if folders && needle.is_empty() {
        folder_rows_of(app, user_id.unwrap_or(""), owner, entries);
    } else if let Some(playlists) = app.library.playlists.get() {
        for (index, playlist) in playlists.iter().enumerate() {
            if !owner.admits(playlist, user_id) {
                continue;
            }
            if !needle.is_empty() && !playlist.name.to_lowercase().contains(needle) {
                continue;
            }
            entries.push(playlist_entry(
                app.locale,
                playlist,
                index,
                user_id.unwrap_or(""),
                app.can_edit_playlist(playlist),
                0,
                EntryArt::of(app),
            ));
        }
    }
}

/// Playlist rows in account order, including collapsible folders (#95).
/// With the By You filter only owned playlists are shown; folders left
/// empty are hidden and their counts reflect the visible rows.
#[cfg(all(test, feature = "demo"))]
fn folder_rows(app: &App, user_id: &str, entries: &mut Vec<Entry>) {
    folder_rows_of(app, user_id, Ownership::of(app, false), entries);
}

fn folder_rows_of(app: &App, user_id: &str, owner_filter: Ownership, entries: &mut Vec<Entry>) {
    use crate::player::RootlistEntry;
    let Some(playlists) = app.library.playlists.get() else {
        return;
    };
    let by_uri: std::collections::HashMap<&str, (usize, &crate::api::models::Playlist)> = playlists
        .iter()
        .enumerate()
        .map(|(index, playlist)| (playlist.uri.as_str(), (index, playlist)))
        .collect();
    let by_you = owner_filter != Ownership::Anyone;
    let owner = app.user_id().filter(|id| !id.is_empty());
    let visible = |playlist: &crate::api::models::Playlist| owner_filter.admits(playlist, owner);
    let mut depth = 0u8;
    // Rows inside a rolled-up folder stay off the list; the stack knows
    // how deep the rolled-up one sits.
    let mut hidden_from: Option<u8> = None;
    // Folders emptied by the ownership filter are skipped with their whole
    // subtree; the depth below remembers where the skipped folder started.
    let mut skipped_from: Option<u8> = None;
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for row in &app.rootlist {
        match row {
            RootlistEntry::FolderStart { id, name } => {
                if skipped_from.is_some() || hidden_from.is_some() {
                    depth += 1;
                    continue;
                }
                let count = if by_you {
                    folder_visible_playlists(&app.rootlist, &by_uri, &visible, id)
                } else {
                    folder_playlists(&app.rootlist, id)
                };
                if by_you && count == 0 {
                    skipped_from = Some(depth);
                    depth += 1;
                    continue;
                }
                let collapsed = app.collapsed_folders.contains(id);
                entries.push(Entry {
                    image: None,
                    grid_image: None,
                    name: if name.is_empty() {
                        gettext(app.locale, "Folder").into_owned()
                    } else {
                        name.clone()
                    },
                    subtitle: app.locale.folder_playlist_count(count as u32),
                    grid_subtitle: app.locale.playlist_count(count as u32),
                    page: Page::Home,
                    uri: String::new(),
                    round: false,
                    liked: false,
                    owned: false,
                    editable: false,
                    playlist_index: None,
                    folder: Some((id.clone(), collapsed, count)),
                    depth,
                    added_at: None,
                });
                if collapsed {
                    hidden_from = Some(depth);
                }
                depth += 1;
            }
            RootlistEntry::FolderEnd => {
                depth = depth.saturating_sub(1);
                if skipped_from == Some(depth) {
                    skipped_from = None;
                }
                if hidden_from == Some(depth) {
                    hidden_from = None;
                }
            }
            RootlistEntry::Playlist(uri) => {
                if skipped_from.is_some() {
                    continue;
                }
                let Some((index, playlist)) = by_uri.get(uri.as_str()) else {
                    continue;
                };
                if !visible(playlist) {
                    continue;
                }
                if !seen.insert(uri.as_str()) {
                    continue;
                }
                if hidden_from.is_some() && !app.settings.pinned_contexts.contains(uri) {
                    continue;
                }
                entries.push(playlist_entry(
                    app.locale,
                    playlist,
                    *index,
                    user_id,
                    app.can_edit_playlist(playlist),
                    depth,
                    EntryArt::of(app),
                ));
            }
        }
    }
    // Playlists the rootlist has not met yet, the newly followed, wait at
    // the end rather than vanish.
    for (index, playlist) in playlists.iter().enumerate() {
        if !visible(playlist) {
            continue;
        }
        if !seen.contains(playlist.uri.as_str()) {
            entries.push(playlist_entry(
                app.locale,
                playlist,
                index,
                user_id,
                app.can_edit_playlist(playlist),
                0,
                EntryArt::of(app),
            ));
        }
    }
}

/// How many playlists a folder holds that pass the ownership filter,
/// nested ones included. Counts occurrences like `folder_playlists`.
fn folder_visible_playlists(
    rootlist: &[crate::player::RootlistEntry],
    by_uri: &std::collections::HashMap<&str, (usize, &crate::api::models::Playlist)>,
    visible: &impl Fn(&crate::api::models::Playlist) -> bool,
    id: &str,
) -> usize {
    use crate::player::RootlistEntry;
    let mut counting = false;
    let mut depth = 0usize;
    let mut count = 0;
    for row in rootlist {
        match row {
            RootlistEntry::FolderStart { id: this, .. } => {
                if counting {
                    depth += 1;
                } else if this == id {
                    counting = true;
                    depth = 1;
                }
            }
            RootlistEntry::FolderEnd if counting => {
                depth -= 1;
                if depth == 0 {
                    return count;
                }
            }
            RootlistEntry::Playlist(uri)
                if counting
                    && by_uri
                        .get(uri.as_str())
                        .is_some_and(|(_, playlist)| visible(playlist)) =>
            {
                count += 1;
            }
            _ => {}
        }
    }
    count
}

/// How many playlists a folder holds, nested ones included.
fn folder_playlists(rootlist: &[crate::player::RootlistEntry], id: &str) -> usize {
    use crate::player::RootlistEntry;
    let mut counting = false;
    let mut depth = 0usize;
    let mut count = 0;
    for row in rootlist {
        match row {
            RootlistEntry::FolderStart { id: this, .. } => {
                if counting {
                    depth += 1;
                } else if this == id {
                    counting = true;
                    depth = 1;
                }
            }
            RootlistEntry::FolderEnd if counting => {
                depth -= 1;
                if depth == 0 {
                    return count;
                }
            }
            RootlistEntry::Playlist(_) if counting => count += 1,
            _ => {}
        }
    }
    count
}

/// The artwork widths a row and a grid card ask for on this display.
#[derive(Clone, Copy)]
struct EntryArt {
    row: u32,
    grid: u32,
}

impl EntryArt {
    fn of(app: &App) -> Self {
        Self {
            row: app.art_px(ROW_ART),
            grid: app.art_px(super::widgets::CARD_ART),
        }
    }
}

fn playlist_entry(
    locale: Locale,
    playlist: &crate::api::models::Playlist,
    index: usize,
    user_id: &str,
    editable: bool,
    depth: u8,
    art: EntryArt,
) -> Entry {
    Entry {
        image: pick_image(&playlist.images, art.row).map(str::to_string),
        grid_image: pick_image(&playlist.images, art.grid).map(str::to_string),
        name: playlist.name.clone(),
        // Translators: {owner} is the name of the playlist's owner.
        subtitle: gettext(locale, "Playlist • {owner}").replace("{owner}", playlist.owner_name()),
        grid_subtitle: playlist.owner_name().to_string(),
        page: Page::Playlist(playlist.id.clone()),
        uri: playlist.uri.clone(),
        round: false,
        liked: false,
        owned: playlist.owned_by(user_id),
        editable,
        playlist_index: Some(index),
        folder: None,
        depth,
        added_at: None,
    }
}

fn nav_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    icon: Icon,
    label: &str,
    active: bool,
    collapsed: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), NAV_ROW_HEIGHT), Sense::click());
    if ui.is_rect_visible(rect) {
        let color = if active || response.hovered() {
            palette.text
        } else {
            palette.secondary
        };
        let icon_x = if collapsed {
            rect.center().x
        } else {
            rect.left() + 12.0
        };
        let icon_rect = Rect::from_center_size(pos2(icon_x, rect.center().y), Vec2::splat(24.0));
        icon.image(color, 24.0).paint_at(ui, icon_rect);
        if !collapsed {
            crate::bidi::paint_line(
                &ui.painter().with_clip_rect(rect),
                rect.left() + 44.0,
                rect.right(),
                rect.center().y,
                label,
                theme::bold(15.0),
                color,
            );
        }
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), active, label)
    });
    theme::focus_ring(ui, &response);
    if collapsed {
        response.on_hover_text(label)
    } else {
        response
    }
}

/// The state of the shelf controls that lives only for this session.
struct ShelfState {
    filter: Filter,
    show_search: bool,
    by_spotify: bool,
}

const FILTER_ID: &str = "sidebar-filter";
const SHOW_SEARCH_ID: &str = "sidebar-show-search";
const BY_SPOTIFY_ID: &str = "sidebar-by-spotify";

fn shelf_state(ui: &egui::Ui) -> ShelfState {
    ui.data(|data| ShelfState {
        filter: data
            .get_temp::<Filter>(egui::Id::new(FILTER_ID))
            .unwrap_or_default(),
        show_search: data
            .get_temp::<bool>(egui::Id::new(SHOW_SEARCH_ID))
            .unwrap_or(false),
        by_spotify: data
            .get_temp::<bool>(egui::Id::new(BY_SPOTIFY_ID))
            .unwrap_or(false),
    })
}

fn store_shelf_state(ui: &egui::Ui, state: &ShelfState) {
    ui.data_mut(|data| {
        data.insert_temp(egui::Id::new(FILTER_ID), state.filter);
        data.insert_temp(egui::Id::new(SHOW_SEARCH_ID), state.show_search);
        data.insert_temp(egui::Id::new(BY_SPOTIFY_ID), state.by_spotify);
    });
}

/// The rows a shelf shows, before ordering, with its loading state.
struct Listing {
    entries: Vec<Entry>,
    loading: bool,
    error: Option<String>,
    more_page: Option<Page>,
}

/// Ask for the selected shelf while it has not loaded, or for the rest of
/// it while a sort other than the library's own needs every row.
fn load_shelf(app: &mut App, filter: Filter, sort: LibrarySort) {
    match filter {
        Filter::Playlists => {}
        Filter::Albums => {
            if !app.library.albums.loading
                && app.library.albums.error.is_none()
                && (!app.library.albums.loaded_once
                    || (sort != LibrarySort::Library && app.library.albums.can_load_more()))
            {
                app.actions.push(Action::LoadMore(Page::Albums));
            }
        }
        Filter::Artists => {
            if !app.library.artists.loading
                && app.library.artists.error.is_none()
                && (!app.library.artists.loaded_once
                    || (sort != LibrarySort::Library && app.library.artists.can_load_more()))
            {
                app.actions.push(Action::LoadMore(Page::Artists));
            }
        }
        Filter::Podcasts => {
            if !app.library.shows.loading
                && app.library.shows.error.is_none()
                && (!app.library.shows.loaded_once
                    || (sort != LibrarySort::Library && app.library.shows.can_load_more()))
            {
                app.actions.push(Action::LoadMore(Page::Podcasts));
            }
        }
    }
}

/// Collect the selected shelf's rows matching `needle`.
fn library_entries(
    app: &App,
    filter: Filter,
    sort: LibrarySort,
    owner: Ownership,
    needle: &str,
) -> Listing {
    let locale = app.locale;
    let needle = needle.to_string();
    let mut entries: Vec<Entry> = Vec::new();
    let art = EntryArt::of(app);
    let mut loading = false;
    let mut error: Option<String> = None;
    let mut more_page: Option<Page> = None;
    match filter {
        Filter::Playlists => {
            playlist_rows_of(app, sort, &needle, owner, &mut entries);
            match &app.library.playlists {
                Loadable::Loaded(_) => {}
                Loadable::Loading | Loadable::NotLoaded => loading = true,
                Loadable::Failed(message) => error = Some(message.clone()),
            }
        }
        Filter::Albums => {
            for saved in &app.library.albums.items {
                let album = &saved.album;
                if !needle.is_empty()
                    && !album.name.to_lowercase().contains(&needle)
                    && !album
                        .artists
                        .iter()
                        .any(|a| a.name.to_lowercase().contains(&needle))
                {
                    continue;
                }
                let artists = crate::api::models::join_names(
                    album.artists.iter().map(|artist| artist.name.as_str()),
                );
                entries.push(Entry {
                    image: pick_image(&album.images, art.row).map(str::to_string),
                    grid_image: pick_image(&album.images, art.grid).map(str::to_string),
                    name: album.name.clone(),
                    subtitle: format!("{} • {artists}", app.album_kind_label(album)),
                    grid_subtitle: artists,
                    page: Page::Album(album.id.clone()),
                    uri: album.uri.clone(),
                    round: false,
                    liked: false,
                    owned: false,
                    editable: false,
                    playlist_index: None,
                    folder: None,
                    depth: 0,
                    added_at: saved_time(saved.added_at.as_deref()),
                });
            }
            loading = app.library.albums.loading && app.library.albums.items.is_empty();
            error = app.library.albums.error.clone();
            if app.library.albums.error.is_none() && app.library.albums.can_load_more() {
                more_page = Some(Page::Albums);
            }
        }
        Filter::Artists => {
            for artist in &app.library.artists.items {
                if !needle.is_empty() && !artist.name.to_lowercase().contains(&needle) {
                    continue;
                }
                entries.push(Entry {
                    image: pick_image(&artist.images, art.row).map(str::to_string),
                    grid_image: pick_image(&artist.images, art.grid).map(str::to_string),
                    name: artist.name.clone(),
                    subtitle: gettext(locale, "Artist").into_owned(),
                    grid_subtitle: String::new(),
                    page: Page::Artist(artist.id.clone()),
                    uri: artist.uri.clone(),
                    round: true,
                    liked: false,
                    owned: false,
                    editable: false,
                    playlist_index: None,
                    folder: None,
                    depth: 0,
                    added_at: None,
                });
            }
            loading = app.library.artists.loading && app.library.artists.items.is_empty();
            error = app.library.artists.error.clone();
            if app.library.artists.error.is_none() && app.library.artists.can_load_more() {
                more_page = Some(Page::Artists);
            }
        }
        Filter::Podcasts => {
            for saved in &app.library.shows.items {
                let show = &saved.show;
                // Audiobooks arrive as shows, but librespot can't play them.
                if app.audiobook_shows.contains(&show.uri) {
                    continue;
                }
                if !needle.is_empty() && !show.name.to_lowercase().contains(&needle) {
                    continue;
                }
                entries.push(Entry {
                    image: pick_image(&show.images, art.row).map(str::to_string),
                    grid_image: pick_image(&show.images, art.grid).map(str::to_string),
                    name: show.name.clone(),
                    // Translators: {publisher} is the podcast's publisher.
                    subtitle: gettext(locale, "Podcast • {publisher}")
                        .replace("{publisher}", &show.publisher),
                    grid_subtitle: show.publisher.clone(),
                    page: Page::Show(show.id.clone()),
                    uri: show.uri.clone(),
                    round: false,
                    liked: false,
                    owned: false,
                    editable: false,
                    playlist_index: None,
                    folder: None,
                    depth: 0,
                    added_at: saved_time(saved.added_at.as_deref()),
                });
            }
            loading = app.library.shows.loading && app.library.shows.items.is_empty();
            error = app.library.shows.error.clone();
            if app.library.shows.error.is_none() && app.library.shows.can_load_more() {
                more_page = Some(Page::Podcasts);
            }
        }
    }

    Listing {
        entries,
        loading,
        error,
        more_page,
    }
}

/// The shelf's rows in the order they are drawn, with the shelf state.
fn ordered_listing(app: &App, state: &ShelfState, sort: LibrarySort) -> Listing {
    let needle = app.library.filter.trim().to_lowercase();
    let owner = Ownership::of(app, state.by_spotify);
    let mut listing = library_entries(app, state.filter, sort, owner, &needle);
    order_entries(app, state.filter, sort, &mut listing.entries);
    listing
}

/// A pill button in Spotify's library style: the filter chips and Create.
/// Selected chips turn white with dark text.
fn pill(
    ui: &mut egui::Ui,
    palette: &Palette,
    icon: Option<Icon>,
    text: &str,
    label: &str,
    selected: bool,
) -> egui::Response {
    let font = if icon.is_some() {
        theme::bold(14.0)
    } else {
        theme::regular(14.0)
    };
    let color = if selected {
        palette.window
    } else {
        palette.text
    };
    let galley = crate::bidi::layout_line(ui.painter(), text, font, color);
    let icon_size = 16.0;
    let icon_width = if icon.is_some() { icon_size + 8.0 } else { 0.0 };
    let padding = if icon.is_some() { 16.0 } else { 12.0 };
    let width = if text.is_empty() {
        PILL_HEIGHT
    } else {
        galley.size().x + icon_width + 2.0 * padding
    };
    let (rect, response) = ui.allocate_exact_size(vec2(width, PILL_HEIGHT), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), selected, label)
    });
    if ui.is_rect_visible(rect) {
        let fill = match (selected, response.hovered()) {
            (true, false) => palette.text,
            (true, true) => palette.text.gamma_multiply(0.9),
            (false, false) => palette.surface,
            (false, true) => palette.surface_hover,
        };
        ui.painter()
            .rect_filled(rect, CornerRadius::same(127), fill);
        let mut left = rect.left() + padding;
        if let Some(icon) = icon {
            let center = if text.is_empty() {
                rect.center()
            } else {
                pos2(left + icon_size / 2.0, rect.center().y)
            };
            icon.image(color, icon_size)
                .paint_at(ui, Rect::from_center_size(center, Vec2::splat(icon_size)));
            left += icon_width;
        }
        if !text.is_empty() {
            ui.painter().galley(
                pos2(left, rect.center().y - galley.size().y / 2.0),
                galley,
                color,
            );
        }
    }
    theme::focus_ring(ui, &response);
    if text.is_empty() || text != label {
        response.on_hover_text(label)
    } else {
        response
    }
}

const PILL_HEIGHT: f32 = 32.0;

/// A round icon button that lights up under the pointer, as Spotify's
/// library header buttons do.
fn round_button(ui: &mut egui::Ui, palette: &Palette, icon: Icon, label: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(PILL_HEIGHT), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered() || response.has_focus();
        if hovered {
            ui.painter()
                .circle_filled(rect.center(), rect.width() / 2.0, palette.surface);
        }
        let color = if hovered {
            palette.text
        } else {
            palette.secondary
        };
        theme::paint_icon(ui, icon, rect, 16.0, color);
    }
    theme::focus_ring(ui, &response);
    response.on_hover_text(label)
}

/// "Your Library", which collapses the sidebar, then Create, Show more or
/// less, and hiding the sidebar. Create gives up its label first, then the
/// heading shrinks and finally gives way in the narrowest sidebar.
fn library_header(app: &mut App, outer: &mut egui::Ui) {
    let palette = app.palette;
    let locale = app.locale;
    let row = Rect::from_min_size(outer.cursor().min, vec2(outer.available_width(), 40.0));
    let mut ui = outer.new_child(
        egui::UiBuilder::new()
            .max_rect(row.shrink2(vec2(8.0, 0.0)))
            .layout(Layout::right_to_left(Align::Center)),
    );
    outer.advance_cursor_after_rect(row);
    let ui = &mut ui;
    ui.spacing_mut().item_spacing.x = 4.0;
    if round_button(
        ui,
        &palette,
        Icon::PanelLeft,
        super::keys::platform_shortcut(
            &gettext(locale, "Hide sidebar (Ctrl+B)"),
            &gettext(locale, "Hide sidebar (Cmd+B)"),
        ),
    )
    .clicked()
    {
        app.actions.push(Action::ToggleSidebar);
    }
    let wide = app.settings.sidebar_width >= WIDE_WIDTH - 1.0;
    let (icon, label) = if wide {
        (Icon::Shrink, gettext(locale, "Show less"))
    } else {
        (Icon::Expand, gettext(locale, "Show more"))
    };
    if round_button(ui, &palette, icon, &label).clicked() {
        let width = if wide {
            restore_width(ui.ctx())
        } else {
            WIDE_WIDTH
        };
        set_sidebar_width(app, ui.ctx(), width);
    }
    ui.add_space(4.0);
    let create_label = gettext(locale, "Create a playlist");
    let create_text = gettext(locale, "Create");
    let heading = gettext(locale, "Your Library");
    let painter = ui.painter().clone();
    let heading_width = |size: f32| {
        painter
            .layout_no_wrap(heading.to_string(), theme::bold(size), palette.text)
            .size()
            .x
    };
    let full_create = ui
        .painter()
        .layout_no_wrap(create_text.to_string(), theme::bold(14.0), palette.text)
        .size()
        .x
        + 56.0;
    // The pill keeps its label while the heading still fits beside it.
    let with_label = ui.available_width() - full_create - 12.0 >= heading_width(16.0);
    if pill(
        ui,
        &palette,
        Some(Icon::Plus),
        if with_label { &create_text } else { "" },
        &create_label,
        false,
    )
    .clicked()
    {
        // One item never deserved a menu: Create makes a playlist directly.
        app.actions.push(Action::ShowDialog(Dialog::CreatePlaylist {
            name: String::new(),
            public: false,
            add_uris: Vec::new(),
        }));
    }
    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
        let room = ui.available_width() - 8.0;
        let Some(size) = [16.0, 15.0, 14.0, 13.0]
            .into_iter()
            .find(|&size| heading_width(size) <= room)
        else {
            return;
        };
        let galley = crate::bidi::layout_line(
            ui.painter(),
            heading.as_ref(),
            theme::bold(size),
            palette.text,
        );
        let (rect, response) =
            ui.allocate_exact_size(vec2(galley.size().x + 8.0, PILL_HEIGHT), Sense::click());
        let collapse = gettext(locale, "Collapse Your Library");
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), collapse.clone())
        });
        let color = if response.hovered() || response.has_focus() {
            palette.text
        } else {
            palette.text.gamma_multiply(0.92)
        };
        ui.painter().galley(
            pos2(rect.left() + 4.0, rect.center().y - galley.size().y / 2.0),
            galley,
            color,
        );
        theme::focus_ring(ui, &response);
        if response.on_hover_text(collapse.as_ref()).clicked() {
            set_sidebar_width(app, ui.ctx(), COLLAPSED_WIDTH);
        }
    });
}

/// The shelf chips in a row that scrolls sideways, with an arrow at either
/// end while more chips wait there. The cross in front clears the filters.
fn filter_chips(app: &mut App, ui: &mut egui::Ui, state: &mut ShelfState) {
    let palette = app.palette;
    let locale = app.locale;
    let by_you = app.settings.library_by_you;
    let filtered = state.filter != Filter::default() || by_you || state.by_spotify;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.add_space(8.0);
        if filtered && round_chip_clear(ui, &palette, &gettext(locale, "Clear filters")).clicked() {
            state.filter = Filter::default();
            state.by_spotify = false;
            if by_you {
                app.actions.push(Action::SetLibraryByYou(false));
            }
        }
        let scroll_id = egui::Id::new("sidebar-chips-scroll");
        let wanted = ui.data(|data| data.get_temp::<f32>(scroll_id));
        let mut area = egui::ScrollArea::horizontal()
            .id_salt("sidebar-chips")
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .max_width(ui.available_width() - 8.0);
        if let Some(offset) = wanted {
            area = area.horizontal_scroll_offset(offset);
            ui.data_mut(|data| data.remove::<f32>(scroll_id));
        }
        let output = area.show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                for (value, label) in [
                    (Filter::Playlists, gettext(locale, "Playlists")),
                    (Filter::Artists, gettext(locale, "Artists")),
                    (Filter::Albums, gettext(locale, "Albums")),
                    (Filter::Podcasts, gettext(locale, "Podcasts")),
                ] {
                    if pill(ui, &palette, None, &label, &label, state.filter == value).clicked() {
                        state.filter = value;
                    }
                    if value == Filter::Playlists && state.filter == Filter::Playlists {
                        // Translators: Library filter for playlists created by the signed-in user.
                        let label = gettext(locale, "By You");
                        if pill(ui, &palette, None, &label, &label, by_you).clicked() {
                            app.actions.push(Action::SetLibraryByYou(!by_you));
                            state.by_spotify = false;
                        }
                        // Translators: Library filter for playlists Spotify made, such as its mixes and editorial playlists.
                        let label = gettext(locale, "By Spotify");
                        if pill(
                            ui,
                            &palette,
                            None,
                            &label,
                            &label,
                            state.by_spotify && !by_you,
                        )
                        .clicked()
                        {
                            state.by_spotify = !(state.by_spotify && !by_you);
                            if by_you {
                                app.actions.push(Action::SetLibraryByYou(false));
                            }
                        }
                    }
                }
            });
        });
        let offset = output.state.offset.x;
        let view = output.inner_rect;
        let overflow = output.content_size.x - view.width();
        let step = view.width() * 0.6;
        let arrows = [
            (
                offset > 0.5,
                view.left(),
                Icon::ChevronLeft,
                (offset - step).max(0.0),
            ),
            (
                offset < overflow - 0.5,
                view.right(),
                Icon::ChevronRight,
                (offset + step).min(overflow.max(0.0)),
            ),
        ];
        for (shown, edge, icon, target) in arrows {
            if !shown {
                continue;
            }
            let leading = icon == Icon::ChevronLeft;
            let center = pos2(
                if leading {
                    edge + PILL_HEIGHT / 2.0
                } else {
                    edge - PILL_HEIGHT / 2.0
                },
                view.center().y,
            );
            let fade = Rect::from_min_max(
                pos2(
                    if leading {
                        edge
                    } else {
                        edge - 2.0 * PILL_HEIGHT
                    },
                    view.top(),
                ),
                pos2(
                    if leading {
                        edge + 2.0 * PILL_HEIGHT
                    } else {
                        edge
                    },
                    view.bottom(),
                ),
            );
            paint_horizontal_fade(ui, fade, palette.panel, leading);
            let rect = Rect::from_center_size(center, Vec2::splat(PILL_HEIGHT));
            let label = if leading {
                gettext(locale, "Previous")
            } else {
                gettext(locale, "Next")
            };
            let response = ui.interact(rect, ui.id().with(("chip-arrow", leading)), Sense::click());
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label.clone())
            });
            let fill = if response.hovered() {
                palette.surface_hover
            } else {
                palette.surface
            };
            ui.painter()
                .circle_filled(rect.center(), PILL_HEIGHT / 2.0, fill);
            theme::paint_icon(ui, icon, rect, 16.0, palette.text);
            if response.clicked() {
                ui.data_mut(|data| data.insert_temp(scroll_id, target));
                ui.ctx().request_repaint();
            }
        }
    });
}

/// The cross that clears the library filters.
fn round_chip_clear(ui: &mut egui::Ui, palette: &Palette, label: &str) -> egui::Response {
    pill(ui, palette, Some(Icon::X), "", label, false)
}

/// Fade the chips into the panel under a scroll arrow.
fn paint_horizontal_fade(ui: &egui::Ui, rect: Rect, color: egui::Color32, leading: bool) {
    let clear = egui::Color32::TRANSPARENT;
    let (left, right) = if leading {
        (color, clear)
    } else {
        (clear, color)
    };
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), left);
    mesh.colored_vertex(rect.right_top(), right);
    mesh.colored_vertex(rect.right_bottom(), right);
    mesh.colored_vertex(rect.left_bottom(), left);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    ui.painter().add(mesh);
}

/// The search button, which opens a field in its place, then the sort
/// menu named after the selected order and the list or grid toggle.
fn search_and_sort(app: &mut App, ui: &mut egui::Ui, state: &mut ShelfState, sort: LibrarySort) {
    let palette = app.palette;
    let locale = app.locale;
    let mut focus_search = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(8.0);
        let right = ui.max_rect().right();
        if state.show_search {
            let sort_room = sort_menu_width(app, ui, sort) + PILL_HEIGHT + 16.0;
            let width = (right - ui.cursor().left() - sort_room).max(PILL_HEIGHT * 2.0);
            let response = super::widgets::search_field(
                ui,
                &palette,
                app.locale,
                egui::Id::new("sidebar-search"),
                &mut app.library.filter,
                &gettext(locale, "Search in Your Library"),
                width,
            );
            // A pressed Escape or a click away from an empty field closes it.
            if response.lost_focus() && app.library.filter.is_empty() {
                state.show_search = false;
            }
        }
        if !state.show_search
            && round_button(
                ui,
                &palette,
                Icon::Search,
                &gettext(locale, "Search Your Library"),
            )
            .clicked()
        {
            state.show_search = true;
            focus_search = true;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(4.0);
            let grid = app.settings.sidebar_grid;
            let (icon, label) = if grid {
                (Icon::LayoutList, gettext(locale, "Show as list"))
            } else {
                (Icon::LayoutGrid, gettext(locale, "Show as grid"))
            };
            if round_button(ui, &palette, icon, &label).clicked() {
                app.actions.push(Action::SetLibraryGrid(!grid));
            }
            sort_menu(app, ui, state.filter, sort);
        });
    });
    if focus_search {
        ui.memory_mut(|memory| memory.request_focus(egui::Id::new("sidebar-search")));
    }
}

/// Draw Your Library: its header, shelf controls, search, and the rows.
/// Playlist ownership and text filters are applied before ordering and pin placement.
fn contents(app: &mut App, ui: &mut egui::Ui, grid_art: Option<Rect>, collapsed: bool) {
    let palette = app.palette;
    let mut state = shelf_state(ui);
    let sort = selected_sort(app, state.filter);
    if collapsed {
        collapsed_header(app, ui);
    } else {
        library_header(app, ui);
        ui.add_space(8.0);
        filter_chips(app, ui, &mut state);
        ui.add_space(8.0);
        search_and_sort(app, ui, &mut state, sort);
        store_shelf_state(ui, &state);
        ui.add_space(4.0);
    }
    let sort = selected_sort(app, state.filter);
    load_shelf(app, state.filter, sort);
    let filter = state.filter;
    let Listing {
        entries,
        loading,
        error,
        more_page,
    } = ordered_listing(app, &state, sort);
    let needle = app.library.filter.trim().to_lowercase();
    let custom_order = filter == Filter::Playlists && sort == LibrarySort::Local;
    let pins = app.settings.library_pins();
    let pinned_rows = entries
        .iter()
        .take_while(|entry| pins.iter().any(|key| key == entry.ordering_key()))
        .count();
    let playing_context = app.playing_context_uri();
    let context_playing = app.believed_playing();
    let current_page = app.page().clone();

    crate::autoscroll::show(
        ui,
        egui::ScrollArea::vertical()
            .id_salt("sidebar-list")
            .auto_shrink([false, false]),
        egui::Vec2b::new(false, true),
        |ui| {
            if egui::DragAndDrop::has_payload_of_type::<DragTrack>(ui.ctx())
                || egui::DragAndDrop::has_payload_of_type::<DragEntry>(ui.ctx())
            {
                super::widgets::scroll_during_drag(ui);
            }
            if loading {
                super::widgets::loading_row(ui, &palette, app.locale);
            }
            if let Some(error) = &error {
                super::widgets::error_row(ui, app, error, None);
            }
            if entries.is_empty() && !loading && error.is_none() {
                ui.add_space(12.0);
                theme::subtle(
                    ui,
                    &palette,
                    &if needle.is_empty() {
                        gettext(app.locale, "Nothing here yet.")
                    } else {
                        gettext(app.locale, "No matches.")
                    },
                );
            }
            if app.settings.sidebar_grid && !collapsed {
                library_grid(
                    app,
                    ui,
                    &entries,
                    filter,
                    custom_order,
                    pinned_rows,
                    grid_art,
                );
                if let Some(page) = more_page {
                    super::widgets::load_more_when_near_end(ui, app, page, true);
                }
                ui.add_space(grid_art.map_or(0.0, |rect| ui.max_rect().bottom() - rect.top()));
                return;
            }

            let compact = app.settings.sidebar_compact && !collapsed;
            let row_height = if compact {
                COMPACT_ROW_HEIGHT
            } else {
                DEFAULT_ROW_HEIGHT
            };
            // Calculate drop positions from fixed row height because rows shift
            // before drawing.
            let list_top = ui.cursor().top();
            let pointer = ui.ctx().pointer_latest_pos().filter(|pos| {
                ui.clip_rect().contains(*pos) && ui.rect_contains_pointer(ui.clip_rect())
            });
            // Tracks may drop on Liked Songs or playlists that take songs
            // from this account.
            let dragging_song = egui::DragAndDrop::has_payload_of_type::<DragTrack>(ui.ctx());
            let drop_target = dragging_song
                .then_some(pointer)
                .flatten()
                .map(|pos| ((pos.y - list_top) / row_height).floor())
                .filter(|row| *row >= 0.0 && *row < entries.len() as f32)
                .map(|row| row as usize)
                .filter(|row| entries[*row].liked || entries[*row].editable);
            // Sidebar entries, including Liked Songs, drop between rows.
            let reordering = egui::DragAndDrop::has_payload_of_type::<DragEntry>(ui.ctx());
            let reorder_slot = reordering.then_some(pointer).flatten().map(|pos| {
                (((pos.y - list_top) / row_height).round().max(0.0) as usize).min(entries.len())
            });
            let art = app.backend.art().clone();
            super::widgets::virtual_rows(ui, entries.len(), row_height, |ui, index| {
                let entry = &entries[index];
                if !compact && let Some(image) = &entry.image {
                    // Prepare the enlarged preview before this row is opened.
                    app.softened_covers.texture(ui.ctx(), &art, image);
                }
                let droppable = entry.liked || entry.editable;
                let drop_hover = drop_target == Some(index);
                let active = entry.folder.is_none() && entry.page == current_page;
                // Liked Songs has no URI of its own here; Spotify plays it
                // as the account's collection context.
                let current = entry_is_playing_context(entry, playing_context.as_deref());
                let playing = context_playing && current;
                let pinned = pins.iter().any(|key| key == entry.ordering_key());
                let (_, rect) = ui.allocate_space(vec2(ui.available_width(), row_height));
                let id = ui.id().with((
                    "library-row",
                    &entry.uri,
                    entry.liked,
                    entry.folder.as_ref().map(|(id, _, _)| id),
                ));
                let response = ui.interact(rect, id, Sense::click_and_drag());
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::Button,
                        ui.is_enabled(),
                        active,
                        if let Some((_, collapsed, _)) = &entry.folder {
                            app.locale.folder_state_label(&entry.name, *collapsed)
                        } else {
                            entry.name.clone()
                        },
                    )
                });
                // Animate rows around the current track or entry drop target.
                let shift = ui.ctx().animate_value_with_time(
                    ui.id().with(("drop-shift", index)),
                    if let Some(slot) = reorder_slot {
                        if index < slot { -4.0 } else { 4.0 }
                    } else {
                        match drop_target {
                            Some(target) if index < target => -4.0,
                            Some(target) if index > target => 4.0,
                            _ => 0.0,
                        }
                    },
                    crate::motion::seconds(ui.ctx(), 0.12),
                );
                let rect = rect.translate(vec2(0.0, shift));
                // Set when the cover play button takes a click, so a double
                // click on it does not also play from the row.
                let mut cover_took_click = false;
                if ui.is_rect_visible(rect) {
                    cover_took_click = paint_list_row(
                        app,
                        ui,
                        ListRow {
                            entry,
                            index,
                            rect,
                            response: &response,
                            active,
                            current,
                            playing,
                            pinned,
                            drop_hover,
                            dims: dragging_song && !droppable,
                            compact,
                            collapsed,
                        },
                    );
                }
                finish_entry_interaction(
                    app,
                    ui,
                    &response,
                    entry,
                    cover_took_click,
                    custom_order,
                    true,
                );
            });
            if let Some(slot) = reorder_slot {
                // A line in the gap the rows opened, so the eye lands
                // where the row will.
                let y = list_top + slot as f32 * row_height;
                ui.painter().hline(
                    ui.max_rect().x_range().shrink(6.0),
                    y,
                    egui::Stroke::new(2.0, palette.accent),
                );
                if ui.input(|input| input.pointer.button_released(egui::PointerButton::Primary))
                    && let Some(drag) = egui::DragAndDrop::take_payload::<DragEntry>(ui.ctx())
                {
                    if filter == Filter::Playlists {
                        drop_playlist_row(app, &entries, pinned_rows, slot, &drag.uri);
                    } else {
                        drop_row(app, &entries, pinned_rows, slot, &drag.uri);
                    }
                }
            }
            if let Some(page) = more_page {
                super::widgets::load_more_when_near_end(ui, app, page, true);
            }
            ui.add_space(PANEL_GAP);
        },
    );
}

/// The collapsed sidebar's header: the library icon that expands it again
/// and the plus that creates a playlist.
fn collapsed_header(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let locale = app.locale;
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.add_space(4.0);
        let expand = gettext(locale, "Expand Your Library");
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), expand.clone())
        });
        let color = if response.hovered() || response.has_focus() {
            palette.text
        } else {
            palette.secondary
        };
        theme::paint_icon(ui, Icon::Library, rect, 24.0, color);
        theme::focus_ring(ui, &response);
        if response.on_hover_text(expand.as_ref()).clicked() {
            let width = restore_width(ui.ctx());
            set_sidebar_width(app, ui.ctx(), width);
        }
        if pill(
            ui,
            &palette,
            Some(Icon::Plus),
            "",
            &gettext(locale, "Create a playlist"),
            false,
        )
        .clicked()
        {
            app.actions.push(Action::ShowDialog(Dialog::CreatePlaylist {
                name: String::new(),
                public: false,
                add_uris: Vec::new(),
            }));
        }
        ui.add_space(8.0);
    });
}

/// One list row, as `paint_list_row` draws it.
struct ListRow<'a> {
    entry: &'a Entry,
    index: usize,
    rect: Rect,
    response: &'a egui::Response,
    active: bool,
    /// The row's context is the playing one, paused or not.
    current: bool,
    playing: bool,
    pinned: bool,
    drop_hover: bool,
    /// A song is being dragged and this row cannot take it.
    dims: bool,
    compact: bool,
    collapsed: bool,
}

/// Paint a Your Library row the way Spotify does: a 48px cover, round for
/// artists, the title (green while it plays) over "Playlist • owner" with a
/// green pin in front when pinned, and the speaker of the playing context
/// or a folder's chevron at the end. Compact rows hold one line of text and
/// collapsed ones only the cover. Returns whether the cover's play button
/// took the click.
fn paint_list_row(app: &mut App, ui: &mut egui::Ui, row: ListRow) -> bool {
    let palette = app.palette;
    let ListRow {
        entry,
        index,
        rect,
        response,
        active,
        current,
        playing,
        pinned,
        drop_hover,
        dims,
        compact,
        collapsed,
    } = row;
    let radius = CornerRadius::same(ROW_RADIUS);
    if active {
        ui.painter()
            .rect_filled(rect, radius, palette.surface_active);
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, radius, palette.surface_hover);
    }
    if drop_hover {
        ui.painter()
            .rect_filled(rect, radius, palette.accent.gamma_multiply(0.18));
        ui.painter().rect_stroke(
            rect,
            radius,
            egui::Stroke::new(1.5, palette.accent),
            egui::StrokeKind::Inside,
        );
    }
    let name_color = if current {
        palette.accent
    } else {
        palette.text
    };
    let indent = if collapsed {
        0.0
    } else {
        f32::from(entry.depth.min(5)) * FOLDER_INDENT
    };
    let mut cover_took_click = false;
    let text_left = if compact {
        rect.left() + LIBRARY_ITEM_PADDING + indent
    } else {
        let cover_rect = Rect::from_min_size(
            pos2(
                if collapsed {
                    rect.center().x - ROW_ART / 2.0
                } else {
                    rect.left() + LIBRARY_ITEM_PADDING + indent
                },
                rect.center().y - ROW_ART / 2.0,
            ),
            Vec2::splat(ROW_ART),
        );
        if entry.liked {
            liked_cover(ui, cover_rect, COVER_RADIUS);
        } else if entry.folder.is_some() {
            folder_cover(ui, &palette, cover_rect);
        } else {
            super::widgets::paint_cover(
                ui,
                &palette,
                entry.image.as_deref(),
                cover_rect,
                if entry.round {
                    ROW_ART / 2.0
                } else {
                    COVER_RADIUS
                },
                if entry.round { Icon::User } else { Icon::Music },
                Some(app.backend.art()),
            );
        }
        if entry.folder.is_none() {
            // Hovering the art offers to play right from here.
            cover_took_click =
                cover_play_button(app, ui, entry, index, cover_rect, response, current);
        }
        cover_rect.right() + 12.0
    };
    if collapsed {
        if playing {
            // The collapsed column keeps a small speaker on the playing cover.
            let badge = Rect::from_center_size(
                pos2(rect.right() - 9.0, rect.top() + 9.0),
                Vec2::splat(16.0),
            );
            ui.painter()
                .circle_filled(badge.center(), 9.0, palette.panel);
            Icon::Volume2.image(palette.accent, 12.0).paint_at(
                ui,
                Rect::from_center_size(badge.center(), Vec2::splat(12.0)),
            );
        }
    } else {
        let trailing = if playing || entry.folder.is_some() {
            32.0
        } else {
            LIBRARY_ITEM_PADDING
        };
        let text_right = rect.right() - trailing;
        let painter = ui.painter().with_clip_rect(
            Rect::from_min_max(pos2(text_left, rect.top()), pos2(text_right, rect.bottom()))
                .intersect(ui.clip_rect()),
        );
        let title_font = theme::regular(if compact { 14.0 } else { 15.0 });
        let subtitle_font = theme::regular(if compact { 14.0 } else { 13.0 });
        let title_y = if compact {
            rect.center().y
        } else {
            rect.center().y - 10.0
        };
        let title = crate::bidi::paint_line(
            &painter,
            text_left,
            text_right,
            title_y,
            &entry.name,
            title_font,
            name_color,
        );
        let (mut subtitle_left, subtitle_y) = if compact {
            (title.right() + 2.0, rect.center().y)
        } else {
            (text_left, rect.center().y + 11.0)
        };
        if compact {
            let dot = painter.text(
                pos2(subtitle_left, subtitle_y),
                egui::Align2::LEFT_CENTER,
                " • ",
                subtitle_font.clone(),
                palette.secondary,
            );
            subtitle_left = dot.right();
        }
        if pinned {
            let pin = Rect::from_min_size(pos2(subtitle_left, subtitle_y - 6.0), Vec2::splat(12.0));
            Icon::Pin.image(palette.accent, 12.0).paint_at(ui, pin);
            subtitle_left = pin.right() + 6.0;
        }
        if subtitle_left < text_right {
            crate::bidi::paint_line(
                &painter,
                subtitle_left,
                text_right,
                subtitle_y,
                &entry.subtitle,
                subtitle_font,
                palette.secondary,
            );
        }
        let end = Rect::from_center_size(
            pos2(rect.right() - 18.0, rect.center().y),
            Vec2::splat(16.0),
        );
        if playing {
            Icon::Volume2.image(palette.accent, 16.0).paint_at(ui, end);
        } else if let Some((_, folded, _)) = &entry.folder {
            let chevron = if *folded {
                Icon::ChevronRight
            } else {
                Icon::ChevronDown
            };
            let color = if response.hovered() {
                palette.text
            } else {
                palette.secondary
            };
            chevron.image(color, 16.0).paint_at(ui, end);
        }
    }
    // Rows that cannot take the song step back a little.
    if dims {
        ui.painter()
            .rect_filled(rect, radius, palette.panel.gamma_multiply(0.5));
    }
    cover_took_click
}

/// A folder's tile: a folder outline on the raised grey Spotify gives
/// covers it has no picture for.
fn folder_cover(ui: &egui::Ui, palette: &Palette, rect: Rect) {
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(COVER_RADIUS as u8),
        palette.surface_hover,
    );
    let size = rect.width() * 0.46;
    let glyph = Rect::from_center_size(rect.center(), vec2(size, size * 0.8));
    let tab = glyph.width() * 0.4;
    let stroke = egui::Stroke::new((rect.width() / 24.0).max(1.5), palette.secondary);
    let points = vec![
        glyph.left_bottom(),
        glyph.left_top(),
        pos2(glyph.left() + tab, glyph.top()),
        pos2(
            glyph.left() + tab + glyph.height() * 0.15,
            glyph.top() + glyph.height() * 0.18,
        ),
        pos2(glyph.right(), glyph.top() + glyph.height() * 0.18),
        glyph.right_bottom(),
    ];
    ui.painter().add(egui::Shape::closed_line(points, stroke));
}

/// A row's corners, a cover's corners, and the indent of each folder level.
const ROW_RADIUS: u8 = 6;
const COVER_RADIUS: f32 = 4.0;
const FOLDER_INDENT: f32 = 16.0;

const EXPANDED_ART_GAP: f32 = 10.0;
const LIBRARY_ITEM_PADDING: f32 = 8.0;
/// The cover of a list row.
const ROW_ART: f32 = 48.0;
const GRID_MIN_CARD_WIDTH: f32 = 108.0;
const GRID_MAX_COLUMNS: usize = 4;
const GRID_TEXT_HEIGHT: f32 = 44.0;

#[derive(Clone, Copy)]
struct GridLayout {
    columns: usize,
    card_width: f32,
    card_height: f32,
    row_height: f32,
}

fn grid_layout(width: f32) -> GridLayout {
    let columns = ((width / GRID_MIN_CARD_WIDTH).floor() as usize).clamp(2, GRID_MAX_COLUMNS);
    let card_width = (width / columns as f32).max(1.0);
    let card_height = card_width + GRID_TEXT_HEIGHT;
    GridLayout {
        columns,
        card_width,
        card_height,
        row_height: card_height,
    }
}

fn paint_grid_text(
    ui: &egui::Ui,
    rect: Rect,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
) {
    let galley = crate::bidi::layout(
        ui.painter(),
        text,
        font,
        color,
        rect.width(),
        1,
        Some(crate::bidi::ELLIPSIS),
    );
    ui.painter()
        .galley(crate::bidi::galley_pos(rect, &galley), galley, color);
}

fn grid_reorder_slot(
    pointer: egui::Pos2,
    origin: egui::Pos2,
    layout: GridLayout,
    count: usize,
) -> usize {
    let row = ((pointer.y - origin.y) / layout.row_height)
        .floor()
        .max(0.0) as usize;
    let column = ((pointer.x - origin.x) / layout.card_width)
        .floor()
        .max(0.0)
        .min((layout.columns - 1) as f32) as usize;
    let after = pointer.x - origin.x - column as f32 * layout.card_width > layout.card_width / 2.0;
    (row * layout.columns + column + usize::from(after)).min(count)
}

fn library_grid(
    app: &mut App,
    ui: &mut egui::Ui,
    entries: &[Entry],
    filter: Filter,
    custom_order: bool,
    pinned_rows: usize,
    grid_art: Option<Rect>,
) {
    if entries.is_empty() {
        return;
    }
    let palette = app.palette;
    let pins = app.settings.library_pins();
    let playing_context = app.playing_context_uri();
    let current_page = app.page().clone();
    let layout = grid_layout(ui.available_width());
    let origin = ui.cursor().min;
    let pointer = ui.ctx().pointer_latest_pos().filter(|pos| {
        ui.clip_rect().contains(*pos)
            && ui.rect_contains_pointer(ui.clip_rect())
            && !grid_art.is_some_and(|art| art.contains(*pos))
    });
    let dragging_song = egui::DragAndDrop::has_payload_of_type::<DragTrack>(ui.ctx());
    let reordering = egui::DragAndDrop::has_payload_of_type::<DragEntry>(ui.ctx());
    let reorder_slot = reordering
        .then_some(pointer)
        .flatten()
        .map(|pointer| grid_reorder_slot(pointer, origin, layout, entries.len()));
    let row_count = entries.len().div_ceil(layout.columns);
    let grid_id = ui.unique_id().with("library-grid");

    super::widgets::virtual_rows(ui, row_count, layout.row_height, |ui, row| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let start = row * layout.columns;
            let end = (start + layout.columns).min(entries.len());
            for index in start..end {
                let entry = &entries[index];
                let entry_id = (
                    entry.ordering_key(),
                    entry.liked,
                    entry.folder.as_ref().map(|(id, _, _)| id.as_str()),
                );
                ui.scope_builder(egui::UiBuilder::new().id(grid_id.with(entry_id)), |ui| {
                    let active = entry.folder.is_none() && entry.page == current_page;
                    let playing_here = entry_is_playing_context(entry, playing_context.as_deref());
                    let pinned = pins.iter().any(|key| key == entry.ordering_key());
                    let droppable = entry.liked || entry.editable;
                    let (rect, response) = ui.allocate_exact_size(
                        vec2(layout.card_width, layout.card_height),
                        Sense::click_and_drag(),
                    );
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::WidgetType::Button,
                            ui.is_enabled(),
                            active,
                            if let Some((_, collapsed, _)) = &entry.folder {
                                app.locale.folder_state_label(&entry.name, *collapsed)
                            } else {
                                entry.name.clone()
                            },
                        )
                    });
                    let cover_rect = Rect::from_min_size(
                        rect.min + Vec2::splat(LIBRARY_ITEM_PADDING),
                        Vec2::splat(layout.card_width - LIBRARY_ITEM_PADDING * 2.0),
                    );
                    let mut cover_took_click = false;
                    if ui.is_rect_visible(rect) {
                        if active {
                            ui.painter().rect_filled(
                                rect,
                                CornerRadius::same(ROW_RADIUS),
                                palette.surface_active,
                            );
                        } else if response.hovered() {
                            ui.painter().rect_filled(
                                rect,
                                CornerRadius::same(ROW_RADIUS),
                                palette.surface_hover,
                            );
                        }
                        if entry.liked {
                            liked_cover(ui, cover_rect, COVER_RADIUS);
                        } else if entry.folder.is_some() {
                            folder_cover(ui, &palette, cover_rect);
                        } else {
                            super::widgets::paint_cover(
                                ui,
                                &palette,
                                entry.grid_image.as_deref().or(entry.image.as_deref()),
                                cover_rect,
                                if entry.round {
                                    layout.card_width / 2.0
                                } else {
                                    COVER_RADIUS
                                },
                                if entry.round { Icon::User } else { Icon::Music },
                                Some(app.backend.art()),
                            );
                        }

                        cover_took_click =
                            grid_play_button(app, ui, entry, cover_rect, &response, playing_here);

                        let text_left = rect.left() + LIBRARY_ITEM_PADDING;
                        let text_width = (rect.width() - LIBRARY_ITEM_PADDING * 2.0).max(0.0);
                        paint_grid_text(
                            ui,
                            Rect::from_min_size(
                                pos2(text_left, cover_rect.bottom() + 4.0),
                                vec2(text_width, 18.0),
                            ),
                            &entry.name,
                            theme::medium(13.5),
                            if playing_here {
                                palette.accent
                            } else {
                                palette.text
                            },
                        );
                        paint_grid_text(
                            ui,
                            Rect::from_min_size(
                                pos2(text_left, cover_rect.bottom() + 23.0),
                                vec2(text_width, 17.0),
                            ),
                            &entry.grid_subtitle,
                            theme::regular(12.0),
                            palette.secondary,
                        );

                        if pinned {
                            let pin_rect = grid_pin_rect(cover_rect);
                            ui.painter().circle_filled(
                                pin_rect.center(),
                                pin_rect.width() / 2.0,
                                egui::Color32::from_black_alpha(170),
                            );
                            Icon::Pin.image(egui::Color32::WHITE, 12.0).paint_at(
                                ui,
                                Rect::from_center_size(pin_rect.center(), Vec2::splat(12.0)),
                            );
                        }
                        if dragging_song && !droppable {
                            ui.painter().rect_filled(
                                rect,
                                CornerRadius::same(6),
                                palette.panel.gamma_multiply(0.5),
                            );
                        } else if dragging_song
                            && droppable
                            && pointer.is_some_and(|pos| rect.contains(pos))
                        {
                            ui.painter().rect_stroke(
                                cover_rect,
                                CornerRadius::same(6),
                                egui::Stroke::new(2.0, palette.accent),
                                egui::StrokeKind::Inside,
                            );
                        }
                        if reorder_slot == Some(index) {
                            ui.painter().vline(
                                rect.left(),
                                rect.y_range(),
                                egui::Stroke::new(2.0, palette.accent),
                            );
                        }
                        if index + 1 == entries.len() && reorder_slot == Some(entries.len()) {
                            ui.painter().vline(
                                rect.right(),
                                rect.y_range(),
                                egui::Stroke::new(2.0, palette.accent),
                            );
                        }
                    }

                    finish_entry_interaction(
                        app,
                        ui,
                        &response,
                        entry,
                        cover_took_click,
                        custom_order,
                        pointer.is_some(),
                    );
                });
            }
        });
    });

    if let Some(slot) = reorder_slot
        && ui.input(|input| input.pointer.button_released(egui::PointerButton::Primary))
        && let Some(drag) = egui::DragAndDrop::take_payload::<DragEntry>(ui.ctx())
    {
        if filter == Filter::Playlists {
            drop_playlist_row(app, entries, pinned_rows, slot, &drag.uri);
        } else {
            drop_row(app, entries, pinned_rows, slot, &drag.uri);
        }
    }
}

fn entry_menu(app: &mut App, response: &egui::Response, entry: &Entry, custom_order: bool) {
    if !entry.uri.is_empty() {
        let owned_playlist = entry
            .owned
            .then_some(entry.playlist_index)
            .flatten()
            .and_then(|index| {
                app.library
                    .playlists
                    .get()
                    .and_then(|list| list.get(index))
                    .cloned()
            });
        egui::Popup::context_menu(response)
            .frame(super::widgets::menu_frame(&app.palette))
            .show(|ui| {
                super::widgets::context_menu_items(
                    ui,
                    app,
                    &entry.uri,
                    &entry.name,
                    owned_playlist.as_ref(),
                );
                pin_menu(app, ui, entry.ordering_key());
                if custom_order
                    && super::widgets::menu_item(
                        ui,
                        &app.palette,
                        Some(Icon::Clock),
                        &gettext(app.locale, "Sort by recently played"),
                    )
                {
                    app.actions.push(Action::SetLibrarySort {
                        shelf: Filter::Playlists,
                        sort: LibrarySort::RecentlyPlayed,
                    });
                }
            });
    } else if entry.liked {
        egui::Popup::context_menu(response)
            .frame(super::widgets::menu_frame(&app.palette))
            .show(|ui| {
                // The same width as every other menu; without it the menu
                // stretches as wide as the window.
                ui.set_min_width(200.0);
                ui.set_max_width(300.0);
                if super::widgets::menu_item(
                    ui,
                    &app.palette,
                    Some(Icon::Play),
                    &gettext(app.locale, "Play"),
                ) && let Some(user) = &app.user
                {
                    app.actions.push(Action::PlayContext {
                        uri: format!("spotify:user:{}:collection", user.id),
                        offset_uri: None,
                        offset_index: None,
                    });
                }
                pin_menu(app, ui, entry.ordering_key());
            });
    }
}

fn pin_menu(app: &mut App, ui: &mut egui::Ui, key: &str) {
    let mut pins = app.settings.library_pins();
    let pinned = pins.iter().any(|held| held == key);
    let label = if pinned {
        gettext(app.locale, "Unpin")
    } else {
        gettext(app.locale, "Pin to top")
    };
    if super::widgets::menu_item(
        ui,
        &app.palette,
        Some(if pinned { Icon::PinOff } else { Icon::Pin }),
        &label,
    ) {
        if pinned {
            pins.retain(|held| held != key);
        } else {
            pins.push(key.to_string());
        }
        app.actions.push(Action::ArrangeLibrary {
            pinned: pins,
            playlist_order: None,
        });
    }
}

/// Drops within the pin block arrange pins. Below it, a drop unpins the
/// moved row and saves the full playlist arrangement at the chosen gap.
fn drop_playlist_row(app: &mut App, entries: &[Entry], pinned_rows: usize, slot: usize, key: &str) {
    if key != LIKED_SONGS_KEY
        && !app
            .library
            .playlists
            .get()
            .is_some_and(|playlists| playlists.iter().any(|playlist| playlist.uri == key))
    {
        return;
    }
    let was_pinned = app.settings.library_pins().iter().any(|held| held == key);
    if slot < pinned_rows || (was_pinned && slot == pinned_rows) {
        drop_row(app, entries, pinned_rows, slot, key);
        return;
    }
    let mut pins = app.settings.library_pins();
    pins.retain(|held| held != key);
    let mut order = full_playlist_order(app);
    let anchor = entries
        .iter()
        .skip(slot)
        .find_map(|entry| {
            if let Some((id, _, _)) = &entry.folder {
                // A drop before a collapsed folder precedes its first child
                // when switching to the flat local arrangement.
                let start = app.rootlist.iter().position(|row| matches!(row, crate::player::RootlistEntry::FolderStart { id: found, .. } if found == id))?;
                app.rootlist[start + 1..].iter().find_map(|row| match row {
                    crate::player::RootlistEntry::Playlist(held) if held != key && order.contains(held) => Some(held.as_str()),
                    _ => None,
                })
            } else {
                let held = entry.ordering_key();
                (!held.is_empty() && held != key).then_some(held)
            }
        })
        .map(str::to_string);
    order.retain(|held| held != key);
    let at = anchor
        .and_then(|anchor| order.iter().position(|held| *held == anchor))
        .unwrap_or(order.len());
    order.insert(at, key.to_string());
    app.actions.push(Action::ArrangeLibrary {
        pinned: pins,
        playlist_order: Some(order),
    });
}

/// Every unpinned loaded playlist in the selected shelf order, including
/// rows hidden by a search or collapsed folder. A drag snapshots this
/// whole arrangement before applying its new local position.
fn full_playlist_order(app: &App) -> Vec<String> {
    let Some(playlists) = app.library.playlists.get() else {
        return Vec::new();
    };
    let mut entries: Vec<_> = playlists
        .iter()
        .enumerate()
        .map(|(index, playlist)| {
            playlist_entry(
                app.locale,
                playlist,
                index,
                app.user_id().unwrap_or(""),
                false,
                0,
                EntryArt::of(app),
            )
        })
        .collect();
    entries.push(liked_entry(app));
    order_entries(
        app,
        Filter::Playlists,
        selected_sort(app, Filter::Playlists),
        &mut entries,
    );
    let pins = app.settings.library_pins();
    entries
        .iter()
        .map(|entry| entry.ordering_key().to_string())
        .filter(|key| !pins.contains(key))
        .collect()
}

/// Reorders the pin block without disturbing pins on another shelf.
fn drop_row(app: &mut App, entries: &[Entry], pinned_rows: usize, slot: usize, key: &str) {
    let mut pins = app.settings.library_pins();
    pins.retain(|held| held != key);
    if slot <= pinned_rows {
        let anchor = entries[..pinned_rows]
            .iter()
            .skip(slot)
            .map(Entry::ordering_key)
            .find(|held| *held != key);
        let at = anchor
            .and_then(|anchor| pins.iter().position(|held| held == anchor))
            .unwrap_or(pins.len());
        pins.insert(at, key.to_string());
    }
    app.actions.push(Action::ArrangeLibrary {
        pinned: pins,
        playlist_order: None,
    });
}

/// The purple-to-blue Liked Songs tile.
pub fn liked_cover(ui: &egui::Ui, rect: Rect, radius: f32) {
    let texture_id = egui::Id::new("liked-cover-gradient");
    let texture = ui
        .data(|data| data.get_temp::<egui::TextureHandle>(texture_id))
        .unwrap_or_else(|| {
            let size = 64;
            let lerp = |a: u8, b: u8, t: f32| (a as f32 + (b as f32 - a as f32) * t) as u8;
            let top_left = [0x45, 0x0a, 0xf5];
            let top_right = [0x6a, 0x3a, 0xe8];
            let bottom_left = [0x8e, 0x9f, 0xe5];
            let bottom_right = [0xc4, 0xef, 0xd9];
            let pixels = (0..size)
                .flat_map(|y| {
                    let y = y as f32 / (size - 1) as f32;
                    (0..size).map(move |x| {
                        let x = x as f32 / (size - 1) as f32;
                        egui::Color32::from_rgb(
                            lerp(
                                lerp(top_left[0], top_right[0], x),
                                lerp(bottom_left[0], bottom_right[0], x),
                                y,
                            ),
                            lerp(
                                lerp(top_left[1], top_right[1], x),
                                lerp(bottom_left[1], bottom_right[1], x),
                                y,
                            ),
                            lerp(
                                lerp(top_left[2], top_right[2], x),
                                lerp(bottom_left[2], bottom_right[2], x),
                                y,
                            ),
                        )
                    })
                })
                .collect();
            let texture = ui.ctx().load_texture(
                "liked-cover-gradient",
                egui::ColorImage::new([size, size], pixels),
                egui::TextureOptions::LINEAR,
            );
            ui.data_mut(|data| data.insert_temp(texture_id, texture.clone()));
            texture
        });
    egui::Image::new(&texture)
        .corner_radius(CornerRadius::same(radius.min(127.0) as u8))
        .paint_at(ui, rect);
    let size = rect.width() * 0.45;
    let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(size));
    Icon::HeartFilled
        .image(egui::Color32::WHITE, size)
        .paint_at(ui, icon_rect);
}

#[cfg(all(test, feature = "demo"))]
mod ordering_tests {
    use super::*;
    use crate::api::models::Playlist;
    use crate::settings::Settings;

    const TEST_ART: EntryArt = EntryArt { row: 64, grid: 300 };

    /// Create an isolated demo application with deliberately nonalphabetical rows
    /// and known recent plays so ordering tests can distinguish each sort mode.
    fn app(name: &str) -> App {
        let root =
            std::env::temp_dir().join(format!("spotifast-order-{name}-{}", std::process::id()));
        let mut app = App::new(
            &crate::backend::Waker::default(),
            crate::paths::AppDirs {
                config: root.join("config"),
                state: root.join("state"),
                cache: root.join("cache"),
            },
            Settings::default(),
            crate::app::AppOptions {
                media_controls: false,
                restore_sign_in: false,
                tray: false,
                autostart: false,
            },
        );
        crate::demo::populate(&mut app);
        app.library.playlists = Loadable::Loaded(
            [
                ("a", "Zebra"),
                ("b", "Alpha"),
                ("c", "alpha"),
                ("d", "Beta"),
            ]
            .into_iter()
            .map(|(id, name)| Playlist {
                id: id.into(),
                name: name.into(),
                uri: format!("spotify:playlist:{id}"),
                ..Default::default()
            })
            .collect(),
        );
        app.recent_contexts = vec![uri("d"), uri("a")];
        app
    }

    /// Drain queued UI actions through the real handler before checking the
    /// resulting saved order, pin state, or filter preference.
    fn apply_actions(app: &mut App) {
        for action in std::mem::take(&mut app.actions) {
            app.apply(action, &egui::Context::default());
        }
    }

    /// Give a fixture playlist the same URI key used by rootlist order and pins.
    fn uri(id: &str) -> String {
        format!("spotify:playlist:{id}")
    }

    /// Project every fixture playlist as a flat row, retaining its library index
    /// and leaving ownership and edit permissions disabled for ordering-only tests.
    fn rows(app: &App) -> Vec<Entry> {
        app.library
            .playlists
            .get()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(index, playlist)| {
                playlist_entry(Locale::English, playlist, index, "", false, 0, TEST_ART)
            })
            .collect()
    }

    /// Extract compact playlist IDs in display order for assertions.
    /// Liked Songs has an empty URI, so its position is represented by an empty ID.
    fn ids(entries: &[Entry]) -> Vec<&str> {
        entries
            .iter()
            .map(|entry| entry.uri.rsplit(':').next().unwrap())
            .collect()
    }

    /// Build a mixed library with two owned playlists, a foreign collaborative
    /// playlist, and an unknown owner, retaining their original library indices.
    fn ownership_app(name: &str) -> App {
        let mut app = app(name);
        let id = app.user_id().unwrap().to_string();
        let playlists = app.library.playlists.get_mut().unwrap();
        playlists[0].owner.id = Some(id.clone());
        playlists[1].owner.id = Some("someone-else".into());
        playlists[1].collaborative = true;
        playlists[2].owner.id = Some(id);
        // The last playlist has no known owner.
        app
    }

    /// Run the production playlist projection and ordering together so assertions
    /// observe the same filtered, sorted, and pinned rows as the sidebar.
    fn projected(app: &App, needle: &str) -> Vec<Entry> {
        let mut entries = Vec::new();
        playlist_rows(
            app,
            selected_sort(app, Filter::Playlists),
            needle,
            &mut entries,
        );
        order_entries(
            app,
            Filter::Playlists,
            selected_sort(app, Filter::Playlists),
            &mut entries,
        );
        entries
    }

    /// Verify that ownership filtering composes with search and pins while
    /// retaining source indices, sort selection, and the saved local arrangement.
    #[test]
    fn by_you_uses_account_ownership_and_composes_with_search_sort_and_pins() {
        let mut app = ownership_app("by-you");
        app.settings.sidebar_order = [uri("b"), uri("c"), uri("a"), uri("d")].to_vec();
        app.settings.pinned_contexts = [uri("b"), uri("a")].to_vec();
        app.apply(Action::SetLibraryByYou(true), &egui::Context::default());
        let entries = projected(&app, "");
        assert!(entries.iter().any(|entry| entry.liked));
        assert_eq!(
            ids(&entries)
                .into_iter()
                .filter(|id| !id.is_empty())
                .collect::<Vec<_>>(),
            ["a", "c"]
        );
        assert_eq!(
            entries
                .iter()
                .find(|entry| entry.uri == uri("c"))
                .unwrap()
                .playlist_index,
            Some(2)
        );
        assert_eq!(ids(&projected(&app, "alpha")), ["c"]);
        assert!(projected(&app, "no match").is_empty());
        app.settings
            .library_sort
            .insert(Filter::Playlists, LibrarySort::Name);
        assert_eq!(ids(&projected(&app, "alpha")), ["c"]);
        app.settings.pinned_contexts.clear();
        let named = projected(&app, "");
        assert_eq!(
            named
                .iter()
                .filter(|entry| !entry.liked)
                .map(|entry| entry.uri.as_str())
                .collect::<Vec<_>>(),
            [uri("c"), uri("a")]
        );
        app.apply(Action::SetLibraryByYou(false), &egui::Context::default());
        assert_eq!(projected(&app, "").len(), 5);
        assert_eq!(
            app.settings.sidebar_order,
            [uri("b"), uri("c"), uri("a"), uri("d")]
        );
        assert_eq!(selected_sort(&app, Filter::Playlists), LibrarySort::Name);
        app.backend.shutdown();
    }

    /// Ensure changing accounts reevaluates ownership and that missing or empty
    /// account identities never match playlists whose owners are unknown.
    #[test]
    fn by_you_rechecks_ownership_after_account_changes_and_keeps_missing_owners_out() {
        let mut app = ownership_app("by-you-account");
        app.settings.library_by_you = true;
        app.user.as_mut().unwrap().id = "someone-else".into();
        assert_eq!(ids(&projected(&app, "alpha")), ["b"]);
        app.user = None;
        assert!(projected(&app, "").iter().all(|entry| entry.liked));
        app.library.playlists.get_mut().unwrap()[3].owner.id = Some(String::new());
        assert!(projected(&app, "beta").is_empty());
        app.backend.shutdown();
    }

    /// Ensure the owned view keeps collapsed folders collapsed, hides folders
    /// left empty, and disabling it restores the tree without changing
    /// folder expansion state.
    #[test]
    fn by_you_keeps_collapsed_folders_and_hides_emptied_ones() {
        use crate::player::RootlistEntry;
        let mut app = ownership_app("by-you-folders");
        app.rootlist = vec![
            RootlistEntry::FolderStart {
                id: "folder".into(),
                name: "Folder".into(),
            },
            RootlistEntry::Playlist(uri("c")),
            RootlistEntry::FolderEnd,
            RootlistEntry::FolderStart {
                id: "empty".into(),
                name: "Empty".into(),
            },
            RootlistEntry::Playlist(uri("b")),
            RootlistEntry::FolderEnd,
        ];
        app.collapsed_folders = vec!["folder".into()];
        app.settings
            .library_sort
            .insert(Filter::Playlists, LibrarySort::Spotify);
        assert!(
            projected(&app, "")
                .iter()
                .any(|entry| entry.folder.is_some())
        );
        assert!(
            !projected(&app, "")
                .iter()
                .any(|entry| entry.uri == uri("c"))
        );
        app.settings.library_by_you = true;
        let entries = projected(&app, "");
        let folder = entries
            .iter()
            .find(|entry| {
                entry
                    .folder
                    .as_ref()
                    .is_some_and(|folder| folder.0 == "folder")
            })
            .expect("owned folder stays");
        assert_eq!(folder.folder.as_ref().unwrap().2, 1);
        assert_eq!(folder.depth, 0);
        assert!(!entries.iter().any(|entry| {
            entry
                .folder
                .as_ref()
                .is_some_and(|folder| folder.0 == "empty")
        }));
        // The collapsed folder keeps hiding its owned playlist; the other
        // owned playlist waits at the top level.
        assert!(
            !entries.iter().any(|entry| entry.uri == uri("c")),
            "collapsed folders stay collapsed"
        );
        let top = entries
            .iter()
            .find(|entry| entry.uri == uri("a"))
            .expect("owned top-level playlist stays");
        assert_eq!(top.depth, 0);
        assert!(entries.iter().any(|entry| entry.liked));
        // Expanding reveals the owned playlist inside its folder.
        app.collapsed_folders.clear();
        let expanded = projected(&app, "");
        let inside = expanded
            .iter()
            .find(|entry| entry.uri == uri("c"))
            .expect("owned playlist in its folder");
        assert_eq!(inside.depth, 1);
        app.collapsed_folders = vec!["folder".into()];
        app.settings.library_by_you = false;
        let restored = projected(&app, "");
        assert!(restored.iter().any(|entry| {
            entry
                .folder
                .as_ref()
                .is_some_and(|folder| folder.0 == "empty")
        }));
        assert_eq!(app.collapsed_folders, ["folder"]);
        app.backend.shutdown();
    }

    /// Verify the owned Spotify order keeps folders, uses the first duplicate
    /// rootlist entry, keeps unlisted playlists visible, and applies pins
    /// without rewriting data.
    #[test]
    fn by_you_spotify_order_keeps_folders_first_occurrences_missing_rows_and_pins() {
        use crate::player::RootlistEntry;
        let mut app = ownership_app("by-you-rootlist-index");
        let owner = app.user_id().unwrap().to_string();
        app.library.playlists.get_mut().unwrap()[3].owner.id = Some(owner);
        app.settings.library_by_you = true;
        app.settings.liked_songs_pinned = false;
        app.settings
            .library_sort
            .insert(Filter::Playlists, LibrarySort::Spotify);
        app.rootlist = vec![
            RootlistEntry::FolderStart {
                id: "folder".into(),
                name: "Folder".into(),
            },
            RootlistEntry::Playlist(uri("c")),
            RootlistEntry::Playlist(uri("b")),
            RootlistEntry::FolderEnd,
            RootlistEntry::Playlist(uri("a")),
            RootlistEntry::Playlist(uri("c")),
        ];
        let rootlist = app.rootlist.clone();
        let entries = projected(&app, "");
        let folder = entries
            .iter()
            .find(|entry| entry.folder.is_some())
            .expect("folder stays with the filter on");
        assert_eq!(folder.folder.as_ref().unwrap().2, 1);
        assert_eq!(folder.depth, 0);
        assert_eq!(
            entries
                .iter()
                .find(|entry| entry.uri == uri("c"))
                .expect("owned playlist stays in its folder")
                .depth,
            1
        );
        assert!(!entries.iter().any(|entry| entry.uri == uri("b")));
        assert_eq!(ids(&entries), ["", "c", "a", "d", ""]);
        app.settings.pinned_contexts = vec![uri("a")];
        assert_eq!(ids(&projected(&app, "")), ["a", "", "c", "d", ""]);
        assert_eq!(app.rootlist, rootlist);
        assert_eq!(
            app.library
                .playlists
                .get()
                .unwrap()
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b", "c", "d"]
        );
        app.backend.shutdown();
    }

    /// Check that pre-filter settings load with By You disabled and an enabled
    /// preference survives serialization without requiring a settings migration.
    #[test]
    fn by_you_defaults_off_in_older_settings_and_round_trips() {
        let mut settings: Settings = serde_json::from_str("{}").unwrap();
        assert!(!settings.library_by_you);
        settings.library_by_you = true;
        let restored: Settings =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert!(restored.library_by_you);
    }

    #[test]
    fn grid_cards_use_smaller_art_than_page_headers() {
        let images = [64, 300, 640]
            .map(|width| crate::api::models::Image {
                url: width.to_string(),
                width: Some(width),
                height: Some(width),
            })
            .to_vec();
        let playlist = Playlist {
            images: images.clone(),
            ..Default::default()
        };
        let entry = playlist_entry(Locale::English, &playlist, 0, "", false, 0, TEST_ART);
        assert_eq!(entry.image.as_deref(), Some("64"));
        assert_eq!(entry.grid_image.as_deref(), Some("300"));
        assert_eq!(pick_image(&images, 640), Some("640"));
    }

    #[test]
    fn library_grid_adds_columns_as_the_sidebar_grows() {
        let narrow = grid_layout(230.0);
        let medium = grid_layout(360.0);
        let wide = grid_layout(500.0);
        assert_eq!(narrow.columns, 2);
        assert_eq!(medium.columns, 3);
        assert_eq!(wide.columns, 4);
        for (width, layout) in [(230.0, narrow), (360.0, medium), (500.0, wide)] {
            let filled = layout.card_width * layout.columns as f32;
            assert!((filled - width).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn grid_pin_and_play_controls_keep_opposite_corners() {
        let cover = Rect::from_min_size(pos2(10.0, 20.0), Vec2::splat(GRID_MIN_CARD_WIDTH));
        let pin = grid_pin_rect(cover);
        let play = grid_play_rect(cover);
        assert!(pin.center().x < cover.center().x && pin.center().y < cover.center().y);
        assert!(play.center().x > cover.center().x && play.center().y > cover.center().y);
        assert!(!pin.intersects(play));
    }

    #[test]
    fn grid_drop_positions_follow_visual_row_order() {
        let layout = grid_layout(360.0);
        let origin = pos2(20.0, 40.0);
        assert_eq!(grid_reorder_slot(pos2(21.0, 41.0), origin, layout, 8), 0);
        assert_eq!(
            grid_reorder_slot(
                pos2(origin.x + layout.card_width, origin.y + 1.0),
                origin,
                layout,
                8,
            ),
            1
        );
        assert_eq!(
            grid_reorder_slot(
                pos2(origin.x + 1.0, origin.y + layout.row_height + 1.0),
                origin,
                layout,
                8,
            ),
            3
        );
    }

    #[test]
    fn name_and_recent_sorts_keep_pins_and_do_not_rewrite_library_data() {
        let mut app = app("sorts");
        let mut entries = rows(&app);
        order_entries(
            &app,
            Filter::Playlists,
            LibrarySort::RecentlyPlayed,
            &mut entries,
        );
        assert_eq!(ids(&entries), ["d", "a", "b", "c"]);
        order_entries(&app, Filter::Playlists, LibrarySort::Name, &mut entries);
        assert_eq!(ids(&entries), ["b", "c", "d", "a"]);
        app.settings.pinned_contexts = vec![uri("a")];
        order_entries(&app, Filter::Playlists, LibrarySort::Name, &mut entries);
        assert_eq!(ids(&entries), ["a", "b", "c", "d"]);
        assert_eq!(ids(&rows(&app)), ["a", "b", "c", "d"]);
        app.backend.shutdown();
    }

    /// Duplicate saved keys keep their first rank, while rows absent from
    /// the saved order keep their relative positions without changing source data.
    #[test]
    fn ranked_sorts_keep_first_duplicate_and_stable_order_for_missing_entries() {
        use crate::player::RootlistEntry;

        let mut app = app("ranked-sorts");
        app.settings.liked_songs_pinned = false;
        app.settings.pinned_contexts.clear();
        app.recent_contexts = ["c", "a", "c"].map(uri).to_vec();
        app.settings.sidebar_order = ["c", "a", "c"].map(uri).to_vec();
        app.rootlist = ["c", "a", "c"]
            .map(|id| RootlistEntry::Playlist(uri(id)))
            .to_vec();

        for (sort, expected) in [
            (LibrarySort::RecentlyPlayed, ["c", "a", "b", "d"]),
            (LibrarySort::Local, ["b", "d", "c", "a"]),
            (LibrarySort::Spotify, ["c", "a", "b", "d"]),
        ] {
            let mut entries = rows(&app);
            order_entries(&app, Filter::Playlists, sort, &mut entries);
            assert_eq!(ids(&entries), expected, "{sort:?}");
        }

        app.settings.pinned_contexts = ["c", "a", "c"].map(uri).to_vec();
        let mut entries = rows(&app);
        order_entries(&app, Filter::Playlists, LibrarySort::Library, &mut entries);
        assert_eq!(ids(&entries), ["c", "a", "b", "d"]);
        assert_eq!(ids(&rows(&app)), ["a", "b", "c", "d"]);
        app.backend.shutdown();
    }

    #[test]
    fn switching_sort_and_filtered_drag_preserve_the_full_local_arrangement() {
        let mut app = app("saved");
        app.settings.sidebar_order = ["c", "a", "b", "d"].map(uri).to_vec();
        let saved = app.settings.sidebar_order.clone();
        assert_eq!(selected_sort(&app, Filter::Playlists), LibrarySort::Local);
        app.settings
            .library_sort
            .insert(Filter::Playlists, LibrarySort::Name);
        assert_eq!(full_playlist_order(&app), ["b", "c", "d", "a"].map(uri));
        assert_eq!(app.settings.sidebar_order, saved);
        app.rootlist = vec![crate::player::RootlistEntry::Playlist(uri("d"))];
        app.settings
            .library_sort
            .insert(Filter::Playlists, LibrarySort::Local);
        assert_eq!(
            full_playlist_order(&app),
            saved,
            "a late rootlist cannot replace the chosen local order"
        );
        app.settings
            .library_sort
            .insert(Filter::Playlists, LibrarySort::Name);
        let filtered: Vec<_> = rows(&app)
            .into_iter()
            .filter(|row| row.uri == uri("c") || row.uri == uri("a"))
            .rev()
            .collect();
        drop_playlist_row(&mut app, &filtered, 0, 0, &uri("a"));
        apply_actions(&mut app);
        assert_eq!(app.settings.sidebar_order, ["b", "a", "c", "d"].map(uri));
        assert_eq!(selected_sort(&app, Filter::Playlists), LibrarySort::Local);
        app.settings
            .library_sort
            .insert(Filter::Playlists, LibrarySort::Name);
        drop_playlist_row(&mut app, &filtered, 0, 0, &uri("a"));
        apply_actions(&mut app);
        assert_eq!(app.settings.sidebar_order, ["b", "a", "c", "d"].map(uri));
        assert_eq!(
            selected_sort(&app, Filter::Playlists),
            LibrarySort::Local,
            "recreating a saved arrangement still leaves the automatic sort"
        );
        app.backend.shutdown();
    }

    #[test]
    fn spotify_order_keeps_nested_folders_and_pins_visible_when_collapsed() {
        use crate::player::RootlistEntry::{FolderEnd, FolderStart, Playlist};
        let mut app = app("folders");
        app.rootlist = vec![
            FolderStart {
                id: "outer".into(),
                name: "Outer".into(),
            },
            Playlist(uri("a")),
            FolderStart {
                id: "inner".into(),
                name: "Inner".into(),
            },
            Playlist(uri("b")),
            Playlist(uri("c")),
            FolderEnd,
            FolderEnd,
            Playlist(uri("d")),
            Playlist(uri("b")),
        ];
        app.collapsed_folders = vec!["inner".into()];
        app.settings.pinned_contexts = vec![uri("b")];
        let mut entries = vec![];
        folder_rows(&app, "", &mut entries);
        order_entries(&app, Filter::Playlists, LibrarySort::Spotify, &mut entries);
        assert_eq!(
            entries
                .iter()
                .map(|row| (row.name.as_str(), row.depth))
                .collect::<Vec<_>>(),
            [
                ("Alpha", 0),
                ("Outer", 0),
                ("Zebra", 1),
                ("Inner", 1),
                ("Beta", 0)
            ]
        );
        app.collapsed_folders.clear();
        let mut entries = vec![];
        folder_rows(&app, "", &mut entries);
        order_entries(&app, Filter::Playlists, LibrarySort::Spotify, &mut entries);
        assert_eq!(
            entries
                .iter()
                .map(|row| (row.name.as_str(), row.depth))
                .collect::<Vec<_>>(),
            [
                ("Alpha", 0),
                ("Outer", 0),
                ("Zebra", 1),
                ("Inner", 1),
                ("alpha", 2),
                ("Beta", 0)
            ]
        );
        app.backend.shutdown();
    }

    /// Name keeps playlists in their folders, as Spotify does: each folder's
    /// rows follow it in name order, folders sort among their siblings by
    /// name, and a pin still comes out of its folder to the top (#651).
    #[test]
    fn name_order_sorts_within_each_folder() {
        use crate::player::RootlistEntry::{FolderEnd, FolderStart, Playlist};
        let mut app = app("name-folders");
        app.settings.liked_songs_pinned = false;
        app.settings.pinned_contexts.clear();
        app.settings
            .library_sort
            .insert(Filter::Playlists, LibrarySort::Name);
        app.rootlist = vec![
            FolderStart {
                id: "outer".into(),
                name: "Outer".into(),
            },
            Playlist(uri("a")),
            FolderStart {
                id: "inner".into(),
                name: "Inner".into(),
            },
            Playlist(uri("c")),
            Playlist(uri("b")),
            FolderEnd,
            FolderEnd,
            Playlist(uri("d")),
        ];
        let rows = |app: &App| {
            projected(app, "")
                .iter()
                .map(|row| (row.name.clone(), row.depth))
                .collect::<Vec<_>>()
        };
        let expected = |rows: &[(&str, u8)]| {
            rows.iter()
                .map(|(name, depth)| (name.to_string(), *depth))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            rows(&app),
            expected(&[
                ("Beta", 0),
                ("Liked Songs", 0),
                ("Outer", 0),
                ("Inner", 1),
                ("Alpha", 2),
                ("alpha", 2),
                ("Zebra", 1),
            ])
        );
        app.settings.pinned_contexts = vec![uri("b")];
        assert_eq!(
            rows(&app),
            expected(&[
                ("Alpha", 0),
                ("Beta", 0),
                ("Liked Songs", 0),
                ("Outer", 0),
                ("Inner", 1),
                ("alpha", 2),
                ("Zebra", 1),
            ])
        );
        // A search still lists matching playlists flat.
        assert!(projected(&app, "a").iter().all(|row| row.depth == 0));
        app.backend.shutdown();
    }

    #[test]
    fn permute_places_each_entry_at_its_rank() {
        let mut items = ['a', 'b', 'c', 'd', 'e'];
        permute(&mut items, &[3, 0, 4, 1, 2]);
        assert_eq!(items, ['d', 'a', 'e', 'b', 'c']);
    }

    #[test]
    fn dragging_before_a_collapsed_folder_and_unpinning_keep_the_drop_position() {
        use crate::player::RootlistEntry::{FolderEnd, FolderStart, Playlist};
        let mut app = app("folder-drop");
        app.rootlist = vec![
            FolderStart {
                id: "folder".into(),
                name: "Folder".into(),
            },
            Playlist(uri("b")),
            Playlist(uri("c")),
            FolderEnd,
            Playlist(uri("d")),
            Playlist(uri("a")),
        ];
        app.collapsed_folders = vec!["folder".into()];
        let mut entries = vec![];
        folder_rows(&app, "", &mut entries);
        order_entries(&app, Filter::Playlists, LibrarySort::Spotify, &mut entries);
        drop_playlist_row(&mut app, &entries, 0, 0, &uri("a"));
        apply_actions(&mut app);
        assert_eq!(app.settings.sidebar_order, ["a", "b", "c", "d"].map(uri));
        app.settings.sidebar_order.clear();
        app.settings
            .library_sort
            .insert(Filter::Playlists, LibrarySort::Name);
        app.settings.pinned_contexts = vec![uri("a")];
        let mut entries = rows(&app);
        order_entries(&app, Filter::Playlists, LibrarySort::Name, &mut entries);
        drop_playlist_row(&mut app, &entries, 1, 2, &uri("a"));
        apply_actions(&mut app);
        assert_eq!(app.settings.library_pins(), [LIKED_SONGS_KEY]);
        assert_eq!(app.settings.sidebar_order, ["b", "a", "c", "d"].map(uri));
        app.backend.shutdown();
    }

    #[test]
    fn folder_names_and_subtitles_follow_the_locale() {
        use crate::i18n::Locale;
        use crate::player::RootlistEntry::{FolderEnd, FolderStart, Playlist};
        let mut app = app("folder-locale");
        app.locale = Locale::German;
        app.rootlist = vec![
            FolderStart {
                id: "folder".into(),
                name: String::new(),
            },
            Playlist(uri("a")),
            Playlist(uri("b")),
            FolderEnd,
        ];
        let mut entries = vec![];
        folder_rows(&app, "", &mut entries);
        assert_eq!(entries[0].name, "Ordner");
        assert_eq!(entries[0].subtitle, "Ordner • 2 Playlists");
        assert_eq!(entries[0].grid_subtitle, "2 Playlists");
        app.backend.shutdown();
    }

    #[test]
    fn added_sort_uses_actual_instants_and_keeps_missing_dates_last() {
        let mut app = app("dates");
        let mut entries = rows(&app);
        for (row, value) in entries.iter_mut().zip([
            Some("2026-09-09T09:00:00Z"),
            Some("2026-09-09T10:00:00+02:00"),
            None,
            Some("unknown"),
        ]) {
            row.added_at = saved_time(value);
        }
        entries.swap(0, 1);
        order_entries(
            &app,
            Filter::Albums,
            LibrarySort::RecentlyAdded,
            &mut entries,
        );
        assert_eq!(ids(&entries), ["a", "b", "c", "d"]);
        assert!(!LibrarySort::RecentlyAdded.supports(Filter::Playlists));
        assert!(!LibrarySort::RecentlyAdded.supports(Filter::Artists));
        assert!(LibrarySort::RecentlyAdded.supports(Filter::Podcasts));
        app.backend.shutdown();
    }

    #[test]
    fn preferences_round_trip_and_new_playlists_still_precede_saved_order() {
        let mut app = app("migration");
        app.settings = serde_json::from_str(
            r#"{"sidebar_order":["spotify:playlist:c","spotify:playlist:a","spotify:playlist:b"]}"#,
        )
        .unwrap();
        assert_eq!(selected_sort(&app, Filter::Playlists), LibrarySort::Local);
        assert_eq!(selected_sort(&app, Filter::Albums), LibrarySort::Library);
        assert_eq!(full_playlist_order(&app), ["d", "c", "a", "b"].map(uri));
        app.settings
            .library_sort
            .insert(Filter::Playlists, LibrarySort::Spotify);
        app.settings
            .library_sort
            .insert(Filter::Albums, LibrarySort::RecentlyAdded);
        let restored: Settings =
            serde_json::from_str(&serde_json::to_string(&app.settings).unwrap()).unwrap();
        assert_eq!(restored, app.settings);
        app.backend.shutdown();
    }

    #[test]
    fn dropping_at_the_end_of_the_pin_block_keeps_the_item_pinned() {
        let mut app = app("pin-boundary");
        app.settings.pinned_contexts = [uri("d"), uri("a")].to_vec();
        let mut entries = rows(&app);
        entries.push(liked_entry(&app));
        order_entries(
            &app,
            Filter::Playlists,
            selected_sort(&app, Filter::Playlists),
            &mut entries,
        );

        drop_playlist_row(&mut app, &entries, 3, 3, LIKED_SONGS_KEY);
        apply_actions(&mut app);

        assert_eq!(
            app.settings.library_pins(),
            [uri("d"), uri("a"), LIKED_SONGS_KEY.into()]
        );
        app.backend.shutdown();
    }

    #[test]
    fn liked_songs_moves_among_pins_and_retains_its_local_position() {
        let mut app = app("liked-position");
        app.settings.pinned_contexts = [uri("d"), uri("a")].to_vec();
        let ordered = |app: &App| {
            let mut entries = rows(app);
            entries.push(liked_entry(app));
            order_entries(
                app,
                Filter::Playlists,
                selected_sort(app, Filter::Playlists),
                &mut entries,
            );
            entries
        };
        let entries = ordered(&app);
        assert_eq!(entries[0].ordering_key(), LIKED_SONGS_KEY);
        drop_playlist_row(&mut app, &entries, 3, 2, LIKED_SONGS_KEY);
        assert_eq!(
            app.settings.library_pins()[0],
            LIKED_SONGS_KEY,
            "the view only emits an action"
        );
        apply_actions(&mut app);
        assert_eq!(
            app.settings.library_pins(),
            [uri("d"), LIKED_SONGS_KEY.into(), uri("a")]
        );
        assert!(app.settings.sidebar_order.is_empty());
        let entries = ordered(&app);
        drop_playlist_row(&mut app, &entries, 3, 4, LIKED_SONGS_KEY);
        apply_actions(&mut app);
        assert!(!app.settings.liked_songs_pinned);
        let saved = [uri("b"), LIKED_SONGS_KEY.into(), uri("c")];
        assert_eq!(full_playlist_order(&app), saved);
        app.settings
            .library_sort
            .insert(Filter::Playlists, LibrarySort::Name);
        assert_eq!(
            full_playlist_order(&app),
            [uri("b"), uri("c"), LIKED_SONGS_KEY.into()]
        );
        app.settings
            .library_sort
            .insert(Filter::Playlists, LibrarySort::Local);
        app.rootlist = vec![crate::player::RootlistEntry::Playlist(uri("c"))];
        app.user.as_mut().unwrap().id = "another-account".into();
        app.settings =
            serde_json::from_str(&serde_json::to_string(&app.settings).unwrap()).unwrap();
        assert_eq!(
            full_playlist_order(&app),
            saved,
            "restart and account/rootlist changes preserve the local placement"
        );
        let filtered: Vec<_> = ordered(&app)
            .into_iter()
            .filter(|entry| entry.uri == uri("c") || entry.uri == uri("b"))
            .collect();
        drop_playlist_row(&mut app, &filtered, 0, 0, &uri("c"));
        apply_actions(&mut app);
        assert_eq!(
            full_playlist_order(&app),
            [uri("c"), uri("b"), LIKED_SONGS_KEY.into()],
            "a hidden Liked Songs keeps its place in the full arrangement"
        );
        assert!(
            liked_entry(&app).uri.is_empty(),
            "the ordering key is never a Spotify URI"
        );
        app.backend.shutdown();
    }

    #[test]
    fn unpinned_liked_songs_follows_recent_play_and_spotify_order_without_entering_folders() {
        use crate::player::RootlistEntry::{FolderEnd, FolderStart, Playlist};
        let mut app = app("liked-sorts");
        app.settings.liked_songs_pinned = false;
        app.recent_contexts = vec![
            uri("a"),
            format!("spotify:user:{}:collection", app.user_id().unwrap()),
            uri("c"),
        ];
        app.settings
            .library_sort
            .insert(Filter::Playlists, LibrarySort::RecentlyPlayed);
        assert_eq!(
            full_playlist_order(&app),
            [
                uri("a"),
                LIKED_SONGS_KEY.into(),
                uri("c"),
                uri("b"),
                uri("d")
            ]
        );
        app.rootlist = vec![
            FolderStart {
                id: "folder".into(),
                name: "Folder".into(),
            },
            Playlist(uri("c")),
            FolderEnd,
            Playlist(uri("a")),
        ];
        app.collapsed_folders = vec!["folder".into()];
        app.settings
            .library_sort
            .insert(Filter::Playlists, LibrarySort::Spotify);
        let mut entries = vec![liked_entry(&app)];
        folder_rows(&app, "", &mut entries);
        order_entries(&app, Filter::Playlists, LibrarySort::Spotify, &mut entries);
        assert!(entries.last().unwrap().liked);
        assert_eq!(entries.last().unwrap().depth, 0);
        drop_playlist_row(&mut app, &entries, 0, 0, LIKED_SONGS_KEY);
        apply_actions(&mut app);
        assert_eq!(
            full_playlist_order(&app),
            [
                LIKED_SONGS_KEY.into(),
                uri("c"),
                uri("a"),
                uri("b"),
                uri("d")
            ]
        );
        let mut entries = rows(&app);
        entries.push(liked_entry(&app));
        let end = entries.len();
        drop_playlist_row(&mut app, &entries, 0, end, LIKED_SONGS_KEY);
        apply_actions(&mut app);
        assert_eq!(full_playlist_order(&app).last().unwrap(), LIKED_SONGS_KEY);
        app.backend.shutdown();
    }

    /// Draw whole frames of the app with the screen-reader tree on, so a
    /// test can find the sidebar's controls by name and press them.
    fn ui_frame(
        ctx: &egui::Context,
        app: &mut App,
        events: Vec<egui::Event>,
    ) -> egui::accesskit::TreeUpdate {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1280.0, 800.0))),
                events,
                ..Default::default()
            },
            |ui| app.frame_ui(ui),
        );
        output.textures_delta.clear();
        output
            .platform_output
            .accesskit_update
            .expect("screen-reader tree")
    }

    fn ui_app(name: &str) -> (egui::Context, App) {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut app = app(name);
        app.settings.art_expanded = false;
        app.reveal_theme_changes = false;
        app.attach(&ctx);
        ui_frame(&ctx, &mut app, vec![]);
        (ctx, app)
    }

    fn button(tree: &egui::accesskit::TreeUpdate, label: &str) -> Option<egui::accesskit::NodeId> {
        tree.nodes
            .iter()
            .find(|(_, node)| {
                node.label() == Some(label) && node.role() == egui::accesskit::Role::Button
            })
            .map(|(id, _)| *id)
    }

    fn press(ctx: &egui::Context, app: &mut App, label: &str) -> egui::accesskit::TreeUpdate {
        let tree = ui_frame(ctx, app, vec![]);
        let target = button(&tree, label).unwrap_or_else(|| panic!("no {label:?} button"));
        ui_frame(
            ctx,
            app,
            vec![egui::Event::AccessKitActionRequest(
                egui::accesskit::ActionRequest {
                    target_tree: egui::accesskit::TreeId::ROOT,
                    target_node: target,
                    action: egui::accesskit::Action::Click,
                    data: None,
                },
            )],
        );
        ui_frame(ctx, app, vec![])
    }

    #[test]
    fn your_library_collapses_to_covers_and_expands_back() {
        let (ctx, mut app) = ui_app("collapse");
        app.settings.sidebar_width = 320.0;
        let tree = press(&ctx, &mut app, "Collapse Your Library");
        assert_eq!(app.settings.sidebar_width, COLLAPSED_WIDTH);
        // The collapsed column keeps its rows, Create and Home, but no
        // shelf controls.
        assert!(button(&tree, "Expand Your Library").is_some());
        assert!(button(&tree, "Create a playlist").is_some());
        assert!(button(&tree, "Home").is_some());
        assert!(button(&tree, "Zebra").is_some());
        assert!(button(&tree, "Search Your Library").is_none());
        assert!(button(&tree, "Playlists").is_none());
        let sidebar = egui::containers::panel::PanelState::load(&ctx, egui::Id::new("sidebar"))
            .expect("the sidebar was drawn");
        assert_eq!(sidebar.outer_rect.width(), COLLAPSED_WIDTH);

        let tree = press(&ctx, &mut app, "Expand Your Library");
        assert_eq!(app.settings.sidebar_width, 320.0);
        assert!(button(&tree, "Search Your Library").is_some());
        app.backend.shutdown();
    }

    #[test]
    fn show_more_widens_your_library_and_show_less_restores_it() {
        let (ctx, mut app) = ui_app("show-more");
        app.settings.sidebar_width = 300.0;
        press(&ctx, &mut app, "Show more");
        assert_eq!(app.settings.sidebar_width, WIDE_WIDTH);
        press(&ctx, &mut app, "Show less");
        assert_eq!(app.settings.sidebar_width, 300.0);
        app.backend.shutdown();
    }

    #[test]
    fn a_released_drag_settles_collapsed_or_at_least_the_narrowest_sidebar() {
        assert_eq!(settled_width(COLLAPSED_WIDTH + 20.0), COLLAPSED_WIDTH);
        assert_eq!(
            settled_width(COLLAPSE_BELOW + 1.0),
            super::super::SIDEBAR_MIN_WIDTH
        );
        assert_eq!(settled_width(400.0), 400.0);
        assert!(!is_collapsed_width(Settings::default().sidebar_width));
    }

    #[test]
    fn by_spotify_lists_only_spotify_playlists_and_clear_filters_resets_the_shelf() {
        let mut app = ownership_app("by-spotify");
        app.library.playlists.get_mut().unwrap()[3].owner.id = Some(SPOTIFY_OWNER.into());
        let mut entries = Vec::new();
        playlist_rows_of(
            &app,
            LibrarySort::Name,
            "",
            Ownership::Spotify,
            &mut entries,
        );
        // Liked Songs belongs to the account, not to Spotify.
        assert_eq!(ids(&entries), ["d"]);
        assert_eq!(Ownership::of(&app, true), Ownership::Spotify);
        app.settings.library_by_you = true;
        assert_eq!(Ownership::of(&app, true), Ownership::You);

        let (ctx, mut app) = ui_app("clear-filters");
        app.library.playlists.get_mut().unwrap()[3].owner.id = Some(SPOTIFY_OWNER.into());
        let tree = ui_frame(&ctx, &mut app, vec![]);
        assert!(button(&tree, "Clear filters").is_none());
        let tree = press(&ctx, &mut app, "By Spotify");
        assert!(button(&tree, "Beta").is_some());
        assert!(button(&tree, "Zebra").is_none());
        let tree = press(&ctx, &mut app, "Clear filters");
        assert!(button(&tree, "Zebra").is_some());
        assert!(button(&tree, "Clear filters").is_none());
        assert!(!app.settings.library_by_you);
        app.backend.shutdown();
    }

    #[test]
    fn the_view_menu_maps_onto_the_compact_and_grid_settings() {
        assert_eq!(LibraryView::of(false, false), LibraryView::List);
        assert_eq!(LibraryView::of(true, false), LibraryView::Compact);
        assert_eq!(LibraryView::of(true, true), LibraryView::Grid);
        assert_eq!(LibraryView::Compact.flags(false), (true, false));
        assert_eq!(LibraryView::List.flags(true), (false, false));
        assert_eq!(LibraryView::Grid.flags(true), (true, true));

        let (ctx, mut app) = ui_app("view-menu");
        app.settings
            .library_sort
            .insert(Filter::Playlists, LibrarySort::Name);
        press(&ctx, &mut app, "Name");
        press(&ctx, &mut app, "Compact");
        assert!(app.settings.sidebar_compact);
        assert!(!app.settings.sidebar_grid);
        app.backend.shutdown();
    }
}
