//! Times the room's per-tick step with many players against the 60 Hz budget. Usage:
//! `room-cost <file.cake> [players] [typical|pressing|throwing] [silent players]`. A typical round
//! carries the cake onto the stand, tilts the knife, lowers it to check the line, turns the
//! stand, and checks again. A pressing round holds the knife pressed into the cake while tilting
//! it and turning the stand every tick, the costliest input the core has. A throwing round throws
//! the cake about the table as the core's `throw_cost` does. Silent players, the last of the
//! players, go quiet partway in, as a page that hangs does, in a pressing round with the knife
//! pressed in. The round runs on past its last tick until the results arrive.
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "percentiles and scripted inputs over counts far below every type's range"
)]

use std::time::{Duration, Instant};

use cake_core::cakes::CakeModel;
use cake_core::input::{HandInput, Input, KnifeInput, quantize_meters, quantize_radians};
use cake_core::knife::START_TILT_RAD;
use cake_core::replay::calc_spawned_top_grab_at;
use cake_core::sim::{ROUND_TICKS, TICKS_PER_S, World};
use cake_server::catalog::Catalog;
use cake_server::replay::calc_idle_input;
use cake_server::room::{Frame, HOST_PASSWORD, Room, RoomConfig, RoomEvent, create_outbox};
use tokio::sync::mpsc::Receiver;

const DEFAULT_PLAYERS: u32 = 12;
const HASH_INTERVAL_TICKS: u32 = 60;
/// The throwing round's script, as the core's `throw_cost` plays it: ticks of zigzag carry, each
/// leg [`ZIGZAG_LEG_TICKS`], then a swing toward the throw's target, let go mid-swing, then free.
const ZIGZAG_TICKS: u32 = 120;
const ZIGZAG_LEG_TICKS: u32 = 30;
const SWING_TICKS: u32 = 30;
const FREE_TICKS: u32 = 180;
/// Zigzag corners, in meters on the table.
const ZIGZAG_XZ_M: [[f64; 2]; 2] = [[0.5, 0.3], [-0.5, -0.3]];
/// Where each throw swings toward, alternating: past the stand's center and out over the table.
const THROW_XZ_M: [[f64; 2]; 2] = [[-0.2, -0.1], [0.45, -0.25]];
/// Directions the grab point lies farthest along, one per throw, in world coordinates.
const GRAB_DIRECTIONS: [[f64; 3]; 6] =
    [[0.0, 1.0, 0.0], [0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [-1.0, 1.0, 1.0], [1.0, -1.0, -1.0]];
/// Past the pressing round's first pressed tick, so a silent player's world holds the knife pressed in.
const SILENT_FROM_TICK: u32 = 400;
/// Past the round's deadline and the results' wait, so a round that never ends shows as a failure.
const MAX_EXTRA_S: u32 = 10;

fn main() {
    let mut args = std::env::args().skip(1);
    let path =
        args.next().expect("usage: room-cost <file.cake> [players] [typical|pressing|throwing] [silent players]");
    let player_len: u32 = args.next().map_or(DEFAULT_PLAYERS, |len| len.parse().expect("players: a count"));
    let scenario = args.next().unwrap_or_else(|| "typical".to_string());
    let silent_len: u32 = args.next().map_or(0, |len| len.parse().expect("silent players: a count"));
    assert!(silent_len <= player_len, "silent players: at most the players");
    let model = CakeModel::decode(&std::fs::read(&path).expect("cake file readable")).expect("cake file playable");
    let inputs = match scenario.as_str() {
        "typical" => build_typical_inputs(&model),
        "pressing" => build_pressing_inputs(&model),
        "throwing" => build_throwing_inputs(&model),
        _ => panic!("scenario: expected typical, pressing, or throwing, got {scenario}"),
    };
    // The judging reveal steps no world, so timing stops where it starts.
    let config = RoomConfig {
        judging_hold_ms: 0,
        judging_max_count_ms: 0,
        judging_after_ms: 0,
        judging_tail_ms: 0,
        ..RoomConfig::default()
    };
    let mut room = Room::create(config, Catalog::create([("cake".to_string(), model)]));
    let mut inboxes = Vec::new();
    for id in 1..=player_len {
        let (outbox, inbox) = create_outbox();
        let (assigned, _) = tokio::sync::oneshot::channel();
        let join = RoomEvent::Join {
            connection: id,
            name: format!("P{id}"),
            standup_preference: Some(0.5),
            seat: None,
            is_debug: false,
            outbox,
            assigned,
        };
        room.on_event(join, 0);
        inboxes.push(inbox);
    }
    room.on_event(RoomEvent::ClaimHost { connection: 1, password: HOST_PASSWORD.to_string() }, 0);
    let start = Instant::now();
    room.on_event(RoomEvent::Start { connection: 1 }, 0);
    let start_cost = start.elapsed();
    for connection in 1..=player_len {
        room.on_event(RoomEvent::Ready { connection }, 0);
    }
    room.step(0);

    let talking_len = player_len - silent_len;
    let mut step_costs = Vec::new();
    let mut is_over = false;
    let last_tick = ROUND_TICKS + MAX_EXTRA_S * TICKS_PER_S;
    for tick in 0..last_tick {
        let now_ms = u64::from(tick) * 1000 / u64::from(TICKS_PER_S);
        let start = Instant::now();
        if let Some(&input) = inputs.get(tick as usize) {
            let sending_len = if tick < SILENT_FROM_TICK { player_len } else { talking_len };
            send_tick(&mut room, sending_len, tick, input, now_ms);
        }
        room.step(now_ms);
        step_costs.push(start.elapsed());
        is_over = drain_for_results(&mut inboxes);
        if is_over {
            break;
        }
    }
    assert!(is_over, "no results within {MAX_EXTRA_S} s of the round's last tick");
    step_costs.sort();
    let max_cost = *step_costs.last().expect("a round steps at least once");
    let at = |fraction: f64| step_costs[((step_costs.len() - 1) as f64 * fraction) as usize];
    let mean = step_costs.iter().sum::<Duration>() / step_costs.len() as u32;
    let budget = Duration::from_secs(1) / TICKS_PER_S;
    println!(
        "{scenario} round, {player_len} players ({silent_len} silent), {} steps to the results, budget {budget:?}",
        step_costs.len()
    );
    println!("round start (worlds built): {start_cost:?}");
    // A step over budget delays the replay, whose inputs queue, so the mean decides whether the
    // server keeps up over a round, and the tail how far behind it falls.
    println!("step: mean {mean:?} median {:?} p95 {:?} p99 {:?} max {max_cost:?}", at(0.5), at(0.95), at(0.99));
}

/// Sends `input` for `tick` from each of the first `talking_len` players, and a hash once a second.
fn send_tick(room: &mut Room, talking_len: u32, tick: u32, input: Input, now_ms: u64) {
    for connection in 1..=talking_len {
        room.on_event(RoomEvent::Input { connection, tick, input }, now_ms);
        if (tick + 1).is_multiple_of(HASH_INTERVAL_TICKS) {
            // A wrong hash costs the server the same check as a right one.
            room.on_event(RoomEvent::Hash { connection, tick: tick + 1, hash: 0 }, now_ms);
        }
    }
}

/// Empties every inbox, returning whether the results came.
fn drain_for_results(inboxes: &mut [Receiver<Frame>]) -> bool {
    let mut is_over = false;
    for inbox in inboxes {
        while let Ok(frame) = inbox.try_recv() {
            is_over |= matches!(frame, Frame::Text(text) if text.starts_with("{\"type\":\"results\""));
        }
    }
    is_over
}

fn build_typical_inputs(model: &CakeModel) -> Vec<Input> {
    let carry = HandInput { grab_at: calc_spawned_top_grab_at(model), target_xz: [0, quantize_meters(0.02)] };
    let start_tilt = quantize_radians(START_TILT_RAD);
    let aimed_tilt = quantize_radians(-0.2);
    let checked_lower = quantize_meters(0.15);
    let turned_stand = quantize_radians(0.5);
    let ease = |tick: i32, from: i32, to: i32, start: i32, len: i32| {
        from + (i64::from(to - from) * i64::from((tick - start).clamp(0, len)) / i64::from(len)) as i32
    };
    (0..ROUND_TICKS as i32)
        .map(|tick| {
            let hand = (20..240).contains(&tick).then_some(carry);
            let tilt = ease(tick, start_tilt, aimed_tilt, 400, 120) + ease(tick, 0, 3_000, 1000, 100);
            let lower = match tick {
                520..640 => ease(tick, 0, checked_lower, 520, 60),
                640..760 => ease(tick, checked_lower, 0, 640, 60),
                940..1100 => ease(tick, 0, checked_lower, 940, 60),
                1100..1200 => ease(tick, checked_lower, 0, 1100, 60),
                _ => 0,
            };
            let stand_angle = ease(tick, 0, turned_stand, 760, 180);
            Input { hand, knife: KnifeInput { tilt, lower }, stand_angle }
        })
        .collect()
}

/// Throws the cake about as the core's `throw_cost` example does: each throw grabs it at the point
/// farthest along one of [`GRAB_DIRECTIONS`], zigzags it across the table at the hand's full
/// speed, lets go mid-swing toward the stand or out over the table, and leaves it to tumble for
/// 3 s. The grab points come from a world stepped alongside, as a client grabs the cake where it lies.
fn build_throwing_inputs(model: &CakeModel) -> Vec<Input> {
    let mut world = World::create(model.clone(), START_TILT_RAD, None);
    let knife = KnifeInput { tilt: quantize_radians(START_TILT_RAD), lower: 0 };
    let held_ticks = ZIGZAG_TICKS + SWING_TICKS;
    let mut inputs = Vec::new();
    for throw_idx in 0.. {
        if inputs.len() >= ROUND_TICKS as usize {
            break;
        }
        let direction = GRAB_DIRECTIONS[throw_idx % GRAB_DIRECTIONS.len()];
        let grab_at = calc_farthest_vertex(world.get_cake_positions().iter().copied().map(<[f64; 3]>::from), direction);
        let throw_xz_m = THROW_XZ_M[throw_idx % THROW_XZ_M.len()];
        for tick in 0..held_ticks + FREE_TICKS {
            let hand = (tick < held_ticks).then(|| {
                let target_xz_m = match tick {
                    t if t < ZIGZAG_TICKS => ZIGZAG_XZ_M[(t / ZIGZAG_LEG_TICKS) as usize % ZIGZAG_XZ_M.len()],
                    _ => throw_xz_m,
                };
                HandInput { grab_at: grab_at.map(quantize_meters), target_xz: target_xz_m.map(quantize_meters) }
            });
            let input = Input { hand, knife, stand_angle: 0 };
            world.step(&input);
            inputs.push(input);
        }
    }
    inputs.truncate(ROUND_TICKS as usize);
    inputs
}

fn calc_farthest_vertex(positions: impl Iterator<Item = [f64; 3]>, direction: [f64; 3]) -> [f64; 3] {
    let along = |p: &[f64; 3]| p[0] * direction[0] + p[1] * direction[1] + p[2] * direction[2];
    positions.max_by(|a, b| along(a).total_cmp(&along(b))).expect("a cake has vertices")
}

fn build_pressing_inputs(model: &CakeModel) -> Vec<Input> {
    let mut inputs = build_typical_inputs(model);
    let pressing_from = 300;
    for (tick, input) in inputs.iter_mut().enumerate().skip(pressing_from) {
        let pressed_ticks = (tick - pressing_from) as i32;
        let tilt = calc_idle_input().knife.tilt - 150 * (pressed_ticks % 600);
        input.knife = KnifeInput { tilt, lower: quantize_meters(0.3) };
        input.stand_angle = 400 * pressed_ticks;
    }
    inputs
}
