use core::f32::consts::TAU;

use rapier3d::math::Vec3;

use crate::mesh::Mesh;

/// A cake's render mesh plus the convex pieces its collider is built from, in its local frame
/// with the serving side down at y = 0.
#[derive(Clone, Debug)]
pub struct CakeModel {
    pub mesh: Mesh,
    pub hulls: Vec<Vec<Vec3>>,
}

const SEGMENTS_PER_TURN: f32 = 48.0;

/// A placeholder until sourced models arrive: a round cake with a slice missing, under a smaller
/// tier set off center.
pub fn build_slice_missing_cake() -> CakeModel {
    let mut model = CakeModel { mesh: Mesh { positions: Vec::new(), triangles: Vec::new() }, hulls: Vec::new() };
    let bottom = Tier { center_x: 0.0, center_z: 0.0, radius: 0.11, y0: 0.0, y1: 0.07 };
    let missing_slice_start = 290.0_f32.to_radians();
    let split = missing_slice_start / 2.0;
    add_sector_prism(&mut model, &bottom, 0.0, split);
    add_sector_prism(&mut model, &bottom, split, missing_slice_start);
    let top_offset_angle = 145.0_f32.to_radians();
    let top = Tier {
        center_x: 0.03 * libm::cosf(top_offset_angle),
        center_z: -0.03 * libm::sinf(top_offset_angle),
        radius: 0.055,
        y0: 0.07,
        y1: 0.12,
    };
    add_cylinder(&mut model, &top);
    model
}

struct Tier {
    center_x: f32,
    center_z: f32,
    radius: f32,
    y0: f32,
    y1: f32,
}

impl Tier {
    fn calc_rim_point(&self, angle: f32, y: f32) -> Vec3 {
        Vec3::new(self.center_x + self.radius * libm::cosf(angle), y, self.center_z - self.radius * libm::sinf(angle))
    }
}

/// Adds a convex wedge of a tier spanning `angle0..angle1` counterclockwise seen from above, at
/// most half a turn.
fn add_sector_prism(model: &mut CakeModel, tier: &Tier, angle0: f32, angle1: f32) {
    debug_assert!(angle1 - angle0 <= TAU / 2.0);
    let segment_len = libm::ceilf(SEGMENTS_PER_TURN * (angle1 - angle0) / TAU) as u32;
    let base = model.mesh.positions.len() as u32;
    let (bottom_center, top_center) = (base, base + 1);
    let rim = |i: u32| (base + 2 + 2 * i, base + 3 + 2 * i);
    let positions = &mut model.mesh.positions;
    positions.push(Vec3::new(tier.center_x, tier.y0, tier.center_z));
    positions.push(Vec3::new(tier.center_x, tier.y1, tier.center_z));
    for i in 0..=segment_len {
        let angle = angle0 + (angle1 - angle0) * i as f32 / segment_len as f32;
        positions.push(tier.calc_rim_point(angle, tier.y0));
        positions.push(tier.calc_rim_point(angle, tier.y1));
    }
    let triangles = &mut model.mesh.triangles;
    for i in 0..segment_len {
        let ((b0, t0), (b1, t1)) = (rim(i), rim(i + 1));
        triangles.extend([[top_center, t0, t1], [bottom_center, b1, b0], [b0, b1, t1], [b0, t1, t0]]);
    }
    let ((b_start, t_start), (b_end, t_end)) = (rim(0), rim(segment_len));
    triangles.extend([[bottom_center, b_start, t_start], [bottom_center, t_start, top_center]]);
    triangles.extend([[bottom_center, t_end, b_end], [bottom_center, top_center, t_end]]);
    model.hulls.push(model.mesh.positions[base as usize..].to_vec());
}

fn add_cylinder(model: &mut CakeModel, tier: &Tier) {
    let segment_len = SEGMENTS_PER_TURN as u32;
    let base = model.mesh.positions.len() as u32;
    let (bottom_center, top_center) = (base, base + 1);
    let rim = |i: u32| (base + 2 + 2 * (i % segment_len), base + 3 + 2 * (i % segment_len));
    let positions = &mut model.mesh.positions;
    positions.push(Vec3::new(tier.center_x, tier.y0, tier.center_z));
    positions.push(Vec3::new(tier.center_x, tier.y1, tier.center_z));
    for i in 0..segment_len {
        let angle = TAU * i as f32 / segment_len as f32;
        positions.push(tier.calc_rim_point(angle, tier.y0));
        positions.push(tier.calc_rim_point(angle, tier.y1));
    }
    let triangles = &mut model.mesh.triangles;
    for i in 0..segment_len {
        let ((b0, t0), (b1, t1)) = (rim(i), rim(i + 1));
        triangles.extend([[top_center, t0, t1], [bottom_center, b1, b0], [b0, b1, t1], [b0, t1, t0]]);
    }
    model.hulls.push(model.mesh.positions[base as usize..].to_vec());
}
