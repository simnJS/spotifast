//! Listening statistics, in the manner of stats.fm.
//!
//! The figures come from one of two places ([`Origin`]): the log kept on
//! this computer, or, while one is connected in Settings, the statistics
//! server ([`crate::stats_api`]), which reads recently-played every few
//! minutes even while this computer is off and answers every figure
//! itself ("API mode"). In API mode the log is neither read nor written:
//! it stays on the disk as it was and is shown again once the server is
//! disconnected.
//!
//! These sources feed the local per-account log of plays:
//!
//! - the logger ([`logger`]), which reads `/me/player/recently-played` every
//!   twenty minutes, at start and after waking, and so catches the plays of
//!   every device, plus the plays Spotifast saw itself ([`crate::history`])
//!   that Spotify's answer leaves out;
//! - an import ([`import`]) of the "Extended streaming history" ZIP (or its
//!   JSON files, or the older account-data `StreamingHistory_music_*.json`)
//!   Spotify's privacy page sends on request.
//!
//! The log lives under the state folder ([`store`]), one directory per
//! account, as append-only segments of JSON lines that are each written
//! whole and atomically. Loading it merges duplicates ([`dedupe`]) and
//! builds an [`Index`] that the pure queries in [`query`] aggregate.
//! Track, artist and album details the plays do not carry are fetched a
//! few at a time ([`metadata`]) and cached beside the log.
//!
//! A play counts as a *stream* once it lasted 30 seconds (stats.fm's rule);
//! shorter plays are kept as skips.

pub mod dedupe;
pub mod import;
pub mod index;
pub mod logger;
pub mod metadata;
pub mod query;
pub mod service;
pub mod store;

use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub use index::Index;
pub use query::{Item, Range};

/// A play lasting at least this long is a stream; shorter ones are skips.
pub const STREAM_MS: u32 = 30_000;

/// Where a play was learnt from. Plays from different sources may describe
/// the same listen and are merged; plays from one source never are, except
/// for an exact repeat (the same file imported twice).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Spotify's "Extended streaming history": end time to the second and
    /// the exact time played.
    Extended,
    /// The older account-data history: end time to the minute, exact time
    /// played, no URI.
    Account,
    /// `/me/player/recently-played`: every device, no time played.
    Spotify,
    /// A play Spotifast itself saw ([`crate::history`]).
    Local,
    /// A statistics server's `/history`, which earlier versions pulled into
    /// the log: what it read from recently-played, with the track's
    /// duration for the time played, and what was imported into it, with
    /// the time played. Logs that hold such plays still read.
    Server,
}

/// Where the figures shown come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The log kept on this computer.
    Local,
    /// The statistics server at this address: API mode.
    Server(String),
}

impl Source {
    /// Which of two records for the same listen is kept: the one that
    /// knows the most about it.
    fn preference(self) -> u8 {
        match self {
            Self::Extended => 0,
            Self::Account => 1,
            Self::Spotify => 2,
            // Spotify's own `played_at`, as the logger's, but fewer details.
            Self::Server => 3,
            Self::Local => 4,
        }
    }

    fn bit(self) -> u8 {
        1 << self.preference()
    }
}

/// An artist as a play names it: always a name, an ID when the source has
/// one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistName {
    #[serde(rename = "i", default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "n")]
    pub name: String,
}

/// One play, as stored: one JSON line in a segment. The short field names
/// keep a log of 100,000 plays to a few megabytes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayRecord {
    /// When the play happened, in milliseconds since the Unix epoch: the
    /// end of the play for an import, Spotify's `played_at` for the logger,
    /// and the moment the play counted for a local play.
    #[serde(rename = "t")]
    pub at_ms: i64,
    /// How long it was listened to.
    #[serde(rename = "ms")]
    pub ms_played: u32,
    /// `ms_played` is the track's duration, because the source does not say.
    #[serde(rename = "e", default, skip_serializing_if = "std::ops::Not::not")]
    pub estimated: bool,
    #[serde(rename = "s")]
    pub source: Source,
    /// `spotify:track:…`, when the source names it.
    #[serde(rename = "u", default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(rename = "n")]
    pub track: String,
    #[serde(rename = "a", default, skip_serializing_if = "Vec::is_empty")]
    pub artists: Vec<ArtistName>,
    #[serde(rename = "al", default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(rename = "ai", default, skip_serializing_if = "Option::is_none")]
    pub album_id: Option<String>,
    #[serde(rename = "d", default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u32>,
    #[serde(rename = "sk", default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<bool>,
    #[serde(rename = "sh", default, skip_serializing_if = "Option::is_none")]
    pub shuffle: Option<bool>,
}

impl PlayRecord {
    /// Whether it counts as a stream rather than a skip.
    pub fn is_stream(&self) -> bool {
        self.ms_played >= STREAM_MS
    }

    /// The Spotify ID of the track, from its URI.
    pub fn track_id(&self) -> Option<&str> {
        self.uri
            .as_deref()
            .and_then(|uri| uri.strip_prefix("spotify:track:"))
            .filter(|id| !id.is_empty())
    }

    /// What identifies the song across sources, URI or not: its first
    /// artist and its title, folded to lower case.
    pub fn song_key(&self) -> String {
        let artist = self
            .artists
            .first()
            .map(|artist| artist.name.as_str())
            .unwrap_or_default();
        format!(
            "{}\u{1f}{}",
            artist.trim().to_lowercase(),
            self.track.trim().to_lowercase()
        )
    }
}

/// What the import read and kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportSummary {
    /// History files read.
    pub files: usize,
    /// Plays found in them.
    pub read: usize,
    /// Plays new to the log (or better than the copy it had).
    pub added: usize,
    /// Plays the log already had.
    pub duplicates: usize,
}

/// How far an import has got.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportProgress {
    pub files_done: usize,
    pub files_total: usize,
    pub plays_read: usize,
}

/// What the stats service tells the interface.
#[derive(Clone, Debug)]
pub enum Update {
    /// The account's plays, freshly indexed.
    Loaded {
        account_id: String,
        index: Arc<Index>,
    },
    /// The log could not be read or refreshed.
    Failed {
        account_id: String,
        error: String,
    },
    ImportProgress(ImportProgress),
    ImportFinished(Result<ImportSummary, String>),
    /// About the statistics server.
    Server(crate::stats_api::Update),
}

/// The listening stats as the interface holds them (`app.stats`).
#[derive(Clone, Debug, Default)]
pub struct StatsState {
    /// The account the index belongs to.
    pub account_id: Option<String>,
    /// The indexed plays, which [`query`] aggregates.
    pub index: crate::model::Loadable<Arc<Index>>,
    /// An import in progress.
    pub importing: Option<ImportProgress>,
    /// How the last import ended.
    pub last_import: Option<Result<ImportSummary, String>>,
    /// The statistics server, when one is connected.
    pub server: crate::stats_api::State,
}

impl StatsState {
    /// Takes in an update from the service. An update for another account
    /// than the signed-in one is dropped.
    pub fn receive(&mut self, update: Update, signed_in: Option<&str>) {
        match update {
            Update::Loaded { account_id, index } => {
                if signed_in.is_some_and(|account| account != account_id) {
                    return;
                }
                self.account_id = Some(account_id);
                self.index = crate::model::Loadable::Loaded(index);
            }
            Update::Failed { account_id, error } => {
                if signed_in.is_some_and(|account| account != account_id) {
                    return;
                }
                // Plays already shown stay: a failed refresh loses nothing.
                if self.index.get().is_none() || self.account_id.as_deref() != Some(&account_id) {
                    self.index = crate::model::Loadable::Failed(error);
                } else {
                    log::warn!("listening stats refresh failed: {error}");
                }
                self.account_id = Some(account_id);
            }
            Update::ImportProgress(progress) => self.importing = Some(progress),
            Update::ImportFinished(result) => {
                self.importing = None;
                self.last_import = Some(result);
            }
            Update::Server(update) => self.server.receive(update),
        }
    }

    /// Where the figures shown come from: the statistics server while one
    /// is connected, the local log otherwise.
    pub fn origin(&self) -> Origin {
        match self.server.connected() {
            Some(server) => Origin::Server(server.to_owned()),
            None => Origin::Local,
        }
    }

    /// Marks the stats as being read, keeping what is shown for the same
    /// account.
    pub fn loading(&mut self, account_id: &str) {
        if self.account_id.as_deref() != Some(account_id) {
            self.account_id = Some(account_id.to_owned());
            self.index = crate::model::Loadable::Loading;
        } else if self.index.get().is_none() {
            self.index = crate::model::Loadable::Loading;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats_api::{self, DEFAULT_SERVER, Me};

    fn me() -> Me {
        Me {
            id: 1,
            spotify_id: "me".into(),
            display_name: "Me".into(),
        }
    }

    #[test]
    fn the_figures_come_from_the_server_only_while_it_is_connected() {
        let mut stats = StatsState::default();
        let local = Arc::new(Index::default());
        stats.receive(
            Update::Loaded {
                account_id: "me".into(),
                index: Arc::clone(&local),
            },
            Some("me"),
        );
        assert_eq!(stats.origin(), Origin::Local);
        // Connecting is not connected yet: the log stays on show.
        stats.server.authorizing(DEFAULT_SERVER.into());
        assert_eq!(stats.origin(), Origin::Local);
        stats.receive(
            Update::Server(stats_api::Update::Connected {
                server: DEFAULT_SERVER.into(),
                account: me(),
            }),
            Some("me"),
        );
        assert_eq!(stats.origin(), Origin::Server(DEFAULT_SERVER.into()));
        // A failed check keeps API mode.
        stats.receive(
            Update::Server(stats_api::Update::Failed("offline".into())),
            Some("me"),
        );
        assert_eq!(stats.origin(), Origin::Server(DEFAULT_SERVER.into()));
        // The log's index is kept meanwhile, so disconnecting shows it again.
        stats.receive(
            Update::Server(stats_api::Update::Disconnected(None)),
            Some("me"),
        );
        assert_eq!(stats.origin(), Origin::Local);
        assert!(Arc::ptr_eq(stats.index.get().unwrap(), &local));
        // Restored at start.
        stats.server.connecting(DEFAULT_SERVER.into());
        assert_eq!(stats.origin(), Origin::Local);
        stats.receive(
            Update::Server(stats_api::Update::Restored {
                server: DEFAULT_SERVER.into(),
            }),
            Some("me"),
        );
        assert_eq!(stats.origin(), Origin::Server(DEFAULT_SERVER.into()));
        stats.server.disconnect();
        assert_eq!(stats.origin(), Origin::Local);
    }
}
