//! The browser's handle on the core simulation.

use cake_core::cakes::build_slice_missing_cake;
use cake_core::input::Input;
use cake_core::knife::{BLADE_LEN_M, MAX_TILT_RAD, START_KNIFE};
use cake_core::replay::{build_scripted_inputs, replay};
use cake_core::sim::{Phase, ROUND_TICKS, TICKS_PER_S, World};
use cake_core::table::{CAKE_SPAWN, CUTTING_REGION_RADIUS_M, PIVOT, TABLE_HALF_X_M, TABLE_HALF_Z_M};
use cake_core::{cut::DENSITY_G_PER_M3, input};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct Game {
    world: World,
}

/// Layout constants the client draws and aims with, all in meters, radians, or ticks.
#[wasm_bindgen(getter_with_clone)]
pub struct Layout {
    pub table_half_x_m: f64,
    pub table_half_z_m: f64,
    pub cutting_region_radius_m: f64,
    pub pivot_y_m: f64,
    pub cake_spawn_x_m: f64,
    pub blade_len_m: f64,
    pub max_tilt_rad: f64,
    pub position_unit_m: f64,
    pub angle_unit_rad: f64,
    pub ticks_per_s: u32,
    pub round_ticks: u32,
    pub density_g_per_m3: f64,
}

#[wasm_bindgen]
pub fn get_layout() -> Layout {
    Layout {
        table_half_x_m: TABLE_HALF_X_M,
        table_half_z_m: TABLE_HALF_Z_M,
        cutting_region_radius_m: CUTTING_REGION_RADIUS_M,
        pivot_y_m: PIVOT.y,
        cake_spawn_x_m: CAKE_SPAWN.x,
        blade_len_m: BLADE_LEN_M,
        max_tilt_rad: MAX_TILT_RAD,
        position_unit_m: input::POSITION_UNIT_M,
        angle_unit_rad: input::ANGLE_UNIT_RAD,
        ticks_per_s: TICKS_PER_S,
        round_ticks: ROUND_TICKS,
        density_g_per_m3: DENSITY_G_PER_M3,
    }
}

/// The determinism checkpoints of the scripted round, to compare against a native replay.
#[wasm_bindgen]
pub fn replay_script() -> Vec<u64> {
    replay(&build_scripted_inputs())
}

#[wasm_bindgen]
impl Game {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Game {
        Game { world: World::create(build_slice_missing_cake(), START_KNIFE) }
    }

    /// Advances one tick with an input in the core's wire format.
    pub fn step(&mut self, encoded_input: &[i32]) -> Result<(), JsError> {
        let input = Input::decode(encoded_input).map_err(|error| JsError::new(&format!("input: {error:?}")))?;
        self.world.step(&input);
        Ok(())
    }

    /// Cuts now, returning the volumes above and below the knife's plane in cubic meters.
    pub fn cut(&mut self) -> Vec<f64> {
        let split = self.world.cut();
        vec![split.above_m3, split.below_m3]
    }

    /// The cut's volumes, or none while the round is still playing.
    pub fn find_split(&self) -> Option<Vec<f64>> {
        match self.world.get_phase() {
            Phase::Playing => None,
            Phase::Cut { split, .. } => Some(vec![split.above_m3, split.below_m3]),
        }
    }

    pub fn tick(&self) -> u32 {
        self.world.get_tick()
    }

    pub fn hash(&self) -> u64 {
        self.world.calc_hash()
    }

    /// Translation then rotation quaternion: x, y, z, qx, qy, qz, qw.
    pub fn cake_pose(&self) -> Vec<f32> {
        let pose = self.world.get_cake_pose();
        pose.translation.to_array().into_iter().chain(pose.rotation.to_array()).collect()
    }

    /// Tilt and heading in radians, then lowering in meters.
    pub fn knife_pose(&self) -> Vec<f64> {
        let knife = self.world.get_knife();
        vec![knife.tilt_rad, knife.heading_rad, knife.lower_m]
    }

    /// The knife's unit edge, down, and normal vectors, then the midpoint of its edge, flattened.
    pub fn knife_frame(&self) -> Vec<f64> {
        let knife = self.world.get_knife();
        let frame = knife.calc_frame();
        let edge_mid = PIVOT + frame.down * knife.lower_m;
        [frame.edge, frame.down, frame.normal, edge_mid].iter().flat_map(|v| v.to_array()).collect()
    }

    /// The cake mesh's local vertex positions, flattened.
    pub fn cake_positions(&self) -> Vec<f32> {
        self.world.get_model().mesh.positions.iter().flat_map(|p| p.to_array()).collect()
    }

    /// The cake mesh's triangles as vertex indices, flattened.
    pub fn cake_triangles(&self) -> Vec<u32> {
        self.world.get_model().mesh.triangles.iter().flatten().copied().collect()
    }
}

impl Default for Game {
    fn default() -> Self {
        Game::new()
    }
}
