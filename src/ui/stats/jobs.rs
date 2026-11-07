//! The Stats page's work that must not wait on the UI thread: reading a
//! range's summary from a large log, the native file dialogs, and drawing
//! and saving the share card. Each runs on a thread of its own and leaves
//! its answer in a [`Pending`] the page takes on a later frame.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::images::ArtLoader;
use crate::share_card::{self, Card, Cover};
use crate::stats::Index;

use super::summary::{self, Span, Summary};

/// An answer still on its way.
pub struct Pending<T>(Arc<Mutex<Option<T>>>);

impl<T> Clone for Pending<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T: Send + 'static> Pending<T> {
    /// Runs `work` on a thread of its own, and asks for a frame once it
    /// has answered.
    pub fn spawn(ctx: &egui::Context, name: &str, work: impl FnOnce() -> T + Send + 'static) -> Self {
        let slot = Arc::new(Mutex::new(None));
        let answer = Arc::clone(&slot);
        let ctx = ctx.clone();
        let started = std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                let value = work();
                *answer.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(value);
                ctx.request_repaint();
            });
        if let Err(error) = started {
            log::warn!("could not start the {name} thread: {error}");
        }
        Self(slot)
    }

    /// The answer, once, when it has come.
    pub fn take(&self) -> Option<T> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }
}

/// Reads `span`'s summary from `index` off the UI thread.
pub fn summarize(ctx: &egui::Context, index: Arc<Index>, span: Span, now_ms: i64) -> Pending<Summary> {
    Pending::spawn(ctx, "stats-summary", move || {
        summary::summarize(&index, span, now_ms, &jiff::tz::TimeZone::system())
    })
}

/// Waits for a native dialog's future on this thread. rfd builds the
/// dialog on the UI thread (AppKit needs it there) and only its answer is
/// awaited here.
fn wait<F: std::future::Future>(future: F) -> Option<F::Output> {
    match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => Some(runtime.block_on(future)),
        Err(error) => {
            log::warn!("could not wait for the file dialog: {error}");
            None
        }
    }
}

/// Asks for a streaming history: Spotify's ZIP or one of its JSON files.
/// Built on the UI thread, answered on another.
pub fn pick_history(ctx: &egui::Context, title: &str) -> Pending<Option<PathBuf>> {
    let dialog = rfd::AsyncFileDialog::new()
        .set_title(title)
        .add_filter("Spotify streaming history", &["zip", "json"])
        .pick_file();
    Pending::spawn(ctx, "stats-pick", move || {
        wait(dialog)
            .flatten()
            .map(|handle| handle.path().to_path_buf())
    })
}

/// How a share ended.
pub struct Shared {
    /// The card, for the clipboard.
    pub image: egui::ColorImage,
    /// Where it was saved: nowhere when the dialog was cancelled.
    pub saved: Result<Option<PathBuf>, String>,
}

/// How long the card waits for covers still downloading before drawing
/// their placeholders.
const COVER_WAIT: Duration = Duration::from_secs(8);

/// Reads the covers the art loader has on disk, waiting a little for those
/// on their way. A missing one stays `None`.
fn covers(art: &ArtLoader, urls: &[Option<String>]) -> Vec<Option<Cover>> {
    let deadline = Instant::now() + COVER_WAIT;
    let mut covers: Vec<Option<Cover>> = vec![None; urls.len()];
    // Read, or never to be: no URL, or a file that would not decode and
    // is drawn as a placeholder.
    let mut settled: Vec<bool> = urls.iter().map(Option::is_none).collect();
    loop {
        for ((url, cover), settled) in urls.iter().zip(&mut covers).zip(&mut settled) {
            let Some(url) = url.as_deref().filter(|_| !*settled) else {
                continue;
            };
            if let Some(bytes) = art.cached_file(url).and_then(|path| std::fs::read(path).ok()) {
                *cover = Cover::decode(&bytes, 320);
                *settled = true;
            } else if art.is_coverless(url) {
                *settled = true;
            }
        }
        if settled.iter().all(|settled| *settled) || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    covers
}

/// Draws `card` with the covers at `urls` (its artists' then its tracks'),
/// copies nothing itself, and saves it where the save dialog answers.
pub fn share(
    ctx: &egui::Context,
    art: ArtLoader,
    mut card: Card,
    urls: Vec<Option<String>>,
    title: &str,
) -> Pending<Shared> {
    for url in urls.iter().flatten() {
        art.prefetch(ctx, url);
    }
    let dialog = rfd::AsyncFileDialog::new()
        .set_title(title)
        .set_file_name(share_card::file_name(&card.range))
        .add_filter("PNG", &["png"])
        .save_file();
    Pending::spawn(ctx, "stats-share", move || {
        let mut covers = covers(&art, &urls).into_iter();
        for entry in card.artists.iter_mut().chain(card.tracks.iter_mut()) {
            entry.cover = covers.next().flatten();
        }
        let image = share_card::render(&card);
        let saved = match wait(dialog).flatten() {
            None => Ok(None),
            Some(handle) => {
                let path = handle.path().to_path_buf();
                share_card::png(&image).and_then(|png| {
                    fastframe_file::write(&path, png)
                        .map(|()| Some(path))
                        .map_err(|error| error.to_string())
                })
            }
        };
        Shared { image, saved }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pending_answer_is_taken_once() {
        let ctx = egui::Context::default();
        let pending = Pending::spawn(&ctx, "test", || 42);
        let deadline = Instant::now() + Duration::from_secs(5);
        let value = loop {
            if let Some(value) = pending.take() {
                break value;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(value, 42);
        assert_eq!(pending.take(), None);
    }
}
