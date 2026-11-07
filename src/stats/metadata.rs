//! Track and artist details the plays do not carry, cached on disk.
//!
//! An imported history names a track and its album artist but has no
//! artist IDs, no album ID or cover, no duration, and no genres. Those
//! come from the Web API, one track (`GET /tracks/{id}`) and one artist
//! (`GET /artists/{id}`) at a time: Spotify removed the batch `?ids=` reads
//! for Development Mode apps in February 2026, and the playback session's
//! metadata carries no genres. Only items missing from the cache are asked
//! for, the most played first, a bounded number per refresh, so a large
//! import fills in over a few refreshes without spending the app's quota at
//! once. Items Spotify no longer has are remembered and not asked again.

use std::collections::{HashMap, HashSet};
use std::future::Future;

use serde::{Deserialize, Serialize};

use super::{ArtistName, PlayRecord};
use crate::api::ApiError;
use crate::api::models::{Artist, Track};

/// Tracks asked for per refresh.
pub const TRACKS_PER_RUN: usize = 40;
/// Artists asked for per refresh.
pub const ARTISTS_PER_RUN: usize = 40;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AlbumMeta {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TrackMeta {
    pub name: String,
    pub duration_ms: u32,
    pub artists: Vec<ArtistName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<AlbumMeta>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ArtistMeta {
    pub name: String,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

/// The cache, by track URI and artist ID.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    #[serde(default)]
    pub tracks: HashMap<String, TrackMeta>,
    #[serde(default)]
    pub artists: HashMap<String, ArtistMeta>,
    /// Track URIs and artist IDs Spotify answered "not found" for.
    #[serde(default)]
    pub missing: HashSet<String>,
}

/// The largest image: the one a stats page shows big.
fn best_image(images: &[crate::api::models::Image]) -> Option<String> {
    images
        .iter()
        .max_by_key(|image| image.width.unwrap_or(0))
        .map(|image| image.url.clone())
        .filter(|url| !url.is_empty())
}

impl Metadata {
    /// Remembers a full track object, as recently-played and `/tracks/{id}`
    /// return them.
    pub fn learn_track(&mut self, track: &Track) -> bool {
        if !track.uri.starts_with("spotify:track:") || track.name.is_empty() {
            return false;
        }
        let meta = TrackMeta {
            name: track.name.clone(),
            duration_ms: track.duration_ms,
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
                .filter(|album| !album.id.is_empty())
                .map(|album| AlbumMeta {
                    id: album.id.clone(),
                    name: album.name.clone(),
                    image: best_image(&album.images),
                }),
        };
        let changed = self.tracks.get(&track.uri) != Some(&meta);
        self.tracks.insert(track.uri.clone(), meta);
        changed
    }

    pub fn learn_artist(&mut self, artist: &Artist) -> bool {
        if artist.id.is_empty() {
            return false;
        }
        let meta = ArtistMeta {
            name: artist.name.clone(),
            genres: artist.genres.clone(),
            image: best_image(&artist.images),
        };
        let changed = self.artists.get(&artist.id) != Some(&meta);
        self.artists.insert(artist.id.clone(), meta);
        changed
    }

    /// Track URIs played but not in the cache, the most played first.
    pub fn tracks_wanted(&self, records: &[PlayRecord]) -> Vec<String> {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for record in records {
            if let Some(uri) = record.uri.as_deref()
                && !self.tracks.contains_key(uri)
                && !self.missing.contains(uri)
            {
                *counts.entry(uri).or_default() += 1;
            }
        }
        by_count(counts)
    }

    /// Artist IDs played but not in the cache, the most played first.
    pub fn artists_wanted(&self, records: &[PlayRecord]) -> Vec<String> {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for record in records {
            let from_cache = record
                .uri
                .as_deref()
                .and_then(|uri| self.tracks.get(uri))
                .map(|track| track.artists.as_slice());
            for artist in from_cache.unwrap_or(&record.artists) {
                if let Some(id) = artist.id.as_deref()
                    && !self.artists.contains_key(id)
                    && !self.missing.contains(id)
                {
                    *counts.entry(id).or_default() += 1;
                }
            }
        }
        by_count(counts)
    }
}

fn by_count(counts: HashMap<&str, usize>) -> Vec<String> {
    let mut wanted: Vec<(&str, usize)> = counts.into_iter().collect();
    wanted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    wanted.into_iter().map(|(key, _)| key.to_owned()).collect()
}

/// Where details come from: the Web API, or a fake in tests.
pub trait MetadataSource {
    fn track(&self, id: &str) -> impl Future<Output = Result<Track, ApiError>> + Send;
    fn artist(&self, id: &str) -> impl Future<Output = Result<Artist, ApiError>> + Send;
}

/// Whether an answer means the item is gone, not that asking failed.
fn gone(error: &ApiError) -> bool {
    matches!(error.status(), Some(400 | 404))
}

/// Asks for up to the per-run budget of missing tracks, then of missing
/// artists. Returns whether the cache changed. A failure other than "not
/// found" (a rate limit, the quota, the network) stops the run: the rest
/// waits for the next refresh.
pub async fn fill(
    source: &impl MetadataSource,
    metadata: &mut Metadata,
    records: &[PlayRecord],
) -> bool {
    let mut changed = false;
    for uri in metadata
        .tracks_wanted(records)
        .into_iter()
        .take(TRACKS_PER_RUN)
    {
        let id = uri.trim_start_matches("spotify:track:");
        match source.track(id).await {
            Ok(track) => {
                let mut track = track;
                // A relinked track answers with another URI; the cache is
                // by the one played.
                track.uri.clone_from(&uri);
                changed |= metadata.learn_track(&track);
            }
            Err(error) if gone(&error) => {
                changed |= metadata.missing.insert(uri);
            }
            Err(error) => {
                log::debug!("track details for the stats stopped: {error}");
                return changed;
            }
        }
    }
    for id in metadata
        .artists_wanted(records)
        .into_iter()
        .take(ARTISTS_PER_RUN)
    {
        match source.artist(&id).await {
            Ok(artist) => {
                let mut artist = artist;
                artist.id.clone_from(&id);
                changed |= metadata.learn_artist(&artist);
            }
            Err(error) if gone(&error) => {
                changed |= metadata.missing.insert(id);
            }
            Err(error) => {
                log::debug!("artist details for the stats stopped: {error}");
                return changed;
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::{Album, ArtistRef, Image};
    use crate::stats::Source;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Fake {
        asked: Mutex<Vec<String>>,
    }

    impl MetadataSource for Fake {
        async fn track(&self, id: &str) -> Result<Track, ApiError> {
            self.asked.lock().unwrap().push(format!("track:{id}"));
            if id == "gone" {
                return Err(ApiError::Status {
                    status: 404,
                    message: "not found".into(),
                });
            }
            Ok(Track {
                id: Some(id.into()),
                uri: format!("spotify:track:{id}"),
                name: format!("Song {id}"),
                duration_ms: 123_000,
                artists: vec![ArtistRef {
                    id: Some("band".into()),
                    name: "Band".into(),
                    uri: None,
                }],
                album: Some(Album {
                    id: "album".into(),
                    name: "Album".into(),
                    images: vec![
                        Image {
                            url: "small".into(),
                            width: Some(64),
                            height: Some(64),
                        },
                        Image {
                            url: "big".into(),
                            width: Some(640),
                            height: Some(640),
                        },
                    ],
                    ..Album::default()
                }),
                ..Track::default()
            })
        }

        async fn artist(&self, id: &str) -> Result<Artist, ApiError> {
            self.asked.lock().unwrap().push(format!("artist:{id}"));
            Ok(Artist {
                id: id.into(),
                name: "Band".into(),
                genres: vec!["french house".into()],
                ..Artist::default()
            })
        }
    }

    fn imported(uri: &str) -> PlayRecord {
        PlayRecord {
            at_ms: 0,
            ms_played: 100_000,
            estimated: false,
            source: Source::Extended,
            uri: Some(uri.into()),
            track: "x".into(),
            artists: vec![ArtistName {
                id: None,
                name: "Band".into(),
            }],
            album: None,
            album_id: None,
            duration_ms: None,
            skipped: None,
            shuffle: None,
        }
    }

    #[tokio::test]
    async fn only_missing_items_are_asked_for_the_most_played_first() {
        let records = vec![
            imported("spotify:track:one"),
            imported("spotify:track:two"),
            imported("spotify:track:two"),
            imported("spotify:track:gone"),
            imported("spotify:track:known"),
        ];
        let mut metadata = Metadata::default();
        metadata.tracks.insert(
            "spotify:track:known".into(),
            TrackMeta {
                name: "Known".into(),
                ..TrackMeta::default()
            },
        );
        let fake = Fake::default();
        assert!(fill(&fake, &mut metadata, &records).await);
        assert_eq!(
            *fake.asked.lock().unwrap(),
            vec![
                "track:two",
                "track:gone",
                "track:one",
                // The artist became known through the tracks.
                "artist:band"
            ]
        );
        let two = &metadata.tracks["spotify:track:two"];
        assert_eq!(two.duration_ms, 123_000);
        assert_eq!(two.album.as_ref().unwrap().image.as_deref(), Some("big"));
        assert_eq!(metadata.artists["band"].genres, vec!["french house"]);
        assert!(metadata.missing.contains("spotify:track:gone"));
        // Nothing is asked twice.
        let fake = Fake::default();
        assert!(!fill(&fake, &mut metadata, &records).await);
        assert!(fake.asked.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_rate_limit_stops_the_run() {
        struct Limited;
        impl MetadataSource for Limited {
            async fn track(&self, _: &str) -> Result<Track, ApiError> {
                Err(ApiError::RateLimited)
            }
            async fn artist(&self, _: &str) -> Result<Artist, ApiError> {
                panic!("no artist after a rate limit")
            }
        }
        let mut metadata = Metadata::default();
        let records = vec![imported("spotify:track:one")];
        assert!(!fill(&Limited, &mut metadata, &records).await);
        assert!(metadata.missing.is_empty());
    }
}
