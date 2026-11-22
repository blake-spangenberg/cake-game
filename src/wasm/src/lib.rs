//! The browser's handle on the core simulation.

use cake_core::cakes::build_slice_missing_cake;
use cake_core::input::Input;
use cake_core::knife::{
    BLADE_ARC_RADIUS_M, BLADE_BACK_M, BLADE_HALF_SPAN_M, BLADE_THICKNESS_M, HANDLE_LEN_M, HANDLE_RADIUS_M,
    MAX_TILT_RAD, START_KNIFE, calc_blade_outline, calc_handle_centers,
};
use cake_core::replay::{build_scripted_inputs, replay};
use cake_core::sim::{Phase, ROUND_TICKS, TICKS_PER_S, World};
use cake_core::table::{
    PIVOT, STAND_PARTS, STAND_PLATE_THICKNESS_M, STAND_RADIUS_M, STAND_TOP_Y_M, TABLE_HALF_X_M, TABLE_HALF_Z_M,
};
use cake_core::{cut::DENSITY_G_PER_M3, cut::Split, input};
use wasm_bindgen::prelude::*;

const BLADE_DRAW_SEGMENTS: u32 = 48;

#[wasm_bindgen]
pub struct Game {
    world: World,
}

/// Layout constants the client draws and aims with, all in meters, radians, or ticks.
#[wasm_bindgen(getter_with_clone)]
pub struct Layout {
    pub table_half_x_m: f64,
    pub table_half_z_m: f64,
    pub stand_radius_m: f64,
    pub stand_top_y_m: f64,
    pub stand_plate_thickness_m: f64,
    pub pivot_y_m: f64,
    pub blade_arc_radius_m: f64,
    pub blade_half_span_m: f64,
    pub blade_back_m: f64,
    pub blade_thickness_m: f64,
    pub handle_radius_m: f64,
    pub handle_len_m: f64,
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
        stand_radius_m: STAND_RADIUS_M,
        stand_top_y_m: STAND_TOP_Y_M,
        stand_plate_thickness_m: STAND_PLATE_THICKNESS_M,
        pivot_y_m: PIVOT.y,
        blade_arc_radius_m: BLADE_ARC_RADIUS_M,
        blade_half_span_m: BLADE_HALF_SPAN_M,
        blade_back_m: BLADE_BACK_M,
        blade_thickness_m: BLADE_THICKNESS_M,
        handle_radius_m: HANDLE_RADIUS_M,
        handle_len_m: HANDLE_LEN_M,
        max_tilt_rad: MAX_TILT_RAD,
        position_unit_m: input::POSITION_UNIT_M,
        angle_unit_rad: input::ANGLE_UNIT_RAD,
        ticks_per_s: TICKS_PER_S,
        round_ticks: ROUND_TICKS,
        density_g_per_m3: DENSITY_G_PER_M3,
    }
}

/// The stand's cylinders from top to bottom as flattened `(radius, y0, y1)` triples, meters.
#[wasm_bindgen]
pub fn get_stand_parts() -> Vec<f64> {
    STAND_PARTS.iter().flat_map(|part| [part.radius_m, part.y0_m, part.y1_m]).collect()
}

/// The blade's outline in the knife's local frame as flattened `(x, y)` pairs, meters.
#[wasm_bindgen]
pub fn get_blade_outline() -> Vec<f64> {
    calc_blade_outline(BLADE_DRAW_SEGMENTS).into_iter().flatten().collect()
}

/// The two handles' centers in the knife's local frame, flattened `(x, y, z)`, meters.
#[wasm_bindgen]
pub fn get_handle_centers() -> Vec<f32> {
    calc_handle_centers().iter().flat_map(|center| center.to_array()).collect()
}

/// The determinism checkpoints of the scripted round, to compare against a native replay.
#[wasm_bindgen]
pub fn replay_script() -> Vec<u64> {
    replay(&build_scripted_inputs())
}

#[wasm_bindgen]
impl Game {
    /// A round that cuts itself at `round_ticks`, or never when it is undefined.
    #[wasm_bindgen(constructor)]
    pub fn new(round_ticks: Option<u32>) -> Game {
        Game { world: World::create(build_slice_missing_cake(), START_KNIFE, round_ticks) }
    }

    /// Advances one tick with an input in the core's wire format.
    pub fn step(&mut self, encoded_input: &[i32]) -> Result<(), JsError> {
        let input = Input::decode(encoded_input).map_err(|error| JsError::new(&format!("input: {error:?}")))?;
        self.world.step(&input);
        Ok(())
    }

    /// Cuts now, returning the volumes above and below the knife's plane in cubic meters, then
    /// the unrounded score from 0 to `MAX_SCORE`.
    pub fn cut(&mut self) -> Vec<f64> {
        to_result(&self.world.cut())
    }

    /// The cut's volumes and score as `cut` returns them, or none while the round is playing.
    pub fn find_split(&self) -> Option<Vec<f64>> {
        match self.world.get_phase() {
            Phase::Playing => None,
            Phase::Cut { split, .. } => Some(to_result(&split)),
        }
    }

    pub fn is_cut_through(&self) -> bool {
        self.world.is_cut_through()
    }

    /// How far the knife lowers through the cake when it cuts, in meters.
    pub fn calc_cut_lower_m(&self) -> f64 {
        self.world.calc_cut_lower_m()
    }

    pub fn get_tick(&self) -> u32 {
        self.world.get_tick()
    }

    pub fn calc_hash(&self) -> u64 {
        self.world.calc_hash()
    }

    /// Translation then rotation quaternion: x, y, z, qx, qy, qz, qw.
    pub fn get_cake_pose(&self) -> Vec<f32> {
        let pose = self.world.get_cake_pose();
        pose.translation.to_array().into_iter().chain(pose.rotation.to_array()).collect()
    }

    pub fn get_stand_angle_rad(&self) -> f64 {
        self.world.get_stand_angle_rad()
    }

    /// Whether the knife stopped the stand short of its input last tick.
    pub fn is_stand_blocked(&self) -> bool {
        self.world.is_stand_blocked()
    }

    /// Tilt in radians, then lowering in meters.
    pub fn get_knife_pose(&self) -> Vec<f64> {
        let knife = self.world.get_knife();
        vec![knife.tilt_rad, knife.lower_m]
    }

    /// The knife's unit edge, down, and normal vectors, flattened.
    pub fn get_knife_frame(&self) -> Vec<f64> {
        let frame = self.world.get_knife().calc_frame();
        [frame.edge, frame.down, frame.normal].iter().flat_map(|v| v.to_array()).collect()
    }

    /// The cake mesh's local vertex positions, flattened.
    pub fn get_cake_mesh_positions(&self) -> Vec<f32> {
        self.world.get_model().mesh.positions.iter().flat_map(|p| p.to_array()).collect()
    }

    /// The cake mesh's triangles as vertex indices, flattened.
    pub fn get_cake_mesh_triangles(&self) -> Vec<u32> {
        self.world.get_model().mesh.triangles.iter().flatten().copied().collect()
    }
}

fn to_result(split: &Split) -> Vec<f64> {
    vec![split.above_m3, split.below_m3, split.calc_score()]
}
