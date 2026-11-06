//! Desktop notifications for new releases from the artists the account
//! follows, as the bell in Spotify's own apps tells of them.
//!
//! Each answer from Spotify's What's New feed (see [`crate::new_releases`])
//! is compared with the releases already seen, kept per account in a small
//! state file. An account Spotifast has not seen before, a missing file and
//! an unreadable one are seeded silently, so the first start never floods
//! the desktop. Afterwards each release the feed adds is announced once, at
//! most [`MAX_SHOWN`] at a time with one summary for the rest, and counted on
//! the New releases entry until that page is opened. Nothing is sent back to
//! Spotify: the feed stays unread in Spotify's apps.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::mpsc;

use serde::{Deserialize, Serialize};

use crate::api::models::Album;

/// The most releases announced one by one from one answer; the rest share
/// a single summary.
pub const MAX_SHOWN: usize = 5;

/// How often the feed is read again while Spotifast runs.
pub const REFRESH: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// The releases remembered per account. The feed holds 50, so this keeps
/// a release that leaves it and comes back from being announced again.
const REMEMBERED: usize = 500;

/// Notification handles kept for their clicks; older ones are released and
/// stay in the desktop's history, as the crate's README advises.
const KEPT_HANDLES: usize = 32;

/// The file's format version.
const VERSION: u32 = 1;

/// What one account has seen of the feed.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct AccountReleases {
    /// Release URIs already read in the feed, oldest first.
    #[serde(default)]
    pub seen: Vec<String>,
    /// Releases announced since the New releases page was last opened.
    #[serde(default)]
    pub unread: Vec<String>,
}

/// The seen releases of every account signed in here.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReleaseState {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub accounts: BTreeMap<String, AccountReleases>,
}

/// What an answer from the feed is to announce.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    /// Releases announced one by one, in the feed's order.
    pub shown: Vec<Album>,
    /// New releases past [`MAX_SHOWN`], announced together.
    pub more: usize,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.shown.is_empty() && self.more == 0
    }
}

impl ReleaseState {
    /// Reads the file. A missing or unreadable one starts empty, so every
    /// account seeds again instead of announcing the whole feed.
    pub fn load(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(error) => {
                log::warn!("could not read the seen new releases, starting afresh: {error}");
                return Self::default();
            }
        };
        Self::decode(&text).unwrap_or_else(|| {
            log::warn!("the seen new releases are unreadable, starting afresh");
            Self::default()
        })
    }

    pub fn decode(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok()
    }

    pub fn encode(&self) -> Option<String> {
        let mut state = self.clone();
        state.version = VERSION;
        serde_json::to_string(&state).ok()
    }

    /// Takes in one answer from the feed for `account` and says what to
    /// announce. The first answer for an account only seeds what it has
    /// seen. `changed` tells whether the state needs saving.
    pub fn observe(&mut self, account: &str, albums: &[Album]) -> (Plan, bool) {
        let uris = albums.iter().map(|album| album.uri.as_str());
        let Some(known) = self.accounts.get_mut(account) else {
            let mut seen = Vec::new();
            for uri in uris {
                if !seen.iter().any(|known| known == uri) {
                    seen.push(uri.to_owned());
                }
            }
            self.accounts.insert(
                account.to_owned(),
                AccountReleases {
                    seen,
                    unread: Vec::new(),
                },
            );
            return (Plan::default(), true);
        };
        let mut fresh: Vec<&Album> = Vec::new();
        for album in albums {
            if album.uri.is_empty()
                || known.seen.contains(&album.uri)
                || fresh.iter().any(|seen| seen.uri == album.uri)
            {
                continue;
            }
            fresh.push(album);
        }
        if fresh.is_empty() {
            return (Plan::default(), false);
        }
        for album in &fresh {
            known.seen.push(album.uri.clone());
            known.unread.push(album.uri.clone());
        }
        let over = known.seen.len().saturating_sub(REMEMBERED);
        known.seen.drain(..over);
        let over = known.unread.len().saturating_sub(REMEMBERED);
        known.unread.drain(..over);
        let more = fresh.len().saturating_sub(MAX_SHOWN);
        let shown = fresh.into_iter().take(MAX_SHOWN).cloned().collect();
        (Plan { shown, more }, true)
    }

    /// Releases announced to `account` since it last opened New releases.
    pub fn unread(&self, account: &str) -> usize {
        self.accounts
            .get(account)
            .map_or(0, |account| account.unread.len())
    }

    /// Clears the badge, here only. Answers whether anything changed.
    pub fn mark_read(&mut self, account: &str) -> bool {
        self.accounts
            .get_mut(account)
            .is_some_and(|account| !std::mem::take(&mut account.unread).is_empty())
    }
}

/// What a click on a notification asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Click {
    /// Open this album.
    Album(String),
    /// Open the New releases page.
    NewReleases,
}

/// The desktop's notification service, with the clicks it reports.
pub struct Notifier {
    client: Option<fastframe_notifications::Client>,
    handles: std::collections::VecDeque<fastframe_notifications::Handle>,
    clicks: mpsc::Receiver<Click>,
    sender: mpsc::Sender<Click>,
    wake: Option<crate::backend::Waker>,
    /// Whether a failure has been logged, so it is logged once.
    warned: bool,
}

impl Notifier {
    /// Connects to the desktop's service on its own worker. `wake` is
    /// `None` in tests and demos, which announce nothing.
    pub fn new(wake: Option<crate::backend::Waker>) -> Self {
        let (sender, clicks) = mpsc::channel();
        let mut warned = false;
        let client = wake.as_ref().and_then(|_| match client() {
            Ok(client) => Some(client),
            Err(error) => {
                warned = true;
                log::warn!("desktop notifications are unavailable: {error}");
                None
            }
        });
        Self {
            client,
            handles: std::collections::VecDeque::new(),
            clicks,
            sender,
            wake,
            warned,
        }
    }

    /// Queues one notification; delivery happens on the service's worker.
    pub fn show(&mut self, notification: fastframe_notifications::Notification, click: Click) {
        let Some(client) = &self.client else {
            return;
        };
        let sender = self.sender.clone();
        let wake = self.wake.clone();
        let result = client.show_with(notification, move |event| match event {
            fastframe_notifications::Event::Activated
            | fastframe_notifications::Event::Action(_) => {
                if sender.send(click.clone()).is_ok()
                    && let Some(wake) = &wake
                {
                    wake.wake();
                }
            }
            fastframe_notifications::Event::Failed(error) => {
                log::debug!("a new release notification failed: {error}");
            }
            _ => {}
        });
        match result {
            Ok(handle) => {
                self.handles.push_back(handle);
                while self.handles.len() > KEPT_HANDLES {
                    self.handles.pop_front();
                }
            }
            Err(error) => {
                if !std::mem::replace(&mut self.warned, true) {
                    log::warn!("could not show a new release notification: {error}");
                }
            }
        }
    }

    /// The clicks reported since the last call.
    pub fn clicks(&self) -> Vec<Click> {
        self.clicks.try_iter().collect()
    }

    /// Whether this run announces releases at all: tests and demos do not,
    /// and do not read the feed on its schedule either. A desktop without a
    /// notification service still gets the badge.
    pub fn is_enabled(&self) -> bool {
        self.wake.is_some()
    }
}

/// Spotifast's identity for the desktop, the one its media controls and
/// window already use: the desktop entry (Flatpak's exported one inside a
/// Flatpak) on Linux, the `spotifast` ID elsewhere.
fn client() -> Result<fastframe_notifications::Client, fastframe_notifications::Error> {
    let app = fastframe_notifications::App::new("spotifast", "Spotifast");
    #[cfg(target_os = "linux")]
    let app = {
        let entry = fastframe_now_playing::desktop_entry("spotifast");
        app.icon(entry.clone()).desktop_entry(entry)
    };
    // Windows shows a blank square without a logo file; the tile takes
    // the dark theme's accent.
    #[cfg(target_os = "windows")]
    let app = app.logo(
        include_bytes!("../packaging/macos/icon-1024.png"),
        [0x1e, 0xd7, 0x60],
    );
    fastframe_notifications::Client::new(app)
}

/// A release's type as the notification names it: album, single or EP.
pub fn kind(album: &Album, locale: crate::i18n::Locale) -> std::borrow::Cow<'static, str> {
    use crate::i18n::gettext;
    match album
        .album_type
        .as_deref()
        .or(album.album_group.as_deref())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("single") => gettext(locale, "Single"),
        Some("ep") => gettext(locale, "EP"),
        Some("compilation") => gettext(locale, "Compilation"),
        _ => gettext(locale, "Album"),
    }
}

/// The notification for one release: the artist, then the release and
/// its type, with the cover when it is already on the disk.
pub fn release_notification(
    album: &Album,
    locale: crate::i18n::Locale,
    cover: Option<std::path::PathBuf>,
) -> fastframe_notifications::Notification {
    let artists = crate::api::models::join_names(album.artists.iter().map(|a| a.name.as_str()));
    let title = if artists.is_empty() {
        album.name.clone()
    } else {
        artists
    };
    let notification = fastframe_notifications::Notification::new(title)
        .body(format!("{} • {}", album.name, kind(album, locale)))
        .key(format!("new-release-{}", album.id));
    match cover {
        Some(cover) => notification.image(cover),
        None => notification,
    }
}

/// The one notification for the releases past [`MAX_SHOWN`].
pub fn more_notification(
    more: usize,
    locale: crate::i18n::Locale,
) -> fastframe_notifications::Notification {
    let count = u32::try_from(more).unwrap_or(u32::MAX);
    let body = crate::i18n::ngettext(
        locale,
        // Translators: Keep {count} exactly as written. It becomes a number of releases.
        "And {count} more new release",
        "And {count} more new releases",
        count,
    )
    .replace("{count}", &count.to_string());
    fastframe_notifications::Notification::new(crate::i18n::gettext(locale, "New releases"))
        .body(body)
        .key("new-releases-more")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(n: usize) -> Album {
        let id = format!("{n:022}");
        Album {
            uri: format!("spotify:album:{id}"),
            id,
            name: format!("Release {n}"),
            album_type: Some("single".into()),
            artists: vec![crate::api::models::ArtistRef {
                id: None,
                name: format!("Artist {n}"),
                uri: None,
            }],
            ..Album::default()
        }
    }

    fn feed(range: std::ops::Range<usize>) -> Vec<Album> {
        range.map(release).collect()
    }

    #[test]
    fn the_first_answer_seeds_without_announcing() {
        let mut state = ReleaseState::default();
        let (plan, changed) = state.observe("me", &feed(0..20));
        assert!(plan.is_empty());
        assert!(changed);
        assert_eq!(state.accounts["me"].seen.len(), 20);
        assert_eq!(state.unread("me"), 0);
        let (plan, changed) = state.observe("me", &feed(0..20));
        assert!(plan.is_empty());
        assert!(!changed);
    }

    #[test]
    fn a_new_release_is_announced_once() {
        let mut state = ReleaseState::default();
        state.observe("me", &feed(1..4));
        let mut answer = feed(0..1);
        answer.extend(feed(1..4));
        let (plan, changed) = state.observe("me", &answer);
        assert!(changed);
        assert_eq!(plan.shown, vec![release(0)]);
        assert_eq!(plan.more, 0);
        assert_eq!(state.unread("me"), 1);
        let (plan, changed) = state.observe("me", &answer);
        assert!(plan.is_empty());
        assert!(!changed);
    }

    #[test]
    fn seen_releases_stay_seen_across_restarts() {
        let dir = std::env::temp_dir().join(format!(
            "spotifast-seen-releases-{:016x}",
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("new-releases.json");
        let mut state = ReleaseState::default();
        state.observe("me", &feed(0..3));
        state.observe("me", &feed(0..4));
        std::fs::write(&path, state.encode().unwrap()).unwrap();

        let mut restarted = ReleaseState::load(&path);
        assert_eq!(restarted.accounts, state.accounts);
        assert_eq!(restarted.version, VERSION);
        assert_eq!(restarted.unread("me"), 1, "the badge survives a restart");
        let (plan, _) = restarted.observe("me", &feed(0..4));
        assert!(plan.is_empty(), "nothing already seen is announced again");
        let (plan, _) = restarted.observe("me", &feed(0..5));
        assert_eq!(plan.shown, vec![release(4)]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn at_most_five_are_announced_and_the_rest_summed_up() {
        let mut state = ReleaseState::default();
        state.observe("me", &[]);
        let (plan, _) = state.observe("me", &feed(0..12));
        assert_eq!(plan.shown, feed(0..MAX_SHOWN));
        assert_eq!(plan.more, 7);
        assert_eq!(state.unread("me"), 12);
        let (plan, _) = state.observe("me", &feed(0..12));
        assert!(plan.is_empty(), "the summed up releases count as seen");
    }

    #[test]
    fn a_missing_or_unreadable_file_seeds_again() {
        let dir = std::env::temp_dir().join(format!(
            "spotifast-unreadable-releases-{:016x}",
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("new-releases.json");
        assert_eq!(ReleaseState::load(&path), ReleaseState::default());
        std::fs::write(&path, "{not json").unwrap();
        let mut state = ReleaseState::load(&path);
        assert_eq!(state, ReleaseState::default());
        let (plan, _) = state.observe("me", &feed(0..30));
        assert!(
            plan.is_empty(),
            "an unreadable file never floods the desktop"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn each_account_seeds_its_own_releases() {
        let mut state = ReleaseState::default();
        state.observe("me", &feed(0..2));
        let (plan, _) = state.observe("other", &feed(5..9));
        assert!(plan.is_empty());
        let (plan, _) = state.observe("me", &feed(0..3));
        assert_eq!(plan.shown, vec![release(2)]);
    }

    #[test]
    fn opening_the_page_clears_the_badge() {
        let mut state = ReleaseState::default();
        state.observe("me", &feed(0..1));
        state.observe("me", &feed(0..3));
        assert_eq!(state.unread("me"), 2);
        assert!(state.mark_read("me"));
        assert_eq!(state.unread("me"), 0);
        assert!(!state.mark_read("me"));
        assert!(!state.mark_read("nobody"));
    }

    #[test]
    fn the_memory_of_seen_releases_stays_small() {
        let mut state = ReleaseState::default();
        state.observe("me", &[]);
        state.observe("me", &feed(0..REMEMBERED + 20));
        assert_eq!(state.accounts["me"].seen.len(), REMEMBERED);
        assert_eq!(state.accounts["me"].unread.len(), REMEMBERED);
    }

    #[test]
    fn a_notification_names_the_artist_release_and_type() {
        let locale = crate::i18n::Locale::English;
        let mut ep = release(3);
        ep.album_type = Some("EP".into());
        assert_eq!(kind(&ep, locale), "EP");
        assert_eq!(kind(&release(1), locale), "Single");
        ep.album_type = None;
        assert_eq!(kind(&ep, locale), "Album");
    }
}
