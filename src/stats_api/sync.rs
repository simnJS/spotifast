//! Pulling the server's plays into the local stats.
//!
//! `GET /history` lists the plays newest first, a page at a time. A pull
//! reads pages until it meets the newest play the last pull brought, so
//! each play comes over once; the [`Cursor`] that remembers where it
//! stopped lives beside the account's play log. A server that took in
//! plays older than that cursor since, an import say, holds more plays
//! than the pulls brought: the next pull reads everything once more, and
//! the log's merge drops what it already has.

use std::future::Future;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::client::{Client, Error};
use super::{HistoryItem, HistoryPage, Me, Status};
use crate::stats::{ArtistName, PlayRecord, Source};

/// The plays asked for per page: the most the server gives.
pub const PAGE: u32 = 200;
/// The most pages one pull reads, so a server that never ends its pages
/// cannot hold the stats forever: 400,000 plays.
const MAX_PAGES: usize = 2_000;
/// The cursor's file, in the account's stats folder.
const CURSOR_FILE: &str = "stats-server.json";

/// Where the last pull stopped.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    /// The server and the server's account it belongs to: another one
    /// starts over.
    pub server: String,
    pub user: i64,
    /// The newest play pulled: its `played_at`, in milliseconds.
    #[serde(default)]
    pub newest: Option<i64>,
    /// How many plays the server held, as far as the pulls know.
    #[serde(default)]
    pub count: i64,
    /// When the last pull finished.
    #[serde(default)]
    pub synced_at: Option<i64>,
}

impl Cursor {
    /// The cursor kept in the stats folder `dir`, if any. Blocking.
    pub fn load(dir: &Path) -> Option<Self> {
        std::fs::read(dir.join(CURSOR_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    }

    /// Replaces the cursor in the stats folder `dir`. Blocking.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        let bytes = serde_json::to_vec(self).map_err(std::io::Error::other)?;
        fastframe_file::write_private(&dir.join(CURSOR_FILE), bytes)
    }
}

/// What a pull needs of the server: the [`Client`], or a fake in tests.
pub trait Server {
    /// The server's address, which the cursor belongs to.
    fn url(&self) -> &str;
    /// `GET /me`.
    fn account(&self) -> impl Future<Output = Result<Me, Error>> + Send;
    /// `GET /me/status`.
    fn poller(&self) -> impl Future<Output = Result<Status, Error>> + Send;
    /// `GET /history?limit=<limit>&before=<before>`.
    fn plays(
        &self,
        before: Option<i64>,
        limit: u32,
    ) -> impl Future<Output = Result<HistoryPage, Error>> + Send;
}

impl Server for Client {
    fn url(&self) -> &str {
        self.server()
    }

    async fn account(&self) -> Result<Me, Error> {
        self.me().await
    }

    async fn poller(&self) -> Result<Status, Error> {
        self.status().await
    }

    async fn plays(&self, before: Option<i64>, limit: u32) -> Result<HistoryPage, Error> {
        self.history(before, limit).await
    }
}

/// What a pull brought.
pub struct Pulled {
    pub account: Me,
    pub status: Status,
    /// The plays new since the cursor, as the stats log keeps them.
    pub records: Vec<PlayRecord>,
    /// The cursor to keep once those plays are saved.
    pub cursor: Cursor,
}

/// The plays after `newest` (all of them when `None`), newest first.
async fn plays_since(server: &impl Server, newest: Option<i64>) -> Result<Vec<HistoryItem>, Error> {
    let mut items = Vec::new();
    let mut before: Option<i64> = None;
    for _ in 0..MAX_PAGES {
        let page = server.plays(before, PAGE).await?;
        let mut reached = false;
        for item in page.items {
            if newest.is_some_and(|newest| item.played_at <= newest) {
                reached = true;
                break;
            }
            items.push(item);
        }
        match page.next_before {
            // Each page must reach further back, or the server is
            // repeating itself.
            Some(next) if !reached && before.is_none_or(|before| next < before) => {
                before = Some(next);
            }
            _ => break,
        }
    }
    Ok(items)
}

/// Pulls the plays the server recorded since `cursor` for the Spotify
/// account `account_id`. The server must record that same account: the
/// plays of another never enter this one's stats.
pub async fn pull(
    server: &impl Server,
    account_id: &str,
    cursor: Option<Cursor>,
    now_ms: i64,
) -> Result<Pulled, Error> {
    let account = server.account().await?;
    if account.spotify_id != account_id {
        return Err(Error::OtherAccount);
    }
    // Asked before the plays, so a play recorded meanwhile can only make
    // the pull bring more than the count says, never less.
    let status = server.poller().await?;
    let cursor = cursor.filter(|cursor| cursor.server == server.url() && cursor.user == account.id);
    let newest = cursor.as_ref().and_then(|cursor| cursor.newest);
    let mut counted = cursor.as_ref().map_or(0, |cursor| cursor.count);
    let mut items = plays_since(server, newest).await?;
    if newest.is_some() && status.plays_count > counted + items.len() as i64 {
        // Plays arrived below the cursor: read them all once.
        items = plays_since(server, None).await?;
        counted = 0;
    }
    let cursor = Cursor {
        server: server.url().to_owned(),
        user: account.id,
        newest: items.iter().map(|item| item.played_at).chain(newest).max(),
        // At least what the server said, so plays its pages never list
        // cannot make every pull read everything.
        count: (counted + items.len() as i64).max(status.plays_count),
        synced_at: Some(now_ms),
    };
    Ok(Pulled {
        records: items.iter().filter_map(record).collect(),
        account,
        status,
        cursor,
    })
}

/// The record the stats log keeps of one of the server's plays, or `None`
/// for one that names no track.
///
/// The server records a play it read from Spotify at Spotify's
/// `played_at`, which has milliseconds, with the track's duration for the
/// time played, as the logger does; an imported one at the end of the
/// stream, to the second, with the time really played. The first is an
/// estimate, the second is not.
pub fn record(item: &HistoryItem) -> Option<PlayRecord> {
    let id = item.spotify_id.trim();
    let track = item.name.trim();
    if id.is_empty() || track.is_empty() {
        return None;
    }
    let estimated = item.played_at % 1000 != 0;
    let ms_played = u32::try_from(item.ms_played.max(0)).unwrap_or(u32::MAX);
    let artist = item.artist_name.trim();
    let album = item.album_name.trim();
    Some(PlayRecord {
        at_ms: item.played_at,
        ms_played,
        estimated,
        source: Source::Server,
        uri: Some(format!("spotify:track:{id}")),
        track: track.to_owned(),
        artists: if artist.is_empty() {
            Vec::new()
        } else {
            vec![ArtistName {
                id: None,
                name: artist.to_owned(),
            }]
        },
        album: (!album.is_empty()).then(|| album.to_owned()),
        album_id: None,
        // The estimate is the track's duration.
        duration_ms: (estimated && ms_played > 0).then_some(ms_played),
        skipped: None,
        shuffle: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const URL: &str = "https://stats.example";

    fn item(played_at: i64, id: &str) -> HistoryItem {
        HistoryItem {
            played_at,
            ms_played: 180_000,
            spotify_id: id.into(),
            name: format!("Song {id}"),
            artist_name: "Band".into(),
            album_name: "Album".into(),
        }
    }

    /// Answers like the server: newest first, `before` exclusive,
    /// `next_before` only after a full page. Records each `before` asked.
    struct Fake {
        spotify_id: String,
        plays: Mutex<Vec<HistoryItem>>,
        asked: Mutex<Vec<Option<i64>>>,
    }

    impl Fake {
        fn new(played_at: impl IntoIterator<Item = i64>) -> Self {
            Self {
                spotify_id: "me".into(),
                plays: Mutex::new(
                    played_at
                        .into_iter()
                        .map(|at| item(at, &format!("t{at}")))
                        .collect(),
                ),
                asked: Mutex::new(Vec::new()),
            }
        }

        fn add(&self, played_at: i64) {
            self.plays
                .lock()
                .unwrap()
                .push(item(played_at, &format!("t{played_at}")));
        }
    }

    impl Server for Fake {
        fn url(&self) -> &str {
            URL
        }

        async fn account(&self) -> Result<Me, Error> {
            Ok(Me {
                id: 7,
                spotify_id: self.spotify_id.clone(),
                display_name: "Me".into(),
            })
        }

        async fn poller(&self) -> Result<Status, Error> {
            Ok(Status {
                plays_count: self.plays.lock().unwrap().len() as i64,
                ..Status::default()
            })
        }

        async fn plays(&self, before: Option<i64>, limit: u32) -> Result<HistoryPage, Error> {
            self.asked.lock().unwrap().push(before);
            let mut items: Vec<HistoryItem> = self
                .plays
                .lock()
                .unwrap()
                .iter()
                .filter(|item| before.is_none_or(|before| item.played_at < before))
                .cloned()
                .collect();
            items.sort_by_key(|item| std::cmp::Reverse(item.played_at));
            items.truncate(limit as usize);
            let next_before = if items.len() == limit as usize {
                items.last().map(|item| item.played_at)
            } else {
                None
            };
            Ok(HistoryPage { items, next_before })
        }
    }

    fn at(n: i64) -> i64 {
        1_700_000_000_000 + n * 240_123
    }

    #[tokio::test]
    async fn a_first_pull_reads_every_page_and_the_next_stops_at_the_cursor() {
        let fake = Fake::new((0..450).map(at));
        let first = pull(&fake, "me", None, 1).await.unwrap();
        assert_eq!(first.records.len(), 450);
        // Three pages of 200, 200 and 50.
        assert_eq!(fake.asked.lock().unwrap().len(), 3);
        assert_eq!(first.cursor.newest, Some(at(449)));
        assert_eq!(first.cursor.count, 450);
        assert_eq!(first.cursor.server, URL);
        assert_eq!(first.cursor.user, 7);
        assert_eq!(first.cursor.synced_at, Some(1));

        fake.asked.lock().unwrap().clear();
        fake.add(at(450));
        fake.add(at(451));
        let next = pull(&fake, "me", Some(first.cursor), 2).await.unwrap();
        // One page reaches the cursor.
        assert_eq!(*fake.asked.lock().unwrap(), vec![None]);
        let ids: Vec<_> = next
            .records
            .iter()
            .map(|record| record.uri.clone().unwrap())
            .collect();
        assert_eq!(
            ids,
            vec![
                format!("spotify:track:t{}", at(451)),
                format!("spotify:track:t{}", at(450)),
            ]
        );
        assert_eq!(next.cursor.newest, Some(at(451)));
        assert_eq!(next.cursor.count, 452);

        // Nothing new: nothing pulled, the cursor stays.
        let idle = pull(&fake, "me", Some(next.cursor.clone()), 3)
            .await
            .unwrap();
        assert!(idle.records.is_empty());
        assert_eq!(idle.cursor.newest, next.cursor.newest);
        assert_eq!(idle.cursor.count, 452);
    }

    #[tokio::test]
    async fn plays_imported_below_the_cursor_are_read_once_more() {
        let fake = Fake::new((10..20).map(at));
        let first = pull(&fake, "me", None, 1).await.unwrap();
        // An import on the server adds older plays.
        for n in 0..5 {
            fake.add(at(n));
        }
        let next = pull(&fake, "me", Some(first.cursor), 2).await.unwrap();
        assert_eq!(next.records.len(), 15);
        assert_eq!(next.cursor.count, 15);
        assert_eq!(next.cursor.newest, Some(at(19)));
        // Once caught up, the next pull reads one page again.
        fake.asked.lock().unwrap().clear();
        let idle = pull(&fake, "me", Some(next.cursor), 3).await.unwrap();
        assert!(idle.records.is_empty());
        assert_eq!(fake.asked.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_cursor_of_another_server_or_account_starts_over() {
        let fake = Fake::new((0..3).map(at));
        let other = Cursor {
            server: "https://elsewhere.example".into(),
            user: 7,
            newest: Some(at(2)),
            count: 3,
            synced_at: None,
        };
        assert_eq!(
            pull(&fake, "me", Some(other), 1)
                .await
                .unwrap()
                .records
                .len(),
            3
        );
        let other_user = Cursor {
            server: URL.into(),
            user: 8,
            newest: Some(at(2)),
            count: 3,
            synced_at: None,
        };
        assert_eq!(
            pull(&fake, "me", Some(other_user), 1)
                .await
                .unwrap()
                .records
                .len(),
            3
        );
    }

    #[tokio::test]
    async fn another_spotify_accounts_plays_are_refused() {
        let fake = Fake::new((0..3).map(at));
        assert_eq!(
            pull(&fake, "someone-else", None, 1).await.err(),
            Some(Error::OtherAccount)
        );
        assert!(fake.asked.lock().unwrap().is_empty());
    }

    #[test]
    fn a_polled_play_is_an_estimate_and_an_imported_one_is_not() {
        let polled = record(&item(1_700_000_000_123, "a")).unwrap();
        assert_eq!(polled.source, Source::Server);
        assert!(polled.estimated);
        assert_eq!(polled.ms_played, 180_000);
        assert_eq!(polled.duration_ms, Some(180_000));
        assert_eq!(polled.uri.as_deref(), Some("spotify:track:a"));
        assert_eq!(polled.track, "Song a");
        assert_eq!(polled.artists[0].name, "Band");
        assert_eq!(polled.album.as_deref(), Some("Album"));

        let mut imported = item(1_700_000_000_000, "b");
        imported.ms_played = 95_000;
        let imported = record(&imported).unwrap();
        assert!(!imported.estimated);
        assert_eq!(imported.ms_played, 95_000);
        assert_eq!(imported.duration_ms, None);

        let mut nameless = item(1, "c");
        nameless.name = " ".into();
        assert!(record(&nameless).is_none());
        let mut no_track = item(1, "");
        no_track.spotify_id.clear();
        assert!(record(&no_track).is_none());
    }

    #[test]
    fn the_cursor_is_kept_beside_the_log() {
        let dir = std::env::temp_dir().join(format!(
            "spotifast-stats-server-cursor-{:016x}",
            rand::random::<u64>()
        ));
        assert_eq!(Cursor::load(&dir), None);
        let cursor = Cursor {
            server: URL.into(),
            user: 7,
            newest: Some(5),
            count: 9,
            synced_at: Some(11),
        };
        cursor.save(&dir).unwrap();
        assert_eq!(Cursor::load(&dir), Some(cursor));
        let _ = std::fs::remove_dir_all(dir);
    }
}
