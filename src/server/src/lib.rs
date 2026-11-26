//! The multiplayer server: serves the client build, precompressed and cached, and one game room
//! over WebSocket on one plain-HTTP port, replaying every player's inputs in its own copy of their
//! world to score them.

pub mod bot;
pub mod catalog;
mod files;
pub mod protocol;
pub mod replay;
pub mod room;
pub mod standup;

use std::future::IntoFuture;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use axum::routing::get;
use cake_core::sim::TICKS_PER_S;
use tokio::net::TcpListener;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::time::Instant;

use protocol::{ClientMessage, FrameError, PlayerId, decode_input_frame};
use room::{ConnectionId, DebugCommand, Frame, NameTaken, NextStep, Room, RoomEvent, create_outbox};

/// The longest name a player may type, in characters.
pub const MAX_NAME_LEN: usize = 24;
/// The largest WebSocket message a client may send, in bytes, past which its connection closes.
///
/// The largest a client has cause to send is a join with a name of [`MAX_NAME_LEN`] four-byte
/// characters, a preference of 17 significant digits, no scrum update, a seat token, and the
/// `?debug` flag, about 245 bytes.
pub const MAX_MESSAGE_SIZE: usize = 256;
/// Text messages a connection may send at once, past which it must keep to
/// [`TEXT_MESSAGES_PER_S`] or be closed.
///
/// A page sends a hash a second and a control message per click, while each of some, such as a
/// host claim, costs a message to every player.
pub const TEXT_MESSAGE_BURST: u32 = 30;
pub const TEXT_MESSAGES_PER_S: u32 = 10;
/// Events the connections may have waiting for the room, past which each waits its turn to add
/// one and stops reading its socket meanwhile. A second of every input from 16 pages.
const EVENTS_LEN: usize = 1024;
/// The room steps once per tick of the simulation.
const STEP_INTERVAL: Duration = Duration::from_nanos(1_000_000_000 / TICKS_PER_S as u64);
/// Pings keep an idle connection open through a reverse proxy's read timeout.
const PING_INTERVAL: Duration = Duration::from_secs(20);
/// How long one frame may take to send before the connection is closed as stalled.
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
struct AppState {
    events: Sender<RoomEvent>,
    next_connection: Arc<AtomicU32>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum NameError {
    Empty,
    TooLong,
    ControlCharacter,
}

/// A standup order preference outside 0 to 1.
#[derive(Debug, PartialEq, Eq)]
pub struct PreferenceOutOfRange;

/// Serves `client_dir` and the room's WebSocket at `ws` on `listener` until the listener fails or
/// the room stops.
///
/// The room runs on its own thread, stepping at the simulation's tick rate while a round loads,
/// plays, or is judged, and otherwise only as a held seat expires, and stops only by panicking,
/// which the panic hook logs.
///
/// # Errors
/// When the room's thread or runtime cannot start, `client_dir` cannot be read, serving fails,
/// or the room stops.
pub async fn serve(listener: TcpListener, client_dir: PathBuf, room: Room) -> std::io::Result<()> {
    let (events, received) = tokio::sync::mpsc::channel(EVENTS_LEN);
    let (room_alive, room_stopped) = tokio::sync::oneshot::channel::<()>();
    // Its own runtime on its own thread, so the room's steps never hold up a connection's task.
    let room_runtime = tokio::runtime::Builder::new_current_thread().enable_time().build()?;
    std::thread::Builder::new().name("room".into()).spawn(move || {
        // Dropped as the thread ends, by a return or a panic's unwinding, which ends `serve`.
        let _room_alive = room_alive;
        room_runtime.block_on(run_room(room, received));
    })?;
    let state = AppState { events, next_connection: Arc::new(AtomicU32::new(1)) };
    let files = files::build_router(&client_dir)?;
    let router = Router::new().route("/ws", get(on_upgrade)).with_state(state).fallback_service(files);
    tokio::select! {
        served = axum::serve(listener, router).into_future() => served,
        _ = room_stopped => Err(std::io::Error::other("the room thread stopped")),
    }
}

/// Trims a typed name and accepts it if it is 1 to [`MAX_NAME_LEN`] printable characters.
///
/// # Errors
/// [`NameError`] naming the first rule the trimmed name breaks.
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

/// Accepts a standup order preference from 0, first to give an update, to 1, last.
///
/// # Errors
/// [`PreferenceOutOfRange`] for any other value, NaN included.
pub fn validate_standup_preference(preference: f64) -> Result<f64, PreferenceOutOfRange> {
    if !(0.0..=1.0).contains(&preference) {
        return Err(PreferenceOutOfRange);
    }
    Ok(preference)
}

/// Steps `room` as often as it asks, at most once per [`STEP_INTERVAL`], and gives it every event,
/// so a room with nothing timed on wakes the host's CPU only for events.
async fn run_room(mut room: Room, mut events: Receiver<RoomEvent>) {
    let start = Instant::now();
    // The earliest the next step may run, which keeps the steps a tick apart.
    let mut next_step = start;
    loop {
        let step_at = find_step_instant(room.get_next_step(), start, next_step);
        tokio::select! {
            // A step that is due goes before any event waiting, however many connections send.
            biased;
            due = sleep_until_some(step_at) => {
                let now = Instant::now();
                room.step(calc_ms_since(start, now));
                // A step that overran its interval skips the steps it missed rather than bunching them.
                next_step = (due + STEP_INTERVAL).max(now);
            }
            event = events.recv() => {
                let Some(event) = event else {
                    return;
                };
                let now = Instant::now();
                room.on_event(event, calc_ms_since(start, now));
                // After a wait on events alone, a step this event makes due, as a round's start
                // does, runs at once and the next a tick later, rather than two at once.
                next_step = next_step.max(now);
            }
        }
    }
}

/// When the room's next step is due, none while it waits for an event.
fn find_step_instant(next: NextStep, start: Instant, next_step: Instant) -> Option<Instant> {
    match next {
        NextStep::EveryTick => Some(next_step),
        // Out of the clock's range is never.
        NextStep::At { at_ms } => start.checked_add(Duration::from_millis(at_ms)).map(|at| at.max(next_step)),
        NextStep::AfterEvent => None,
    }
}

/// Sleeps until `at` and returns it, or sleeps forever without one.
async fn sleep_until_some(at: Option<Instant>) -> Instant {
    let Some(at) = at else {
        return std::future::pending().await;
    };
    tokio::time::sleep_until(at).await;
    at
}

fn calc_ms_since(start: Instant, now: Instant) -> u64 {
    u64::try_from((now - start).as_millis()).unwrap_or(u64::MAX)
}

async fn on_upgrade(upgrade: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    upgrade
        .max_message_size(MAX_MESSAGE_SIZE)
        .max_frame_size(MAX_MESSAGE_SIZE)
        .on_upgrade(move |socket| run_connection(socket, state))
}

async fn run_connection(mut socket: WebSocket, state: AppState) {
    let mut text_rate = TextRate::create(Instant::now());
    let Some(Joining { name, standup_preference, seat, is_debug }) = wait_join(&mut socket, &mut text_rate).await
    else {
        return;
    };
    let connection = state.next_connection.fetch_add(1, Ordering::Relaxed);
    let (outbox, mut inbox) = create_outbox();
    let (assigned, assigned_id) = tokio::sync::oneshot::channel();
    let join = RoomEvent::Join { connection, name, standup_preference, seat, is_debug, outbox, assigned };
    if state.events.send(join).await.is_err() {
        return;
    }
    let id = match assigned_id.await {
        Ok(Ok(id)) => id,
        Ok(Err(NameTaken)) => {
            let refusal =
                serde_json::to_string(&protocol::ServerMessage::NameTaken).expect("server messages serialize");
            // The page learns of the refusal from this message or, failing that, the close.
            let _ = try_send_in_time(&mut socket, Message::Text(refusal.into())).await;
            return;
        }
        Err(_) => return,
    };
    let mut ping = tokio::time::interval(PING_INTERVAL);
    let mut bad_frames = BadFrames::default();
    // The room disconnects a connection by closing its outbox, whose frames still queued would
    // otherwise go out first, keeping the page from learning it must reopen its seat.
    while !inbox.is_closed() {
        tokio::select! {
            received = socket.recv() => {
                let Some(Ok(message)) = received else {
                    break;
                };
                if matches!(message, Message::Text(_)) && !text_rate.try_admit(Instant::now()) {
                    eprintln!("#{id}: text messages over {TEXT_MESSAGES_PER_S} a second, disconnecting");
                    break;
                }
                let event = to_event(connection, message).unwrap_or_else(|error| {
                    bad_frames.note(id, error, Instant::now());
                    None
                });
                // A full queue holds this connection here, unread, until the room catches up.
                if let Some(event) = event && state.events.send(event).await.is_err() {
                    break;
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
    bad_frames.flush(id);
    let _ = state.events.send(RoomEvent::Leave { connection }).await;
}

/// A connection's malformed input frames, logged at most once per room step, so a flood of them
/// costs one line a step.
#[derive(Default)]
struct BadFrames {
    /// Frames since the last line, and the error of the newest.
    unlogged: Option<(u32, FrameError)>,
    logged: Option<Instant>,
}

impl BadFrames {
    fn note(&mut self, id: PlayerId, error: FrameError, now: Instant) {
        let len = self.unlogged.take().map_or(0, |(len, _)| len) + 1;
        self.unlogged = Some((len, error));
        if self.logged.is_some_and(|logged| now - logged < STEP_INTERVAL) {
            return;
        }
        self.flush(id);
        self.logged = Some(now);
    }

    /// Logs the frames noted since the last line, if any.
    fn flush(&mut self, id: PlayerId) {
        match self.unlogged.take() {
            None => {}
            Some((1, error)) => eprintln!("#{id}: bad input frame: {error:?}"),
            Some((len, error)) => eprintln!("#{id}: {len} bad input frames, the last: {error:?}"),
        }
    }
}

/// A connection's allowance of text messages: [`TEXT_MESSAGE_BURST`] at once, refilled at
/// [`TEXT_MESSAGES_PER_S`].
struct TextRate {
    allowance: f64,
    refilled: Instant,
}

impl TextRate {
    fn create(now: Instant) -> Self {
        Self { allowance: f64::from(TEXT_MESSAGE_BURST), refilled: now }
    }

    /// Counts one text message at `now` against the allowance, returning whether it was within it.
    fn try_admit(&mut self, now: Instant) -> bool {
        let refill = (now - self.refilled).as_secs_f64() * f64::from(TEXT_MESSAGES_PER_S);
        self.allowance = (self.allowance + refill).min(f64::from(TEXT_MESSAGE_BURST));
        self.refilled = now;
        if self.allowance < 1.0 {
            return false;
        }
        self.allowance -= 1.0;
        true
    }
}

/// Whether `message` went out within [`SEND_TIMEOUT`]. A peer that stops reading stalls the send
/// once the socket's buffers fill, and would otherwise hold its connection open forever.
async fn try_send_in_time(socket: &mut WebSocket, message: Message) -> bool {
    matches!(tokio::time::timeout(SEND_TIMEOUT, socket.send(message)).await, Ok(Ok(())))
}

/// A connection's validated join, its standup preference none without a scrum update.
struct Joining {
    name: String,
    standup_preference: Option<f64>,
    seat: Option<String>,
    is_debug: bool,
}

/// The validated join from the connection's first join message, or none if it closes first,
/// exceeds `text_rate`, or the join is refused, which tells the client why.
async fn wait_join(socket: &mut WebSocket, text_rate: &mut TextRate) -> Option<Joining> {
    while let Some(Ok(message)) = socket.recv().await {
        let Message::Text(text) = message else {
            continue;
        };
        if !text_rate.try_admit(Instant::now()) {
            eprintln!("connection: text messages over {TEXT_MESSAGES_PER_S} a second before joining, disconnecting");
            return None;
        }
        let Ok(ClientMessage::Join { name, standup_preference, has_scrum_update, seat, is_debug }) =
            serde_json::from_str(text.as_str())
        else {
            eprintln!("connection: expected a join message, got {:?}", text.as_str());
            continue;
        };
        let validated = match (validate_name(&name), validate_standup_preference(standup_preference)) {
            (Ok(name), Ok(standup_preference)) => {
                Ok(Joining { name, standup_preference: has_scrum_update.then_some(standup_preference), seat, is_debug })
            }
            (Err(error), _) => Err(format!("name refused: {error:?}")),
            (_, Err(PreferenceOutOfRange)) => Err(format!("standup preference {standup_preference} refused")),
        };
        match validated {
            Ok(joined) => return Some(joined),
            Err(message) => {
                eprintln!("connection: join {name:?} refused: {message}");
                let refusal = protocol::ServerMessage::Error { message };
                let _ = socket
                    .send(Message::Text(serde_json::to_string(&refusal).expect("server messages serialize").into()))
                    .await;
                return None;
            }
        }
    }
    None
}

/// The room event a joined connection's message carries, none for a text message the room has no
/// use for, which is logged, or a malformed input frame's error.
fn to_event(connection: ConnectionId, message: Message) -> Result<Option<RoomEvent>, FrameError> {
    match message {
        Message::Binary(bytes) => {
            decode_input_frame(&bytes).map(|(tick, input)| Some(RoomEvent::Input { connection, tick, input }))
        }
        Message::Text(text) => Ok(serde_json::from_str(text.as_str())
            .inspect_err(|error| eprintln!("connection {connection}: bad message {:?}: {error}", text.as_str()))
            .ok()
            .and_then(|parsed| to_control_event(connection, parsed))),
        Message::Ping(_) | Message::Pong(_) | Message::Close(_) => Ok(None),
    }
}

fn to_control_event(connection: ConnectionId, parsed: ClientMessage) -> Option<RoomEvent> {
    match parsed {
        ClientMessage::Join { .. } => {
            eprintln!("connection {connection}: joined twice");
            None
        }
        ClientMessage::ClaimHost { password } => Some(RoomEvent::ClaimHost { connection, password }),
        ClientMessage::Pick { cake } => Some(RoomEvent::Pick { connection, cake }),
        ClientMessage::Start => Some(RoomEvent::Start { connection }),
        ClientMessage::ReturnToLobby => Some(RoomEvent::ReturnToLobby { connection }),
        ClientMessage::Ready => Some(RoomEvent::Ready { connection }),
        ClientMessage::Cut { tick } => Some(RoomEvent::Cut { connection, tick }),
        ClientMessage::Hash { tick, hash } => match hash.parse() {
            Ok(hash) => Some(RoomEvent::Hash { connection, tick, hash }),
            Err(error) => {
                eprintln!("connection {connection}: bad hash {hash:?}: {error}");
                None
            }
        },
        ClientMessage::AddBot => Some(RoomEvent::Debug { connection, command: DebugCommand::AddBot }),
        ClientMessage::RemoveBot => Some(RoomEvent::Debug { connection, command: DebugCommand::RemoveBot }),
        ClientMessage::SetRoundLength { round_s } => {
            Some(RoomEvent::Debug { connection, command: DebugCommand::SetRoundLength { round_s } })
        }
        ClientMessage::SetBotLatency { latency_ms } => {
            Some(RoomEvent::Debug { connection, command: DebugCommand::SetBotLatency { latency_ms } })
        }
    }
}
