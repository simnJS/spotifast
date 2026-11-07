//! The Stats page: the listening figures Spotifast keeps on this computer,
//! in the manner of stats.fm and in Spotify's look. Range chips, a hero of
//! big numbers over the top artist's colour, the top artists, tracks,
//! albums and genres with how their ranks moved, two charts, the import of
//! Spotify's full history and the logger's last sync, and a Share button
//! that saves the range's top as an image ([`crate::share_card`]).
//!
//! Every figure comes from [`crate::stats::query`], read on a worker
//! thread (`jobs`) whenever the index or the range changes.

mod charts;
mod jobs;
pub mod summary;

use std::sync::{Arc, Mutex};

use egui::{Align, CornerRadius, Frame, Layout, Margin, Rect, Sense, Ui, Vec2, pos2, vec2};

use crate::app::App;
use crate::i18n::Locale;
use crate::model::{Action, Loadable, Page};
use crate::stats::Index;
use crate::theme::{self, Icon, Palette};

use super::widgets;
use jobs::Pending;
use summary::{Key, Move, Span, Summary, tr};

/// Where the privacy page that sends the full history is.
const PRIVACY_URL: &str = "https://www.spotify.com/account/privacy/";

/// How many top tracks show before "Show all".
const TRACKS_FOLDED: usize = 10;

/// What the page keeps between frames: the range, the summary on show and
/// the one being read, and the dialogs and the share on their way. It lives
/// in egui's memory for the session, so App holds nothing of the page's.
#[derive(Default)]
struct View {
    span: Span,
    shown: Option<(Key, Arc<Summary>)>,
    reading: Option<(Key, Pending<Summary>)>,
    /// The index on show and when it came: the logger's last sync.
    loaded: Option<(usize, i64)>,
    /// A Sync now on its way: the index it should replace, and when.
    syncing: Option<(usize, i64)>,
    /// The page asked for the stats once already.
    requested: bool,
    sharing: Option<Pending<jobs::Shared>>,
    all_tracks: bool,
}

type Shared = Arc<Mutex<View>>;

fn view(ctx: &egui::Context) -> Shared {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_insert_with(egui::Id::new("stats-page"), Shared::default)
            .clone()
    })
}

fn lock(view: &Shared) -> std::sync::MutexGuard<'_, View> {
    view.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The top artist's image for the page's wash: the summary on show.
pub fn hero_image(ctx: &egui::Context) -> Option<String> {
    let view = view(ctx);
    let view = lock(&view);
    view.shown
        .as_ref()
        .and_then(|(_, summary)| summary.hero_image().map(str::to_owned))
}

fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

pub fn page(app: &mut App, ui: &mut Ui) {
    let shared = view(ui.ctx());
    let mut view = lock(&shared);
    take_answers(app, ui.ctx(), &mut view);

    let palette = app.palette;
    let locale = app.locale;
    let index = match &app.stats.index {
        Loadable::Loaded(index) => Some(Arc::clone(index)),
        _ => None,
    };
    if matches!(app.stats.index, Loadable::NotLoaded) && !view.requested {
        view.requested = true;
        app.actions.push(Action::RefreshStats);
    }
    let summary = index
        .as_ref()
        .and_then(|index| current(ui.ctx(), &mut view, index));

    hero(app, ui, &view, summary.as_deref());
    ui.add_space(20.0);
    action_bar(app, ui, &mut view, summary.as_deref());
    ui.add_space(24.0);

    match (&app.stats.index, summary) {
        (Loadable::Failed(error), _) => {
            let error = error.clone();
            failed(app, ui, &error);
        }
        (Loadable::Loaded(_), Some(summary)) if !summary.any_streams => {
            widgets::empty_state(
                ui,
                &palette,
                Icon::TrendingUp,
                &tr(locale, "No listening stats yet"),
                &tr(
                    locale,
                    "Play some music, or import your streaming history below.",
                ),
            );
            ui.add_space(32.0);
        }
        (Loadable::Loaded(_), Some(summary)) if summary.totals.streams == 0 => {
            widgets::empty_state(
                ui,
                &palette,
                Icon::Clock,
                &tr(locale, "Nothing played in this range"),
                &tr(locale, "Choose a longer range to see your top."),
            );
            ui.add_space(32.0);
        }
        (Loadable::Loaded(_), Some(summary)) => sections(app, ui, &mut view, &summary),
        _ => skeleton(ui, &palette, locale),
    }
    import_card(app, ui, &mut view);
}

/// Takes in the dialogs' and the share's answers.
fn take_answers(app: &mut App, ctx: &egui::Context, view: &mut View) {
    if let Some(shared) = view.sharing.as_ref().and_then(Pending::take) {
        view.sharing = None;
        ctx.copy_image(shared.image);
        match shared.saved {
            Ok(Some(path)) => app.toast(
                tr(app.locale, "Saved to {path} and copied")
                    .replace("{path}", &path.display().to_string()),
            ),
            Ok(None) => app.toast(tr(app.locale, "Image copied")),
            Err(error) => app.toast_error(
                tr(app.locale, "Could not save the image: {error}").replace("{error}", &error),
            ),
        }
    }
}

/// The summary for the chosen range: the one on show while another is
/// read, and nothing until the first is.
fn current(ctx: &egui::Context, view: &mut View, index: &Arc<Index>) -> Option<Arc<Summary>> {
    let now = now_ms();
    let address = Arc::as_ptr(index) as usize;
    if view.loaded.is_none_or(|(loaded, _)| loaded != address) {
        view.loaded = Some((address, now));
    }
    if view
        .syncing
        .is_some_and(|(before, at)| before != address || now - at > 60_000)
    {
        view.syncing = None;
    }
    let key = Key::new(index, view.span, now);
    if let Some((reading, pending)) = &view.reading {
        if *reading == key {
            if let Some(summary) = pending.take() {
                view.shown = Some((key.clone(), Arc::new(summary)));
                view.reading = None;
            }
        } else {
            view.reading = None;
        }
    }
    let fresh = view.shown.as_ref().is_some_and(|(shown, _)| *shown == key);
    if !fresh && view.reading.is_none() {
        let pending = jobs::summarize(ctx, Arc::clone(index), view.span, now);
        view.reading = Some((key, pending));
    }
    // A summary of another range is not shown as this one's.
    view.shown
        .as_ref()
        .filter(|(shown, _)| shown.index == address && shown.span == view.span)
        .map(|(_, summary)| Arc::clone(summary))
}

/// How large the hero's number is: Spotify's huge header type, smaller on
/// a narrow page.
pub fn hero_size(width: f32) -> f32 {
    (width * 0.11).clamp(40.0, 96.0)
}

fn hero(app: &App, ui: &mut Ui, view: &View, summary: Option<&Summary>) {
    let palette = app.palette;
    let locale = app.locale;
    ui.add_space(36.0);
    theme::text(
        ui,
        format!(
            "{} · {}",
            tr(locale, "Listening stats"),
            view.span.label(locale)
        ),
        theme::semibold(14.0),
        palette.text,
    );
    ui.add_space(4.0);
    let width = ui.available_width();
    let size = hero_size(width);
    let Some(summary) = summary else {
        // The hero's skeleton: the number, then the three figures.
        let fill = widgets::skeleton_fill(&palette);
        let (rect, _) = ui.allocate_exact_size(vec2(width.min(520.0), size), Sense::hover());
        ui.painter().rect_filled(
            rect.shrink2(vec2(0.0, size * 0.12)),
            CornerRadius::same(8),
            fill,
        );
        ui.add_space(12.0);
        let (rect, _) = ui.allocate_exact_size(vec2(width.min(380.0), 44.0), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(6), fill);
        return;
    };
    let totals = summary.totals;
    let number = summary::count(totals.minutes());
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        ui.add(
            egui::Label::new(
                egui::RichText::new(number)
                    .font(theme::bold(size))
                    .color(palette.text),
            )
            .selectable(false),
        );
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.add_space(size * 0.14);
            theme::text(
                ui,
                tr(locale, "minutes listened"),
                theme::bold((size * 0.3).max(18.0)),
                palette.text,
            );
        });
    });
    ui.add_space(10.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(36.0, 8.0);
        for (value, label) in [
            (u64::from(totals.streams), tr(locale, "streams")),
            (totals.artists as u64, tr(locale, "artists")),
            (totals.tracks as u64, tr(locale, "tracks")),
        ] {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                theme::text(ui, summary::count(value), theme::bold(28.0), palette.text);
                theme::text(ui, label, theme::regular(13.5), palette.secondary);
            });
        }
    });
}

/// The range chips and the Share button, as Spotify's filter row.
fn action_bar(app: &mut App, ui: &mut Ui, view: &mut View, summary: Option<&Summary>) {
    let palette = app.palette;
    let locale = app.locale;
    let labels: Vec<(Span, String)> = Span::ALL
        .iter()
        .map(|span| (*span, span.chip(locale)))
        .collect();
    let options: Vec<(Span, &str)> = labels
        .iter()
        .map(|(span, label)| (*span, label.as_str()))
        .collect();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        if let Some(span) = widgets::wrapped_chips(ui, &palette, &options, view.span) {
            view.span = span;
            view.all_tracks = false;
        }
        ui.add_space(8.0);
        let sharing = view.sharing.is_some();
        let ready = summary.is_some_and(|summary| summary.totals.streams > 0);
        let label = if sharing {
            tr(locale, "Sharing…")
        } else {
            tr(locale, "Share")
        };
        let share = ui
            .add_enabled_ui(ready && !sharing, |ui| {
                theme::soft_button(ui, &palette, Some(Icon::ExternalLink), &label, false)
            })
            .inner
            .on_hover_text(tr(locale, "Save this range's top as an image and copy it"));
        if share.clicked()
            && let Some(summary) = summary
        {
            start_share(app, ui.ctx(), view, summary);
        }
    });
}

/// Builds the share card from the summary on show and starts drawing it.
fn start_share(app: &mut App, ctx: &egui::Context, view: &mut View, summary: &Summary) {
    let locale = app.locale;
    let accent = summary
        .hero_image()
        .map(str::to_owned)
        .and_then(|url| app.tint_for(Some(&url)))
        .map(|color| [color.r(), color.g(), color.b()]);
    let name = app.user.as_ref().and_then(|user| user.display_name.clone());
    let today = jiff::Zoned::now().date();
    let (card, urls) = share_card(locale, summary, name.as_deref(), accent, today);
    view.sharing = Some(jobs::share(
        ctx,
        app.backend.art().clone(),
        card,
        urls,
        &tr(locale, "Save your top"),
    ));
}

/// The share card of `summary`, without its covers, and the covers' URLs:
/// its artists' then its tracks'.
pub fn share_card(
    locale: Locale,
    summary: &Summary,
    name: Option<&str>,
    accent: Option<[u8; 3]>,
    today: jiff::civil::Date,
) -> (crate::share_card::Card, Vec<Option<String>>) {
    let totals = summary.totals;
    let title = name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map_or_else(
            || tr(locale, "Your top"),
            |name| tr(locale, "{name}'s top").replace("{name}", name),
        );
    let artists = summary.artists.iter().take(crate::share_card::TOP);
    let tracks = summary.tracks.iter().take(crate::share_card::TOP);
    let mut urls: Vec<Option<String>> =
        artists.clone().map(|artist| artist.image.clone()).collect();
    urls.extend(tracks.clone().map(|track| track.image.clone()));
    let figure = |value: u64, label: &'static str| (summary::count(value), tr(locale, label));
    let card = crate::share_card::Card {
        title,
        range: summary.span.label(locale),
        figures: vec![
            figure(totals.minutes(), "minutes"),
            figure(u64::from(totals.streams), "streams"),
            figure(totals.artists as u64, "artists"),
            figure(totals.tracks as u64, "tracks"),
        ],
        artists_title: tr(locale, "Top artists"),
        artists: artists
            .map(|artist| crate::share_card::Entry {
                title: artist.name.clone(),
                subtitle: String::new(),
                figure: summary::streams(locale, artist.count.streams),
                cover: None,
            })
            .collect(),
        tracks_title: tr(locale, "Top tracks"),
        tracks: tracks
            .map(|track| crate::share_card::Entry {
                title: track.name.clone(),
                subtitle: credits(track),
                figure: summary::played(locale, track.count),
                cover: None,
            })
            .collect(),
        accent,
        footer: tr(locale, "Made with Spotifast · {date}")
            .replace("{date}", &today.strftime("%B %-d, %Y").to_string()),
    };
    (card, urls)
}

fn credits(track: &summary::TrackRow) -> String {
    track
        .artists
        .iter()
        .map(|artist| artist.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn failed(app: &mut App, ui: &mut Ui, error: &str) {
    let palette = app.palette;
    let locale = app.locale;
    let message = widgets::sanitize_error_message(error, locale).into_owned();
    ui.horizontal_wrapped(|ui| {
        theme::icon(ui, Icon::CircleAlert, 16.0, palette.danger);
        theme::text(ui, message, theme::regular(13.5), palette.secondary);
        if theme::soft_button(
            ui,
            &palette,
            Some(Icon::Refresh),
            &tr(locale, "Retry"),
            false,
        )
        .clicked()
        {
            app.actions.push(Action::RefreshStats);
        }
    });
    ui.add_space(32.0);
}

/// The page's shape while the stats load, in the app's skeleton style.
fn skeleton(ui: &mut Ui, palette: &Palette, locale: Locale) {
    widgets::section_title(ui, palette, &tr(locale, "Top artists"));
    ui.add_space(8.0);
    widgets::grid(ui, |ui| {
        let columns = (ui.available_width() / (widgets::CARD_WIDTH + widgets::CARD_GAP / 2.0))
            .floor()
            .clamp(1.0, 6.0) as usize;
        widgets::skeleton_cards(ui, palette, locale, columns, true);
    });
    ui.add_space(24.0);
    widgets::section_title(ui, palette, &tr(locale, "Top tracks"));
    ui.add_space(8.0);
    widgets::skeleton_rows(ui, palette, locale, 5);
    ui.add_space(32.0);
}

fn section(ui: &mut Ui, palette: &Palette, title: &str, note: Option<&str>) {
    ui.add_space(8.0);
    widgets::section_title(ui, palette, title);
    if let Some(note) = note {
        theme::text(ui, note, theme::regular(13.5), palette.secondary);
    }
    ui.add_space(10.0);
}

fn sections(app: &mut App, ui: &mut Ui, view: &mut View, summary: &Summary) {
    let palette = app.palette;
    let locale = app.locale;
    if !summary.artists.is_empty() {
        section(ui, &palette, &tr(locale, "Top artists"), None);
        artists(app, ui, summary);
        ui.add_space(28.0);
    }
    if !summary.tracks.is_empty() {
        section(ui, &palette, &tr(locale, "Top tracks"), None);
        tracks(app, ui, view, summary);
        ui.add_space(28.0);
    }
    if !summary.albums.is_empty() {
        section(ui, &palette, &tr(locale, "Top albums"), None);
        albums(app, ui, summary);
        ui.add_space(28.0);
    }
    section(
        ui,
        &palette,
        &tr(locale, "Streams per day"),
        Some(&summary.span.label(locale)),
    );
    charts::per_day(ui, &palette, locale, &summary.days);
    ui.add_space(28.0);
    // Genres and hours side by side when the page is wide enough.
    let wide = ui.available_width() >= 760.0;
    let genres = |app: &mut App, ui: &mut Ui| {
        section(ui, &palette, &tr(locale, "Top genres"), None);
        if summary.genres.is_empty() {
            theme::text(
                ui,
                tr(
                    locale,
                    "Genres appear once the artists' details have been read.",
                ),
                theme::regular(13.5),
                palette.secondary,
            );
        } else {
            genre_bars(app, ui, summary);
        }
    };
    let hours = |ui: &mut Ui| {
        section(ui, &palette, &tr(locale, "Listening by hour of day"), None);
        charts::per_hour(ui, &palette, locale, &summary.hours);
    };
    if wide {
        ui.columns(2, |columns| {
            columns[0].add_space(0.0);
            let (left, right) = columns.split_at_mut(1);
            genres(app, &mut left[0]);
            hours(&mut right[0]);
        });
    } else {
        genres(app, ui);
        ui.add_space(28.0);
        hours(ui);
    }
    ui.add_space(32.0);
}

/// Paints how a rank moved, from `left` on the line centred at `y`.
/// Answers the width taken.
/// How wide [`paint_move`] draws `mv`.
fn move_width(ui: &Ui, palette: &Palette, mv: &Move) -> f32 {
    let painter = ui.painter();
    match mv {
        Move::Still => 0.0,
        Move::Up(places) | Move::Down(places) => {
            11.0 + painter
                .layout_no_wrap(places.clone(), theme::semibold(11.5), palette.text)
                .size()
                .x
        }
        Move::New(label) => {
            10.0 + painter
                .layout_no_wrap(label.clone(), theme::bold(9.5), palette.on_accent)
                .size()
                .x
        }
    }
}

/// [`paint_move`] in the top right corner of a cover, on a pill of the
/// panel's colour so it reads over any picture.
fn paint_move_on_cover(ui: &Ui, palette: &Palette, mv: &Move, top_right: egui::Pos2) {
    let width = move_width(ui, palette, mv);
    if width <= 0.0 {
        return;
    }
    let padding = if matches!(mv, Move::New(_)) { 0.0 } else { 6.0 };
    let pill = Rect::from_min_max(
        pos2(top_right.x - width - 2.0 * padding, top_right.y),
        pos2(top_right.x, top_right.y + 18.0),
    );
    if padding > 0.0 {
        ui.painter().rect_filled(
            pill,
            CornerRadius::same(9),
            palette.panel.gamma_multiply(0.9),
        );
    }
    paint_move(ui, palette, mv, pill.left() + padding, pill.center().y);
}

fn paint_move(ui: &Ui, palette: &Palette, mv: &Move, left: f32, y: f32) -> f32 {
    let painter = ui.painter();
    let font = theme::semibold(11.5);
    match mv {
        Move::Still => 0.0,
        Move::Up(places) | Move::Down(places) => {
            let up = matches!(mv, Move::Up(_));
            let color = if up { palette.accent } else { palette.danger };
            let (tip, base) = if up { (-4.0, 3.0) } else { (4.0, -3.0) };
            painter.add(egui::Shape::convex_polygon(
                vec![
                    pos2(left + 4.0, y + tip),
                    pos2(left + 8.0, y + base),
                    pos2(left, y + base),
                ],
                color,
                egui::Stroke::NONE,
            ));
            let text = painter.text(
                pos2(left + 11.0, y),
                egui::Align2::LEFT_CENTER,
                places,
                font,
                color,
            );
            text.right() - left
        }
        Move::New(label) => {
            let galley = painter.layout_no_wrap(label.clone(), theme::bold(9.5), palette.on_accent);
            let rect = Rect::from_min_size(pos2(left, y - 8.0), vec2(galley.size().x + 10.0, 16.0));
            painter.rect_filled(rect, CornerRadius::same(8), palette.accent);
            painter.galley(
                pos2(rect.left() + 5.0, y - galley.size().y / 2.0),
                galley,
                palette.on_accent,
            );
            rect.width()
        }
    }
}

fn artists(app: &mut App, ui: &mut Ui, summary: &Summary) {
    let palette = app.palette;
    let locale = app.locale;
    widgets::grid(ui, |ui| {
        for artist in &summary.artists {
            let card = widgets::card(
                ui,
                app,
                artist.image.as_deref(),
                &artist.name,
                &summary::played(locale, artist.count),
                widgets::CardCover {
                    circular: true,
                    playable: false,
                    playing: false,
                },
            );
            let art = Rect::from_min_size(
                card.response.rect.min + Vec2::splat(widgets::CARD_PAD),
                Vec2::splat(widgets::CARD_ART),
            );
            rank_badge(
                ui,
                &palette,
                art.left_bottom() + vec2(18.0, -18.0),
                artist.rank,
            );
            let mv = summary::movement(locale, artist.delta);
            paint_move_on_cover(ui, &palette, &mv, art.right_top() + vec2(-8.0, 8.0));
            if card.clicked
                && let Some(id) = &artist.id
            {
                app.actions.push(Action::Open(Page::Artist(id.clone())));
            }
        }
    });
}

/// The rank on a card's cover: a disc in the panel's colour.
fn rank_badge(ui: &Ui, palette: &Palette, center: egui::Pos2, rank: u32) {
    let painter = ui.painter();
    let fill = if rank == 1 {
        palette.accent
    } else {
        palette.panel
    };
    let text = if rank == 1 {
        palette.on_accent
    } else {
        palette.text
    };
    painter.circle_filled(center, 15.0, fill);
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        rank.to_string(),
        theme::bold(13.0),
        text,
    );
}

fn tracks(app: &mut App, ui: &mut Ui, view: &mut View, summary: &Summary) {
    let palette = app.palette;
    let locale = app.locale;
    let (uris, positions) = summary.playable();
    let shown = if view.all_tracks {
        summary.tracks.len()
    } else {
        summary.tracks.len().min(TRACKS_FOLDED)
    };
    let playing = app.now_playing().map(|now| now.uri);
    for (track, position) in summary.tracks.iter().zip(&positions).take(shown) {
        let here = playing.is_some() && playing == track.uri;
        if track_row(app, ui, track, here)
            && let Some(index) = position
        {
            app.actions.push(Action::PlayUris {
                uris: uris.clone(),
                index: u32::try_from(*index).unwrap_or(0),
            });
        }
    }
    if summary.tracks.len() > TRACKS_FOLDED {
        ui.add_space(8.0);
        let label = if view.all_tracks {
            tr(locale, "Show less")
        } else {
            tr(locale, "Show all")
        };
        if theme::link(ui, &label, theme::bold(14.0), palette.secondary).clicked() {
            view.all_tracks = !view.all_tracks;
        }
    }
}

/// One row of the top tracks, as Spotify's track list draws it. Answers
/// whether it was double-clicked (or Enter pressed on it): play it.
fn track_row(app: &mut App, ui: &mut Ui, track: &summary::TrackRow, playing: bool) -> bool {
    let palette = app.palette;
    let locale = app.locale;
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, theme::ROW_HEIGHT), Sense::click());
    let credits = credits(track);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            true,
            format!(
                "{}. {}, {}, {}",
                track.rank,
                track.name,
                credits,
                summary::played(locale, track.count)
            ),
        )
    });
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered() || response.has_focus();
        if hovered {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(4), palette.surface_hover);
        }
        let middle = rect.center().y;
        let mut left = rect.left() + 8.0;
        // The rank, or the play glyph under the pointer.
        let rank = Rect::from_min_size(pos2(left, middle - 10.0), vec2(28.0, 20.0));
        if hovered && track.uri.is_some() {
            theme::paint_icon(ui, Icon::PlayFilled, rank, 14.0, palette.text);
        } else {
            ui.painter().text(
                rank.right_center(),
                egui::Align2::RIGHT_CENTER,
                track.rank.to_string(),
                theme::regular(16.0),
                if playing {
                    palette.accent
                } else {
                    palette.secondary
                },
            );
        }
        left = rank.right() + 10.0;
        paint_move(
            ui,
            &palette,
            &summary::movement(locale, track.delta),
            left,
            middle,
        );
        left += 34.0;
        let cover = Rect::from_min_size(pos2(left, middle - 20.0), Vec2::splat(40.0));
        widgets::paint_cover(
            ui,
            &palette,
            track.image.as_deref(),
            cover,
            4.0,
            Icon::Music,
            Some(app.backend.art()),
        );
        left = cover.right() + 12.0;
        // The figures on the right: streams always, minutes when there is room.
        let wide = width >= 560.0;
        let figures = if wide { 210.0 } else { 96.0 };
        let right = rect.right() - 12.0;
        ui.painter().text(
            pos2(if wide { right - 96.0 } else { right }, middle),
            egui::Align2::RIGHT_CENTER,
            summary::streams(locale, track.count.streams),
            theme::regular(14.0),
            palette.secondary,
        );
        if wide {
            ui.painter().text(
                pos2(right, middle),
                egui::Align2::RIGHT_CENTER,
                summary::minutes(locale, track.count.minutes()),
                theme::regular(14.0),
                palette.secondary,
            );
        }
        let room = (right - figures - left).max(40.0);
        let title = widgets::ellipsized(
            ui,
            &track.name,
            theme::regular(16.0),
            if playing {
                palette.accent
            } else {
                palette.text
            },
            room,
            1,
        );
        let artists = widgets::ellipsized(
            ui,
            &credits,
            theme::regular(14.0),
            palette.secondary,
            room,
            1,
        );
        let top = middle - (title.size().y + artists.size().y + 2.0) / 2.0;
        ui.painter()
            .galley(pos2(left, top), title.clone(), palette.text);
        ui.painter().galley(
            pos2(left, top + title.size().y + 2.0),
            artists,
            palette.secondary,
        );
    }
    theme::focus_ring(ui, &response);
    let activated = response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
    response.double_clicked() || activated
}

fn albums(app: &mut App, ui: &mut Ui, summary: &Summary) {
    let palette = app.palette;
    let locale = app.locale;
    widgets::grid(ui, |ui| {
        for album in &summary.albums {
            let subtitle = match &album.artist {
                Some(artist) => format!(
                    "{artist} · {}",
                    summary::streams(locale, album.count.streams)
                ),
                None => summary::played(locale, album.count),
            };
            let card = widgets::card(
                ui,
                app,
                album.image.as_deref(),
                &album.name,
                &subtitle,
                widgets::CardCover {
                    circular: false,
                    playable: album.id.is_some(),
                    playing: false,
                },
            );
            let art = Rect::from_min_size(
                card.response.rect.min + Vec2::splat(widgets::CARD_PAD),
                Vec2::splat(widgets::CARD_ART),
            );
            rank_badge(ui, &palette, art.left_top() + vec2(18.0, 18.0), album.rank);
            let mv = summary::movement(locale, album.delta);
            paint_move_on_cover(ui, &palette, &mv, art.right_top() + vec2(-8.0, 8.0));
            if let Some(id) = &album.id {
                if card.play {
                    app.actions.push(Action::PlayContext {
                        uri: format!("spotify:album:{id}"),
                        offset_uri: None,
                        offset_index: None,
                    });
                } else if card.clicked {
                    app.actions.push(Action::Open(Page::Album(id.clone())));
                }
            }
        }
    });
}

fn genre_bars(app: &App, ui: &mut Ui, summary: &Summary) {
    let palette = app.palette;
    let locale = app.locale;
    for genre in &summary.genres {
        let width = ui.available_width();
        let (rect, response) = ui.allocate_exact_size(vec2(width, 34.0), Sense::hover());
        let figure = summary::streams(locale, genre.count.streams);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Label,
                true,
                format!("{}, {figure}", genre.name),
            )
        });
        if !ui.is_rect_visible(rect) {
            continue;
        }
        let name_width = (width * 0.36).clamp(90.0, 200.0);
        let name = widgets::ellipsized(
            ui,
            &capitalized(&genre.name),
            theme::regular(14.0),
            palette.text,
            name_width - 8.0,
            1,
        );
        ui.painter().galley(
            pos2(rect.left(), rect.center().y - name.size().y / 2.0),
            name,
            palette.text,
        );
        let figure_width = 92.0;
        let track = Rect::from_min_max(
            pos2(rect.left() + name_width, rect.center().y - 4.0),
            pos2(
                (rect.right() - figure_width).max(rect.left() + name_width + 8.0),
                rect.center().y + 4.0,
            ),
        );
        ui.painter()
            .rect_filled(track, CornerRadius::same(4), palette.surface);
        let fill = Rect::from_min_size(
            track.min,
            vec2((track.width() * genre.share).max(4.0), track.height()),
        );
        ui.painter().rect_filled(
            fill,
            CornerRadius::same(4),
            if response.hovered() {
                palette.accent_hover
            } else {
                palette.accent
            },
        );
        ui.painter().text(
            pos2(rect.right(), rect.center().y),
            egui::Align2::RIGHT_CENTER,
            figure,
            theme::regular(13.0),
            palette.secondary,
        );
    }
}

fn capitalized(text: &str) -> String {
    let mut characters = text.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// How to get the full history, the import and its progress, and the
/// logger's last sync.
fn import_card(app: &mut App, ui: &mut Ui, view: &mut View) {
    let palette = app.palette;
    let locale = app.locale;
    Frame::new()
        .fill(palette.surface)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(20))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0;
            theme::text(
                ui,
                tr(locale, "Import your full history"),
                theme::bold(20.0),
                palette.text,
            );
            ui.add(
                egui::Label::new(
                    egui::RichText::new(tr(
                        locale,
                        "Spotify keeps every play since your account began. On your Spotify \
                         account page, open Privacy, ask to download your data and tick \
                         \"Extended streaming history\". Spotify emails the ZIP within a few \
                         days. Choose it (or its JSON files) here: it stays on this computer.",
                    ))
                    .font(theme::regular(13.5))
                    .color(palette.secondary),
                )
                .wrap()
                .selectable(false),
            );
            if theme::link(
                ui,
                tr(locale, "Open Spotify's privacy page"),
                theme::semibold(13.5),
                palette.text,
            )
            .clicked()
            {
                app.actions.push(Action::OpenUrl(PRIVACY_URL.to_owned()));
            }
            ui.add_space(8.0);
            let importing = app.stats.importing.clone();
            ui.horizontal_wrapped(|ui| {
                let busy = importing.is_some();
                let choose = ui
                    .add_enabled_ui(!busy, |ui| {
                        theme::pill_button(ui, &palette, &tr(locale, "Choose file…"), true)
                    })
                    .inner;
                if choose.clicked() {
                    // The backend asks for the file in its native dialog.
                    app.actions.push(Action::ImportStreamingHistory(None));
                }
                if let Some(progress) = &importing {
                    theme::spinner(ui, 16.0, palette.accent);
                    theme::text(
                        ui,
                        tr(locale, "Reading file {done} of {total} · {plays} plays")
                            .replace("{done}", &progress.files_done.to_string())
                            .replace("{total}", &progress.files_total.to_string())
                            .replace("{plays}", &summary::count(progress.plays_read as u64)),
                        theme::regular(13.5),
                        palette.secondary,
                    );
                } else if let Some(result) = &app.stats.last_import {
                    let (icon, color, text) = match result {
                        Ok(done) => (
                            Icon::CircleCheck,
                            palette.accent,
                            tr(
                                locale,
                                "Added {added} plays from {files} files ({known} already known)",
                            )
                            .replace("{added}", &summary::count(done.added as u64))
                            .replace("{files}", &done.files.to_string())
                            .replace("{known}", &summary::count(done.duplicates as u64)),
                        ),
                        Err(error) => (
                            Icon::CircleAlert,
                            palette.danger,
                            widgets::sanitize_error_message(error, locale).into_owned(),
                        ),
                    };
                    theme::icon(ui, icon, 16.0, color);
                    theme::text(ui, text, theme::regular(13.5), palette.secondary);
                }
            });
            ui.add_space(10.0);
            ui.separator();
            ui.add_space(6.0);
            sync_line(app, ui, view);
        });
    ui.add_space(24.0);
}

fn sync_line(app: &mut App, ui: &mut Ui, view: &mut View) {
    let palette = app.palette;
    let locale = app.locale;
    ui.horizontal_wrapped(|ui| {
        let syncing = view.syncing.is_some() || app.stats.index.is_loading();
        if syncing {
            theme::spinner(ui, 16.0, palette.accent);
            theme::text(
                ui,
                tr(locale, "Syncing…"),
                theme::regular(13.5),
                palette.secondary,
            );
        } else if let Some((_, at)) = view.loaded {
            theme::icon(ui, Icon::Refresh, 14.0, palette.secondary);
            theme::text(
                ui,
                summary::updated(locale, now_ms() - at),
                theme::regular(13.5),
                palette.secondary,
            );
            // The "ago" moves on while the page stays open.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs(30));
        }
        let sync = ui
            .add_enabled_ui(!syncing && app.stats.importing.is_none(), |ui| {
                theme::soft_button(ui, &palette, None, &tr(locale, "Sync now"), false)
            })
            .inner;
        if sync.clicked() {
            let address = match &app.stats.index {
                Loadable::Loaded(index) => Arc::as_ptr(index) as usize,
                _ => 0,
            };
            view.syncing = Some((address, now_ms()));
            app.actions.push(Action::RefreshStats);
        }
    });
    theme::text(
        ui,
        tr(
            locale,
            "While Spotifast runs, it records what you play on any device every 20 minutes.",
        ),
        theme::regular(12.0),
        palette.dim,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hero_number_shrinks_on_a_narrow_page() {
        assert_eq!(hero_size(1400.0), 96.0);
        assert_eq!(hero_size(300.0), 40.0);
        assert!(hero_size(600.0) > 40.0 && hero_size(600.0) < 96.0);
    }

    #[test]
    fn the_share_card_carries_the_range_top_five_and_totals() {
        let now = 1_700_000_000_000;
        let index = crate::demo::stats_index(now);
        let summary = summary::summarize(&index, Span::Months6, now, &jiff::tz::TimeZone::UTC);
        let locale = Locale::default();
        let today = jiff::civil::date(2026, 11, 7);
        let (card, urls) = share_card(locale, &summary, Some("Simon"), None, today);
        assert_eq!(card.title, "Simon's top");
        assert_eq!(card.range, "Last 6 months");
        assert_eq!(card.footer, "Made with Spotifast · November 7, 2026");
        assert_eq!(card.figures[0].0, summary::count(summary.totals.minutes()));
        assert_eq!(
            card.figures[1].0,
            summary::count(u64::from(summary.totals.streams))
        );
        assert_eq!(card.artists.len(), 5);
        assert_eq!(card.tracks.len(), 5);
        assert_eq!(card.artists[0].title, summary.artists[0].name);
        assert_eq!(card.tracks[4].title, summary.tracks[4].name);
        assert_eq!(urls.len(), 10);
        assert_eq!(urls[0], summary.artists[0].image);
        assert_eq!(urls[5], summary.tracks[0].image);
        let (card, _) = share_card(locale, &summary, Some("  "), None, today);
        assert_eq!(card.title, "Your top");
        // It draws: the PNG a Share saves.
        let image = crate::share_card::render(&card);
        let png = crate::share_card::png(&image).unwrap();
        if let Ok(path) = std::env::var("SPOTIFAST_SHARE_CARD_OUT") {
            std::fs::write(path, &png).unwrap();
        }
        assert_eq!(
            image.size,
            [crate::share_card::WIDTH, crate::share_card::HEIGHT]
        );
    }

    #[test]
    fn genres_start_with_a_capital() {
        assert_eq!(capitalized("indie rock"), "Indie rock");
        assert_eq!(capitalized(""), "");
        assert_eq!(capitalized("électro"), "Électro");
    }
}
