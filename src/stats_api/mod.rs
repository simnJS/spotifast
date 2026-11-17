//! The optional statistics server: a self-hosted
//! [spotifast-stats-api](https://github.com/simnJS/spotifast-stats-api)
//! that reads the account's recently played every few minutes, day and
//! night, and keeps the streaming history imported into it.
//!
//! Settings connects to it through the browser ([`loopback`]) or with a
//! pasted token, kept in the credential store ([`Connection`]). While
//! connected, every stats refresh (at start, after connecting and every
//! twenty minutes) pulls the plays the server recorded since the last pull
//! ([`sync`]) into the local log, where they merge with the logger's and
//! the imports' like any other source ([`crate::stats::Source::Server`]).
//! Every request runs on the backend's runtime ([`client`]).

pub mod client;
pub mod loopback;
pub mod sync;

use serde::{Deserialize, Serialize};

/// The server Settings offers until another address is typed.
pub const DEFAULT_SERVER: &str = "https://spotapi.simnjs.fr";

/// An API token of the server. It is a usable secret: its `Debug` shows
/// nothing of it, and it is kept in the credential store, never in the
/// settings or the log.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Token(String);

impl Token {
    /// A token as typed or received, without surrounding spaces; `None`
    /// when nothing is left or it holds spaces, which no token does.
    pub fn new(token: &str) -> Option<Self> {
        let token = token.trim();
        (!token.is_empty() && !token.chars().any(char::is_whitespace))
            .then(|| Self(token.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(…)")
    }
}

/// The server connected to and its token, as the credential store keeps
/// them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    /// The server's address, as [`normalize_server`] gives it.
    pub server: String,
    pub token: Token,
}

impl Connection {
    /// Whether a stored connection can be used.
    pub fn valid(&self) -> bool {
        normalize_server(&self.server).is_ok_and(|server| server == self.server)
            && Token::new(self.token.as_str()).is_some()
    }
}

/// The server's address as requests use it: without surrounding spaces or
/// a trailing slash. The token travels in every request, so plain `http`
/// is only for a server on this computer.
pub fn normalize_server(address: &str) -> Result<String, String> {
    let address = address.trim().trim_end_matches('/');
    let invalid = || format!("Enter the server's address, like {DEFAULT_SERVER}.");
    let url = reqwest::Url::parse(address).map_err(|_| invalid())?;
    let host = url.host_str().ok_or_else(invalid)?;
    if url.query().is_some() || url.fragment().is_some() || !url.username().is_empty() {
        return Err(invalid());
    }
    match url.scheme() {
        "https" => Ok(address.to_owned()),
        "http" if matches!(host, "127.0.0.1" | "localhost" | "[::1]") => Ok(address.to_owned()),
        "http" => Err(
            "Use https:// for a server on another computer: the token would \
             cross the network unencrypted. Only a server on this computer may \
             use http://."
                .to_owned(),
        ),
        _ => Err(invalid()),
    }
}

/// The server's account, `GET /me`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Me {
    /// The server's own ID of the account.
    pub id: i64,
    /// The Spotify account it records.
    pub spotify_id: String,
    #[serde(default)]
    pub display_name: String,
}

/// Where the server's poller stands for the account, `GET /me/status`.
/// Times are in milliseconds since the Unix epoch.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Status {
    /// The last time the server read Spotify successfully.
    #[serde(default)]
    pub last_poll_ok_at: Option<i64>,
    /// When it reads Spotify next.
    #[serde(default)]
    pub next_poll_at: Option<i64>,
    /// Why the last read failed, when it did.
    #[serde(default)]
    pub last_error: Option<String>,
    /// The newest play the poller recorded.
    #[serde(default)]
    pub last_played_at: Option<i64>,
    /// Every play the server holds for the account.
    #[serde(default)]
    pub plays_count: i64,
}

/// One play of `GET /history`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct HistoryItem {
    /// When it was played, in milliseconds since the Unix epoch.
    pub played_at: i64,
    /// How long it was played, as the server knows it.
    #[serde(default)]
    pub ms_played: i64,
    /// The Spotify ID of the track.
    pub spotify_id: String,
    pub name: String,
    /// The track's first artist.
    #[serde(default)]
    pub artist_name: String,
    #[serde(default)]
    pub album_name: String,
}

/// A page of `GET /history`, newest first.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct HistoryPage {
    #[serde(default)]
    pub items: Vec<HistoryItem>,
    /// The `before` of the next page; `None` after the last one.
    #[serde(default)]
    pub next_before: Option<i64>,
}

/// A pull that went through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Synced {
    /// When it finished.
    pub at_ms: i64,
    /// The plays new to the local log.
    pub added: usize,
    pub account: Me,
    pub status: Status,
}

/// How the connection stands, as Settings shows it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Disconnected,
    /// Waiting for the consent in the browser.
    Authorizing,
    /// Checking a token, or reading the saved one.
    Connecting,
    Connected,
}

/// What the backend tells the interface about the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Update {
    /// A new token was checked with the server and saved.
    Connected { server: String, account: Me },
    /// The saved connection was read back at start.
    Restored { server: String },
    /// A pull went through.
    Synced(Synced),
    /// Something failed. A connection already made stays.
    Failed(String),
    /// The connection is gone; with why, when the server refused it.
    Disconnected(Option<String>),
    /// The streaming history being imported is also on its way to the
    /// server.
    Uploading,
    /// That upload ended: the plays the server took in, or why it did not.
    Uploaded(Result<u64, String>),
}

/// The statistics server as the interface holds it (`app.stats.server`).
#[derive(Clone, Debug, Default)]
pub struct State {
    pub phase: Phase,
    /// The server connected to, or being connected to.
    pub server: Option<String>,
    /// The server's account, once it answered.
    pub account: Option<Me>,
    /// The server's poller, as of the last pull.
    pub status: Option<Status>,
    /// The last pull that went through.
    pub synced: Option<Synced>,
    /// The last failure, until something goes through.
    pub error: Option<String>,
    /// An upload to the server is on its way.
    pub uploading: bool,
    /// How the last upload ended.
    pub uploaded: Option<Result<u64, String>>,
}

impl State {
    /// Takes in an update from the backend.
    pub fn receive(&mut self, update: Update) {
        match update {
            Update::Connected { server, account } => {
                self.phase = Phase::Connected;
                self.server = Some(server);
                self.account = Some(account);
                self.error = None;
            }
            Update::Restored { server } => {
                self.phase = Phase::Connected;
                self.server = Some(server);
                self.error = None;
            }
            Update::Synced(synced) => {
                // A pull that ended after a disconnection says nothing now.
                if self.phase == Phase::Connected {
                    self.account = Some(synced.account.clone());
                    self.status = Some(synced.status.clone());
                    self.synced = Some(synced);
                    self.error = None;
                }
            }
            Update::Failed(error) => {
                if self.phase != Phase::Connected {
                    self.phase = Phase::Disconnected;
                }
                self.error = Some(error);
            }
            Update::Disconnected(reason) => {
                *self = Self {
                    error: reason,
                    ..Self::default()
                };
            }
            Update::Uploading => {
                self.uploading = true;
                self.uploaded = None;
            }
            Update::Uploaded(result) => {
                self.uploading = false;
                self.uploaded = Some(result);
            }
        }
    }

    /// Shows the browser step the moment Connect is used.
    pub fn authorizing(&mut self, server: String) {
        *self = Self {
            phase: Phase::Authorizing,
            server: Some(server),
            ..Self::default()
        };
    }

    /// Shows the check of a pasted or saved token the moment it starts.
    pub fn connecting(&mut self, server: String) {
        *self = Self {
            phase: Phase::Connecting,
            server: Some(server),
            ..Self::default()
        };
    }

    /// Shows the connection gone the moment Disconnect is used.
    pub fn disconnect(&mut self) {
        *self = Self::default();
    }

    /// The server connected to, once connected.
    pub fn connected(&self) -> Option<&str> {
        self.server
            .as_deref()
            .filter(|_| self.phase == Phase::Connected)
    }
}

/// The server's host, as the interface names it: `spotapi.simnjs.fr`
/// for `https://spotapi.simnjs.fr`.
pub fn host(server: &str) -> &str {
    let rest = server
        .strip_prefix("https://")
        .or_else(|| server.strip_prefix("http://"))
        .unwrap_or(server);
    rest.split('/').next().unwrap_or(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_trimmed_and_never_shown_in_debug() {
        let token = Token::new("  abc123  ").unwrap();
        assert_eq!(token.as_str(), "abc123");
        assert!(!format!("{token:?}").contains("abc123"));
        assert!(Token::new("   ").is_none());
        assert!(Token::new("two words").is_none());
        let connection = Connection {
            server: DEFAULT_SERVER.into(),
            token,
        };
        assert!(!format!("{connection:?}").contains("abc123"));
        assert!(connection.valid());
    }

    #[test]
    fn server_addresses_are_normalized_and_plain_http_stays_local() {
        assert_eq!(
            normalize_server(" https://spotapi.simnjs.fr/ ").unwrap(),
            "https://spotapi.simnjs.fr"
        );
        assert_eq!(
            normalize_server("https://example.com/stats").unwrap(),
            "https://example.com/stats"
        );
        assert_eq!(
            normalize_server("http://127.0.0.1:8080").unwrap(),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            normalize_server("http://localhost:8080/").unwrap(),
            "http://localhost:8080"
        );
        assert!(normalize_server("http://example.com").is_err());
        assert!(normalize_server("example.com").is_err());
        assert!(normalize_server("ftp://example.com").is_err());
        assert!(normalize_server("https://example.com/?a=b").is_err());
        assert!(normalize_server("").is_err());
        assert_eq!(host("https://spotapi.simnjs.fr"), "spotapi.simnjs.fr");
        assert_eq!(host("http://127.0.0.1:8080/api"), "127.0.0.1:8080");
    }

    #[test]
    fn me_and_status_read_the_servers_answers() {
        let me: Me =
            serde_json::from_str(r#"{"id": 3, "spotify_id": "simnjs", "display_name": "Simon"}"#)
                .unwrap();
        assert_eq!(
            me,
            Me {
                id: 3,
                spotify_id: "simnjs".into(),
                display_name: "Simon".into(),
            }
        );
        let status: Status = serde_json::from_str(
            r#"{"last_poll_ok_at": 1700000000000, "next_poll_at": 1700000120000,
                "poll_interval_secs": 120, "rate_limited_until": null,
                "last_error": null, "last_error_at": null,
                "last_played_at": 1699999990000, "plays_count": 4321}"#,
        )
        .unwrap();
        assert_eq!(status.last_poll_ok_at, Some(1_700_000_000_000));
        assert_eq!(status.next_poll_at, Some(1_700_000_120_000));
        assert_eq!(status.last_error, None);
        assert_eq!(status.last_played_at, Some(1_699_999_990_000));
        assert_eq!(status.plays_count, 4321);
        // A server that never polled yet answers nulls.
        let fresh: Status = serde_json::from_str(
            r#"{"last_poll_ok_at": null, "next_poll_at": null, "poll_interval_secs": null,
                "rate_limited_until": null, "last_error": "spotify refused the token",
                "last_error_at": 5, "last_played_at": null, "plays_count": 0}"#,
        )
        .unwrap();
        assert_eq!(fresh.last_poll_ok_at, None);
        assert_eq!(
            fresh.last_error.as_deref(),
            Some("spotify refused the token")
        );
    }

    #[test]
    fn a_history_page_reads_with_and_without_a_next_cursor() {
        let page: HistoryPage = serde_json::from_str(
            r#"{"items": [
                {"played_at": 1700000300123, "ms_played": 201000, "spotify_id": "t2",
                 "name": "Second", "artist_name": "Band", "album_name": "Album"},
                {"played_at": 1700000000000, "ms_played": 95000, "spotify_id": "t1",
                 "name": "First", "artist_name": "Band", "album_name": "Album"}
               ], "next_before": 1700000000000}"#,
        )
        .unwrap();
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.items[0].played_at, 1_700_000_300_123);
        assert_eq!(page.items[0].ms_played, 201_000);
        assert_eq!(page.items[1].spotify_id, "t1");
        assert_eq!(page.next_before, Some(1_700_000_000_000));
        let last: HistoryPage =
            serde_json::from_str(r#"{"items": [], "next_before": null}"#).unwrap();
        assert!(last.items.is_empty());
        assert_eq!(last.next_before, None);
    }

    fn me() -> Me {
        Me {
            id: 1,
            spotify_id: "me".into(),
            display_name: "Me".into(),
        }
    }

    #[test]
    fn the_state_shows_each_step_and_keeps_a_connection_through_failures() {
        let mut state = State::default();
        state.authorizing(DEFAULT_SERVER.into());
        assert_eq!(state.phase, Phase::Authorizing);
        assert_eq!(state.connected(), None);
        state.receive(Update::Failed("timed out".into()));
        assert_eq!(state.phase, Phase::Disconnected);
        assert_eq!(state.error.as_deref(), Some("timed out"));

        state.connecting(DEFAULT_SERVER.into());
        assert_eq!(state.error, None);
        state.receive(Update::Connected {
            server: DEFAULT_SERVER.into(),
            account: me(),
        });
        assert_eq!(state.connected(), Some(DEFAULT_SERVER));
        // A failed pull keeps the connection.
        state.receive(Update::Failed("offline".into()));
        assert_eq!(state.phase, Phase::Connected);
        let synced = Synced {
            at_ms: 10,
            added: 2,
            account: me(),
            status: Status::default(),
        };
        state.receive(Update::Synced(synced.clone()));
        assert_eq!(state.error, None);
        assert_eq!(state.synced, Some(synced.clone()));

        state.disconnect();
        assert_eq!(state.phase, Phase::Disconnected);
        // A pull that ends after it changes nothing.
        state.receive(Update::Synced(synced));
        assert_eq!(state.synced, None);
        state.receive(Update::Disconnected(Some("refused".into())));
        assert_eq!(state.error.as_deref(), Some("refused"));
    }
}
