//! Times the core's tick while a cake is thrown about: grabbed at a different point each throw,
//! carried briskly back and forth across the table at the hand's full speed, released while
//! still moving over the stand or the table, and left to tumble and settle. Usage:
//! `throw_cost <file.cake>...`. Prints, per cake, the step cost while held, in the first second
//! after each release (landing), and after that (resting), over several repeats of the script,
//! plus how many ticks the cake still moved in each, how long each throw took to come to rest,
//! and a hash over every tick's world, which `.agent/scripts/throw-cost-wasm.mjs` repeats in
//! WASM. A step's cost includes `is_cut_through`, which the client asks every frame.

#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "percentiles and statistics over counts far below f32's whole-number range"
)]

use std::time::{Duration, Instant};

use cake_core::input::{HandInput, Input, KnifeInput, quantize_meters, quantize_radians};
use cake_core::knife::START_TILT_RAD;
use cake_core::sim::{Cake, TICKS_PER_S, World};
use rapier3d::glamx::DVec3;

mod common;

const REPEATS: u32 = 3;
/// Ticks of zigzag carry before each throw, switching target every [`ZIGZAG_LEG_TICKS`].
const ZIGZAG_TICKS: u32 = 120;
const ZIGZAG_LEG_TICKS: u32 = 30;
/// Ticks the hand swings toward the throw's target before letting go mid-swing.
const SWING_TICKS: u32 = 30;
const FREE_TICKS: u32 = 180;
/// Ticks after a release counted as the landing, before the cake counts as resting.
const LANDING_TICKS: u32 = TICKS_PER_S;
/// A cake whose pose moves less than this in a tick counts as still.
const STILL_M: f32 = 1e-5;
const STILL_RAD: f32 = 1e-4;
/// Zigzag corners, in meters on the table.
const ZIGZAG_XZ_M: [[f64; 2]; 2] = [[0.5, 0.3], [-0.5, -0.3]];
/// Where each throw swings toward, alternating: past the stand's center and out over the table.
const THROW_XZ_M: [[f64; 2]; 2] = [[-0.2, -0.1], [0.45, -0.25]];
/// Directions the grab point lies farthest along, one per throw, in world coordinates.
const GRAB_DIRECTIONS: [DVec3; 6] =
    [DVec3::Y, DVec3::NEG_Y, DVec3::X, DVec3::NEG_Z, DVec3::new(-1.0, 1.0, 1.0), DVec3::new(1.0, -1.0, -1.0)];

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Held,
    Landing,
    Resting,
}

#[derive(Default)]
struct StageCosts {
    costs: Vec<Duration>,
    moving_ticks: u32,
}

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        common::exit_with("usage: throw_cost <file.cake>...");
    }
    for path in &paths {
        let model = common::fetch_cake_file(path);
        let hull_lens: Vec<usize> = model.hulls.iter().map(Vec::len).collect();
        println!("{path}: {} triangles, {} hulls {hull_lens:?}", model.mesh.triangles.len(), model.hulls.len());
        let cake = std::sync::Arc::new(Cake::build(model));
        let mut stages: [StageCosts; 3] = Default::default();
        for _ in 0..REPEATS {
            let run = run_script(&cake, &mut stages);
            println!("  repeat: mean step {:?}, hash {:016x}", run.mean_cost, run.hash);
            println!("    per throw: {}", run.throw_ends.join("; "));
        }
        for (name, stage) in ["held", "landing", "resting"].iter().zip(&mut stages) {
            report(name, stage);
        }
    }
}

fn report(name: &str, stage: &mut StageCosts) {
    let costs = &mut stage.costs;
    costs.sort();
    let at = |fraction: f64| costs[((costs.len() - 1) as f64 * fraction) as usize];
    let mean = costs.iter().sum::<Duration>() / costs.len() as u32;
    let moving_share = f64::from(stage.moving_ticks) / costs.len() as f64;
    println!(
        "  {name:8} {} ticks, {:.0}% moving: mean {mean:?} median {:?} p95 {:?} max {:?}",
        costs.len(),
        moving_share * 100.0,
        at(0.5),
        at(0.95),
        at(1.0)
    );
}

struct ScriptRun {
    /// Over every tick's world.
    hash: u64,
    mean_cost: Duration,
    /// Per throw, the ticks after release until the cake last moved, or how it still moved at
    /// the end.
    throw_ends: Vec<String>,
}

/// Plays every throw once, adding each tick's cost to its stage.
fn run_script(cake: &std::sync::Arc<Cake>, stages: &mut [StageCosts; 3]) -> ScriptRun {
    let mut world = World::create(cake.clone(), START_TILT_RAD, None);
    let knife = KnifeInput { tilt: quantize_radians(START_TILT_RAD), lower: 0 };
    let (mut hash, mut total, mut ticks) = (0u64, Duration::ZERO, 0u32);
    let mut throw_ends = Vec::new();
    for (throw_idx, &direction) in GRAB_DIRECTIONS.iter().enumerate() {
        let mut last_moving_tick = 0;
        let (mut last_speed_m_s, mut last_spin_rad_s, mut last_y_m) = (0.0, 0.0, 0.0);
        let grab_at = calc_farthest_vertex(world.get_cake_positions(), direction).map(quantize_meters);
        let throw_xz = THROW_XZ_M[throw_idx % THROW_XZ_M.len()];
        let held_ticks = ZIGZAG_TICKS + SWING_TICKS;
        for tick in 0..held_ticks + FREE_TICKS {
            let hand = (tick < held_ticks).then(|| {
                let target_xz_m = match tick {
                    t if t < ZIGZAG_TICKS => ZIGZAG_XZ_M[(t / ZIGZAG_LEG_TICKS) as usize % ZIGZAG_XZ_M.len()],
                    _ => throw_xz,
                };
                HandInput { grab_at, target_xz: target_xz_m.map(quantize_meters) }
            });
            let stage = match tick {
                t if t < held_ticks => Stage::Held,
                t if t < held_ticks + LANDING_TICKS => Stage::Landing,
                _ => Stage::Resting,
            };
            let before = world.get_cake_pose();
            let start = Instant::now();
            world.step(&Input { hand, knife, stand_angle: 0 });
            // The client asks this every frame to light `Cut!`, so it counts toward the frame.
            std::hint::black_box(world.is_cut_through());
            let cost = start.elapsed();
            let after = world.get_cake_pose();
            let step_m = before.translation.distance(after.translation);
            let step_rad = common::calc_turn_rad(before.rotation, after.rotation);
            let is_moving = step_m > STILL_M || step_rad > STILL_RAD;
            let stage_costs = &mut stages[stage as usize];
            stage_costs.costs.push(cost);
            stage_costs.moving_ticks += u32::from(is_moving);
            if is_moving {
                last_moving_tick = tick;
            }
            last_speed_m_s = step_m * TICKS_PER_S as f32;
            last_spin_rad_s = step_rad * TICKS_PER_S as f32;
            last_y_m = after.translation.y;
            hash = hash.rotate_left(5) ^ world.calc_hash();
            total += cost;
            ticks += 1;
        }
        let is_still_at_end = last_moving_tick + 1 < held_ticks + FREE_TICKS;
        throw_ends.push(if is_still_at_end {
            format!("rests after {}", (last_moving_tick + 1).saturating_sub(held_ticks))
        } else {
            format!("moving at y {last_y_m:.3} m: {last_speed_m_s:.4} m/s, {last_spin_rad_s:.3} rad/s")
        });
    }
    ScriptRun { hash, mean_cost: total / ticks, throw_ends }
}

fn calc_farthest_vertex(positions: &[DVec3], direction: DVec3) -> [f64; 3] {
    let farthest = positions.iter().max_by(|a, b| a.dot(direction).total_cmp(&b.dot(direction)));
    farthest.expect("a cake has vertices").to_array()
}
