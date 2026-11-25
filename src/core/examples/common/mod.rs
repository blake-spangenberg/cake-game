use std::{env, fs, process};

use cake_core::cakes::{CakeModel, build_slice_missing_cake};
use rapier3d::math::Rotation;

/// The baked cake file named by the first argument, or the placeholder cake with none.
#[allow(dead_code, reason = "throw_cost and replay read several cake files")]
pub fn fetch_model() -> CakeModel {
    env::args().nth(1).map_or_else(build_slice_missing_cake, |path| fetch_cake_file(&path))
}

/// The baked cake file at `path`, exiting with the reason if it is unreadable or unplayable.
#[allow(dead_code, reason = "replay reads packed cakes")]
pub fn fetch_cake_file(path: &str) -> CakeModel {
    let bytes = fs::read(path).unwrap_or_else(|error| exit_with(&format!("cannot read {path}: {error}")));
    CakeModel::decode(&bytes).unwrap_or_else(|error| exit_with(&format!("{path} is not a playable cake: {error:?}")))
}

/// The angle of the turn from `a` to `b`. Unlike f32 `angle_between`, which reads up to 1e-3 rad
/// between equal rotations, it is accurate near zero, so a still cake reads as still.
#[allow(dead_code, reason = "replay measures no turns")]
pub fn calc_turn_rad(a: Rotation, b: Rotation) -> f32 {
    let turn = b * a.inverse();
    2.0 * turn.xyz().length().atan2(turn.w.abs())
}

#[expect(clippy::exit, reason = "the examples report a bad argument from wherever they read it")]
pub fn exit_with(message: &str) -> ! {
    eprintln!("{message}");
    process::exit(1);
}
