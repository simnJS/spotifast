//! Requests to the statistics server, with the account's token. Every one
//! runs on the backend's runtime, or on a blocking thread for an import,
//! never on the UI thread; none logs the token or carries it in a URL.

use std::path::Path;

use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::import::{self, Report, Sent};
use super::stats::{
    self, Answer, Hours, Item, ItemFigures, ItemStats, Kind, List, Period, RangeStats, TopAlbum,
    TopArtist, TopGenre, TopTrack, Totals,
};
use super::{Me, Status, Token};

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
    /// The file is not one the server reads.
    NotHistory,
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
                 signed in here, so its stats are not shown.",
            ),
            Self::TooLarge => f.write_str("The file is larger than the statistics server takes."),
            Self::File(error) => write!(f, "Couldn't read the file: {error}"),
            Self::NotHistory => f.write_str(
                "Your statistics server takes Spotify's export ZIP, or one of its \
                 Streaming_History_Audio files.",
            ),
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

/// [`check`] for an answer of the blocking client.
fn check_blocking(
    response: reqwest::blocking::Response,
) -> Result<reqwest::blocking::Response, Error> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(Error::Unauthorized);
    }
    let message = response
        .json::<ErrorBody>()
        .map(|body| body.error.chars().take(MESSAGE_CHARS).collect::<String>())
        .unwrap_or_default();
    Err(Error::Status(status.as_u16(), message))
}

/// Whether `id` can be a Spotify ID, which is safe in a path as it is.
fn spotify_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric())
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

    /// Everything the Stats page shows for `period`, with the days and
    /// hours in this computer's time zone, `zone`. The routes are asked
    /// side by side; any failure fails the whole range.
    pub async fn range(&self, period: Period, zone: &str) -> Result<RangeStats, Error> {
        let range = || ("range", period.param().to_owned());
        let top = |limit: u32| [range(), ("limit", limit.to_string())];
        let summary = [range()];
        let (top_artists, top_tracks, top_albums, top_genres) = (
            top(stats::ARTISTS),
            top(stats::TRACKS),
            top(stats::ALBUMS),
            top(stats::GENRES),
        );
        let days = [
            range(),
            ("bucket", "day".to_owned()),
            ("tz", zone.to_owned()),
        ];
        let hours = [range(), ("tz", zone.to_owned())];
        let lifetime = async {
            if period == Period::Lifetime {
                Ok(None)
            } else {
                self.get::<Totals>("/stats/summary", &[("range", "lifetime".to_owned())])
                    .await
                    .map(Some)
            }
        };
        let (totals, lifetime, artists, tracks, albums, genres, days, hours) = tokio::try_join!(
            self.get::<Totals>("/stats/summary", &summary),
            lifetime,
            self.get::<List<TopArtist>>("/stats/top/artists", &top_artists),
            self.get::<List<TopTrack>>("/stats/top/tracks", &top_tracks),
            self.get::<List<TopAlbum>>("/stats/top/albums", &top_albums),
            self.get::<List<TopGenre>>("/stats/top/genres", &top_genres),
            self.get::<List<stats::DayBucket>>("/stats/listening-time", &days),
            self.get::<Hours>("/stats/hours", &hours),
        )?;
        Ok(RangeStats {
            period,
            lifetime_plays: lifetime.as_ref().map_or(totals.plays, |all| all.plays),
            totals,
            artists: artists.into_vec(),
            tracks: tracks.into_vec(),
            albums: albums.into_vec(),
            genres: genres.into_vec(),
            days: days.into_vec(),
            hours: hours.hours,
            zone: zone.to_owned(),
        })
    }

    /// `GET /stats/item/{kind}/{id}`; `None` when the server has no play
    /// of it (HTTP 404).
    async fn item_stats(&self, item: &Item) -> Result<Option<ItemStats>, Error> {
        if !spotify_id(&item.id) {
            return Ok(None);
        }
        let path = format!("/stats/item/{}/{}", item.kind.path(), item.id);
        match self.get::<ItemStats>(&path, &[]).await {
            Ok(stats) => Ok(Some(stats)),
            Err(Error::Status(404, _)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// An item's figures; for a song, with its main `artist`'s all-time
    /// rank.
    pub async fn item(&self, item: &Item, artist: Option<&str>) -> Result<ItemFigures, Error> {
        let artist_rank = async {
            let Some(id) = artist.filter(|_| item.kind == Kind::Track) else {
                return Ok(None);
            };
            let artist = Item {
                kind: Kind::Artist,
                id: id.to_owned(),
            };
            Ok(self
                .item_stats(&artist)
                .await?
                .and_then(|stats| stats.rank_lifetime))
        };
        let (stats, artist_rank) = tokio::try_join!(self.item_stats(item), artist_rank)?;
        Ok(ItemFigures { stats, artist_rank })
    }

    /// What a page asked for.
    pub async fn fetch(&self, fetch: &stats::Fetch, zone: &str) -> Result<Answer, Error> {
        Ok(match fetch {
            stats::Fetch::Range(period) => {
                Answer::Range(std::sync::Arc::new(self.range(*period, zone).await?))
            }
            stats::Fetch::Item { item, artist } => Answer::Item(std::sync::Arc::new(
                self.item(item, artist.as_deref()).await?,
            )),
        })
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

    /// Sends the streaming history at `path` to `POST /import` through
    /// `http`, reporting the bytes sent as they go, and reads the server's
    /// report. Blocking: run it on a blocking thread, with a client built
    /// there.
    pub fn import(
        &self,
        http: &reqwest::blocking::Client,
        path: &Path,
        progress: impl FnMut(Sent) + Send + 'static,
    ) -> Result<Report, Error> {
        let mut progress = progress;
        let upload = import::open(path)?.ok_or(Error::NotHistory)?;
        let name = upload.name.clone();
        progress(Sent {
            bytes: 0,
            total: upload.len,
        });
        let body = reqwest::blocking::Body::sized(
            import::Progress::new(upload.file, upload.len, progress),
            upload.len,
        );
        let response = http
            .post(format!("{}/import", self.server))
            .bearer_auth(self.token.as_str())
            .header("Content-Type", upload.content_type)
            .header("Accept", "application/json")
            .timeout(import::TIMEOUT)
            .body(body)
            .send()
            .map_err(network)?;
        let body = check_blocking(response)?.bytes().map_err(network)?;
        Report::parse(&body, &name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::sync::{Arc, Mutex};

    /// What the one-request server below received.
    #[derive(Debug, Default)]
    struct Received {
        request_line: String,
        headers: Vec<String>,
        body: Vec<u8>,
    }

    /// Answers one request on a port of this computer with `status` and
    /// `body`, and keeps what it received.
    fn serve_once(
        status: &'static str,
        body: &'static str,
    ) -> (String, std::thread::JoinHandle<Received>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut received = Received::default();
            reader.read_line(&mut received.request_line).unwrap();
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let line = line.trim_end().to_owned();
                if line.is_empty() {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().unwrap();
                }
                received.headers.push(line.to_ascii_lowercase());
            }
            received.body = vec![0; length];
            reader.read_exact(&mut received.body).unwrap();
            let mut stream = stream;
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            stream.flush().unwrap();
            received
        });
        (address, handle)
    }

    fn client(server: String) -> Client {
        Client::new(
            reqwest::Client::new(),
            server,
            Token::new("secret-token").unwrap(),
        )
    }

    fn blocking() -> reqwest::blocking::Client {
        reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
    }

    fn export(dir: &std::path::Path, bytes: usize) -> std::path::PathBuf {
        let path = dir.join("my_spotify_data.zip");
        let mut data = b"PK\x03\x04".to_vec();
        data.resize(bytes, 9);
        std::fs::write(&path, data).unwrap();
        path
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "spotifast-stats-server-{name}-{:016x}",
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_import_streams_the_file_reports_its_bytes_and_reads_the_report() {
        let (server, handle) = serve_once(
            "200 OK",
            r#"{"files": [{"name": "Streaming_History_Audio_2024.json", "received": 3,
                "imported": 2, "skipped_short": 1, "skipped_not_track": 0,
                "skipped_duplicate": 0}],
               "totals": {"received": 3, "imported": 2, "skipped_short": 1,
                "skipped_not_track": 0, "skipped_duplicate": 0},
               "ignored_files": ["ReadMeFirst.pdf"]}"#,
        );
        let dir = temp_dir("import");
        let path = export(&dir, 300_000);
        let sent = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&sent);
        let report = client(server)
            .import(&blocking(), &path, move |progress| {
                seen.lock().unwrap().push(progress)
            })
            .unwrap();
        let received = handle.join().unwrap();
        assert!(received.request_line.starts_with("POST /import "));
        assert!(
            received
                .headers
                .contains(&"content-type: application/zip".to_owned())
        );
        assert!(
            received
                .headers
                .contains(&"authorization: bearer secret-token".to_owned())
        );
        assert_eq!(received.body.len(), 300_000);
        assert!(received.body.starts_with(b"PK\x03\x04"));
        let sent = sent.lock().unwrap();
        let last = sent.last().unwrap();
        assert_eq!((last.bytes, last.total), (300_000, 300_000));
        // Once before the first byte, then a hundred times at most.
        assert_eq!(sent[0].bytes, 0);
        assert!(sent.len() <= 102);
        assert_eq!(report.totals.imported, 2);
        assert_eq!(report.files[0].counts.skipped_short, 1);
        assert_eq!(report.ignored_files, vec!["ReadMeFirst.pdf".to_owned()]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_import_says_why_the_server_refused_it() {
        let (server, handle) = serve_once("400 Bad Request", r#"{"error": "invalid zip"}"#);
        let dir = temp_dir("import-refused");
        let path = export(&dir, 1_000);
        let refused = client(server).import(&blocking(), &path, |_| {});
        handle.join().unwrap();
        assert_eq!(refused, Err(Error::Status(400, "invalid zip".into())));
        let (server, handle) = serve_once("401 Unauthorized", "{}");
        let refused = client(server).import(&blocking(), &path, |_| {});
        handle.join().unwrap();
        assert_eq!(refused, Err(Error::Unauthorized));
        // A file the server does not read is not sent.
        let other = dir.join("StreamingHistory_music_0.json");
        std::fs::write(&other, b"[]").unwrap();
        assert_eq!(
            client("http://127.0.0.1:9".into()).import(&blocking(), &other, |_| {}),
            Err(Error::NotHistory)
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn only_a_spotify_id_goes_into_an_items_path() {
        assert!(spotify_id("4uLU6hMCjMI75M1A2tKUQC"));
        assert!(!spotify_id(""));
        assert!(!spotify_id("../me"));
        assert!(!spotify_id("a b"));
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
