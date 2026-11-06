//! Spotify Canvas: the short, silent, looping clip some tracks show in place
//! of their cover.
//!
//! [`fetch`] asks Pathfinder's `canvas` query which Canvas a track has,
//! [`Downloads`] fetches and holds its file, and [`Player`] decodes a video
//! Canvas on its own thread into an egui texture. Canvas files are plain,
//! DRM-free MP4s on Spotify's CDN; music videos are a different, protected
//! format and are not played here. Anything that fails leaves the caller on
//! the cover.

mod player;
mod video;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, anyhow, bail};
use librespot_core::Session;

use crate::api::ApiClient;
use crate::http::Http;

pub use player::Player;
pub use video::{Frame, Video};

/// Persisted hash of Pathfinder's `canvas` query, read from both Spotify's
/// desktop and web clients. Spotify rotates hashes between builds; an
/// outdated one fails the query and the caller keeps the cover.
const CANVAS_HASH: &str = "575138ab27cd5c1b3e54da54d0a7cc8d85485402de26340c2145f0f6bb5e7a9f";
/// Canvas videos are a few seconds of portrait video, usually 1 to 5 MB.
const MAX_CANVAS_BYTES: usize = 24 * 1024 * 1024;
/// Downloaded files held in memory, newest first, so returning to a track
/// does not fetch its Canvas again.
const HELD_BYTES: usize = 48 * 1024 * 1024;
/// Tracks whose Canvas, or lack of one, is remembered.
const LOOKUPS_KEPT: usize = 256;

/// Six 72x128 frames at 12 fps, High profile with B-frames, made with
/// `ffmpeg -f lavfi -i testsrc2=size=72x128:rate=12 -frames:v 6 -an
/// -c:v libx264 -profile:v high -pix_fmt yuv420p -bf 2 -g 6`. The tests
/// decode it, and the demo loops it in the Now Playing panel.
#[cfg(any(test, feature = "demo"))]
pub const FIXTURE: &[u8] = include_bytes!("fixture.mp4");

/// A track's Canvas as Pathfinder describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Canvas {
    /// `spotify:canvas:…`, when Spotify names one.
    pub uri: Option<String>,
    /// The file on Spotify's Canvas CDN.
    pub url: String,
    pub kind: Kind,
    pub file_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An MP4 to loop with [`Player`]. Spotify's `VIDEO`, `VIDEO_LOOPING`
    /// and `VIDEO_LOOPING_RANDOM`.
    Video,
    /// A still picture, drawn like a cover.
    Image,
    /// An animated GIF. Spotifast's image decoder does not read GIF, so the
    /// caller keeps the cover for it.
    Gif,
}

/// Which Canvas `track_uri` has, or `None` when it has none.
pub async fn fetch(
    session: &Session,
    web: &ApiClient,
    track_uri: &str,
) -> anyhow::Result<Option<Canvas>> {
    let login = session.login5().auth_token().await?;
    let client_token = session.spclient().client_token().await?;
    let body = web
        .partner_request(&login.access_token, &client_token, &query(track_uri))
        .await?;
    parse(&body)
}

fn query(track_uri: &str) -> serde_json::Value {
    serde_json::json!({
        "operationName": "canvas",
        "variables": {"trackUri": track_uri},
        "extensions": {"persistedQuery": {"version": 1, "sha256Hash": CANVAS_HASH}}
    })
}

/// Reads `data.trackUnion.canvas`. A missing or null canvas, or one of a type
/// this module does not know, is no Canvas; an answer with no track at all
/// is an error, so the caller can tell a failed query from a plain track.
fn parse(body: &serde_json::Value) -> anyhow::Result<Option<Canvas>> {
    let track = body
        .pointer("/data/trackUnion")
        .filter(|track| track.is_object())
        .ok_or_else(|| anyhow!("Spotify returned no track for the Canvas query"))?;
    let Some(canvas) = track.get("canvas").filter(|canvas| canvas.is_object()) else {
        return Ok(None);
    };
    let text = |key: &str| {
        canvas
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let kind = match text("type").as_deref() {
        Some("VIDEO" | "VIDEO_LOOPING" | "VIDEO_LOOPING_RANDOM") => Kind::Video,
        Some("IMAGE") => Kind::Image,
        Some("GIF") => Kind::Gif,
        _ => return Ok(None),
    };
    let Some(url) = text("url").filter(|url| url.starts_with("https://")) else {
        return Ok(None);
    };
    Ok(Some(Canvas {
        uri: text("uri"),
        url,
        kind,
        file_id: text("fileId"),
    }))
}

/// Which Canvas recent tracks have, and the files of the recent ones, so a
/// track played again asks Spotify nothing.
pub struct Library {
    downloads: Downloads,
    lookups: Mutex<VecDeque<(String, Option<Canvas>)>>,
}

impl Library {
    pub fn new(http: Http) -> Self {
        Self {
            downloads: Downloads::new(http),
            lookups: Mutex::new(VecDeque::new()),
        }
    }

    /// The video to loop for `track_uri`, or `None` when the track has no
    /// Canvas or a still or GIF one, which leaves the cover.
    pub async fn video(
        &self,
        session: &Session,
        web: &ApiClient,
        track_uri: &str,
    ) -> anyhow::Result<Option<Arc<[u8]>>> {
        let canvas = match self.lookup(track_uri) {
            Some(canvas) => canvas,
            None => {
                let canvas = fetch(session, web, track_uri).await?;
                self.remember(track_uri, canvas.clone());
                canvas
            }
        };
        match canvas {
            Some(Canvas {
                kind: Kind::Video,
                url,
                ..
            }) => Ok(Some(self.downloads.get(&url).await?)),
            _ => Ok(None),
        }
    }

    /// The remembered answer for `track_uri`: `Some(None)` for a track known
    /// to have no Canvas, `None` when it was never asked.
    fn lookup(&self, track_uri: &str) -> Option<Option<Canvas>> {
        let lookups = self.lookups.lock().unwrap_or_else(|p| p.into_inner());
        lookups
            .iter()
            .find(|(uri, _)| uri == track_uri)
            .map(|(_, canvas)| canvas.clone())
    }

    fn remember(&self, track_uri: &str, canvas: Option<Canvas>) {
        let mut lookups = self.lookups.lock().unwrap_or_else(|p| p.into_inner());
        lookups.retain(|(uri, _)| uri != track_uri);
        lookups.push_front((track_uri.to_string(), canvas));
        lookups.truncate(LOOKUPS_KEPT);
    }
}

/// Downloads Canvas files and keeps the recent ones in memory.
pub struct Downloads {
    http: Http,
    held: Mutex<VecDeque<(String, Arc<[u8]>)>>,
}

impl Downloads {
    pub fn new(http: Http) -> Self {
        Self {
            http,
            held: Mutex::new(VecDeque::new()),
        }
    }

    /// The file at `url`, from memory when it was fetched recently.
    pub async fn get(&self, url: &str) -> anyhow::Result<Arc<[u8]>> {
        if let Some(bytes) = self.held(url) {
            return Ok(bytes);
        }
        let bytes = self.download(url).await?;
        self.keep(url, Arc::clone(&bytes));
        Ok(bytes)
    }

    fn held(&self, url: &str) -> Option<Arc<[u8]>> {
        let mut held = self.held.lock().unwrap_or_else(|p| p.into_inner());
        let index = held.iter().position(|(held, _)| held == url)?;
        let entry = held.remove(index)?;
        let bytes = Arc::clone(&entry.1);
        held.push_front(entry);
        Some(bytes)
    }

    fn keep(&self, url: &str, bytes: Arc<[u8]>) {
        let mut held = self.held.lock().unwrap_or_else(|p| p.into_inner());
        held.retain(|(held, _)| held != url);
        held.push_front((url.to_string(), bytes));
        let mut total = 0;
        held.retain(|(_, bytes)| {
            total += bytes.len();
            total <= HELD_BYTES
        });
    }

    async fn download(&self, url: &str) -> anyhow::Result<Arc<[u8]>> {
        if !url.starts_with("https://") {
            bail!("Canvas URL is not HTTPS");
        }
        let mut response = self
            .http
            .client()
            .map_err(|error| anyhow!(error))?
            .get(url)
            .send()
            .await
            .map_err(|error| error.without_url())
            .context("Canvas download failed")?;
        if !response.status().is_success() {
            bail!("Canvas download failed: {}", response.status());
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_CANVAS_BYTES as u64)
        {
            bail!("Canvas is too large");
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| error.without_url())
            .context("Canvas download failed")?
        {
            if bytes.len() + chunk.len() > MAX_CANVAS_BYTES {
                bail!("Canvas is too large");
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(Arc::from(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_video_canvas() {
        let body = serde_json::json!({"data": {"trackUnion": {
            "__typename": "Track",
            "canvas": {
                "__typename": "Canvas",
                "fileId": "1b2e3a4f5c6d7e8f9a0b1c2d3e4f5a6b",
                "type": "VIDEO_LOOPING_RANDOM",
                "uri": "spotify:canvas:4nQ2KQ9bC2cIZg8lXZp1rV",
                "url": "https://canvaz.scdn.co/upload/artist/0OdUWJ0sBjDrqHygGUXeCF/video/4f4a9b6e.cnvs.mp4"
            }
        }}});
        assert_eq!(
            parse(&body).unwrap(),
            Some(Canvas {
                uri: Some("spotify:canvas:4nQ2KQ9bC2cIZg8lXZp1rV".into()),
                url: "https://canvaz.scdn.co/upload/artist/0OdUWJ0sBjDrqHygGUXeCF/video/4f4a9b6e.cnvs.mp4".into(),
                kind: Kind::Video,
                file_id: Some("1b2e3a4f5c6d7e8f9a0b1c2d3e4f5a6b".into()),
            })
        );
    }

    #[test]
    fn reads_image_and_gif_canvases() {
        for (kind, expected) in [("IMAGE", Kind::Image), ("GIF", Kind::Gif)] {
            let body = serde_json::json!({"data": {"trackUnion": {"canvas": {
                "type": kind, "url": "https://canvaz.scdn.co/upload/a.jpg", "fileId": ""
            }}}});
            let canvas = parse(&body).unwrap().expect("a canvas");
            assert_eq!(canvas.kind, expected);
            assert_eq!(canvas.file_id, None);
            assert_eq!(canvas.uri, None);
        }
    }

    #[test]
    fn a_track_without_canvas_has_none() {
        for canvas in [
            serde_json::Value::Null,
            serde_json::json!({"type": "UNKNOWN", "url": "https://canvaz.scdn.co/x"}),
            serde_json::json!({"type": "VIDEO", "url": ""}),
            serde_json::json!({"type": "VIDEO", "url": "http://canvaz.scdn.co/x.mp4"}),
        ] {
            let body = serde_json::json!({"data": {"trackUnion": {"canvas": canvas}}});
            assert_eq!(parse(&body).unwrap(), None);
        }
        let body = serde_json::json!({"data": {"trackUnion": {"__typename": "Track"}}});
        assert_eq!(parse(&body).unwrap(), None);
    }

    #[test]
    fn an_answer_without_a_track_is_an_error() {
        assert!(parse(&serde_json::json!({"data": {"trackUnion": null}})).is_err());
        assert!(parse(&serde_json::json!({"errors": [{"message": "x"}]})).is_err());
    }

    #[test]
    fn the_query_names_the_track() {
        let query = query("spotify:track:4uLU6hMCjMI75M1A2tKUQC");
        assert_eq!(query["operationName"], "canvas");
        assert_eq!(
            query["variables"]["trackUri"],
            "spotify:track:4uLU6hMCjMI75M1A2tKUQC"
        );
        assert_eq!(
            query["extensions"]["persistedQuery"]["sha256Hash"],
            CANVAS_HASH
        );
    }

    #[test]
    fn remembers_recent_tracks_with_or_without_a_canvas() {
        let library = Library::new(Http::default());
        assert_eq!(library.lookup("spotify:track:a"), None);
        library.remember("spotify:track:a", None);
        assert_eq!(library.lookup("spotify:track:a"), Some(None));
        let canvas = Canvas {
            uri: None,
            url: "https://canvaz.scdn.co/a.mp4".into(),
            kind: Kind::Video,
            file_id: None,
        };
        library.remember("spotify:track:a", Some(canvas.clone()));
        assert_eq!(library.lookup("spotify:track:a"), Some(Some(canvas)));
        for index in 0..LOOKUPS_KEPT {
            library.remember(&format!("spotify:track:{index}"), None);
        }
        assert_eq!(library.lookup("spotify:track:a"), None, "the oldest goes");
        assert_eq!(library.lookup("spotify:track:0"), Some(None));
    }

    #[test]
    fn keeps_recent_downloads_within_the_cap() {
        let downloads = Downloads::new(Http::default());
        let big: Arc<[u8]> = Arc::from(vec![0u8; HELD_BYTES / 2]);
        downloads.keep("a", Arc::clone(&big));
        downloads.keep("b", Arc::clone(&big));
        assert!(downloads.held("a").is_some());
        // "a" is now the most recent, so "b" is the one to go.
        downloads.keep("c", Arc::clone(&big));
        assert!(downloads.held("a").is_some());
        assert!(downloads.held("b").is_none());
        assert!(downloads.held("c").is_some());
    }
}
