//! The room driven in process by simulated clients, on a made-up clock.

use cake_core::cakes::build_slice_missing_cake;
use cake_core::cut::Split;
use cake_core::input::Input;
use cake_core::knife::START_TILT_RAD;
use cake_core::replay::{build_placement_inputs, build_scripted_inputs};
use cake_core::sim::World;
use cake_server::catalog::Catalog;
use cake_server::protocol::{Member, POSE_RECORD_SIZE, PhaseName, PlayerId, ServerMessage, Standing, build_pose};
use cake_server::replay::calc_idle_input;
use cake_server::room::{Frame, OUTBOX_LEN, Room, RoomConfig, RoomEvent, create_outbox};
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::error::TryRecvError;

const CAKE: &str = "slice";
const ROUND_TICKS: u32 = 900;
/// 900 ticks at 60 Hz, then the grace for late inputs.
const ROUND_MS: u64 = 15_000;
const GRACE_MS: u64 = 2_000;
const RESULTS_HOLD_MS: u64 = 5_000;
/// Past the round's end, so a test may send a whole round's inputs at once.
const WHOLE_ROUND_LEAD_TICKS: u32 = 2 * ROUND_TICKS;

struct Client {
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
            for record in bytes.chunks_exact(POSE_RECORD_SIZE) {
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

fn create_room() -> Room {
    create_room_with_lead(WHOLE_ROUND_LEAD_TICKS)
}

fn create_room_with_lead(max_lead_ticks: u32) -> Room {
    let config = RoomConfig {
        round_ticks: ROUND_TICKS,
        load_timeout_ms: 10_000,
        deadline_grace_ms: GRACE_MS,
        results_hold_ms: RESULTS_HOLD_MS,
        max_lead_ticks,
    };
    Room::create(config, Catalog::create([(CAKE.to_string(), build_slice_missing_cake())]))
}

fn join(room: &mut Room, id: PlayerId, name: &str, now_ms: u64) -> Client {
    let (outbox, inbox) = create_outbox();
    room.on_event(RoomEvent::Join { id, name: name.to_string(), outbox }, now_ms);
    Client { id, inbox }
}

/// The host starts a round on the placeholder cake, every player loads it, and play starts at `now_ms`.
fn start_round(room: &mut Room, host: &Client, players: &[&Client], now_ms: u64) {
    room.on_event(RoomEvent::Start { id: host.id, cake: CAKE.to_string() }, now_ms);
    for player in players {
        room.on_event(RoomEvent::Ready { id: player.id }, now_ms);
    }
    room.step(now_ms);
}

fn send_inputs(room: &mut Room, client: &Client, inputs: &[Input], now_ms: u64) {
    for (tick, &input) in inputs.iter().enumerate() {
        room.on_event(RoomEvent::Input { id: client.id, tick: tick as u32, input }, now_ms);
    }
}

fn send_cut(room: &mut Room, client: &Client, tick: u32, now_ms: u64) {
    room.on_event(RoomEvent::Cut { id: client.id, tick }, now_ms);
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
        ServerMessage::Results { standings } => Some(standings.clone()),
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
    assert_eq!(messages[0], ServerMessage::Welcome { id: 3 });
    assert!(messages.contains(&ServerMessage::Round {
        cake: CAKE.into(),
        round_ticks: ROUND_TICKS,
        players: vec![1, 2]
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
    room.step(5_116 + RESULTS_HOLD_MS);
    room.on_event(RoomEvent::Start { id: 1, cake: CAKE.into() }, 11_000);
    assert!(carol.take_messages().contains(&ServerMessage::Round {
        cake: CAKE.into(),
        round_ticks: ROUND_TICKS,
        players: vec![1, 2, 3]
    }));
}

#[test]
fn host_passes_to_the_longest_present_player_when_the_host_leaves() {
    let mut room = create_room();
    let alice = join(&mut room, 1, "Alice", 0);
    let mut bob = join(&mut room, 2, "Bob", 0);
    let mut carol = join(&mut room, 3, "Carol", 0);
    room.on_event(RoomEvent::Leave { id: alice.id }, 100);
    let expected_room = ServerMessage::Room {
        phase: PhaseName::Lobby,
        host: Some(2),
        members: vec![Member { id: 2, name: "Bob".into() }, Member { id: 3, name: "Carol".into() }],
    };
    assert_eq!(bob.take_messages().last(), Some(&expected_room));

    room.on_event(RoomEvent::Start { id: carol.id, cake: CAKE.into() }, 200);
    assert!(matches!(carol.take_messages().last(), Some(ServerMessage::Error { .. })));
    room.on_event(RoomEvent::Start { id: bob.id, cake: CAKE.into() }, 200);
    assert!(carol.take_messages().contains(&ServerMessage::Round {
        cake: CAKE.into(),
        round_ticks: ROUND_TICKS,
        players: vec![2, 3]
    }));
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
fn start_is_refused_for_an_unknown_cake_and_while_a_round_is_on() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    room.on_event(RoomEvent::Start { id: 1, cake: "teapot".into() }, 0);
    assert_eq!(find_error(&alice.take_messages()), Some("no cake \"teapot\""));

    room.on_event(RoomEvent::Start { id: 1, cake: CAKE.into() }, 0);
    alice.take_messages();
    room.on_event(RoomEvent::Start { id: 1, cake: CAKE.into() }, 100);
    assert_eq!(alice.take_messages(), [ServerMessage::Error { message: "a round is already on".into() }]);
}

#[test]
fn random_start_plays_a_cake_from_the_catalog() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    room.on_event(RoomEvent::Start { id: 1, cake: "random".into() }, 0);
    assert!(alice.take_messages().contains(&ServerMessage::Round {
        cake: CAKE.into(),
        round_ticks: ROUND_TICKS,
        players: vec![1]
    }));
}

#[test]
fn round_starts_without_a_player_still_loading_once_the_load_timeout_passes() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    let _bob = join(&mut room, 2, "Bob", 0);
    room.on_event(RoomEvent::Start { id: 1, cake: CAKE.into() }, 0);
    room.on_event(RoomEvent::Ready { id: 1 }, 100);
    room.step(9_999);
    assert!(!alice.take_messages().contains(&ServerMessage::Start { elapsed_ms: 0 }));

    room.step(10_000);
    assert!(alice.take_messages().contains(&ServerMessage::Start { elapsed_ms: 0 }));
}

#[test]
fn player_who_leaves_mid_round_drops_out_of_the_results() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    let bob = join(&mut room, 2, "Bob", 0);
    start_round(&mut room, &alice, &[&alice, &bob], 0);
    send_inputs(&mut room, &alice, &build_placement_inputs(), 100);
    send_cut(&mut room, &alice, 300, 100);
    room.step(116);
    room.on_event(RoomEvent::Leave { id: bob.id }, 200);
    room.step(216);

    let standings = find_results(&alice.take_messages()).unwrap();
    assert_eq!(standings.iter().map(|standing| standing.id).collect::<Vec<_>>(), [1]);
}

#[test]
fn late_joiner_after_a_player_who_cut_has_left_gets_the_round_without_the_leaver() {
    let mut room = create_room();
    let alice = join(&mut room, 1, "Alice", 0);
    let bob = join(&mut room, 2, "Bob", 0);
    start_round(&mut room, &alice, &[&alice, &bob], 0);
    send_inputs(&mut room, &alice, &build_placement_inputs(), 100);
    send_cut(&mut room, &alice, 300, 100);
    room.step(116);
    room.on_event(RoomEvent::Leave { id: alice.id }, 200);

    let mut carol = join(&mut room, 3, "Carol", 300);
    let messages = carol.take_messages();
    assert!(messages.contains(&ServerMessage::Round { cake: CAKE.into(), round_ticks: ROUND_TICKS, players: vec![2] }));
    assert!(messages.contains(&ServerMessage::Start { elapsed_ms: 300 }));
    assert!(!messages.iter().any(|message| matches!(message, ServerMessage::Cut { .. })));
}

#[test]
fn an_input_or_cut_stamped_beyond_the_lead_over_the_round_clock_is_refused() {
    let mut room = create_room_with_lead(30);
    let mut alice = join(&mut room, 1, "Alice", 0);
    start_round(&mut room, &alice, &[&alice], 0);
    // At 100 ms the round's clock reads tick 6, so stamps up to tick 36 are taken.
    room.on_event(RoomEvent::Input { id: 1, tick: 36, input: calc_idle_input() }, 100);
    room.on_event(RoomEvent::Input { id: 1, tick: 37, input: calc_idle_input() }, 100);
    room.on_event(RoomEvent::Cut { id: 1, tick: 899 }, 100);
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
    room.on_event(RoomEvent::Start { id: bob.id, cake: CAKE.into() }, 20_000);

    assert_eq!(bob.take_frames().len(), OUTBOX_LEN);
    assert_eq!(bob.inbox.try_recv(), Err(TryRecvError::Disconnected));
    let expected_room = ServerMessage::Room {
        phase: PhaseName::Playing,
        host: Some(1),
        members: vec![Member { id: 1, name: "Alice".into() }],
    };
    assert_eq!(alice_messages.last(), Some(&expected_room));
}

#[test]
fn a_hash_that_differs_from_the_replay_is_reported_to_its_client() {
    let mut room = create_room();
    let mut alice = join(&mut room, 1, "Alice", 0);
    start_round(&mut room, &alice, &[&alice], 0);
    let inputs = build_placement_inputs();
    let right_hash = replay_locally(&inputs[..60]).calc_hash();
    send_inputs(&mut room, &alice, &inputs[..60], 100);
    room.on_event(RoomEvent::Hash { id: 1, tick: 60, hash: right_hash }, 100);
    room.step(116);
    assert!(!alice.take_messages().iter().any(|message| matches!(message, ServerMessage::Desync { .. })));

    room.on_event(RoomEvent::Hash { id: 1, tick: 60, hash: right_hash ^ 1 }, 200);
    room.step(216);
    assert_eq!(alice.take_messages(), [ServerMessage::Desync { tick: 60 }]);
}
