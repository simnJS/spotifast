//! The server's statistics routes, as API mode reads them.
//!
//! While a statistics server is connected, every figure Spotifast shows
//! comes from it: one range's totals, tops, days and hours for the Stats
//! page and its share card ([`RangeStats`]), and one song's, artist's or
//! album's figures for their pages and the Now Playing panel
//! ([`ItemFigures`]). The backend fetches them ([`super::client::Client`]);
//! the interface keeps each answer per range and per item ([`Answers`]),
//! shows it until the next refresh has asked again, and never reads the
//! log kept on this computer meanwhile.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Deserializer};

use super::client::Error;

/// As many of each top as the Stats page shows.
pub const ARTISTS: u32 = 10;
pub const TRACKS: u32 = 50;
pub const ALBUMS: u32 = 10;
pub const GENRES: u32 = 10;
/// How many items keep their answers before the oldest are dropped.
const ITEMS_KEPT: usize = 64;

/// The server's time zone parameter for this computer: the system zone's
/// IANA name, or UTC when the system names none.
pub fn zone() -> String {
    jiff::tz::TimeZone::system()
        .iana_name()
        .unwrap_or("UTC")
        .to_owned()
}

/// A period of the Stats page, as the server's `range`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Period {
    Weeks4,
    Months6,
    Year,
    Lifetime,
}

impl Period {
    pub fn param(self) -> &'static str {
        match self {
            Self::Weeks4 => "4w",
            Self::Months6 => "6m",
            Self::Year => "1y",
            Self::Lifetime => "lifetime",
        }
    }
}

/// What an item's figures are about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Track,
    Artist,
    Album,
}

impl Kind {
    pub fn path(self) -> &'static str {
        match self {
            Self::Track => "track",
            Self::Artist => "artist",
            Self::Album => "album",
        }
    }
}

/// One song, artist or album, by its Spotify ID.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Item {
    pub kind: Kind,
    pub id: String,
}

/// What the interface asks the backend to fetch.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Fetch {
    /// One range of the Stats page.
    Range(Period),
    /// One item's figures. For a song, `artist` is its main artist's
    /// Spotify ID, whose all-time rank comes along.
    Item { item: Item, artist: Option<String> },
}

/// What the server answered to a [`Fetch`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    Range(Arc<RangeStats>),
    Item(Arc<ItemFigures>),
}

/// A time in an answer: milliseconds since the Unix epoch, or an RFC 3339
/// timestamp. Anything else reads as unknown.
fn instant<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<i64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Millis(i64),
        Text(String),
        Other(serde_json::Value),
    }
    Ok(match Option::<Raw>::deserialize(deserializer)? {
        Some(Raw::Millis(ms)) => Some(ms),
        Some(Raw::Text(text)) => text
            .parse::<jiff::Timestamp>()
            .ok()
            .map(jiff::Timestamp::as_millisecond),
        Some(Raw::Other(_)) | None => None,
    })
}

/// A list the server answers: `{"items": [...]}`, as the existing routes
/// do, or a bare array.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum List<T> {
    Items { items: Vec<T> },
    Bare(Vec<T>),
}

impl<T> List<T> {
    pub fn into_vec(self) -> Vec<T> {
        match self {
            Self::Items { items } | Self::Bare(items) => items,
        }
    }
}

/// `GET /stats/summary`: a range's totals.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Totals {
    #[serde(default)]
    pub plays: u64,
    #[serde(default)]
    pub ms_played: u64,
    #[serde(default)]
    pub distinct_tracks: u64,
    #[serde(default)]
    pub distinct_artists: u64,
    #[serde(default)]
    pub distinct_albums: u64,
    #[serde(default, deserialize_with = "instant")]
    pub first_played_at: Option<i64>,
    #[serde(default, deserialize_with = "instant")]
    pub last_played_at: Option<i64>,
}

/// An artist as a song or an album credits it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Credit {
    #[serde(default)]
    pub spotify_id: Option<String>,
    #[serde(default)]
    pub name: String,
}

/// A song's album.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct AlbumRef {
    #[serde(default)]
    pub spotify_id: Option<String>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub image_url: Option<String>,
}

/// One row of `GET /stats/top/tracks`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct TopTrack {
    pub rank: u32,
    /// Its rank over the period of the same length just before; `None`
    /// when it was not ranked then, or for the lifetime top.
    #[serde(default)]
    pub previous_rank: Option<u32>,
    pub plays: u64,
    #[serde(default)]
    pub ms_played: u64,
    #[serde(default)]
    pub spotify_id: Option<String>,
    #[serde(default)]
    pub uri: Option<String>,
    pub name: String,
    #[serde(default)]
    pub image_url: Option<String>,
    #[serde(default)]
    pub artists: Vec<Credit>,
    #[serde(default)]
    pub album: Option<AlbumRef>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

/// One row of `GET /stats/top/artists`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct TopArtist {
    pub rank: u32,
    #[serde(default)]
    pub previous_rank: Option<u32>,
    pub plays: u64,
    #[serde(default)]
    pub ms_played: u64,
    #[serde(default)]
    pub spotify_id: Option<String>,
    #[serde(default)]
    pub uri: Option<String>,
    pub name: String,
    #[serde(default)]
    pub image_url: Option<String>,
}

/// One row of `GET /stats/top/albums`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct TopAlbum {
    pub rank: u32,
    #[serde(default)]
    pub previous_rank: Option<u32>,
    pub plays: u64,
    #[serde(default)]
    pub ms_played: u64,
    #[serde(default)]
    pub spotify_id: Option<String>,
    #[serde(default)]
    pub uri: Option<String>,
    pub name: String,
    #[serde(default)]
    pub image_url: Option<String>,
    #[serde(default)]
    pub artists: Vec<Credit>,
}

/// One row of `GET /stats/top/genres`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct TopGenre {
    pub genre: String,
    pub rank: u32,
    #[serde(default)]
    pub previous_rank: Option<u32>,
    pub plays: u64,
    #[serde(default)]
    pub ms_played: u64,
}

/// One day of `GET /stats/listening-time?bucket=day`: its start, and the
/// plays in it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct DayBucket {
    #[serde(default, deserialize_with = "instant")]
    pub start: Option<i64>,
    /// The day itself, `YYYY-MM-DD`, when the server names it.
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub plays: u64,
    #[serde(default)]
    pub ms_played: u64,
}

impl DayBucket {
    /// The day it covers in `zone`. The server starts a day at midnight in
    /// the zone asked for; an older server, at midnight UTC. The day the
    /// start's noon falls on is the right one either way, daylight saving
    /// time included.
    pub fn day(&self, zone: &jiff::tz::TimeZone) -> Option<jiff::civil::Date> {
        if let Some(date) = self
            .date
            .as_deref()
            .and_then(|date| date.parse::<jiff::civil::Date>().ok())
        {
            return Some(date);
        }
        let noon = self.start?.checked_add(12 * 60 * 60 * 1000)?;
        jiff::Timestamp::from_millisecond(noon)
            .ok()
            .map(|at| at.to_zoned(zone.clone()).date())
    }
}

/// One hour of `GET /stats/hours`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct HourBucket {
    pub hour: u8,
    #[serde(default)]
    pub plays: u64,
    #[serde(default)]
    pub ms_played: u64,
}

/// `GET /stats/hours`: the 24 hours of the day (and the weekdays, which
/// Spotifast does not show).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Hours {
    #[serde(default)]
    pub hours: Vec<HourBucket>,
}

/// Everything the Stats page shows for one range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RangeStats {
    pub period: Period,
    pub totals: Totals,
    /// The plays the server holds ever, which tells an account with
    /// nothing played from a range with nothing in it.
    pub lifetime_plays: u64,
    pub artists: Vec<TopArtist>,
    pub tracks: Vec<TopTrack>,
    pub albums: Vec<TopAlbum>,
    pub genres: Vec<TopGenre>,
    /// The days with plays, oldest first.
    pub days: Vec<DayBucket>,
    pub hours: Vec<HourBucket>,
    /// The time zone of `days` and `hours`, an IANA name.
    pub zone: String,
}

/// One of an artist's or an album's most played songs. Right after an
/// import the server may not know its ID yet: it cannot be played then.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ItemTrack {
    #[serde(default)]
    pub spotify_id: Option<String>,
    pub name: String,
    pub plays: u64,
    #[serde(default)]
    pub ms_played: u64,
}

/// `GET /stats/item/{kind}/{spotify_id}`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct ItemStats {
    pub plays: u64,
    #[serde(default)]
    pub ms_played: u64,
    #[serde(default, deserialize_with = "instant")]
    pub first_played_at: Option<i64>,
    #[serde(default, deserialize_with = "instant")]
    pub last_played_at: Option<i64>,
    /// Its rank among its kind, all time and over four weeks; `None`
    /// when it is not in that top.
    #[serde(default)]
    pub rank_lifetime: Option<u32>,
    #[serde(default)]
    pub rank_4w: Option<u32>,
    /// For an artist or an album: its most played songs.
    #[serde(default)]
    pub top_tracks: Vec<ItemTrack>,
}

/// An item's figures as a page shows them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemFigures {
    /// `None` when the server has no play of it (HTTP 404).
    pub stats: Option<ItemStats>,
    /// For a song: its main artist's all-time rank.
    pub artist_rank: Option<u32>,
}

/// One answer as the interface keeps it: the last that came, shown until
/// another replaces it, and why the last fetch failed, if it did.
#[derive(Clone, Debug)]
pub struct Slot<T> {
    /// The last answer that came, and when, in milliseconds.
    pub value: Option<(Arc<T>, i64)>,
    /// Why the last fetch failed, until one goes through.
    pub error: Option<Error>,
    /// A fetch is on its way.
    pub fetching: bool,
    /// The round of the last fetch asked ([`Answers::refresh`]).
    asked: Option<u64>,
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self {
            value: None,
            error: None,
            fetching: false,
            asked: None,
        }
    }
}

impl<T> Slot<T> {
    pub fn get(&self) -> Option<&Arc<T>> {
        self.value.as_ref().map(|(value, _)| value)
    }

    fn receive(&mut self, result: Result<Arc<T>, Error>, at_ms: i64) {
        self.fetching = false;
        match result {
            Ok(value) => {
                self.value = Some((value, at_ms));
                self.error = None;
            }
            // Figures of another account are never shown, not even the
            // ones that came before.
            Err(Error::OtherAccount) => {
                self.value = None;
                self.error = Some(Error::OtherAccount);
            }
            // A failed refresh keeps what is shown.
            Err(error) => self.error = Some(error),
        }
    }
}

/// The server's answers, per range and per item, for one connection: a
/// new connection starts with none.
#[derive(Clone, Debug, Default)]
pub struct Answers {
    /// Bumped by each refresh. An answer asked in an earlier round is
    /// asked again the next time a page shows it, and stays shown
    /// meanwhile.
    round: u64,
    ranges: HashMap<Period, Slot<RangeStats>>,
    items: HashMap<Item, Slot<ItemFigures>>,
    /// The items held, oldest first, for dropping.
    order: Vec<Item>,
}

impl Answers {
    pub fn range(&self, period: Period) -> Option<&Slot<RangeStats>> {
        self.ranges.get(&period)
    }

    pub fn item(&self, item: &Item) -> Option<&Slot<ItemFigures>> {
        self.items.get(item)
    }

    fn slot_asked(&self, fetch: &Fetch) -> Option<(bool, Option<u64>)> {
        match fetch {
            Fetch::Range(period) => self
                .ranges
                .get(period)
                .map(|slot| (slot.fetching, slot.asked)),
            Fetch::Item { item, .. } => {
                self.items.get(item).map(|slot| (slot.fetching, slot.asked))
            }
        }
    }

    /// Whether `fetch` must be asked for: never asked, or not since the
    /// last refresh, and none on its way.
    pub fn wants(&self, fetch: &Fetch) -> bool {
        match self.slot_asked(fetch) {
            None => true,
            Some((fetching, asked)) => !fetching && asked != Some(self.round),
        }
    }

    /// Notes that `fetch` is on its way.
    pub fn fetching(&mut self, fetch: &Fetch) {
        let round = self.round;
        match fetch {
            Fetch::Range(period) => {
                let slot = self.ranges.entry(*period).or_default();
                slot.fetching = true;
                slot.asked = Some(round);
            }
            Fetch::Item { item, .. } => {
                if !self.items.contains_key(item) {
                    if self.order.len() >= ITEMS_KEPT {
                        let oldest = self.order.remove(0);
                        self.items.remove(&oldest);
                    }
                    self.order.push(item.clone());
                }
                let slot = self.items.entry(item.clone()).or_default();
                slot.fetching = true;
                slot.asked = Some(round);
            }
        }
    }

    /// Takes in the answer to `fetch`, which came at `at_ms`. An answer of
    /// the wrong shape, or for an item dropped meanwhile, changes nothing.
    pub fn receive(&mut self, fetch: &Fetch, result: Result<Answer, Error>, at_ms: i64) {
        match fetch {
            Fetch::Range(period) => {
                let result = match result {
                    Ok(Answer::Range(stats)) => Ok(stats),
                    Ok(Answer::Item(_)) => return,
                    Err(error) => Err(error),
                };
                self.ranges
                    .entry(*period)
                    .or_default()
                    .receive(result, at_ms);
            }
            Fetch::Item { item, .. } => {
                let result = match result {
                    Ok(Answer::Item(figures)) => Ok(figures),
                    Ok(Answer::Range(_)) => return,
                    Err(error) => Err(error),
                };
                if let Some(slot) = self.items.get_mut(item) {
                    slot.receive(result, at_ms);
                }
            }
        }
    }

    /// A refresh: every answer is asked again when next shown.
    pub fn refresh(&mut self) {
        self.round += 1;
    }

    /// Whether any answer is on its way.
    pub fn fetching_any(&self) -> bool {
        self.ranges.values().any(|slot| slot.fetching)
            || self.items.values().any(|slot| slot.fetching)
    }
}

/// A range's answers as the server gives them, for the tests here and
/// the Stats page's.
#[cfg(test)]
pub(crate) mod samples {
    use super::*;

    pub const SUMMARY: &str = r#"{"plays": 72, "ms_played": 15000000, "distinct_tracks": 3,
        "distinct_artists": 2, "distinct_albums": 2,
        "first_played_at": 1699000000000, "last_played_at": 1699990000000}"#;

    /// Right after an import the server may not know an item's ID, URI or
    /// image yet: the second song and the second album.
    pub const TOP_TRACKS: &str = r#"{"items": [
        {"rank": 1, "previous_rank": 3, "plays": 42, "ms_played": 9000000,
         "spotify_id": "t1", "uri": "spotify:track:t1", "name": "First",
         "image_url": "https://i.scdn.co/image/a1",
         "artists": [{"spotify_id": "a1", "name": "Band"}, {"spotify_id": null, "name": "Guest"}],
         "album": {"spotify_id": "al1", "name": "Album", "image_url": "https://i.scdn.co/image/a1"},
         "duration_ms": 214000},
        {"rank": 2, "previous_rank": null, "plays": 30, "ms_played": 6000000,
         "spotify_id": null, "uri": null, "name": "Second",
         "image_url": null, "artists": [{"spotify_id": null, "name": "Solo"}],
         "album": null, "duration_ms": null}
    ]}"#;

    pub const TOP_ARTISTS: &str = r#"{"items": [
        {"rank": 1, "previous_rank": 1, "plays": 42, "ms_played": 9000000,
         "spotify_id": "a1", "uri": "spotify:artist:a1", "name": "Band",
         "image_url": "https://i.scdn.co/image/b"},
        {"rank": 2, "previous_rank": 1, "plays": 30, "ms_played": 6000000,
         "spotify_id": null, "uri": null, "name": "Solo", "image_url": null}
    ]}"#;

    pub const TOP_ALBUMS: &str = r#"{"items": [
        {"rank": 1, "previous_rank": null, "plays": 42, "ms_played": 9000000,
         "spotify_id": "al1", "uri": "spotify:album:al1", "name": "Album",
         "image_url": "https://i.scdn.co/image/a1",
         "artists": [{"spotify_id": "a1", "name": "Band"}]},
        {"rank": 2, "previous_rank": 4, "plays": 30, "ms_played": 6000000,
         "spotify_id": null, "uri": null, "name": "Unknown album",
         "image_url": null, "artists": []}
    ]}"#;

    pub const TOP_GENRES: &str = r#"{"items": [
        {"genre": "indie", "rank": 1, "previous_rank": null, "plays": 42, "ms_played": 9000000},
        {"genre": "pop", "rank": 2, "previous_rank": 1, "plays": 21, "ms_played": 4000000}
    ]}"#;

    /// Two days in UTC, two days apart.
    pub const DAYS: &str = r#"{"items": [
        {"start": 1699833600000, "plays": 40, "ms_played": 8000000},
        {"start": 1700006400000, "plays": 32, "ms_played": 7000000}
    ]}"#;

    pub const HOURS: &str = r#"{"hours": [
        {"hour": 0, "plays": 0, "ms_played": 0}, {"hour": 1, "plays": 0, "ms_played": 0},
        {"hour": 2, "plays": 0, "ms_played": 0}, {"hour": 3, "plays": 0, "ms_played": 0},
        {"hour": 4, "plays": 0, "ms_played": 0}, {"hour": 5, "plays": 0, "ms_played": 0},
        {"hour": 6, "plays": 0, "ms_played": 0}, {"hour": 7, "plays": 2, "ms_played": 400000},
        {"hour": 8, "plays": 0, "ms_played": 0}, {"hour": 9, "plays": 0, "ms_played": 0},
        {"hour": 10, "plays": 0, "ms_played": 0}, {"hour": 11, "plays": 0, "ms_played": 0},
        {"hour": 12, "plays": 0, "ms_played": 0}, {"hour": 13, "plays": 0, "ms_played": 0},
        {"hour": 14, "plays": 0, "ms_played": 0}, {"hour": 15, "plays": 0, "ms_played": 0},
        {"hour": 16, "plays": 0, "ms_played": 0}, {"hour": 17, "plays": 0, "ms_played": 0},
        {"hour": 18, "plays": 0, "ms_played": 0}, {"hour": 19, "plays": 0, "ms_played": 0},
        {"hour": 20, "plays": 0, "ms_played": 0}, {"hour": 21, "plays": 70, "ms_played": 14600000},
        {"hour": 22, "plays": 0, "ms_played": 0}, {"hour": 23, "plays": 0, "ms_played": 0}
    ], "weekdays": [{"weekday": 1, "plays": 72, "ms_played": 15000000}]}"#;

    fn list<T: serde::de::DeserializeOwned>(body: &str) -> Vec<T> {
        serde_json::from_str::<List<T>>(body).unwrap().into_vec()
    }

    /// The range those answers make, as [`super::super::client::Client::range`]
    /// puts it together.
    pub fn range(period: Period) -> RangeStats {
        let totals: Totals = serde_json::from_str(SUMMARY).unwrap();
        RangeStats {
            period,
            lifetime_plays: totals.plays,
            totals,
            artists: list(TOP_ARTISTS),
            tracks: list(TOP_TRACKS),
            albums: list(TOP_ALBUMS),
            genres: list(TOP_GENRES),
            days: list(DAYS),
            hours: serde_json::from_str::<Hours>(HOURS).unwrap().hours,
            zone: "UTC".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::samples::TOP_TRACKS;
    use super::*;

    #[test]
    fn a_whole_range_reads_from_the_samples() {
        let range = samples::range(Period::Months6);
        assert_eq!(range.totals.plays, 72);
        assert_eq!(range.artists.len(), 2);
        assert_eq!(range.hours.len(), 24);
        assert_eq!(range.days.len(), 2);
        assert_eq!(range.genres[1].previous_rank, Some(1));
    }

    #[test]
    fn tops_read_every_field_and_nulls_as_unknown() {
        let tracks = serde_json::from_str::<List<TopTrack>>(TOP_TRACKS)
            .unwrap()
            .into_vec();
        assert_eq!(tracks.len(), 2);
        let first = &tracks[0];
        assert_eq!((first.rank, first.previous_rank), (1, Some(3)));
        assert_eq!((first.plays, first.ms_played), (42, 9_000_000));
        assert_eq!(first.uri.as_deref(), Some("spotify:track:t1"));
        assert_eq!(first.artists[1].spotify_id, None);
        assert_eq!(first.artists[1].name, "Guest");
        let album = first.album.as_ref().unwrap();
        assert_eq!(album.spotify_id.as_deref(), Some("al1"));
        assert_eq!(first.duration_ms, Some(214_000));
        let second = &tracks[1];
        assert_eq!(second.previous_rank, None);
        assert_eq!(
            (second.spotify_id.as_deref(), second.uri.as_deref()),
            (None, None)
        );
        assert_eq!(second.image_url, None);
        assert_eq!(second.album, None);
        assert_eq!(second.duration_ms, None);

        let artists: Vec<TopArtist> = serde_json::from_str::<List<TopArtist>>(
            r#"{"items": [{"rank": 1, "previous_rank": 1, "plays": 80, "ms_played": 1,
                "spotify_id": "a1", "uri": "spotify:artist:a1", "name": "Band",
                "image_url": "https://i.scdn.co/image/b"}]}"#,
        )
        .unwrap()
        .into_vec();
        assert_eq!(artists[0].spotify_id.as_deref(), Some("a1"));
        assert_eq!(
            artists[0].image_url.as_deref(),
            Some("https://i.scdn.co/image/b")
        );

        let albums = serde_json::from_str::<List<TopAlbum>>(
            r#"{"items": [{"rank": 1, "previous_rank": 2, "plays": 9, "ms_played": 5,
                "spotify_id": "al1", "uri": "spotify:album:al1", "name": "Album",
                "image_url": null, "artists": [{"spotify_id": "a1", "name": "Band"}]}]}"#,
        )
        .unwrap()
        .into_vec();
        assert_eq!(albums[0].artists[0].name, "Band");

        // Genres as a bare array, or under "items".
        for body in [
            r#"[{"genre": "indie", "rank": 1, "previous_rank": null, "plays": 7, "ms_played": 70}]"#,
            r#"{"items": [{"genre": "indie", "rank": 1, "plays": 7, "ms_played": 70}]}"#,
        ] {
            let genres = serde_json::from_str::<List<TopGenre>>(body)
                .unwrap()
                .into_vec();
            assert_eq!(genres[0].genre, "indie");
            assert_eq!(genres[0].previous_rank, None);
        }
    }

    #[test]
    fn totals_days_and_hours_read_the_servers_answers() {
        let totals: Totals = serde_json::from_str(
            r#"{"plays": 1200, "ms_played": 250000000, "distinct_tracks": 300,
                "distinct_artists": 80, "distinct_albums": 120,
                "first_played_at": 1600000000000, "last_played_at": "2023-11-14T22:13:20Z"}"#,
        )
        .unwrap();
        assert_eq!(totals.plays, 1200);
        assert_eq!(totals.distinct_albums, 120);
        assert_eq!(totals.first_played_at, Some(1_600_000_000_000));
        assert_eq!(totals.last_played_at, Some(1_700_000_000_000));
        let empty: Totals = serde_json::from_str(
            r#"{"plays": 0, "ms_played": 0, "distinct_tracks": 0, "distinct_artists": 0,
                "distinct_albums": 0, "first_played_at": null, "last_played_at": null}"#,
        )
        .unwrap();
        assert_eq!(empty, Totals::default());

        let paris = jiff::tz::TimeZone::get("Europe/Paris").unwrap();
        let days = serde_json::from_str::<List<DayBucket>>(
            // Midnight in Paris on 14 November 2023, then midnight UTC on
            // the 15th (an older server), then a named day.
            r#"{"items": [
                {"start": 1699916400000, "plays": 3, "ms_played": 600000},
                {"start": 1700006400000, "plays": 1, "ms_played": 200000},
                {"date": "2023-11-17", "plays": 2, "ms_played": 1}
            ]}"#,
        )
        .unwrap()
        .into_vec();
        let named: Vec<String> = days
            .iter()
            .map(|day| day.day(&paris).unwrap().to_string())
            .collect();
        assert_eq!(named, ["2023-11-14", "2023-11-15", "2023-11-17"]);
        // West of UTC, midnight UTC is still the right day.
        let new_york = jiff::tz::TimeZone::get("America/New_York").unwrap();
        assert_eq!(days[1].day(&new_york).unwrap().to_string(), "2023-11-15");

        let hours: Hours = serde_json::from_str(
            r#"{"hours": [{"hour": 0, "plays": 1, "ms_played": 2}, {"hour": 23, "plays": 4, "ms_played": 5}],
                "weekdays": [{"weekday": 1, "plays": 5, "ms_played": 7}]}"#,
        )
        .unwrap();
        assert_eq!(hours.hours[1].hour, 23);
        assert_eq!(hours.hours[1].plays, 4);
    }

    #[test]
    fn an_items_figures_read_with_and_without_ranks() {
        let artist: ItemStats = serde_json::from_str(
            r#"{"plays": 120, "ms_played": 25000000, "first_played_at": 1500000000000,
                "last_played_at": 1700000000000, "rank_lifetime": 3, "rank_4w": null,
                "top_tracks": [{"spotify_id": "t1", "name": "First", "plays": 40, "ms_played": 9000000},
                               {"spotify_id": null, "name": "Imported", "plays": 2, "ms_played": 1}]}"#,
        )
        .unwrap();
        assert_eq!(artist.plays, 120);
        assert_eq!(artist.first_played_at, Some(1_500_000_000_000));
        assert_eq!((artist.rank_lifetime, artist.rank_4w), (Some(3), None));
        assert_eq!(artist.top_tracks[0].spotify_id.as_deref(), Some("t1"));
        // Not identified yet, right after an import.
        assert_eq!(artist.top_tracks[1].spotify_id, None);
        let track: ItemStats = serde_json::from_str(r#"{"plays": 2, "ms_played": 1}"#).unwrap();
        assert!(track.top_tracks.is_empty());
        assert_eq!(track.rank_lifetime, None);
    }

    fn range(period: Period) -> Arc<RangeStats> {
        Arc::new(RangeStats {
            period,
            totals: Totals::default(),
            lifetime_plays: 0,
            artists: Vec::new(),
            tracks: Vec::new(),
            albums: Vec::new(),
            genres: Vec::new(),
            days: Vec::new(),
            hours: Vec::new(),
            zone: "UTC".into(),
        })
    }

    #[test]
    fn answers_are_asked_once_per_refresh_and_stay_shown_through_failures() {
        let mut answers = Answers::default();
        let weeks = Fetch::Range(Period::Weeks4);
        assert!(answers.wants(&weeks));
        answers.fetching(&weeks);
        // On its way: not asked twice.
        assert!(!answers.wants(&weeks));
        assert!(answers.fetching_any());
        let first = range(Period::Weeks4);
        answers.receive(&weeks, Ok(Answer::Range(Arc::clone(&first))), 10);
        assert!(!answers.wants(&weeks));
        let slot = answers.range(Period::Weeks4).unwrap();
        assert!(Arc::ptr_eq(slot.get().unwrap(), &first));
        assert_eq!(slot.value.as_ref().unwrap().1, 10);

        // A refresh asks again; the answer shown stays until another comes,
        // and through a failure.
        answers.refresh();
        assert!(answers.wants(&weeks));
        answers.fetching(&weeks);
        answers.receive(&weeks, Err(Error::Network("offline".into())), 20);
        let slot = answers.range(Period::Weeks4).unwrap();
        assert!(Arc::ptr_eq(slot.get().unwrap(), &first));
        assert_eq!(slot.error, Some(Error::Network("offline".into())));
        // A failure is not asked again before the next refresh.
        assert!(!answers.wants(&weeks));
        answers.refresh();
        answers.fetching(&weeks);
        answers.receive(&weeks, Ok(Answer::Range(range(Period::Weeks4))), 30);
        assert_eq!(answers.range(Period::Weeks4).unwrap().error, None);

        // Another account's figures never show, not even earlier ones.
        answers.refresh();
        answers.fetching(&weeks);
        answers.receive(&weeks, Err(Error::OtherAccount), 40);
        assert!(answers.range(Period::Weeks4).unwrap().get().is_none());
        assert!(!answers.fetching_any());
    }

    #[test]
    fn items_are_kept_per_item_and_the_oldest_dropped() {
        let mut answers = Answers::default();
        let fetch = |id: usize| Fetch::Item {
            item: Item {
                kind: Kind::Artist,
                id: format!("a{id}"),
            },
            artist: None,
        };
        for id in 0..ITEMS_KEPT + 1 {
            answers.fetching(&fetch(id));
        }
        let first = Item {
            kind: Kind::Artist,
            id: "a0".into(),
        };
        assert!(answers.item(&first).is_none());
        // An answer for a dropped item is not kept.
        answers.receive(&fetch(0), Ok(Answer::Item(Arc::default())), 1);
        assert!(answers.item(&first).is_none());
        let last = Item {
            kind: Kind::Artist,
            id: format!("a{ITEMS_KEPT}"),
        };
        let figures = Arc::new(ItemFigures {
            stats: None,
            artist_rank: None,
        });
        answers.receive(
            &fetch(ITEMS_KEPT),
            Ok(Answer::Item(Arc::clone(&figures))),
            2,
        );
        assert!(Arc::ptr_eq(
            answers.item(&last).unwrap().get().unwrap(),
            &figures
        ));
        // The answer of a range is not an item's.
        answers.receive(
            &fetch(ITEMS_KEPT),
            Ok(Answer::Range(range(Period::Lifetime))),
            3,
        );
        assert!(Arc::ptr_eq(
            answers.item(&last).unwrap().get().unwrap(),
            &figures
        ));
    }
}
