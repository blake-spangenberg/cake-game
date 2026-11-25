//! Times the room's per-tick step with many players against the 60 Hz budget. Usage:
//! `room-cost <file.cake> [players] [typical|pressing]`. A typical round carries the cake onto
//! the stand, tilts the knife, lowers it to check the line, turns the stand, and checks again.
//! A pressing round holds the knife pressed into the cake while tilting it and turning the stand
//! every tick, the costliest input the core has.

use std::time::{Duration, Instant};

use cake_core::cakes::CakeModel;
use cake_core::input::{HandInput, Input, KnifeInput, quantize_meters, quantize_radians};
use cake_core::knife::START_TILT_RAD;
use cake_core::replay::calc_spawned_top_grab_at;
use cake_core::sim::{ROUND_TICKS, TICKS_PER_S};
use cake_server::catalog::Catalog;
use cake_server::replay::calc_idle_input;
use cake_server::room::{Room, RoomConfig, RoomEvent, create_outbox};

const DEFAULT_PLAYERS: u32 = 12;
const HASH_INTERVAL_TICKS: u32 = 60;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: room-cost <file.cake> [players] [typical|pressing]");
    let player_len: u32 = args.next().map_or(DEFAULT_PLAYERS, |len| len.parse().expect("players: a count"));
    let scenario = args.next().unwrap_or_else(|| "typical".to_string());
    let model = CakeModel::decode(&std::fs::read(&path).expect("cake file readable")).expect("cake file playable");
    let inputs = match scenario.as_str() {
        "typical" => build_typical_inputs(&model),
        "pressing" => build_pressing_inputs(&model),
        _ => panic!("scenario: expected typical or pressing, got {scenario}"),
    };
    let mut room = Room::create(RoomConfig::default(), Catalog::create([("cake".to_string(), model)]));
    let mut inboxes = Vec::new();
    for id in 1..=player_len {
        let (outbox, inbox) = create_outbox();
        room.on_event(RoomEvent::Join { id, name: format!("P{id}"), outbox }, 0);
        inboxes.push(inbox);
    }
    let start = Instant::now();
    room.on_event(RoomEvent::Start { id: 1, cake: "cake".to_string() }, 0);
    let start_cost = start.elapsed();
    for id in 1..=player_len {
        room.on_event(RoomEvent::Ready { id }, 0);
    }
    room.step(0);

    let mut step_costs = Vec::new();
    for (tick, &input) in inputs.iter().enumerate() {
        let tick = tick as u32;
        let now_ms = tick as u64 * 1000 / TICKS_PER_S as u64;
        let start = Instant::now();
        for id in 1..=player_len {
            room.on_event(RoomEvent::Input { id, tick, input }, now_ms);
            if (tick + 1).is_multiple_of(HASH_INTERVAL_TICKS) {
                // A wrong hash costs the server the same check as a right one.
                room.on_event(RoomEvent::Hash { id, tick: tick + 1, hash: 0 }, now_ms);
            }
        }
        room.step(now_ms);
        step_costs.push(start.elapsed());
        for inbox in &mut inboxes {
            while inbox.try_recv().is_ok() {}
        }
    }
    step_costs.sort();
    let at = |fraction: f64| step_costs[((step_costs.len() - 1) as f64 * fraction) as usize];
    let mean = step_costs.iter().sum::<Duration>() / step_costs.len() as u32;
    let budget = Duration::from_secs(1) / TICKS_PER_S;
    println!("{scenario} round, {player_len} players, {} steps, budget {budget:?}", step_costs.len());
    println!("round start (worlds built): {start_cost:?}");
    // A step over budget delays the replay, whose inputs queue, so the mean decides whether the
    // server keeps up over a round, and the tail how far behind it falls.
    println!("step: mean {mean:?} median {:?} p95 {:?} p99 {:?} max {:?}", at(0.5), at(0.95), at(0.99), at(1.0));
}

fn build_typical_inputs(model: &CakeModel) -> Vec<Input> {
    let carry = HandInput { grab_at: calc_spawned_top_grab_at(model), target_xz: [0, quantize_meters(0.02)] };
    let start_tilt = quantize_radians(START_TILT_RAD);
    let aimed_tilt = quantize_radians(-0.2);
    let checked_lower = quantize_meters(0.15);
    let turned_stand = quantize_radians(0.5);
    let ease = |tick: i32, from: i32, to: i32, start: i32, len: i32| {
        from + ((to - from) as i64 * (tick - start).clamp(0, len) as i64 / len as i64) as i32
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
