//! The recently-played logger.
//!
//! Spotify keeps only the last fifty plays of an account, from every
//! device, behind `GET /me/player/recently-played`. Read every twenty
//! minutes, at start and after waking, with `after` set to the newest play
//! already logged, it hands over each new play once, and the log keeps
//! them for good. Spotify does not say how long each was listened to, so
//! the time played is the track's duration, marked as an estimate.

use std::future::Future;
use std::time::Duration;

use super::{ArtistName, PlayRecord, Source};
use crate::api::ApiError;
use crate::api::models::{CursorPage, PlayHistory};

/// How often the logger reads Spotify while Spotifast runs.
pub const INTERVAL: Duration = Duration::from_secs(20 * 60);
/// The most recently-played returns at once.
pub const PAGE: u32 = 50;

/// Where recently-played comes from: the Web API, or a fake in tests.
pub trait RecentSource {
    /// `GET /me/player/recently-played?limit=50&after=<after_ms>`.
    fn recently_played(
        &self,
        after_ms: Option<i64>,
    ) -> impl Future<Output = Result<CursorPage<PlayHistory>, ApiError>> + Send;
}

/// The record of one recently-played or local history entry, or `None`
/// for one that is not a Spotify track or has no time.
pub fn record(play: &PlayHistory, source: Source) -> Option<PlayRecord> {
    let at = play.played_at.as_deref()?.parse::<jiff::Timestamp>().ok()?;
    let track = &play.track;
    if !track.uri.starts_with("spotify:track:") || track.name.is_empty() {
        return None;
    }
    let duration = (track.duration_ms > 0).then_some(track.duration_ms);
    Some(PlayRecord {
        at_ms: at.as_millisecond(),
        ms_played: track.duration_ms,
        estimated: true,
        source,
        uri: Some(track.uri.clone()),
        track: track.name.clone(),
        artists: track
            .artists
            .iter()
            .map(|artist| ArtistName {
                id: artist.id.clone().filter(|id| !id.is_empty()),
                name: artist.name.clone(),
            })
            .collect(),
        album: track
            .album
            .as_ref()
            .map(|album| album.name.clone())
            .filter(|name| !name.is_empty()),
        album_id: track
            .album
            .as_ref()
            .map(|album| album.id.clone())
            .filter(|id| !id.is_empty()),
        duration_ms: duration,
        skipped: None,
        shuffle: None,
    })
}

/// The plays Spotify reports after `after_ms` (all fifty it keeps when
/// `None`), with the full track objects they came with, oldest first.
/// One page is all there is: Spotify keeps no more than fifty plays, so
/// asking again after a full page could not reach older ones.
pub async fn fetch(
    source: &impl RecentSource,
    after_ms: Option<i64>,
) -> Result<Vec<PlayHistory>, ApiError> {
    let mut plays = source.recently_played(after_ms).await?.items;
    plays.sort_by_key(|play| {
        play.played_at
            .as_deref()
            .and_then(|at| at.parse::<jiff::Timestamp>().ok())
    });
    Ok(plays)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::{Album, ArtistRef, Cursors, Track};
    use std::sync::Mutex;

    fn played(uri: &str, at: &str) -> PlayHistory {
        PlayHistory {
            track: Track {
                id: Some(uri.trim_start_matches("spotify:track:").into()),
                uri: uri.into(),
                name: "Song".into(),
                duration_ms: 210_000,
                artists: vec![ArtistRef {
                    id: Some("band".into()),
                    name: "Band".into(),
                    uri: None,
                }],
                album: Some(Album {
                    id: "album".into(),
                    name: "Album".into(),
                    ..Album::default()
                }),
                ..Track::default()
            },
            played_at: Some(at.into()),
            context: None,
        }
    }

    /// Answers like Spotify: the plays strictly after the cursor, newest
    /// first, fifty at most, and records each request's `after`.
    struct FakeSpotify {
        plays: Vec<PlayHistory>,
        asked: Mutex<Vec<Option<i64>>>,
    }

    impl RecentSource for FakeSpotify {
        async fn recently_played(
            &self,
            after_ms: Option<i64>,
        ) -> Result<CursorPage<PlayHistory>, ApiError> {
            self.asked.lock().unwrap().push(after_ms);
            let at = |play: &PlayHistory| {
                play.played_at
                    .as_deref()
                    .unwrap()
                    .parse::<jiff::Timestamp>()
                    .unwrap()
                    .as_millisecond()
            };
            let mut items: Vec<PlayHistory> = self
                .plays
                .iter()
                .filter(|play| after_ms.is_none_or(|after| at(play) > after))
                .cloned()
                .collect();
            items.sort_by_key(|play| std::cmp::Reverse(at(play)));
            items.truncate(PAGE as usize);
            let after = items.iter().map(at).max().map(|ms| ms.to_string());
            Ok(CursorPage {
                items,
                total: None,
                next: None,
                cursors: Some(Cursors {
                    after,
                    before: None,
                }),
            })
        }
    }

    #[tokio::test]
    async fn the_logger_asks_after_its_cursor_and_estimates_the_time_played() {
        let spotify = FakeSpotify {
            plays: vec![
                played("spotify:track:a", "2024-03-01T10:00:00.000Z"),
                played("spotify:track:b", "2024-03-01T10:04:00.500Z"),
            ],
            asked: Mutex::new(Vec::new()),
        };
        let cursor = "2024-03-01T10:00:00Z"
            .parse::<jiff::Timestamp>()
            .unwrap()
            .as_millisecond();
        let plays = fetch(&spotify, Some(cursor)).await.unwrap();
        assert_eq!(*spotify.asked.lock().unwrap(), vec![Some(cursor)]);
        assert_eq!(plays.len(), 1);
        let logged = record(&plays[0], Source::Spotify).unwrap();
        assert_eq!(logged.uri.as_deref(), Some("spotify:track:b"));
        assert_eq!(logged.ms_played, 210_000);
        assert!(logged.estimated);
        assert_eq!(logged.artists[0].id.as_deref(), Some("band"));
        assert_eq!(logged.album_id.as_deref(), Some("album"));
        assert_eq!(logged.at_ms, cursor + 4 * 60_000 + 500);
    }

    #[tokio::test]
    async fn one_request_brings_the_fifty_plays_spotify_keeps() {
        let start = "2024-03-01T00:00:00Z".parse::<jiff::Timestamp>().unwrap();
        let plays: Vec<PlayHistory> = (0..70)
            .map(|n| {
                let at = start + jiff::SignedDuration::from_mins(4 * n);
                played(&format!("spotify:track:t{n}"), &at.to_string())
            })
            .collect();
        let spotify = FakeSpotify {
            plays,
            asked: Mutex::new(Vec::new()),
        };
        let got = fetch(&spotify, None).await.unwrap();
        assert_eq!(*spotify.asked.lock().unwrap(), vec![None]);
        assert_eq!(got.len(), 50);
        // Oldest first, and the newest play is the last.
        let last = got.last().unwrap().played_at.clone().unwrap();
        assert_eq!(
            last.parse::<jiff::Timestamp>().unwrap(),
            start + jiff::SignedDuration::from_mins(4 * 69)
        );
    }

    #[test]
    fn episodes_and_local_files_are_not_logged() {
        let mut play = played("spotify:episode:e", "2024-03-01T10:00:00Z");
        assert!(record(&play, Source::Spotify).is_none());
        play.track.uri = "spotify:local:a:b:c:100".into();
        assert!(record(&play, Source::Local).is_none());
        play.track.uri = "spotify:track:a".into();
        play.played_at = None;
        assert!(record(&play, Source::Local).is_none());
    }
}
