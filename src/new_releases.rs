//! The latest releases from the artists the account follows, for Home's
//! **New releases** shelf, the page it opens and the What's New page.
//!
//! Spotify's own apps read them from the private Pathfinder feed behind their
//! "What's New" bell (`queryWhatsNewFeed`), with the playback session's tokens.
//! Spotify can change that query without notice, so the Web API's followed
//! artists and their latest albums stand in when it fails. The feed is only
//! read: the mutation that marks its items as seen is never sent, so reading
//! here leaves the bell in Spotify's apps as it was.

use std::time::{Duration, Instant};

use librespot_core::Session;

use crate::api::ApiError;
use crate::api::models::{Album, ArtistRef, Episode, Image, Show};
use crate::http::Http;

const ENDPOINT: &str = "https://api-partner.spotify.com/pathfinder/v2/query";
const QUERY_HASH: &str = "d889c8c936ab192af8ced595427f5ba2acdf63478fdc0a181c8d477f8322630e";
const TIMEOUT: Duration = Duration::from_secs(30);

/// The most releases the shelf and its page hold, as many as the feed's
/// first page, which is all Spotify's own apps read.
pub const LIMIT: usize = 50;

/// The followed artists the Web API fallback asks about: one request each.
pub const FALLBACK_ARTISTS: usize = 50;

/// How long the Web API stand-in's answer is kept: it costs a request for
/// each followed artist, and releases come out once a day at most.
const FALLBACK_AGE: Duration = Duration::from_secs(60 * 60);

/// What one answer from the feed holds, each list newest first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Feed {
    /// Albums, singles and EPs, without repeats, at most [`LIMIT`].
    pub albums: Vec<Album>,
    /// Podcast episodes, with their show, without repeats.
    pub episodes: Vec<Episode>,
}

/// The feed reader, with the Web API stand-in's last answer for one account.
pub struct NewReleases {
    http: Http,
    fallback: tokio::sync::Mutex<Option<(String, Instant, Vec<Album>)>>,
}

impl NewReleases {
    pub fn new(http: Http) -> Self {
        Self {
            http,
            fallback: tokio::sync::Mutex::default(),
        }
    }

    /// Reads the account's feed of new releases over the playback session.
    pub async fn feed(&self, session: &Session) -> Result<Feed, ApiError> {
        tokio::time::timeout(TIMEOUT, read_feed(session, &self.http))
            .await
            .map_err(|_| {
                ApiError::Network("New releases took too long to load. Try again.".into())
            })?
    }

    /// The Web API stand-in's answer for `account`, while it is recent.
    pub async fn cached_fallback(&self, account: &str) -> Option<Vec<Album>> {
        self.fallback
            .lock()
            .await
            .as_ref()
            .filter(|(owner, at, _)| owner == account && at.elapsed() < FALLBACK_AGE)
            .map(|(_, _, albums)| albums.clone())
    }

    pub async fn keep_fallback(&self, account: &str, albums: &[Album]) {
        *self.fallback.lock().await = Some((account.to_owned(), Instant::now(), albums.to_vec()));
    }
}

async fn read_feed(session: &Session, http: &Http) -> Result<Feed, ApiError> {
    let client = http.client().map_err(ApiError::Network)?;
    let token = session
        .login5()
        .auth_token()
        .await
        .map_err(|_| session_error())?;
    let client_token = session
        .spclient()
        .client_token()
        .await
        .map_err(|_| session_error())?;
    let response = client
        .post(ENDPOINT)
        .bearer_auth(&token.access_token)
        .header("client-token", &client_token)
        .json(&query())
        .send()
        .await
        .map_err(|error| ApiError::Network(error.without_url().to_string()))?;
    let status = response.status();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(ApiError::RateLimited);
    }
    if !status.is_success() {
        return Err(ApiError::Status {
            status: status.as_u16(),
            message: "Spotify couldn't load new releases through the playback session.".into(),
        });
    }
    let body = response
        .json::<serde_json::Value>()
        .await
        .map_err(|_| response_error())?;
    parse(&body)
}

fn session_error() -> ApiError {
    ApiError::Network("Couldn't authorize the playback session for new releases. Try again.".into())
}

fn response_error() -> ApiError {
    ApiError::Decode("New releases are unavailable in Spotify's response. Try again later.".into())
}

/// The first page of the feed, albums only, as Spotify's web player asks
/// for it. [`parse`] still reads the episodes an answer holds, and the What's
/// New page adds the followed shows' newest episodes from the Web API.
fn query() -> serde_json::Value {
    serde_json::json!({
        "operationName": "queryWhatsNewFeed",
        "variables": {
            "offset": 0,
            "limit": LIMIT,
            "onlyUnPlayedItems": false,
            "includedContentTypes": ["ALBUM"],
            "includeEpisodeContentRatingsV2": false
        },
        "extensions": {"persistedQuery": {"version": 1, "sha256Hash": QUERY_HASH}}
    })
}

/// The items of a feed answer, newest first: the releases, as the shelf
/// has always read them, and the podcast episodes. Any other kind, such as
/// an event notice or one Spotify adds later, is passed over; an answer
/// without the feed at all is an error rather than an empty shelf.
fn parse(body: &serde_json::Value) -> Result<Feed, ApiError> {
    if body.get("errors").is_some_and(|errors| {
        !errors.is_null() && errors.as_array().is_none_or(|errors| !errors.is_empty())
    }) {
        return Err(response_error());
    }
    let items = body
        .pointer("/data/whatsNewFeedItems/items")
        .and_then(|items| items.as_array())
        .ok_or_else(response_error)?;
    let mut albums = Vec::new();
    let mut episodes: Vec<Episode> = Vec::new();
    for item in items {
        let Some(data) = item.pointer("/content/data") else {
            continue;
        };
        match item
            .pointer("/content/__typename")
            .and_then(|kind| kind.as_str())
        {
            Some("AlbumResponseWrapper") => {
                if let Some(album) = album(data)
                    && !albums.iter().any(|known: &Album| known.uri == album.uri)
                {
                    albums.push(album);
                }
            }
            Some("EpisodeOrChapterResponseWrapper") => {
                if let Some(episode) = episode(data)
                    && !episodes.iter().any(|known| known.uri == episode.uri)
                {
                    episodes.push(episode);
                }
            }
            _ => {}
        }
    }
    episodes.sort_by(|a, b| b.release_date.cmp(&a.release_date));
    Ok(Feed {
        albums: newest_first(albums),
        episodes,
    })
}

/// A 22-character base62 ID behind `prefix` in `uri`.
fn spotify_id<'a>(uri: &'a str, prefix: &str) -> Option<&'a str> {
    uri.strip_prefix(prefix)
        .filter(|id| id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric()))
}

fn images(sources: Option<&serde_json::Value>) -> Vec<Image> {
    sources
        .and_then(|sources| sources.as_array())
        .into_iter()
        .flatten()
        .filter_map(|source| serde_json::from_value::<Image>(source.clone()).ok())
        .filter(|image| image.url.starts_with("https://"))
        .collect()
}

/// A podcast episode card, with the show it belongs to when the card names
/// it. Audiobook chapters, which share the card's wrapper, are left out.
fn episode(data: &serde_json::Value) -> Option<Episode> {
    if data.get("__typename").and_then(|kind| kind.as_str()) != Some("Episode") {
        return None;
    }
    let uri = data.get("uri")?.as_str()?;
    let id = spotify_id(uri, "spotify:episode:")?;
    let name = data.get("name")?.as_str()?;
    let release_date = data
        .pointer("/releaseDate/isoString")
        .and_then(|date| date.as_str())
        .map(|date| release_date(date, data.pointer("/releaseDate/precision")));
    let duration_ms = data
        .pointer("/duration/totalMilliseconds")
        .and_then(serde_json::Value::as_u64)
        .and_then(|ms| u32::try_from(ms).ok())
        .unwrap_or(0);
    let show = data.pointer("/podcastV2/data").and_then(|show| {
        let uri = show.get("uri")?.as_str()?;
        let id = spotify_id(uri, "spotify:show:")?;
        Some(Show {
            id: id.to_owned(),
            uri: uri.to_owned(),
            name: show
                .get("name")
                .and_then(|name| name.as_str())
                .unwrap_or_default()
                .to_owned(),
            images: images(show.pointer("/coverArt/sources")),
            ..Show::default()
        })
    });
    Some(Episode {
        id: id.to_owned(),
        uri: uri.to_owned(),
        name: name.to_owned(),
        duration_ms,
        images: images(data.pointer("/coverArt/sources")),
        release_date,
        show,
        ..Episode::default()
    })
}

fn album(data: &serde_json::Value) -> Option<Album> {
    if data.get("__typename").and_then(|kind| kind.as_str()) != Some("Album") {
        return None;
    }
    let uri = data.get("uri")?.as_str()?;
    let id = spotify_id(uri, "spotify:album:")?;
    let name = data.get("name")?.as_str()?;
    let album_type = data
        .get("albumType")
        .and_then(|kind| kind.as_str())
        .map(|kind| kind.to_ascii_lowercase());
    let release_date = data
        .pointer("/date/isoString")
        .and_then(|date| date.as_str())
        .map(|date| release_date(date, data.pointer("/date/precision")));
    let artists = data
        .pointer("/artists/items")
        .and_then(|items| items.as_array())
        .into_iter()
        .flatten()
        .filter_map(|artist| {
            let name = artist.pointer("/profile/name")?.as_str()?;
            let uri = artist.get("uri").and_then(|uri| uri.as_str());
            Some(ArtistRef {
                id: uri
                    .and_then(|uri| uri.strip_prefix("spotify:artist:"))
                    .map(str::to_owned),
                name: name.to_owned(),
                uri: uri.map(str::to_owned),
            })
        })
        .collect();
    let images = images(data.pointer("/coverArt/sources"));
    Some(Album {
        id: id.to_owned(),
        uri: uri.to_owned(),
        name: name.to_owned(),
        album_type,
        release_date,
        artists,
        images,
        ..Album::default()
    })
}

/// A feed date in the Web API's form: `2026-10-16`, or `2026-10` and
/// `2026` when Spotify only knows the month or the year.
fn release_date(iso: &str, precision: Option<&serde_json::Value>) -> String {
    let day = iso.get(..10).unwrap_or(iso);
    match precision.and_then(|precision| precision.as_str()) {
        Some("YEAR") => day.get(..4).unwrap_or(day).to_owned(),
        Some("MONTH") => day.get(..7).unwrap_or(day).to_owned(),
        _ => day.to_owned(),
    }
}

/// How long the What's New page and its bell keep an item, as Spotify's
/// own bell does.
pub const KEPT_DAYS: i64 = 60;

/// The most items the What's New page lists, as Spotify's own bell does.
pub const KEPT_ITEMS: usize = 100;

/// The day a release or an episode came out, from a Web API date: `YYYY-MM-DD`,
/// or the first of the month or the year when Spotify knows no more.
pub fn release_day(date: &str) -> Option<jiff::civil::Date> {
    let mut parts = date.get(..10).unwrap_or(date).splitn(3, '-');
    let year = parts.next()?.parse().ok()?;
    let month = parts.next().map_or(Some(1), |month| month.parse().ok())?;
    let day = parts.next().map_or(Some(1), |day| day.parse().ok())?;
    jiff::civil::Date::new(year, month, day).ok()
}

/// Whether an item released on `date` is still kept on `today`. An item
/// without a readable date is not.
pub fn is_recent(date: Option<&str>, today: jiff::civil::Date) -> bool {
    let oldest = today
        .checked_sub(jiff::Span::new().days(KEPT_DAYS))
        .unwrap_or(today);
    date.and_then(release_day).is_some_and(|day| day >= oldest)
}

/// Releases sorted newest first, ties kept in the order given, at most
/// [`LIMIT`] of them. Web API dates are `YYYY`, `YYYY-MM` or `YYYY-MM-DD`,
/// which sort as text; a release without a date goes last.
pub fn newest_first(mut albums: Vec<Album>) -> Vec<Album> {
    albums.sort_by(|a, b| b.release_date.cmp(&a.release_date));
    albums.truncate(LIMIT);
    albums
}

/// The Web API stand-in: each followed artist's latest albums and singles,
/// merged, without repeats, newest first.
pub fn merge_artist_releases(pages: Vec<Vec<Album>>) -> Vec<Album> {
    let mut albums: Vec<Album> = Vec::new();
    for album in pages.into_iter().flatten() {
        if matches!(
            album.album_group.as_deref(),
            Some("appears_on" | "compilation")
        ) {
            continue;
        }
        if !albums.iter().any(|known| known.uri == album.uri) {
            albums.push(album);
        }
    }
    newest_first(albums)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn feed_item(uri: &str, name: &str, kind: &str, date: &str) -> serde_json::Value {
        json!({
            "id": "music_release:1",
            "state": {"state": "NEW"},
            "content": {
                "__typename": "AlbumResponseWrapper",
                "data": {
                    "__typename": "Album",
                    "uri": uri,
                    "name": name,
                    "albumType": kind,
                    "date": {"isoString": date, "precision": "DAY"},
                    "artists": {"items": [{"uri": "spotify:artist:0000000000000000000001", "profile": {"name": "Artist"}}]},
                    "coverArt": {"sources": [{"url": "https://i.scdn.co/image/a", "width": 300, "height": 300}]}
                }
            }
        })
    }

    #[test]
    fn reads_the_feeds_albums_newest_first() {
        let body = json!({"data": {"whatsNewFeedItems": {"items": [
            feed_item("spotify:album:1111111111111111111111", "Older", "ALBUM", "2026-10-02T00:00:00Z"),
            feed_item("spotify:album:2222222222222222222222", "Newest", "SINGLE", "2026-10-16T00:00:00Z"),
            {"content": {"__typename": "EpisodeOrChapterResponseWrapper", "data": {"__typename": "Episode"}}},
            feed_item("spotify:album:2222222222222222222222", "Newest", "SINGLE", "2026-10-16T00:00:00Z"),
        ], "totalCount": 4}}});
        let albums = parse(&body).unwrap().albums;
        assert_eq!(
            albums
                .iter()
                .map(|album| album.name.as_str())
                .collect::<Vec<_>>(),
            ["Newest", "Older"]
        );
        let newest = &albums[0];
        assert_eq!(newest.id, "2222222222222222222222");
        assert_eq!(newest.album_type.as_deref(), Some("single"));
        assert_eq!(newest.release_date.as_deref(), Some("2026-10-16"));
        assert_eq!(newest.artists[0].name, "Artist");
        assert_eq!(
            newest.artists[0].id.as_deref(),
            Some("0000000000000000000001")
        );
        assert_eq!(newest.images[0].width, Some(300));
    }

    #[test]
    fn a_refused_or_missing_feed_is_not_an_empty_shelf() {
        assert!(parse(&json!({"errors": [{"message": "PersistedQueryNotFound"}]})).is_err());
        assert!(parse(&json!({"data": null})).is_err());
        assert!(
            parse(&json!({"data": {"whatsNewFeedItems": {"items": []}}})).unwrap()
                == Feed::default()
        );
    }

    #[test]
    fn bad_uris_and_unknown_cards_are_passed_over() {
        let mut bad = feed_item(
            "spotify:album:short",
            "Bad",
            "ALBUM",
            "2026-10-02T00:00:00Z",
        );
        let mut unknown = feed_item(
            "spotify:album:3333333333333333333333",
            "Unknown",
            "ALBUM",
            "2026-10-02T00:00:00Z",
        );
        unknown["content"]["data"]["__typename"] = json!("FutureThing");
        bad["content"]["data"]["coverArt"] = json!(null);
        let body = json!({"data": {"whatsNewFeedItems": {"items": [bad, unknown]}}});
        assert_eq!(parse(&body).unwrap(), Feed::default());
    }

    fn episode_item(uri: &str, name: &str, date: &str) -> serde_json::Value {
        json!({
            "id": "podcast_episode:1",
            "content": {
                "__typename": "EpisodeOrChapterResponseWrapper",
                "data": {
                    "__typename": "Episode",
                    "uri": uri,
                    "name": name,
                    "releaseDate": {"isoString": date, "precision": "DAY"},
                    "duration": {"totalMilliseconds": 1_800_000},
                    "coverArt": {"sources": [{"url": "https://i.scdn.co/image/e", "width": 64, "height": 64}]},
                    "podcastV2": {"data": {
                        "__typename": "Podcast",
                        "uri": "spotify:show:5555555555555555555555",
                        "name": "The Show",
                        "coverArt": {"sources": [{"url": "https://i.scdn.co/image/s"}]}
                    }}
                }
            }
        })
    }

    #[test]
    fn reads_the_feeds_episodes_with_their_show() {
        let body = json!({"data": {"whatsNewFeedItems": {"items": [
            episode_item("spotify:episode:6666666666666666666666", "Older talk", "2026-10-01T05:00:00Z"),
            feed_item("spotify:album:1111111111111111111111", "Record", "EP", "2026-10-02T00:00:00Z"),
            episode_item("spotify:episode:7777777777777777777777", "Newest talk", "2026-10-20T05:00:00Z"),
            episode_item("spotify:episode:7777777777777777777777", "Newest talk", "2026-10-20T05:00:00Z"),
        ]}}});
        let feed = parse(&body).unwrap();
        assert_eq!(feed.albums.len(), 1);
        assert_eq!(feed.albums[0].album_type.as_deref(), Some("ep"));
        assert_eq!(
            feed.episodes
                .iter()
                .map(|episode| episode.name.as_str())
                .collect::<Vec<_>>(),
            ["Newest talk", "Older talk"]
        );
        let newest = &feed.episodes[0];
        assert_eq!(newest.id, "7777777777777777777777");
        assert_eq!(newest.release_date.as_deref(), Some("2026-10-20"));
        assert_eq!(newest.duration_ms, 1_800_000);
        assert_eq!(newest.images[0].url, "https://i.scdn.co/image/e");
        let show = newest.show.as_ref().unwrap();
        assert_eq!(show.id, "5555555555555555555555");
        assert_eq!(show.name, "The Show");
    }

    #[test]
    fn unknown_kinds_chapters_and_events_are_skipped_silently() {
        let mut chapter = episode_item(
            "spotify:episode:8888888888888888888888",
            "Chapter",
            "2026-10-01T00:00:00Z",
        );
        chapter["content"]["data"]["__typename"] = json!("Chapter");
        let mut bad_episode = episode_item("spotify:episode:short", "Bad", "2026-10-01T00:00:00Z");
        bad_episode["content"]["data"]["podcastV2"] = json!(null);
        let body = json!({"data": {"whatsNewFeedItems": {"items": [
            {"content": {"__typename": "ConcertResponseWrapper", "data": {"__typename": "Concert", "uri": "spotify:concert:1"}}},
            {"content": {"__typename": "SomethingNew", "data": {}}},
            {"content": {"__typename": "AlbumResponseWrapper"}},
            {"unexpected": true},
            chapter,
            bad_episode,
            feed_item("spotify:album:1111111111111111111111", "Kept", "ALBUM", "2026-10-02T00:00:00Z"),
        ]}}});
        let feed = parse(&body).unwrap();
        assert!(feed.episodes.is_empty());
        assert_eq!(feed.albums.len(), 1);
        assert_eq!(feed.albums[0].name, "Kept");
    }

    #[test]
    fn an_episode_without_its_show_keeps_its_own_fields() {
        let mut item = episode_item(
            "spotify:episode:9999999999999999999999",
            "Alone",
            "2026-10-01T00:00:00Z",
        );
        item["content"]["data"]["podcastV2"] = json!(null);
        item["content"]["data"]["duration"] = json!("long");
        let body = json!({"data": {"whatsNewFeedItems": {"items": [item]}}});
        let feed = parse(&body).unwrap();
        assert_eq!(feed.episodes.len(), 1);
        assert!(feed.episodes[0].show.is_none());
        assert_eq!(feed.episodes[0].duration_ms, 0);
    }

    #[test]
    fn items_are_kept_for_sixty_days() {
        let today = jiff::civil::date(2026, 11, 6);
        assert_eq!(
            release_day("2026-10-16"),
            Some(jiff::civil::date(2026, 10, 16))
        );
        assert_eq!(release_day("2026-10"), Some(jiff::civil::date(2026, 10, 1)));
        assert_eq!(release_day("2026"), Some(jiff::civil::date(2026, 1, 1)));
        assert_eq!(release_day("soon"), None);
        assert!(is_recent(Some("2026-11-06"), today));
        assert!(is_recent(Some("2026-09-07"), today));
        assert!(!is_recent(Some("2026-09-06"), today));
        assert!(!is_recent(Some("2026"), today));
        assert!(!is_recent(None, today));
    }

    #[test]
    fn dates_keep_the_precision_spotify_knows() {
        assert_eq!(
            release_date("2026-10-16T00:00:00Z", Some(&json!("DAY"))),
            "2026-10-16"
        );
        assert_eq!(
            release_date("2026-10-01T00:00:00Z", Some(&json!("MONTH"))),
            "2026-10"
        );
        assert_eq!(
            release_date("2026-01-01T00:00:00Z", Some(&json!("YEAR"))),
            "2026"
        );
    }

    #[test]
    fn the_fallback_merges_artists_releases_newest_first_without_guest_spots() {
        let release = |id: char, date: &str, group: &str| Album {
            id: id.to_string().repeat(22),
            uri: format!("spotify:album:{}", id.to_string().repeat(22)),
            name: id.to_string(),
            release_date: Some(date.into()),
            album_group: Some(group.into()),
            ..Album::default()
        };
        let merged = merge_artist_releases(vec![
            vec![
                release('a', "2026-09-01", "album"),
                release('b', "2026-10-10", "single"),
            ],
            vec![
                release('b', "2026-10-10", "single"),
                release('c', "2026-10-12", "appears_on"),
                release('d', "2025", "album"),
            ],
        ]);
        assert_eq!(
            merged
                .iter()
                .map(|album| album.name.as_str())
                .collect::<Vec<_>>(),
            ["b", "a", "d"]
        );
    }

    #[test]
    fn the_query_reads_albums_and_never_marks_them_seen() {
        let query = query();
        assert_eq!(query["operationName"], "queryWhatsNewFeed");
        assert_eq!(query["variables"]["includedContentTypes"], json!(["ALBUM"]));
        assert_eq!(query["variables"]["limit"], LIMIT);
        assert!(!query.to_string().contains("SetItemsStateInWhatsNewFeed"));
    }
}
