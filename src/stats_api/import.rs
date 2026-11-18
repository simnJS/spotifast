//! Importing a streaming history into the server, in API mode.
//!
//! The server's `POST /import` takes the body as it is: the whole export
//! ZIP (`application/zip`) or one `Streaming_History_Audio_*.json` file
//! (`application/json`). The file is streamed from the disk, so a large
//! export never sits in memory, and the bytes handed to the connection
//! are reported as they go ([`Sent`]). The server answers a report per
//! file ([`Report`]).

use std::io::Read;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use super::client::Error;

/// The largest export archive the server takes.
pub const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
/// The largest single history file the server takes.
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// A whole export can take a while to send on a slow line and for the
/// server to read, longer than the shared client's thirty seconds.
pub const TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// How often, at most, the bytes sent are reported: every hundredth of
/// the file.
const STEPS: u64 = 100;

/// The content types the server takes.
pub const ZIP: &str = "application/zip";
pub const JSON: &str = "application/json";

/// How far a file being sent has got.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sent {
    pub bytes: u64,
    pub total: u64,
}

impl Sent {
    /// The whole file has gone: the server is reading it.
    pub fn done(&self) -> bool {
        self.bytes >= self.total
    }

    /// The share sent, from 0 to 1.
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            1.0
        } else {
            (self.bytes as f64 / self.total as f64).clamp(0.0, 1.0) as f32
        }
    }
}

/// What the server did with the plays of one file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Counts {
    /// Entries in the file.
    #[serde(default)]
    pub received: u64,
    /// Plays new to the server.
    #[serde(default)]
    pub imported: u64,
    /// Entries under 30 seconds.
    #[serde(default)]
    pub skipped_short: u64,
    /// Podcasts and local files.
    #[serde(default)]
    pub skipped_not_track: u64,
    /// Plays the server already had.
    #[serde(default)]
    pub skipped_duplicate: u64,
}

impl Counts {
    /// Entries that are not plays the server keeps.
    pub fn skipped(&self) -> u64 {
        self.skipped_short.saturating_add(self.skipped_not_track)
    }
}

/// One file of the report.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct FileReport {
    pub name: String,
    #[serde(flatten)]
    pub counts: Counts,
}

/// The server's report of an import.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Each history file it read, in its order.
    pub files: Vec<FileReport>,
    pub totals: Counts,
    /// The files of an archive that are not a streaming history.
    pub ignored_files: Vec<String>,
}

/// The server's answer: an archive's report, or the counts of the one
/// file sent.
#[derive(Deserialize)]
#[serde(untagged)]
enum Answer {
    Archive {
        files: Vec<FileReport>,
        totals: Counts,
        #[serde(default)]
        ignored_files: Vec<String>,
    },
    File(Counts),
}

impl Report {
    /// Reads the server's answer to sending the file `name`.
    pub fn parse(body: &[u8], name: &str) -> Result<Self, Error> {
        Ok(
            match serde_json::from_slice::<Answer>(body).map_err(|_| Error::Invalid)? {
                Answer::Archive {
                    files,
                    totals,
                    ignored_files,
                } => Self {
                    files,
                    totals,
                    ignored_files,
                },
                Answer::File(counts) => Self {
                    files: vec![FileReport {
                        name: name.to_owned(),
                        counts: counts.clone(),
                    }],
                    totals: counts,
                    ignored_files: Vec::new(),
                },
            },
        )
    }
}

/// How the server takes a file: its content type and the most it takes.
fn kind(name: &str, first: &[u8]) -> Option<(&'static str, u64)> {
    if first.starts_with(b"PK\x03\x04") {
        Some((ZIP, MAX_ARCHIVE_BYTES))
    } else if name.starts_with("Streaming_History_Audio") && name.ends_with(".json") {
        Some((JSON, MAX_FILE_BYTES))
    } else {
        None
    }
}

/// A streaming history file opened for the server.
pub struct Upload {
    pub file: std::fs::File,
    pub name: String,
    pub content_type: &'static str,
    pub len: u64,
}

/// Opens the streaming history at `path` for the server: an export ZIP,
/// or a `Streaming_History_Audio_*.json` file on its own. `Ok(None)` for
/// any other file, such as the older account data's
/// `StreamingHistory_music_*.json`, which the server does not read.
/// Blocking.
pub fn open(path: &Path) -> Result<Option<Upload>, Error> {
    use std::io::{Seek, SeekFrom};
    let io = |error: std::io::Error| Error::File(error.to_string());
    let mut file = std::fs::File::open(path).map_err(io)?;
    let mut first = [0u8; 4];
    let read = file.read(&mut first).map_err(io)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_owned();
    let Some((content_type, limit)) = kind(&name, &first[..read]) else {
        return Ok(None);
    };
    let len = file.metadata().map_err(io)?.len();
    if len > limit {
        return Err(Error::TooLarge);
    }
    file.seek(SeekFrom::Start(0)).map_err(io)?;
    Ok(Some(Upload {
        file,
        name,
        content_type,
        len,
    }))
}

/// Reads `inner` for the connection and reports the bytes read so far,
/// at most every hundredth of `total` and once at the end.
pub struct Progress<R, F> {
    inner: R,
    sent: Sent,
    next: u64,
    report: F,
}

impl<R: Read, F: FnMut(Sent)> Progress<R, F> {
    pub fn new(inner: R, total: u64, report: F) -> Self {
        Self {
            inner,
            sent: Sent { bytes: 0, total },
            next: 0,
            report,
        }
    }
}

impl<R: Read, F: FnMut(Sent)> Read for Progress<R, F> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf)?;
        if read > 0 {
            self.sent.bytes = self.sent.bytes.saturating_add(read as u64);
            if self.sent.bytes >= self.next || self.sent.done() {
                let step = (self.sent.total / STEPS).max(1);
                self.next = self.sent.bytes.saturating_add(step);
                (self.report)(self.sent);
            }
        }
        Ok(read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_server_takes_an_archive_or_an_extended_history_file_only() {
        assert_eq!(
            kind("my_spotify_data.zip", b"PK\x03\x04"),
            Some((ZIP, MAX_ARCHIVE_BYTES))
        );
        // An archive by its content, whatever its name.
        assert_eq!(
            kind("download", b"PK\x03\x04"),
            Some((ZIP, MAX_ARCHIVE_BYTES))
        );
        assert_eq!(
            kind("Streaming_History_Audio_2023_4.json", b"[{\"t"),
            Some((JSON, MAX_FILE_BYTES))
        );
        // The older account-data history is not one the server reads.
        assert_eq!(kind("StreamingHistory_music_0.json", b"[{\"e"), None);
        assert_eq!(kind("notes.txt", b"hell"), None);
    }

    #[test]
    fn a_file_opens_from_its_start_and_others_are_left() {
        let dir = std::env::temp_dir().join(format!(
            "spotifast-stats-server-import-{:016x}",
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let extended = dir.join("Streaming_History_Audio_2024.json");
        std::fs::write(&extended, b"[{\"ts\": 1}]").unwrap();
        let mut upload = open(&extended).unwrap().unwrap();
        assert_eq!(upload.content_type, JSON);
        assert_eq!(upload.name, "Streaming_History_Audio_2024.json");
        assert_eq!(upload.len, 11);
        let mut body = Vec::new();
        upload.file.read_to_end(&mut body).unwrap();
        assert_eq!(body, b"[{\"ts\": 1}]");
        let other = dir.join("StreamingHistory_music_0.json");
        std::fs::write(&other, b"[]").unwrap();
        assert!(open(&other).unwrap().is_none());
        assert!(matches!(
            open(&dir.join("missing.zip")),
            Err(Error::File(_))
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn progress_counts_every_byte_and_reports_a_hundred_times_at_most() {
        let data = vec![7u8; 1_000_000];
        let mut reports = Vec::new();
        let mut reader = Progress::new(data.as_slice(), data.len() as u64, |sent| {
            reports.push(sent)
        });
        let mut chunk = [0u8; 1000];
        let mut total = 0;
        loop {
            let read = reader.read(&mut chunk).unwrap();
            if read == 0 {
                break;
            }
            total += read;
        }
        assert_eq!(total, 1_000_000);
        assert!(reports.len() <= 101, "{}", reports.len());
        assert!(reports.len() >= 50, "{}", reports.len());
        assert!(reports.windows(2).all(|pair| pair[0].bytes < pair[1].bytes));
        let last = reports.last().unwrap();
        assert!(last.done());
        assert_eq!(last.bytes, 1_000_000);
        assert_eq!(last.fraction(), 1.0);
        assert_eq!(
            Sent {
                bytes: 250,
                total: 1000
            }
            .fraction(),
            0.25
        );
    }

    #[test]
    fn the_report_reads_an_archive_per_file_and_a_single_file() {
        let archive = Report::parse(
            br#"{"files": [
                {"name": "Spotify Extended Streaming History/Streaming_History_Audio_2019-2021_0.json",
                 "received": 1000, "imported": 940, "skipped_short": 40,
                 "skipped_not_track": 5, "skipped_duplicate": 15},
                {"name": "Spotify Extended Streaming History/Streaming_History_Audio_2021-2023_1.json",
                 "received": 10, "imported": 0, "skipped_short": 0,
                 "skipped_not_track": 0, "skipped_duplicate": 10}],
              "totals": {"received": 1010, "imported": 940, "skipped_short": 40,
                 "skipped_not_track": 5, "skipped_duplicate": 25},
              "ignored_files": ["Spotify Extended Streaming History/Streaming_History_Video_2020.json",
                 "ReadMeFirst.pdf"]}"#,
            "my_spotify_data.zip",
        )
        .unwrap();
        assert_eq!(archive.files.len(), 2);
        assert_eq!(archive.files[0].counts.imported, 940);
        assert_eq!(archive.files[0].counts.skipped(), 45);
        assert_eq!(archive.files[1].counts.skipped_duplicate, 10);
        assert_eq!(archive.totals.imported, 940);
        assert_eq!(archive.ignored_files.len(), 2);

        let single = Report::parse(
            br#"{"received": 10, "imported": 7, "skipped_short": 2,
                 "skipped_not_track": 1, "skipped_duplicate": 0}"#,
            "Streaming_History_Audio_2024.json",
        )
        .unwrap();
        assert_eq!(single.files.len(), 1);
        assert_eq!(single.files[0].name, "Streaming_History_Audio_2024.json");
        assert_eq!(single.files[0].counts, single.totals);
        assert_eq!(single.totals.imported, 7);
        assert!(single.ignored_files.is_empty());

        assert_eq!(Report::parse(b"not json", "a.zip"), Err(Error::Invalid));
    }
}
