//! What the stats pages ask of an [`Index`]: pure functions of the plays,
//! a range and the current time.
//!
//! Only streams (plays of 30 seconds or more) count towards the totals,
//! the tops and the charts; skips are kept in the log but count for
//! nothing here, as on stats.fm. Minutes are the time streamed.

use std::collections::HashMap;

use super::index::{Index, Play};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// The period a query covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Range {
    /// The last 28 days.
    Weeks4,
    /// The last 182 days.
    Months6,
    /// The last 365 days.
    Months12,
    /// Every play.
    AllTime,
    /// From `start_ms` up to, not including, `end_ms`.
    Custom { start_ms: i64, end_ms: i64 },
}

impl Range {
    /// The bounds of the range at `now_ms`, start included, end excluded.
    pub fn bounds(self, now_ms: i64) -> (i64, i64) {
        let end = now_ms + 1;
        match self {
            Self::Weeks4 => (end - 28 * DAY_MS, end),
            Self::Months6 => (end - 182 * DAY_MS, end),
            Self::Months12 => (end - 365 * DAY_MS, end),
            Self::AllTime => (i64::MIN, end),
            Self::Custom { start_ms, end_ms } => (start_ms, end_ms.max(start_ms)),
        }
    }

    /// The period of the same length just before, for rank changes. All
    /// time has none.
    pub fn previous(self, now_ms: i64) -> Option<(i64, i64)> {
        if self == Self::AllTime {
            return None;
        }
        let (start, end) = self.bounds(now_ms);
        Some((start - (end - start), start))
    }
}

/// What a top or an item query is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Track,
    Artist,
    Album,
    Genre,
}

/// One track, artist, album or genre, by its index in the [`Index`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Item {
    Track(u32),
    Artist(u32),
    Album(u32),
    Genre(u32),
}

impl Item {
    pub fn kind(self) -> Kind {
        match self {
            Self::Track(_) => Kind::Track,
            Self::Artist(_) => Kind::Artist,
            Self::Album(_) => Kind::Album,
            Self::Genre(_) => Kind::Genre,
        }
    }
}

/// Streams and time streamed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Count {
    pub streams: u32,
    pub ms: u64,
}

impl Count {
    fn add(&mut self, play: &Play) {
        self.streams += 1;
        self.ms += u64::from(play.ms_played);
    }

    pub fn minutes(&self) -> u64 {
        self.ms / 60_000
    }
}

/// The range's figures.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub streams: u32,
    /// Plays shorter than a stream.
    pub skips: u32,
    pub ms: u64,
    pub tracks: usize,
    pub artists: usize,
    pub albums: usize,
}

impl Totals {
    pub fn minutes(&self) -> u64 {
        self.ms / 60_000
    }
}

/// How an item's rank moved since the period before.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delta {
    /// Not ranked in the period before.
    New,
    /// Up this many places.
    Up(u32),
    /// Down this many places.
    Down(u32),
    Same,
    /// No period before to compare with (all time).
    None,
}

/// One row of a top.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ranked {
    pub item: Item,
    /// 1 for the most streamed.
    pub rank: u32,
    pub count: Count,
    pub delta: Delta,
}

/// One day of the chart, in the time zone asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Day {
    pub date: jiff::civil::Date,
    pub count: Count,
}

/// One item's figures for a range.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ItemStats {
    pub count: Count,
    /// Its rank in the range's top of its kind, when streamed in it.
    pub rank: Option<u32>,
    /// Its first and last streams ever, whatever the range.
    pub first_ms: Option<i64>,
    pub last_ms: Option<i64>,
}

fn streams(index: &Index, (start, end): (i64, i64)) -> impl Iterator<Item = &Play> {
    index
        .plays_between(start, end)
        .iter()
        .filter(|play| play.is_stream())
}

/// The items of `kind` a play counts for: its track, each of its artists,
/// its album, or the genres of its artists (each once).
fn items_of(index: &Index, play: &Play, kind: Kind, out: &mut Vec<u32>) {
    out.clear();
    let track = &index.tracks[play.track as usize];
    match kind {
        Kind::Track => out.push(play.track),
        Kind::Artist => out.extend(&track.artists),
        Kind::Album => out.extend(track.album),
        Kind::Genre => {
            for &artist in &track.artists {
                for &genre in &index.artists[artist as usize].genres {
                    if !out.contains(&genre) {
                        out.push(genre);
                    }
                }
            }
        }
    }
}

fn item(kind: Kind, id: u32) -> Item {
    match kind {
        Kind::Track => Item::Track(id),
        Kind::Artist => Item::Artist(id),
        Kind::Album => Item::Album(id),
        Kind::Genre => Item::Genre(id),
    }
}

/// Every item of `kind` streamed in `bounds`, most streamed first: more
/// streams, then more time, then the one streamed first.
fn ranking(index: &Index, kind: Kind, bounds: (i64, i64)) -> Vec<(u32, Count)> {
    let mut counts: HashMap<u32, (Count, i64)> = HashMap::new();
    let mut found = Vec::new();
    for play in streams(index, bounds) {
        items_of(index, play, kind, &mut found);
        for &id in &found {
            counts
                .entry(id)
                .or_insert((Count::default(), play.at_ms))
                .0
                .add(play);
        }
    }
    let mut ranked: Vec<(u32, Count, i64)> = counts
        .into_iter()
        .map(|(id, (count, first))| (id, count, first))
        .collect();
    ranked.sort_by(|a, b| {
        b.1.streams
            .cmp(&a.1.streams)
            .then(b.1.ms.cmp(&a.1.ms))
            .then(a.2.cmp(&b.2))
            .then(a.0.cmp(&b.0))
    });
    ranked
        .into_iter()
        .map(|(id, count, _)| (id, count))
        .collect()
}

/// The range's streams, skips, minutes and distinct tracks, artists and
/// albums.
pub fn totals(index: &Index, range: Range, now_ms: i64) -> Totals {
    let (start, end) = range.bounds(now_ms);
    let mut totals = Totals::default();
    let mut tracks = std::collections::HashSet::new();
    let mut artists = std::collections::HashSet::new();
    let mut albums = std::collections::HashSet::new();
    for play in index.plays_between(start, end) {
        if !play.is_stream() {
            totals.skips += 1;
            continue;
        }
        totals.streams += 1;
        totals.ms += u64::from(play.ms_played);
        let track = &index.tracks[play.track as usize];
        tracks.insert(play.track);
        artists.extend(track.artists.iter().copied());
        albums.extend(track.album);
    }
    totals.tracks = tracks.len();
    totals.artists = artists.len();
    totals.albums = albums.len();
    totals
}

/// The `limit` most streamed items of `kind` in the range, each with how
/// its rank moved since the period of the same length before.
pub fn top(index: &Index, kind: Kind, range: Range, now_ms: i64, limit: usize) -> Vec<Ranked> {
    let current = ranking(index, kind, range.bounds(now_ms));
    let before: Option<HashMap<u32, u32>> = range.previous(now_ms).map(|bounds| {
        ranking(index, kind, bounds)
            .into_iter()
            .enumerate()
            .map(|(rank, (id, _))| (id, rank as u32 + 1))
            .collect()
    });
    current
        .into_iter()
        .take(limit)
        .enumerate()
        .map(|(rank, (id, count))| {
            let rank = rank as u32 + 1;
            let delta = match &before {
                None => Delta::None,
                Some(before) => match before.get(&id) {
                    None => Delta::New,
                    Some(&was) if was > rank => Delta::Up(was - rank),
                    Some(&was) if was < rank => Delta::Down(rank - was),
                    Some(_) => Delta::Same,
                },
            };
            Ranked {
                item: item(kind, id),
                rank,
                count,
                delta,
            }
        })
        .collect()
}

/// The streams of each day of the range in `zone`, oldest first, days
/// without any included. All time starts on the day of the first play.
pub fn per_day(index: &Index, range: Range, now_ms: i64, zone: &jiff::tz::TimeZone) -> Vec<Day> {
    let (start, end) = range.bounds(now_ms);
    // All time starts with the first play; a fixed range shows each of
    // its days, played or not.
    let start = if range == Range::AllTime {
        index.first_play_ms().unwrap_or(end - 1)
    } else {
        start
    };
    let date = |ms: i64| {
        jiff::Timestamp::from_millisecond(ms)
            .map(|at| at.to_zoned(zone.clone()).date())
            .ok()
    };
    let (Some(first), Some(last)) = (date(start), date(end - 1)) else {
        return Vec::new();
    };
    let mut days = Vec::new();
    let mut day = first;
    while day <= last {
        days.push(Day {
            date: day,
            count: Count::default(),
        });
        match day.tomorrow() {
            Ok(next) => day = next,
            Err(_) => break,
        }
    }
    for play in streams(index, (start, end)) {
        if let Some(on) = date(play.at_ms)
            && let Ok(offset) = usize::try_from((on - first).get_days())
            && let Some(day) = days.get_mut(offset)
        {
            day.count.add(play);
        }
    }
    days
}

/// The streams of each hour of the day (0 to 23) in `zone` over the range.
pub fn per_hour(
    index: &Index,
    range: Range,
    now_ms: i64,
    zone: &jiff::tz::TimeZone,
) -> [Count; 24] {
    let mut hours = [Count::default(); 24];
    for play in streams(index, range.bounds(now_ms)) {
        if let Ok(at) = jiff::Timestamp::from_millisecond(play.at_ms) {
            let hour = at.to_zoned(zone.clone()).hour();
            hours[hour as usize].add(play);
        }
    }
    hours
}

/// One item's streams, minutes and rank in the range, and its first and
/// last streams ever.
pub fn item_stats(index: &Index, which: Item, range: Range, now_ms: i64) -> ItemStats {
    let kind = which.kind();
    let id = match which {
        Item::Track(id) | Item::Artist(id) | Item::Album(id) | Item::Genre(id) => id,
    };
    let mut stats = ItemStats::default();
    let mut found = Vec::new();
    for play in index.plays.iter().filter(|play| play.is_stream()) {
        items_of(index, play, kind, &mut found);
        if found.contains(&id) {
            stats.first_ms.get_or_insert(play.at_ms);
            stats.last_ms = Some(play.at_ms);
        }
    }
    let ranked = ranking(index, kind, range.bounds(now_ms));
    if let Some(position) = ranked.iter().position(|(found, _)| *found == id) {
        stats.rank = Some(position as u32 + 1);
        stats.count = ranked[position].1;
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::metadata::{ArtistMeta, Metadata};
    use crate::stats::{ArtistName, PlayRecord, Source};

    const NOW: i64 = 1_700_000_000_000;
    const HOUR: i64 = 60 * 60 * 1000;

    fn play(days_ago: i64, track: &str, artist: &str, ms: u32) -> PlayRecord {
        PlayRecord {
            at_ms: NOW - days_ago * DAY_MS,
            ms_played: ms,
            estimated: false,
            source: Source::Extended,
            uri: Some(format!("spotify:track:{track}")),
            track: track.into(),
            artists: vec![ArtistName {
                id: Some(artist.into()),
                name: artist.into(),
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
                name: "x".into(),
                genres: vec!["rock".into(), "pop".into()],
                image: None,
            },
        );
        metadata.artists.insert(
            "y".into(),
            ArtistMeta {
                name: "y".into(),
                genres: vec!["pop".into()],
                image: None,
            },
        );
        let mut records = records.to_vec();
        records.sort_by_key(|record| record.at_ms);
        Index::build(&records, &metadata)
    }

    fn uri(index: &Index, track: &str) -> u32 {
        index
            .track_by_uri(&format!("spotify:track:{track}"))
            .unwrap()
    }

    #[test]
    fn totals_count_streams_and_leave_skips_out() {
        let index = index(&[
            play(1, "a", "x", 120_000),
            play(2, "a", "x", 60_000),
            play(3, "b", "y", 10_000),   // a skip
            play(40, "c", "y", 600_000), // outside four weeks
        ]);
        let totals = totals(&index, Range::Weeks4, NOW);
        assert_eq!(
            totals,
            Totals {
                streams: 2,
                skips: 1,
                ms: 180_000,
                tracks: 1,
                artists: 1,
                albums: 1,
            }
        );
        assert_eq!(totals.minutes(), 3);
        let all = super::totals(&index, Range::AllTime, NOW);
        assert_eq!(
            (all.streams, all.tracks, all.artists, all.albums),
            (3, 2, 2, 2)
        );
    }

    #[test]
    fn tops_rank_by_streams_then_time_with_moves_since_the_period_before() {
        let mut records = vec![
            // This four weeks: b 3 streams, a 2, c 2 but longer.
            play(1, "b", "y", 60_000),
            play(2, "b", "y", 60_000),
            play(3, "b", "y", 60_000),
            play(1, "a", "x", 60_000),
            play(2, "a", "x", 60_000),
            play(4, "c", "x", 300_000),
            play(5, "c", "x", 300_000),
            play(6, "d", "y", 60_000),
        ];
        // The four weeks before: a first, then b, then d; c unplayed.
        for days in [30, 31, 32] {
            records.push(play(days, "a", "x", 60_000));
        }
        for days in [33, 34] {
            records.push(play(days, "b", "y", 60_000));
        }
        records.push(play(35, "d", "y", 60_000));
        let index = index(&records);
        let tracks = top(&index, Kind::Track, Range::Weeks4, NOW, 10);
        let order: Vec<(Item, u32, Delta)> = tracks
            .iter()
            .map(|row| (row.item, row.count.streams, row.delta))
            .collect();
        assert_eq!(
            order,
            vec![
                (Item::Track(uri(&index, "b")), 3, Delta::Up(1)),
                (Item::Track(uri(&index, "c")), 2, Delta::New),
                (Item::Track(uri(&index, "a")), 2, Delta::Down(2)),
                (Item::Track(uri(&index, "d")), 1, Delta::Down(1)),
            ]
        );
        assert_eq!(tracks[1].count.ms, 600_000);
        assert_eq!(top(&index, Kind::Track, Range::Weeks4, NOW, 2).len(), 2);

        let artists = top(&index, Kind::Artist, Range::Weeks4, NOW, 10);
        let x = index.artist_by_id("x").unwrap();
        let y = index.artist_by_id("y").unwrap();
        // Both have four streams; x streamed longer. The period before
        // was a tie of three each, which y won by streaming first.
        assert_eq!(
            artists
                .iter()
                .map(|row| (row.item, row.count.streams, row.delta))
                .collect::<Vec<_>>(),
            vec![
                (Item::Artist(x), 4, Delta::Up(1)),
                (Item::Artist(y), 4, Delta::Down(1)),
            ]
        );

        let all = top(&index, Kind::Track, Range::AllTime, NOW, 10);
        assert!(all.iter().all(|row| row.delta == Delta::None));
    }

    #[test]
    fn genres_count_each_stream_once() {
        let index = index(&[play(1, "a", "x", 60_000), play(2, "b", "y", 60_000)]);
        let genres = top(&index, Kind::Genre, Range::Weeks4, NOW, 10);
        let named: Vec<(&str, u32)> = genres
            .iter()
            .map(|row| match row.item {
                Item::Genre(genre) => (index.genres[genre as usize].as_str(), row.count.streams),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(named, vec![("pop", 2), ("rock", 1)]);
        assert_eq!(top(&index, Kind::Album, Range::Weeks4, NOW, 10).len(), 2);
    }

    #[test]
    fn per_day_has_every_day_and_per_hour_every_hour() {
        let utc = jiff::tz::TimeZone::UTC;
        let index = index(&[
            play(0, "a", "x", 60_000),
            play(2, "a", "x", 60_000),
            play(2, "b", "x", 30_000),
            play(2, "b", "x", 29_999), // a skip
        ]);
        let days = per_day(&index, Range::AllTime, NOW, &utc);
        assert_eq!(days.len(), 3);
        assert_eq!(days[0].count.streams, 2);
        assert_eq!(days[1].count, Count::default());
        assert_eq!(days[2].count.streams, 1);
        let weeks = per_day(&index, Range::Weeks4, NOW, &utc);
        assert!((28..=29).contains(&weeks.len()), "{}", weeks.len());

        let hour = jiff::Timestamp::from_millisecond(NOW)
            .unwrap()
            .to_zoned(utc.clone())
            .hour() as usize;
        let hours = per_hour(&index, Range::AllTime, NOW, &utc);
        assert_eq!(hours[hour].streams, 3);
        assert_eq!(hours.iter().map(|hour| hour.streams).sum::<u32>(), 3);
        // Two hours east, the same plays are two hours later.
        let east = jiff::tz::TimeZone::fixed(jiff::tz::offset(2));
        let shifted = per_hour(&index, Range::AllTime, NOW, &east);
        assert_eq!(shifted[(hour + 2) % 24].streams, 3);
    }

    #[test]
    fn item_stats_give_the_range_figures_and_the_first_and_last_streams() {
        let index = index(&[
            play(100, "a", "x", 60_000),
            play(10, "a", "x", 60_000),
            play(1, "a", "x", 90_000),
            play(1, "b", "x", 60_000),
            play(2, "b", "x", 60_000),
            play(3, "b", "x", 60_000),
        ]);
        let a = Item::Track(uri(&index, "a"));
        let stats = item_stats(&index, a, Range::Weeks4, NOW);
        assert_eq!(
            stats.count,
            Count {
                streams: 2,
                ms: 150_000
            }
        );
        assert_eq!(stats.rank, Some(2));
        assert_eq!(stats.first_ms, Some(NOW - 100 * DAY_MS));
        assert_eq!(stats.last_ms, Some(NOW - DAY_MS));
        let artist = Item::Artist(index.artist_by_id("x").unwrap());
        let stats = item_stats(&index, artist, Range::AllTime, NOW);
        assert_eq!((stats.count.streams, stats.rank), (6, Some(1)));
        let album = Item::Album(index.album_by_id("x-album").unwrap());
        assert_eq!(
            item_stats(&index, album, Range::Months6, NOW).count.streams,
            6
        );
        // Not streamed in the range: no rank, but its history stays.
        let custom = Range::Custom {
            start_ms: NOW - 50 * DAY_MS,
            end_ms: NOW - 40 * DAY_MS,
        };
        let stats = item_stats(&index, a, custom, NOW);
        assert_eq!((stats.count.streams, stats.rank), (0, None));
        assert!(stats.first_ms.is_some());
    }

    #[test]
    fn ranges_and_their_previous_periods() {
        let (start, end) = Range::Months6.bounds(NOW);
        assert_eq!(end - start, 182 * DAY_MS);
        assert_eq!(
            Range::Months6.previous(NOW),
            Some((start - 182 * DAY_MS, start))
        );
        assert_eq!(Range::AllTime.previous(NOW), None);
        let custom = Range::Custom {
            start_ms: NOW - 2 * HOUR,
            end_ms: NOW,
        };
        assert_eq!(custom.previous(NOW), Some((NOW - 4 * HOUR, NOW - 2 * HOUR)));
    }
}
