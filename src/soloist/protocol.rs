//! Only commands documented by Spotify's Soloist WebSocket API. Context
//! loads retain their offsets through Spotifast's existing Spotify API path;
//! a lone song starts as its own context through Soloist's `play`, as
//! Spotify's apps start one, so autoplay follows it from that song, with
//! Shuffle on or off. Songs join the queue through Soloist's own
//! `add_to_queue`, and the queue keeps each row's `source` and `uid`.
use crate::{
    api::{client::ApiError, models::*},
    backend::{ApiRequest, ApiResponse, RemoteAction},
};
use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, RwLock,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{Message, protocol::WebSocketConfig},
};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
pub async fn connect(port: u16) -> Result<Socket> {
    let config = WebSocketConfig::default()
        .max_message_size(Some(4 * 1024 * 1024))
        .max_frame_size(Some(4 * 1024 * 1024));
    let (socket, _) = tokio_tungstenite::connect_async_with_config(
        format!("ws://127.0.0.1:{port}"),
        Some(config),
        false,
    )
    .await?;
    Ok(socket)
}
pub async fn send(socket: &mut Socket, mut body: Value) -> Result<()> {
    body["type"] = json!("command");
    socket.send(Message::Text(body.to_string().into())).await?;
    Ok(())
}
pub async fn send_reconnecting(socket: &mut Socket, port: u16, body: Value) -> Result<bool> {
    if send(socket, body.clone()).await.is_ok() {
        return Ok(false);
    }
    *socket = tokio::time::timeout(Duration::from_secs(5), connect(port))
        .await
        .context("Soloist's local API did not reconnect in time")??;
    send(socket, body).await?;
    Ok(true)
}
pub async fn receive(socket: &mut Socket) -> Result<Value> {
    loop {
        match socket.next().await.context("Soloist disconnected")?? {
            Message::Text(text) => return Ok(serde_json::from_str(&text)?),
            Message::Close(_) => bail!("Soloist disconnected"),
            Message::Ping(_) => socket.flush().await?,
            _ => {}
        }
    }
}

pub async fn receive_reconnecting(socket: &mut Socket, port: u16) -> Result<(Option<Value>, bool)> {
    match receive(socket).await {
        Ok(message) => Ok((Some(message), false)),
        Err(_) => {
            *socket = tokio::time::timeout(Duration::from_secs(5), connect(port))
                .await
                .context("Soloist's local API did not reconnect in time")??;
            Ok((None, true))
        }
    }
}

#[derive(Default)]
struct Endpoint {
    port: Option<u16>,
    device: Option<Device>,
    active: bool,
}
#[derive(Clone, Default)]
pub struct Client {
    endpoint: Arc<RwLock<Endpoint>>,
    /// Soloist acknowledgements have command names, not request IDs.
    serial: Arc<tokio::sync::Mutex<()>>,
    /// How far behind Soloist's position the sound is, in our audio route.
    audio_delay_ms: Arc<AtomicU32>,
}
impl Client {
    pub fn bind(&self, device: Option<Device>) {
        self.endpoint
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .device = device;
    }
    pub fn listening(&self, port: Option<u16>) {
        let mut state = self.endpoint.write().unwrap_or_else(|e| e.into_inner());
        state.port = port;
        state.active = false;
    }
    pub fn active(&self, active: bool) {
        self.endpoint
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .active = active;
    }
    /// Lyrics and the clock follow what is heard, not what Soloist has sent.
    pub fn audio_delay(&self, ms: u32) {
        self.audio_delay_ms.store(ms, Ordering::Relaxed);
    }
    pub fn clear(&self) {
        *self.endpoint.write().unwrap_or_else(|e| e.into_inner()) = Endpoint::default();
    }

    pub async fn dispatch(&self, request: &ApiRequest) -> Option<ApiResponse> {
        let (port, device, active) = {
            let state = self.endpoint.read().unwrap_or_else(|e| e.into_inner());
            (state.port?, state.device.clone()?, state.active)
        };
        match request {
            ApiRequest::PlaybackState { seq } if active => {
                let state = self
                    .rpc(port, json!({"command":"get_state"}), Some("playback_state"))
                    .await;
                if state
                    .as_ref()
                    .is_ok_and(|state| state["is_active"] == false)
                {
                    self.active(false);
                    return None;
                }
                let result = state
                    .and_then(|state| {
                        playback(&state, device, self.audio_delay_ms.load(Ordering::Relaxed))
                    })
                    .map(Some)
                    .map_err(failure);
                Some(ApiResponse::PlaybackState { seq: *seq, result })
            }
            ApiRequest::Queue { seq } if active => {
                let state = self
                    .rpc(port, json!({"command":"get_state"}), Some("playback_state"))
                    .await;
                if state
                    .as_ref()
                    .is_ok_and(|state| state["is_active"] == false)
                {
                    self.active(false);
                    return None;
                }
                let result = async {
                    let state = state?;
                    let queue = self
                        .rpc(
                            port,
                            json!({"command":"get_queue", "limit":0}),
                            Some("queue_changed"),
                        )
                        .await?;
                    read_queue(&state, &queue)
                }
                .await
                .map_err(failure);
                Some(ApiResponse::Queue { seq: *seq, result })
            }
            // Shuffling one song means nothing: a lone song starts as its
            // own context whatever the Shuffle setting, so autoplay follows
            // it as with Shuffle off (#1320). Anything else keeps the Web
            // API's shuffled start.
            ApiRequest::ShufflePlay { device_id, play }
                if active && device_id.is_some() && device_id == &device.id =>
            {
                let uri = lone_song(play)?.to_string();
                let result = self
                    .rpc(port, json!({"command":"play", "uri":uri}), None)
                    .await
                    .map(|_| ())
                    .map_err(failure);
                Some(ApiResponse::Remote {
                    action: RemoteAction::Play,
                    result,
                })
            }
            // Soloist queues songs itself, spending no Web API quota. One
            // lock over a whole batch keeps an album's songs together and
            // in order, as the Web API's queue writer does.
            ApiRequest::AddManyToQueue {
                request: batch,
                uris,
                device_id,
            } if active
                && device_id.is_some()
                && device_id == &device.id
                && uris.iter().all(|uri| soloist_queues(uri)) =>
            {
                let _guard = self.serial.lock().await;
                let mut added = 0;
                let mut result = Ok(());
                for uri in uris {
                    let command = json!({"command":"add_to_queue", "uri":uri});
                    if let Err(error) = self.rpc_unlocked(port, command, None).await {
                        result = Err(failure(error));
                        break;
                    }
                    added += 1;
                }
                Some(ApiResponse::QueueBatchAdded {
                    request: *batch,
                    added,
                    result,
                })
            }
            ApiRequest::AddToQueue {
                uri,
                device_id,
                label,
            } if active
                && device_id.is_some()
                && device_id == &device.id
                && soloist_queues(uri) =>
            {
                let result = self
                    .rpc(port, json!({"command":"add_to_queue", "uri":uri}), None)
                    .await
                    .map(|_| ())
                    .map_err(failure);
                Some(ApiResponse::QueueAdded {
                    label: label.clone(),
                    result,
                })
            }
            ApiRequest::Remote {
                action, device_id, ..
            } if device_id.is_some() && device_id == &device.id => {
                let commands = commands(request, active)?;
                let result = async {
                    let _guard = self.serial.lock().await;
                    for command in commands {
                        self.rpc_unlocked(port, command, None).await?;
                    }
                    Ok(())
                }
                .await
                .map_err(failure);
                Some(ApiResponse::Remote {
                    action: *action,
                    result,
                })
            }
            // Episodes, which Soloist's `add_to_queue` refuses, and a
            // Soloist that is not the active player keep the Web API.
            _ => None,
        }
    }
    async fn rpc(&self, port: u16, body: Value, response_type: Option<&str>) -> Result<Value> {
        let _guard = self.serial.lock().await;
        self.rpc_unlocked(port, body, response_type).await
    }
    async fn rpc_unlocked(
        &self,
        port: u16,
        body: Value,
        response_type: Option<&str>,
    ) -> Result<Value> {
        if self.endpoint.read().unwrap_or_else(|e| e.into_inner()).port != Some(port) {
            bail!("Soloist restarted before this command could be sent");
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut socket = connect(port).await?;
            send(&mut socket, body.clone()).await?;
            loop {
                let response = receive(&mut socket).await?;
                let kind = response["type"].as_str().unwrap_or_default();
                if kind == "error" {
                    bail!("Soloist refused this command. Check its connection and try again.");
                }
                if response_type == Some(kind)
                    || (response_type.is_none()
                        && kind == "command_result"
                        && response["command"] == body["command"])
                {
                    return Ok(response);
                }
            }
        })
        .await
        .context("Soloist did not answer in time")?
    }
}
fn failure(error: anyhow::Error) -> ApiError {
    ApiError::Network(error.to_string())
}

/// The song a play request starts on its own, with no list or context
/// around it and from its beginning. Spotify's apps play such a song with
/// the song itself as the context (`play({uri: track})`), and autoplay
/// then carries on from it; the Web API refuses a track as `context_uri`
/// and can only send it as a one-song `uris` list, which Spotify's player
/// does not treat as that song's context.
fn lone_song(play: &crate::api::PlayRequest) -> Option<&str> {
    let uri = match (play.context_uri.as_deref(), play.uris.as_slice()) {
        (None, [uri])
            if play
                .offset_uri
                .as_deref()
                .is_none_or(|offset| offset == uri.as_str()) =>
        {
            uri.as_str()
        }
        // A resume can name a later song than its context: that one plays.
        (Some(context), _) if crate::util::uri_kind(context) == Some("track") => {
            play.offset_uri.as_deref().unwrap_or(context)
        }
        _ => return None,
    };
    let from_start = play.position_ms == 0 && play.offset_position.unwrap_or(0) == 0;
    (from_start && crate::util::uri_kind(uri) == Some("track")).then_some(uri)
}

/// Soloist's `add_to_queue` takes a Spotify track URI only.
fn soloist_queues(uri: &str) -> bool {
    crate::util::uri_kind(uri) == Some("track")
}

fn commands(request: &ApiRequest, active: bool) -> Option<Vec<Value>> {
    let ApiRequest::Remote {
        action,
        play,
        position_ms,
        percent,
        flag,
        repeat,
        ..
    } = request
    else {
        return None;
    };
    let command = match action {
        RemoteAction::Play if play.is_none() => json!({"command":"play"}),
        // An active Soloist starts a lone song as its own context. Taking
        // the device over stays with the Web API, which documents it.
        RemoteAction::Play => match play.as_ref().and_then(lone_song) {
            Some(uri) if active => json!({"command":"play", "uri":uri}),
            // Preserve context, ordering and selected occurrence.
            _ => return None,
        },
        RemoteAction::Pause => json!({"command":"pause"}),
        RemoteAction::Next => json!({"command":"skip_next"}),
        RemoteAction::Previous => json!({"command":"skip_prev"}),
        RemoteAction::Seek => json!({"command":"seek", "position_ms":position_ms}),
        RemoteAction::Volume { .. } => json!({"command":"set_volume", "volume":percent.min(&100)}),
        RemoteAction::Shuffle => json!({"command":"set_shuffle", "enabled":flag}),
        RemoteAction::Repeat if repeat == "track" => {
            return Some(vec![
                json!({"command":"set_repeat_context", "enabled":false}),
                json!({"command":"set_repeat_track", "enabled":true}),
            ]);
        }
        RemoteAction::Repeat => {
            return Some(vec![
                json!({"command":"set_repeat_track", "enabled":repeat == "track"}),
                json!({"command":"set_repeat_context", "enabled":repeat == "context"}),
            ]);
        }
    };
    Some(vec![command])
}
fn name(entity: &Value) -> String {
    entity["decorations"]["identity"]["name"]
        .as_str()
        .unwrap_or_default()
        .into()
}
fn uri(entity: &Value) -> String {
    entity["uri"].as_str().unwrap_or_default().into()
}
fn id(entity: &Value) -> Option<String> {
    uri(entity)
        .rsplit(':')
        .next()
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}
fn images(entity: &Value) -> Vec<Image> {
    entity["decorations"]["visual_identity"]["cover"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|image| {
            let url = image["url"].as_str()?;
            let width = match image["size"].as_str() {
                Some("small") => 64,
                Some("default") => 300,
                Some("large") => 640,
                _ => 1280,
            };
            Some(Image {
                url: url.into(),
                width: Some(width),
                height: Some(width),
            })
        })
        .collect()
}
fn item(entity: &Value) -> Option<PlayableItem> {
    let decorations = &entity["decorations"];
    let duration_ms = decorations["playback"]["duration_ms"]
        .as_u64()
        .and_then(|ms| u32::try_from(ms).ok())
        .unwrap_or(0);
    let explicit = decorations["playback"]["content_ratings"]
        .as_array()
        .is_some_and(|ratings| ratings.iter().any(|rating| rating == "explicit"));
    match entity["entity_type"].as_str()? {
        "track" => {
            let parent = &decorations["parent"]["entity"];
            let artists = decorations["creators"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|creator| {
                    let entity = &creator["entity"];
                    ArtistRef {
                        id: id(entity),
                        name: name(entity),
                        uri: Some(uri(entity)),
                    }
                })
                .collect();
            Some(PlayableItem::Track(Track {
                id: id(entity),
                uri: uri(entity),
                name: name(entity),
                duration_ms,
                explicit,
                artists,
                album: Some(Album {
                    id: id(parent).unwrap_or_default(),
                    uri: uri(parent),
                    name: name(parent),
                    images: images(entity),
                    ..Default::default()
                }),
                ..Default::default()
            }))
        }
        "episode" => Some(PlayableItem::Episode(Episode {
            id: id(entity).unwrap_or_default(),
            uri: uri(entity),
            name: name(entity),
            duration_ms,
            explicit,
            images: images(entity),
            ..Default::default()
        })),
        _ => None,
    }
}
/// A queue entry's item. Soloist decorates only the head of a long queue
/// and sends the rest as bare URIs, sometimes without an `entity_type`.
/// Those stay as rows without names, which Spotifast fills afterwards, so
/// no row is lost and every row keeps its place.
fn queue_item(entity: &Value) -> Option<PlayableItem> {
    if let Some(item) = item(entity) {
        return Some(item);
    }
    let uri = uri(entity);
    match crate::util::uri_kind(&uri)? {
        "track" => Some(PlayableItem::Track(Track {
            id: id(entity),
            uri,
            ..Default::default()
        })),
        "episode" => Some(PlayableItem::Episode(Episode {
            id: id(entity).unwrap_or_default(),
            uri,
            ..Default::default()
        })),
        _ => None,
    }
}

/// One entry of `get_queue`: its item, and where it comes from as the
/// player says, with its stable id. Spotify's apps split the queue panel
/// by that `source`; an unknown one counts as the context's.
fn queue_entry(entry: &Value) -> Option<(PlayableItem, QueueEntry)> {
    let item = queue_item(&entry["item"])?;
    let source = entry["source"]
        .as_str()
        .and_then(QueueProvider::parse)
        .unwrap_or(QueueProvider::Context);
    let uid = entry["uid"]
        .as_str()
        .filter(|uid| !uid.is_empty())
        .unwrap_or_else(|| item.uri())
        .to_string();
    Some((item, QueueEntry { uid, source }))
}

/// The queue from Soloist's state and its `queue_changed` answer: the
/// upcoming rows with their sources and ids, and the player's history.
fn read_queue(state: &Value, queue: &Value) -> Result<Queue> {
    let (rows, entries): (Vec<_>, Vec<_>) = queue["upcoming"]
        .as_array()
        .context("Missing Soloist queue")?
        .iter()
        .filter_map(queue_entry)
        .unzip();
    let previous = queue["previous"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| queue_item(&entry["item"]))
        .collect();
    Ok(Queue {
        currently_playing: item(&state["item"]),
        queue: rows,
        entries: Some(entries),
        previous,
    })
}

fn playback(state: &Value, mut device: Device, delay_ms: u32) -> Result<PlaybackState> {
    let active = state["is_active"]
        .as_bool()
        .context("Missing Soloist activity state")?;
    device.is_active = active;
    device.volume_percent = state["volume"].as_u64().map(|v| v.min(100) as u8);
    let context = &state["context"];
    let position = &state["position"];
    let position_ms = position["position_ms"].as_u64().unwrap_or(0);
    let at = position["timestamp_ms"].as_u64().unwrap_or(0);
    let speed = position["speed"].as_f64().unwrap_or(0.0).clamp(0.0, 4.0);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64;
    let elapsed = if at == 0 {
        0
    } else {
        now_ms.saturating_sub(at)
    };
    let advanced = position_ms.saturating_add((elapsed as f64 * speed) as u64);
    let heard = if speed > 0.0 {
        advanced.saturating_sub(u64::from(delay_ms))
    } else {
        advanced
    };
    Ok(PlaybackState {
        device: Some(device),
        item: item(&state["item"]),
        is_playing: active && state["status"] == "playing",
        progress_ms: Some(heard.min(u32::MAX as u64) as u32),
        context: context["uri"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(|uri| crate::api::models::Context {
                uri: uri.into(),
                kind: context["entity_type"].as_str().unwrap_or_default().into(),
            }),
        repeat_state: state["options"]["repeat"].as_str().unwrap_or("off").into(),
        shuffle_state: state["options"]["shuffle"].as_bool().unwrap_or(false),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    /// A moving position is ahead of what is heard by our queue's delay.
    #[test]
    fn a_playing_position_is_held_back_by_the_route_s_delay() {
        let state =
            json!({"is_active":true,"status":"playing","position":{"position_ms":2300,"speed":1}});
        let position = |delay| {
            playback(&state, Device::default(), delay)
                .unwrap()
                .progress_ms
        };
        assert_eq!(position(0), Some(2300));
        assert_eq!(position(70), Some(2230));
        assert_eq!(position(9000), Some(0));
    }

    #[test]
    fn documented_state_keeps_context_and_metadata() {
        let state = json!({"is_active":true,"status":"playing","volume":52,"position":{"position_ms":2300,"speed":0},"context":{"uri":"spotify:playlist:p","entity_type":"playlist"},"options":{"shuffle":true,"repeat":"track"},"item":{"uri":"spotify:track:t","entity_type":"track","decorations":{"identity":{"name":"Song"},"playback":{"duration_ms":123000},"creators":[{"entity":{"uri":"spotify:artist:a","decorations":{"identity":{"name":"Artist"}}}}]}}});
        let got = playback(
            &state,
            Device {
                id: Some("own".into()),
                ..Default::default()
            },
            70,
        )
        .unwrap();
        assert_eq!(got.progress_ms, Some(2300), "paused: nothing is queued");
        assert_eq!(got.context.unwrap().uri, "spotify:playlist:p");
        assert_eq!(got.item.unwrap().name(), "Song");
        assert_eq!(got.repeat_state, "track");
        assert!(got.shuffle_state);
    }
    #[tokio::test]
    async fn unbound_or_inactive_player_does_not_replace_remote_state() {
        let client = Client::default();
        client.listening(Some(12345));
        client.active(true);
        assert!(
            client
                .dispatch(&ApiRequest::PlaybackState { seq: 1 })
                .await
                .is_none()
        );
        client.bind(Some(Device::default()));
        client.active(false);
        assert!(
            client
                .dispatch(&ApiRequest::PlaybackState { seq: 1 })
                .await
                .is_none()
        );
    }
    fn remote(action: RemoteAction) -> ApiRequest {
        ApiRequest::Remote {
            action,
            device_id: Some("soloist".into()),
            play: None,
            position_ms: 0,
            percent: 50,
            flag: false,
            repeat: "off".into(),
        }
    }
    #[tokio::test]
    async fn acknowledgements_are_correlated_and_remote_errors_are_redacted() {
        for reject in [false, true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                let message = socket.next().await.unwrap().unwrap();
                let command: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
                assert_eq!(command["type"], "command");
                assert_eq!(command["command"], "pause");
                socket
                    .send(Message::Text(
                        json!({"type":"command_result", "command":"play"})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
                let result = if reject {
                    json!({"type":"error", "message":"private-server-detail"})
                } else {
                    json!({"type":"command_result", "command":"pause"})
                };
                socket
                    .send(Message::Text(result.to_string().into()))
                    .await
                    .unwrap();
            });
            let client = Client::default();
            client.listening(Some(port));
            client.bind(Some(Device {
                id: Some("soloist".into()),
                ..Default::default()
            }));
            let Some(ApiResponse::Remote { result, .. }) =
                client.dispatch(&remote(RemoteAction::Pause)).await
            else {
                panic!("command did not reach Soloist");
            };
            assert_eq!(result.is_err(), reject);
            if let Err(error) = result {
                assert!(!error.to_string().contains("private-server-detail"));
            }
            server.await.unwrap();
        }
    }
    #[tokio::test]
    async fn event_connection_recovers_after_soloist_drops_it() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            tokio_tungstenite::accept_async(stream)
                .await
                .unwrap()
                .close(None)
                .await
                .unwrap();
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            socket
                .send(Message::Text(
                    json!({"type":"auth_state","logged_in":false})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
        });
        let mut socket = connect(port).await.unwrap();
        let (message, reconnected) = receive_reconnecting(&mut socket, port).await.unwrap();
        assert!(message.is_none());
        assert!(reconnected);
        let (message, reconnected) = receive_reconnecting(&mut socket, port).await.unwrap();
        let message = message.unwrap();
        assert_eq!(message["type"], "auth_state");
        assert!(!reconnected);
        server.await.unwrap();
    }
    #[tokio::test]
    async fn command_connection_recovers_after_soloist_drops_it() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            socket.next().await.unwrap().unwrap();
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let message = socket.next().await.unwrap().unwrap();
            let command: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
            assert_eq!(command["command"], "get_state");
        });
        let mut socket = connect(port).await.unwrap();
        socket.close(None).await.unwrap();
        assert!(
            send_reconnecting(&mut socket, port, json!({"command":"get_state"}))
                .await
                .unwrap()
        );
        server.await.unwrap();
    }
    #[tokio::test]
    async fn playlist_offsets_and_other_devices_keep_the_existing_route() {
        let client = Client::default();
        client.listening(Some(12345));
        client.bind(Some(Device {
            id: Some("soloist".into()),
            ..Default::default()
        }));
        let mut request = remote(RemoteAction::Play);
        if let ApiRequest::Remote { play, .. } = &mut request {
            *play = Some(crate::api::PlayRequest {
                context_uri: Some("spotify:playlist:original".into()),
                offset_position: Some(17),
                ..Default::default()
            });
        }
        assert!(client.dispatch(&request).await.is_none());
        let mut other = remote(RemoteAction::Pause);
        if let ApiRequest::Remote { device_id, .. } = &mut other {
            *device_id = Some("speaker".into());
        }
        assert!(client.dispatch(&other).await.is_none());
    }
    /// A lone song starts as its own context, the way Spotify's apps play
    /// one from search, so Spotify's autoplay follows that song. Lists,
    /// contexts, episodes, resumes partway in and a Soloist that is not
    /// yet the active device keep the Web API route.
    #[test]
    fn a_lone_song_plays_as_its_own_context() {
        let play = |request: crate::api::PlayRequest, active: bool| {
            let mut remote = remote(RemoteAction::Play);
            if let ApiRequest::Remote { play, .. } = &mut remote {
                *play = Some(request);
            }
            commands(&remote, active)
        };
        let song = || crate::api::PlayRequest::tracks(vec!["spotify:track:a".into()]);
        let expected = Some(vec![json!({"command":"play", "uri":"spotify:track:a"})]);
        assert_eq!(play(song(), true), expected);
        assert_eq!(play(song().starting_at_index(0), true), expected);
        assert_eq!(
            play(crate::api::PlayRequest::context("spotify:track:a"), true),
            expected
        );
        // A resume names the later song its context had moved on to.
        assert_eq!(
            play(
                crate::api::PlayRequest::context("spotify:track:old")
                    .starting_at_uri("spotify:track:a"),
                true
            ),
            expected
        );

        assert_eq!(play(song(), false), None);
        let mut partway = song();
        partway.position_ms = 30_000;
        assert_eq!(play(partway, true), None);
        let list = crate::api::PlayRequest::tracks(vec![
            "spotify:track:a".into(),
            "spotify:track:b".into(),
        ]);
        assert_eq!(play(list.starting_at_index(1), true), None);
        let episode = crate::api::PlayRequest::tracks(vec!["spotify:episode:e".into()]);
        assert_eq!(play(episode, true), None);
        let playlist = crate::api::PlayRequest::context("spotify:playlist:p")
            .starting_at_uri("spotify:track:a");
        assert_eq!(play(playlist, true), None);
    }
    #[test]
    fn repeat_track_does_not_also_enable_context_repeat() {
        let mut request = remote(RemoteAction::Repeat);
        if let ApiRequest::Remote { repeat, .. } = &mut request {
            *repeat = "track".into();
        }
        assert_eq!(
            commands(&request, true).unwrap(),
            vec![
                json!({"command":"set_repeat_context","enabled":false}),
                json!({"command":"set_repeat_track","enabled":true})
            ]
        );
    }
    #[tokio::test]
    async fn a_transfer_during_a_queue_read_leaves_the_new_devices_queue_alone() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let command = socket.next().await.unwrap().unwrap();
            let command: Value = serde_json::from_str(command.to_text().unwrap()).unwrap();
            assert_eq!(command["command"], "get_state");
            socket
                .send(Message::Text(
                    json!({"type":"playback_state","is_active":false})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
        });
        let client = Client::default();
        client.listening(Some(port));
        client.bind(Some(Device {
            id: Some("soloist".into()),
            ..Default::default()
        }));
        client.active(true);
        assert!(
            client
                .dispatch(&ApiRequest::Queue { seq: 9 })
                .await
                .is_none()
        );
        server.await.unwrap();
    }

    /// The queue keeps each row's source and id as Soloist reports them,
    /// so the panel splits by them, and the player's own history. A row
    /// Soloist sends bare, without names or even an entity type, stays in
    /// its place to be filled in later.
    #[test]
    fn the_queue_keeps_sources_ids_history_and_bare_rows() {
        let state = json!({"is_active":true,"item":{"uri":"spotify:track:now","entity_type":"track","decorations":{"identity":{"name":"Now"}}}});
        let queue = json!({
            "type":"queue_changed",
            "previous":[
                {"uid":"p1","source":"context","item":{"uri":"spotify:track:before","entity_type":"track","decorations":{"identity":{"name":"Before"}}}}
            ],
            "upcoming":[
                {"uid":"q1","source":"queue","item":{"uri":"spotify:track:mine","entity_type":"track","decorations":{"identity":{"name":"Mine"}}}},
                {"uid":"c1","source":"context","item":{"uri":"spotify:track:next","entity_type":"track"}},
                {"source":"context","item":{"uri":"spotify:track:bare"}},
                {"uid":"a1","source":"autoplay","item":{"uri":"spotify:episode:talk"}},
                {"uid":"x","source":"queue","item":{"uri":"spotify:artist:nope"}}
            ]
        });
        let got = read_queue(&state, &queue).unwrap();
        assert_eq!(got.currently_playing.as_ref().unwrap().name(), "Now");
        let uris: Vec<&str> = got.queue.iter().map(PlayableItem::uri).collect();
        assert_eq!(
            uris,
            [
                "spotify:track:mine",
                "spotify:track:next",
                "spotify:track:bare",
                "spotify:episode:talk"
            ]
        );
        assert_eq!(got.queue[0].name(), "Mine");
        assert_eq!(got.queue[2].name(), "", "a bare row has no name yet");
        assert_eq!(got.queue[2].id(), Some("bare"));
        let entries = got.entries.clone().unwrap();
        let sources: Vec<QueueProvider> = entries.iter().map(|entry| entry.source).collect();
        assert_eq!(
            sources,
            [
                QueueProvider::Queue,
                QueueProvider::Context,
                QueueProvider::Context,
                QueueProvider::Autoplay
            ]
        );
        let uids: Vec<&str> = entries.iter().map(|entry| entry.uid.as_str()).collect();
        assert_eq!(uids, ["q1", "c1", "spotify:track:bare", "a1"]);
        assert_eq!(got.provided_queued_len(), Some(1));
        assert_eq!(got.autoplay_from(), Some(3));
        assert_eq!(got.previous.len(), 1);
        assert_eq!(got.previous[0].uri(), "spotify:track:before");
    }

    /// A Soloist that acknowledges every command, each on the connection
    /// the client opens for it, and reports the commands it was sent.
    async fn acknowledging_soloist(commands: usize) -> (u16, tokio::task::JoinHandle<Vec<Value>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let mut seen = Vec::new();
            for _ in 0..commands {
                let (stream, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                let message = socket.next().await.unwrap().unwrap();
                let command: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
                socket
                    .send(Message::Text(
                        json!({"type":"command_result", "command":command["command"]})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
                seen.push(command);
            }
            seen
        });
        (port, server)
    }

    fn active_soloist(port: u16) -> Client {
        let client = Client::default();
        client.listening(Some(port));
        client.bind(Some(Device {
            id: Some("soloist".into()),
            ..Default::default()
        }));
        client.active(true);
        client
    }

    /// With Shuffle on, a lone song still starts as its own context
    /// through Soloist's `play`, so autoplay follows it (#1320). A
    /// shuffled collection keeps the Web API's route.
    #[tokio::test]
    async fn a_lone_song_with_shuffle_on_still_plays_as_its_own_context() {
        let (port, server) = acknowledging_soloist(1).await;
        let client = active_soloist(port);
        let lone = ApiRequest::ShufflePlay {
            device_id: Some("soloist".into()),
            play: crate::api::PlayRequest::tracks(vec!["spotify:track:a".into()]),
        };
        let Some(ApiResponse::Remote {
            action: RemoteAction::Play,
            result,
        }) = client.dispatch(&lone).await
        else {
            panic!("the lone song did not reach Soloist");
        };
        assert!(result.is_ok());
        assert_eq!(
            server.await.unwrap(),
            [json!({"type":"command", "command":"play", "uri":"spotify:track:a"})]
        );
        let collection = ApiRequest::ShufflePlay {
            device_id: Some("soloist".into()),
            play: crate::api::PlayRequest::context("spotify:playlist:p"),
        };
        assert!(client.dispatch(&collection).await.is_none());
    }

    /// Songs added to the queue go through Soloist's own `add_to_queue`,
    /// one after another in order, with no Web API request. A batch with
    /// an episode, which Soloist refuses, keeps the Web API's route.
    #[tokio::test]
    async fn additions_go_through_soloists_own_queue_in_order() {
        let (port, server) = acknowledging_soloist(3).await;
        let client = active_soloist(port);
        let uris: Vec<String> = ["a", "b", "a"]
            .iter()
            .map(|id| format!("spotify:track:{id}"))
            .collect();
        let batch = ApiRequest::AddManyToQueue {
            request: 7,
            uris: uris.clone(),
            device_id: Some("soloist".into()),
        };
        let Some(ApiResponse::QueueBatchAdded {
            request,
            added,
            result,
        }) = client.dispatch(&batch).await
        else {
            panic!("the batch did not reach Soloist");
        };
        assert_eq!((request, added), (7, 3));
        assert!(result.is_ok());
        let sent: Vec<Value> = server.await.unwrap();
        let expected: Vec<Value> = uris
            .iter()
            .map(|uri| json!({"type":"command", "command":"add_to_queue", "uri":uri}))
            .collect();
        assert_eq!(sent, expected);
        let episodes = ApiRequest::AddManyToQueue {
            request: 8,
            uris: vec!["spotify:track:a".into(), "spotify:episode:e".into()],
            device_id: Some("soloist".into()),
        };
        assert!(client.dispatch(&episodes).await.is_none());
    }
}
