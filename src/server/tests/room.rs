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
use cake_server::room::{
    ConnectionId, DebugCommand, Frame, HOST_PASSWORD, NameTaken, NextStep, OUTBOX_LEN, Room, RoomConfig, RoomEvent,
    create_outbox,
};
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::oneshot;

const CAKE: &str = "slice";
const OTHER_CAKE: &str = "other slice";
const ROUND_TICKS: u32 = 900;
/// 900 ticks at 60 Hz, then the grace for late inputs.
const ROUND_MS: u64 = 15_000;
const GRACE_MS: u64 = 2_000;
/// The most a page's round trip holds its world back beyond the grace, as the server sets it.
const MAX_INPUT_LAG_MS: u64 = 3_000;
const LOAD_TIMEOUT_MS: u64 = 10_000;
/// The real wait for the judging after the last cut, for the tests of it.
const JUDGING_DELAY_MS: u64 = 3_000;
/// An hour, as the owner set it.
const SEAT_HOLD_MS: u64 = 3_600_000;
/// The real judging reveal's lengths, for the tests of it.
const JUDGING_HOLD_MS: u64 = 500;
const JUDGING_MAX_COUNT_MS: u64 = 4_000;
const JUDGING_AFTER_MS: u64 = 500;
const JUDGING_TAIL_MS: u64 = 1_500;
/// A perfect cut's judging scene, the longest any player's takes.
const LONGEST_JUDGING_SCENE_MS: u64 = JUDGING_HOLD_MS + JUDGING_MAX_COUNT_MS + JUDGING_AFTER_MS;
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
        max_input_lag_ms: MAX_INPUT_LAG_MS,
        max_lead_ticks,
        seat_hold_ms: SEAT_HOLD_MS,
        judging_delay_ms,
        judging_hold_ms: 0,
        judging_min_count_ms: 0,
        judging_max_count_ms: 0,
        judging_after_ms: 0,
        judging_tail_ms: 0,
    }
}

/// A room whose judging reveal has the real lengths.
fn create_judging_room() -> Room {
    let config = RoomConfig {
        judging_hold_ms: JUDGING_HOLD_MS,
        judging_max_count_ms: JUDGING_MAX_COUNT_MS,
        judging_after_ms: JUDGING_AFTER_MS,
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
        judging_hold_ms: 0,
        judging_min_count_ms: 0,
        judging_max_count_ms: 0,
        judging_after_ms: 0,
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
    try_join(room, id, name, Some(standup_preference), seat, now_ms).unwrap()
}

/// Joins on connection `connection` with the standup preference none without a scrum update, or
/// is refused the name.
fn try_join(
    room: &mut Room,
    connection: ConnectionId,
    name: &str,
    standup_preference: Option<f64>,
    seat: Option<String>,
    now_ms: u64,
) -> Result<Client, NameTaken> {
    try_join_as(room, connection, name, standup_preference, seat, false, now_ms)
}

/// Joins as a page opened with `?debug` that is not the host.
fn join_debug_guest(room: &mut Room, id: PlayerId, name: &str, now_ms: u64) -> Client {
    try_join_as(room, id, name, Some(0.5), None, true, now_ms).unwrap()
}

/// Joins as a page opened with `?debug` and claims host.
fn join_debug(room: &mut Room, id: PlayerId, name: &str, now_ms: u64) -> Client {
    let client = join_debug_guest(room, id, name, now_ms);
    claim_host(room, &client, HOST_PASSWORD);
    client
}

fn try_join_as(
    room: &mut Room,
    connection: ConnectionId,
    name: &str,
    standup_preference: Option<f64>,
    seat: Option<String>,
    is_debug: bool,
    now_ms: u64,
) -> Result<Client, NameTaken> {
    let (outbox, inbox) = create_outbox();
    let (assigned, mut assigned_id) = oneshot::channel();
    let join =
        RoomEvent::Join { connection, name: name.to_string(), standup_preference, seat, is_debug, outbox, assigned };
    room.on_event(join, now_ms);
    let id = assigned_id.try_recv().unwrap()?;
    Ok(Client { connection, id, inbox })
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

/// The judging message's `order`, its scenes' `hold_ms`, `count_ms`, and `after_ms`, `elapsed_ms`,
/// and `standings`, if one came.
type Judging = (Vec<PlayerId>, (u64, Vec<u64>, u64), u64, Vec<Standing>);

fn find_judging(messages: &[ServerMessage]) -> Option<Judging> {
    messages.iter().find_map(|message| match message {
        ServerMessage::Judging { order, hold_ms, count_ms, after_ms, elapsed_ms, standings } => {
            Some((order.clone(), (*hold_ms, count_ms.clone(), *after_ms), *elapsed_ms, standings.clone()))
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
    let (order, scenes, elapsed_ms, standings) = find_judging(&alice_messages).unwrap();
    let mut judged = order.clone();
    judged.sort_unstable();
    assert_eq!(judged, [1, 2, 3]);
    assert_eq!(elapsed_ms, 0);
    assert_eq!(find_standing(&standings, 1).score, cut_locally(&build_placement_inputs()).calc_score());
    assert_eq!(find_results(&alice_messages), None);
    assert!(
        alice_messages.iter().any(|message| matches!(message, ServerMessage::Room { phase: PhaseName::Judging, .. }))
    );
    // Bob's and Carol's misses, judged first, count nothing, and so does Alice's 0.03% sliver.
    assert_eq!(scenes, (JUDGING_HOLD_MS, vec![0, 0, 0], JUDGING_AFTER_MS));
    let judging = Some((order, scenes, elapsed_ms, standings.clone()));
    assert_eq!(find_judging(&bob.take_messages()), judging);
    assert_eq!(find_judging(&carol.take_messages()), judging);

    let results_ms = 116 + 3 * (JUDGING_HOLD_MS + JUDGING_AFTER_MS) + JUDGING_TAIL_MS;
    room.step(results_ms - 1);
    assert_eq!(find_results(&alice.take_messages()), None);
    room.step(results_ms);
    assert_eq!(find_results(&alice.take_messages()), Some(standings));
}

/// A count long enough that the placement cut's 0.03% sliver counts for a while.
const SLIVER_COUNTING_MAX_COUNT_MS: u64 = 1_000_000_000_000;

/// Alice cuts the placement's sliver and Bob misses, both by the step at 116 ms, which starts the
/// judging on `config`.
fn judge_a_sliver_and_a_miss(config: RoomConfig) -> (Room, Client) {
    let mut room = Room::create(config, Catalog::create([(CAKE.to_string(), build_slice_missing_cake())]));
    let alice = join(&mut room, 1, "Alice", 0);
    let bob = join(&mut room, 2, "Bob", 0);
    start_round(&mut room, &alice, &[&alice, &bob], 0);
    send_inputs(&mut room, &alice, &build_placement_inputs(), 100);
    send_cut(&mut room, &alice, 300, 100);
    send_cut(&mut room, &bob, 0, 100);
    room.step(116);
    (room, alice)
}

#[test]
fn judging_counts_a_cut_from_the_floor_toward_the_longest_count_by_its_score_share_squared_and_a_miss_for_none() {
    let config = RoomConfig {
        judging_min_count_ms: 1_000_000,
        judging_max_count_ms: SLIVER_COUNTING_MAX_COUNT_MS,
        ..build_config(WHOLE_ROUND_LEAD_TICKS, 0)
    };
    let (_room, mut alice) = judge_a_sliver_and_a_miss(config);

    let (order, (_, count_ms, _), _, standings) = find_judging(&alice.take_messages()).unwrap();
    assert_eq!(find_standing(&standings, 1).score, 0.030_176_947_593_552_05);
    assert_eq!(order, [2, 1]);
    // 1e6 ms, plus the 1e12 - 1e6 ms above it times (0.0003017694759355205)^2.
    assert_eq!(count_ms, [0, 1_091_065]);
}

#[test]
fn judging_gives_way_to_the_results_once_every_scene_has_held_counted_and_lingered_and_the_tail_held() {
    let config = RoomConfig {
        judging_hold_ms: JUDGING_HOLD_MS,
        judging_max_count_ms: SLIVER_COUNTING_MAX_COUNT_MS,
        judging_after_ms: JUDGING_AFTER_MS,
        judging_tail_ms: JUDGING_TAIL_MS,
        ..build_config(WHOLE_ROUND_LEAD_TICKS, 0)
    };
    let (mut room, mut alice) = judge_a_sliver_and_a_miss(config);
    alice.take_messages();

    // Bob's miss counts for none and Alice's sliver for 91065 ms.
    let results_ms = 116 + 2 * (JUDGING_HOLD_MS + JUDGING_AFTER_MS) + 91_065 + JUDGING_TAIL_MS;
    room.step(results_ms - 1);
    assert_eq!(find_results(&alice.take_messages()), None);
    room.step(results_ms);
    assert!(find_results(&alice.take_messages()).is_some());
}

#[test]
fn judging_shows_every_miss_before_a_cut_through_the_cake() {
    // The misses' own order is random, so several rooms make a wrong order show.
    for _ in 0..8 {
        let mut room = create_judging_room();
        let [mut alice, ..] = judge_three(&mut room);

        let (order, _, _, standings) = find_judging(&alice.take_messages()).unwrap();
        // Bob and Carol cut at tick 0, with the cake still across the table from the knife.
        assert_eq!(find_standing(&standings, 2).below_m3, 0.0);
        assert_eq!(find_standing(&standings, 3).below_m3, 0.0);
        assert!(find_standing(&standings, 1).below_m3 > 0.0);
        assert_eq!(order[2], 1, "{order:?}");
    }
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

/// A reloaded page often joins before the server sees its old socket close.
#[test]
fn a_lone_page_reloaded_during_the_judging_rejoins_the_reveal() {
    let mut room = create_judging_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    let seat = find_seat(&alice.take_messages());
    start_round(&mut room, &alice, &[&alice], 0);
    send_cut(&mut room, &alice, 0, 100);
    room.step(116);

    let mut reloaded = join_seat(&mut room, 7, "Alice", 0.5, Some(seat), 1_000);
    let (order, _, elapsed_ms, _) = find_judging(&reloaded.take_messages()).unwrap();
    assert_eq!((order, elapsed_ms), (vec![1], 884));
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
fn late_joiner_plays_the_round_in_progress_with_the_time_left_once_loaded_and_is_judged() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
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
        players: build_members(&[(1, "Alice"), (2, "Bob"), (3, "Carol")])
    }));
    assert!(messages.contains(&ServerMessage::Start { elapsed_ms: 5_000 }));
    assert!(messages.contains(&ServerMessage::StillLoading { players: vec![3] }));
    assert!(messages.iter().any(|message| matches!(message, ServerMessage::Cut { id: 1, tick: 300, .. })));

    // Carol's page loads the cake a second later, at tick 360, steps the ticks it missed with the
    // idle input it holds, then plays and cuts.
    alice.take_messages();
    room.on_event(RoomEvent::Ready { connection: carol.connection }, 6_000);
    assert_eq!(alice.take_messages(), [ServerMessage::NowPlaying { player: Member { id: 3, name: "Carol".into() } }]);
    let mut carol_inputs = vec![calc_idle_input(); 360];
    carol_inputs.extend(build_placement_inputs());
    send_inputs(&mut room, &carol, &carol_inputs[..600], 6_100);
    send_cut(&mut room, &carol, 600, 6_100);
    send_cut(&mut room, &bob, 200, 6_100);
    room.step(6_116);
    let standings = find_results(&carol.take_messages()).unwrap();
    assert_eq!(standings.len(), 3);
    let carol_standing = find_standing(&standings, 3);
    assert_eq!((carol_standing.name.as_str(), carol_standing.tick), ("Carol", 600));
    assert_eq!(carol_standing.score, cut_locally(&carol_inputs[..600]).calc_score());
}

#[test]
fn a_late_joiners_world_catches_up_with_the_round_clock_a_few_ticks_a_step() {
    let mut room = create_room();
    let alice = join(&mut room, 1, "Alice", 0);
    start_round(&mut room, &alice, &[&alice], 0);
    room.step(10_000);
    let mut carol = join(&mut room, 3, "Carol", 10_000);
    // The round's clock reads ticks 600, 601, and 603 at these steps, and each holds the new world
    // on 30 ticks beyond the clock's advance since the step before.
    for now_ms in [10_016, 10_033, 10_050] {
        room.step(now_ms);
    }
    let last_tick = |records: &[(PlayerId, u32)], player: PlayerId| {
        records.iter().rev().find(|&&(id, _)| id == player).map(|&(_, tick)| tick)
    };
    assert_eq!(last_tick(&carol.take_pose_records(), 3), Some(93));

    let mut now_ms = 10_050;
    while now_ms < 11_000 {
        now_ms += 16;
        room.step(now_ms);
    }
    // Caught up, the world trails the clock by the grace for inputs, as Alice's does.
    let records = carol.take_pose_records();
    assert!(last_tick(&records, 1).is_some_and(|tick| tick >= calc_round_tick(now_ms) - 121));
    assert_eq!(last_tick(&records, 3), last_tick(&records, 1));
}

/// Every round player has cut and the whistle has blown, so the round has no time left to give.
#[test]
fn a_page_joining_after_every_player_cut_spectates_the_rest_of_the_round() {
    let mut room = create_room_with(WHOLE_ROUND_LEAD_TICKS, JUDGING_DELAY_MS);
    let mut alice = join(&mut room, 1, "Alice", 0);
    start_round(&mut room, &alice, &[&alice], 0);
    send_cut(&mut room, &alice, 0, 100);
    room.step(116);
    assert!(alice.take_messages().contains(&ServerMessage::PlayOver));

    let mut carol = join(&mut room, 3, "Carol", 1_000);
    let messages = carol.take_messages();
    let players = build_members(&[(1, "Alice")]);
    assert!(messages.contains(&ServerMessage::Round { cake: CAKE.into(), round_ticks: ROUND_TICKS, players }));
    assert!(!messages.iter().any(|message| matches!(message, ServerMessage::Resume { .. })));
    room.step(116 + JUDGING_DELAY_MS);
    let (order, ..) = find_judging(&carol.take_messages()).unwrap();
    assert_eq!(order, [1]);
}

#[test]
fn a_bot_added_after_every_player_cut_waits_for_the_next_round() {
    let mut room = create_room_with(WHOLE_ROUND_LEAD_TICKS, JUDGING_DELAY_MS);
    let mut alice = join_debug(&mut room, 1, "Alice", 0);
    start_round(&mut room, &alice, &[&alice], 0);
    send_cut(&mut room, &alice, 0, 100);
    room.step(116);
    send_debug(&mut room, &alice, DebugCommand::AddBot, 1_000);
    room.step(116 + JUDGING_DELAY_MS);
    let (order, ..) = find_judging(&alice.take_messages()).unwrap();
    assert_eq!(order, [1]);

    room.on_event(RoomEvent::ReturnToLobby { connection: alice.connection }, 200_000);
    room.on_event(RoomEvent::Start { connection: alice.connection }, 200_000);
    let players = build_members(&[(1, "Alice"), (FIRST_BOT, "Bot 1")]);
    let round = ServerMessage::Round { cake: CAKE.into(), round_ticks: ROUND_TICKS, players };
    assert!(alice.take_messages().contains(&round));
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
    assert_eq!(
        bob.take_messages().last(),
        Some(&ServerMessage::Pick { cake: OTHER_CAKE.into(), is_random: false, round_ticks: ROUND_TICKS })
    );
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
    let Some(ServerMessage::Pick { cake, is_random: true, .. }) = alice.take_messages().pop() else {
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
    let mut room = create_judging_room();
    let mut alice = join_preferring(&mut room, 1, "Alice", 1.0, 0);
    let bob = join_preferring(&mut room, 2, "Bob", 1.0, 0);
    start_round(&mut room, &alice, &[&alice, &bob], 0);
    send_inputs(&mut room, &alice, &build_placement_inputs(), 100);
    send_cut(&mut room, &alice, 300, 100);
    send_cut(&mut room, &bob, 0, 100);
    room.step(116);
    // Joining during the judging, Carol spectates.
    let _carol = join_preferring(&mut room, 3, "Carol", 1.0, 200);
    room.step(116 + 2 * LONGEST_JUDGING_SCENE_MS + JUDGING_TAIL_MS);

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
fn a_page_still_loading_at_the_load_timeout_plays_once_loaded_with_the_time_left_and_shows_from_then() {
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
    let loading_idx = messages.iter().position(|message| *message == ServerMessage::StillLoading { players: vec![2] });
    let start_idx = messages.iter().position(|message| *message == ServerMessage::Start { elapsed_ms: 0 });
    assert!(loading_idx.unwrap() < start_idx.unwrap());

    // Bob's page loads 5 s into the round, at tick 300, steps the ticks it missed with the input
    // it holds, the idle one, then plays and cuts.
    let loaded_ms = LOAD_TIMEOUT_MS + 5_000;
    room.step(loaded_ms);
    alice.take_messages();
    room.on_event(RoomEvent::Ready { connection: bob.connection }, loaded_ms);
    assert_eq!(alice.take_messages(), [ServerMessage::NowPlaying { player: Member { id: 2, name: "Bob".into() } }]);
    let mut bob_inputs = vec![calc_idle_input(); 300];
    bob_inputs.extend(build_placement_inputs());
    send_inputs(&mut room, &bob, &bob_inputs[..550], loaded_ms + 100);
    send_cut(&mut room, &bob, 550, loaded_ms + 100);
    send_cut(&mut room, &alice, 0, loaded_ms + 100);
    room.step(loaded_ms + 116);
    let standings = find_results(&bob.take_messages()).unwrap();
    let bob_standing = find_standing(&standings, 2);
    assert_eq!((bob_standing.tick, bob_standing.has_touched), (550, true));
    assert_eq!(bob_standing.score, cut_locally(&bob_inputs[..550]).calc_score());
}

#[test]
fn a_round_no_page_loaded_by_the_load_timeout_starts_on_time_and_judges_its_untouched_worlds() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    claim_host(&mut room, &alice, HOST_PASSWORD);
    room.on_event(RoomEvent::Start { connection: alice.connection }, 0);
    alice.take_messages();
    room.step(LOAD_TIMEOUT_MS);
    let messages = alice.take_messages();
    assert_eq!(messages[0], ServerMessage::StillLoading { players: vec![1] });
    assert!(messages.contains(&ServerMessage::Start { elapsed_ms: 0 }));

    room.step(LOAD_TIMEOUT_MS + ROUND_MS + GRACE_MS);
    let standings = find_results(&alice.take_messages()).unwrap();
    assert_eq!((standings[0].id, standings[0].tick, standings[0].has_touched), (1, ROUND_TICKS, false));
}

#[test]
fn a_page_joining_while_the_round_loads_plays_it_without_holding_up_its_start() {
    let mut room = create_room();
    let alice = join(&mut room, 1, "Alice", 0);
    claim_host(&mut room, &alice, HOST_PASSWORD);
    room.on_event(RoomEvent::Start { connection: alice.connection }, 0);
    let mut carol = join(&mut room, 3, "Carol", 50);
    assert!(carol.take_messages().contains(&ServerMessage::Round {
        cake: CAKE.into(),
        round_ticks: ROUND_TICKS,
        players: build_members(&[(1, "Alice"), (3, "Carol")])
    }));
    room.on_event(RoomEvent::Ready { connection: alice.connection }, 100);
    room.step(116);
    let messages = carol.take_messages();
    assert!(messages.contains(&ServerMessage::StillLoading { players: vec![3] }));
    assert!(messages.contains(&ServerMessage::Start { elapsed_ms: 0 }));
}

#[test]
fn a_page_joining_while_the_round_loads_is_shown_to_every_page_once_loaded_since_the_round_message_left_it_out() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    claim_host(&mut room, &alice, HOST_PASSWORD);
    room.on_event(RoomEvent::Start { connection: alice.connection }, 0);
    let carol = join(&mut room, 3, "Carol", 50);
    alice.take_messages();
    room.on_event(RoomEvent::Ready { connection: carol.connection }, 80);
    assert_eq!(alice.take_messages(), [ServerMessage::NowPlaying { player: Member { id: 3, name: "Carol".into() } }]);
    room.on_event(RoomEvent::Ready { connection: alice.connection }, 100);
    assert_eq!(alice.take_messages(), []);
}

#[test]
fn a_page_that_joins_while_the_round_loads_and_leaves_is_shown_to_every_page_as_a_leaver() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    claim_host(&mut room, &alice, HOST_PASSWORD);
    room.on_event(RoomEvent::Start { connection: alice.connection }, 0);
    let carol = join(&mut room, 3, "Carol", 50);
    alice.take_messages();
    room.on_event(RoomEvent::Leave { connection: carol.connection }, 80);
    let now_playing = ServerMessage::NowPlaying { player: Member { id: 3, name: "Carol".into() } };
    assert!(alice.take_messages().contains(&now_playing));
}

/// The start waits only for the players in the room when the host pressed Start.
#[test]
fn a_page_that_joined_while_the_round_loads_and_reopens_does_not_hold_up_its_start() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    claim_host(&mut room, &alice, HOST_PASSWORD);
    room.on_event(RoomEvent::Start { connection: alice.connection }, 0);
    let mut carol = join(&mut room, 3, "Carol", 50);
    let seat = find_seat(&carol.take_messages());
    room.on_event(RoomEvent::Leave { connection: carol.connection }, 80);
    let carol = join_seat(&mut room, 7, "Carol", 0.5, Some(seat), 90);
    room.on_event(RoomEvent::Ready { connection: alice.connection }, 100);
    room.step(116);
    let messages = alice.take_messages();
    assert!(messages.contains(&ServerMessage::Start { elapsed_ms: 0 }));
    assert!(messages.contains(&ServerMessage::StillLoading { players: vec![3] }));

    room.on_event(RoomEvent::Ready { connection: carol.connection }, 200);
    assert_eq!(alice.take_messages(), [ServerMessage::NowPlaying { player: Member { id: 3, name: "Carol".into() } }]);
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
    // Bob's first input arrives at once, so he is near, and then he goes silent too.
    send_inputs(&mut room, &bob, &[calc_idle_input()], 0);
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
    // The page's first input arrives at once, so it is near, and then its network stalls.
    send_inputs(&mut room, &alice, &[calc_idle_input()], 0);
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
    // The page's first input arrives at once, so the page is near, and then its network stalls.
    send_inputs(&mut room, &alice, &inputs[..1], 0);
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

/// The earliest time the round's clock, for a round started at 0, reads `tick`.
fn calc_ms_at_round_tick(tick: u32) -> u64 {
    (u64::from(tick) * 1000).div_ceil(u64::from(TICKS_PER_S))
}

/// A page keeps to the round's clock from when `start` reached it, so its inputs arrive a round
/// trip behind the clock. A round trip of 2.5 s outlasts the grace, yet holds the world back no
/// more than a page next door's, so it never parts and the timeout cuts it at the round's last tick.
#[test]
fn a_page_a_round_trip_past_the_grace_away_plays_to_the_timeout_on_its_own_inputs_without_a_resync() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    start_round(&mut room, &alice, &[&alice], 0);
    alice.take_messages();
    // The player waits a second before touching anything, then plays the script to the timeout.
    let mut played = vec![calc_idle_input(); 60];
    played.extend(build_scripted_inputs());
    played.resize(ROUND_TICKS as usize, *played.last().unwrap());

    let round_trip_ticks = 150;
    let mut messages = Vec::new();
    for clock_tick in 1..=ROUND_TICKS + round_trip_ticks {
        let now_ms = calc_ms_at_round_tick(clock_tick);
        // The input the page stepped a round trip of the clock ago arrives.
        let arrived =
            clock_tick.checked_sub(round_trip_ticks).and_then(|tick| Some((tick, *played.get(tick as usize)?)));
        if let Some((tick, input)) = arrived {
            room.on_event(RoomEvent::Input { connection: alice.connection, tick, input }, now_ms);
        }
        room.step(now_ms);
        messages.extend(alice.take_messages());
    }

    assert!(!messages.iter().any(|message| matches!(message, ServerMessage::Resume { .. })));
    let standings = find_results(&messages).unwrap();
    assert_eq!(find_standing(&standings, 1).tick, ROUND_TICKS);
    assert_eq!(find_standing(&standings, 1).score, cut_locally(&played).calc_score());
}

/// A page 1000 ms away each way, a little past the grace's round trip, whose player grabs the cake
/// on the round's first tick, as one who pressed on it before the start reached them does, never
/// parts, though the server hears of the page only a round trip after its start.
#[test]
fn a_page_a_second_away_that_moves_from_the_first_tick_plays_without_a_resync() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    start_round(&mut room, &alice, &[&alice], 0);
    alice.take_messages();
    let mut played: Vec<Input> =
        build_scripted_inputs().into_iter().skip_while(|&input| input == calc_idle_input()).collect();
    played.resize(ROUND_TICKS as usize, *played.last().unwrap());

    let round_trip_ticks = 125;
    let mut messages = Vec::new();
    for clock_tick in 1..=ROUND_TICKS + round_trip_ticks {
        let now_ms = calc_ms_at_round_tick(clock_tick);
        if let Some(tick) = clock_tick.checked_sub(round_trip_ticks).filter(|&tick| tick < ROUND_TICKS) {
            room.on_event(
                RoomEvent::Input { connection: alice.connection, tick, input: played[tick as usize] },
                now_ms,
            );
        }
        room.step(now_ms);
        messages.extend(alice.take_messages());
    }

    assert!(!messages.iter().any(|message| matches!(message, ServerMessage::Resume { .. })));
    let standings = find_results(&messages).unwrap();
    assert_eq!(find_standing(&standings, 1).score, cut_locally(&played).calc_score());
}

/// A page 1000 ms away each way whose stream stalls for a lost packet's resend, a round trip and
/// 0.1 s, as TCP's fast retransmit takes, while its player carries the cake, never parts.
#[test]
fn a_page_a_second_away_rides_out_a_resend_a_round_trip_long_without_a_resync() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    start_round(&mut room, &alice, &[&alice], 0);
    alice.take_messages();
    let mut played = vec![calc_idle_input(); 60];
    played.extend(build_scripted_inputs());
    played.resize(ROUND_TICKS as usize, *played.last().unwrap());

    let round_trip_ticks = 120;
    // The input stamped tick 120 is lost. The page learns so a round trip later and resends it,
    // which takes half a round trip more, so it and every input sent meanwhile arrive at once.
    let lost_tick = 120;
    let resent_at_clock_tick = lost_tick + 2 * round_trip_ticks + 6;
    let arrival_clock_tick =
        |tick: u32| (tick + round_trip_ticks).max(if tick >= lost_tick { resent_at_clock_tick } else { 0 });
    // The carry moves the hand in the ticks the stall holds up.
    assert!(
        played[lost_tick as usize..lost_tick as usize + 6].iter().any(|&input| input != played[lost_tick as usize - 1])
    );

    let mut messages = Vec::new();
    let mut next_tick = 0;
    for clock_tick in 1..=ROUND_TICKS + 3 * round_trip_ticks {
        let now_ms = calc_ms_at_round_tick(clock_tick);
        while next_tick < ROUND_TICKS && arrival_clock_tick(next_tick) <= clock_tick {
            let input = played[next_tick as usize];
            room.on_event(RoomEvent::Input { connection: alice.connection, tick: next_tick, input }, now_ms);
            next_tick += 1;
        }
        room.step(now_ms);
        messages.extend(alice.take_messages());
    }

    assert!(!messages.iter().any(|message| matches!(message, ServerMessage::Resume { .. })));
    let standings = find_results(&messages).unwrap();
    assert_eq!(find_standing(&standings, 1).score, cut_locally(&played).calc_score());
}

/// However late a page's inputs arrive, its world trails the round's clock by at most the grace
/// and twice 3 s more, a round trip and a resend's, so a round never waits long on one far-off page.
#[test]
fn a_page_ten_seconds_away_has_its_world_held_eight_seconds_behind_the_round_clock() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    start_round(&mut room, &alice, &[&alice], 0);
    let inputs = vec![calc_idle_input(); 121];
    // At 12000 ms the round's clock reads tick 720, and the input for tick 120 is 600 ticks late.
    send_inputs(&mut room, &alice, &inputs, 12_000);
    alice.take_frames();
    room.step(12_000);
    room.step(12_016);

    // 720 less the grace's 120 ticks, and the round trip's 180 at most twice.
    assert_eq!(alice.take_pose_records(), [(1, 240)]);
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
        players: build_members(&[(1, "Alice"), (2, "Bob"), (3, "Carol")])
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
fn a_page_reopened_while_the_round_loads_must_load_the_cake_again_or_start_still_loading() {
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
    assert!(messages.contains(&ServerMessage::StillLoading { players: vec![1] }));
    assert!(messages.contains(&ServerMessage::Start { elapsed_ms: 0 }));
}

/// The members the last room message in `messages` lists.
fn find_members(messages: &[ServerMessage]) -> Vec<Member> {
    messages
        .iter()
        .rev()
        .find_map(|message| match message {
            ServerMessage::Room { members, .. } => Some(members.clone()),
            _ => None,
        })
        .unwrap()
}

#[test]
fn a_page_rejoining_with_its_seat_token_while_its_old_connection_is_open_takes_the_seat_and_closes_the_old_one() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    let mut bob = join(&mut room, 2, "Bob", 0);
    let seat = find_seat(&alice.take_messages());
    let mut reloaded = join_seat(&mut room, 3, "Alice", 0.5, Some(seat.clone()), 100);
    assert_eq!(reloaded.id, 1);
    assert_eq!(reloaded.take_messages()[0], ServerMessage::Welcome { id: 1, seat });
    alice.take_frames();
    assert!(alice.inbox.is_closed());

    // The old connection's socket closes at last, which leaves the reloaded page seated.
    room.on_event(RoomEvent::Leave { connection: alice.connection }, 200);
    claim_host(&mut room, &reloaded, HOST_PASSWORD);
    assert_eq!(find_host(&reloaded.take_messages()), Some(Some(1)));
    assert_eq!(find_members(&bob.take_messages()), build_members(&[(1, "Alice"), (2, "Bob")]));
}

#[test]
fn an_empty_room_next_steps_as_its_earliest_held_seat_expires_an_hour_after_its_players_last_activity() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    let alice_seat = find_seat(&alice.take_messages());
    let bob = join(&mut room, 2, "Bob", 500);
    room.on_event(RoomEvent::Leave { connection: alice.connection }, 1_000);
    room.on_event(RoomEvent::Leave { connection: bob.connection }, 1_000);
    assert_eq!(room.get_next_step(), NextStep::At { at_ms: SEAT_HOLD_MS + 1 });

    room.step(SEAT_HOLD_MS + 1);
    assert_eq!(room.get_next_step(), NextStep::At { at_ms: SEAT_HOLD_MS + 501 });
    let late = join_seat(&mut room, 3, "Alice", 0.5, Some(alice_seat), SEAT_HOLD_MS + 2);
    assert_eq!(late.id, 3);
}

#[test]
fn a_seat_reopens_up_to_an_hour_after_its_players_last_activity() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    let seat = find_seat(&alice.take_messages());
    room.on_event(RoomEvent::Leave { connection: alice.connection }, 200);
    room.step(SEAT_HOLD_MS);
    let reopened = join_seat(&mut room, 3, "Alice", 0.5, Some(seat), SEAT_HOLD_MS);
    assert_eq!(reopened.id, 1);
}

#[test]
fn a_lobby_needs_no_step_until_an_event_whether_empty_or_not() {
    let mut room = create_room();
    assert_eq!(room.get_next_step(), NextStep::AfterEvent);
    let _alice = join(&mut room, 1, "Alice", 0);
    assert_eq!(room.get_next_step(), NextStep::AfterEvent);
}

#[test]
fn a_room_steps_every_tick_from_the_rounds_start_until_its_results_show() {
    let mut room = create_judging_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    claim_host(&mut room, &alice, HOST_PASSWORD);
    pick(&mut room, &alice, CAKE);
    room.on_event(RoomEvent::Start { connection: alice.connection }, 0);
    assert_eq!(room.get_next_step(), NextStep::EveryTick, "loading");
    room.on_event(RoomEvent::Ready { connection: alice.connection }, 0);
    room.step(0);
    assert_eq!(room.get_next_step(), NextStep::EveryTick, "playing");
    send_cut(&mut room, &alice, 0, 100);
    room.step(116);
    assert_eq!(room.get_next_step(), NextStep::EveryTick, "judging");

    room.step(1_000_000);
    assert!(find_results(&alice.take_messages()).is_some());
    assert_eq!(room.get_next_step(), NextStep::AfterEvent);
}

#[test]
fn a_bot_needs_no_step_in_the_lobby_or_the_results_since_it_plays_only_in_a_round() {
    let mut room = create_room();
    let mut alice = join_debug(&mut room, 1, "Alice", 0);
    send_debug(&mut room, &alice, DebugCommand::AddBot, 100);
    assert_eq!(room.get_next_step(), NextStep::AfterEvent, "lobby");
    start_round(&mut room, &alice, &[&alice], 200);
    assert_eq!(room.get_next_step(), NextStep::EveryTick, "playing");

    send_cut(&mut room, &alice, 0, 300);
    assert!(step_until_results(&mut room, &mut alice, 300, ROUND_MS + GRACE_MS).is_some());
    assert_eq!(room.get_next_step(), NextStep::AfterEvent, "results");
}

#[test]
fn a_join_with_the_name_of_a_connected_player_or_spectator_is_refused_ignoring_case_and_surrounding_spaces() {
    let mut room = create_room();
    let alice = join(&mut room, 1, "Alice", 0);
    start_round(&mut room, &alice, &[&alice], 0);
    let mut bob = join(&mut room, 2, "Bob", 100);
    bob.take_messages();

    assert!(matches!(try_join(&mut room, 3, "ALICE", Some(0.5), None, 200), Err(NameTaken)));
    assert!(matches!(try_join(&mut room, 4, "  bob ", Some(0.5), None, 200), Err(NameTaken)));
    assert_eq!(bob.take_messages(), []);
    assert!(try_join(&mut room, 5, "Bobby", Some(0.5), None, 200).is_ok());
}

#[test]
fn a_join_with_the_name_of_a_player_whose_page_closed_takes_over_their_seat_and_its_token() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    let bob = join(&mut room, 2, "Bob", 0);
    let seat = find_seat(&alice.take_messages());
    start_round(&mut room, &bob, &[&alice, &bob], 0);
    let sent = &build_placement_inputs()[..200];
    send_inputs(&mut room, &alice, sent, 100);
    room.step(116);
    room.on_event(RoomEvent::Leave { connection: alice.connection }, 200);

    // Another browser, with no token, types the name.
    let mut joiner = try_join(&mut room, 9, " alice", Some(0.5), None, 5_000).unwrap();
    assert_eq!(joiner.id, 1);
    let messages = joiner.take_messages();
    assert_eq!(messages[0], ServerMessage::Welcome { id: 1, seat });
    assert!(messages.iter().any(|message| matches!(message, ServerMessage::Resume { .. })));
}

#[test]
fn a_player_without_a_scrum_update_plays_and_ranks_but_gets_no_standup_slot() {
    let mut room = create_judging_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    let bob = try_join(&mut room, 2, "Bob", None, None, 0).unwrap();
    start_round(&mut room, &alice, &[&alice, &bob], 0);
    send_cut(&mut room, &alice, 0, 100);
    send_cut(&mut room, &bob, 0, 100);
    room.step(116);
    // Joining during the judging, Carol and Dave spectate.
    let _carol = try_join(&mut room, 3, "Carol", None, None, 200).unwrap();
    let _dave = join(&mut room, 4, "Dave", 200);
    room.step(116 + 2 * LONGEST_JUDGING_SCENE_MS + JUDGING_TAIL_MS);

    let messages = alice.take_messages();
    let ranked: Vec<PlayerId> = find_results(&messages).unwrap().iter().map(|standing| standing.id).collect();
    assert_eq!(ranked, [1, 2]);
    assert_eq!(find_standup(&messages), Some(vec![1, 4]));
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

    // Bob's world waited for his first input until he left, and catches up a few ticks a step.
    let mut messages = Vec::new();
    for step_ms in (now_ms + 32..=ROUND_MS + GRACE_MS).step_by(16) {
        room.step(step_ms);
        messages.extend(alice.take_messages());
    }
    let standings = find_results(&messages).unwrap();
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

/// The first bot a room adds, by its id.
const FIRST_BOT: PlayerId = (1 << 30) + 1;

fn send_debug(room: &mut Room, client: &Client, command: DebugCommand, now_ms: u64) {
    room.on_event(RoomEvent::Debug { connection: client.connection, command }, now_ms);
}

/// Steps the room every 16 ms after `from_ms` until the results come or `until_ms` passes,
/// returning the results and when they came.
fn step_until_results(
    room: &mut Room,
    client: &mut Client,
    from_ms: u64,
    until_ms: u64,
) -> Option<(Vec<Standing>, u64)> {
    let mut now_ms = from_ms;
    while now_ms < until_ms {
        now_ms += 16;
        room.step(now_ms);
        if let Some(standings) = find_results(&client.take_messages()) {
            return Some((standings, now_ms));
        }
    }
    None
}

#[test]
fn a_debug_page_hears_the_debug_settings_and_a_page_without_debug_cannot_change_them() {
    let mut room = create_room();
    let mut alice = join_debug(&mut room, 1, "Alice", 0);
    assert!(alice.take_messages().contains(&ServerMessage::Debug { round_s: 15, bots: 0, bot_latency_ms: 0 }));
    let mut bob = join(&mut room, 2, "Bob", 0);
    assert!(!bob.take_messages().iter().any(|message| matches!(message, ServerMessage::Debug { .. })));
    alice.take_messages();

    send_debug(&mut room, &bob, DebugCommand::AddBot, 100);
    assert!(matches!(bob.take_messages().as_slice(), [ServerMessage::Error { .. }]));
    assert_eq!(alice.take_messages(), []);
}

#[test]
fn only_the_host_changes_the_debug_settings_of_a_room_with_two_debug_pages() {
    let mut room = create_room();
    let mut alice = join_debug(&mut room, 1, "Alice", 0);
    let mut bob = join_debug_guest(&mut room, 2, "Bob", 0);
    alice.take_messages();
    bob.take_messages();

    send_debug(&mut room, &bob, DebugCommand::AddBot, 100);
    send_debug(&mut room, &bob, DebugCommand::SetRoundLength { round_s: 10 }, 100);
    assert!(matches!(bob.take_messages().as_slice(), [ServerMessage::Error { .. }, ServerMessage::Error { .. }]));
    assert_eq!(alice.take_messages(), []);

    send_debug(&mut room, &alice, DebugCommand::AddBot, 200);
    send_debug(&mut room, &alice, DebugCommand::SetRoundLength { round_s: 10 }, 200);
    let debug = ServerMessage::Debug { round_s: 10, bots: 1, bot_latency_ms: 0 };
    assert!(alice.take_messages().contains(&debug));
    assert!(bob.take_messages().contains(&debug), "the guest still hears the settings");
}

#[test]
fn a_debug_page_adds_bots_to_the_room_and_removes_the_newest() {
    let mut room = create_room();
    let mut alice = join_debug(&mut room, 1, "Alice", 0);
    send_debug(&mut room, &alice, DebugCommand::AddBot, 100);
    send_debug(&mut room, &alice, DebugCommand::AddBot, 100);
    let messages = alice.take_messages();
    assert_eq!(find_members(&messages), build_members(&[(1, "Alice"), (FIRST_BOT, "Bot 1"), (FIRST_BOT + 1, "Bot 2")]));
    assert!(messages.contains(&ServerMessage::Debug { round_s: 15, bots: 2, bot_latency_ms: 0 }));

    send_debug(&mut room, &alice, DebugCommand::RemoveBot, 200);
    let messages = alice.take_messages();
    assert_eq!(find_members(&messages), build_members(&[(1, "Alice"), (FIRST_BOT, "Bot 1")]));
    assert!(messages.contains(&ServerMessage::Debug { round_s: 15, bots: 1, bot_latency_ms: 0 }));
}

#[test]
fn a_bot_is_refused_once_the_room_holds_16_members_but_a_page_still_joins() {
    let mut room = create_room();
    let mut alice = join_debug(&mut room, 1, "Alice", 0);
    for _ in 0..15 {
        send_debug(&mut room, &alice, DebugCommand::AddBot, 0);
    }
    alice.take_messages();
    send_debug(&mut room, &alice, DebugCommand::AddBot, 100);
    assert!(matches!(alice.take_messages().as_slice(), [ServerMessage::Error { .. }]));

    join(&mut room, 2, "Bob", 200);
    let members = find_members(&alice.take_messages());
    assert_eq!(members.len(), 17);
    assert!(members.contains(&Member { id: 2, name: "Bob".into() }));
}

#[test]
fn a_bot_takes_the_lowest_bot_name_no_one_in_the_room_or_its_round_has() {
    let mut room = create_room();
    let mut alice = join_debug(&mut room, 1, "bot 1", 0);
    send_debug(&mut room, &alice, DebugCommand::AddBot, 0);
    send_debug(&mut room, &alice, DebugCommand::AddBot, 0);
    start_round(&mut room, &alice, &[&alice], 0);
    // Bot 3 leaves the room but stays in the round, so the bot added next is not named after it.
    send_debug(&mut room, &alice, DebugCommand::RemoveBot, 100);
    send_debug(&mut room, &alice, DebugCommand::AddBot, 100);
    let names: Vec<String> = find_members(&alice.take_messages()).into_iter().map(|member| member.name).collect();
    assert_eq!(names, ["bot 1", "Bot 2", "Bot 4"]);
}

#[test]
fn a_bot_loads_plays_and_cuts_the_round_and_is_ranked() {
    let mut room = create_room();
    let mut alice = join_debug(&mut room, 1, "Alice", 0);
    send_debug(&mut room, &alice, DebugCommand::AddBot, 0);
    start_round(&mut room, &alice, &[&alice], 0);
    assert!(alice.take_messages().contains(&ServerMessage::Start { elapsed_ms: 0 }));
    send_cut(&mut room, &alice, 0, 16);

    let (standings, _) = step_until_results(&mut room, &mut alice, 0, ROUND_MS + GRACE_MS).unwrap();
    let bot = find_standing(&standings, FIRST_BOT);
    assert_eq!((bot.name.as_str(), bot.has_touched), ("Bot 1", true));
    // A bot cuts at random between half and 95% of its time.
    assert!((450..=855).contains(&bot.tick), "cut at tick {}", bot.tick);
}

#[test]
fn a_bot_added_mid_round_plays_the_time_left_and_is_ranked() {
    let mut room = create_room();
    let mut alice = join_debug(&mut room, 1, "Alice", 0);
    start_round(&mut room, &alice, &[&alice], 0);
    room.step(5_000);
    alice.take_messages();
    send_debug(&mut room, &alice, DebugCommand::AddBot, 5_000);
    room.step(5_016);
    let bot = Member { id: FIRST_BOT, name: "Bot 1".into() };
    assert!(alice.take_messages().contains(&ServerMessage::NowPlaying { player: bot }));

    let (standings, _) = step_until_results(&mut room, &mut alice, 5_016, ROUND_MS + GRACE_MS + 100).unwrap();
    let bot = find_standing(&standings, FIRST_BOT);
    assert!(bot.has_touched);
    // It started at tick 300 of 900, so cuts at random between half and 95% of the 600 left.
    assert!((600..=870).contains(&bot.tick), "cut at tick {}", bot.tick);
}

#[test]
fn a_bots_latency_delays_its_ready_and_its_inputs_each_way() {
    let mut room = create_room();
    let mut alice = join_debug(&mut room, 1, "Alice", 0);
    send_debug(&mut room, &alice, DebugCommand::SetBotLatency { latency_ms: 500 }, 0);
    assert!(alice.take_messages().contains(&ServerMessage::Debug { round_s: 15, bots: 0, bot_latency_ms: 500 }));
    send_debug(&mut room, &alice, DebugCommand::AddBot, 0);
    // The round message reaches the bot 500 ms late, and its ready the room 500 ms after that.
    start_round(&mut room, &alice, &[&alice], 1_000);
    room.step(1_999);
    assert!(!alice.take_messages().contains(&ServerMessage::Start { elapsed_ms: 0 }));
    room.step(2_000);
    assert!(alice.take_messages().contains(&ServerMessage::Start { elapsed_ms: 0 }));

    // 2 s into the round, the bot's inputs have reached the room for its first second only.
    let mut now_ms = 2_000;
    while now_ms < 4_000 {
        now_ms += 16;
        room.step(now_ms);
    }
    let bot_tick = alice.take_pose_records().into_iter().rev().find(|&(id, _)| id == FIRST_BOT).map(|(_, tick)| tick);
    assert!(bot_tick.is_some_and(|tick| (58..=60).contains(&tick)), "bot at tick {bot_tick:?}");
}

#[test]
fn a_round_length_a_debug_page_sets_comes_with_the_pick_and_plays_from_the_next_round() {
    let mut room = create_room();
    let mut alice = join_debug(&mut room, 1, "Alice", 0);
    claim_host(&mut room, &alice, HOST_PASSWORD);
    pick(&mut room, &alice, CAKE);
    alice.take_messages();
    send_debug(&mut room, &alice, DebugCommand::SetRoundLength { round_s: 10 }, 100);
    assert_eq!(
        alice.take_messages(),
        [
            ServerMessage::Pick { cake: CAKE.into(), is_random: false, round_ticks: 600 },
            ServerMessage::Debug { round_s: 10, bots: 0, bot_latency_ms: 0 }
        ]
    );
    send_debug(&mut room, &alice, DebugCommand::SetRoundLength { round_s: 0 }, 100);
    assert!(matches!(alice.take_messages().as_slice(), [ServerMessage::Error { .. }]));

    room.on_event(RoomEvent::Start { connection: alice.connection }, 200);
    let players = build_members(&[(1, "Alice")]);
    assert!(alice.take_messages().contains(&ServerMessage::Round { cake: CAKE.into(), round_ticks: 600, players }));
}
