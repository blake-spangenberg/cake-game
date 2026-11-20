//! A scripted round for the determinism test, replayed natively and in WASM alike.

use crate::cakes::build_slice_missing_cake;
use crate::input::{ANGLE_UNIT_RAD, HandInput, Input, KnifeInput, POSITION_UNIT_M};
use crate::knife::START_KNIFE;
use crate::sim::{Phase, World};
use crate::table::CAKE_SPAWN;

pub const CHECKPOINT_INTERVAL_TICKS: usize = 30;
const SCRIPT_TICKS: usize = 600;

/// Grabs the placeholder cake by its highest point, carries it to the table's center, drops it,
/// then sweeps the knife while lowering it, ending without a cut.
pub fn build_scripted_inputs() -> Vec<Input> {
    let model = build_slice_missing_cake();
    let top = model.mesh.positions.iter().copied().fold(model.mesh.positions[0], |a, b| if b.y > a.y { b } else { a });
    let grab_at = [CAKE_SPAWN.x + top.x as f64, top.y as f64, CAKE_SPAWN.z + top.z as f64]
        .map(|m| (m / POSITION_UNIT_M).round() as i32);
    let start_tilt = (START_KNIFE.tilt_rad / ANGLE_UNIT_RAD).round() as i32;
    let start_heading = (START_KNIFE.heading_rad / ANGLE_UNIT_RAD).round() as i32;
    (0..SCRIPT_TICKS)
        .map(|tick| {
            let tick = tick as i32;
            let hand = (20..240).contains(&tick).then_some(HandInput { grab_at, target_xz: [0, 2_000] });
            let knife = KnifeInput {
                tilt: start_tilt - 300 * tick.min(400),
                heading: start_heading + 500 * tick,
                is_lowering: tick >= 300,
            };
            Input { hand, knife }
        })
        .collect()
}

/// Replays `inputs` on a fresh world, returning its hash every [`CHECKPOINT_INTERVAL_TICKS`],
/// then the bits of the cut's two volumes when cut at the end.
pub fn replay(inputs: &[Input]) -> Vec<u64> {
    let mut world = World::create(build_slice_missing_cake(), START_KNIFE);
    let mut checkpoints = Vec::new();
    for (idx, input) in inputs.iter().enumerate() {
        world.step(input);
        if (idx + 1) % CHECKPOINT_INTERVAL_TICKS == 0 {
            checkpoints.push(world.calc_hash());
        }
    }
    let split = world.cut();
    assert!(matches!(world.get_phase(), Phase::Cut { .. }));
    checkpoints.extend([split.above_m3.to_bits(), split.below_m3.to_bits()]);
    checkpoints
}
