//! A scripted round for the determinism test, replayed natively and in WASM alike.

use rapier3d::glamx::DVec3;

use crate::cakes::{CakeModel, build_slice_missing_cake};
use crate::input::{HandInput, Input, KnifeInput, quantize_meters, quantize_radians};
use crate::knife::START_KNIFE;
use crate::sim::{Phase, World};
use crate::table::CAKE_SPAWN;

pub const CHECKPOINT_INTERVAL_TICKS: usize = 30;
const PLACE_TICKS: usize = 300;
const PLACE_AND_TURN_TICKS: usize = 600;
const RAISE_KNIFE_TICKS: usize = 20;
const CARRY_OFF_TICKS: usize = 120;
const DROP_TICKS: usize = 60;
const CARRY_OFF_TARGET_M: [f64; 2] = [0.45, 0.15];

/// The script's first stretch: grabs the placeholder cake by its highest point, carries it onto
/// the stand, places it, and lets go.
pub fn build_placement_inputs() -> Vec<Input> {
    build_place_and_turn_inputs(PLACE_TICKS)
}

/// The placement, then turns the stand while tilting and lowering the knife, then raises the
/// knife, picks the cake up off the stand by its highest point, carries it away, and drops it,
/// ending without a cut.
pub fn build_scripted_inputs() -> Vec<Input> {
    let mut inputs = build_place_and_turn_inputs(PLACE_AND_TURN_TICKS);
    let last = inputs[PLACE_AND_TURN_TICKS - 1];
    let raised = Input { knife: KnifeInput { lower: 0, ..last.knife }, ..last };
    inputs.extend([raised; RAISE_KNIFE_TICKS]);
    let mut world = World::create(build_slice_missing_cake(), START_KNIFE, None);
    for input in &inputs {
        world.step(input);
    }
    let top =
        world.get_cake_positions().iter().copied().fold(DVec3::NEG_INFINITY, |a, b| if b.y > a.y { b } else { a });
    let hand =
        HandInput { grab_at: top.to_array().map(quantize_meters), target_xz: CARRY_OFF_TARGET_M.map(quantize_meters) };
    inputs.extend([Input { hand: Some(hand), ..raised }; CARRY_OFF_TICKS]);
    inputs.extend([raised; DROP_TICKS]);
    inputs
}

fn build_place_and_turn_inputs(tick_len: usize) -> Vec<Input> {
    let grab_at = calc_spawned_top_grab_at(&build_slice_missing_cake());
    let start_tilt = quantize_radians(START_KNIFE.tilt_rad);
    (0..tick_len)
        .map(|tick| {
            let tick = tick as i32;
            let hand = (20..240).contains(&tick).then_some(HandInput { grab_at, target_xz: [0, 2_000] });
            let knife = KnifeInput { tilt: start_tilt - 300 * tick.min(400), lower: 100 * (tick - 300).max(0) };
            Input { hand, knife, stand_angle: 400 * (tick - 300).max(0) }
        })
        .collect()
}

/// The grab point on the highest vertex of `model`'s cake where it rests at its spawn.
pub fn calc_spawned_top_grab_at(model: &CakeModel) -> [i32; 3] {
    let top = model.mesh.positions.iter().copied().fold(model.mesh.positions[0], |a, b| if b.y > a.y { b } else { a });
    [CAKE_SPAWN.x + top.x as f64, top.y as f64, CAKE_SPAWN.z + top.z as f64].map(quantize_meters)
}

/// Replays `inputs` on a fresh world, returning its hash every [`CHECKPOINT_INTERVAL_TICKS`],
/// then the bits of the cut's two volumes when cut at the end.
pub fn replay(inputs: &[Input]) -> Vec<u64> {
    let mut world = World::create(build_slice_missing_cake(), START_KNIFE, None);
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
