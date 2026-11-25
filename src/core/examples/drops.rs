//! Grabs a cake at a spread of surface points, lifts it, and drops it on the table, printing per
//! grab how far its centroid traveled after the drop and when it last moved, for tuning against
//! rolling away and wobbling forever. Usage: `drops [file.cake [grab_count]]`, the placeholder with
//! no file. It grabs at `grab_count` vertices spread through the mesh, every 7th vertex by default,
//! which runs for tens of minutes on a 40000-triangle cake.

#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "statistics printed to a few digits, over counts far below f32's whole-number range"
)]

use cake_core::input::{HandInput, Input, KnifeInput, quantize_meters};
use cake_core::knife::START_TILT_RAD;
use cake_core::sim::{Cake, TICKS_PER_S, World};
use cake_core::table::CAKE_SPAWN;

mod common;

const HOLD_TICKS: u32 = 90;
const WATCH_TICKS: u32 = 600;
const DEFAULT_GRAB_STRIDE: usize = 7;
/// Below these the cake counts as still.
const STILL_SPEED_M_PER_S: f32 = 0.002;
const STILL_SPIN_RAD_PER_S: f32 = 0.05;

fn main() {
    let model = common::fetch_model();
    let grab_stride = match std::env::args().nth(2) {
        None => DEFAULT_GRAB_STRIDE,
        Some(arg) => match arg.parse::<usize>() {
            Ok(grab_count) if grab_count > 0 => (model.mesh.positions.len() / grab_count).max(1),
            _ => common::exit_with(&format!("grab_count must be a positive whole number, not {arg}")),
        },
    };
    let cake = std::sync::Arc::new(Cake::build(model));
    let tick_s = 1.0 / TICKS_PER_S as f32;
    println!("grab_x  grab_y  grab_z  roll_m  last_moving_s");
    let (mut worst_roll_m, mut worst_settle_s) = (0.0_f32, 0.0_f32);
    let (mut grab_len, mut moving_at_end_len) = (0, 0);
    for local in cake.get_model().mesh.positions.iter().step_by(grab_stride) {
        let grab = [CAKE_SPAWN.x as f32 + local.x, local.y, CAKE_SPAWN.z as f32 + local.z];
        let grab_at = grab.map(|m| quantize_meters(f64::from(m)));
        let target_xz = [grab_at[0], grab_at[2]];
        let knife = KnifeInput { tilt: 0, lower: 0 };
        let mut world = World::create(cake.clone(), START_TILT_RAD, None);
        for _ in 0..HOLD_TICKS {
            world.step(&Input { hand: Some(HandInput { grab_at, target_xz }), knife, stand_angle: 0 });
        }
        let calc_centroid = |world: &World| {
            let positions = world.get_cake_positions();
            positions.iter().copied().sum::<rapier3d::glamx::DVec3>() / positions.len() as f64
        };
        let dropped_at = calc_centroid(&world);
        let mut last = world.get_cake_pose();
        let mut last_moving_tick = 0;
        for tick in 0..WATCH_TICKS {
            world.step(&Input { hand: None, knife, stand_angle: 0 });
            let pose = world.get_cake_pose();
            let speed = pose.translation.distance(last.translation) / tick_s;
            let spin = common::calc_turn_rad(last.rotation, pose.rotation) / tick_s;
            if speed > STILL_SPEED_M_PER_S || spin > STILL_SPIN_RAD_PER_S {
                last_moving_tick = tick + 1;
            }
            last = pose;
        }
        let roll_m = (calc_centroid(&world) - dropped_at).with_y(0.0).length() as f32;
        let settle_s = last_moving_tick as f32 * tick_s;
        (worst_roll_m, worst_settle_s) = (worst_roll_m.max(roll_m), worst_settle_s.max(settle_s));
        grab_len += 1;
        moving_at_end_len += u32::from(last_moving_tick == WATCH_TICKS);
        println!("{:6.3}  {:6.3}  {:6.3}  {roll_m:6.3}  {settle_s:13.2}", local.x, local.y, local.z);
    }
    println!(
        "{moving_at_end_len} of {grab_len} grabs still moving at the end; worst roll {worst_roll_m:.3} m, worst settle \
         {worst_settle_s:.2} s of {:.0} s watched",
        WATCH_TICKS as f32 * tick_s
    );
}
