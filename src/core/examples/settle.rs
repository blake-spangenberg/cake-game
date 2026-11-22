//! Prints how the placeholder cake moves through a scripted carry, place, and stand turn, for
//! tuning the physics against jitter: every 10 ticks, its position, speed, and spin.

use cake_core::cakes::build_slice_missing_cake;
use cake_core::input::{ANGLE_UNIT_RAD, HandInput, Input, KnifeInput, quantize_radians};
use cake_core::knife::START_KNIFE;
use cake_core::replay::calc_spawned_top_grab_at;
use cake_core::sim::{TICKS_PER_S, World};

const CARRY_END_TICK: i32 = 150;
const RELEASE_TICK: i32 = 300;
const TURN_START_TICK: i32 = 500;
const TURN_TICKS: i32 = 60;
const END_TICK: i32 = 800;

fn main() {
    let model = build_slice_missing_cake();
    let grab_at = calc_spawned_top_grab_at(&model);
    let start_tilt = quantize_radians(START_KNIFE.tilt_rad);
    let mut world = World::create(model, START_KNIFE, None);
    let mut last = world.get_cake_pose();
    println!("tick holding      x      y      z   speed_m_s  spin_rad_s");
    for tick in 0..END_TICK {
        let progress = (tick.min(CARRY_END_TICK) as f64) / CARRY_END_TICK as f64;
        let target_xz = [grab_at[0] as f64 * (1.0 - progress), grab_at[2] as f64 * (1.0 - progress)].map(|v| v as i32);
        let hand = (tick < RELEASE_TICK).then_some(HandInput { grab_at, target_xz });
        let turn = (tick - TURN_START_TICK).clamp(0, TURN_TICKS) as f64 / TURN_TICKS as f64;
        let stand_angle = (turn * core::f64::consts::PI / ANGLE_UNIT_RAD) as i32;
        world.step(&Input { hand, knife: KnifeInput { tilt: start_tilt, lower: 0 }, stand_angle });
        let pose = world.get_cake_pose();
        let speed_m_s = pose.translation.distance(last.translation) * TICKS_PER_S as f32;
        let spin_rad_s = pose.rotation.angle_between(last.rotation) * TICKS_PER_S as f32;
        last = pose;
        if tick % 10 == 9 {
            let [x, y, z] = pose.translation.to_array();
            let holding = world.is_holding();
            println!("{:4} {holding:7} {x:6.3} {y:6.3} {z:6.3} {speed_m_s:11.5} {spin_rad_s:11.5}", tick + 1);
        }
    }
}
