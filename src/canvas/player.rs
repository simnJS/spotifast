//! Plays a Canvas video into an egui texture. A background thread decodes a
//! few frames ahead; the UI thread only uploads the frame that is due.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use super::video::{Frame, Video};

/// Frames decoded ahead of the one on screen. Each 720x1280 frame is about
/// 3.7 MB, so the buffer stays short; a full channel also parks the decode
/// thread while playback is paused.
const AHEAD: usize = 3;
/// How soon to look again while the decoder has nothing ready yet.
const WAITING: Duration = Duration::from_millis(15);
/// A gap between two draws longer than this means the clip was off screen
/// (a hidden panel, a minimized window). That time is not played, so the
/// decoder never races through it to catch up.
const UNSEEN: Duration = Duration::from_millis(500);

const TEXTURE: egui::TextureOptions = egui::TextureOptions::LINEAR;

/// A looping, silent Canvas video. Drop it to stop its thread.
pub struct Player {
    frames: Receiver<Result<Frame, String>>,
    /// The next frame, received but not yet due.
    pending: Option<Frame>,
    texture: Option<egui::TextureHandle>,
    /// When the frame on screen is due, on the clip's own timeline.
    shown: Option<Duration>,
    clock: Clock,
    /// When [`Player::frame`] last ran.
    drawn: Option<Instant>,
    error: Option<String>,
}

impl Player {
    /// Starts decoding `bytes`, an MP4 file, on a thread of its own. The
    /// clip plays from its first frame; [`Player::pause`] holds it.
    pub fn new(bytes: Arc<[u8]>) -> Self {
        let (sender, frames) = mpsc::sync_channel(AHEAD);
        let spawned = std::thread::Builder::new()
            .name("canvas-decode".into())
            .spawn({
                let sender = sender.clone();
                move || {
                    let mut video = match Video::open(bytes) {
                        Ok(video) => video,
                        Err(error) => {
                            let _ = sender.send(Err(format!("{error:#}")));
                            return;
                        }
                    };
                    loop {
                        let frame = video.next_frame().map_err(|error| format!("{error:#}"));
                        let failed = frame.is_err();
                        // A dropped Player closes the channel, which ends
                        // the thread here.
                        if sender.send(frame).is_err() || failed {
                            return;
                        }
                    }
                }
            });
        let error = spawned
            .err()
            .map(|error| format!("Canvas decode thread: {error}"));
        drop(sender);
        Self {
            frames,
            pending: None,
            texture: None,
            shown: None,
            clock: Clock::default(),
            drawn: None,
            error,
        }
    }

    /// The texture to draw now, after uploading the frame that is due at
    /// `now`. `None` until the first frame is decoded, and for a clip that
    /// failed before it had one: draw the cover then. Asks `ctx` to repaint
    /// when the next frame is due, and only while playing.
    pub fn frame(&mut self, ctx: &egui::Context, now: Instant) -> Option<egui::TextureHandle> {
        if self.texture.is_none() {
            let first = self.receive()?;
            self.upload(ctx, first);
        }
        if let Some(drawn) = self.drawn.replace(now)
            && self.clock.playing
            && now.saturating_duration_since(drawn) > UNSEEN
        {
            // Played up to the last draw, then on from now.
            self.clock.pause(drawn);
            self.clock.playing = true;
        }
        if !self.clock.playing {
            return self.texture.clone();
        }
        let position = self.clock.start(now);
        let mut due = None;
        while let Some(frame) = self.pending.take().or_else(|| self.receive()) {
            if frame.at > position {
                self.pending = Some(frame);
                break;
            }
            // When decoding fell behind, skip straight to the newest frame
            // that is due rather than uploading each late one.
            due = Some(frame);
        }
        if let Some(frame) = due {
            self.upload(ctx, frame);
        }
        match &self.pending {
            Some(next) => ctx.request_repaint_after(next.at.saturating_sub(position)),
            None if self.error.is_none() => ctx.request_repaint_after(WAITING),
            None => {}
        }
        self.texture.clone()
    }

    /// Holds the current frame, as when the music pauses.
    pub fn pause(&mut self, now: Instant) {
        self.clock.pause(now);
    }

    /// Carries on from the held frame.
    pub fn resume(&mut self) {
        self.clock.playing = true;
    }

    pub fn is_playing(&self) -> bool {
        self.clock.playing
    }

    /// Why the clip stopped, if it did. Frames already shown stay, but a
    /// clip that failed before its first frame never gets a texture.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// When the frame on screen is due on the clip's timeline, which keeps
    /// counting across loops.
    pub fn shown_at(&self) -> Option<Duration> {
        self.shown
    }

    fn receive(&mut self) -> Option<Frame> {
        if self.error.is_some() {
            return None;
        }
        match self.frames.try_recv() {
            Ok(Ok(frame)) => Some(frame),
            Ok(Err(error)) => {
                self.error = Some(error);
                None
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.error = Some("Canvas decode thread stopped".into());
                None
            }
        }
    }

    fn upload(&mut self, ctx: &egui::Context, frame: Frame) {
        self.shown = Some(frame.at);
        match &mut self.texture {
            Some(texture) => texture.set(frame.image, TEXTURE),
            None => self.texture = Some(ctx.load_texture("canvas", frame.image, TEXTURE)),
        }
    }
}

/// Playback time that advances only while playing. It starts on the first
/// call after a resume, so the time spent decoding the first frame or
/// paused is never skipped over.
struct Clock {
    playing: bool,
    /// Time played before `since`.
    base: Duration,
    since: Option<Instant>,
}

impl Default for Clock {
    fn default() -> Self {
        Self {
            playing: true,
            base: Duration::ZERO,
            since: None,
        }
    }
}

impl Clock {
    /// The current position, starting the clock at `now` if it is not
    /// running yet.
    fn start(&mut self, now: Instant) -> Duration {
        let since = *self.since.get_or_insert(now);
        self.base + now.saturating_duration_since(since)
    }

    fn pause(&mut self, now: Instant) {
        if let Some(since) = self.since.take() {
            self.base += now.saturating_duration_since(since);
        }
        self.playing = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Polls until the decode thread has the first frame and a full buffer
    /// behind it, so the timing checks below do not race it.
    fn first_frame(player: &mut Player, ctx: &egui::Context, now: Instant) -> egui::TextureHandle {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(texture) = player.frame(ctx, now) {
                std::thread::sleep(Duration::from_millis(200));
                return texture;
            }
            assert!(
                Instant::now() < deadline,
                "no first frame: {:?}",
                player.error()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn player() -> Player {
        Player::new(Arc::from(super::super::video::tests::FIXTURE))
    }

    #[test]
    fn shows_the_frame_due_now_and_holds_it_while_paused() {
        let ctx = egui::Context::default();
        let start = Instant::now();
        let mut player = player();
        let texture = first_frame(&mut player, &ctx, start);
        assert_eq!(texture.size(), [72, 128]);
        assert_eq!(player.shown_at(), Some(Duration::ZERO));

        player.frame(&ctx, start + Duration::from_millis(100));
        assert_eq!(player.shown_at(), Some(Duration::from_nanos(83_333_333)));

        player.pause(start + Duration::from_millis(100));
        player.frame(&ctx, start + Duration::from_secs(30));
        assert_eq!(player.shown_at(), Some(Duration::from_nanos(83_333_333)));

        // Resumed 30 s later, the clip carries on from 100 ms.
        player.resume();
        let resumed = start + Duration::from_secs(30);
        player.frame(&ctx, resumed);
        player.frame(&ctx, resumed + Duration::from_millis(70));
        assert_eq!(player.shown_at(), Some(Duration::from_nanos(166_666_666)));
        assert!(player.error().is_none());
    }

    #[test]
    fn time_off_screen_is_not_played() {
        let ctx = egui::Context::default();
        let start = Instant::now();
        let mut player = player();
        let _ = first_frame(&mut player, &ctx, start);
        player.frame(&ctx, start + Duration::from_millis(90));
        assert_eq!(player.shown_at(), Some(Duration::from_nanos(83_333_333)));
        // Drawn again ten seconds later, it carries on from where it was
        // seen rather than skipping ten seconds of clip.
        let back = start + Duration::from_secs(10);
        player.frame(&ctx, back);
        assert_eq!(player.shown_at(), Some(Duration::from_nanos(83_333_333)));
        player.frame(&ctx, back + Duration::from_millis(80));
        assert_eq!(player.shown_at(), Some(Duration::from_nanos(166_666_666)));
    }

    #[test]
    fn keeps_looping_past_the_end_of_the_clip() {
        let ctx = egui::Context::default();
        let start = Instant::now();
        let mut player = player();
        let _ = first_frame(&mut player, &ctx, start);
        // Step through two and a half loops, a frame at a time, as repaints
        // would.
        let mut now = start;
        while now < start + Duration::from_millis(1250) {
            now += Duration::from_millis(20);
            player.frame(&ctx, now);
            std::thread::sleep(Duration::from_millis(2));
        }
        let shown = player.shown_at().expect("a frame is on screen");
        assert!(
            shown > Duration::from_millis(1100) && shown <= Duration::from_millis(1250),
            "shown at {shown:?}"
        );
        assert!(player.error().is_none());
    }

    #[test]
    fn a_broken_file_reports_an_error_and_no_texture() {
        let ctx = egui::Context::default();
        let mut player = Player::new(Arc::from(&b"garbage"[..]));
        let deadline = Instant::now() + Duration::from_secs(10);
        while player.error().is_none() {
            assert!(player.frame(&ctx, Instant::now()).is_none());
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(player.frame(&ctx, Instant::now()).is_none());
    }
}
