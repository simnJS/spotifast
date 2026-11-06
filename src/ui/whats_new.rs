//! What's new, behind the bell in the top bar: the latest releases from the
//! artists the account follows and the newest episodes of its podcasts, as
//! Spotify's own bell lists them. What arrived since the page was last
//! opened is marked and listed first; opening the page clears the bell here
//! only, never in Spotify's apps.

use jiff::civil::Date;

use crate::api::models::{Album, Episode, PlayableItem, Show, pick_image};
use crate::app::App;
use crate::i18n::{Locale, gettext, ngettext};
use crate::model::{Action, Loadable, Page};
use crate::theme::{self, Icon, Palette};

use super::widgets;

/// Which items the page lists, as the chips above them choose.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Music,
    Podcasts,
}

impl Filter {
    fn keeps(self, entry: &Entry) -> bool {
        match (self, &entry.item) {
            (Filter::All, _)
            | (Filter::Music, Item::Release(_))
            | (Filter::Podcasts, Item::Episode(_)) => true,
            (Filter::Music, Item::Episode(_)) | (Filter::Podcasts, Item::Release(_)) => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Release(Album),
    /// An episode, with its show filled in.
    Episode(Episode),
}

impl Item {
    pub fn uri(&self) -> &str {
        match self {
            Item::Release(album) => &album.uri,
            Item::Episode(episode) => &episode.uri,
        }
    }
}

/// One row of the page.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub item: Item,
    pub day: Date,
    pub unread: bool,
}

/// What the page can list, newest first: the feed's releases and episodes
/// and the saved podcasts' newest episodes, without repeats, released in the
/// last [`crate::new_releases::KEPT_DAYS`] days, at most
/// [`crate::new_releases::KEPT_ITEMS`] of them, then narrowed to `filter`.
/// `skip` leaves out shows that are audiobooks or no longer saved.
pub fn entries(
    albums: &[Album],
    feed_episodes: &[Episode],
    podcasts: &[(Show, Vec<Episode>)],
    skip: impl Fn(&Show) -> bool,
    unread: impl Fn(&str) -> bool,
    today: Date,
    filter: Filter,
) -> Vec<Entry> {
    let day = |date: Option<&str>| {
        crate::new_releases::is_recent(date, today)
            .then(|| date.and_then(crate::new_releases::release_day))
            .flatten()
    };
    let mut entries: Vec<Entry> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for album in albums {
        if let Some(day) = day(album.release_date.as_deref())
            && !album.uri.is_empty()
            && seen.insert(album.uri.clone())
        {
            entries.push(Entry {
                unread: unread(&album.uri),
                item: Item::Release(album.clone()),
                day,
            });
        }
    }
    let saved = podcasts
        .iter()
        .filter(|(show, _)| !skip(show))
        .flat_map(|(show, episodes)| {
            episodes.iter().map(move |episode| {
                let mut episode = episode.clone();
                // A show's episode list leaves out the show; rows need it.
                if episode.show.is_none() {
                    episode.show = Some(show.clone());
                }
                episode
            })
        });
    for episode in feed_episodes.iter().cloned().chain(saved) {
        if let Some(day) = day(episode.release_date.as_deref())
            && !episode.uri.is_empty()
            && seen.insert(episode.uri.clone())
        {
            entries.push(Entry {
                unread: unread(&episode.uri),
                item: Item::Episode(episode),
                day,
            });
        }
    }
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.day));
    entries.truncate(crate::new_releases::KEPT_ITEMS);
    entries.retain(|entry| filter.keeps(entry));
    entries
}

/// When an item came out, as the row says it: today, yesterday, so many
/// days ago, then so many weeks ago.
pub fn relative_day(locale: Locale, day: Date, today: Date) -> String {
    let days = (today - day).get_days().max(0);
    match days {
        0 => gettext(locale, "Today").into_owned(),
        1 => gettext(locale, "Yesterday").into_owned(),
        2..=6 => {
            let count = u32::try_from(days).unwrap_or(u32::MAX);
            ngettext(
                locale,
                // Translators: Keep {count} exactly as written. It becomes a number of days.
                "{count} day ago",
                "{count} days ago",
                count,
            )
            .replace("{count}", &count.to_string())
        }
        _ => {
            let count = u32::try_from(days / 7).unwrap_or(u32::MAX);
            ngettext(
                locale,
                // Translators: Keep {count} exactly as written. It becomes a number of weeks.
                "{count} week ago",
                "{count} weeks ago",
                count,
            )
            .replace("{count}", &count.to_string())
        }
    }
}

fn filter_id() -> egui::Id {
    egui::Id::new("whats-new-filter")
}

pub fn page(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let locale = app.locale;
    if matches!(app.home.new_releases, Loadable::NotLoaded) {
        app.ensure_loaded(Page::WhatsNew);
    }
    ui.add_space(12.0);
    theme::text(
        ui,
        gettext(locale, "What's new"),
        theme::bold(30.0),
        palette.text,
    );
    ui.add_space(4.0);
    theme::text(
        ui,
        gettext(
            locale,
            "The latest releases from artists you follow and episodes of your podcasts.",
        ),
        theme::regular(13.5),
        palette.secondary,
    );
    ui.add_space(14.0);

    let filter: Filter = ui
        .ctx()
        .data(|data| data.get_temp(filter_id()))
        .unwrap_or_default();
    let all = gettext(locale, "All");
    let music = gettext(locale, "Music");
    let podcasts_label = gettext(locale, "Podcasts & Shows");
    let options = [
        (Filter::All, all.as_ref()),
        (Filter::Music, music.as_ref()),
        (Filter::Podcasts, podcasts_label.as_ref()),
    ];
    if let Some(picked) = widgets::chips(ui, &palette, &options, filter) {
        ui.ctx()
            .data_mut(|data| data.insert_temp(filter_id(), picked));
    }
    ui.add_space(14.0);

    let today = jiff::Zoned::now().date();
    let albums = app.home.new_releases.get().cloned().unwrap_or_default();
    let shown = entries(
        &albums,
        &app.home.feed_episodes,
        &app.home.podcasts,
        |show| app.audiobook_shows.contains(&show.uri) || app.is_saved(&show.uri) == Some(false),
        |uri| app.is_whats_new_unread(uri),
        today,
        filter,
    );
    if shown.is_empty() {
        match &app.home.new_releases {
            Loadable::Loading | Loadable::NotLoaded => {
                widgets::loading_row(ui, &palette, locale);
                return;
            }
            Loadable::Failed(error) if filter != Filter::Podcasts => {
                let error = error.clone();
                widgets::error_row(ui, app, &error, Some(Page::WhatsNew));
                return;
            }
            _ => {}
        }
        let body = match filter {
            Filter::All => gettext(
                locale,
                "New releases from artists you follow and new episodes of your podcasts will appear here.",
            ),
            Filter::Music => gettext(
                locale,
                "New albums and singles from artists you follow will appear here.",
            ),
            Filter::Podcasts => gettext(
                locale,
                "New episodes of the podcasts you follow will appear here.",
            ),
        };
        widgets::empty_state(
            ui,
            &palette,
            Icon::Sparkles,
            &gettext(locale, "Nothing new yet"),
            &body,
        );
        return;
    }
    let (new, earlier): (Vec<&Entry>, Vec<&Entry>) = shown.iter().partition(|entry| entry.unread);
    for (title, group) in [
        (gettext(locale, "New"), new),
        (gettext(locale, "Earlier"), earlier),
    ] {
        if group.is_empty() {
            continue;
        }
        ui.add_space(6.0);
        theme::text(ui, title, theme::bold(17.0), palette.text);
        ui.add_space(6.0);
        for entry in group {
            row(app, ui, &palette, entry, today);
        }
        ui.add_space(12.0);
    }
}

const ROW_HEIGHT: f32 = 64.0;
const COVER: f32 = 48.0;
/// The room before the cover for the unread dot.
const DOT_ROOM: f32 = 20.0;
const PLAY: f32 = 32.0;

fn row(app: &mut App, ui: &mut egui::Ui, palette: &Palette, entry: &Entry, today: Date) {
    let locale = app.locale;
    let (title, subtitle, cover, open, play, round) = match &entry.item {
        Item::Release(album) => {
            let artists =
                crate::api::models::join_names(album.artists.iter().map(|a| a.name.as_str()));
            let kind = crate::notifications::kind(album, locale);
            let subtitle = if artists.is_empty() {
                kind.into_owned()
            } else {
                format!("{kind} • {artists}")
            };
            let context_here = app.playing_context_uri().as_deref() == Some(album.uri.as_str());
            let play = if context_here {
                Action::TogglePlay
            } else {
                Action::PlayContext {
                    uri: album.uri.clone(),
                    offset_uri: None,
                    offset_index: None,
                }
            };
            (
                album.name.clone(),
                subtitle,
                pick_image(&album.images, app.art_px(COVER)).map(str::to_owned),
                Some(Page::Album(album.id.clone())),
                play,
                4.0,
            )
        }
        Item::Episode(episode) => {
            let show = episode.show.as_ref();
            let kind = gettext(locale, "Episode");
            let subtitle = match show.filter(|show| !show.name.is_empty()) {
                Some(show) => format!("{kind} • {}", show.name),
                None => kind.into_owned(),
            };
            let cover = pick_image(&episode.images, app.art_px(COVER))
                .or_else(|| show.and_then(|show| pick_image(&show.images, app.art_px(COVER))))
                .map(str::to_owned);
            (
                episode.name.clone(),
                subtitle,
                cover,
                show.filter(|show| !show.id.is_empty())
                    .map(|show| Page::Show(show.id.clone())),
                Action::PlayEpisode {
                    uri: episode.uri.clone(),
                    resume_ms: episode.resume_ms(),
                },
                8.0,
            )
        }
    };
    let width = ui.available_width();
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &title)
    });
    let play_rect = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 12.0 - PLAY / 2.0, rect.center().y),
        egui::Vec2::splat(PLAY),
    );
    let play_tooltip = gettext(locale, "Play");
    let play_response = ui
        .scope_builder(egui::UiBuilder::new().max_rect(play_rect), |ui| {
            theme::circle_button(
                ui,
                Icon::PlayFilled,
                PLAY,
                palette.accent,
                palette.accent_hover,
                palette.on_accent,
                &play_tooltip,
            )
        })
        .inner;
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if response.hovered() || response.has_focus() {
            painter.rect_filled(rect, 6.0, palette.surface_hover);
        }
        if entry.unread {
            painter.circle_filled(
                egui::pos2(rect.left() + DOT_ROOM / 2.0, rect.center().y),
                4.0,
                palette.accent,
            );
        }
        let cover_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left() + DOT_ROOM, rect.center().y - COVER / 2.0),
            egui::Vec2::splat(COVER),
        );
        let fallback = match entry.item {
            Item::Release(_) => Icon::Disc,
            Item::Episode(_) => Icon::Mic,
        };
        widgets::paint_cover(
            ui,
            palette,
            cover.as_deref(),
            cover_rect,
            round,
            fallback,
            Some(app.backend.art()),
        );
        let date = painter.layout_no_wrap(
            relative_day(locale, entry.day, today),
            theme::regular(12.5),
            palette.secondary,
        );
        let date_right = play_rect.left() - 16.0;
        let text_left = cover_rect.right() + 12.0;
        let text_width = (date_right - date.size().x - 16.0 - text_left).max(0.0);
        let title_galley = widgets::ellipsized(
            ui,
            &title,
            theme::semibold(14.5),
            palette.text,
            text_width,
            1,
        );
        let subtitle_galley = widgets::ellipsized(
            ui,
            &subtitle,
            theme::regular(13.0),
            palette.secondary,
            text_width,
            1,
        );
        let block = title_galley.size().y + 3.0 + subtitle_galley.size().y;
        let top = rect.center().y - block / 2.0;
        let title_height = title_galley.size().y;
        painter.galley(egui::pos2(text_left, top), title_galley, palette.text);
        painter.galley(
            egui::pos2(text_left, top + title_height + 3.0),
            subtitle_galley,
            palette.secondary,
        );
        if text_width > 0.0 {
            painter.galley(
                egui::pos2(
                    date_right - date.size().x,
                    rect.center().y - date.size().y / 2.0,
                ),
                date,
                palette.secondary,
            );
        }
    }
    theme::focus_ring(ui, &response);
    if play_response.clicked() {
        app.actions.push(play);
    } else if response.clicked()
        && let Some(page) = open
    {
        app.actions.push(Action::Open(page));
    }
    egui::Popup::context_menu(&response)
        .id(ui.make_persistent_id(("whats-new-menu", entry.item.uri())))
        .frame(widgets::menu_frame(palette))
        .show(|ui| match &entry.item {
            Item::Release(album) => {
                widgets::context_menu_items(ui, app, &album.uri, &album.name, None);
            }
            Item::Episode(episode) => {
                widgets::item_menu(ui, app, &PlayableItem::Episode(episode.clone()), None, None);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> Date {
        jiff::civil::date(2026, 11, 6)
    }

    fn release(n: u8, date: &str) -> Album {
        Album {
            id: format!("a{n}"),
            uri: format!("spotify:album:a{n}"),
            name: format!("Release {n}"),
            release_date: Some(date.into()),
            ..Album::default()
        }
    }

    fn episode(n: u8, date: &str) -> Episode {
        Episode {
            id: format!("e{n}"),
            uri: format!("spotify:episode:e{n}"),
            name: format!("Episode {n}"),
            release_date: Some(date.into()),
            ..Episode::default()
        }
    }

    fn show(n: u8) -> Show {
        Show {
            id: format!("s{n}"),
            uri: format!("spotify:show:s{n}"),
            name: format!("Show {n}"),
            ..Show::default()
        }
    }

    fn names(entries: &[Entry]) -> Vec<String> {
        entries
            .iter()
            .map(|entry| match &entry.item {
                Item::Release(album) => album.name.clone(),
                Item::Episode(episode) => episode.name.clone(),
            })
            .collect()
    }

    #[test]
    fn releases_and_episodes_are_listed_newest_first_for_sixty_days() {
        let albums = [
            release(1, "2026-11-01"),
            release(2, "2026-08-01"),
            release(1, "2026-11-01"),
        ];
        let feed = [episode(1, "2026-11-05")];
        let podcasts = vec![(
            show(1),
            vec![
                episode(1, "2026-11-05"),
                episode(2, "2026-10-20"),
                episode(3, "2026-01-01"),
            ],
        )];
        let listed = entries(
            &albums,
            &feed,
            &podcasts,
            |_| false,
            |uri| uri == "spotify:episode:e2",
            today(),
            Filter::All,
        );
        assert_eq!(names(&listed), ["Episode 1", "Release 1", "Episode 2"]);
        assert!(listed[2].unread);
        assert!(!listed[0].unread);
        let Item::Episode(saved) = &listed[2].item else {
            panic!("an episode");
        };
        assert_eq!(
            saved.show.as_ref().map(|show| show.name.as_str()),
            Some("Show 1")
        );

        let music = entries(
            &albums,
            &feed,
            &podcasts,
            |_| false,
            |_| false,
            today(),
            Filter::Music,
        );
        assert_eq!(names(&music), ["Release 1"]);
        let shows = entries(
            &albums,
            &feed,
            &podcasts,
            |_| false,
            |_| false,
            today(),
            Filter::Podcasts,
        );
        assert_eq!(names(&shows), ["Episode 1", "Episode 2"]);
        let skipped = entries(
            &[],
            &[],
            &podcasts,
            |_| true,
            |_| false,
            today(),
            Filter::All,
        );
        assert!(
            skipped.is_empty(),
            "audiobooks and unsaved shows are left out"
        );
    }

    #[test]
    fn at_most_a_hundred_items_are_kept() {
        let albums: Vec<Album> = (0..=120u8)
            .map(|n| release(n, &format!("2026-10-{:02}", 1 + n % 30)))
            .collect();
        let listed = entries(
            &albums,
            &[],
            &[],
            |_| false,
            |_| false,
            today(),
            Filter::All,
        );
        assert_eq!(listed.len(), crate::new_releases::KEPT_ITEMS);
    }

    #[test]
    fn dates_read_as_spotify_says_them() {
        let locale = Locale::English;
        let day = |d| jiff::civil::date(2026, 11, d);
        assert_eq!(relative_day(locale, day(6), today()), "Today");
        assert_eq!(relative_day(locale, day(5), today()), "Yesterday");
        assert_eq!(relative_day(locale, day(4), today()), "2 days ago");
        assert_eq!(relative_day(locale, day(1), today()), "5 days ago");
        assert_eq!(
            relative_day(locale, jiff::civil::date(2026, 10, 30), today()),
            "1 week ago"
        );
        assert_eq!(
            relative_day(locale, jiff::civil::date(2026, 9, 10), today()),
            "8 weeks ago"
        );
        assert_eq!(relative_day(locale, day(8), today()), "Today");
    }
}
