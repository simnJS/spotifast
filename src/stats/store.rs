//! The per-account play log on disk.
//!
//! The log is a folder of segments, `plays-<n>.jsonl`, one JSON line per
//! play. New plays are appended as a new segment, each written whole and
//! atomically (a temporary file synced, then moved over), so a crash
//! leaves either the segment or nothing and never half a line. When the
//! segments grow many they are compacted into one, written the same way
//! before the old ones go; a crash in between leaves exact repeats, which
//! the merge on reading drops. The metadata cache sits beside them in
//! `metadata.json`, replaced whole.
//!
//! Everything here waits for the disk: run it on a blocking thread, never
//! on the UI thread.

use std::path::{Path, PathBuf};

use super::PlayRecord;
use super::dedupe::{Merged, merge};
use super::metadata::Metadata;

/// Segments kept before they are compacted into one.
const MAX_SEGMENTS: usize = 24;

fn segment_number(path: &Path) -> Option<u64> {
    path.file_name()?
        .to_str()?
        .strip_prefix("plays-")?
        .strip_suffix(".jsonl")?
        .parse()
        .ok()
}

fn segment_path(dir: &Path, number: u64) -> PathBuf {
    dir.join(format!("plays-{number:010}.jsonl"))
}

fn encode(records: &[PlayRecord]) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(records.len() * 128);
    for record in records {
        serde_json::to_writer(&mut bytes, record).map_err(std::io::Error::other)?;
        bytes.push(b'\n');
    }
    Ok(bytes)
}

/// One account's plays, read from its folder.
#[derive(Debug)]
pub struct Log {
    dir: PathBuf,
    segments: Vec<(u64, PathBuf)>,
    /// Every listen once, oldest first.
    records: Vec<PlayRecord>,
}

/// What appending found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Appended {
    pub added: usize,
    pub duplicates: usize,
}

impl Log {
    /// Reads the log in `dir`; a missing folder is an empty log. A line
    /// that does not read (written by a later version, say) is skipped.
    pub fn open(dir: &Path) -> std::io::Result<Self> {
        let mut segments: Vec<(u64, PathBuf)> = match std::fs::read_dir(dir) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter_map(|path| segment_number(&path).map(|number| (number, path)))
                .collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        segments.sort();
        let mut records = Vec::new();
        let mut unreadable = 0usize;
        for (_, path) in &segments {
            let text = std::fs::read_to_string(path)?;
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                match serde_json::from_str::<PlayRecord>(line) {
                    Ok(record) => records.push(record),
                    Err(_) => unreadable += 1,
                }
            }
        }
        if unreadable > 0 {
            log::warn!("skipped {unreadable} unreadable plays in {}", dir.display());
        }
        let Merged { records, .. } = merge(records, Vec::new());
        Ok(Self {
            dir: dir.to_owned(),
            segments,
            records,
        })
    }

    pub fn records(&self) -> &[PlayRecord] {
        &self.records
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Merges `new` into the log and writes the plays it did not have as
    /// a new segment. The log in memory changes only once the segment is
    /// on the disk.
    pub fn append(&mut self, new: Vec<PlayRecord>) -> std::io::Result<Appended> {
        if new.is_empty() {
            return Ok(Appended::default());
        }
        let Merged {
            records,
            added,
            duplicates,
        } = merge(self.records.clone(), new);
        if !added.is_empty() {
            let number = self.segments.last().map_or(0, |(number, _)| number + 1);
            let path = segment_path(&self.dir, number);
            // What someone listened to is theirs: readable by them only.
            fastframe_file::write_private(&path, encode(&added)?)?;
            self.segments.push((number, path));
        }
        self.records = records;
        let appended = Appended {
            added: added.len(),
            duplicates,
        };
        if self.segments.len() > MAX_SEGMENTS
            && let Err(error) = self.compact()
        {
            log::warn!("could not compact the play log: {error}");
        }
        Ok(appended)
    }

    /// Writes the whole log as one segment and removes the others.
    pub fn compact(&mut self) -> std::io::Result<()> {
        let number = self.segments.last().map_or(0, |(number, _)| number + 1);
        let path = segment_path(&self.dir, number);
        fastframe_file::write_private(&path, encode(&self.records)?)?;
        for (_, old) in std::mem::take(&mut self.segments) {
            if let Err(error) = std::fs::remove_file(&old) {
                log::warn!("could not remove {}: {error}", old.display());
            }
        }
        self.segments.push((number, path));
        Ok(())
    }

    /// The newest `played_at` Spotify's recently-played gave: the cursor
    /// the logger asks after.
    pub fn spotify_cursor(&self) -> Option<i64> {
        self.records
            .iter()
            .rev()
            .find(|record| record.source == super::Source::Spotify)
            .map(|record| record.at_ms)
    }

    /// The cached track and artist details; empty when there are none.
    pub fn read_metadata(&self) -> Metadata {
        std::fs::read(metadata_path(&self.dir))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }
}

fn metadata_path(dir: &Path) -> PathBuf {
    dir.join("metadata.json")
}

/// Replaces the metadata cache of the log in `dir`.
pub fn write_metadata(dir: &Path, metadata: &Metadata) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(metadata).map_err(std::io::Error::other)?;
    fastframe_file::write_private(&metadata_path(dir), bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::{ArtistName, Source};

    fn dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "spotifast-stats-store-{name}-{:016x}",
            rand::random::<u64>()
        ))
    }

    fn play(at_s: i64, track: &str, source: Source) -> PlayRecord {
        PlayRecord {
            at_ms: at_s * 1000,
            ms_played: 180_000,
            estimated: source == Source::Spotify,
            source,
            uri: Some(format!("spotify:track:{track}")),
            track: track.into(),
            artists: vec![ArtistName {
                id: None,
                name: "Band".into(),
            }],
            album: None,
            album_id: None,
            duration_ms: Some(180_000),
            skipped: None,
            shuffle: None,
        }
    }

    #[test]
    fn appended_plays_are_read_back_once() {
        let dir = dir("append");
        let mut log = Log::open(&dir).unwrap();
        assert!(log.records().is_empty());
        let first = log
            .append(vec![
                play(100, "a", Source::Spotify),
                play(500, "b", Source::Spotify),
            ])
            .unwrap();
        assert_eq!(
            first,
            Appended {
                added: 2,
                duplicates: 0
            }
        );
        let second = log
            .append(vec![
                play(500, "b", Source::Spotify),
                play(900, "c", Source::Spotify),
            ])
            .unwrap();
        assert_eq!(
            second,
            Appended {
                added: 1,
                duplicates: 1
            }
        );
        assert_eq!(log.spotify_cursor(), Some(900_000));
        let again = Log::open(&dir).unwrap();
        assert_eq!(again.records(), log.records());
        assert_eq!(again.records().len(), 3);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn many_segments_are_compacted_into_one() {
        let dir = dir("compact");
        let mut log = Log::open(&dir).unwrap();
        for n in 0..(MAX_SEGMENTS as i64 + 2) {
            log.append(vec![play(n * 1000, "a", Source::Spotify)])
                .unwrap();
        }
        let files = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| segment_number(&entry.path()).is_some())
            .count();
        assert!(files <= 2, "{files} segments left");
        assert_eq!(Log::open(&dir).unwrap().records().len(), MAX_SEGMENTS + 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_crash_during_compaction_leaves_no_duplicates() {
        let dir = dir("crash");
        let mut log = Log::open(&dir).unwrap();
        log.append(vec![play(1, "a", Source::Spotify)]).unwrap();
        // The compacted copy written, the old segment not yet removed.
        std::fs::write(segment_path(&dir, 7), encode(log.records()).unwrap()).unwrap();
        assert_eq!(Log::open(&dir).unwrap().records().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_unreadable_line_is_skipped() {
        let dir = dir("unreadable");
        std::fs::create_dir_all(&dir).unwrap();
        let mut text = String::from("not json\n");
        text.push_str(&String::from_utf8(encode(&[play(1, "a", Source::Local)]).unwrap()).unwrap());
        std::fs::write(segment_path(&dir, 0), text).unwrap();
        assert_eq!(Log::open(&dir).unwrap().records().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
