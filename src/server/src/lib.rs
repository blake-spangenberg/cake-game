//! The multiplayer server: serves the client build and one game room over WebSocket on one
//! plain-HTTP port, replaying every player's inputs in its own copy of their world to score them.

pub mod catalog;
pub mod protocol;
pub mod replay;
pub mod room;

use std::future::IntoFuture;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use axum::routing::get;
use cake_core::sim::TICKS_PER_S;
use tokio::net::TcpListener;
use tower_http::services::ServeDir;

use protocol::{ClientMessage, PlayerId, decode_input_frame};
use room::{Frame, Room, RoomEvent, create_outbox};

/// The longest name a player may type, in characters.
pub const MAX_NAME_LEN: usize = 24;
/// The largest WebSocket message a client may send, in bytes, past which its connection closes.
/// The largest a client has cause to send is a join with a name of [`MAX_NAME_LEN`] four-byte
/// characters, about 120 bytes.
pub const MAX_MESSAGE_SIZE: usize = 256;
/// Pings keep an idle connection open through a reverse proxy's read timeout.
const PING_INTERVAL: Duration = Duration::from_secs(20);
/// How long one frame may take to send before the connection is closed as stalled.
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
struct AppState {
    events: Sender<RoomEvent>,
    next_id: Arc<AtomicU32>,
}

#[derive(Debug, PartialEq)]
pub enum NameError {
    Empty,
    TooLong,
    ControlCharacter,
}

/// Serves `client_dir` and the room's WebSocket at `ws` on `listener` until the listener fails or
/// the room stops. The room runs on its own thread, stepping at the simulation's tick rate, and
/// stops only by panicking, which the panic hook logs.
pub async fn serve(listener: TcpListener, client_dir: PathBuf, room: Room) -> std::io::Result<()> {
    let (events, received) = std::sync::mpsc::channel();
    let (room_alive, room_stopped) = tokio::sync::oneshot::channel::<()>();
    std::thread::Builder::new().name("room".into()).spawn(move || {
        // Dropped as the thread ends, by a return or a panic's unwinding, which ends `serve`.
        let _room_alive = room_alive;
        run_room(room, received);
    })?;
    let state = AppState { events, next_id: Arc::new(AtomicU32::new(1)) };
    let router =
        Router::new().route("/ws", get(on_upgrade)).fallback_service(ServeDir::new(client_dir)).with_state(state);
    tokio::select! {
        served = axum::serve(listener, router).into_future() => served,
        _ = room_stopped => Err(std::io::Error::other("the room thread stopped")),
    }
}

/// Trims a typed name and accepts it if it is 1 to [`MAX_NAME_LEN`] printable characters.
pub fn validate_name(typed: &str) -> Result<String, NameError> {
    let name = typed.trim();
    if name.is_empty() {
        return Err(NameError::Empty);
    }
    if name.chars().count() > MAX_NAME_LEN {
        return Err(NameError::TooLong);
    }
    if name.chars().any(char::is_control) {
        return Err(NameError::ControlCharacter);
    }
    Ok(name.to_string())
}

fn run_room(mut room: Room, events: Receiver<RoomEvent>) {
    let start = Instant::now();
    let step_interval = Duration::from_secs(1) / TICKS_PER_S;
    let mut next_step = start;
    loop {
        let now = Instant::now();
        if now >= next_step {
            room.step(calc_ms_since(start, now));
            // A step that overran its interval skips the steps it missed rather than bunching them.
            next_step = (next_step + step_interval).max(now);
            continue;
        }
        match events.recv_timeout(next_step - now) {
            Ok(event) => room.on_event(event, calc_ms_since(start, Instant::now())),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn calc_ms_since(start: Instant, now: Instant) -> u64 {
    (now - start).as_millis() as u64
}

async fn on_upgrade(upgrade: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    upgrade
        .max_message_size(MAX_MESSAGE_SIZE)
        .max_frame_size(MAX_MESSAGE_SIZE)
        .on_upgrade(move |socket| run_connection(socket, state))
}

async fn run_connection(mut socket: WebSocket, state: AppState) {
    let Some(name) = wait_join(&mut socket).await else {
        return;
    };
    let id = state.next_id.fetch_add(1, Ordering::Relaxed);
    let (outbox, mut inbox) = create_outbox();
    if state.events.send(RoomEvent::Join { id, name, outbox }).is_err() {
        return;
    }
    let mut ping = tokio::time::interval(PING_INTERVAL);
    loop {
        tokio::select! {
            received = socket.recv() => {
                let Some(Ok(message)) = received else {
                    break;
                };
                if let Some(event) = to_event(id, message) {
                    let _ = state.events.send(event);
                }
            }
            frame = inbox.recv() => {
                let Some(frame) = frame else {
                    break;
                };
                let message = match frame {
                    Frame::Text(text) => Message::Text(text.into()),
                    Frame::Binary(bytes) => Message::Binary(bytes.into()),
                };
                if !try_send_in_time(&mut socket, message).await {
                    break;
                }
            }
            _ = ping.tick() => {
                if !try_send_in_time(&mut socket, Message::Ping(Bytes::new())).await {
                    break;
                }
            }
        }
    }
    let _ = state.events.send(RoomEvent::Leave { id });
}

/// Whether `message` went out within [`SEND_TIMEOUT`]. A peer that stops reading stalls the send
/// once the socket's buffers fill, and would otherwise hold its connection open forever.
async fn try_send_in_time(socket: &mut WebSocket, message: Message) -> bool {
    matches!(tokio::time::timeout(SEND_TIMEOUT, socket.send(message)).await, Ok(Ok(())))
}

/// The validated name from the connection's first join message, or none if it closes first or
/// names no valid name.
async fn wait_join(socket: &mut WebSocket) -> Option<String> {
    while let Some(Ok(message)) = socket.recv().await {
        let Message::Text(text) = message else {
            continue;
        };
        let Ok(ClientMessage::Join { name }) = serde_json::from_str(text.as_str()) else {
            eprintln!("connection: expected a join message, got {text}");
            continue;
        };
        match validate_name(&name) {
            Ok(name) => return Some(name),
            Err(error) => {
                eprintln!("connection: name {name:?} refused: {error:?}");
                let refusal = protocol::ServerMessage::Error { message: format!("name refused: {error:?}") };
                let _ = socket.send(Message::Text(serde_json::to_string(&refusal).unwrap().into())).await;
                return None;
            }
        }
    }
    None
}

/// The room event a joined player's message carries, or none for a message the room has no use
/// for, which is logged.
fn to_event(id: PlayerId, message: Message) -> Option<RoomEvent> {
    match message {
        Message::Binary(bytes) => decode_input_frame(&bytes)
            .inspect_err(|error| eprintln!("#{id}: bad input frame: {error:?}"))
            .ok()
            .map(|(tick, input)| RoomEvent::Input { id, tick, input }),
        Message::Text(text) => serde_json::from_str(text.as_str())
            .inspect_err(|error| eprintln!("#{id}: bad message {text}: {error}"))
            .ok()
            .and_then(|parsed| to_control_event(id, parsed)),
        Message::Ping(_) | Message::Pong(_) | Message::Close(_) => None,
    }
}

fn to_control_event(id: PlayerId, parsed: ClientMessage) -> Option<RoomEvent> {
    match parsed {
        ClientMessage::Join { .. } => {
            eprintln!("#{id}: joined twice");
            None
        }
        ClientMessage::Start { cake } => Some(RoomEvent::Start { id, cake }),
        ClientMessage::Ready => Some(RoomEvent::Ready { id }),
        ClientMessage::Cut { tick } => Some(RoomEvent::Cut { id, tick }),
        ClientMessage::Hash { tick, hash } => match hash.parse() {
            Ok(hash) => Some(RoomEvent::Hash { id, tick, hash }),
            Err(error) => {
                eprintln!("#{id}: bad hash {hash:?}: {error}");
                None
            }
        },
    }
}
