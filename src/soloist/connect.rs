//! Spotify Connect commands to Soloist, sent over the signed-in session.
//!
//! Spotify's own apps drive every Connect device, Soloist included, with
//! player commands posted to the Connect state service
//! (`/connect-state/v1/player/command/from/<this device>/to/<that device>`),
//! not with the Web API. Soloist's local API cannot skip to a given row or
//! change its queue; these commands can. Spotifast already sends them to
//! restricted devices (`restricted_device`). This module builds the ones
//! Spotify's web player sends for a clicked queue row, a queue edit and a
//! start with play options, in that client's format: its `vendor~web-player`
//! bundle (`_sendPlayerCommand`, `nextTrack`, `setInternalPlayerQueue`,
//! `_play`, `_generatePlayOptions`, `setShuffleMode`), and the queue edits of
//! its player API (`removeFromQueue`, `clearQueue`, `reorderQueue`,
//! `insertIntoQueue`). The tests read every body back with the parser
//! librespot uses for the commands Spotify's apps send.
//!
//! # What to call
//!
//! `to` is always the target's Connect device id: Soloist's, as Spotify lists
//! it (the key of the Connect cluster's devices, the Web API's device id).
//!
//! - **Finding 4**, a clicked queue row: `skip_to_row(session, to, &row)`.
//!   One `skip_next` that names the row, so nothing in between plays. Take
//!   the row from a snapshot (`ConnectQueue::find`) when there is one, or
//!   build it from Soloist's `get_queue` entry:
//!   `QueueTrack::row(uri, uid, source)`.
//! - **Finding 6**, editing the queue: read a snapshot with `fetch_queue`
//!   (or `ConnectQueue::from_cluster` on a cluster push), then call
//!   `remove`, `reorder`, `insert` or `clear`. Each rewrites the player's next
//!   tracks with one `set_queue` that carries the snapshot's
//!   `queue_revision`, as Spotify's apps do. `set_queue` sends a list built
//!   by hand from the snapshot's methods (`removing`, `moving`, `inserting`,
//!   `clearing`).
//! - **Finding 3** (and the Connect part of 10 and 11), a start that carries
//!   its options: `play(session, to, &ContextPlay { .. })`. One `play` of the
//!   context with `skip_to` and `player_options_override` (shuffle mode,
//!   repeat), so the shuffle mode applies to the new collection and never to
//!   the old one first. Without `skip_to`, a shuffled start lets the player
//!   pick the first song across the whole collection (10).
//!   `context_metadata` carries a sorted or filtered view
//!   (`sorting.criteria`, `filtering.predicate`), and an artist's Popular
//!   row plays the artist with `skip_to` the song (11).
//!
//! # Falling back
//!
//! Every call returns a `Result` with `ConnectError` and logs the refusal at
//! debug. Any error means the command was not carried out, and the caller
//! does what it does today: one `Next` per row, the Web API's play, or no
//! queue editing. `ConnectError::is_network` is true only for a failure on
//! the way; any other error says this device or this queue state refuses
//! the command, so retrying it unchanged is pointless.
//!
//! Spotify answers once it has passed a command on; the device can still
//! ignore it. A caller that must be sure checks the next state Soloist
//! reports (the playing row's `uid`, the queue) and falls back when it did
//! not change.
//!
//! # What a real device must confirm
//!
//! Nothing here has run against Soloist yet:
//!
//! - Soloist takes a Connect `skip_next` with a `track` and lands on that
//!   row, matching it by `uid` (librespot matches by URI only).
//! - The `uid` Soloist's `get_queue` reports is the Connect row's `uid`.
//! - `GET /connect-state/v1/cluster` answers this session with the active
//!   player's state, `queue_revision` included. If it does not, read the
//!   snapshot from a cluster push (`ConnectQueue::from_cluster`).
//! - Soloist takes a `set_queue` built from that snapshot: it keeps the
//!   context's rows past the ones sent, and refuses a stale revision rather
//!   than losing a change made in between.
//! - A `play` with `player_options_override` starts shuffled (or not) in one
//!   step, and `context_enhancement: NONE` keeps it out of Smart Shuffle.

use std::collections::BTreeMap;

use librespot_core::error::ErrorKind;
use librespot_core::session::Session;
use librespot_protocol::connect::Cluster;
use librespot_protocol::player::{PlayerState, ProvidedTrack};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::restricted_device;

/// The license Spotifast's Connect `play` commands carry, as for restricted
/// devices.
const LICENSE: &str = "on-demand";

/// Who started a playback, as Connect's `play_origin` names it.
const FEATURE: &str = "spotifast";

/// Why a Connect command was not carried out. Whatever the variant, the
/// caller falls back to what it did before this route existed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConnectError {
    /// Spotify or the device refused the command: the session may not
    /// control it, the body was not understood, or the device does not take
    /// this command.
    #[error("Spotify Connect refused the command: {0}")]
    Refused(String),
    /// Spotify no longer knows the target device.
    #[error("Spotify Connect does not know that device: {0}")]
    NoDevice(String),
    /// The player's restrictions forbid rewriting its queue now. Nothing was
    /// sent.
    #[error("the player does not allow changing its queue now: {0:?}")]
    QueueLocked(Vec<String>),
    /// A row named is not in the queue snapshot, which is out of date.
    /// Nothing was sent.
    #[error("the row is not in the player's queue any more")]
    NotInQueue,
    /// Connect described no player, or no queue revision to edit against.
    #[error("Spotify Connect describes no player")]
    NoPlayer,
    /// The request did not get through, or Spotify failed; the same command
    /// can work later.
    #[error("Spotify Connect could not be reached: {0}")]
    Network(String),
}

impl ConnectError {
    /// Whether the command failed on the way rather than being refused: only
    /// then can the same command work later on the same device.
    pub fn is_network(&self) -> bool {
        matches!(self, Self::Network(_))
    }

    /// Sorts Spotify's answer by the HTTP status librespot read it from.
    fn of_kind(kind: ErrorKind, message: String) -> Self {
        match kind {
            ErrorKind::NotFound => Self::NoDevice(message),
            ErrorKind::PermissionDenied
            | ErrorKind::Unauthenticated
            | ErrorKind::InvalidArgument
            | ErrorKind::FailedPrecondition
            | ErrorKind::Aborted
            | ErrorKind::OutOfRange
            | ErrorKind::AlreadyExists
            | ErrorKind::Unimplemented => Self::Refused(message),
            _ => Self::Network(message),
        }
    }
}

impl From<librespot_core::Error> for ConnectError {
    fn from(error: librespot_core::Error) -> Self {
        Self::of_kind(error.kind, error.to_string())
    }
}

/// For a caller that reports through the Web API's error, as the restricted
/// device route does.
impl From<ConnectError> for crate::api::client::ApiError {
    fn from(error: ConnectError) -> Self {
        Self::Network(error.to_string())
    }
}

/// One row of a Connect player's queue: Connect's `ProvidedTrack`, as
/// Spotify's apps send it back in `skip_next` and `set_queue`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct QueueTrack {
    pub uri: String,
    /// The player's id for this row: the same song queued twice has two.
    pub uid: String,
    /// Where the row comes from: `queue`, `context` or `autoplay`, what
    /// Soloist calls its `source`.
    pub provider: String,
    pub metadata: BTreeMap<String, String>,
    /// Why the player removed the row, as it reported them.
    pub removed: Vec<String>,
    /// Why the player blocked the row, as it reported them.
    pub blocked: Vec<String>,
}

impl QueueTrack {
    /// A row as Soloist's `get_queue` names it: its URI, `uid` and `source`.
    pub fn row(
        uri: impl Into<String>,
        uid: impl Into<String>,
        provider: impl Into<String>,
    ) -> Self {
        Self {
            uri: uri.into(),
            uid: uid.into(),
            provider: provider.into(),
            ..Self::default()
        }
    }

    /// Whether the row was queued by hand (*Next in queue*), by the player's
    /// own marks.
    pub fn is_queued(&self) -> bool {
        self.provider == "queue"
            || self
                .metadata
                .get("is_queued")
                .is_some_and(|queued| queued == "true")
    }

    /// Spotify's `isSameItem`: the `uid`s when both rows have one, else the
    /// URIs.
    pub fn same_row(&self, other: &Self) -> bool {
        if self.uid.is_empty() || other.uid.is_empty() {
            self.uri == other.uri
        } else {
            self.uid == other.uid
        }
    }

    /// The row marked as queued, as Spotify's `markAsQueued` does to a row
    /// moved among queued rows.
    fn into_queued(mut self) -> Self {
        self.provider = "queue".into();
        self.metadata.insert("is_queued".into(), "true".into());
        self
    }

    /// The row as Spotify's web player writes a track into a command: queued
    /// rows carry `provider: queue` and `is_queued`, delimiters and other
    /// `spotify:meta:` rows stay hidden, and empty fields are left out.
    pub fn json(&self) -> Value {
        let mut metadata = self.metadata.clone();
        let mut provider = self.provider.as_str();
        if self.is_queued() {
            metadata.insert("is_queued".into(), "true".into());
            provider = "queue";
        }
        if self.uri == "spotify:delimiter" || self.uri.starts_with("spotify:meta:") {
            metadata.insert("hidden".into(), "true".into());
        }
        let mut track = Map::new();
        track.insert("uri".into(), json!(self.uri));
        if !self.uid.is_empty() {
            track.insert("uid".into(), json!(self.uid));
        }
        track.insert("metadata".into(), json!(metadata));
        if !self.removed.is_empty() {
            track.insert("removed".into(), json!(self.removed));
        }
        if !self.blocked.is_empty() {
            track.insert("blocked".into(), json!(self.blocked));
        }
        if !provider.is_empty() {
            track.insert("provider".into(), json!(provider));
        }
        Value::Object(track)
    }
}

impl From<&ProvidedTrack> for QueueTrack {
    fn from(track: &ProvidedTrack) -> Self {
        Self {
            uri: track.uri.clone(),
            uid: track.uid.clone(),
            provider: track.provider.clone(),
            metadata: track
                .metadata
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            removed: track.removed.clone(),
            blocked: track.blocked.clone(),
        }
    }
}

/// Where `moving` and `inserting` put rows: next to a row already queued,
/// as Spotify's queue API takes `{before}` or `{after}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    Before(QueueTrack),
    After(QueueTrack),
}

impl Place {
    fn row(&self) -> &QueueTrack {
        match self {
            Self::Before(row) | Self::After(row) => row,
        }
    }

    fn is_after(&self) -> bool {
        matches!(self, Self::After(_))
    }
}

/// A Connect player's queue as Spotify last described it, to edit from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectQueue {
    /// The player's `queue_revision`, sent back with every `set_queue`.
    pub revision: String,
    /// The device this queue plays on; compare it with the command's target.
    pub device: String,
    /// The playing row.
    pub current: Option<QueueTrack>,
    /// `next_tracks`: the rows queued by hand, then the context's, hidden
    /// rows and delimiters included. A `set_queue` replaces all of them.
    pub next: Vec<QueueTrack>,
    /// `prev_tracks`, sent back unchanged.
    pub prev: Vec<QueueTrack>,
    /// `disallow_set_queue_reasons`: while any is set, the queue cannot
    /// change.
    pub set_queue_disallowed: Vec<String>,
}

/// The parts of the cluster JSON Connect's state service returns that a
/// queue edit needs, with its own field names.
#[derive(Default, Deserialize)]
#[serde(default)]
struct ClusterJson {
    active_device_id: String,
    player_state: Option<PlayerStateJson>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct PlayerStateJson {
    queue_revision: String,
    track: Option<QueueTrack>,
    next_tracks: Vec<QueueTrack>,
    prev_tracks: Vec<QueueTrack>,
    restrictions: RestrictionsJson,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct RestrictionsJson {
    disallow_set_queue_reasons: Vec<String>,
}

impl ConnectQueue {
    /// The queue in a cluster pushed over the session's dealer connection.
    pub fn from_cluster(cluster: &Cluster) -> Option<Self> {
        let state = cluster.player_state.as_ref()?;
        Some(Self {
            device: cluster.active_device_id.clone(),
            ..Self::from_state(state)
        })
    }

    fn from_state(state: &PlayerState) -> Self {
        Self {
            revision: state.queue_revision.clone(),
            device: String::new(),
            current: state.track.as_ref().map(QueueTrack::from),
            next: state.next_tracks.iter().map(QueueTrack::from).collect(),
            prev: state.prev_tracks.iter().map(QueueTrack::from).collect(),
            set_queue_disallowed: state.restrictions.disallow_set_queue_reasons.clone(),
        }
    }

    /// The queue in the cluster JSON Connect's state service returns.
    fn from_cluster_json(body: &[u8]) -> Result<Self, ConnectError> {
        let cluster: ClusterJson = serde_json::from_slice(body).map_err(|error| {
            log::debug!("Unreadable Connect cluster: {error}");
            ConnectError::NoPlayer
        })?;
        let state = cluster.player_state.ok_or(ConnectError::NoPlayer)?;
        Ok(Self {
            revision: state.queue_revision,
            device: cluster.active_device_id,
            current: state.track,
            next: state.next_tracks,
            prev: state.prev_tracks,
            set_queue_disallowed: state.restrictions.disallow_set_queue_reasons,
        })
    }

    /// The next row a row from Soloist names, as Spotify's `skipToNext`
    /// finds it: the same URI and, when the row has one, the same `uid`.
    pub fn find(&self, row: &QueueTrack) -> Option<&QueueTrack> {
        self.next
            .iter()
            .find(|track| track.uri == row.uri && (row.uid.is_empty() || track.uid == row.uid))
    }

    /// `next` without the rows named, any of them, a context row included
    /// (Spotify's `removeFromQueue`).
    pub fn removing(&self, rows: &[QueueTrack]) -> Result<Vec<QueueTrack>, ConnectError> {
        let kept: Vec<QueueTrack> = self
            .next
            .iter()
            .filter(|track| !rows.iter().any(|row| row.same_row(track)))
            .cloned()
            .collect();
        if kept.len() == self.next.len() {
            return Err(ConnectError::NotInQueue);
        }
        Ok(kept)
    }

    /// `next` without the rows queued by hand; the context's rows stay
    /// (Spotify's `clearQueue`).
    pub fn clearing(&self) -> Vec<QueueTrack> {
        self.next
            .iter()
            .filter(|track| !track.is_queued())
            .cloned()
            .collect()
    }

    /// `next` with the rows named moved before or after another row, in the
    /// order named (Spotify's `reorderQueue`). Rows moved next to a queued
    /// row become queued, as a context row dragged into *Next in queue*
    /// does. Moving a row next to itself changes nothing.
    pub fn moving(
        &self,
        rows: &[QueueTrack],
        place: &Place,
    ) -> Result<Vec<QueueTrack>, ConnectError> {
        let anchor = self.anchor(place)?;
        let mut picked: Vec<usize> = Vec::new();
        for row in rows {
            let found = self.next.iter().position(|track| row.same_row(track));
            if let Some(index) = found.filter(|index| !picked.contains(index)) {
                picked.push(index);
            }
        }
        if picked.is_empty() {
            return Err(ConnectError::NotInQueue);
        }
        if picked.contains(&anchor) {
            return Ok(self.next.clone());
        }
        let queued = self.next[anchor].is_queued();
        let mut moved: Vec<QueueTrack> = picked
            .iter()
            .map(|&index| {
                let track = self.next[index].clone();
                if queued { track.into_queued() } else { track }
            })
            .collect();
        let mut next = Vec::with_capacity(self.next.len());
        for (index, track) in self.next.iter().enumerate() {
            if picked.contains(&index) {
                continue;
            }
            let here = index == anchor;
            if here && !place.is_after() {
                next.append(&mut moved);
            }
            next.push(track.clone());
            if here && place.is_after() {
                next.append(&mut moved);
            }
        }
        Ok(next)
    }

    /// `next` with new songs before or after a row (Spotify's
    /// `insertIntoQueue`): queued when that row is, else part of the context.
    /// The player gives them their `uid`s.
    pub fn inserting(
        &self,
        uris: &[String],
        place: &Place,
    ) -> Result<Vec<QueueTrack>, ConnectError> {
        let anchor = self.anchor(place)?;
        let queued = self.next[anchor].is_queued();
        let at = anchor + usize::from(place.is_after());
        let mut next = Vec::with_capacity(self.next.len() + uris.len());
        next.extend_from_slice(&self.next[..at]);
        next.extend(uris.iter().map(|uri| QueueTrack {
            uri: uri.clone(),
            provider: String::from(if queued { "queue" } else { "context" }),
            metadata: BTreeMap::from([("is_queued".to_string(), queued.to_string())]),
            ..QueueTrack::default()
        }));
        next.extend_from_slice(&self.next[at..]);
        Ok(next)
    }

    fn anchor(&self, place: &Place) -> Result<usize, ConnectError> {
        self.next
            .iter()
            .position(|track| place.row().same_row(track))
            .ok_or(ConnectError::NotInQueue)
    }
}

/// Where a `play` starts in its context. Empty, the player chooses: the
/// first row, or a random one when the start is shuffled.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkipTo {
    pub track_uid: Option<String>,
    pub track_uri: Option<String>,
    pub track_index: Option<u32>,
    pub page_index: Option<u32>,
}

impl SkipTo {
    /// The song with this URI, its first row when it appears twice.
    pub fn uri(uri: impl Into<String>) -> Self {
        Self {
            track_uri: Some(uri.into()),
            ..Self::default()
        }
    }

    /// The row with this `uid`.
    pub fn uid(uid: impl Into<String>) -> Self {
        Self {
            track_uid: Some(uid.into()),
            ..Self::default()
        }
    }

    /// The row at this position in the context.
    pub fn index(index: u32) -> Self {
        Self {
            track_index: Some(index),
            ..Self::default()
        }
    }

    fn json(&self) -> Option<Value> {
        let mut skip = Map::new();
        if let Some(uid) = &self.track_uid {
            skip.insert("track_uid".into(), json!(uid));
        }
        if let Some(index) = self.track_index {
            skip.insert("track_index".into(), json!(index));
        }
        if let Some(uri) = &self.track_uri {
            skip.insert("track_uri".into(), json!(uri));
        }
        if let Some(page) = self.page_index {
            skip.insert("page_index".into(), json!(page));
        }
        if skip.is_empty() {
            None
        } else {
            Some(Value::Object(skip))
        }
    }
}

/// A context's shuffle mode, as Spotify's apps set it: off, shuffle, or
/// Smart Shuffle, which mixes recommendations in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShuffleMode {
    Off,
    On,
    Smart,
}

impl ShuffleMode {
    fn shuffles(self) -> bool {
        !matches!(self, Self::Off)
    }

    fn context_enhancement(self) -> &'static str {
        match self {
            Self::Smart => "RECOMMENDATION",
            Self::Off | Self::On => "NONE",
        }
    }
}

/// A context's repeat mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Repeat {
    Off,
    Context,
    Track,
}

/// A Connect `play` of a whole context, with the options the start
/// carries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContextPlay {
    /// The playlist, album, artist, show or collection to play. A lone song
    /// is no context: it goes through Soloist's own `play`.
    pub context_uri: String,
    pub skip_to: SkipTo,
    /// The context's shuffle mode for this start; `None` leaves the
    /// player's.
    pub shuffle: Option<ShuffleMode>,
    /// The repeat mode for this start; `None` leaves the player's.
    pub repeat: Option<Repeat>,
    pub seek_to_ms: Option<u32>,
    /// Start paused.
    pub paused: Option<bool>,
    /// The context's metadata, such as `sorting.criteria` or
    /// `filtering.predicate` for a sorted or filtered view.
    pub context_metadata: BTreeMap<String, String>,
}

/// Skips straight to a row of the queue: one `skip_next` naming it, so
/// nothing in between plays (finding 4).
pub async fn skip_to_row(
    session: &Session,
    to: &str,
    row: &QueueTrack,
) -> Result<(), ConnectError> {
    if row.uri.is_empty() {
        return logged("skip_next", to, Err(ConnectError::NotInQueue));
    }
    send(
        session,
        to,
        "skip_next",
        &skip_next_command(row, &command_id()),
    )
    .await
}

/// Starts a context with its options in one command (finding 3).
pub async fn play(session: &Session, to: &str, start: &ContextPlay) -> Result<(), ConnectError> {
    if start.context_uri.is_empty() {
        return logged("play", to, Err(ConnectError::NoPlayer));
    }
    send(session, to, "play", &play_command(start, &command_id())).await
}

/// Reads the queue Connect holds for the active player, with the revision
/// a `set_queue` must carry: the read Spotify's web player makes
/// (`GET /connect-state/v1/cluster`).
pub async fn fetch_queue(session: &Session) -> Result<ConnectQueue, ConnectError> {
    let result = session
        .spclient()
        .request_as_json(
            &reqwest::Method::GET,
            "/connect-state/v1/cluster",
            None,
            None,
        )
        .await
        .map_err(ConnectError::from)
        .and_then(|body| ConnectQueue::from_cluster_json(&body));
    logged("cluster read", "the active device", result)
}

/// Rewrites the player's next tracks with `next`, built from `queue`, in one
/// `set_queue` against the revision `queue` was read at (finding 6). Sends
/// nothing when `next` already is `queue.next`.
pub async fn set_queue(
    session: &Session,
    to: &str,
    queue: &ConnectQueue,
    next: &[QueueTrack],
) -> Result<(), ConnectError> {
    if next == queue.next.as_slice() {
        return Ok(());
    }
    let body = logged(
        "set_queue",
        to,
        set_queue_command(queue, next, &command_id()),
    )?;
    send(session, to, "set_queue", &body).await
}

/// Removes rows from the queue, context rows included.
pub async fn remove(
    session: &Session,
    to: &str,
    queue: &ConnectQueue,
    rows: &[QueueTrack],
) -> Result<(), ConnectError> {
    let next = logged("set_queue", to, queue.removing(rows))?;
    set_queue(session, to, queue, &next).await
}

/// Moves rows before or after another row.
pub async fn reorder(
    session: &Session,
    to: &str,
    queue: &ConnectQueue,
    rows: &[QueueTrack],
    place: &Place,
) -> Result<(), ConnectError> {
    let next = logged("set_queue", to, queue.moving(rows, place))?;
    set_queue(session, to, queue, &next).await
}

/// Inserts songs before or after a row.
pub async fn insert(
    session: &Session,
    to: &str,
    queue: &ConnectQueue,
    uris: &[String],
    place: &Place,
) -> Result<(), ConnectError> {
    let next = logged("set_queue", to, queue.inserting(uris, place))?;
    set_queue(session, to, queue, &next).await
}

/// Removes the rows queued by hand, leaving the context's.
pub async fn clear(session: &Session, to: &str, queue: &ConnectQueue) -> Result<(), ConnectError> {
    set_queue(session, to, queue, &queue.clearing()).await
}

/// Posts a command and sorts a failure into a refusal or a network error.
async fn send(
    session: &Session,
    to: &str,
    endpoint: &str,
    body: &Value,
) -> Result<(), ConnectError> {
    let result = restricted_device::post_command(session, to, body)
        .await
        .map_err(ConnectError::from);
    logged(endpoint, to, result)
}

/// Logs a command that was not carried out, at debug: the caller falls
/// back, so it is no failure the listener sees.
fn logged<T>(what: &str, to: &str, result: Result<T, ConnectError>) -> Result<T, ConnectError> {
    result.inspect_err(|error| log::debug!("Connect {what} to {to} not carried out: {error}"))
}

/// A fresh `command_id`: 32 hex digits, as Spotify's apps make them.
fn command_id() -> String {
    format!("{:032x}", rand::random::<u128>())
}

/// `logging_params` as Spotify's web player builds them for a command of
/// its own (`_generateLoggingParams`).
fn logging_params(command_id: &str) -> Value {
    json!({ "page_instance_ids": [], "interaction_ids": [], "command_id": command_id })
}

/// `skipToNext({uri, uid})`: a `skip_next` whose `track` is the row.
fn skip_next_command(row: &QueueTrack, command_id: &str) -> Value {
    json!({ "command": {
        "endpoint": "skip_next",
        "track": row.json(),
        "logging_params": logging_params(command_id),
    }})
}

/// `setInternalPlayerQueue`: every next track, the previous ones unchanged,
/// and the revision they were read at. The queue manager sends it with a
/// bare `command_id`.
fn set_queue_command(
    queue: &ConnectQueue,
    next: &[QueueTrack],
    command_id: &str,
) -> Result<Value, ConnectError> {
    if !queue.set_queue_disallowed.is_empty() {
        return Err(ConnectError::QueueLocked(
            queue.set_queue_disallowed.clone(),
        ));
    }
    if queue.revision.is_empty() {
        return Err(ConnectError::NoPlayer);
    }
    Ok(json!({ "command": {
        "endpoint": "set_queue",
        "next_tracks": tracks_json(next),
        "prev_tracks": tracks_json(&queue.prev),
        "queue_revision": queue.revision,
        "logging_params": { "command_id": command_id },
    }}))
}

fn tracks_json(rows: &[QueueTrack]) -> Vec<Value> {
    rows.iter().map(QueueTrack::json).collect()
}

/// `playURI` through `_play`: the context, who started it, and the options
/// `_generatePlayOptions` writes, `player_options_override` always present.
fn play_command(play: &ContextPlay, command_id: &str) -> Value {
    let mut context = restricted_device::uri_context(&play.context_uri);
    context["metadata"] = json!(play.context_metadata);
    let mut overrides = Map::new();
    if let Some(shuffle) = play.shuffle {
        overrides.insert("shuffling_context".into(), json!(shuffle.shuffles()));
        overrides.insert(
            "modes".into(),
            json!({ "context_enhancement": shuffle.context_enhancement() }),
        );
    }
    if let Some(repeat) = play.repeat {
        overrides.insert("repeating_context".into(), json!(repeat == Repeat::Context));
        overrides.insert("repeating_track".into(), json!(repeat == Repeat::Track));
    }
    let mut options = Map::new();
    options.insert("license".into(), json!(LICENSE));
    if let Some(skip_to) = play.skip_to.json() {
        options.insert("skip_to".into(), skip_to);
    }
    if let Some(position) = play.seek_to_ms {
        options.insert("seek_to".into(), json!(position));
    }
    if let Some(paused) = play.paused {
        options.insert("initially_paused".into(), json!(paused));
    }
    options.insert("player_options_override".into(), Value::Object(overrides));
    json!({ "command": {
        "endpoint": "play",
        "context": context,
        "play_origin": {
            "feature_identifier": FEATURE,
            "feature_version": env!("CARGO_PKG_VERSION"),
        },
        "options": Value::Object(options),
        "logging_params": logging_params(command_id),
    }})
}

#[cfg(test)]
mod tests {
    use super::*;
    use librespot_core::dealer::protocol::Command;

    const ID: &str = "0123456789abcdef0123456789abcdef";

    /// What a Connect receiver makes of a body: librespot's parser for the
    /// commands Spotify's apps send.
    fn received(body: &Value) -> Command {
        serde_json::from_value(body["command"].clone()).expect("a Connect command")
    }

    fn queued(uri: &str, uid: &str) -> QueueTrack {
        let mut track = QueueTrack::row(uri, uid, "queue");
        track.metadata.insert("is_queued".into(), "true".into());
        track
    }

    fn queue() -> ConnectQueue {
        ConnectQueue {
            revision: "4617382194628735619".into(),
            device: "soloist".into(),
            current: Some(QueueTrack::row("spotify:track:now", "u0", "context")),
            next: vec![
                queued("spotify:track:q1", "q1"),
                queued("spotify:track:q2", "q2"),
                QueueTrack::row("spotify:track:c1", "c1", "context"),
                QueueTrack::row("spotify:track:c2", "c2", "context"),
                QueueTrack::row("spotify:track:c1", "c1-again", "context"),
            ],
            prev: vec![QueueTrack::row("spotify:track:p1", "p1", "context")],
            set_queue_disallowed: Vec::new(),
        }
    }

    fn uids(tracks: &[QueueTrack]) -> Vec<&str> {
        tracks.iter().map(|track| track.uid.as_str()).collect()
    }

    /// A clicked row is what Spotify's web player sends for
    /// `skipToNext({uri, uid})`: one `skip_next` whose `track` is the row.
    #[test]
    fn a_clicked_row_is_one_skip_that_names_it() {
        let row = queue()
            .find(&QueueTrack::row("spotify:track:c1", "c1-again", ""))
            .cloned();
        let row = row.expect("the second c1 row");
        let body = skip_next_command(&row, ID);
        assert_eq!(
            body,
            json!({ "command": {
                "endpoint": "skip_next",
                "track": {
                    "uri": "spotify:track:c1",
                    "uid": "c1-again",
                    "metadata": {},
                    "provider": "context",
                },
                "logging_params": { "page_instance_ids": [], "interaction_ids": [], "command_id": ID },
            }})
        );
        let Command::SkipNext(skip) = received(&body) else {
            panic!("not read as a skip_next");
        };
        let track = skip.track.expect("the row it names");
        assert_eq!(track.uri, "spotify:track:c1");
        assert_eq!(track.uid, "c1-again");
    }

    /// Rows go out as the web player's `z()` writes them.
    #[test]
    fn rows_keep_the_marks_spotify_writes() {
        let soloist_row = QueueTrack::row("spotify:track:q1", "q1", "queue");
        assert_eq!(
            soloist_row.json(),
            json!({
                "uri": "spotify:track:q1",
                "uid": "q1",
                "metadata": { "is_queued": "true" },
                "provider": "queue",
            })
        );
        let delimiter = QueueTrack::row("spotify:delimiter", "", "context");
        assert_eq!(
            delimiter.json(),
            json!({ "uri": "spotify:delimiter", "metadata": { "hidden": "true" }, "provider": "context" })
        );
        let mut removed = QueueTrack::row("spotify:track:x", "x", "context");
        removed.removed = vec!["context/removed".into()];
        assert_eq!(removed.json()["removed"], json!(["context/removed"]));
        assert_eq!(
            queue()
                .find(&QueueTrack::row("spotify:track:c1", "", ""))
                .unwrap()
                .uid,
            "c1"
        );
    }

    /// Removing a row sends every next track but that one, the previous
    /// tracks unchanged, and the revision they were read at.
    #[test]
    fn removing_a_row_rewrites_the_next_tracks_at_their_revision() {
        let queue = queue();
        let next = queue
            .removing(&[QueueTrack::row("spotify:track:c1", "c1", "")])
            .unwrap();
        assert_eq!(uids(&next), ["q1", "q2", "c2", "c1-again"]);
        let body = set_queue_command(&queue, &next, ID).unwrap();
        assert_eq!(
            body,
            json!({ "command": {
                "endpoint": "set_queue",
                "next_tracks": [
                    { "uri": "spotify:track:q1", "uid": "q1", "metadata": { "is_queued": "true" }, "provider": "queue" },
                    { "uri": "spotify:track:q2", "uid": "q2", "metadata": { "is_queued": "true" }, "provider": "queue" },
                    { "uri": "spotify:track:c2", "uid": "c2", "metadata": {}, "provider": "context" },
                    { "uri": "spotify:track:c1", "uid": "c1-again", "metadata": {}, "provider": "context" },
                ],
                "prev_tracks": [
                    { "uri": "spotify:track:p1", "uid": "p1", "metadata": {}, "provider": "context" },
                ],
                "queue_revision": "4617382194628735619",
                "logging_params": { "command_id": ID },
            }})
        );
        let Command::SetQueue(set) = received(&body) else {
            panic!("not read as a set_queue");
        };
        assert_eq!(set.next_tracks.len(), 4);
        assert_eq!(set.next_tracks[2].uid, "c2");
        assert_eq!(set.prev_tracks.len(), 1);
        assert_eq!(set.queue_revision, "4617382194628735619");
    }

    /// Without a `uid`, a row is its URI, as in Spotify's `isSameItem`.
    #[test]
    fn a_row_without_uid_matches_every_row_of_that_song() {
        let next = queue()
            .removing(&[QueueTrack::row("spotify:track:c1", "", "")])
            .unwrap();
        assert_eq!(uids(&next), ["q1", "q2", "c2"]);
    }

    #[test]
    fn clearing_keeps_the_contexts_rows() {
        assert_eq!(uids(&queue().clearing()), ["c1", "c2", "c1-again"]);
    }

    /// Spotify's `reorderQueue`: rows land next to the anchor in the order
    /// named, and become queued among queued rows.
    #[test]
    fn moving_rows_follows_spotifys_reorder() {
        let queue = queue();
        let c2 = QueueTrack::row("spotify:track:c2", "c2", "");
        let q1 = QueueTrack::row("spotify:track:q1", "q1", "");

        let next = queue
            .moving(std::slice::from_ref(&c2), &Place::Before(q1.clone()))
            .unwrap();
        assert_eq!(uids(&next), ["c2", "q1", "q2", "c1", "c1-again"]);
        assert!(next[0].is_queued());
        assert_eq!(next[0].json()["provider"], "queue");

        let next = queue
            .moving(std::slice::from_ref(&q1), &Place::After(c2.clone()))
            .unwrap();
        assert_eq!(uids(&next), ["q2", "c1", "c2", "q1", "c1-again"]);
        assert!(next[3].is_queued());
        assert!(!next[2].is_queued());

        let both = [c2.clone(), QueueTrack::row("spotify:track:c1", "c1", "")];
        let next = queue.moving(&both, &Place::After(q1.clone())).unwrap();
        assert_eq!(uids(&next), ["q1", "c2", "c1", "q2", "c1-again"]);

        assert_eq!(
            queue.moving(std::slice::from_ref(&c2), &Place::After(c2.clone())),
            Ok(queue.next.clone())
        );
        let gone = QueueTrack::row("spotify:track:gone", "gone", "");
        assert_eq!(
            queue.moving(std::slice::from_ref(&gone), &Place::After(q1)),
            Err(ConnectError::NotInQueue)
        );
        assert_eq!(
            queue.moving(&[c2], &Place::After(gone)),
            Err(ConnectError::NotInQueue)
        );
    }

    /// Spotify's `insertIntoQueue` writes new rows as `createQueueItem`
    /// does: no `uid`, queued only next to a queued row.
    #[test]
    fn inserted_songs_take_the_section_of_their_neighbour() {
        let queue = queue();
        let new = vec!["spotify:track:n".to_string()];
        let next = queue
            .inserting(
                &new,
                &Place::After(QueueTrack::row("spotify:track:q2", "q2", "")),
            )
            .unwrap();
        assert_eq!(uids(&next), ["q1", "q2", "", "c1", "c2", "c1-again"]);
        assert_eq!(
            next[2].json(),
            json!({ "uri": "spotify:track:n", "metadata": { "is_queued": "true" }, "provider": "queue" })
        );
        let next = queue
            .inserting(
                &new,
                &Place::Before(QueueTrack::row("spotify:track:c1", "c1", "")),
            )
            .unwrap();
        assert_eq!(next[2].uri, "spotify:track:n");
        assert_eq!(
            next[2].json(),
            json!({ "uri": "spotify:track:n", "metadata": { "is_queued": "false" }, "provider": "context" })
        );
    }

    /// A start carries its shuffle mode and repeat in the one `play`, as
    /// `_generatePlayOptions` writes them.
    #[test]
    fn a_shuffled_start_carries_its_mode_in_the_play() {
        let play = ContextPlay {
            context_uri: "spotify:playlist:p".into(),
            skip_to: SkipTo {
                track_uid: Some("u7".into()),
                track_uri: Some("spotify:track:t7".into()),
                ..SkipTo::default()
            },
            shuffle: Some(ShuffleMode::On),
            repeat: Some(Repeat::Context),
            seek_to_ms: Some(1_500),
            paused: None,
            context_metadata: BTreeMap::from([(
                "sorting.criteria".to_string(),
                "title".to_string(),
            )]),
        };
        let body = play_command(&play, ID);
        assert_eq!(
            body,
            json!({ "command": {
                "endpoint": "play",
                "context": {
                    "uri": "spotify:playlist:p",
                    "url": "context://spotify:playlist:p",
                    "metadata": { "sorting.criteria": "title" },
                },
                "play_origin": {
                    "feature_identifier": "spotifast",
                    "feature_version": env!("CARGO_PKG_VERSION"),
                },
                "options": {
                    "license": "on-demand",
                    "skip_to": { "track_uid": "u7", "track_uri": "spotify:track:t7" },
                    "seek_to": 1_500,
                    "player_options_override": {
                        "shuffling_context": true,
                        "modes": { "context_enhancement": "NONE" },
                        "repeating_context": true,
                        "repeating_track": false,
                    },
                },
                "logging_params": { "page_instance_ids": [], "interaction_ids": [], "command_id": ID },
            }})
        );
        let Command::Play(command) = received(&body) else {
            panic!("not read as a play");
        };
        assert_eq!(command.context.uri.as_deref(), Some("spotify:playlist:p"));
        assert_eq!(
            command
                .context
                .metadata
                .get("sorting.criteria")
                .map(String::as_str),
            Some("title")
        );
        let skip_to = command.options.skip_to.clone().expect("a start row");
        assert_eq!(skip_to.track_uid.as_deref(), Some("u7"));
        assert_eq!(command.options.seek_to, Some(1_500));
        let overrides = command
            .options
            .player_options_override
            .clone()
            .expect("options");
        assert_eq!(overrides.shuffling_context, Some(true));
        assert_eq!(overrides.repeating_context, Some(true));
        assert_eq!(overrides.repeating_track, Some(false));
        assert_eq!(
            overrides
                .modes
                .get("context_enhancement")
                .map(String::as_str),
            Some("NONE")
        );
    }

    /// Smart Shuffle is shuffle with recommendations; with no start row the
    /// player chooses. A start without options still sends an empty
    /// override, as Spotify's web player does.
    #[test]
    fn a_start_names_only_the_options_it_sets() {
        let smart = ContextPlay {
            context_uri: "spotify:collection:tracks".into(),
            shuffle: Some(ShuffleMode::Smart),
            ..ContextPlay::default()
        };
        let body = play_command(&smart, ID);
        let options = &body["command"]["options"];
        assert!(options.get("skip_to").is_none());
        assert_eq!(
            options["player_options_override"],
            json!({ "shuffling_context": true, "modes": { "context_enhancement": "RECOMMENDATION" } })
        );

        let off = ContextPlay {
            context_uri: "spotify:album:a".into(),
            skip_to: SkipTo::index(3),
            shuffle: Some(ShuffleMode::Off),
            paused: Some(true),
            ..ContextPlay::default()
        };
        let body = play_command(&off, ID);
        let options = &body["command"]["options"];
        assert_eq!(options["skip_to"], json!({ "track_index": 3 }));
        assert_eq!(options["initially_paused"], json!(true));
        assert_eq!(
            options["player_options_override"],
            json!({ "shuffling_context": false, "modes": { "context_enhancement": "NONE" } })
        );

        let plain = ContextPlay {
            context_uri: "spotify:artist:r".into(),
            skip_to: SkipTo::uri("spotify:track:hit"),
            ..ContextPlay::default()
        };
        let body = play_command(&plain, ID);
        assert_eq!(
            body["command"]["options"]["player_options_override"],
            json!({})
        );
        assert_eq!(
            body["command"]["options"]["skip_to"],
            json!({ "track_uri": "spotify:track:hit" })
        );
        assert!(matches!(received(&body), Command::Play(_)));
        assert_eq!(SkipTo::uid("u").json(), Some(json!({ "track_uid": "u" })));
    }

    /// The same queue comes out of a cluster pushed to the session and of
    /// the cluster JSON Connect's state service returns.
    #[test]
    fn the_queue_reads_from_a_cluster_push_and_from_its_json() {
        let mut cluster = Cluster::new();
        cluster.active_device_id = "soloist".into();
        let state = cluster.player_state.mut_or_insert_default();
        state.queue_revision = "42".into();
        let mut track = ProvidedTrack::new();
        track.uri = "spotify:track:q1".into();
        track.uid = "q1".into();
        track.provider = "queue".into();
        track.metadata.insert("is_queued".into(), "true".into());
        state.next_tracks.push(track);
        state
            .restrictions
            .mut_or_insert_default()
            .disallow_set_queue_reasons
            .push("not_supported".into());

        let pushed = ConnectQueue::from_cluster(&cluster).expect("a player state");
        assert_eq!(
            pushed,
            ConnectQueue {
                revision: "42".into(),
                device: "soloist".into(),
                current: None,
                next: vec![queued("spotify:track:q1", "q1")],
                prev: Vec::new(),
                set_queue_disallowed: vec!["not_supported".into()],
            }
        );
        let json = br#"{
            "active_device_id": "soloist",
            "player_state": {
                "queue_revision": "42",
                "next_tracks": [
                    { "uri": "spotify:track:q1", "uid": "q1", "provider": "queue",
                      "metadata": { "is_queued": "true" }, "album_uri": "spotify:album:a" }
                ],
                "restrictions": { "disallow_set_queue_reasons": ["not_supported"] }
            }
        }"#;
        assert_eq!(ConnectQueue::from_cluster_json(json), Ok(pushed));
        assert_eq!(
            ConnectQueue::from_cluster_json(b"{}"),
            Err(ConnectError::NoPlayer)
        );
        assert_eq!(ConnectQueue::from_cluster(&Cluster::new()), None);
    }

    /// A queue the player locks, or one with no revision, is refused before
    /// anything is sent, and so is a row no longer in it.
    #[test]
    fn an_edit_that_cannot_apply_is_refused_before_sending() {
        let mut locked = queue();
        locked.set_queue_disallowed = vec!["restricted".into()];
        assert_eq!(
            set_queue_command(&locked, &[], ID),
            Err(ConnectError::QueueLocked(vec!["restricted".into()]))
        );
        let mut unknown = queue();
        unknown.revision.clear();
        assert_eq!(
            set_queue_command(&unknown, &[], ID),
            Err(ConnectError::NoPlayer)
        );
        let gone = QueueTrack::row("spotify:track:gone", "gone", "");
        assert_eq!(queue().removing(&[gone]), Err(ConnectError::NotInQueue));
    }

    /// Spotify's answer says whether to give up on this route for the
    /// device or to try again; either way the caller falls back now.
    #[test]
    fn a_refusal_is_told_from_a_lost_network() {
        assert_eq!(
            ConnectError::of_kind(ErrorKind::NotFound, "gone".into()),
            ConnectError::NoDevice("gone".into())
        );
        for kind in [
            ErrorKind::PermissionDenied,
            ErrorKind::Unauthenticated,
            ErrorKind::InvalidArgument,
            ErrorKind::FailedPrecondition,
            ErrorKind::Aborted,
            ErrorKind::Unimplemented,
        ] {
            let error = ConnectError::of_kind(kind, "no".into());
            assert!(matches!(error, ConnectError::Refused(_)), "{kind:?}");
            assert!(!error.is_network(), "{kind:?}");
        }
        for kind in [
            ErrorKind::Unavailable,
            ErrorKind::DeadlineExceeded,
            ErrorKind::ResourceExhausted,
            ErrorKind::Internal,
        ] {
            assert!(
                ConnectError::of_kind(kind, "later".into()).is_network(),
                "{kind:?}"
            );
        }
        let restricted = librespot_core::Error::permission_denied("Restriction violated");
        assert!(matches!(
            ConnectError::from(restricted),
            ConnectError::Refused(_)
        ));
        let reported: crate::api::client::ApiError = ConnectError::NotInQueue.into();
        assert!(reported.is_network());
    }

    #[test]
    fn each_command_gets_its_own_hex_id() {
        let (one, two) = (command_id(), command_id());
        assert_ne!(one, two);
        assert_eq!(one.len(), 32);
        assert!(one.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }
}
