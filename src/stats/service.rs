//! The stats work the backend runs: refreshing the log from Spotify and
//! the local history, importing a streaming history, and indexing the
//! result. Everything that waits for the disk runs on a blocking thread;
//! nothing here runs on the UI thread.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::index::Index;
use super::logger::{self, RecentSource};
use super::metadata::{self, Metadata, MetadataSource};
use super::store::Log;
use super::{ImportProgress, ImportSummary, PlayRecord, Source};

/// What the service needs of the Web API.
pub trait Api: RecentSource + MetadataSource + Sync {}
impl<T: RecentSource + MetadataSource + Sync> Api for T {}

/// The log of the account last used, kept open between refreshes.
struct Open {
    account_id: String,
    log: Log,
    metadata: Metadata,
}

/// One per backend; the backend holds it behind a lock so refreshes and
/// imports run one at a time.
#[derive(Default)]
pub struct Service {
    open: Option<Open>,
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| format!("the stats worker stopped: {error}"))
}

/// The plays Spotifast saw itself, from its history file.
fn local_plays(history_file: &Path) -> Vec<PlayRecord> {
    crate::history::History::load(history_file)
        .plays()
        .iter()
        .filter_map(|play| logger::record(play, Source::Local))
        .collect()
}

impl Service {
    /// The account's log, read from `dir` unless it is the one open.
    async fn take(&mut self, account_id: &str, dir: PathBuf) -> Result<Open, String> {
        if let Some(open) = self.open.take()
            && open.account_id == account_id
        {
            return Ok(open);
        }
        let (log, metadata) = blocking(move || {
            let log = Log::open(&dir)?;
            let metadata = log.read_metadata();
            Ok::<_, std::io::Error>((log, metadata))
        })
        .await?
        .map_err(|error| format!("could not read the listening stats: {error}"))?;
        Ok(Open {
            account_id: account_id.to_owned(),
            log,
            metadata,
        })
    }

    /// Appends `new` to the log, off this thread.
    async fn append(
        open: Open,
        new: Vec<PlayRecord>,
    ) -> Result<(Open, super::store::Appended), String> {
        blocking(move || {
            let mut open = open;
            let appended = open.log.append(new);
            (open, appended)
        })
        .await
        .and_then(|(open, appended)| {
            appended
                .map(|appended| (open, appended))
                .map_err(|error| format!("could not save the listening stats: {error}"))
        })
    }

    /// Fetches missing details, saves them if any came, and indexes the log.
    async fn finish(mut open: Open, api: &impl Api) -> Result<(Open, Arc<Index>), String> {
        if metadata::fill(api, &mut open.metadata, open.log.records()).await {
            let snapshot = open.metadata.clone();
            let dir = open.log.dir().to_owned();
            blocking(move || {
                if let Err(error) = super::store::write_metadata(&dir, &snapshot) {
                    log::warn!("could not save the stats metadata: {error}");
                }
            })
            .await?;
        }
        blocking(move || {
            let index = Arc::new(Index::build(open.log.records(), &open.metadata));
            (open, index)
        })
        .await
    }

    /// Reads what Spotify played since the last refresh and what the local
    /// history holds that the log lacks, saves the new plays, fills in a
    /// few missing details, and indexes the log.
    pub async fn refresh(
        &mut self,
        account_id: &str,
        dir: PathBuf,
        history_file: PathBuf,
        api: &impl Api,
    ) -> Result<Arc<Index>, String> {
        let mut open = self.take(account_id, dir).await?;
        let mut new = blocking(move || local_plays(&history_file)).await?;
        match logger::fetch(api, open.log.spotify_cursor()).await {
            Ok(plays) => {
                for play in &plays {
                    open.metadata.learn_track(&play.track);
                }
                new.extend(
                    plays
                        .iter()
                        .filter_map(|play| logger::record(play, Source::Spotify)),
                );
            }
            // The local plays and the log still stand; the next refresh asks
            // Spotify again from the same cursor.
            Err(error) => log::warn!("recently played for the stats failed: {error}"),
        }
        let (open, _) = Self::append(open, new).await?;
        let (open, index) = Self::finish(open, api).await?;
        self.open = Some(open);
        Ok(index)
    }

    /// Imports the streaming history at `path` into the account's log.
    pub async fn import(
        &mut self,
        account_id: &str,
        dir: PathBuf,
        path: PathBuf,
        api: &impl Api,
        progress: impl FnMut(ImportProgress) + Send + 'static,
    ) -> Result<(ImportSummary, Arc<Index>), String> {
        let read = blocking(move || {
            let mut progress = progress;
            super::import::read_path(&path, &mut progress)
        })
        .await?
        .map_err(|error| error.to_string())?;
        let mut summary = ImportSummary {
            files: read.files,
            read: read.plays.len(),
            ..ImportSummary::default()
        };
        let open = self.take(account_id, dir).await?;
        let (open, appended) = Self::append(open, read.plays).await?;
        summary.added = appended.added;
        summary.duplicates = appended.duplicates;
        let (open, index) = Self::finish(open, api).await?;
        self.open = Some(open);
        Ok((summary, index))
    }

    /// Forgets the open log, as when the account signs out.
    pub fn close(&mut self) {
        self.open = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ApiError;
    use crate::api::models::{Album, Artist, ArtistRef, CursorPage, Cursors, PlayHistory, Track};
    use crate::stats::query::{self, Kind, Range};
    use std::sync::Mutex;

    fn track(id: &str) -> Track {
        Track {
            id: Some(id.into()),
            uri: format!("spotify:track:{id}"),
            name: format!("Song {id}"),
            duration_ms: 200_000,
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
        }
    }

    struct Fake {
        plays: Vec<PlayHistory>,
        asked: Mutex<Vec<String>>,
    }

    impl RecentSource for Fake {
        async fn recently_played(
            &self,
            after_ms: Option<i64>,
        ) -> Result<CursorPage<PlayHistory>, ApiError> {
            self.asked
                .lock()
                .unwrap()
                .push(format!("recent:{after_ms:?}"));
            let items = self
                .plays
                .iter()
                .filter(|play| {
                    let at = play.played_at.as_deref().unwrap();
                    let at = at.parse::<jiff::Timestamp>().unwrap().as_millisecond();
                    after_ms.is_none_or(|after| at > after)
                })
                .cloned()
                .collect();
            Ok(CursorPage {
                items,
                total: None,
                next: None,
                cursors: Some(Cursors::default()),
            })
        }
    }

    impl MetadataSource for Fake {
        async fn track(&self, id: &str) -> Result<Track, ApiError> {
            self.asked.lock().unwrap().push(format!("track:{id}"));
            Ok(track(id))
        }
        async fn artist(&self, id: &str) -> Result<Artist, ApiError> {
            self.asked.lock().unwrap().push(format!("artist:{id}"));
            Ok(Artist {
                id: id.into(),
                name: "Band".into(),
                genres: vec!["indie".into()],
                ..Artist::default()
            })
        }
    }

    fn played(id: &str, at: jiff::Timestamp) -> PlayHistory {
        PlayHistory {
            track: track(id),
            played_at: Some(at.to_string()),
            context: None,
        }
    }

    #[tokio::test]
    async fn a_refresh_logs_spotify_and_local_plays_and_an_import_merges_in() {
        let root = std::env::temp_dir().join(format!(
            "spotifast-stats-service-{:016x}",
            rand::random::<u64>()
        ));
        let dir = root.join("account");
        let history = root.join("history.json");
        let now = jiff::Timestamp::now();
        let ago = |minutes: i64| now - jiff::SignedDuration::from_mins(minutes);
        // Spotifast saw "a" (also in Spotify's answer) and "local" (not).
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            &history,
            serde_json::to_string(&vec![played("a", ago(59)), played("local", ago(30))]).unwrap(),
        )
        .unwrap();
        let fake = Fake {
            plays: vec![played("a", ago(60)), played("b", ago(50))],
            asked: Mutex::new(Vec::new()),
        };
        let mut service = Service::default();
        let index = service
            .refresh("me", dir.clone(), history.clone(), &fake)
            .await
            .unwrap();
        assert_eq!(index.plays.len(), 3);
        let asked = fake.asked.lock().unwrap().clone();
        assert_eq!(asked[0], "recent:None");
        // Recently-played came with full tracks: only the local-only track
        // and the artist are asked for.
        assert_eq!(
            asked[1..],
            ["track:local".to_string(), "artist:band".to_string()]
        );
        let genres = query::top(&index, Kind::Genre, Range::Weeks4, now.as_millisecond(), 5);
        assert_eq!(genres.len(), 1);
        assert_eq!(genres[0].count.streams, 3);

        // The next refresh asks after the newest logged play.
        fake.asked.lock().unwrap().clear();
        let index = service
            .refresh("me", dir.clone(), history.clone(), &fake)
            .await
            .unwrap();
        assert_eq!(index.plays.len(), 3);
        assert_eq!(
            *fake.asked.lock().unwrap(),
            vec![format!("recent:{:?}", Some(ago(50).as_millisecond()))]
        );

        // Importing the extended history of the same "b" play keeps one.
        let import = root.join("Streaming_History_Audio_2024.json");
        let end = ago(50) + jiff::SignedDuration::from_secs(2);
        std::fs::write(
            &import,
            format!(
                r#"[{{"ts": "{}", "ms_played": 150000,
                  "master_metadata_track_name": "Song b",
                  "master_metadata_album_artist_name": "Band",
                  "master_metadata_album_album_name": "Album",
                  "spotify_track_uri": "spotify:track:b"}},
                  {{"ts": "2020-01-01T00:00:00Z", "ms_played": 100000,
                  "master_metadata_track_name": "Old",
                  "master_metadata_album_artist_name": "Band",
                  "master_metadata_album_album_name": "Album",
                  "spotify_track_uri": "spotify:track:old"}}]"#,
                end.strftime("%Y-%m-%dT%H:%M:%SZ")
            ),
        )
        .unwrap();
        let (summary, index) = service
            .import("me", dir.clone(), import, &fake, |_| {})
            .await
            .unwrap();
        assert_eq!(
            summary,
            ImportSummary {
                files: 1,
                read: 2,
                // The old play, and the import's better record of "b".
                added: 2,
                duplicates: 0,
            }
        );
        assert_eq!(index.plays.len(), 4);
        let b = index.track_by_uri("spotify:track:b").unwrap();
        let b_play = index.plays.iter().find(|play| play.track == b).unwrap();
        assert_eq!(b_play.ms_played, 150_000);
        assert!(!b_play.estimated);

        // A fresh service reads the same log back from the disk.
        let mut fresh = Service::default();
        let index = fresh.refresh("me", dir, history, &fake).await.unwrap();
        assert_eq!(index.plays.len(), 4);
        let _ = std::fs::remove_dir_all(root);
    }
}
