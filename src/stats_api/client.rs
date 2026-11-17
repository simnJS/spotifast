//! Requests to the statistics server, with the account's token. Every one
//! runs on the backend's runtime, never on the UI thread; none logs the
//! token or carries it in a URL.

use std::io::Read as _;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::{HistoryPage, Me, Status, Token};

/// The largest export archive the server takes.
const MAX_ARCHIVE_BYTES: u64 = 200 * 1024 * 1024;
/// The largest single history file the server takes.
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// A whole export can take a while to send on a slow line, longer than
/// the shared client's thirty seconds.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// The most of the server's own error message shown.
const MESSAGE_CHARS: usize = 200;

/// Why a request to the server did not go through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The server no longer accepts the token (HTTP 401).
    Unauthorized,
    /// The server's account records another Spotify account than the one
    /// signed in here.
    OtherAccount,
    /// The file is larger than the server takes.
    TooLarge,
    /// The file could not be read.
    File(String),
    /// The server answered with this status and message.
    Status(u16, String),
    /// The server could not be reached.
    Network(String),
    /// The server's answer did not read.
    Invalid,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthorized => f.write_str(
                "The statistics server no longer accepts this connection. Connect again.",
            ),
            Self::OtherAccount => f.write_str(
                "The statistics server records another Spotify account than the one \
                 signed in here, so its plays were not added.",
            ),
            Self::TooLarge => f.write_str("The file is larger than the statistics server takes."),
            Self::File(error) => write!(f, "Couldn't read the file: {error}"),
            Self::Status(status, message) if message.is_empty() => {
                write!(f, "The statistics server answered HTTP {status}.")
            }
            Self::Status(status, message) => {
                write!(f, "The statistics server answered HTTP {status}: {message}")
            }
            Self::Network(error) => write!(f, "Couldn't reach the statistics server: {error}"),
            Self::Invalid => f.write_str("The statistics server's answer could not be read."),
        }
    }
}

impl std::error::Error for Error {}

/// The server's error body, `{"error": "..."}`.
#[derive(Deserialize)]
struct ErrorBody {
    #[serde(default)]
    error: String,
}

/// `GET /auth/tokens`.
#[derive(Deserialize)]
struct Tokens {
    #[serde(default)]
    tokens: Vec<TokenInfo>,
}

#[derive(Deserialize)]
struct TokenInfo {
    id: i64,
    #[serde(default)]
    current: bool,
}

/// `POST /import`: the counts of one file, or `totals` for an archive.
#[derive(Deserialize)]
struct ImportReport {
    #[serde(default)]
    imported: Option<u64>,
    #[serde(default)]
    totals: Option<ImportTotals>,
}

#[derive(Deserialize)]
struct ImportTotals {
    #[serde(default)]
    imported: u64,
}

impl ImportReport {
    fn imported(&self) -> u64 {
        self.totals
            .as_ref()
            .map(|totals| totals.imported)
            .or(self.imported)
            .unwrap_or(0)
    }
}

/// A connection to the server with its token. No `Debug`: the token is
/// a usable secret.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    server: String,
    token: Token,
}

/// Keeps the server's error status and message, and tells a refused
/// token apart.
async fn check(response: reqwest::Response) -> Result<reqwest::Response, Error> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(Error::Unauthorized);
    }
    let message = response
        .json::<ErrorBody>()
        .await
        .map(|body| body.error.chars().take(MESSAGE_CHARS).collect::<String>())
        .unwrap_or_default();
    Err(Error::Status(status.as_u16(), message))
}

fn network(error: reqwest::Error) -> Error {
    Error::Network(error.without_url().to_string())
}

impl Client {
    /// `server` as [`super::normalize_server`] gives it.
    pub fn new(http: reqwest::Client, server: String, token: Token) -> Self {
        Self {
            http,
            server,
            token,
        }
    }

    pub fn server(&self) -> &str {
        &self.server
    }

    async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, Error> {
        let response = self
            .http
            .get(format!("{}{path}", self.server))
            .query(query)
            .bearer_auth(self.token.as_str())
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(network)?;
        check(response)
            .await?
            .json::<T>()
            .await
            .map_err(|_| Error::Invalid)
    }

    /// `GET /me`: the account the token belongs to.
    pub async fn me(&self) -> Result<Me, Error> {
        self.get("/me", &[]).await
    }

    /// `GET /me/status`: the server's poller for the account.
    pub async fn status(&self) -> Result<Status, Error> {
        self.get("/me/status", &[]).await
    }

    /// `GET /history`: up to `limit` plays before `before`, newest first.
    pub async fn history(&self, before: Option<i64>, limit: u32) -> Result<HistoryPage, Error> {
        let mut query = vec![("limit", limit.to_string())];
        if let Some(before) = before {
            query.push(("before", before.to_string()));
        }
        self.get("/history", &query).await
    }

    /// Revokes the token on the server: finds which of the account's tokens
    /// it is (`GET /auth/tokens`), then `DELETE /auth/token/{id}`.
    pub async fn revoke(&self) -> Result<(), Error> {
        let tokens: Tokens = self.get("/auth/tokens", &[]).await?;
        let Some(current) = tokens.tokens.iter().find(|token| token.current) else {
            return Ok(());
        };
        let response = self
            .http
            .delete(format!("{}/auth/token/{}", self.server, current.id))
            .bearer_auth(self.token.as_str())
            .send()
            .await
            .map_err(network)?;
        check(response).await.map(|_| ())
    }

    /// Sends a streaming history to `POST /import`; the plays the server
    /// took in.
    pub async fn upload(&self, upload: Upload) -> Result<u64, Error> {
        let response = self
            .http
            .post(format!("{}/import", self.server))
            .bearer_auth(self.token.as_str())
            .header("Content-Type", upload.content_type)
            .header("Accept", "application/json")
            .timeout(UPLOAD_TIMEOUT)
            .body(upload.body)
            .send()
            .await
            .map_err(network)?;
        let report: ImportReport = check(response)
            .await?
            .json()
            .await
            .map_err(|_| Error::Invalid)?;
        Ok(report.imported())
    }
}

/// A streaming history file read for `POST /import`.
pub struct Upload {
    body: Vec<u8>,
    content_type: &'static str,
}

/// The content types the server takes.
const ZIP: &str = "application/zip";
const JSON: &str = "application/json";

/// The content type the server takes `path` as, from its first bytes and
/// its name: an export ZIP, or one extended history JSON file.
fn upload_kind(name: &str, first: &[u8]) -> Option<(&'static str, u64)> {
    if first.starts_with(b"PK\x03\x04") {
        Some((ZIP, MAX_ARCHIVE_BYTES))
    } else if name.starts_with("Streaming_History_Audio") && name.ends_with(".json") {
        Some((JSON, MAX_FILE_BYTES))
    } else {
        None
    }
}

/// Reads the streaming history at `path` for the server: an export ZIP as
/// it is, or a `Streaming_History_Audio_*.json` file on its own. `Ok(None)`
/// for any other file, which the server would refuse. Blocking: run it off
/// the runtime's threads.
pub fn read_upload(path: &Path) -> Result<Option<Upload>, Error> {
    let io = |error: std::io::Error| Error::File(error.to_string());
    let mut file = std::fs::File::open(path).map_err(io)?;
    let mut first = [0u8; 4];
    let read = file.read(&mut first).map_err(io)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let Some((content_type, limit)) = upload_kind(name, &first[..read]) else {
        return Ok(None);
    };
    if file.metadata().map_err(io)?.len() > limit {
        return Err(Error::TooLarge);
    }
    drop(file);
    let body = std::fs::read(path).map_err(io)?;
    Ok(Some(Upload { body, content_type }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uploads_send_an_archive_or_an_extended_history_file_only() {
        assert_eq!(
            upload_kind("my_spotify_data.zip", b"PK\x03\x04"),
            Some((ZIP, MAX_ARCHIVE_BYTES))
        );
        assert_eq!(
            upload_kind("Streaming_History_Audio_2023_4.json", b"[{\"t"),
            Some((JSON, MAX_FILE_BYTES))
        );
        // The older account-data history is not one the server reads.
        assert_eq!(upload_kind("StreamingHistory_music_0.json", b"[{\"e"), None);
        assert_eq!(upload_kind("notes.txt", b"hell"), None);
    }

    #[test]
    fn an_upload_reads_an_export_and_leaves_other_files() {
        let dir = std::env::temp_dir().join(format!(
            "spotifast-stats-server-upload-{:016x}",
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let extended = dir.join("Streaming_History_Audio_2024.json");
        std::fs::write(&extended, b"[]").unwrap();
        let upload = read_upload(&extended).unwrap().unwrap();
        assert_eq!(upload.content_type, JSON);
        assert_eq!(upload.body, b"[]");
        let other = dir.join("StreamingHistory_music_0.json");
        std::fs::write(&other, b"[]").unwrap();
        assert!(read_upload(&other).unwrap().is_none());
        assert!(matches!(
            read_upload(&dir.join("missing.zip")),
            Err(Error::File(_))
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_import_report_counts_a_file_or_an_archive() {
        let single: ImportReport = serde_json::from_str(
            r#"{"received": 10, "imported": 7, "skipped_short": 2,
                "skipped_not_track": 1, "skipped_duplicate": 0}"#,
        )
        .unwrap();
        assert_eq!(single.imported(), 7);
        let archive: ImportReport = serde_json::from_str(
            r#"{"files": [{"name": "a.json", "received": 3, "imported": 3,
                 "skipped_short": 0, "skipped_not_track": 0, "skipped_duplicate": 0}],
                "totals": {"received": 3, "imported": 3, "skipped_short": 0,
                 "skipped_not_track": 0, "skipped_duplicate": 0},
                "ignored_files": ["Userdata.json"]}"#,
        )
        .unwrap();
        assert_eq!(archive.imported(), 3);
    }

    #[test]
    fn errors_say_what_happened_without_the_token() {
        assert!(Error::Unauthorized.to_string().contains("Connect again"));
        assert_eq!(
            Error::Status(400, "bad file".into()).to_string(),
            "The statistics server answered HTTP 400: bad file"
        );
        assert_eq!(
            Error::Status(502, String::new()).to_string(),
            "The statistics server answered HTTP 502."
        );
    }
}
