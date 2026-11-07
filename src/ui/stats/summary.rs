//! What the Stats page shows for one range, read from the index by the
//! queries in [`crate::stats::query`] on a worker thread, and the pure
//! formatting the page and the share card use.

use std::sync::Arc;

use crate::i18n::{Locale, gettext};
use crate::stats::Index;
use crate::stats::query::{self, Count, Day, Delta, Item, Kind, Totals};

/// How many of each the page shows.
pub const ARTISTS: usize = 10;
pub const TRACKS: usize = 50;
pub const ALBUMS: usize = 10;
pub const GENRES: usize = 10;

/// The range chips: Spotify's own filter chips.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Span {
    #[default]
    Weeks4,
    Months6,
    Months12,
    AllTime,
}

impl Span {
    pub const ALL: [Span; 4] = [Span::Weeks4, Span::Months6, Span::Months12, Span::AllTime];

    pub fn range(self) -> query::Range {
        match self {
            Span::Weeks4 => query::Range::Weeks4,
            Span::Months6 => query::Range::Months6,
            Span::Months12 => query::Range::Months12,
            Span::AllTime => query::Range::AllTime,
        }
    }

    /// The chip's label.
    pub fn chip(self, locale: Locale) -> String {
        match self {
            Span::Weeks4 => gettext(locale, "4 weeks"),
            Span::Months6 => gettext(locale, "6 months"),
            Span::Months12 => gettext(locale, "12 months"),
            Span::AllTime => gettext(locale, "All time"),
        }
    }

    /// The range in a sentence or on the share card.
    pub fn label(self, locale: Locale) -> String {
        match self {
            Span::Weeks4 => gettext(locale, "Last 4 weeks"),
            Span::Months6 => gettext(locale, "Last 6 months"),
            Span::Months12 => gettext(locale, "Last 12 months"),
            Span::AllTime => gettext(locale, "All time"),
        }
    }
}

/// An artist as a play names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credit {
    pub id: Option<String>,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArtistRow {
    pub id: Option<String>,
    pub name: String,
    pub image: Option<String>,
    pub rank: u32,
    pub count: Count,
    pub delta: Delta,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrackRow {
    pub uri: Option<String>,
    pub name: String,
    pub artists: Vec<Credit>,
    pub album_id: Option<String>,
    pub image: Option<String>,
    pub rank: u32,
    pub count: Count,
    pub delta: Delta,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AlbumRow {
    pub id: Option<String>,
    pub name: String,
    pub artist: Option<String>,
    pub image: Option<String>,
    pub rank: u32,
    pub count: Count,
    pub delta: Delta,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GenreRow {
    pub name: String,
    pub count: Count,
    /// Its streams as a share of the top genre's, for the bar.
    pub share: f32,
}

/// Everything the page draws for one range.
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub span: Span,
    pub totals: Totals,
    /// Whether the log holds any stream at all, whatever the range.
    pub any_streams: bool,
    pub artists: Vec<ArtistRow>,
    pub tracks: Vec<TrackRow>,
    pub albums: Vec<AlbumRow>,
    pub genres: Vec<GenreRow>,
    pub days: Vec<Day>,
    pub hours: [Count; 24],
}

impl Summary {
    /// The top artist's image, which tints the hero and the share card.
    pub fn hero_image(&self) -> Option<&str> {
        self.artists
            .iter()
            .find_map(|artist| artist.image.as_deref())
    }

    /// The URIs of the top tracks that have one, in order, and where each
    /// row lands among them: what a double-click plays.
    pub fn playable(&self) -> (Vec<String>, Vec<Option<usize>>) {
        let mut uris = Vec::new();
        let positions = self
            .tracks
            .iter()
            .map(|track| {
                track.uri.as_ref().map(|uri| {
                    uris.push(uri.clone());
                    uris.len() - 1
                })
            })
            .collect();
        (uris, positions)
    }
}

/// Reads the summary of `span` from the index at `now_ms` in `zone`.
pub fn summarize(index: &Index, span: Span, now_ms: i64, zone: &jiff::tz::TimeZone) -> Summary {
    let range = span.range();
    let artists = query::top(index, Kind::Artist, range, now_ms, ARTISTS)
        .into_iter()
        .filter_map(|row| {
            let Item::Artist(id) = row.item else {
                return None;
            };
            let artist = index.artists.get(id as usize)?;
            Some(ArtistRow {
                id: artist.id.clone(),
                name: artist.name.clone(),
                image: artist.image.clone(),
                rank: row.rank,
                count: row.count,
                delta: row.delta,
            })
        })
        .collect();
    let album_of = |album: Option<u32>| album.and_then(|album| index.albums.get(album as usize));
    let tracks = query::top(index, Kind::Track, range, now_ms, TRACKS)
        .into_iter()
        .filter_map(|row| {
            let Item::Track(id) = row.item else {
                return None;
            };
            let track = index.tracks.get(id as usize)?;
            let album = album_of(track.album);
            Some(TrackRow {
                uri: track.uri.clone(),
                name: track.name.clone(),
                artists: track
                    .artists
                    .iter()
                    .filter_map(|artist| index.artists.get(*artist as usize))
                    .map(|artist| Credit {
                        id: artist.id.clone(),
                        name: artist.name.clone(),
                    })
                    .collect(),
                album_id: album.and_then(|album| album.id.clone()),
                image: album.and_then(|album| album.image.clone()),
                rank: row.rank,
                count: row.count,
                delta: row.delta,
            })
        })
        .collect();
    let albums = query::top(index, Kind::Album, range, now_ms, ALBUMS)
        .into_iter()
        .filter_map(|row| {
            let Item::Album(id) = row.item else {
                return None;
            };
            let album = index.albums.get(id as usize)?;
            Some(AlbumRow {
                id: album.id.clone(),
                name: album.name.clone(),
                artist: album
                    .artist
                    .and_then(|artist| index.artists.get(artist as usize))
                    .map(|artist| artist.name.clone()),
                image: album.image.clone(),
                rank: row.rank,
                count: row.count,
                delta: row.delta,
            })
        })
        .collect();
    let genres = query::top(index, Kind::Genre, range, now_ms, GENRES);
    let most = genres.first().map_or(1, |row| row.count.streams.max(1));
    let genres = genres
        .into_iter()
        .filter_map(|row| {
            let Item::Genre(id) = row.item else {
                return None;
            };
            Some(GenreRow {
                name: index.genres.get(id as usize)?.clone(),
                count: row.count,
                share: row.count.streams as f32 / most as f32,
            })
        })
        .collect();
    Summary {
        span,
        totals: query::totals(index, range, now_ms),
        any_streams: index.plays.iter().any(|play| play.is_stream()),
        artists,
        tracks,
        albums,
        genres,
        days: query::per_day(index, range, now_ms, zone),
        hours: query::per_hour(index, range, now_ms, zone),
    }
}

/// What the page asks for: one range of one index, at about one time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    /// The index, by address: a refresh brings a new one.
    pub index: usize,
    pub span: Span,
    /// The time, to ten minutes, so the ranges move on while the page stays
    /// open without being read again every frame.
    pub tick: i64,
}

impl Key {
    pub const TICK_MS: i64 = 10 * 60 * 1000;

    pub fn new(index: &Arc<Index>, span: Span, now_ms: i64) -> Self {
        Self {
            index: Arc::as_ptr(index) as usize,
            span,
            tick: now_ms.div_euclid(Self::TICK_MS),
        }
    }
}

/// `12,345`.
pub fn count(value: u64) -> String {
    crate::util::format_count(value)
}

/// "1 stream" or "12 streams".
pub fn streams(locale: Locale, streams: u32) -> String {
    let template = if streams == 1 {
        gettext(locale, "{count} stream")
    } else {
        gettext(locale, "{count} streams")
    };
    template.replace("{count}", &count(u64::from(streams)))
}

/// "12 min" or "1,234 min".
pub fn minutes(locale: Locale, minutes: u64) -> String {
    gettext(locale, "{minutes} min").replace("{minutes}", &count(minutes))
}

/// "42 streams · 120 min".
pub fn played(locale: Locale, played: Count) -> String {
    format!(
        "{} · {}",
        streams(locale, played.streams),
        minutes(locale, played.minutes())
    )
}

/// How a rank moved, as the page writes it beside the row: "2" up, "1"
/// down, "NEW", or nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Move {
    Up(String),
    Down(String),
    New(String),
    Still,
}

pub fn movement(locale: Locale, delta: Delta) -> Move {
    match delta {
        Delta::Up(places) => Move::Up(places.to_string()),
        Delta::Down(places) => Move::Down(places.to_string()),
        // Translators: A song or artist new to the person's top, shown as a small badge.
        Delta::New => Move::New(gettext(locale, "NEW")),
        Delta::Same | Delta::None => Move::Still,
    }
}

/// "Updated just now" or "Updated 5 min ago", from milliseconds.
pub fn updated(locale: Locale, ago_ms: i64) -> String {
    let minutes = ago_ms.max(0) / 60_000;
    if minutes < 1 {
        gettext(locale, "Updated just now")
    } else if minutes < 60 {
        gettext(locale, "Updated {minutes} min ago").replace("{minutes}", &minutes.to_string())
    } else {
        gettext(locale, "Updated {hours} h ago").replace("{hours}", &(minutes / 60).to_string())
    }
}

/// The per-day chart's bars: the days themselves while they fit, else runs
/// of whole weeks (or longer) so at most `most` bars remain. Each bar is
/// (first day, last day, total).
pub fn buckets(days: &[Day], most: usize) -> Vec<(jiff::civil::Date, jiff::civil::Date, Count)> {
    let most = most.max(1);
    let mut size = 1;
    while days.len().div_ceil(size) > most {
        size = if size == 1 { 7 } else { size + 7 };
    }
    days.chunks(size)
        .map(|chunk| {
            let mut total = Count::default();
            for day in chunk {
                total.streams += day.count.streams;
                total.ms += day.count.ms;
            }
            (chunk[0].date, chunk[chunk.len() - 1].date, total)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::metadata::{ArtistMeta, Metadata};
    use crate::stats::{ArtistName, PlayRecord, Source};

    const NOW: i64 = 1_700_000_000_000;
    const DAY: i64 = 24 * 60 * 60 * 1000;

    fn play(days_ago: i64, track: &str, artist: &str, ms: u32) -> PlayRecord {
        PlayRecord {
            at_ms: NOW - days_ago * DAY,
            ms_played: ms,
            estimated: false,
            source: Source::Extended,
            uri: Some(format!("spotify:track:{track}")),
            track: track.to_uppercase(),
            artists: vec![ArtistName {
                id: Some(artist.into()),
                name: artist.to_uppercase(),
            }],
            album: Some(format!("{artist} album")),
            album_id: Some(format!("{artist}-album")),
            duration_ms: None,
            skipped: None,
            shuffle: None,
        }
    }

    fn index(records: &[PlayRecord]) -> Index {
        let mut metadata = Metadata::default();
        metadata.artists.insert(
            "x".into(),
            ArtistMeta {
                name: "X".into(),
                genres: vec!["rock".into(), "pop".into()],
                image: Some("https://i.scdn.co/image/x".into()),
            },
        );
        let mut records = records.to_vec();
        records.sort_by_key(|record| record.at_ms);
        Index::build(&records, &metadata)
    }

    #[test]
    fn the_summary_reads_its_numbers_from_the_queries() {
        let index = index(&[
            play(1, "a", "x", 120_000),
            play(2, "a", "x", 60_000),
            play(3, "b", "y", 90_000),
            play(4, "b", "y", 1_000), // a skip
            play(40, "c", "y", 600_000),
        ]);
        let utc = jiff::tz::TimeZone::UTC;
        let summary = summarize(&index, Span::Weeks4, NOW, &utc);
        assert_eq!(summary.totals, query::totals(&index, query::Range::Weeks4, NOW));
        assert_eq!(summary.totals.streams, 3);
        assert!(summary.any_streams);
        assert_eq!(
            summary
                .tracks
                .iter()
                .map(|track| (track.name.as_str(), track.count.streams))
                .collect::<Vec<_>>(),
            vec![("A", 2), ("B", 1)]
        );
        assert_eq!(summary.tracks[0].uri.as_deref(), Some("spotify:track:a"));
        assert_eq!(summary.tracks[0].artists[0].id.as_deref(), Some("x"));
        assert_eq!(summary.tracks[0].album_id.as_deref(), Some("x-album"));
        assert_eq!(summary.artists[0].name, "X");
        assert_eq!(summary.hero_image(), Some("https://i.scdn.co/image/x"));
        assert_eq!(summary.albums.len(), 2);
        // x is rock and pop; y has no genres.
        assert_eq!(
            summary
                .genres
                .iter()
                .map(|genre| (genre.name.as_str(), genre.share))
                .collect::<Vec<_>>(),
            vec![("pop", 1.0), ("rock", 1.0)]
        );
        assert!((28..=29).contains(&summary.days.len()));
        assert_eq!(summary.hours.iter().map(|hour| hour.streams).sum::<u32>(), 3);

        let all = summarize(&index, Span::AllTime, NOW, &utc);
        assert_eq!(all.totals.streams, 4);
        assert!(all.tracks.iter().all(|track| track.delta == Delta::None));
    }

    #[test]
    fn an_empty_log_summarizes_to_nothing() {
        let summary = summarize(&Index::default(), Span::Months6, NOW, &jiff::tz::TimeZone::UTC);
        assert!(!summary.any_streams);
        assert_eq!(summary.totals, Totals::default());
        assert!(summary.tracks.is_empty() && summary.artists.is_empty());
        assert_eq!(summary.hero_image(), None);
    }

    #[test]
    fn a_double_click_plays_the_tracks_that_have_a_uri() {
        let index = index(&[play(1, "a", "x", 60_000), play(2, "b", "y", 60_000)]);
        let mut summary = summarize(&index, Span::Weeks4, NOW, &jiff::tz::TimeZone::UTC);
        summary.tracks[0].uri = None;
        let (uris, positions) = summary.playable();
        assert_eq!(uris, vec!["spotify:track:b".to_owned()]);
        assert_eq!(positions, vec![None, Some(0)]);
    }

    #[test]
    fn figures_are_written_as_spotify_writes_them() {
        let locale = Locale::default();
        assert_eq!(count(1_234_567), "1,234,567");
        assert_eq!(streams(locale, 1), "1 stream");
        assert_eq!(streams(locale, 1_200), "1,200 streams");
        assert_eq!(minutes(locale, 12_345), "12,345 min");
        assert_eq!(
            played(locale, Count { streams: 3, ms: 185_000 }),
            "3 streams · 3 min"
        );
        assert_eq!(movement(locale, Delta::Up(2)), Move::Up("2".into()));
        assert_eq!(movement(locale, Delta::Down(1)), Move::Down("1".into()));
        assert_eq!(movement(locale, Delta::New), Move::New("NEW".into()));
        assert_eq!(movement(locale, Delta::Same), Move::Still);
        assert_eq!(movement(locale, Delta::None), Move::Still);
        assert_eq!(updated(locale, 20_000), "Updated just now");
        assert_eq!(updated(locale, 5 * 60_000), "Updated 5 min ago");
        assert_eq!(updated(locale, 3 * 60 * 60_000), "Updated 3 h ago");
        assert_eq!(Span::Months12.label(locale), "Last 12 months");
        assert_eq!(Span::AllTime.chip(locale), "All time");
    }

    #[test]
    fn long_ranges_draw_weeks_instead_of_days() {
        let start = jiff::civil::date(2024, 1, 1);
        let days: Vec<Day> = (0..365)
            .map(|offset| Day {
                date: start.checked_add(jiff::Span::new().days(offset)).unwrap(),
                count: Count {
                    streams: 1,
                    ms: 60_000,
                },
            })
            .collect();
        let short = buckets(&days[..28], 120);
        assert_eq!(short.len(), 28);
        assert_eq!(short[3].0, short[3].1);
        let weeks = buckets(&days, 120);
        assert_eq!(weeks.len(), 53);
        assert_eq!(weeks[0].2.streams, 7);
        assert_eq!(weeks[0].1, jiff::civil::date(2024, 1, 7));
        assert_eq!(weeks.iter().map(|bar| bar.2.streams).sum::<u32>(), 365);
        let years: Vec<Day> = (0..3000).map(|offset| days[offset % 365]).collect();
        assert!(buckets(&years, 120).len() <= 120);
        assert!(buckets(&[], 120).is_empty());
    }

    #[test]
    fn keys_change_with_the_index_the_range_and_the_ten_minutes() {
        let index = Arc::new(Index::default());
        let key = Key::new(&index, Span::Weeks4, NOW);
        assert_eq!(key, Key::new(&index, Span::Weeks4, NOW + 1));
        assert_ne!(key, Key::new(&index, Span::AllTime, NOW));
        assert_ne!(key, Key::new(&index, Span::Weeks4, NOW + Key::TICK_MS));
        assert_ne!(key, Key::new(&Arc::new(Index::default()), Span::Weeks4, NOW));
    }
}
