//! Local listening statistics, in the manner of stats.fm, kept on this
//! computer only.
//!
//! Two sources feed one per-account log of plays:
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
}

impl Source {
    /// Which of two records for the same listen is kept: the one that
    /// knows the most about it.
    fn preference(self) -> u8 {
        match self {
            Self::Extended => 0,
            Self::Account => 1,
            Self::Spotify => 2,
            Self::Local => 3,
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
