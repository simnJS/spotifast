//! The sink wrapper that runs the equalizer and limits the post-EQ signal.

use std::sync::Arc;

use librespot_playback::SAMPLE_RATE;
use librespot_playback::audio_backend::{Sink, SinkResult};
use librespot_playback::convert::Converter;
use librespot_playback::decoder::AudioPacket;
use librespot_playback::mixer::VolumeGetter;

/// Runs the equalizer and the limiter before passing the signal to the real sink.
pub struct Tapped {
    inner: Box<dyn Sink>,
    control: Arc<crate::sink::AudioControl>,
    eq: crate::eq::Processor,
    /// Player volume, used to calculate the limiter ceiling.
    volume: Box<dyn VolumeGetter + Send>,
    /// Whether this wrapper applies volume after the equalizer. Otherwise the
    /// inner sink applies it, to already queued audio.
    applies_volume: bool,
    /// Final limiter, placed here because this stage knows the output volume.
    limiter: crate::limiter::Limiter,
}

impl Tapped {
    pub fn new(
        inner: Box<dyn Sink>,
        control: Arc<crate::sink::AudioControl>,
        volume: Box<dyn VolumeGetter + Send>,
        applies_volume: bool,
        eq: crate::eq::SharedEq,
    ) -> Self {
        Self {
            inner,
            control,
            eq: crate::eq::Processor::new(eq),
            volume,
            applies_volume,
            limiter: crate::limiter::Limiter::new(f64::from(SAMPLE_RATE)),
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
        self.inner.stop()
    }

    fn write(&mut self, packet: AudioPacket, converter: &mut Converter) -> SinkResult<()> {
        if self.control.take_processing_reset() {
            self.eq.reset();
            self.limiter = crate::limiter::Limiter::new(f64::from(SAMPLE_RATE));
        }
        let packet = match packet {
            AudioPacket::Samples(mut samples) => {
                self.eq.process(&mut samples);
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
    use std::sync::Mutex;

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
            Box::new(librespot_playback::mixer::NoOpVolume),
            false,
            eq,
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
    fn full_scale_follows_the_volume_still_to_come() {
        assert_eq!(full_scale(0.5, true), Some(1.0), "already applied: one");
        assert_eq!(full_scale(0.25, false), Some(4.0), "a quarter to come");
        assert_eq!(full_scale(1.0, false), Some(1.0), "full volume to come");
        assert_eq!(full_scale(0.0, false), None, "silence has no ceiling");
    }
}
