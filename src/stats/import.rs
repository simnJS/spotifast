//! Reading the streaming history Spotify sends on request from its privacy
//! page.
//!
//! The "Extended streaming history" is a ZIP holding
//! `Spotify Extended Streaming History/Streaming_History_Audio_*.json`, each
//! an array of plays (`ts`, `ms_played`, `master_metadata_*`,
//! `spotify_track_uri`, `shuffle`, `skipped`, …). The older "Account data"
//! holds `StreamingHistory_music_*.json` (or `StreamingHistory*.json`), with
//! `endTime` to the minute, `artistName`, `trackName` and `msPlayed`. Either
//! is accepted as the ZIP or as one of its JSON files. Podcast episodes and
//! videos are left out: the stats are about music.

use std::io::Read;
use std::path::Path;

use serde::Deserialize;

use super::{ArtistName, ImportProgress, PlayRecord, Source};

/// The largest history file read from an archive. Spotify's files hold
/// about 15,000 plays, a few megabytes; this bounds a hostile archive.
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("could not read {path}: {error}")]
    Io { path: String, error: String },
    #[error("{0} is not a Spotify streaming history")]
    NotHistory(String),
    #[error("{file} is not a streaming history file: {error}")]
    Json { file: String, error: String },
    #[error("{file} is too large to be a streaming history file")]
    TooLarge { file: String },
}

/// One entry of either format: the fields read, all optional, so one
/// struct reads both and a field Spotify drops or nulls is not an error.
#[derive(Deserialize)]
struct RawEntry {
    // Extended streaming history.
    ts: Option<String>,
    ms_played: Option<u64>,
    master_metadata_track_name: Option<String>,
    master_metadata_album_artist_name: Option<String>,
    master_metadata_album_album_name: Option<String>,
    spotify_track_uri: Option<String>,
    shuffle: Option<bool>,
    skipped: Option<bool>,
    // Account data.
    #[serde(rename = "endTime")]
    end_time: Option<String>,
    #[serde(rename = "artistName")]
    artist_name: Option<String>,
    #[serde(rename = "trackName")]
    track_name: Option<String>,
    #[serde(rename = "msPlayed")]
    ms_played_legacy: Option<u64>,
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

fn clamp_ms(ms: u64) -> u32 {
    u32::try_from(ms).unwrap_or(u32::MAX)
}

impl RawEntry {
    /// The play this entry records, or `None` for an episode, a video, or
    /// an entry without a time.
    fn into_record(self) -> Option<PlayRecord> {
        if let Some(ts) = self.ts {
            let at = ts.parse::<jiff::Timestamp>().ok()?;
            let track = non_empty(self.master_metadata_track_name)?;
            let artist = non_empty(self.master_metadata_album_artist_name);
            return Some(PlayRecord {
                at_ms: at.as_millisecond(),
                ms_played: clamp_ms(self.ms_played.unwrap_or_default()),
                estimated: false,
                source: Source::Extended,
                uri: non_empty(self.spotify_track_uri)
                    .filter(|uri| uri.starts_with("spotify:track:")),
                track,
                artists: artist
                    .map(|name| vec![ArtistName { id: None, name }])
                    .unwrap_or_default(),
                album: non_empty(self.master_metadata_album_album_name),
                album_id: None,
                duration_ms: None,
                skipped: self.skipped,
                shuffle: self.shuffle,
            });
        }
        let end = self.end_time?;
        let at = jiff::civil::DateTime::strptime("%Y-%m-%d %H:%M", end.trim())
            .ok()?
            .to_zoned(jiff::tz::TimeZone::UTC)
            .ok()?
            .timestamp();
        let track = non_empty(self.track_name)?;
        let artist = non_empty(self.artist_name)?;
        Some(PlayRecord {
            at_ms: at.as_millisecond(),
            ms_played: clamp_ms(self.ms_played_legacy.unwrap_or_default()),
            estimated: false,
            source: Source::Account,
            uri: None,
            track,
            artists: vec![ArtistName {
                id: None,
                name: artist,
            }],
            album: None,
            album_id: None,
            duration_ms: None,
            skipped: None,
            shuffle: None,
        })
    }
}

/// The plays in one history file. `file` names it in errors.
pub fn parse_json(file: &str, bytes: &[u8]) -> Result<Vec<PlayRecord>, ImportError> {
    let entries: Vec<RawEntry> =
        serde_json::from_slice(bytes).map_err(|error| ImportError::Json {
            file: file.to_owned(),
            error: error.to_string(),
        })?;
    Ok(entries
        .into_iter()
        .filter_map(RawEntry::into_record)
        .collect())
}

/// Whether a file in the archive is a music history file.
fn is_history_file(name: &str) -> bool {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    if !base.ends_with(".json") {
        return false;
    }
    base.starts_with("Streaming_History_Audio_")
        || base.starts_with("StreamingHistory_music_")
        // The account data before Spotify split music from podcasts.
        || (base.starts_with("StreamingHistory")
            && base["StreamingHistory".len()..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit()))
}

/// What reading a path found.
#[derive(Debug, Default)]
pub struct Contents {
    pub plays: Vec<PlayRecord>,
    pub files: usize,
}

/// Reads the plays in the ZIP or JSON file at `path`, telling `progress`
/// after each file. Blocking: call it off the UI thread.
pub fn read_path(
    path: &Path,
    progress: &mut dyn FnMut(ImportProgress),
) -> Result<Contents, ImportError> {
    let io = |error: std::io::Error| ImportError::Io {
        path: path.display().to_string(),
        error: error.to_string(),
    };
    let mut file = std::fs::File::open(path).map_err(io)?;
    let mut magic = [0u8; 4];
    let zipped = file.read_exact(&mut magic).is_ok() && magic == *b"PK\x03\x04";
    drop(file);
    if zipped {
        let file = std::fs::File::open(path).map_err(io)?;
        return read_zip(file, &path.display().to_string(), progress);
    }
    let size = std::fs::metadata(path).map_err(io)?.len();
    let name = path.display().to_string();
    if size > MAX_FILE_BYTES {
        return Err(ImportError::TooLarge { file: name });
    }
    let bytes = std::fs::read(path).map_err(io)?;
    let plays = parse_json(&name, &bytes)?;
    progress(ImportProgress {
        files_done: 1,
        files_total: 1,
        plays_read: plays.len(),
    });
    Ok(Contents { plays, files: 1 })
}

/// Reads every music history file in an archive.
pub fn read_zip<R: std::io::Read + std::io::Seek>(
    reader: R,
    name: &str,
    progress: &mut dyn FnMut(ImportProgress),
) -> Result<Contents, ImportError> {
    let not_history = || ImportError::NotHistory(name.to_owned());
    let mut archive = zip::ZipArchive::new(reader).map_err(|_| not_history())?;
    let mut wanted: Vec<String> = archive
        .file_names()
        .filter_map(Result::ok)
        .filter(|file| is_history_file(file))
        .map(|file| file.into_owned())
        .collect();
    if wanted.is_empty() {
        return Err(not_history());
    }
    wanted.sort();
    let mut read = Contents::default();
    let total = wanted.len();
    for (done, file) in wanted.into_iter().enumerate() {
        let mut entry = archive.by_name(&file).map_err(|error| ImportError::Io {
            path: file.clone(),
            error: error.to_string(),
        })?;
        if entry.size() > MAX_FILE_BYTES {
            return Err(ImportError::TooLarge { file });
        }
        let mut bytes = Vec::with_capacity(usize::try_from(entry.size()).unwrap_or(0));
        // The size in the header is the archive's word; the read is bounded
        // whatever it says.
        (&mut entry)
            .take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| ImportError::Io {
                path: file.clone(),
                error: error.to_string(),
            })?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(ImportError::TooLarge { file });
        }
        read.plays.extend(parse_json(&file, &bytes)?);
        read.files += 1;
        progress(ImportProgress {
            files_done: done + 1,
            files_total: total,
            plays_read: read.plays.len(),
        });
    }
    Ok(read)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const EXTENDED: &str = r#"[
      {"ts": "2023-05-01T10:03:20Z", "platform": "linux", "ms_played": 200000,
       "conn_country": "FR", "ip_addr": "1.2.3.4",
       "master_metadata_track_name": "Song A",
       "master_metadata_album_artist_name": "Band",
       "master_metadata_album_album_name": "Album",
       "spotify_track_uri": "spotify:track:aaa",
       "episode_name": null, "episode_show_name": null, "spotify_episode_uri": null,
       "reason_start": "clickrow", "reason_end": "trackdone",
       "shuffle": false, "skipped": false, "offline": false,
       "offline_timestamp": null, "incognito_mode": false},
      {"ts": "2023-05-01T10:03:30Z", "platform": "android", "ms_played": 4000,
       "master_metadata_track_name": "Song B",
       "master_metadata_album_artist_name": "Band",
       "master_metadata_album_album_name": "Album",
       "spotify_track_uri": "spotify:track:bbb",
       "reason_start": "fwdbtn", "reason_end": "fwdbtn",
       "shuffle": true, "skipped": true},
      {"ts": "2023-05-01T11:00:00Z", "ms_played": 1800000,
       "master_metadata_track_name": null,
       "master_metadata_album_artist_name": null,
       "master_metadata_album_album_name": null,
       "spotify_track_uri": null,
       "episode_name": "An episode", "episode_show_name": "A show",
       "spotify_episode_uri": "spotify:episode:eee",
       "shuffle": false, "skipped": null}
    ]"#;

    pub(crate) const ACCOUNT: &str = r#"[
      {"endTime": "2019-02-05 22:10", "artistName": "Old Band",
       "trackName": "Old Song", "msPlayed": 185000},
      {"endTime": "2019-02-05 22:11", "artistName": "Old Band",
       "trackName": "Skipped Song", "msPlayed": 2500}
    ]"#;

    #[test]
    fn the_extended_history_reads_songs_and_leaves_out_episodes() {
        let plays = parse_json("Streaming_History_Audio_2023.json", EXTENDED.as_bytes()).unwrap();
        assert_eq!(plays.len(), 2);
        let first = &plays[0];
        assert_eq!(first.source, Source::Extended);
        assert_eq!(
            first.at_ms,
            "2023-05-01T10:03:20Z"
                .parse::<jiff::Timestamp>()
                .unwrap()
                .as_millisecond()
        );
        assert_eq!(first.ms_played, 200_000);
        assert!(!first.estimated);
        assert_eq!(first.uri.as_deref(), Some("spotify:track:aaa"));
        assert_eq!(first.track, "Song A");
        assert_eq!(first.artists[0].name, "Band");
        assert_eq!(first.album.as_deref(), Some("Album"));
        assert!(first.is_stream());
        let second = &plays[1];
        assert_eq!(second.skipped, Some(true));
        assert_eq!(second.shuffle, Some(true));
        assert!(!second.is_stream(), "4 s is a skip");
    }

    #[test]
    fn the_account_data_history_reads_minutes_in_utc() {
        let plays = parse_json("StreamingHistory_music_0.json", ACCOUNT.as_bytes()).unwrap();
        assert_eq!(plays.len(), 2);
        assert_eq!(plays[0].source, Source::Account);
        assert_eq!(
            plays[0].at_ms,
            "2019-02-05T22:10:00Z"
                .parse::<jiff::Timestamp>()
                .unwrap()
                .as_millisecond()
        );
        assert_eq!(plays[0].ms_played, 185_000);
        assert_eq!(plays[0].uri, None);
        assert_eq!(plays[0].artists[0].name, "Old Band");
        assert!(!plays[1].is_stream());
    }

    #[test]
    fn other_json_is_refused() {
        assert!(matches!(
            parse_json("x.json", b"{\"not\": \"a list\"}"),
            Err(ImportError::Json { .. })
        ));
    }

    /// A small archive shaped as Spotify sends it, with files the import
    /// must leave alone beside the history.
    pub(crate) fn zip_fixture() -> Vec<u8> {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in [
            ("Spotify Extended Streaming History/ReadMeFirst.pdf", "%PDF"),
            (
                "Spotify Extended Streaming History/Streaming_History_Audio_2023.json",
                EXTENDED,
            ),
            (
                "Spotify Extended Streaming History/Streaming_History_Video_2023.json",
                "[]",
            ),
            (
                "Spotify Account Data/StreamingHistory_music_0.json",
                ACCOUNT,
            ),
            ("Spotify Account Data/StreamingHistory_podcast_0.json", "[]"),
            ("Spotify Account Data/Userdata.json", "{}"),
        ] {
            zip.start_file(name, options).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn the_zip_reads_only_its_music_history_files() {
        let mut seen = Vec::new();
        let read = read_zip(
            std::io::Cursor::new(zip_fixture()),
            "my_spotify_data.zip",
            &mut |progress| seen.push(progress),
        )
        .unwrap();
        assert_eq!(read.files, 2);
        assert_eq!(read.plays.len(), 4);
        assert_eq!(
            seen.last(),
            Some(&ImportProgress {
                files_done: 2,
                files_total: 2,
                plays_read: 4
            })
        );
    }

    #[test]
    fn a_zip_without_history_is_refused() {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file("photo.jpg", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"jpeg").unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        assert!(matches!(
            read_zip(std::io::Cursor::new(bytes), "x.zip", &mut |_| {}),
            Err(ImportError::NotHistory(_))
        ));
    }

    #[test]
    fn a_path_is_read_as_a_zip_or_as_json() {
        let dir = std::env::temp_dir().join(format!(
            "spotifast-stats-import-{:016x}",
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let archive = dir.join("my_spotify_data.zip");
        std::fs::write(&archive, zip_fixture()).unwrap();
        assert_eq!(read_path(&archive, &mut |_| {}).unwrap().plays.len(), 4);
        let json = dir.join("Streaming_History_Audio_2023.json");
        std::fs::write(&json, EXTENDED).unwrap();
        assert_eq!(read_path(&json, &mut |_| {}).unwrap().plays.len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_file_names() {
        assert!(is_history_file(
            "a/Streaming_History_Audio_2020-2021_0.json"
        ));
        assert!(is_history_file("StreamingHistory_music_3.json"));
        assert!(is_history_file("MyData/StreamingHistory0.json"));
        assert!(!is_history_file("StreamingHistory_podcast_0.json"));
        assert!(!is_history_file("Streaming_History_Video_2021.json"));
        assert!(!is_history_file("Streaming_History_Audio_2021.pdf"));
    }
}
