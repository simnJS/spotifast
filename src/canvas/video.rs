//! Demuxes a Canvas MP4 and decodes its H.264 track into RGBA frames, looping
//! for as long as frames are asked for. Blocking work: [`super::Player`] runs
//! it on its own thread, never on the UI thread.

use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, anyhow, bail};
use mp4::{MediaType, Mp4Reader};
use openh264::OpenH264API;
use openh264::decoder::{Decoder, DecoderConfig, Flush};
use openh264::formats::YUVSource as _;

/// Larger than any Canvas Spotify serves (portrait, at most 1080p), small
/// enough that a malformed header cannot ask for a huge allocation per frame.
const MAX_SIDE: usize = 4096;
/// A Canvas lasts a few seconds; a file claiming more samples than this is
/// not one.
const MAX_SAMPLES: u32 = 20_000;
const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// One decoded picture and the moment it is due, measured from the first
/// frame of the first loop. Times keep growing across loops.
pub struct Frame {
    pub image: egui::ColorImage,
    pub at: Duration,
}

/// A looping H.264 decoder over one MP4 file.
pub struct Video {
    reader: Mp4Reader<Cursor<Arc<[u8]>>>,
    track: u32,
    samples: u32,
    /// The next sample to send to the decoder, from 1.
    next: u32,
    decoder: Decoder,
    /// SPS and PPS in Annex B, sent before every sync sample so each loop
    /// restarts from a self-contained picture.
    parameters: Vec<u8>,
    length_size: usize,
    /// Presentation times of one loop in display order, in track ticks,
    /// starting at zero.
    times: Vec<u64>,
    /// One loop's length in track ticks.
    span: u64,
    timescale: u32,
    loops: u64,
    /// How many pictures the current loop has produced.
    shown: usize,
    ready: VecDeque<Frame>,
}

impl Video {
    /// Reads the MP4 header and prepares the decoder. Fails on a file with no
    /// H.264 track, which is how the caller knows to keep the cover.
    pub fn open(bytes: Arc<[u8]>) -> anyhow::Result<Self> {
        let size = bytes.len() as u64;
        let mut reader =
            Mp4Reader::read_header(Cursor::new(bytes), size).context("Canvas is not an MP4")?;
        let track = reader
            .tracks()
            .values()
            .find(|track| matches!(track.media_type(), Ok(MediaType::H264)))
            .ok_or_else(|| anyhow!("Canvas has no H.264 track"))?;
        let track_id = track.track_id();
        let timescale = track.timescale();
        if timescale == 0 {
            bail!("Canvas track has no timescale");
        }
        let avcc = &track
            .trak
            .mdia
            .minf
            .stbl
            .stsd
            .avc1
            .as_ref()
            .ok_or_else(|| anyhow!("Canvas track has no avc1 entry"))?
            .avcc;
        let length_size = usize::from(avcc.length_size_minus_one & 0b11) + 1;
        let mut parameters = Vec::new();
        for unit in avcc
            .sequence_parameter_sets
            .iter()
            .chain(&avcc.picture_parameter_sets)
        {
            parameters.extend_from_slice(&START_CODE);
            parameters.extend_from_slice(&unit.bytes);
        }
        if parameters.is_empty() {
            bail!("Canvas track has no parameter sets");
        }
        let samples = reader.sample_count(track_id)?;
        if samples == 0 || samples > MAX_SAMPLES {
            bail!("Canvas track has {samples} samples");
        }

        // Display times come from the sample table; B-frames make them
        // differ from decode order, and the decoder hands pictures back in
        // display order.
        let mut times = Vec::with_capacity(samples as usize);
        let mut first_dts = None;
        let mut end = 0u64;
        for id in 1..=samples {
            let sample = reader
                .read_sample(track_id, id)?
                .ok_or_else(|| anyhow!("Canvas sample {id} is missing"))?;
            first_dts.get_or_insert(sample.start_time);
            let pts = i128::from(sample.start_time) + i128::from(sample.rendering_offset);
            times.push(pts);
            end = end.max(sample.start_time + u64::from(sample.duration));
        }
        let lowest = times.iter().copied().min().unwrap_or(0);
        let mut times: Vec<u64> = times
            .into_iter()
            .map(|pts| u64::try_from(pts - lowest).unwrap_or(0))
            .collect();
        times.sort_unstable();
        let span = end.saturating_sub(first_dts.unwrap_or(0));
        let last = times.last().copied().unwrap_or(0);
        // A loop is at least as long as its last picture's start plus one
        // frame, whatever the durations in the sample table say.
        let frame = span / u64::from(samples).max(1);
        let span = span.max(last + frame.max(1));

        let decoder = Decoder::with_api_config(
            OpenH264API::from_source(),
            DecoderConfig::new().flush_after_decode(Flush::NoFlush),
        )
        .map_err(|error| anyhow!("H.264 decoder: {error}"))?;

        Ok(Self {
            reader,
            track: track_id,
            samples,
            next: 1,
            decoder,
            parameters,
            length_size,
            times,
            span,
            timescale,
            loops: 0,
            shown: 0,
            ready: VecDeque::new(),
        })
    }

    /// How long one pass through the clip takes.
    pub fn loop_duration(&self) -> Duration {
        self.duration(self.span)
    }

    /// The next picture in display order, starting the clip over after its
    /// last one.
    pub fn next_frame(&mut self) -> anyhow::Result<Frame> {
        let mut restarts = 0;
        loop {
            if let Some(frame) = self.ready.pop_front() {
                return Ok(frame);
            }
            if self.next > self.samples {
                let frames = self
                    .decoder
                    .flush_remaining()
                    .map_err(|error| anyhow!("H.264 decoder: {error}"))?;
                let images: Vec<_> = frames.iter().map(to_image).collect::<Result<_, _>>()?;
                for image in images {
                    self.push(image);
                }
                if self.shown == 0 {
                    restarts += 1;
                    if restarts > 1 {
                        bail!("Canvas decoded no pictures");
                    }
                }
                self.next = 1;
                self.loops += 1;
                self.shown = 0;
                continue;
            }
            let sample = self
                .reader
                .read_sample(self.track, self.next)?
                .ok_or_else(|| anyhow!("Canvas sample {} is missing", self.next))?;
            self.next += 1;
            let mut packet = Vec::with_capacity(sample.bytes.len() + self.parameters.len() + 16);
            if sample.is_sync {
                packet.extend_from_slice(&self.parameters);
            }
            annex_b(&sample.bytes, self.length_size, &mut packet)?;
            let image = match self.decoder.decode(&packet) {
                Ok(Some(picture)) => Some(to_image(&picture)?),
                Ok(None) => None,
                Err(error) => bail!("H.264 decoder: {error}"),
            };
            if let Some(image) = image {
                self.push(image);
            }
        }
    }

    fn push(&mut self, image: egui::ColorImage) {
        // A decoder that returns more pictures than the file has samples
        // would put two on one time; drop the extra.
        let Some(&time) = self.times.get(self.shown) else {
            return;
        };
        self.shown += 1;
        let at = self.duration(self.loops * self.span + time);
        self.ready.push_back(Frame { image, at });
    }

    fn duration(&self, ticks: u64) -> Duration {
        let nanos = u128::from(ticks) * 1_000_000_000 / u128::from(self.timescale);
        Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
    }
}

fn to_image(picture: &openh264::decoder::DecodedYUV<'_>) -> anyhow::Result<egui::ColorImage> {
    let (width, height) = picture.dimensions();
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        bail!("Canvas picture is {width}x{height}");
    }
    let mut rgba = vec![0; width * height * 4];
    picture.write_rgba8(&mut rgba);
    Ok(egui::ColorImage::from_rgba_premultiplied(
        [width, height],
        &rgba,
    ))
}

/// Rewrites an MP4 sample's length-prefixed NAL units with Annex B start
/// codes, the form OpenH264 reads.
fn annex_b(mut sample: &[u8], length_size: usize, out: &mut Vec<u8>) -> anyhow::Result<()> {
    while !sample.is_empty() {
        if sample.len() < length_size {
            bail!("Canvas sample ends inside a NAL length");
        }
        let (prefix, rest) = sample.split_at(length_size);
        let length = prefix
            .iter()
            .fold(0usize, |length, byte| (length << 8) | usize::from(*byte));
        if length > rest.len() {
            bail!("Canvas NAL unit runs past its sample");
        }
        let (unit, rest) = rest.split_at(length);
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(unit);
        sample = rest;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Six 72x128 frames at 12 fps, High profile with B-frames, made with
    /// `ffmpeg -f lavfi -i testsrc2=size=72x128:rate=12 -frames:v 6 -an
    /// -c:v libx264 -profile:v high -pix_fmt yuv420p -bf 2 -g 6`.
    pub(crate) const FIXTURE: &[u8] = include_bytes!("fixture.mp4");

    #[test]
    fn decodes_every_frame_at_its_time_and_loops() {
        let mut video = Video::open(Arc::from(FIXTURE)).expect("fixture opens");
        assert_eq!(video.loop_duration(), Duration::from_millis(500));
        let mut times = Vec::new();
        for _ in 0..13 {
            let frame = video.next_frame().expect("frame decodes");
            assert_eq!(frame.image.size, [72, 128]);
            times.push(frame.at.as_millis());
        }
        assert_eq!(
            times,
            [
                0, 83, 166, 250, 333, 416, 500, 583, 666, 750, 833, 916, 1000
            ]
        );
    }

    #[test]
    fn loops_show_the_same_pictures() {
        let mut video = Video::open(Arc::from(FIXTURE)).expect("fixture opens");
        let first: Vec<_> = (0..6)
            .map(|_| video.next_frame().expect("frame").image)
            .collect();
        let second: Vec<_> = (0..6)
            .map(|_| video.next_frame().expect("frame").image)
            .collect();
        assert!(first == second, "the second pass repeats the first");
        assert!(first[0] != first[1], "consecutive frames differ");
    }

    #[test]
    fn rejects_a_file_that_is_not_a_video() {
        assert!(Video::open(Arc::from(&b"not an mp4 at all"[..])).is_err());
    }

    #[test]
    fn rewrites_length_prefixes_as_start_codes() {
        let mut out = Vec::new();
        annex_b(&[0, 0, 0, 2, 0x65, 0x88, 0, 0, 0, 1, 0x06], 4, &mut out).unwrap();
        assert_eq!(out, [0, 0, 0, 1, 0x65, 0x88, 0, 0, 0, 1, 0x06]);
        assert!(annex_b(&[0, 0, 0, 9, 0x65], 4, &mut Vec::new()).is_err());
    }
}
