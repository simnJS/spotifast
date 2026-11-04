//! The audio tap: the sink wrapper that limits the post-EQ signal and keeps
//! the last half second of it.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use librespot_playback::audio_backend::{Sink, SinkResult};
use librespot_playback::convert::Converter;
use librespot_playback::decoder::AudioPacket;
use librespot_playback::mixer::VolumeGetter;
use librespot_playback::{NUM_CHANNELS, SAMPLE_RATE};

/// Half a second of audio.
const KEPT: usize = SAMPLE_RATE as usize / 2;
/// The last half second of sound, shared between the player's thread and
/// whatever reads it: the mono mix.
pub struct AudioTap {
    samples: Mutex<VecDeque<f32>>,
}

impl std::fmt::Debug for AudioTap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AudioTap")
    }
}

impl Default for AudioTap {
    fn default() -> Self {
        Self {
            samples: Mutex::new(VecDeque::with_capacity(KEPT)),
        }
    }
}

impl AudioTap {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Adds scaled stereo samples to the mono analyser buffer.
    pub fn push(&self, interleaved: &[f64], gain: f32) {
        let mut samples = self.samples.lock().unwrap_or_else(|p| p.into_inner());
        record_mono(&mut samples, interleaved, gain);
    }

    /// `push` for a realtime thread: no allocation, and nothing it would
    /// have to wait for. A buffer being read costs the analyser this block,
    /// which is better than the sound waiting for it.
    #[cfg(target_os = "linux")]
    pub fn try_push(&self, interleaved: &[f64], gain: f32) {
        if let Ok(mut samples) = self.samples.try_lock() {
            record_mono(&mut samples, interleaved, gain);
        }
    }

    /// `clear` for a realtime thread, when it can have the lock.
    #[cfg(target_os = "linux")]
    pub fn try_clear(&self) {
        if let Ok(mut samples) = self.samples.try_lock() {
            samples.clear();
        }
    }

    /// The `count` samples ending `lag` samples before the newest, with
    /// silence where there is less than that.
    pub fn window(&self, count: usize, lag: usize) -> Vec<f32> {
        let samples = self.samples.lock().unwrap_or_else(|p| p.into_inner());
        let end = samples.len().saturating_sub(lag);
        let start = end.saturating_sub(count);
        let mut out = vec![0.0; count];
        let taken = end - start;
        for (slot, sample) in out[count - taken..]
            .iter_mut()
            .zip(samples.range(start..end))
        {
            *slot = *sample;
        }
        out
    }

    pub fn clear(&self) {
        self.samples
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }
}

fn record_mono(samples: &mut VecDeque<f32>, interleaved: &[f64], gain: f32) {
    for frame in interleaved.as_chunks::<{ NUM_CHANNELS as usize }>().0 {
        let mono = frame.iter().sum::<f64>() as f32 / frame.len() as f32 * gain;
        if samples.len() == KEPT {
            samples.pop_front();
        }
        samples.push_back(mono);
    }
}

/// Runs the equalizer and taps the signal before passing it to the real sink.
pub struct Tapped {
    inner: Box<dyn Sink>,
    control: Arc<crate::sink::AudioControl>,
    tap: Arc<AudioTap>,
    eq: crate::eq::Processor,
    /// Player volume, used to calculate the limiter ceiling.
    volume: Box<dyn VolumeGetter + Send>,
    /// Whether this wrapper applies volume after the tap. Otherwise the inner
    /// sink applies it, still after the tap and to already queued audio.
    applies_volume: bool,
    /// Final limiter, placed here because this stage knows the output volume.
    limiter: crate::limiter::Limiter,
    /// Track normalization factor. The tap removes it so visualizers show the
    /// source dynamics, as Winamp's analyser did.
    normalisation: Arc<std::sync::atomic::AtomicU64>,
}

impl Tapped {
    pub fn new(
        inner: Box<dyn Sink>,
        control: Arc<crate::sink::AudioControl>,
        tap: Arc<AudioTap>,
        volume: Box<dyn VolumeGetter + Send>,
        applies_volume: bool,
        eq: crate::eq::SharedEq,
        normalisation: Arc<std::sync::atomic::AtomicU64>,
    ) -> Self {
        Self {
            inner,
            control,
            tap,
            eq: crate::eq::Processor::new(eq),
            volume,
            applies_volume,
            limiter: crate::limiter::Limiter::new(f64::from(SAMPLE_RATE)),
            normalisation,
        }
    }
}

/// Full-scale level for samples leaving `Tapped`.
///
/// This is 1.0 after volume is applied. Before volume, it is the level that
/// becomes 1.0 after the inner sink applies volume.
fn full_scale(volume: f64, applied: bool) -> Option<f64> {
    if applied {
        Some(1.0)
    } else if volume > f64::EPSILON {
        Some(1.0 / volume)
    } else {
        // At zero volume, no finite pre-volume ceiling is needed.
        None
    }
}

impl Sink for Tapped {
    fn start(&mut self) -> SinkResult<()> {
        self.inner.start()
    }

    fn stop(&mut self) -> SinkResult<()> {
        self.tap.clear();
        self.inner.stop()
    }

    fn write(&mut self, packet: AudioPacket, converter: &mut Converter) -> SinkResult<()> {
        if self.control.take_processing_reset() {
            self.eq.reset();
            self.limiter = crate::limiter::Limiter::new(f64::from(SAMPLE_RATE));
            self.tap.clear();
        }
        let packet = match packet {
            AudioPacket::Samples(mut samples) => {
                self.eq.process(&mut samples);
                // Post-EQ, pre-volume, pre-normalisation: the equalizer
                // shapes what the bars show; the volume knob and the
                // loudness housekeeping never move them.
                let factor = f64::from_bits(
                    self.normalisation
                        .load(std::sync::atomic::Ordering::Relaxed),
                );
                let restore = if factor > 0.05 && factor < 20.0 {
                    (1.0 / factor).clamp(0.125, 8.0) as f32
                } else {
                    1.0
                };
                self.tap.push(&samples, restore);
                let attenuation = self.volume.attenuation_factor();
                if self.applies_volume {
                    for sample in &mut samples {
                        *sample *= attenuation;
                    }
                }
                if let Some(full_scale) = full_scale(attenuation, self.applies_volume) {
                    self.limiter.process(&mut samples, full_scale);
                }
                AudioPacket::Samples(samples)
            }
            raw => raw,
        };
        self.inner.write(packet, converter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RecordedSink(Arc<Mutex<Vec<f64>>>);

    impl Sink for RecordedSink {
        fn start(&mut self) -> SinkResult<()> {
            Ok(())
        }

        fn stop(&mut self) -> SinkResult<()> {
            Ok(())
        }

        fn write(&mut self, packet: AudioPacket, _: &mut Converter) -> SinkResult<()> {
            self.0
                .lock()
                .unwrap()
                .extend_from_slice(packet.samples().unwrap());
            Ok(())
        }
    }

    fn recorded_tap(
        control: Arc<crate::sink::AudioControl>,
        eq: crate::eq::SharedEq,
    ) -> (Tapped, Arc<Mutex<Vec<f64>>>) {
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let tapped = Tapped::new(
            Box::new(RecordedSink(Arc::clone(&recorded))),
            control,
            AudioTap::new(),
            Box::new(librespot_playback::mixer::NoOpVolume),
            false,
            eq,
            Arc::new(std::sync::atomic::AtomicU64::new(1.0f64.to_bits())),
        );
        (tapped, recorded)
    }

    fn write_level(tapped: &mut Tapped, level: f64) {
        tapped
            .write(
                AudioPacket::Samples(vec![level; 2048]),
                &mut Converter::new(None),
            )
            .unwrap();
    }

    #[test]
    fn replacing_a_track_discards_held_audio_before_its_silent_intro() {
        for equalizer_on in [false, true] {
            let control = crate::sink::AudioControl::new(crate::sink::DEFAULT_BUFFER_MS);
            let eq = crate::eq::shared();
            {
                let mut settings = eq.lock().unwrap();
                settings.on = equalizer_on;
                settings.bands_db[0] = 12.0;
            }
            let (mut tapped, recorded) = recorded_tap(Arc::clone(&control), eq);
            write_level(&mut tapped, 0.25);
            control.interrupt();
            // Old packets can still arrive while the replacement is loading.
            write_level(&mut tapped, 0.25);
            control.interrupt();
            control.track_changed();
            recorded.lock().unwrap().clear();
            write_level(&mut tapped, 0.0);
            assert!(
                recorded.lock().unwrap().iter().all(|sample| *sample == 0.0),
                "the old limiter delay and equalizer tail must not reach the new track"
            );
        }
    }

    #[test]
    fn a_confirmed_seek_discards_the_processing_history() {
        let control = crate::sink::AudioControl::new(crate::sink::DEFAULT_BUFFER_MS);
        let (mut tapped, recorded) = recorded_tap(Arc::clone(&control), crate::eq::shared());
        write_level(&mut tapped, 0.25);
        control.handle_player_event(&librespot_playback::player::PlayerEvent::Seeked {
            play_request_id: 1,
            track_id: librespot_core::SpotifyUri::from_uri("spotify:track:14XWXWv5FoCbFzLksawpEe")
                .unwrap(),
            position_ms: 0,
        });
        recorded.lock().unwrap().clear();
        write_level(&mut tapped, 0.0);
        assert!(recorded.lock().unwrap().iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn natural_track_changes_and_pause_keep_the_limiter_tail() {
        let control = crate::sink::AudioControl::new(crate::sink::DEFAULT_BUFFER_MS);
        let (mut tapped, recorded) = recorded_tap(Arc::clone(&control), crate::eq::shared());
        write_level(&mut tapped, 0.25);
        control.track_changed();
        tapped.stop().unwrap();
        tapped.start().unwrap();
        recorded.lock().unwrap().clear();
        write_level(&mut tapped, 0.0);
        let samples = recorded.lock().unwrap();
        assert!(samples[..706].iter().all(|sample| *sample == 0.25));
        assert!(samples[706..].iter().all(|sample| *sample == 0.0));
    }
    #[test]
    fn the_tap_mixes_stereo_down_and_pads_with_silence() {
        let tap = AudioTap::new();
        let interleaved: Vec<f64> = vec![0.5, -0.5, 1.0, 0.0, 0.2, 0.2];
        tap.push(&interleaved, 2.0);
        assert_eq!(tap.window(5, 0), [0.0, 0.0, 0.0, 1.0, 0.4]);
        assert_eq!(tap.window(2, 1), [0.0, 1.0]);
        tap.clear();
        assert_eq!(tap.window(2, 0), [0.0, 0.0]);
    }

    #[test]
    fn the_tap_keeps_half_a_second_at_most() {
        let tap = AudioTap::new();
        let interleaved = vec![0.25f64; 2 * (KEPT + 100)];
        tap.push(&interleaved, 1.0);
        let samples = tap.samples.lock().unwrap();
        assert_eq!(samples.len(), KEPT);
    }

    #[test]
    fn full_scale_follows_the_volume_still_to_come() {
        assert_eq!(full_scale(0.5, true), Some(1.0), "already applied: one");
        assert_eq!(full_scale(0.25, false), Some(4.0), "a quarter to come");
        assert_eq!(full_scale(1.0, false), Some(1.0), "full volume to come");
        assert_eq!(full_scale(0.0, false), None, "silence has no ceiling");
    }
}
