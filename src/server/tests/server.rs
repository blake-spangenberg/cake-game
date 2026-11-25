//! The server run in process on a loopback port, driven by WebSocket clients.
#![expect(
    clippy::cast_possible_truncation,
    clippy::unwrap_used,
    reason = "test helpers, which clippy.toml's test allowances miss, and small tick counts"
)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use cake_core::cakes::build_slice_missing_cake;
use cake_core::input::Input;
use cake_core::knife::START_TILT_RAD;
use cake_core::replay::{build_placement_inputs, build_scripted_inputs};
use cake_core::sim::World;
use cake_server::catalog::Catalog;
use cake_server::protocol::{
    ClientMessage, POSE_FLAG_CUT, POSE_FLAG_HOLDING, POSE_RECORD_SIZE, PhaseName, PlayerId, ServerMessage, Standing,
    encode_input_frame,
};
use cake_server::room::{HOST_PASSWORD, Room, RoomConfig};
use cake_server::{
    MAX_MESSAGE_SIZE, MAX_NAME_LEN, NameError, PreferenceOutOfRange, TEXT_MESSAGE_BURST, TEXT_MESSAGES_PER_S, serve,
    validate_name, validate_standup_preference,
};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const CAKE: &str = "slice";
const ROUND_TICKS: u32 = 900;
const INDEX_HTML: &str = "<!doctype html><title>Cake Cut</title>";
/// Stands in for the page's brotli bytes, which the server sends as they are.
const INDEX_HTML_BR: &str = "brotli bytes";
/// Longer than any message should take on loopback, so a missing one fails the test instead of hanging it.
const MESSAGE_TIMEOUT: Duration = Duration::from_secs(10);
/// Longer than the round and every wait a test makes in it.
const BURST_GRACE_MS: u64 = 60_000;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

static NEXT_CLIENT_DIR: AtomicUsize = AtomicUsize::new(0);

/// A client build holding an index page with a stand-in for its brotli variant, one hashed bundle
/// file, and the placeholder cake, in a directory no other call shares.
fn write_client_dir() -> PathBuf {
    let id = NEXT_CLIENT_DIR.fetch_add(1, Ordering::Relaxed);
    let dir =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cake-server-client-{}-{id}", std::process::id()));
    std::fs::create_dir_all(dir.join("cakes")).unwrap();
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("index.html"), INDEX_HTML).unwrap();
    std::fs::write(dir.join("index.html.br"), INDEX_HTML_BR).unwrap();
    std::fs::write(dir.join("assets").join("index-0123abcd.js"), "export {};").unwrap();
    std::fs::write(dir.join("cakes").join(format!("{CAKE}.cake")), build_slice_missing_cake().encode()).unwrap();
    dir
}

/// The raw response to a `GET` of `path` with the given extra header lines.
async fn fetch_raw(address: &str, path: &str, headers: &str) -> String {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let request = format!("GET {path} HTTP/1.1\r\nHost: cake\r\n{headers}Connection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    response
}

/// The value of header `name`, which must be lowercase, in a raw response.
fn find_header<'a>(response: &'a str, name: &str) -> Option<&'a str> {
    let head = response.split("\r\n\r\n").next().unwrap();
    head.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        (key.to_ascii_lowercase() == name).then(|| value.trim())
    })
}

async fn start_server() -> String {
    let client_dir = write_client_dir();
    let catalog = Catalog::load(&client_dir.join("cakes")).unwrap();
    // A lead past the round's end lets a test send a whole round's inputs at once, and a judging
    // with no wait or reveal gives the results with the last cut. A burst sent seconds into the
    // round on a loaded machine is a page gone silent until then, which the grace would hold, so
    // the grace outlasts any message's wait here, and `tests/room.rs` tests it on a made-up clock.
    let config = RoomConfig {
        round_ticks: ROUND_TICKS,
        max_lead_ticks: 2 * ROUND_TICKS,
        deadline_grace_ms: BURST_GRACE_MS,
        judging_delay_ms: 0,
        judging_scene_ms: 0,
        judging_tail_ms: 0,
        ..RoomConfig::default()
    };
    let room = Room::create(config, catalog);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    tokio::spawn(serve(listener, client_dir, room));
    address
}

async fn connect(address: &str, name: &str) -> Socket {
    connect_preferring(address, name, 0.5).await
}

async fn connect_preferring(address: &str, name: &str, standup_preference: f64) -> Socket {
    connect_seated(address, name, standup_preference, None).await
}

async fn connect_seated(address: &str, name: &str, standup_preference: f64, seat: Option<String>) -> Socket {
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws")).await.unwrap();
    send(&mut socket, &ClientMessage::Join { name: name.to_string(), standup_preference, seat }).await;
    socket
}

async fn wait_welcome(socket: &mut Socket) -> (PlayerId, String) {
    wait_for(socket, |message| match message {
        ServerMessage::Welcome { id, seat } => Some((id, seat)),
        _ => None,
    })
    .await
}

/// Claims host and starts a round on the placeholder cake, the server's only one.
async fn claim_and_start(socket: &mut Socket) {
    send(socket, &ClientMessage::ClaimHost { password: HOST_PASSWORD.to_string() }).await;
    send(socket, &ClientMessage::Start).await;
}

async fn wait_error(socket: &mut Socket) -> String {
    wait_for(socket, |message| match message {
        ServerMessage::Error { message } => Some(message),
        _ => None,
    })
    .await
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
        ServerMessage::Results { standings, .. } => Some(standings),
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
async fn a_file_goes_out_precompressed_where_accepted_and_is_revalidated_by_its_content_tag() {
    let address = start_server().await;
    let plain = fetch_raw(&address, "/", "").await;
    assert!(plain.ends_with(INDEX_HTML));
    let compressed = fetch_raw(&address, "/", "Accept-Encoding: gzip, br\r\n").await;
    assert!(compressed.ends_with(INDEX_HTML_BR));
    assert_eq!(find_header(&compressed, "content-encoding"), Some("br"));
    assert_eq!(find_header(&compressed, "content-type"), Some("text/html"));
    assert_eq!(find_header(&compressed, "vary"), Some("accept-encoding"));
    assert_eq!(find_header(&compressed, "cache-control"), Some("no-cache"));
    let tag = find_header(&compressed, "etag").unwrap();
    assert_eq!(find_header(&plain, "etag"), Some(tag));

    let held = fetch_raw(&address, "/index.html", &format!("If-None-Match: {tag}\r\n")).await;
    assert!(held.starts_with("HTTP/1.1 304"));
    let changed = fetch_raw(&address, "/index.html", "If-None-Match: W/\"0\"\r\n").await;
    assert!(changed.starts_with("HTTP/1.1 200"));
    let hashed = fetch_raw(&address, "/assets/index-0123abcd.js", "").await;
    assert_eq!(find_header(&hashed, "cache-control"), Some("public, max-age=31536000, immutable"));
}

/// A browser that cached a file before it had a tag revalidates by date alone, and the 304 hands
/// it the tag and the caching rules, so it stops guessing how long the file stays fresh.
#[tokio::test]
async fn a_file_revalidated_by_date_gets_its_tag_and_caching_rules_with_the_304() {
    let address = start_server().await;
    let tag = find_header(&fetch_raw(&address, "/index.html", "").await, "etag").unwrap().to_string();
    let held = fetch_raw(&address, "/index.html", "If-Modified-Since: Fri, 01 Jan 2100 00:00:00 GMT\r\n").await;
    assert!(held.starts_with("HTTP/1.1 304"));
    assert_eq!(find_header(&held, "etag"), Some(tag.as_str()));
    assert_eq!(find_header(&held, "cache-control"), Some("no-cache"));
    assert_eq!(find_header(&held, "vary"), Some("accept-encoding"));
}

#[tokio::test]
async fn two_players_over_websocket_get_the_scores_their_inputs_give_locally() {
    let alice_inputs = build_placement_inputs();
    let bob_inputs = &build_scripted_inputs()[..600];
    let address = start_server().await;
    let mut alice = connect(&address, "Alice").await;
    let (alice_id, _) = wait_welcome(&mut alice).await;
    let mut bob = connect(&address, " Bob ").await;
    let (bob_id, _) = wait_welcome(&mut bob).await;
    wait_for(&mut alice, |message| match message {
        ServerMessage::Room { members, .. } if members.len() == 2 => Some(()),
        _ => None,
    })
    .await;
    claim_and_start(&mut alice).await;
    for socket in [&mut alice, &mut bob] {
        let players: Vec<PlayerId> = wait_for(socket, |message| match message {
            ServerMessage::Round { cake, players, .. } if cake == CAKE => {
                Some(players.into_iter().map(|member| member.id).collect())
            }
            _ => None,
        })
        .await;
        assert_eq!(players, [alice_id, bob_id]);
        send(socket, &ClientMessage::Ready).await;
    }
    for socket in [&mut alice, &mut bob] {
        wait_for(socket, |message| matches!(message, ServerMessage::Start { .. }).then_some(())).await;
    }

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
    assert_eq!(wait_error(&mut socket).await, "name refused: Empty");
    let after = tokio::time::timeout(MESSAGE_TIMEOUT, socket.next()).await.expect("connection closes");
    assert!(!matches!(after, Some(Ok(Message::Text(_) | Message::Binary(_)))));
}

#[tokio::test]
async fn a_standup_preference_outside_0_to_1_gets_an_error_and_a_closed_connection() {
    let address = start_server().await;
    let mut socket = connect_preferring(&address, "Alice", 1.5).await;
    assert_eq!(wait_error(&mut socket).await, "standup preference 1.5 refused");
    let after = tokio::time::timeout(MESSAGE_TIMEOUT, socket.next()).await.expect("connection closes");
    assert!(!matches!(after, Some(Ok(Message::Text(_) | Message::Binary(_)))));
}

#[tokio::test]
async fn a_host_claim_with_the_wrong_password_is_refused_and_the_right_one_hosts() {
    let address = start_server().await;
    let mut alice = connect(&address, "Alice").await;
    let (alice_id, _) = wait_welcome(&mut alice).await;
    send(&mut alice, &ClientMessage::ClaimHost { password: "goose".to_string() }).await;
    wait_for(&mut alice, |message| (message == ServerMessage::ClaimRefused).then_some(())).await;
    send(&mut alice, &ClientMessage::ClaimHost { password: HOST_PASSWORD.to_string() }).await;
    let host = wait_for(&mut alice, |message| match message {
        ServerMessage::Room { host: Some(host), .. } => Some(host),
        _ => None,
    })
    .await;
    assert_eq!(host, alice_id);
}

#[tokio::test]
async fn a_page_reopened_with_its_seat_token_plays_as_its_old_id() {
    let address = start_server().await;
    let mut alice = connect(&address, "Alice").await;
    let (alice_id, seat) = wait_welcome(&mut alice).await;
    let mut bob = connect(&address, "Bob").await;
    wait_welcome(&mut bob).await;
    alice.close(None).await.unwrap();
    wait_for(&mut bob, |message| match message {
        ServerMessage::Room { members, .. } if members.len() == 1 => Some(()),
        _ => None,
    })
    .await;

    let mut reopened = connect_seated(&address, "Alice", 0.5, Some(seat.clone())).await;
    assert_eq!(wait_welcome(&mut reopened).await, (alice_id, seat));
}

#[tokio::test]
async fn results_carry_the_standup_order_and_the_host_returns_everyone_to_the_lobby() {
    let address = start_server().await;
    let mut alice = connect_preferring(&address, "Alice", 0.0).await;
    claim_and_start(&mut alice).await;
    wait_for(&mut alice, |message| matches!(message, ServerMessage::Round { .. }).then_some(())).await;
    send(&mut alice, &ClientMessage::Ready).await;
    wait_for(&mut alice, |message| matches!(message, ServerMessage::Start { .. }).then_some(())).await;
    send(&mut alice, &ClientMessage::Cut { tick: 0 }).await;
    let standup = wait_for(&mut alice, |message| match message {
        ServerMessage::Results { standup, .. } => Some(standup),
        _ => None,
    })
    .await;
    assert_eq!(standup.iter().map(|member| member.name.as_str()).collect::<Vec<_>>(), ["Alice"]);

    send(&mut alice, &ClientMessage::ReturnToLobby).await;
    wait_for(&mut alice, |message| {
        matches!(message, ServerMessage::Room { phase: PhaseName::Lobby, .. }).then_some(())
    })
    .await;
}

#[tokio::test]
async fn malformed_messages_are_ignored_and_a_wrong_hash_is_reported() {
    let address = start_server().await;
    let mut alice = connect(&address, "Alice").await;
    claim_and_start(&mut alice).await;
    wait_for(&mut alice, |message| matches!(message, ServerMessage::Round { .. }).then_some(())).await;
    send(&mut alice, &ClientMessage::Ready).await;
    wait_for(&mut alice, |message| matches!(message, ServerMessage::Start { .. }).then_some(())).await;

    alice.send(Message::text("{\"type\":\"shout\"}")).await.unwrap();
    alice.send(Message::binary(vec![1, 2, 3])).await.unwrap();
    send(&mut alice, &ClientMessage::Join { name: "Alice".to_string(), standup_preference: 0.5, seat: None }).await;
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

#[tokio::test]
async fn a_connection_sending_text_messages_faster_than_the_limit_is_closed() {
    let address = start_server().await;
    let mut alice = connect(&address, "Alice").await;
    wait_welcome(&mut alice).await;
    for _ in 0..TEXT_MESSAGE_BURST + TEXT_MESSAGES_PER_S {
        // A wrong password, which the server answers without telling anyone else.
        if alice.send(Message::text("{\"type\":\"claim_host\",\"password\":\"goose\"}")).await.is_err() {
            break;
        }
    }
    let read_to_close = async {
        while let Some(Ok(message)) = alice.next().await
            && !message.is_close()
        {}
    };
    tokio::time::timeout(MESSAGE_TIMEOUT, read_to_close).await.expect("connection closes");
}

#[tokio::test]
async fn a_connection_sending_text_faster_than_the_limit_before_joining_is_closed() {
    let address = start_server().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws")).await.unwrap();
    for _ in 0..TEXT_MESSAGE_BURST + TEXT_MESSAGES_PER_S {
        if socket.send(Message::text("{\"type\":\"start\"}")).await.is_err() {
            break;
        }
    }
    let read_to_close = async {
        while let Some(Ok(message)) = socket.next().await
            && !message.is_close()
        {}
    };
    tokio::time::timeout(MESSAGE_TIMEOUT, read_to_close).await.expect("connection closes");
}

/// The client reads pose frames with its own copy of their layout.
#[test]
fn the_client_reads_pose_records_in_the_servers_layout() {
    let net_ts = include_str!("../../client/src/net.ts");
    assert_eq!(POSE_RECORD_SIZE, 56);
    assert_eq!((POSE_FLAG_HOLDING, POSE_FLAG_CUT), (1, 2));
    for line in ["const POSE_FLOATS = 11;", "const POSE_FLAG_HOLDING = 1 << 0;", "const POSE_FLAG_CUT = 1 << 1;"] {
        assert!(net_ts.contains(line), "net.ts lacks {line:?}");
    }
}

#[test]
fn names_are_trimmed_and_must_be_short_printable_text() {
    assert_eq!(validate_name("  Alice "), Ok("Alice".to_string()));
    assert_eq!(validate_name("   "), Err(NameError::Empty));
    assert_eq!(validate_name(&"x".repeat(25)), Err(NameError::TooLong));
    assert_eq!(validate_name("Al\nice"), Err(NameError::ControlCharacter));
}

#[test]
fn standup_preferences_run_from_0_to_1_inclusive() {
    assert_eq!(validate_standup_preference(0.0), Ok(0.0));
    assert_eq!(validate_standup_preference(1.0), Ok(1.0));
    assert_eq!(validate_standup_preference(-0.01), Err(PreferenceOutOfRange));
    assert_eq!(validate_standup_preference(1.01), Err(PreferenceOutOfRange));
}

/// The name field caps typing at the client's copy of the limit, so a name the server would refuse
/// cannot be typed.
#[test]
fn the_client_caps_names_at_the_servers_limit() {
    let net_ts = include_str!("../../client/src/net.ts");
    assert!(net_ts.contains(&format!("export const MAX_NAME_LEN = {MAX_NAME_LEN};")));
}
