//! The plays of one account, indexed for aggregation.
//!
//! Built from the merged log and the metadata cache on a background thread:
//! every track, artist, album and genre is interned once, so each play is a
//! few numbers and the queries in [`super::query`] are plain scans over a
//! sorted array.
//!
//! Sources name things differently, so names are reconciled while
//! building: an artist known by ID anywhere (a logged play, the metadata
//! cache) is the same artist when an import names it only; a play without
//! a URI (the older account-data history) is the track with a URI that has
//! the same artist and title.

use std::collections::HashMap;

use super::metadata::Metadata;
use super::{PlayRecord, Source};

/// One play.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Play {
    pub at_ms: i64,
    pub ms_played: u32,
    /// Index into [`Index::tracks`].
    pub track: u32,
    pub estimated: bool,
    pub skipped: bool,
    pub source: Source,
}

impl Play {
    pub fn is_stream(&self) -> bool {
        self.ms_played >= super::STREAM_MS
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackEntry {
    pub uri: Option<String>,
    pub name: String,
    /// Indexes into [`Index::artists`], main artist first.
    pub artists: Vec<u32>,
    /// Index into [`Index::albums`].
    pub album: Option<u32>,
    pub duration_ms: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArtistEntry {
    pub id: Option<String>,
    pub name: String,
    pub image: Option<String>,
    /// Indexes into [`Index::genres`].
    pub genres: Vec<u32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AlbumEntry {
    pub id: Option<String>,
    pub name: String,
    pub image: Option<String>,
    /// Index into [`Index::artists`].
    pub artist: Option<u32>,
}

/// One account's plays and what they name.
#[derive(Clone, Debug, Default)]
pub struct Index {
    /// Oldest first.
    pub plays: Vec<Play>,
    pub tracks: Vec<TrackEntry>,
    pub artists: Vec<ArtistEntry>,
    pub albums: Vec<AlbumEntry>,
    pub genres: Vec<String>,
    track_keys: HashMap<String, u32>,
    artist_keys: HashMap<String, u32>,
    album_keys: HashMap<String, u32>,
}

fn fold(text: &str) -> String {
    text.trim().to_lowercase()
}

fn intern<T>(
    keys: &mut HashMap<String, u32>,
    entries: &mut Vec<T>,
    key: String,
    make: impl FnOnce() -> T,
) -> u32 {
    *keys.entry(key).or_insert_with(|| {
        entries.push(make());
        (entries.len() - 1) as u32
    })
}

impl Index {
    /// Indexes `records`, oldest first, with what `metadata` knows.
    pub fn build(records: &[PlayRecord], metadata: &Metadata) -> Self {
        // Every artist ID by folded name, so a name alone finds its ID.
        let mut artist_ids: HashMap<String, String> = HashMap::new();
        let mut learn = |name: &str, id: &str| {
            artist_ids
                .entry(fold(name))
                .or_insert_with(|| id.to_owned());
        };
        for record in records {
            for artist in &record.artists {
                if let Some(id) = &artist.id {
                    learn(&artist.name, id);
                }
            }
        }
        for track in metadata.tracks.values() {
            for artist in &track.artists {
                if let Some(id) = &artist.id {
                    learn(&artist.name, id);
                }
            }
        }
        for (id, artist) in &metadata.artists {
            learn(&artist.name, id);
        }
        // The URI of each song by artist and title, for plays without one.
        let mut song_uris: HashMap<String, String> = HashMap::new();
        for record in records {
            if let Some(uri) = &record.uri {
                song_uris
                    .entry(record.song_key())
                    .or_insert_with(|| uri.clone());
            }
        }

        let mut index = Self::default();
        let mut genre_keys: HashMap<String, u32> = HashMap::new();
        let mut plays = Vec::with_capacity(records.len());
        for record in records {
            let uri = record
                .uri
                .clone()
                .or_else(|| song_uris.get(&record.song_key()).cloned());
            let key = uri
                .clone()
                .unwrap_or_else(|| format!("song:{}", record.song_key()));
            let track = match index.track_keys.get(&key) {
                Some(&track) => track,
                None => {
                    let entry = index.track_entry(
                        record,
                        uri.as_deref(),
                        metadata,
                        &artist_ids,
                        &mut genre_keys,
                    );
                    index.tracks.push(entry);
                    let track = (index.tracks.len() - 1) as u32;
                    index.track_keys.insert(key, track);
                    track
                }
            };
            plays.push(Play {
                at_ms: record.at_ms,
                ms_played: record.ms_played,
                track,
                estimated: record.estimated,
                skipped: record.skipped.unwrap_or(false),
                source: record.source,
            });
        }
        plays.sort_by_key(|play| play.at_ms);
        index.plays = plays;
        index
    }

    /// The entry of a track first met in `record`.
    fn track_entry(
        &mut self,
        record: &PlayRecord,
        uri: Option<&str>,
        metadata: &Metadata,
        artist_ids: &HashMap<String, String>,
        genre_keys: &mut HashMap<String, u32>,
    ) -> TrackEntry {
        let meta = uri.and_then(|uri| metadata.tracks.get(uri));
        let named = meta.map_or(record.artists.as_slice(), |meta| meta.artists.as_slice());
        let named = if named.is_empty() {
            record.artists.as_slice()
        } else {
            named
        };
        let mut artists = Vec::new();
        for artist in named {
            let id = artist
                .id
                .clone()
                .or_else(|| artist_ids.get(&fold(&artist.name)).cloned());
            let entry = self.artist(id, &artist.name, metadata, genre_keys);
            if !artists.contains(&entry) {
                artists.push(entry);
            }
        }
        let album = match meta.and_then(|meta| meta.album.as_ref()) {
            Some(album) => Some(self.album(
                Some(album.id.clone()),
                &album.name,
                album.image.clone(),
                artists.first().copied(),
            )),
            None => record.album.as_deref().map(|name| {
                self.album(
                    record.album_id.clone(),
                    name,
                    None,
                    artists.first().copied(),
                )
            }),
        };
        TrackEntry {
            uri: uri.map(str::to_owned),
            name: meta.map_or_else(|| record.track.clone(), |meta| meta.name.clone()),
            artists,
            album,
            duration_ms: meta
                .map(|meta| meta.duration_ms)
                .filter(|&ms| ms > 0)
                .or(record.duration_ms),
        }
    }

    fn artist(
        &mut self,
        id: Option<String>,
        name: &str,
        metadata: &Metadata,
        genre_keys: &mut HashMap<String, u32>,
    ) -> u32 {
        let key = match &id {
            Some(id) => format!("id:{id}"),
            None => format!("name:{}", fold(name)),
        };
        if let Some(&artist) = self.artist_keys.get(&key) {
            return artist;
        }
        let meta = id.as_deref().and_then(|id| metadata.artists.get(id));
        let mut genres: Vec<u32> = Vec::new();
        for genre in meta.map(|meta| meta.genres.as_slice()).unwrap_or_default() {
            let genre = intern(genre_keys, &mut self.genres, fold(genre), || genre.clone());
            if !genres.contains(&genre) {
                genres.push(genre);
            }
        }
        let entry = ArtistEntry {
            name: meta
                .map(|meta| meta.name.clone())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| name.to_owned()),
            image: meta.and_then(|meta| meta.image.clone()),
            id,
            genres,
        };
        intern(&mut self.artist_keys, &mut self.artists, key, || entry)
    }

    fn album(
        &mut self,
        id: Option<String>,
        name: &str,
        image: Option<String>,
        artist: Option<u32>,
    ) -> u32 {
        // An album without an ID is its name under its main artist.
        let named = format!(
            "name:{}\u{1f}{}",
            artist.map_or(String::new(), |artist| fold(
                &self.artists[artist as usize].name
            )),
            fold(name)
        );
        let key = match &id {
            Some(id) => format!("id:{id}"),
            None => named.clone(),
        };
        if let Some(&album) = self.album_keys.get(&key) {
            let entry = &mut self.albums[album as usize];
            if entry.image.is_none() {
                entry.image = image;
            }
            return album;
        }
        // The same album met by name before its ID was known.
        if id.is_some()
            && let Some(&album) = self.album_keys.get(&named)
        {
            let entry = &mut self.albums[album as usize];
            entry.id.clone_from(&id);
            if entry.image.is_none() {
                entry.image = image;
            }
            self.album_keys.insert(key, album);
            return album;
        }
        let entry = AlbumEntry {
            id,
            name: name.to_owned(),
            image,
            artist,
        };
        let album = intern(&mut self.album_keys, &mut self.albums, key, || entry);
        self.album_keys.entry(named).or_insert(album);
        album
    }

    /// The track played as `uri`.
    pub fn track_by_uri(&self, uri: &str) -> Option<u32> {
        self.track_keys.get(uri).copied()
    }

    pub fn artist_by_id(&self, id: &str) -> Option<u32> {
        self.artist_keys.get(&format!("id:{id}")).copied()
    }

    pub fn album_by_id(&self, id: &str) -> Option<u32> {
        self.album_keys.get(&format!("id:{id}")).copied()
    }

    pub fn genre_by_name(&self, name: &str) -> Option<u32> {
        let name = fold(name);
        self.genres
            .iter()
            .position(|genre| fold(genre) == name)
            .map(|genre| genre as u32)
    }

    /// The plays from `start_ms` up to, not including, `end_ms`.
    pub fn plays_between(&self, start_ms: i64, end_ms: i64) -> &[Play] {
        let from = self.plays.partition_point(|play| play.at_ms < start_ms);
        let to = self.plays.partition_point(|play| play.at_ms < end_ms);
        &self.plays[from..to.max(from)]
    }

    pub fn first_play_ms(&self) -> Option<i64> {
        self.plays.first().map(|play| play.at_ms)
    }

    pub fn last_play_ms(&self) -> Option<i64> {
        self.plays.last().map(|play| play.at_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::ArtistName;
    use crate::stats::metadata::{AlbumMeta, ArtistMeta, TrackMeta};

    fn record(
        uri: Option<&str>,
        track: &str,
        artist: (&str, Option<&str>),
        source: Source,
    ) -> PlayRecord {
        PlayRecord {
            at_ms: 0,
            ms_played: 60_000,
            estimated: false,
            source,
            uri: uri.map(str::to_owned),
            track: track.into(),
            artists: vec![ArtistName {
                id: artist.1.map(str::to_owned),
                name: artist.0.into(),
            }],
            album: Some("Album".into()),
            album_id: None,
            duration_ms: None,
            skipped: None,
            shuffle: None,
        }
    }

    #[test]
    fn sources_that_name_things_differently_meet_in_one_entry() {
        let records = vec![
            // Logged: artist with an ID.
            record(
                Some("spotify:track:a"),
                "Song",
                ("Band", Some("band")),
                Source::Spotify,
            ),
            // Imported: the same artist by name only.
            record(
                Some("spotify:track:b"),
                "Other",
                ("band", None),
                Source::Extended,
            ),
            // Account data: no URI at all.
            record(None, "Song", ("Band", None), Source::Account),
        ];
        let mut metadata = Metadata::default();
        metadata.tracks.insert(
            "spotify:track:b".into(),
            TrackMeta {
                name: "Other".into(),
                duration_ms: 99_000,
                artists: vec![ArtistName {
                    id: Some("band".into()),
                    name: "Band".into(),
                }],
                album: Some(AlbumMeta {
                    id: "al".into(),
                    name: "Album".into(),
                    image: Some("cover".into()),
                }),
            },
        );
        metadata.artists.insert(
            "band".into(),
            ArtistMeta {
                name: "Band".into(),
                genres: vec!["Indie".into(), "indie".into()],
                image: Some("portrait".into()),
            },
        );
        let index = Index::build(&records, &metadata);
        assert_eq!(index.tracks.len(), 2, "{:?}", index.tracks);
        assert_eq!(index.artists.len(), 1);
        assert_eq!(index.artists[0].image.as_deref(), Some("portrait"));
        assert_eq!(index.genres, vec!["Indie"]);
        assert_eq!(index.artists[0].genres, vec![0]);
        assert_eq!(index.albums.len(), 1, "{:?}", index.albums);
        assert_eq!(index.albums[0].id.as_deref(), Some("al"));
        assert_eq!(index.albums[0].image.as_deref(), Some("cover"));
        let song = index.track_by_uri("spotify:track:a").unwrap();
        assert_eq!(
            index.plays.iter().filter(|play| play.track == song).count(),
            2
        );
        assert_eq!(
            index.tracks[index.track_by_uri("spotify:track:b").unwrap() as usize].duration_ms,
            Some(99_000)
        );
        assert_eq!(index.artist_by_id("band"), Some(0));
        assert_eq!(index.album_by_id("al"), Some(0));
    }
}
