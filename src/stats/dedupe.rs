//! Merging plays that describe the same listen.
//!
//! The same listen can reach the log three ways: Spotify's recently-played
//! answer, Spotifast's own history, and an imported streaming history. They
//! time it differently (the import gives the end and the time played, the
//! older import only to the minute, recently-played a `played_at` with no
//! time played, the local history the moment the play counted), so each
//! record gives a window for when the listen *started*, and two records of
//! the same song from different sources whose windows overlap are one
//! listen. Each record absorbs at most one record of each other source, so
//! a song played twice in a row stays two plays. Records from one source
//! are never merged, except an exact repeat (the same file imported twice).
//! The record that knows the most is the one kept: real time played before
//! an estimate, then the import before the logger before the local history.

use std::collections::{BTreeSet, HashMap};

use super::PlayRecord;

/// The uncertainty of a timestamp to the second.
const TIGHT_MS: i64 = 5_000;
/// The uncertainty of a timestamp to the minute, or of a local play's
/// counting moment.
const LOOSE_MS: i64 = 60_000;
/// The farthest apart two records of one listen can be: windows are at
/// most a track's length wide, and tracks longer than this are rare
/// enough not to matter.
const SPAN_MS: i64 = 2 * 60 * 60 * 1000;

/// When the listen a record describes may have started, as an interval.
fn start_window(record: &PlayRecord) -> (i64, i64) {
    let length = i64::from(
        record
            .duration_ms
            .unwrap_or(record.ms_played)
            .max(record.ms_played),
    );
    let at = record.at_ms;
    match record.source {
        super::Source::Extended => {
            let start = at - i64::from(record.ms_played);
            (start - TIGHT_MS, start + TIGHT_MS)
        }
        super::Source::Account => {
            let start = at - i64::from(record.ms_played);
            (start - LOOSE_MS, start + LOOSE_MS)
        }
        // `played_at` is close to the start or to the end of the play.
        super::Source::Spotify => (at - length - TIGHT_MS, at + TIGHT_MS),
        // Recorded once the play counted, some time after it started.
        super::Source::Local => (at - length - LOOSE_MS, at + TIGHT_MS),
    }
}

/// Whether `candidate` is the same record as `kept`, met again.
fn exact_repeat(kept: &PlayRecord, candidate: &PlayRecord) -> bool {
    kept.source == candidate.source
        && (kept.at_ms - candidate.at_ms).abs() < 1_000
        && kept.ms_played == candidate.ms_played
}

/// Fills what `kept` lacks from `other`, a record of the same listen.
fn enrich(kept: &mut PlayRecord, other: &PlayRecord) {
    if kept.uri.is_none() {
        kept.uri.clone_from(&other.uri);
    }
    if kept.album.is_none() {
        kept.album.clone_from(&other.album);
    }
    if kept.album_id.is_none() {
        kept.album_id.clone_from(&other.album_id);
    }
    if kept.duration_ms.is_none() {
        kept.duration_ms = other.duration_ms;
    }
    let has_ids = |record: &PlayRecord| record.artists.iter().any(|artist| artist.id.is_some());
    if !has_ids(kept) && has_ids(other) {
        kept.artists.clone_from(&other.artists);
    }
}

/// The result of merging new plays into a log.
#[derive(Debug, Default)]
pub struct Merged {
    /// Every listen once, oldest first.
    pub records: Vec<PlayRecord>,
    /// The new plays that were kept: new listens, or better records of
    /// listens the log had only estimated. These are what to append.
    pub added: Vec<PlayRecord>,
    /// New plays the log already had.
    pub duplicates: usize,
}

/// Merges `new` into `existing`, which may hold duplicates itself (a log
/// read back from its segments).
pub fn merge(existing: Vec<PlayRecord>, new: Vec<PlayRecord>) -> Merged {
    let mut tagged: Vec<(bool, PlayRecord)> = existing
        .into_iter()
        .map(|record| (false, record))
        .chain(new.into_iter().map(|record| (true, record)))
        .collect();
    // The best record of each listen first; among equals what the log
    // already had, so a repeated import counts as duplicates.
    tagged.sort_by(|(new_a, a), (new_b, b)| {
        (a.estimated, a.source.preference(), *new_a, a.at_ms).cmp(&(
            b.estimated,
            b.source.preference(),
            *new_b,
            b.at_ms,
        ))
    });
    struct Kept {
        new: bool,
        record: PlayRecord,
        window: (i64, i64),
        /// The sources whose record of this listen was merged into it.
        absorbed: u8,
    }
    let mut kept: Vec<Kept> = Vec::with_capacity(tagged.len());
    let mut by_song: HashMap<String, BTreeSet<(i64, usize)>> = HashMap::new();
    let mut duplicates = 0;
    for (new, record) in tagged {
        let window = start_window(&record);
        let song = record.song_key();
        let center = (window.0 + window.1) / 2;
        // An exact repeat first, else the nearest record of the same listen.
        let mut same: Option<(i64, usize)> = None;
        for &(_, index) in by_song.get(&song).into_iter().flat_map(|times| {
            times.range((window.0 - SPAN_MS, 0)..(window.1 + SPAN_MS, usize::MAX))
        }) {
            let other = &kept[index];
            let distance = if exact_repeat(&other.record, &record) {
                -1
            } else if other.window.0 <= window.1
                && window.0 <= other.window.1
                && other.record.source != record.source
                && other.absorbed & record.source.bit() == 0
            {
                ((other.window.0 + other.window.1) / 2 - center).abs()
            } else {
                continue;
            };
            if same.is_none_or(|(best, _)| distance < best) {
                same = Some((distance, index));
            }
        }
        let same = same.map(|(_, index)| index);
        match same {
            Some(index) => {
                let other = &mut kept[index];
                if other.record.source != record.source {
                    other.absorbed |= record.source.bit();
                }
                enrich(&mut other.record, &record);
                if new {
                    duplicates += 1;
                }
            }
            None => {
                let index = kept.len();
                by_song
                    .entry(song)
                    .or_default()
                    .insert((record.at_ms, index));
                kept.push(Kept {
                    new,
                    absorbed: record.source.bit(),
                    record,
                    window,
                });
            }
        }
    }
    let mut records = Vec::with_capacity(kept.len());
    let mut added = Vec::new();
    for entry in kept {
        if entry.new {
            added.push(entry.record.clone());
        }
        records.push(entry.record);
    }
    records.sort_by_key(|record| record.at_ms);
    added.sort_by_key(|record| record.at_ms);
    Merged {
        records,
        added,
        duplicates,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::{ArtistName, Source};

    fn play(source: Source, at_s: i64, ms: u32) -> PlayRecord {
        PlayRecord {
            at_ms: at_s * 1000,
            ms_played: ms,
            estimated: matches!(source, Source::Spotify | Source::Local),
            source,
            uri: Some("spotify:track:a".into()),
            track: "Song".into(),
            artists: vec![ArtistName {
                id: None,
                name: "Band".into(),
            }],
            album: None,
            album_id: None,
            duration_ms: Some(200_000),
            skipped: None,
            shuffle: None,
        }
    }

    #[test]
    fn a_repeated_import_adds_nothing() {
        let first = vec![
            play(Source::Extended, 1_000, 200_000),
            play(Source::Extended, 2_000, 10_000),
        ];
        let merged = merge(Vec::new(), first.clone());
        assert_eq!(merged.added.len(), 2);
        let again = merge(merged.records, first);
        assert_eq!(again.records.len(), 2);
        assert!(again.added.is_empty());
        assert_eq!(again.duplicates, 2);
    }

    #[test]
    fn the_logger_and_the_import_of_one_listen_are_one_play() {
        // Spotify said played_at at the end, the import ended a second later.
        let logged = play(Source::Spotify, 10_000, 200_000);
        let imported = play(Source::Extended, 10_001, 200_000);
        let merged = merge(vec![logged], vec![imported.clone()]);
        assert_eq!(merged.records.len(), 1);
        // The import knows the real time played: it replaces the estimate.
        assert_eq!(merged.records[0].source, Source::Extended);
        assert_eq!(merged.added, vec![imported]);
        // Spotify's played_at at the start of the play matches as well.
        let logged = play(Source::Spotify, 20_000, 200_000);
        let imported = play(Source::Extended, 20_150, 150_000);
        let merged = merge(vec![logged], vec![imported]);
        assert_eq!(merged.records.len(), 1);
    }

    #[test]
    fn a_local_play_spotify_also_reports_is_counted_once() {
        // Counted 30 s in; Spotify reports the start.
        let local = play(Source::Local, 5_030, 200_000);
        let logged = play(Source::Spotify, 5_000, 200_000);
        let merged = merge(vec![local], vec![logged]);
        assert_eq!(merged.records.len(), 1);
        assert_eq!(merged.records[0].source, Source::Spotify);
        assert_eq!(merged.duplicates, 0, "the better record is added");
    }

    #[test]
    fn a_song_played_twice_in_a_row_stays_two_plays() {
        let local = vec![
            play(Source::Local, 1_030, 200_000),
            play(Source::Local, 1_230, 200_000),
        ];
        let logged = vec![
            play(Source::Spotify, 1_000, 200_000),
            play(Source::Spotify, 1_200, 200_000),
        ];
        let merged = merge(local, logged);
        assert_eq!(merged.records.len(), 2);
        assert!(merged.records.iter().all(|r| r.source == Source::Spotify));
    }

    #[test]
    fn plays_of_one_source_are_never_merged() {
        let logged = vec![
            play(Source::Spotify, 1_000, 200_000),
            play(Source::Spotify, 1_002, 200_000),
        ];
        assert_eq!(merge(Vec::new(), logged).records.len(), 2);
    }

    #[test]
    fn the_account_history_matches_the_extended_one_to_the_minute() {
        let mut account = play(Source::Account, 3_000, 200_000);
        account.uri = None;
        account.duration_ms = None;
        account.estimated = false;
        let extended = play(Source::Extended, 3_040, 200_000);
        let merged = merge(vec![account], vec![extended]);
        assert_eq!(merged.records.len(), 1);
        assert_eq!(merged.records[0].source, Source::Extended);
    }

    #[test]
    fn a_kept_record_takes_what_it_lacks_from_its_duplicate() {
        let mut account = play(Source::Account, 3_000, 200_000);
        account.uri = None;
        account.estimated = false;
        let mut logged = play(Source::Spotify, 2_800, 200_000);
        logged.album_id = Some("al".into());
        logged.artists[0].id = Some("ar".into());
        let merged = merge(vec![account], vec![logged]);
        assert_eq!(merged.records.len(), 1);
        let kept = &merged.records[0];
        assert_eq!(kept.source, Source::Account);
        assert_eq!(kept.uri.as_deref(), Some("spotify:track:a"));
        assert_eq!(kept.album_id.as_deref(), Some("al"));
        assert_eq!(kept.artists[0].id.as_deref(), Some("ar"));
    }

    #[test]
    fn different_songs_at_the_same_time_are_both_kept() {
        let a = play(Source::Spotify, 1_000, 200_000);
        let mut b = play(Source::Extended, 1_000, 200_000);
        b.track = "Other".into();
        b.uri = Some("spotify:track:b".into());
        assert_eq!(merge(vec![a], vec![b]).records.len(), 2);
    }
}
