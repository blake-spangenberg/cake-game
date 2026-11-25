//! The server run in process on a loopback port, driven by WebSocket clients.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use cake_core::cakes::build_slice_missing_cake;
use cake_core::input::Input;
use cake_core::knife::START_TILT_RAD;
use cake_core::replay::{build_placement_inputs, build_scripted_inputs};
use cake_core::sim::World;
use cake_server::catalog::Catalog;
use cake_server::protocol::{ClientMessage, PlayerId, ServerMessage, Standing, encode_input_frame};
use cake_server::room::{Room, RoomConfig};
use cake_server::{MAX_MESSAGE_SIZE, MAX_NAME_LEN, NameError, serve, validate_name};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const CAKE: &str = "slice";
const ROUND_TICKS: u32 = 900;
const INDEX_HTML: &str = "<!doctype html><title>Cake Cut</title>";
/// Longer than any message should take on loopback, so a missing one fails the test instead of hanging it.
const MESSAGE_TIMEOUT: Duration = Duration::from_secs(10);

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

static NEXT_CLIENT_DIR: AtomicUsize = AtomicUsize::new(0);

/// A client build holding only an index page and the placeholder cake, in a directory no other call shares.
fn write_client_dir() -> PathBuf {
    let id = NEXT_CLIENT_DIR.fetch_add(1, Ordering::Relaxed);
    let dir =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cake-server-client-{}-{id}", std::process::id()));
    std::fs::create_dir_all(dir.join("cakes")).unwrap();
    std::fs::write(dir.join("index.html"), INDEX_HTML).unwrap();
    std::fs::write(dir.join("cakes").join(format!("{CAKE}.cake")), build_slice_missing_cake().encode()).unwrap();
    dir
}

async fn start_server() -> String {
    let client_dir = write_client_dir();
    let catalog = Catalog::load(&client_dir.join("cakes")).unwrap();
    // A lead past the round's end lets a test send a whole round's inputs at once.
    let config = RoomConfig { round_ticks: ROUND_TICKS, max_lead_ticks: 2 * ROUND_TICKS, ..RoomConfig::default() };
    let room = Room::create(config, catalog);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    tokio::spawn(serve(listener, client_dir, room));
    address
}

async fn connect(address: &str, name: &str) -> Socket {
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws")).await.unwrap();
    send(&mut socket, &ClientMessage::Join { name: name.to_string() }).await;
    socket
}

async fn send(socket: &mut Socket, message: &ClientMessage) {
    socket.send(Message::text(serde_json::to_string(message).unwrap())).await.unwrap();
}

async fn send_inputs(socket: &mut Socket, inputs: &[Input]) {
    for (tick, input) in inputs.iter().enumerate() {
        socket.send(Message::binary(encode_input_frame(tick as u32, input))).await.unwrap();
    }
}

/// The first text message `pick` accepts, skipping the rest and every pose frame.
async fn wait_for<T>(socket: &mut Socket, pick: impl Fn(ServerMessage) -> Option<T>) -> T {
    let wait = async {
        loop {
            let message = socket.next().await.expect("connection open").unwrap();
            let Message::Text(text) = message else {
                continue;
            };
            if let Some(picked) = pick(serde_json::from_str(text.as_str()).unwrap()) {
                return picked;
            }
        }
    };
    tokio::time::timeout(MESSAGE_TIMEOUT, wait).await.expect("message arrives")
}

async fn wait_results(socket: &mut Socket) -> Vec<Standing> {
    wait_for(socket, |message| match message {
        ServerMessage::Results { standings } => Some(standings),
        _ => None,
    })
    .await
}

/// The world a client replaying `inputs` sees.
fn replay_locally(inputs: &[Input]) -> World {
    let mut world = World::create(build_slice_missing_cake(), START_TILT_RAD, Some(ROUND_TICKS));
    for input in inputs {
        world.step(input);
    }
    world
}

fn calc_local_score(inputs: &[Input]) -> f64 {
    replay_locally(inputs).cut().calc_score()
}

#[tokio::test]
async fn serves_the_client_build() {
    let address = start_server().await;
    let mut stream = TcpStream::connect(&address).await.unwrap();
    stream.write_all(b"GET / HTTP/1.1\r\nHost: cake\r\nConnection: close\r\n\r\n").await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response.ends_with(INDEX_HTML));
}

#[tokio::test]
async fn two_players_over_websocket_get_the_scores_their_inputs_give_locally() {
    let address = start_server().await;
    let mut alice = connect(&address, "Alice").await;
    let alice_id = wait_for(&mut alice, |message| match message {
        ServerMessage::Welcome { id } => Some(id),
        _ => None,
    })
    .await;
    let mut bob = connect(&address, " Bob ").await;
    let bob_id = wait_for(&mut bob, |message| match message {
        ServerMessage::Welcome { id } => Some(id),
        _ => None,
    })
    .await;
    wait_for(&mut alice, |message| match message {
        ServerMessage::Room { members, .. } if members.len() == 2 => Some(()),
        _ => None,
    })
    .await;
    send(&mut alice, &ClientMessage::Start { cake: CAKE.to_string() }).await;
    for socket in [&mut alice, &mut bob] {
        let players: Vec<PlayerId> = wait_for(socket, |message| match message {
            ServerMessage::Round { cake, players, .. } if cake == CAKE => Some(players),
            _ => None,
        })
        .await;
        assert_eq!(players, [alice_id, bob_id]);
        send(socket, &ClientMessage::Ready).await;
    }
    for socket in [&mut alice, &mut bob] {
        wait_for(socket, |message| matches!(message, ServerMessage::Start { .. }).then_some(())).await;
    }

    let alice_inputs = build_placement_inputs();
    let bob_inputs = &build_scripted_inputs()[..600];
    send_inputs(&mut alice, &alice_inputs).await;
    send(&mut alice, &ClientMessage::Cut { tick: 300 }).await;
    send_inputs(&mut bob, bob_inputs).await;
    send(&mut bob, &ClientMessage::Cut { tick: 600 }).await;

    let standings = wait_results(&mut bob).await;
    let scores: Vec<(PlayerId, String, u32, f64)> =
        standings.into_iter().map(|standing| (standing.id, standing.name, standing.tick, standing.score)).collect();
    let mut expected = vec![
        (alice_id, "Alice".to_string(), 300, calc_local_score(&alice_inputs)),
        (bob_id, "Bob".to_string(), 600, calc_local_score(bob_inputs)),
    ];
    expected.sort_by(|a, b| b.3.total_cmp(&a.3));
    assert_eq!(scores, expected);
}

#[tokio::test]
async fn a_refused_name_gets_an_error_and_a_closed_connection() {
    let address = start_server().await;
    let mut socket = connect(&address, "   ").await;
    let refusal = wait_for(&mut socket, |message| match message {
        ServerMessage::Error { message } => Some(message),
        _ => None,
    })
    .await;
    assert_eq!(refusal, "name refused: Empty");
    let after = tokio::time::timeout(MESSAGE_TIMEOUT, socket.next()).await.expect("connection closes");
    assert!(!matches!(after, Some(Ok(Message::Text(_) | Message::Binary(_)))));
}

#[tokio::test]
async fn malformed_messages_are_ignored_and_a_wrong_hash_is_reported() {
    let address = start_server().await;
    let mut alice = connect(&address, "Alice").await;
    send(&mut alice, &ClientMessage::Start { cake: CAKE.to_string() }).await;
    wait_for(&mut alice, |message| matches!(message, ServerMessage::Round { .. }).then_some(())).await;
    send(&mut alice, &ClientMessage::Ready).await;
    wait_for(&mut alice, |message| matches!(message, ServerMessage::Start { .. }).then_some(())).await;

    alice.send(Message::text("{\"type\":\"shout\"}")).await.unwrap();
    alice.send(Message::binary(vec![1, 2, 3])).await.unwrap();
    send(&mut alice, &ClientMessage::Join { name: "Alice".to_string() }).await;
    send(&mut alice, &ClientMessage::Hash { tick: 0, hash: "not a number".to_string() }).await;
    let inputs = &build_placement_inputs()[..60];
    send_inputs(&mut alice, inputs).await;
    let wrong_hash = replay_locally(inputs).calc_hash() ^ 1;
    send(&mut alice, &ClientMessage::Hash { tick: 60, hash: wrong_hash.to_string() }).await;
    let desync_tick = wait_for(&mut alice, |message| match message {
        ServerMessage::Desync { tick } => Some(tick),
        _ => None,
    })
    .await;
    assert_eq!(desync_tick, 60);
}

#[tokio::test]
async fn a_message_over_the_size_limit_closes_its_connection_and_leaves_the_room() {
    let address = start_server().await;
    let mut alice = connect(&address, "Alice").await;
    let mut bob = connect(&address, "Bob").await;
    wait_for(&mut alice, |message| match message {
        ServerMessage::Room { members, .. } if members.len() == 2 => Some(()),
        _ => None,
    })
    .await;

    bob.send(Message::text("x".repeat(MAX_MESSAGE_SIZE + 1))).await.unwrap();
    let members = wait_for(&mut alice, |message| match message {
        ServerMessage::Room { members, .. } if members.len() == 1 => Some(members),
        _ => None,
    })
    .await;
    assert_eq!(members[0].name, "Alice");
    let read_to_close = async {
        while let Some(Ok(message)) = bob.next().await
            && !message.is_close()
        {}
    };
    tokio::time::timeout(MESSAGE_TIMEOUT, read_to_close).await.expect("connection closes");
}

#[test]
fn names_are_trimmed_and_must_be_short_printable_text() {
    assert_eq!(validate_name("  Alice "), Ok("Alice".to_string()));
    assert_eq!(validate_name("   "), Err(NameError::Empty));
    assert_eq!(validate_name(&"x".repeat(25)), Err(NameError::TooLong));
    assert_eq!(validate_name("Al\nice"), Err(NameError::ControlCharacter));
}

/// The name field caps typing at the client's copy of the limit, so a name the server would refuse
/// cannot be typed.
#[test]
fn the_client_caps_names_at_the_servers_limit() {
    let net_ts = include_str!("../../client/src/net.ts");
    assert!(net_ts.contains(&format!("export const MAX_NAME_LEN = {MAX_NAME_LEN};")));
}
