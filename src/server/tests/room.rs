//! The room driven in process by simulated clients, on a made-up clock.
#![expect(
    clippy::cast_possible_truncation,
    clippy::similar_names,
    clippy::unwrap_used,
    reason = "test helpers, which clippy.toml's test allowances miss, and small counts"
)]

use cake_core::cakes::build_slice_missing_cake;
use cake_core::cut::Split;
use cake_core::input::Input;
use cake_core::knife::START_TILT_RAD;
use cake_core::replay::{build_placement_inputs, build_scripted_inputs};
use cake_core::sim::{TICKS_PER_S, World};
use cake_server::catalog::Catalog;
use cake_server::protocol::{Member, POSE_RECORD_SIZE, PhaseName, PlayerId, Run, ServerMessage, Standing, build_pose};
use cake_server::replay::{calc_idle_input, calc_released_input};
use cake_server::room::{ConnectionId, Frame, HOST_PASSWORD, OUTBOX_LEN, Room, RoomConfig, RoomEvent, create_outbox};
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::oneshot;

const CAKE: &str = "slice";
const OTHER_CAKE: &str = "other slice";
const ROUND_TICKS: u32 = 900;
/// 900 ticks at 60 Hz, then the grace for late inputs.
const ROUND_MS: u64 = 15_000;
const GRACE_MS: u64 = 2_000;
const LOAD_TIMEOUT_MS: u64 = 10_000;
/// The real wait for the judging after the last cut, for the tests of it.
const JUDGING_DELAY_MS: u64 = 3_000;
/// An hour, as the owner set it.
const SEAT_HOLD_MS: u64 = 3_600_000;
/// The real judging reveal's lengths, for the tests of it.
const JUDGING_SCENE_MS: u64 = 5_000;
const JUDGING_TAIL_MS: u64 = 1_500;
/// Past the round's end, so a test may send a whole round's inputs at once.
const WHOLE_ROUND_LEAD_TICKS: u32 = 2 * ROUND_TICKS;

struct Client {
    /// The connection the client joined on, which its events name.
    connection: ConnectionId,
    /// The player the room seated it as.
    id: PlayerId,
    inbox: Receiver<Frame>,
}

impl Client {
    /// The text messages received since the last call.
    fn take_messages(&mut self) -> Vec<ServerMessage> {
        self.take_frames()
            .into_iter()
            .filter_map(|frame| match frame {
                Frame::Text(text) => Some(serde_json::from_str(&text).unwrap()),
                Frame::Binary(_) => None,
            })
            .collect()
    }

    /// The `(id, tick)` of every record in the pose frames received since the last call.
    fn take_pose_records(&mut self) -> Vec<(PlayerId, u32)> {
        let mut records = Vec::new();
        for frame in self.take_frames() {
            let Frame::Binary(bytes) = frame else {
                continue;
            };
            for record in bytes.as_chunks::<POSE_RECORD_SIZE>().0 {
                let word = |idx: usize| u32::from_le_bytes(record[4 * idx..4 * idx + 4].try_into().unwrap());
                records.push((word(0), word(1)));
            }
        }
        records
    }

    fn take_frames(&mut self) -> Vec<Frame> {
        std::iter::from_fn(|| self.inbox.try_recv().ok()).collect()
    }
}

/// A room whose judging and results show in the step that finds every player cut, so a test need
/// not wait for them.
fn create_room() -> Room {
    create_room_with(WHOLE_ROUND_LEAD_TICKS, 0)
}

fn create_room_with(max_lead_ticks: u32, judging_delay_ms: u64) -> Room {
    Room::create(
        build_config(max_lead_ticks, judging_delay_ms),
        Catalog::create([(CAKE.to_string(), build_slice_missing_cake())]),
    )
}

/// The room's config, with a judging reveal of no length.
const fn build_config(max_lead_ticks: u32, judging_delay_ms: u64) -> RoomConfig {
    RoomConfig {
        round_ticks: ROUND_TICKS,
        load_timeout_ms: LOAD_TIMEOUT_MS,
        deadline_grace_ms: GRACE_MS,
        max_lead_ticks,
        seat_hold_ms: SEAT_HOLD_MS,
        judging_delay_ms,
        judging_scene_ms: 0,
        judging_tail_ms: 0,
    }
}

/// A room whose judging reveal lasts `JUDGING_SCENE_MS` per player and `JUDGING_TAIL_MS` more.
fn create_judging_room() -> Room {
    let config = RoomConfig {
        judging_scene_ms: JUDGING_SCENE_MS,
        judging_tail_ms: JUDGING_TAIL_MS,
        ..build_config(WHOLE_ROUND_LEAD_TICKS, 0)
    };
    Room::create(config, Catalog::create([(CAKE.to_string(), build_slice_missing_cake())]))
}

/// A room whose catalog holds two cakes, so a pick can tell them apart.
fn create_room_of_two_cakes() -> Room {
    let config = RoomConfig {
        round_ticks: ROUND_TICKS,
        judging_delay_ms: 0,
        judging_scene_ms: 0,
        judging_tail_ms: 0,
        ..RoomConfig::default()
    };
    let cakes = [(CAKE.to_string(), build_slice_missing_cake()), (OTHER_CAKE.to_string(), build_slice_missing_cake())];
    Room::create(config, Catalog::create(cakes))
}

fn join(room: &mut Room, id: PlayerId, name: &str, now_ms: u64) -> Client {
    join_preferring(room, id, name, 0.5, now_ms)
}

fn join_preferring(room: &mut Room, id: PlayerId, name: &str, standup_preference: f64, now_ms: u64) -> Client {
    join_seat(room, id, name, standup_preference, None, now_ms)
}

/// Joins with the fresh `id`, or in the seat `seat` names, as the room assigns.
fn join_seat(
    room: &mut Room,
    id: PlayerId,
    name: &str,
    standup_preference: f64,
    seat: Option<String>,
    now_ms: u64,
) -> Client {
    let (outbox, inbox) = create_outbox();
    let (assigned, mut assigned_id) = oneshot::channel();
    let join = RoomEvent::Join { connection: id, name: name.to_string(), standup_preference, seat, outbox, assigned };
    room.on_event(join, now_ms);
    Client { connection: id, id: assigned_id.try_recv().unwrap(), inbox }
}

/// The round's players as a round message names them.
fn build_members(players: &[(PlayerId, &str)]) -> Vec<Member> {
    players.iter().map(|&(id, name)| Member { id, name: name.to_string() }).collect()
}

/// The seat token in a client's welcome.
fn find_seat(messages: &[ServerMessage]) -> String {
    messages
        .iter()
        .find_map(|message| match message {
            ServerMessage::Welcome { seat, .. } => Some(seat.clone()),
            _ => None,
        })
        .unwrap()
}

fn claim_host(room: &mut Room, client: &Client, password: &str) {
    room.on_event(RoomEvent::ClaimHost { connection: client.connection, password: password.to_string() }, 0);
}

fn pick(room: &mut Room, client: &Client, cake: &str) {
    room.on_event(RoomEvent::Pick { connection: client.connection, cake: cake.to_string() }, 0);
}

/// `host` claims host and starts a round on the placeholder cake, every player loads it, and play
/// starts at `now_ms`.
fn start_round(room: &mut Room, host: &Client, players: &[&Client], now_ms: u64) {
    claim_host(room, host, HOST_PASSWORD);
    pick(room, host, CAKE);
    room.on_event(RoomEvent::Start { connection: host.connection }, now_ms);
    for player in players {
        room.on_event(RoomEvent::Ready { connection: player.connection }, now_ms);
    }
    room.step(now_ms);
}

/// The host the last room message names, or `None` when no room message came.
#[expect(clippy::option_option, reason = "a room may name no host")]
fn find_host(messages: &[ServerMessage]) -> Option<Option<PlayerId>> {
    messages.iter().rev().find_map(|message| match message {
        ServerMessage::Room { host, .. } => Some(*host),
        _ => None,
    })
}

fn find_standup(messages: &[ServerMessage]) -> Option<Vec<PlayerId>> {
    messages.iter().find_map(|message| match message {
        ServerMessage::Results { standup, .. } => Some(standup.iter().map(|member| member.id).collect()),
        _ => None,
    })
}

fn send_inputs(room: &mut Room, client: &Client, inputs: &[Input], now_ms: u64) {
    for (tick, &input) in inputs.iter().enumerate() {
        room.on_event(RoomEvent::Input { connection: client.connection, tick: tick as u32, input }, now_ms);
    }
}

fn send_cut(room: &mut Room, client: &Client, tick: u32, now_ms: u64) {
    room.on_event(RoomEvent::Cut { connection: client.connection, tick }, now_ms);
}

/// The world a client replaying `inputs` sees, before any cut.
fn replay_locally(inputs: &[Input]) -> World {
    let mut world = World::create(build_slice_missing_cake(), START_TILT_RAD, Some(ROUND_TICKS));
    for input in inputs {
        world.step(input);
    }
    world
}

fn cut_locally(inputs: &[Input]) -> Split {
    replay_locally(inputs).cut()
}

fn find_results(messages: &[ServerMessage]) -> Option<Vec<Standing>> {
    messages.iter().find_map(|message| match message {
        ServerMessage::Results { standings, .. } => Some(standings.clone()),
        _ => None,
    })
}

fn find_standing(standings: &[Standing], id: PlayerId) -> &Standing {
    standings.iter().find(|standing| standing.id == id).unwrap()
}

#[test]
fn server_scores_two_players_as_their_own_clients_do() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    let bob = join(&mut room, 2, "Bob", 0);
    start_round(&mut room, &alice, &[&alice, &bob], 0);
    let alice_inputs = build_placement_inputs();
    let bob_inputs = &build_scripted_inputs()[..600];
    send_inputs(&mut room, &alice, &alice_inputs, 100);
    send_cut(&mut room, &alice, 300, 100);
    send_inputs(&mut room, &bob, bob_inputs, 100);
    send_cut(&mut room, &bob, 600, 100);
    room.step(116);

    let standings = find_results(&alice.take_messages()).unwrap();
    let alice_split = cut_locally(&alice_inputs);
    let bob_split = cut_locally(bob_inputs);
    assert_eq!(find_standing(&standings, 1).score, alice_split.calc_score());
    assert_eq!(find_standing(&standings, 1).above_m3, alice_split.above_m3);
    assert_eq!(find_standing(&standings, 2).score, bob_split.calc_score());
    assert_eq!(find_standing(&standings, 2).below_m3, bob_split.below_m3);
}

#[test]
fn cut_is_scored_at_its_tick_and_announced_with_its_pose_but_no_score() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    let bob = join(&mut room, 2, "Bob", 0);
    start_round(&mut room, &alice, &[&alice, &bob], 0);
    let inputs = build_scripted_inputs();
    send_inputs(&mut room, &alice, &inputs[..450], 100);
    send_cut(&mut room, &alice, 450, 100);
    alice.take_messages();
    room.step(116);

    let local = replay_locally(&inputs[..450]);
    let expected = ServerMessage::Cut {
        id: 1,
        tick: 450,
        pose: build_pose(&local),
        cut_lower_m: local.calc_cut_lower_m(),
        cake_touch_lower_m: local.calc_cake_touch_lower_m(),
        is_cut_through: local.is_cut_through(),
    };
    assert_eq!(alice.take_messages(), [expected]);
    send_cut(&mut room, &bob, 0, 200);
    room.step(216);
    let standings = find_results(&alice.take_messages()).unwrap();
    assert_eq!(find_standing(&standings, 1).tick, 450);
    assert_eq!(find_standing(&standings, 1).score, cut_locally(&inputs[..450]).calc_score());
}

#[test]
fn timeout_cuts_at_the_last_tick_dropping_later_inputs_and_holding_a_stalled_players_last_input() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    let bob = join(&mut room, 2, "Bob", 0);
    start_round(&mut room, &alice, &[&alice, &bob], 0);
    let inputs = build_scripted_inputs();
    assert!(inputs.len() < ROUND_TICKS as usize);
    let overlong: Vec<Input> = inputs.iter().copied().cycle().take(ROUND_TICKS as usize + 120).collect();
    send_inputs(&mut room, &alice, &overlong, 100);
    send_inputs(&mut room, &bob, &inputs[..250], 100);
    room.step(ROUND_MS + GRACE_MS - 1);
    assert_eq!(find_results(&alice.take_messages()), None);
    room.step(ROUND_MS + GRACE_MS);

    let standings = find_results(&alice.take_messages()).unwrap();
    let alice_split = cut_locally(&overlong[..ROUND_TICKS as usize]);
    let mut bob_held = inputs[..250].to_vec();
    bob_held.resize(ROUND_TICKS as usize, inputs[249]);
    let bob_split = cut_locally(&bob_held);
    assert_eq!(find_standing(&standings, 1).tick, ROUND_TICKS);
    assert_eq!(find_standing(&standings, 1).score, alice_split.calc_score());
    assert_eq!(find_standing(&standings, 2).tick, ROUND_TICKS);
    assert_eq!(find_standing(&standings, 2).score, bob_split.calc_score());
}

#[test]
fn results_wait_for_the_last_cut() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    let bob = join(&mut room, 2, "Bob", 0);
    start_round(&mut room, &alice, &[&alice, &bob], 0);
    send_inputs(&mut room, &alice, &build_placement_inputs(), 100);
    send_cut(&mut room, &alice, 300, 100);
    room.step(116);
    assert_eq!(find_results(&alice.take_messages()), None);

    send_inputs(&mut room, &bob, &build_placement_inputs(), 200);
    send_cut(&mut room, &bob, 300, 200);
    room.step(216);
    let messages = alice.take_messages();
    assert_eq!(find_results(&messages).map(|standings| standings.len()), Some(2));
    assert!(messages.contains(&ServerMessage::Room {
        phase: PhaseName::Results,
        host: Some(1),
        members: vec![Member { id: 1, name: "Alice".into() }, Member { id: 2, name: "Bob".into() }],
    }));
}

/// The judging message's `order`, `scene_ms`, `elapsed_ms`, and `standings`, if one came.
type Judging = (Vec<PlayerId>, u64, u64, Vec<Standing>);

fn find_judging(messages: &[ServerMessage]) -> Option<Judging> {
    messages.iter().find_map(|message| match message {
        ServerMessage::Judging { order, scene_ms, elapsed_ms, standings } => {
            Some((order.clone(), *scene_ms, *elapsed_ms, standings.clone()))
        }
        _ => None,
    })
}

/// Alice, Bob, and Carol play a round on the judging room, all cutting by its step at 116 ms,
/// which starts the judging.
fn judge_three(room: &mut Room) -> [Client; 3] {
    let alice = join(room, 1, "Alice", 0);
    let bob = join(room, 2, "Bob", 0);
    let carol = join(room, 3, "Carol", 0);
    start_round(room, &alice, &[&alice, &bob, &carol], 0);
    send_inputs(room, &alice, &build_placement_inputs(), 100);
    send_cut(room, &alice, 300, 100);
    send_cut(room, &bob, 0, 100);
    send_cut(room, &carol, 0, 100);
    room.step(116);
    [alice, bob, carol]
}

#[test]
fn judging_shows_every_page_each_player_once_in_one_order_then_the_results_after_every_scene_and_the_tail() {
    let mut room = create_judging_room();
    let [mut alice, mut bob, mut carol] = judge_three(&mut room);

    let alice_messages = alice.take_messages();
    let (order, scene_ms, elapsed_ms, standings) = find_judging(&alice_messages).unwrap();
    let mut judged = order.clone();
    judged.sort_unstable();
    assert_eq!(judged, [1, 2, 3]);
    assert_eq!((scene_ms, elapsed_ms), (JUDGING_SCENE_MS, 0));
    assert_eq!(find_standing(&standings, 1).score, cut_locally(&build_placement_inputs()).calc_score());
    assert_eq!(find_results(&alice_messages), None);
    assert!(
        alice_messages.iter().any(|message| matches!(message, ServerMessage::Room { phase: PhaseName::Judging, .. }))
    );
    let judging = Some((order, scene_ms, elapsed_ms, standings.clone()));
    assert_eq!(find_judging(&bob.take_messages()), judging);
    assert_eq!(find_judging(&carol.take_messages()), judging);

    let results_ms = 116 + 3 * JUDGING_SCENE_MS + JUDGING_TAIL_MS;
    room.step(results_ms - 1);
    assert_eq!(find_results(&alice.take_messages()), None);
    room.step(results_ms);
    assert_eq!(find_results(&alice.take_messages()), Some(standings));
}

#[test]
fn a_page_joining_during_the_judging_joins_the_reveal_where_the_others_are() {
    let mut room = create_judging_room();
    let [mut alice, ..] = judge_three(&mut room);
    let (order, ..) = find_judging(&alice.take_messages()).unwrap();

    let mut dave = join(&mut room, 4, "Dave", 4_116);
    let messages = dave.take_messages();
    let cut_ids: Vec<PlayerId> = messages
        .iter()
        .filter_map(|message| match message {
            ServerMessage::Cut { id, .. } => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(cut_ids, [1, 2, 3]);
    let (late_order, _, elapsed_ms, _) = find_judging(&messages).unwrap();
    assert_eq!((late_order, elapsed_ms), (order, 4_000));
}

#[test]
fn no_one_can_end_the_judging_early_or_claim_host_during_it() {
    let mut room = create_judging_room();
    let [mut alice, ..] = judge_three(&mut room);
    alice.take_messages();

    room.on_event(RoomEvent::ReturnToLobby { connection: alice.connection }, 1_000);
    claim_host(&mut room, &alice, HOST_PASSWORD);
    let messages = alice.take_messages();
    assert_eq!(messages.iter().filter(|message| matches!(message, ServerMessage::Error { .. })).count(), 2);
    assert!(!messages.iter().any(|message| matches!(message, ServerMessage::Room { .. })));
}

#[test]
fn a_judging_whose_last_page_left_gives_way_to_the_lobby() {
    let mut room = create_judging_room();
    let alice = join(&mut room, 1, "Alice", 0);
    start_round(&mut room, &alice, &[&alice], 0);
    send_cut(&mut room, &alice, 0, 100);
    room.step(116);
    room.on_event(RoomEvent::Leave { connection: alice.connection }, 200);

    let mut bob = join(&mut room, 2, "Bob", 300);
    let messages = bob.take_messages();
    assert!(messages.iter().any(|message| matches!(message, ServerMessage::Room { phase: PhaseName::Lobby, .. })));
    assert_eq!(find_judging(&messages), None);
}

#[test]
fn late_joiner_spectates_with_the_round_state_and_poses_then_plays_the_next_round() {
    let mut room = create_room();
    let alice = join(&mut room, 1, "Alice", 0);
    let bob = join(&mut room, 2, "Bob", 0);
    start_round(&mut room, &alice, &[&alice, &bob], 0);
    send_inputs(&mut room, &alice, &build_placement_inputs(), 100);
    send_cut(&mut room, &alice, 300, 100);
    send_inputs(&mut room, &bob, &build_placement_inputs()[..200], 100);
    room.step(116);

    let mut carol = join(&mut room, 3, "Carol", 5_000);
    let messages = carol.take_messages();
    assert!(matches!(messages[0], ServerMessage::Welcome { id: 3, .. }));
    assert!(messages.contains(&ServerMessage::Round {
        cake: CAKE.into(),
        round_ticks: ROUND_TICKS,
        players: build_members(&[(1, "Alice"), (2, "Bob")])
    }));
    assert!(messages.contains(&ServerMessage::Start { elapsed_ms: 5_000 }));
    assert!(messages.iter().any(|message| matches!(message, ServerMessage::Cut { id: 1, tick: 300, .. })));
    room.step(5_016);
    room.step(5_033);
    assert_eq!(carol.take_pose_records(), [(1, 300), (2, 200)]);

    send_inputs(&mut room, &carol, &build_placement_inputs(), 5_100);
    send_cut(&mut room, &bob, 200, 5_100);
    room.step(5_116);
    assert_eq!(find_results(&carol.take_messages()).map(|standings| standings.len()), Some(2));
    room.on_event(RoomEvent::ReturnToLobby { connection: 1 }, 11_000);
    room.on_event(RoomEvent::Start { connection: 1 }, 11_000);
    assert!(carol.take_messages().contains(&ServerMessage::Round {
        cake: CAKE.into(),
        round_ticks: ROUND_TICKS,
        players: build_members(&[(1, "Alice"), (2, "Bob"), (3, "Carol")])
    }));
}

#[test]
fn no_one_hosts_until_a_player_claims_host_with_the_password() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    let mut bob = join(&mut room, 2, "Bob", 0);
    assert_eq!(find_host(&alice.take_messages()), Some(None));
    room.on_event(RoomEvent::Start { connection: alice.connection }, 0);
    assert!(matches!(alice.take_messages()[..], [ServerMessage::Error { .. }]));

    bob.take_messages();
    claim_host(&mut room, &bob, "goose");
    assert_eq!(bob.take_messages(), [ServerMessage::ClaimRefused]);
    assert_eq!(alice.take_messages(), []);
    claim_host(&mut room, &bob, HOST_PASSWORD);
    assert_eq!(find_host(&alice.take_messages()), Some(Some(2)));
}

#[test]
fn a_claim_takes_host_from_a_host_still_present() {
    let mut room = create_room();
    let alice = join(&mut room, 1, "Alice", 0);
    let mut bob = join(&mut room, 2, "Bob", 0);
    claim_host(&mut room, &alice, HOST_PASSWORD);
    claim_host(&mut room, &bob, HOST_PASSWORD);
    assert_eq!(find_host(&bob.take_messages()), Some(Some(2)));

    room.on_event(RoomEvent::Start { connection: alice.connection }, 0);
    room.on_event(RoomEvent::Start { connection: bob.connection }, 0);
    assert!(bob.take_messages().contains(&ServerMessage::Round {
        cake: CAKE.into(),
        round_ticks: ROUND_TICKS,
        players: build_members(&[(1, "Alice"), (2, "Bob")])
    }));
}

#[test]
fn the_host_seat_stays_vacant_after_the_host_leaves_until_someone_claims_it() {
    let mut room = create_room();
    let alice = join(&mut room, 1, "Alice", 0);
    let mut bob = join(&mut room, 2, "Bob", 0);
    let carol = join(&mut room, 3, "Carol", 0);
    claim_host(&mut room, &alice, HOST_PASSWORD);
    room.on_event(RoomEvent::Leave { connection: alice.connection }, 100);
    let expected_room = ServerMessage::Room {
        phase: PhaseName::Lobby,
        host: None,
        members: vec![Member { id: 2, name: "Bob".into() }, Member { id: 3, name: "Carol".into() }],
    };
    assert_eq!(bob.take_messages().last(), Some(&expected_room));

    claim_host(&mut room, &carol, HOST_PASSWORD);
    assert_eq!(find_host(&bob.take_messages()), Some(Some(3)));
}

#[test]
fn host_cannot_be_claimed_during_a_round() {
    let mut room = create_room();
    let alice = join(&mut room, 1, "Alice", 0);
    let mut bob = join(&mut room, 2, "Bob", 0);
    start_round(&mut room, &alice, &[&alice, &bob], 0);
    bob.take_messages();
    claim_host(&mut room, &bob, HOST_PASSWORD);
    assert!(matches!(bob.take_messages()[..], [ServerMessage::Error { .. }]));
}

#[test]
fn a_page_joining_the_lobby_learns_the_pick_and_every_page_hears_the_hosts_next_pick() {
    let mut room = create_room_of_two_cakes();
    let alice = join(&mut room, 1, "Alice", 0);
    let mut bob = join(&mut room, 2, "Bob", 0);
    let joined_pick = bob.take_messages().into_iter().find(|message| matches!(message, ServerMessage::Pick { .. }));
    assert!(matches!(joined_pick, Some(ServerMessage::Pick { is_random: true, .. })));

    claim_host(&mut room, &alice, HOST_PASSWORD);
    pick(&mut room, &alice, OTHER_CAKE);
    assert_eq!(bob.take_messages().last(), Some(&ServerMessage::Pick { cake: OTHER_CAKE.into(), is_random: false }));
    room.on_event(RoomEvent::Start { connection: alice.connection }, 0);
    assert!(bob.take_messages().contains(&ServerMessage::Round {
        cake: OTHER_CAKE.into(),
        round_ticks: ROUND_TICKS,
        players: build_members(&[(1, "Alice"), (2, "Bob")])
    }));
}

#[test]
fn picking_random_settles_on_a_catalog_cake_at_once_which_the_start_plays() {
    let mut room = create_room_of_two_cakes();
    let mut alice = join(&mut room, 1, "Alice", 0);
    claim_host(&mut room, &alice, HOST_PASSWORD);
    alice.take_messages();
    pick(&mut room, &alice, "random");
    let Some(ServerMessage::Pick { cake, is_random: true }) = alice.take_messages().pop() else {
        panic!("a random pick is announced");
    };
    assert!([CAKE, OTHER_CAKE].contains(&cake.as_str()));

    room.on_event(RoomEvent::Start { connection: alice.connection }, 0);
    let players = build_members(&[(1, "Alice")]);
    assert!(alice.take_messages().contains(&ServerMessage::Round { cake, round_ticks: ROUND_TICKS, players }));
}

#[test]
fn a_pick_is_refused_from_anyone_but_the_host_and_for_an_unknown_cake() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    let mut bob = join(&mut room, 2, "Bob", 0);
    claim_host(&mut room, &alice, HOST_PASSWORD);
    bob.take_messages();
    pick(&mut room, &bob, CAKE);
    assert!(matches!(bob.take_messages()[..], [ServerMessage::Error { .. }]));
    pick(&mut room, &alice, "teapot");
    assert_eq!(find_error(&alice.take_messages()), Some("no cake \"teapot\""));
    assert_eq!(bob.take_messages(), []);
}

#[test]
fn results_stay_until_the_host_returns_everyone_to_a_lobby_preset_to_random() {
    let mut room = create_room();
    let alice = join(&mut room, 1, "Alice", 0);
    let mut bob = join(&mut room, 2, "Bob", 0);
    start_round(&mut room, &alice, &[&alice, &bob], 0);
    bob.take_messages();
    room.on_event(RoomEvent::Cut { connection: alice.connection, tick: 0 }, 100);
    room.on_event(RoomEvent::Cut { connection: bob.connection, tick: 0 }, 100);
    room.step(116);
    room.step(1_000_000);
    room.on_event(RoomEvent::ReturnToLobby { connection: bob.connection }, 1_000_000);
    let messages = bob.take_messages();
    assert!(find_results(&messages).is_some());
    assert!(matches!(messages.last(), Some(ServerMessage::Error { .. })));
    assert!(!messages.iter().any(|message| matches!(message, ServerMessage::Room { phase: PhaseName::Lobby, .. })));

    room.on_event(RoomEvent::ReturnToLobby { connection: alice.connection }, 1_000_000);
    let messages = bob.take_messages();
    assert!(matches!(
        messages[..],
        [ServerMessage::Room { phase: PhaseName::Lobby, .. }, ServerMessage::Pick { is_random: true, .. }]
    ));
}

#[test]
fn standup_order_seats_players_best_first_by_preference_then_spectators() {
    let mut room = create_room();
    let mut alice = join_preferring(&mut room, 1, "Alice", 1.0, 0);
    let bob = join_preferring(&mut room, 2, "Bob", 1.0, 0);
    start_round(&mut room, &alice, &[&alice, &bob], 0);
    let _carol = join_preferring(&mut room, 3, "Carol", 1.0, 100);
    send_inputs(&mut room, &alice, &build_placement_inputs(), 100);
    send_cut(&mut room, &alice, 300, 100);
    send_cut(&mut room, &bob, 0, 100);
    room.step(116);

    let messages = alice.take_messages();
    let standings = find_results(&messages).unwrap();
    let ranks: Vec<(PlayerId, u32)> = standings.iter().map(|standing| (standing.id, standing.rank)).collect();
    assert_eq!(ranks, [(1, 1), (2, 2)]);
    // All three want to go last: Alice, first to choose, gets the last slot, Bob the one before,
    // and Carol, a spectator, the one left.
    assert_eq!(find_standup(&messages), Some(vec![3, 2, 1]));
}

#[test]
fn identical_cuts_rank_the_earlier_first_and_untouched_players_share_a_rank() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    let bob = join(&mut room, 2, "Bob", 0);
    let carol = join(&mut room, 3, "Carol", 0);
    let dave = join(&mut room, 4, "Dave", 0);
    let erin = join(&mut room, 5, "Erin", 0);
    start_round(&mut room, &alice, &[&alice, &bob, &carol, &dave, &erin], 0);
    // The placed cake has settled by tick 600 and the knife holds still, so later cuts match.
    let mut settled = build_placement_inputs();
    settled.resize(700, settled[299]);
    send_inputs(&mut room, &alice, &settled[..600], 100);
    send_cut(&mut room, &alice, 600, 100);
    send_inputs(&mut room, &bob, &settled, 100);
    send_cut(&mut room, &bob, 700, 100);
    send_inputs(&mut room, &carol, &settled[..600], 100);
    send_cut(&mut room, &carol, 600, 100);
    room.step(ROUND_MS + GRACE_MS);

    let standings = find_results(&alice.take_messages()).unwrap();
    let alice_standing = find_standing(&standings, 1);
    assert_eq!(find_standing(&standings, 2).score, alice_standing.score);
    let ranks: Vec<(PlayerId, u32, bool)> = standings.iter().map(|s| (s.id, s.rank, s.has_touched)).collect();
    assert_eq!(ranks, [(1, 1, true), (3, 1, true), (2, 3, true), (4, 4, false), (5, 4, false)]);
}

fn find_error(messages: &[ServerMessage]) -> Option<&str> {
    messages.iter().find_map(|message| match message {
        ServerMessage::Error { message } => Some(message.as_str()),
        _ => None,
    })
}

#[test]
fn start_and_pick_are_refused_while_a_round_is_on() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    claim_host(&mut room, &alice, HOST_PASSWORD);
    room.on_event(RoomEvent::Start { connection: 1 }, 0);
    alice.take_messages();
    room.on_event(RoomEvent::Start { connection: 1 }, 100);
    pick(&mut room, &alice, CAKE);
    let refusal = ServerMessage::Error { message: "a round is already on".into() };
    assert_eq!(alice.take_messages(), [refusal.clone(), refusal]);
}

#[test]
fn a_page_that_has_not_loaded_the_cake_by_the_load_timeout_sits_the_round_out_as_a_spectator() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    let mut bob = join(&mut room, 2, "Bob", 0);
    claim_host(&mut room, &alice, HOST_PASSWORD);
    room.on_event(RoomEvent::Start { connection: 1 }, 0);
    room.on_event(RoomEvent::Ready { connection: 1 }, 100);
    room.step(LOAD_TIMEOUT_MS - 1);
    assert!(!alice.take_messages().contains(&ServerMessage::Start { elapsed_ms: 0 }));

    bob.take_messages();
    room.step(LOAD_TIMEOUT_MS);
    let messages = bob.take_messages();
    let sit_out_idx = messages.iter().position(|message| *message == ServerMessage::SitOut { players: vec![2] });
    let start_idx = messages.iter().position(|message| *message == ServerMessage::Start { elapsed_ms: 0 });
    assert!(sit_out_idx.unwrap() < start_idx.unwrap());
    // Bob's page loads the cake at last, and its ready and inputs reach no world.
    room.on_event(RoomEvent::Ready { connection: bob.connection }, LOAD_TIMEOUT_MS + 100);
    send_inputs(&mut room, &bob, &build_placement_inputs()[..10], LOAD_TIMEOUT_MS + 100);
    send_cut(&mut room, &alice, 0, LOAD_TIMEOUT_MS + 100);
    room.step(LOAD_TIMEOUT_MS + 116);
    let messages = bob.take_messages();
    let standings = find_results(&messages).unwrap();
    assert_eq!(standings.iter().map(|standing| standing.id).collect::<Vec<_>>(), [1]);
    assert_eq!(find_standup(&messages), Some(vec![1, 2]));
}

#[test]
fn a_round_no_page_loaded_in_time_is_called_off_back_to_the_lobby() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    claim_host(&mut room, &alice, HOST_PASSWORD);
    room.on_event(RoomEvent::Start { connection: alice.connection }, 0);
    alice.take_messages();
    room.step(LOAD_TIMEOUT_MS);
    let messages = alice.take_messages();
    assert_eq!(messages[0], ServerMessage::SitOut { players: vec![1] });
    assert!(matches!(messages[1], ServerMessage::Room { phase: PhaseName::Lobby, .. }));
    assert!(matches!(messages[2], ServerMessage::Pick { is_random: true, .. }));
    assert_eq!(messages.len(), 3);
}

/// The inputs a player sent, then their last one released, held to the round's end.
fn build_left_inputs(sent: &[Input]) -> Vec<Input> {
    let mut inputs = sent.to_vec();
    inputs.resize(ROUND_TICKS as usize, calc_released_input(*sent.last().unwrap()));
    inputs
}

/// The inputs a resume message's runs step, one per tick.
fn expand_runs(runs: &[Run]) -> Vec<Input> {
    runs.iter().flat_map(|run| std::iter::repeat_n(Input::decode(&run.input).unwrap(), run.ticks as usize)).collect()
}

#[test]
fn a_player_who_leaves_mid_round_lets_go_and_is_cut_and_ranked_at_the_deadline() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    let bob = join(&mut room, 2, "Bob", 0);
    start_round(&mut room, &alice, &[&alice, &bob], 0);
    send_inputs(&mut room, &alice, &build_placement_inputs(), 100);
    send_cut(&mut room, &alice, 300, 100);
    // Bob leaves mid-carry, holding the cake.
    let bob_inputs = &build_placement_inputs()[..200];
    send_inputs(&mut room, &bob, bob_inputs, 100);
    room.step(116);
    room.on_event(RoomEvent::Leave { connection: bob.connection }, 200);
    room.step(216);
    assert_eq!(find_results(&alice.take_messages()), None);

    room.step(ROUND_MS + GRACE_MS);
    let standings = find_results(&alice.take_messages()).unwrap();
    let bob_standing = find_standing(&standings, 2);
    assert_eq!((bob_standing.name.as_str(), bob_standing.tick), ("Bob", ROUND_TICKS));
    assert_eq!(bob_standing.score, cut_locally(&build_left_inputs(bob_inputs)).calc_score());
}

#[test]
fn a_leaver_is_cut_as_the_round_ends_and_the_judging_waits_for_every_tile_to_play_the_last_cut() {
    let mut room = create_room_with(WHOLE_ROUND_LEAD_TICKS, JUDGING_DELAY_MS);
    let mut alice = join(&mut room, 1, "Alice", 0);
    let bob = join(&mut room, 2, "Bob", 0);
    start_round(&mut room, &alice, &[&alice, &bob], 0);
    send_inputs(&mut room, &alice, &build_placement_inputs(), 100);
    send_cut(&mut room, &alice, 300, 100);
    send_inputs(&mut room, &bob, &build_placement_inputs()[..200], 100);
    room.step(116);
    room.on_event(RoomEvent::Leave { connection: bob.connection }, 200);
    // The round's clock reads tick 898 here, and 900, its last, at ROUND_MS.
    room.step(ROUND_MS - 17);
    let before_end = alice.take_messages();
    assert!(!before_end.iter().any(|message| matches!(message, ServerMessage::Cut { id: 2, .. })));

    room.step(ROUND_MS);
    let at_end = alice.take_messages();
    assert!(at_end.iter().any(|message| matches!(message, ServerMessage::Cut { id: 2, tick: ROUND_TICKS, .. })));
    assert!(at_end.contains(&ServerMessage::PlayOver));
    assert_eq!(find_judging(&at_end), None);
    room.step(ROUND_MS + JUDGING_DELAY_MS - 1);
    assert_eq!(find_judging(&alice.take_messages()), None);
    room.step(ROUND_MS + JUDGING_DELAY_MS);
    assert!(find_judging(&alice.take_messages()).is_some());
}

#[test]
fn a_silent_players_world_trails_the_round_clock_by_the_grace_holding_their_last_input() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    let bob = join(&mut room, 2, "Bob", 0);
    start_round(&mut room, &alice, &[&alice, &bob], 0);
    let sent = &build_placement_inputs()[..100];
    send_inputs(&mut room, &alice, sent, 100);
    alice.take_frames();
    // At 5016 ms the round's clock reads tick 300, and the grace is 120 ticks.
    room.step(5_000);
    room.step(5_016);
    assert_eq!(alice.take_pose_records(), [(1, 180), (2, 180)]);

    // Inputs for ticks the server already held arrive too late to count.
    let late: Vec<Input> = build_placement_inputs()[..180].to_vec();
    send_inputs(&mut room, &alice, &late, 5_100);
    room.step(ROUND_MS + GRACE_MS);
    let standings = find_results(&alice.take_messages()).unwrap();
    let mut held = sent.to_vec();
    held.resize(ROUND_TICKS as usize, sent[99]);
    assert_eq!(find_standing(&standings, 1).score, cut_locally(&held).calc_score());
}

/// A page silent past the grace that then sends its inputs and its cut at once, as a page whose
/// network stalled while its player cut does, has its cut scored on the world the server held.
#[test]
fn inputs_and_a_cut_arriving_in_one_burst_past_the_grace_are_scored_on_the_world_held_meanwhile() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    start_round(&mut room, &alice, &[&alice], 0);
    // At 2500 ms the round's clock reads tick 150, and the grace is 120 ticks.
    room.step(2_500);
    let inputs = build_placement_inputs();
    send_inputs(&mut room, &alice, &inputs, 2_500);
    send_cut(&mut room, &alice, 300, 2_500);
    room.step(2_516);

    let messages = alice.take_messages();
    let mut held = vec![calc_idle_input(); 30];
    held.extend_from_slice(&inputs[30..]);
    let score = find_standing(&find_results(&messages).unwrap(), 1).score;
    assert_eq!(score, cut_locally(&held).calc_score());
    assert_ne!(score, cut_locally(&inputs).calc_score());
}

#[test]
fn a_page_whose_late_inputs_parted_its_world_gets_the_servers_steps_once_and_plays_on_in_step() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    start_round(&mut room, &alice, &[&alice], 0);
    let inputs = build_placement_inputs();
    send_inputs(&mut room, &alice, &inputs[..100], 100);
    // At 5016 ms the round's clock reads tick 300, and the grace is 120 ticks.
    room.step(5_000);
    room.step(5_016);
    alice.take_messages();

    // The page's inputs for ticks the server held with its last input arrive, and differ.
    for tick in 100..180 {
        room.on_event(RoomEvent::Input { connection: 1, tick, input: inputs[tick as usize] }, 5_100);
    }
    room.step(5_116);
    let resumes: Vec<Vec<Input>> = alice
        .take_messages()
        .into_iter()
        .filter_map(|message| match message {
            ServerMessage::Resume { runs, cut_tick: None } => Some(expand_runs(&runs)),
            _ => None,
        })
        .collect();
    // The clock reads tick 306 at 5116 ms, so the world was held to tick 186.
    let mut expected = inputs[..100].to_vec();
    expected.resize(186, inputs[99]);
    assert_eq!(resumes, [expected.clone()]);

    // More of the parted world's inputs arrive, then the rebuilt world's, which play on in step.
    for tick in 180..186 {
        room.on_event(RoomEvent::Input { connection: 1, tick, input: inputs[tick as usize] }, 5_200);
    }
    let rebuilt_inputs = [expected, inputs[186..240].to_vec()].concat();
    for (tick, &input) in rebuilt_inputs.iter().enumerate().skip(186) {
        room.on_event(RoomEvent::Input { connection: 1, tick: tick as u32, input }, 5_200);
    }
    let hash = replay_locally(&rebuilt_inputs).calc_hash();
    room.on_event(RoomEvent::Hash { connection: 1, tick: 240, hash }, 5_200);
    room.step(5_216);
    let messages = alice.take_messages();
    assert!(
        !messages.iter().any(|message| matches!(message, ServerMessage::Resume { .. } | ServerMessage::Desync { .. }))
    );
}

const NO_RESUMES: [usize; 0] = [];

/// The ticks each resume message received since the last call steps.
fn take_resume_ticks(client: &mut Client) -> Vec<usize> {
    let messages = client.take_messages();
    messages
        .iter()
        .filter_map(|message| match message {
            ServerMessage::Resume { runs, .. } => Some(expand_runs(runs).len()),
            _ => None,
        })
        .collect()
}

/// A page whose network stays slow is resynced at most once a second, and a parting within that
/// second is resynced as it ends, even if nothing parts the world after it.
#[test]
fn a_page_whose_world_parts_again_within_a_second_of_its_resync_is_resynced_as_the_second_ends() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    start_round(&mut room, &alice, &[&alice], 0);
    let inputs = build_placement_inputs();
    // The world is held to tick 60, so every carry input after the first parts it.
    room.step(3_000);
    send_inputs(&mut room, &alice, &inputs[..60], 3_000);
    room.step(3_016);
    assert_eq!(take_resume_ticks(&mut alice), [60]);

    // Half a second later the world held on 30 ticks, and an input for one of them parts it again.
    room.step(3_516);
    room.on_event(RoomEvent::Input { connection: 1, tick: 70, input: inputs[70] }, 3_516);
    room.step(3_533);
    assert_eq!(take_resume_ticks(&mut alice), NO_RESUMES);

    // The world reaches tick 120, a second of ticks past the resync, at 4000 ms.
    room.step(3_983);
    assert_eq!(take_resume_ticks(&mut alice), NO_RESUMES);
    room.step(4_000);
    assert_eq!(take_resume_ticks(&mut alice), [120]);
    room.step(5_000);
    assert_eq!(take_resume_ticks(&mut alice), NO_RESUMES);
}

#[test]
fn a_stale_departure_from_a_dropped_connection_does_not_unseat_the_page_reopened_into_its_seat() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    let mut bob = join(&mut room, 2, "Bob", 0);
    let seat = find_seat(&alice.take_messages());
    start_round(&mut room, &bob, &[&alice, &bob], 0);
    // Alice's first connection stops reading until the room disconnects it.
    let mut now_ms = 0;
    while !alice.inbox.is_closed() {
        now_ms += 16;
        room.step(now_ms);
        bob.take_frames();
    }
    let mut reopened = join_seat(&mut room, 7, "Alice", 0.5, Some(seat), now_ms);
    assert_eq!(reopened.id, 1);
    reopened.take_messages();
    bob.take_messages();

    // The first connection's socket closes at last, after a cut its page sent before it stalled.
    send_cut(&mut room, &alice, 0, now_ms);
    room.on_event(RoomEvent::Leave { connection: alice.connection }, now_ms);
    room.step(now_ms + 16);
    assert!(
        !bob.take_messages()
            .iter()
            .any(|message| matches!(message, ServerMessage::Room { .. } | ServerMessage::Cut { .. }))
    );
    let cut_tick = calc_round_tick(now_ms + 16);
    send_cut(&mut room, &reopened, cut_tick, now_ms + 16);
    room.step(now_ms + 32);
    let messages = bob.take_messages();
    assert!(messages.iter().any(|message| matches!(message, ServerMessage::Cut { id: 1, .. })));
}

/// The round's clock at `now_ms`, for a round started at 0.
fn calc_round_tick(now_ms: u64) -> u32 {
    (now_ms * u64::from(TICKS_PER_S) / 1000) as u32
}

#[test]
fn a_late_joiner_gets_the_cut_of_a_player_who_left_since_they_stay_in_the_round() {
    let mut room = create_room();
    let alice = join(&mut room, 1, "Alice", 0);
    let bob = join(&mut room, 2, "Bob", 0);
    start_round(&mut room, &alice, &[&alice, &bob], 0);
    send_inputs(&mut room, &alice, &build_placement_inputs(), 100);
    send_cut(&mut room, &alice, 300, 100);
    room.step(116);
    room.on_event(RoomEvent::Leave { connection: alice.connection }, 200);

    let mut carol = join(&mut room, 3, "Carol", 300);
    let messages = carol.take_messages();
    assert!(messages.contains(&ServerMessage::Round {
        cake: CAKE.into(),
        round_ticks: ROUND_TICKS,
        players: build_members(&[(1, "Alice"), (2, "Bob")])
    }));
    assert!(messages.iter().any(|message| matches!(message, ServerMessage::Cut { id: 1, tick: 300, .. })));
}

#[test]
fn a_page_that_leaves_while_the_round_loads_does_not_hold_up_its_start() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    let bob = join(&mut room, 2, "Bob", 0);
    claim_host(&mut room, &alice, HOST_PASSWORD);
    room.on_event(RoomEvent::Start { connection: alice.connection }, 0);
    room.on_event(RoomEvent::Leave { connection: bob.connection }, 100);
    room.on_event(RoomEvent::Ready { connection: alice.connection }, 100);
    room.step(116);
    assert!(alice.take_messages().contains(&ServerMessage::Start { elapsed_ms: 0 }));
}

#[test]
fn a_reopened_page_takes_its_seat_mid_round_with_every_tick_its_world_stepped() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    let bob = join(&mut room, 2, "Bob", 0);
    let seat = find_seat(&alice.take_messages());
    start_round(&mut room, &alice, &[&alice, &bob], 0);
    let sent = &build_placement_inputs()[..200];
    send_inputs(&mut room, &alice, sent, 100);
    room.step(116);
    room.on_event(RoomEvent::Leave { connection: alice.connection }, 200);
    room.step(216);

    let mut reopened = join_seat(&mut room, 7, "Alice", 0.5, Some(seat.clone()), 5_000);
    assert_eq!(reopened.id, 1);
    let messages = reopened.take_messages();
    assert_eq!(messages[0], ServerMessage::Welcome { id: 1, seat });
    assert!(messages.contains(&ServerMessage::Start { elapsed_ms: 5_000 }));
    let Some(ServerMessage::Resume { runs, cut_tick: None }) =
        messages.iter().find(|message| matches!(message, ServerMessage::Resume { .. }))
    else {
        panic!("a reopened page in the round is told its world's steps");
    };
    // What was sent, then the released last input the server stepped once the page had gone.
    let mut expected = sent.to_vec();
    expected.push(calc_released_input(sent[199]));
    assert_eq!(expand_runs(runs), expected);
}

#[test]
fn a_page_reopened_while_the_round_loads_plays_its_first_tick_as_sent() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    let bob = join(&mut room, 2, "Bob", 0);
    let seat = find_seat(&alice.take_messages());
    claim_host(&mut room, &alice, HOST_PASSWORD);
    room.on_event(RoomEvent::Start { connection: alice.connection }, 0);
    room.on_event(RoomEvent::Leave { connection: alice.connection }, 100);
    let mut alice = join_seat(&mut room, 7, "Alice", 0.5, Some(seat), 2_000);
    room.on_event(RoomEvent::Ready { connection: alice.connection }, 3_000);
    room.on_event(RoomEvent::Ready { connection: bob.connection }, 3_000);
    room.step(3_000);
    alice.take_messages();

    // The stand turned on the first tick, which the page's world steps with.
    let inputs = [Input { stand_angle: 10_000, ..calc_idle_input() }];
    send_inputs(&mut room, &alice, &inputs, 3_100);
    room.on_event(
        RoomEvent::Hash { connection: alice.connection, tick: 1, hash: replay_locally(&inputs).calc_hash() },
        3_100,
    );
    room.step(3_116);
    assert!(!alice.take_messages().iter().any(|message| matches!(message, ServerMessage::Desync { .. })));
}

#[test]
fn a_page_reopened_while_the_round_loads_must_load_the_cake_again_or_sit_out() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    let bob = join(&mut room, 2, "Bob", 0);
    let seat = find_seat(&alice.take_messages());
    claim_host(&mut room, &alice, HOST_PASSWORD);
    room.on_event(RoomEvent::Start { connection: alice.connection }, 0);
    room.on_event(RoomEvent::Leave { connection: alice.connection }, 100);
    let mut alice = join_seat(&mut room, 7, "Alice", 0.5, Some(seat), 2_000);
    room.on_event(RoomEvent::Ready { connection: bob.connection }, 3_000);
    room.step(3_000);
    assert!(!alice.take_messages().contains(&ServerMessage::Start { elapsed_ms: 0 }));

    room.step(LOAD_TIMEOUT_MS);
    let messages = alice.take_messages();
    assert!(messages.contains(&ServerMessage::SitOut { players: vec![1] }));
    assert!(messages.contains(&ServerMessage::Start { elapsed_ms: 0 }));
}

#[test]
fn a_seat_is_not_reopened_while_its_page_is_connected_or_after_an_hour_without_activity() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    let seat = find_seat(&alice.take_messages());
    let second_tab = join_seat(&mut room, 2, "Alice", 0.5, Some(seat.clone()), 100);
    assert_eq!(second_tab.id, 2);

    room.on_event(RoomEvent::Leave { connection: alice.connection }, 200);
    room.step(SEAT_HOLD_MS + 1);
    let late = join_seat(&mut room, 3, "Alice", 0.5, Some(seat), SEAT_HOLD_MS + 2);
    assert_eq!(late.id, 3);
}

#[test]
fn an_input_or_cut_stamped_beyond_the_lead_over_the_round_clock_is_refused() {
    let mut room = create_room_with(30, 0);
    let mut alice = join(&mut room, 1, "Alice", 0);
    start_round(&mut room, &alice, &[&alice], 0);
    // At 100 ms the round's clock reads tick 6, so stamps up to tick 36 are taken.
    room.on_event(RoomEvent::Input { connection: 1, tick: 36, input: calc_idle_input() }, 100);
    room.on_event(RoomEvent::Input { connection: 1, tick: 37, input: calc_idle_input() }, 100);
    room.on_event(RoomEvent::Cut { connection: 1, tick: 899 }, 100);
    room.step(116);
    room.step(133);

    // The taken input was held to and stepped from tick 36, and nothing later moved the world.
    assert_eq!(alice.take_pose_records(), [(1, 37)]);
}

#[test]
fn client_that_stops_reading_is_disconnected_once_its_outbox_fills_and_its_later_messages_ignored() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    let mut bob = join(&mut room, 2, "Bob", 0);
    start_round(&mut room, &alice, &[&alice, &bob], 0);
    let mut alice_messages = Vec::new();
    // A pose frame goes out every other step, so Bob's outbox fills well within these steps.
    for step in 1..=4 * OUTBOX_LEN as u64 {
        room.step(step * 16);
        alice_messages.extend(alice.take_messages());
    }
    room.on_event(RoomEvent::Start { connection: bob.connection }, 20_000);

    assert_eq!(bob.take_frames().len(), OUTBOX_LEN);
    assert_eq!(bob.inbox.try_recv(), Err(TryRecvError::Disconnected));
    let expected_room = ServerMessage::Room {
        phase: PhaseName::Playing,
        host: Some(1),
        members: vec![Member { id: 1, name: "Alice".into() }],
    };
    let last_room = alice_messages.iter().rev().find(|message| matches!(message, ServerMessage::Room { .. }));
    assert_eq!(last_room, Some(&expected_room));
}

#[test]
fn a_disconnected_clients_cut_still_on_its_way_does_not_reach_its_world() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    let bob = join(&mut room, 2, "Bob", 0);
    start_round(&mut room, &alice, &[&alice, &bob], 0);
    let mut now_ms = 0;
    while !bob.inbox.is_closed() {
        now_ms += 16;
        room.step(now_ms);
        alice.take_frames();
    }
    send_cut(&mut room, &alice, 0, now_ms);
    send_cut(&mut room, &bob, 0, now_ms);
    room.step(now_ms + 16);
    assert_eq!(find_results(&alice.take_messages()), None);

    room.step(ROUND_MS + GRACE_MS);
    let standings = find_results(&alice.take_messages()).unwrap();
    assert_eq!(find_standing(&standings, 2).tick, ROUND_TICKS);
}

#[test]
fn a_round_every_page_left_plays_out_and_the_next_page_finds_the_lobby() {
    let mut room = create_room();
    let alice = join(&mut room, 1, "Alice", 0);
    start_round(&mut room, &alice, &[&alice], 0);
    room.on_event(RoomEvent::Leave { connection: alice.connection }, 100);
    room.step(ROUND_MS);

    let mut bob = join(&mut room, 2, "Bob", ROUND_MS + 100);
    let messages = bob.take_messages();
    assert!(messages.iter().any(|message| matches!(message, ServerMessage::Room { phase: PhaseName::Lobby, .. })));
    assert!(messages.iter().any(|message| matches!(message, ServerMessage::Pick { is_random: true, .. })));
    assert_eq!(find_results(&messages), None);
}

#[test]
fn results_whose_last_page_left_give_way_to_the_lobby() {
    let mut room = create_room();
    let alice = join(&mut room, 1, "Alice", 0);
    start_round(&mut room, &alice, &[&alice], 0);
    send_cut(&mut room, &alice, 0, 100);
    room.step(116);
    room.on_event(RoomEvent::Leave { connection: alice.connection }, 200);

    let mut bob = join(&mut room, 2, "Bob", 300);
    let messages = bob.take_messages();
    assert!(messages.iter().any(|message| matches!(message, ServerMessage::Room { phase: PhaseName::Lobby, .. })));
    assert_eq!(find_results(&messages), None);
}

#[test]
fn a_hash_that_differs_from_the_replay_is_reported_to_its_client() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    start_round(&mut room, &alice, &[&alice], 0);
    let inputs = build_placement_inputs();
    let right_hash = replay_locally(&inputs[..60]).calc_hash();
    send_inputs(&mut room, &alice, &inputs[..60], 100);
    room.on_event(RoomEvent::Hash { connection: 1, tick: 60, hash: right_hash }, 100);
    room.step(116);
    assert!(!alice.take_messages().iter().any(|message| matches!(message, ServerMessage::Desync { .. })));

    room.on_event(RoomEvent::Hash { connection: 1, tick: 60, hash: right_hash ^ 1 }, 200);
    room.step(216);
    assert_eq!(alice.take_messages(), [ServerMessage::Desync { tick: 60 }]);
}
