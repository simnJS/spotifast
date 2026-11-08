//! The native Soloist audio adapter for the Mac. The controller holds source
//! volume at unity after redirecting.
//! The OS callbacks never run Rust DSP: a dedicated worker owns this state.
#[cfg(target_os = "macos")]
use super::{Bridge, State};
#[cfg(target_os = "macos")]
use std::sync::Arc;

struct Audio {
    shared: crate::eq::SharedEq,
    eq: crate::eq::Processor,
    limiter: crate::limiter::Limiter,
    rate: u32,
}
impl Audio {
    fn new(shared: crate::eq::SharedEq) -> Self {
        Self {
            eq: crate::eq::Processor::new(shared.clone()),
            shared,
            limiter: crate::limiter::Limiter::new(44_100.0),
            rate: 44_100,
        }
    }
    fn reset(&mut self, rate: u32) {
        self.rate = rate;
        self.eq = crate::eq::Processor::at_rate(self.shared.clone(), rate);
        self.limiter = crate::limiter::Limiter::new(f64::from(rate));
    }
    fn process(&mut self, samples: &mut [f32], rate: u32, volume: u8) {
        if !(8_000..=192_000).contains(&rate) || !samples.len().is_multiple_of(2) {
            samples.fill(0.0);
            return;
        }
        if self.rate != rate {
            self.reset(rate);
        }
        let mut output: Vec<f64> = samples
            .iter()
            .map(|v| if v.is_finite() { f64::from(*v) } else { 0.0 })
            .collect();
        self.eq.process(&mut output);
        self.limiter.process(&mut output, 1.0);
        // Apply volume after the limiter, including held audio. Cubic gain
        // matches the built-in player and PipeWire route.
        let gain = (f64::from(volume.min(100)) / 100.0).powi(3);
        for (sample, output) in samples.iter_mut().zip(output) {
            *sample = (output * gain) as f32;
        }
    }
}

#[cfg(target_os = "macos")]
mod adapter {
    use super::*;
    use std::{cell::UnsafeCell, ffi::c_void, sync::atomic::Ordering, time::Duration};
    struct Context {
        state: Arc<State>,
        audio: UnsafeCell<Audio>,
    }
    // The native function borrows Context until it returns. Only its worker
    // calls process/event, sequentially; real-time callbacks only call control.
    unsafe extern "C" {
        fn spotifast_soloist_audio(
            pid: u32,
            context: *mut c_void,
            control: extern "C" fn(*mut c_void) -> u32,
            process: extern "C" fn(*mut c_void, *mut f32, usize, u32),
            event: extern "C" fn(*mut c_void, u32),
        ) -> i32;
    }
    extern "C" fn control(context: *mut c_void) -> u32 {
        // SAFETY: the native worker joins its callbacks before returning.
        let state = unsafe { &(*context.cast::<Context>()).state };
        u32::from(state.stop.load(Ordering::Acquire))
            | (u32::from(state.playing.load(Ordering::Acquire)) << 1)
    }
    extern "C" fn process(context: *mut c_void, data: *mut f32, count: usize, rate: u32) {
        // SAFETY: exclusive buffer for this call; Audio has one worker owner.
        let context = unsafe { &*context.cast::<Context>() };
        let samples = unsafe { std::slice::from_raw_parts_mut(data, count) };
        let audio = unsafe { &mut *context.audio.get() };
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let volume = context.state.output_volume.percent();
            audio.process(samples, rate, volume)
        }))
        .is_err()
        {
            samples.fill(0.0);
            context.state.fail("native capture stopped");
        }
    }
    extern "C" fn event(context: *mut c_void, event: u32) {
        // SAFETY: events and processing run serially on the native worker.
        let context = unsafe { &*context.cast::<Context>() };
        match event {
            1 => {
                context.state.routed.store(true, Ordering::Release);
                log::info!("Soloist native audio capture is ready");
            }
            2 => {
                let audio = unsafe { &mut *context.audio.get() };
                audio.reset(audio.rate);
            }
            3 => {
                context.state.fail("native capture stopped");
            }
            _ => {}
        }
    }
    pub fn open(
        config: crate::player::EngineConfig,
        output_volume: super::super::OutputVolume,
    ) -> Option<Bridge> {
        let state = Arc::new(State {
            output_volume,
            ..State::new()
        });
        let worker = state.clone();
        std::thread::Builder::new().name("soloist-native-audio".into()).spawn(move || {
            while worker.child.load(Ordering::Acquire) == 0 && !worker.stop.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(20));
            }
            if worker.stop.load(Ordering::Acquire) { return; }
            let mut context = Context {
                audio: UnsafeCell::new(Audio::new(config.eq)),
                state: worker.clone(),
            };
            // SAFETY: Context stays alive until every native callback ends.
            let status = unsafe { spotifast_soloist_audio(worker.child.load(Ordering::Acquire),
                (&mut context as *mut Context).cast(), control, process, event) };
            worker.routed.store(false, Ordering::Release);
            // Errors before the source is muted leave ordinary Soloist
            // playback alone. The muted failure path sends event 3 and waits
            // for child shutdown instead.
            if status != 0 && !worker.stop.load(Ordering::Acquire) {
                log::warn!("Soloist native audio capture unavailable (OS code {status}); using its ordinary output");
            }
        }).ok()?;
        Some(Bridge { state })
    }
}
#[cfg(target_os = "macos")]
pub(super) use adapter::open;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mac_eq_shapes_the_output() {
        let eq = crate::eq::shared();
        {
            let mut settings = eq.lock().unwrap();
            settings.on = true;
            settings.preamp_db = 6.0;
        }
        let mut audio = Audio::new(eq);
        let mut samples = vec![0.1; 4410];
        audio.process(&mut samples, 44_100, 100);
        let expected = 0.1 * 10_f32.powf(6.0 / 20.0);
        assert!((samples[samples.len() - 1] - expected).abs() < 0.0001);
    }
    #[test]
    fn mac_output_fader_follows_the_cubic_curve_even_at_zero() {
        for rate in [44_100, 48_000] {
            let eq = crate::eq::shared();
            {
                let mut settings = eq.lock().unwrap();
                settings.on = true;
                settings.preamp_db = 6.0;
            }
            let mut audio = Audio::new(eq);
            let expected = 0.1 * 10_f32.powf(6.0 / 20.0);
            // Changes use the same live processor, including its held samples.
            for volume in [100, 50, 0, 25, 100] {
                let mut samples = vec![0.1; rate as usize / 5];
                audio.process(&mut samples, rate, volume);
                let gain = (f32::from(volume) / 100.0).powi(3);
                assert!((samples[samples.len() - 1] - expected * gain).abs() < 0.0001);
                if volume == 0 {
                    assert!(samples.iter().all(|v| *v == 0.0));
                }
            }
        }
    }
    #[test]
    fn pause_and_format_changes_clear_old_audio() {
        let mut audio = Audio::new(crate::eq::shared());
        audio.process(&mut vec![0.25; 4800], 48_000, 100);
        audio.reset(48_000);
        let mut silence = vec![0.0; 4800];
        audio.process(&mut silence, 48_000, 100);
        assert!(silence.iter().all(|v| *v == 0.0));
        let mut silence = vec![0.0; 4410];
        audio.process(&mut silence, 44_100, 100);
        assert!(silence.iter().all(|v| *v == 0.0));
    }
    #[test]
    fn native_capture_never_invents_audio_when_the_source_is_muted() {
        let mut audio = Audio::new(crate::eq::shared());
        let mut silence = vec![0.0; 4800];
        audio.process(&mut silence, 48_000, 100);
        assert!(silence.iter().all(|v| *v == 0.0));
        let mut invalid = vec![f32::NAN; 200];
        audio.process(&mut invalid, 1, 100);
        assert!(invalid.iter().all(|v| *v == 0.0));
    }
}
