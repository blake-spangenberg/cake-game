//! Prints how a cake moves through a scripted carry, place, and stand turns, for tuning the
//! physics against jitter: every 10 ticks, its position, speed, and spin. It carries the cake onto
//! the stand by its highest point, lets go, turns the stand half a turn over a second, then makes
//! eight quarter turns at full speed, each from a standstill. It ends with how long the cake took to
//! come to rest after the release and after the start of each turn, and how far it lay from the
//! stand's axis before each quarter turn, which a cake riding the stand keeps. Takes a baked
//! `.cake` file as its argument, else uses the placeholder.

#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "scripted inputs and statistics over counts far below f32's whole-number range"
)]

use cake_core::input::{ANGLE_UNIT_RAD, HandInput, Input, KnifeInput, quantize_radians};
use cake_core::knife::START_TILT_RAD;
use cake_core::replay::calc_spawned_top_grab_at;
use cake_core::sim::{TICKS_PER_S, World};

mod common;

const CARRY_END_TICK: i32 = 150;
const RELEASE_TICK: i32 = 300;
const TURN_START_TICK: i32 = 500;
const TURN_TICKS: i32 = 60;
const QUARTER_TURNS_START_TICK: i32 = 800;
const QUARTER_TURN_TICKS: i32 = 90;
const QUARTER_TURN_LEN: i32 = 8;
const END_TICK: i32 = QUARTER_TURNS_START_TICK + QUARTER_TURN_LEN * QUARTER_TURN_TICKS;
/// A cake whose pose moves less than this in a tick counts as still.
const STILL_M: f32 = 1e-5;
const STILL_RAD: f32 = 1e-4;

fn main() {
    let model = common::fetch_model();
    let grab_at = calc_spawned_top_grab_at(&model);
    let start_tilt = quantize_radians(START_TILT_RAD);
    let mut world = World::create(model, START_TILT_RAD, None);
    let quarter_starts = (0..QUARTER_TURN_LEN).map(|i| QUARTER_TURNS_START_TICK + i * QUARTER_TURN_TICKS);
    let stage_starts: Vec<i32> = [RELEASE_TICK, TURN_START_TICK].into_iter().chain(quarter_starts).collect();
    let mut last_moving_ticks = stage_starts.clone();
    let mut axis_distances_m = Vec::new();
    let mut last = world.get_cake_pose();
    println!("tick holding      x      y      z   speed_m_s  spin_rad_s");
    for tick in 0..END_TICK {
        if tick >= QUARTER_TURNS_START_TICK && (tick - QUARTER_TURNS_START_TICK) % QUARTER_TURN_TICKS == 0 {
            axis_distances_m.push(format!("{:.3}", last.translation.with_y(0.0).length()));
        }
        let progress = f64::from(tick.min(CARRY_END_TICK)) / f64::from(CARRY_END_TICK);
        let target_xz =
            [f64::from(grab_at[0]) * (1.0 - progress), f64::from(grab_at[2]) * (1.0 - progress)].map(|v| v as i32);
        let hand = (tick < RELEASE_TICK).then_some(HandInput { grab_at, target_xz });
        let input =
            Input { hand, knife: KnifeInput { tilt: start_tilt, lower: 0 }, stand_angle: calc_stand_angle(tick) };
        world.step(&input);
        let pose = world.get_cake_pose();
        let step_m = pose.translation.distance(last.translation);
        let step_rad = common::calc_turn_rad(last.rotation, pose.rotation);
        last = pose;
        let stage_idx = stage_starts.iter().rposition(|&start| start <= tick);
        if let Some(stage_idx) = stage_idx.filter(|_| step_m > STILL_M || step_rad > STILL_RAD) {
            last_moving_ticks[stage_idx] = tick + 1;
        }
        if tick % 10 == 9 {
            let [x, y, z] = pose.translation.to_array();
            let (speed_m_s, spin_rad_s) = (step_m * TICKS_PER_S as f32, step_rad * TICKS_PER_S as f32);
            let holding = world.is_holding();
            println!("{:4} {holding:7} {x:6.3} {y:6.3} {z:6.3} {speed_m_s:11.5} {spin_rad_s:11.5}", tick + 1);
        }
    }
    let rest_s: Vec<String> = stage_starts
        .iter()
        .zip(&last_moving_ticks)
        .map(|(start, last_moving)| format!("{:.2}", (last_moving - start) as f32 / TICKS_PER_S as f32))
        .collect();
    println!("moving after the release, the half turn, and each quarter turn, s: {}", rest_s.join(" "));
    println!("off the stand's axis before each quarter turn, m: {}", axis_distances_m.join(" "));
}

/// The stand's input at `tick`: a half turn eased in over [`TURN_TICKS`], then quarter turns asked
/// for all at once.
fn calc_stand_angle(tick: i32) -> i32 {
    let turn = f64::from((tick - TURN_START_TICK).clamp(0, TURN_TICKS)) / f64::from(TURN_TICKS);
    let quarters = ((tick - QUARTER_TURNS_START_TICK).div_euclid(QUARTER_TURN_TICKS) + 1).clamp(0, QUARTER_TURN_LEN);
    let angle_rad = turn * core::f64::consts::PI + f64::from(quarters) * core::f64::consts::FRAC_PI_2;
    (angle_rad / ANGLE_UNIT_RAD) as i32
}
