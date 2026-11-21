use core::f32::consts::TAU;

use rapier3d::math::Vec3;

use crate::mesh::{Mesh, calc_arc_outline};

/// A cake's render mesh plus the convex pieces its collider is built from, in its local frame
/// with the serving side down at y = 0.
#[derive(Clone, Debug)]
pub struct CakeModel {
    pub mesh: Mesh,
    pub hulls: Vec<Vec<Vec3>>,
}

/// Smoothness of the drawn and weighed mesh.
const MESH_SEGMENTS_PER_TURN: f32 = 48.0;
/// Coarser for the collider: a hull face with many vertices resting on the table jitters in
/// Rapier and never sleeps, and facets keep a cake on its side from rolling far.
const HULL_SEGMENTS_PER_TURN: f32 = 16.0;

/// A placeholder until sourced models arrive: a round cake with a slice missing, under a smaller
/// tier set off center.
pub fn build_slice_missing_cake() -> CakeModel {
    let mut model = CakeModel { mesh: Mesh::default(), hulls: Vec::new() };
    let missing_slice_start = 290.0_f32.to_radians();
    let split = missing_slice_start / 2.0;
    for (angle0, angle1) in [(0.0, split), (split, missing_slice_start)] {
        let build_outline = |segments_per_turn: f32| {
            let point_len = libm::ceilf(segments_per_turn * (angle1 - angle0) / TAU) as u32 + 1;
            let mut outline = vec![[0.0, 0.0]];
            outline.extend(calc_arc_outline([0.0, 0.0], 0.11, angle0, angle1, point_len));
            outline
        };
        add_prism(&mut model, build_outline, 0.0, 0.07);
    }
    let top_offset_angle = 145.0_f32.to_radians();
    let top_center = [0.03 * libm::cosf(top_offset_angle), -0.03 * libm::sinf(top_offset_angle)];
    let build_top_outline = |segments_per_turn: f32| {
        let point_len = segments_per_turn as u32;
        calc_arc_outline(top_center, 0.055, 0.0, TAU * (1.0 - 1.0 / segments_per_turn), point_len)
    };
    add_prism(&mut model, build_top_outline, 0.07, 0.12);
    model
}

/// Adds a convex prism to the mesh, and a coarser copy as one of the collider's hulls.
fn add_prism(model: &mut CakeModel, build_outline: impl Fn(f32) -> Vec<[f32; 2]>, y0: f32, y1: f32) {
    model.mesh.push_prism(&build_outline(MESH_SEGMENTS_PER_TURN), y0, y1);
    let hull =
        build_outline(HULL_SEGMENTS_PER_TURN).into_iter().flat_map(|[x, z]| [Vec3::new(x, y0, z), Vec3::new(x, y1, z)]);
    model.hulls.push(hull.collect());
}
