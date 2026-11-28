//! Times the core's tick while the knife is pressed into a cake, tilting while the stand turns:
//! the costliest input the core has. Usage: `knife_cost <file.cake>...`. Two rounds per cake carry
//! it onto the stand and then press: `wall` tilts the lowered knife from its start against the
//! cake's side, as `room-cost`'s pressing round does, and `top` presses the upright knife down into
//! the cake's top and rocks it there. Prints each round's pressing step cost, how many pressing
//! ticks the edge rested on the cake, and a hash over every tick's world, which must not change
//! when only the speed of the knife's scans does. `.agent/scripts/knife-cost-wasm.mjs` repeats it
//! in WASM.

#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "percentiles and scripted inputs over counts far below f64's whole-number range"
)]

use std::time::{Duration, Instant};

use cake_core::cakes::CakeModel;
use cake_core::input::{HandInput, Input, KnifeInput, quantize_meters, quantize_radians};
use cake_core::knife::START_TILT_RAD;
use cake_core::replay::calc_spawned_top_grab_at;
use cake_core::sim::World;
use cake_core::triangle_tree::TriangleTree;

const CARRY_TICKS: i32 = 300;
const PRESS_TICKS: i32 = 600;
/// How far the `top` round rocks the knife either side of upright, in the input's angle units.
const ROCK_TILT: i32 = 30_000;
const TILT_PER_TICK: i32 = 150;
const STAND_PER_TICK: i32 = 400;
/// How near the cake the edge must rest to count as pressed into it.
const TOUCH_M: f64 = 1e-3;
const BUILD_REPEATS: u32 = 10;

#[derive(Clone, Copy)]
enum Round {
    Wall,
    Top,
}

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    assert!(!paths.is_empty(), "usage: knife_cost <file.cake>...");
    for path in &paths {
        let model = CakeModel::decode(&std::fs::read(path).expect("cake file readable")).expect("cake file playable");
        let start = Instant::now();
        for _ in 0..BUILD_REPEATS {
            std::hint::black_box(TriangleTree::build(&model.mesh.positions, &model.mesh.triangles));
        }
        let build_cost = start.elapsed() / BUILD_REPEATS;
        let (vertex_len, triangle_len) = (model.mesh.positions.len(), model.mesh.triangles.len());
        println!("{path}: {vertex_len} vertices, {triangle_len} triangles, tree built in {build_cost:?}");
        for (name, round) in [("wall", Round::Wall), ("top", Round::Top)] {
            report(name, round, model.clone());
        }
    }
}

fn report(name: &str, round: Round, model: CakeModel) {
    let inputs = build_inputs(round, &model);
    let start = Instant::now();
    let mut world = World::create(model, START_TILT_RAD, None);
    let create_cost = start.elapsed();
    let mut press_costs = Vec::new();
    let mut hash = 0u64;
    let (mut touching_ticks, mut blocked_ticks) = (0, 0);
    for (tick, input) in inputs.iter().enumerate() {
        let start = Instant::now();
        world.step(input);
        let cost = start.elapsed();
        hash = hash.rotate_left(5) ^ world.calc_hash();
        if tick < CARRY_TICKS as usize {
            continue;
        }
        press_costs.push(cost);
        touching_ticks += u32::from(world.calc_cake_touch_lower_m() <= world.get_knife().lower_m + TOUCH_M);
        blocked_ticks += u32::from(world.is_stand_blocked());
    }
    press_costs.sort();
    let at = |fraction: f64| press_costs[((press_costs.len() - 1) as f64 * fraction) as usize];
    let mean = press_costs.iter().sum::<Duration>() / press_costs.len() as u32;
    println!(
        "  {name}: world built in {create_cost:?}, edge on the cake {touching_ticks} ticks, stand blocked {blocked_ticks}"
    );
    println!("    pressing step: mean {mean:?} median {:?} p95 {:?} max {:?}", at(0.5), at(0.95), at(1.0));
    println!("    hash {hash:016x}");
}

/// Carries the cake onto the stand, then holds the knife pressed into it while tilting it and
/// turning the stand every tick.
fn build_inputs(round: Round, model: &CakeModel) -> Vec<Input> {
    let carry = HandInput { grab_at: calc_spawned_top_grab_at(model), target_xz: [0, quantize_meters(0.02)] };
    let start_tilt = quantize_radians(START_TILT_RAD);
    let carrying = (0..CARRY_TICKS).map(move |tick| {
        let hand = (20..240).contains(&tick).then_some(carry);
        let tilt = match round {
            Round::Wall => start_tilt,
            Round::Top => start_tilt - (i64::from(start_tilt) * i64::from(tick) / i64::from(CARRY_TICKS - 1)) as i32,
        };
        Input { hand, knife: KnifeInput { tilt, lower: 0 }, stand_angle: 0 }
    });
    let pressing = (0..PRESS_TICKS).map(move |pressed_ticks| {
        let tilt = match round {
            Round::Wall => start_tilt + TILT_PER_TICK * pressed_ticks,
            Round::Top => {
                let phase = (TILT_PER_TICK * pressed_ticks + ROCK_TILT) % (4 * ROCK_TILT);
                ROCK_TILT - (phase - 2 * ROCK_TILT).abs()
            }
        };
        let knife = KnifeInput { tilt, lower: quantize_meters(0.3) };
        Input { hand: None, knife, stand_angle: STAND_PER_TICK * pressed_ticks }
    });
    carrying.chain(pressing).collect()
}
