use rapier3d::glamx::DVec3;

use crate::cut::{Plane, for_each_section_segment};
use crate::input::{KnifeInput, calc_radians};
use crate::table::{CUTTING_REGION_RADIUS_M, PIVOT};

/// Tilt limit either side of vertical, so every cut still looks like slicing a cake.
pub const MAX_TILT_RAD: f64 = core::f64::consts::FRAC_PI_4;
/// Spans the cutting region at every pose, since the edge stays centered under the pivot.
pub const BLADE_LEN_M: f64 = 2.0 * CUTTING_REGION_RADIUS_M;
/// The pose every round starts in, turned and leaned away from any useful cut.
pub const START_KNIFE: Knife = Knife { tilt_rad: MAX_TILT_RAD, heading_rad: 1.2, lower_m: 0.0 };
const LOWER_SPEED_M_PER_S: f64 = 0.35;
const RAISE_SPEED_M_PER_S: f64 = 0.7;

/// The knife's cut plane always passes through [`PIVOT`]. At rest the blade's edge lies on the
/// pivot, and lowering slides it down within its plane by `lower_m`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Knife {
    pub tilt_rad: f64,
    pub heading_rad: f64,
    pub lower_m: f64,
}

/// Unit vectors of the knife's frame: along the edge, down the blade within its plane, and the
/// plane's normal.
#[derive(Clone, Copy, Debug)]
pub struct KnifeFrame {
    pub edge: DVec3,
    pub down: DVec3,
    pub normal: DVec3,
}

impl Knife {
    pub fn calc_frame(&self) -> KnifeFrame {
        let (sin_heading, cos_heading) = (libm::sin(self.heading_rad), libm::cos(self.heading_rad));
        let (sin_tilt, cos_tilt) = (libm::sin(self.tilt_rad), libm::cos(self.tilt_rad));
        let edge = DVec3::new(cos_heading, 0.0, -sin_heading);
        let level_normal = edge.cross(DVec3::Y);
        let down = level_normal * sin_tilt - DVec3::Y * cos_tilt;
        KnifeFrame { edge, down, normal: edge.cross(down) }
    }

    pub fn calc_plane(&self) -> Plane {
        Plane { point: PIVOT, normal: self.calc_frame().normal }
    }

    /// Takes the player's angles, then moves the blade one tick toward its goal: down while
    /// lowering until the edge meets the cake or table, up otherwise. The blade never pushes the
    /// cake; a cake rising into it raises it instead.
    pub fn step(&mut self, input: &KnifeInput, cake_positions: &[DVec3], cake_triangles: &[[u32; 3]], dt: f64) {
        self.tilt_rad = calc_radians(input.tilt).clamp(-MAX_TILT_RAD, MAX_TILT_RAD);
        self.heading_rad = calc_radians(input.heading);
        let limit_m = self.calc_lower_limit_m(cake_positions, cake_triangles);
        let goal_m = if input.is_lowering {
            (self.lower_m + LOWER_SPEED_M_PER_S * dt).min(limit_m)
        } else {
            self.lower_m - RAISE_SPEED_M_PER_S * dt
        };
        self.lower_m = goal_m.min(limit_m).max(0.0);
    }

    /// How far the edge can lower before it touches the cake's surface or the table.
    pub fn calc_lower_limit_m(&self, cake_positions: &[DVec3], cake_triangles: &[[u32; 3]]) -> f64 {
        let frame = self.calc_frame();
        let table_limit_m = PIVOT.y / -frame.down.y;
        let plane = Plane { point: PIVOT, normal: frame.normal };
        let mut limit_m = table_limit_m;
        for_each_section_segment(cake_positions, cake_triangles, &plane, |a, b| {
            if let Some(depth_m) = calc_segment_depth_under_edge(&frame, a, b) {
                limit_m = limit_m.min(depth_m);
            }
        });
        limit_m
    }
}

/// The least lowering at which the edge reaches the in-plane segment `a`-`b`, or `None` where the
/// segment lies beyond the blade's ends.
fn calc_segment_depth_under_edge(frame: &KnifeFrame, a: DVec3, b: DVec3) -> Option<f64> {
    let half_len_m = BLADE_LEN_M / 2.0;
    let (ua, ub) = ((a - PIVOT).dot(frame.edge), (b - PIVOT).dot(frame.edge));
    let (sa, sb) = ((a - PIVOT).dot(frame.down), (b - PIVOT).dot(frame.down));
    let (u_min, u_max) = (ua.min(ub), ua.max(ub));
    if u_max < -half_len_m || u_min > half_len_m {
        return None;
    }
    if ua == ub {
        return Some(sa.min(sb));
    }
    let depth_at = |u: f64| sa + (sb - sa) * ((u - ua) / (ub - ua));
    let (u0, u1) = (u_min.max(-half_len_m), u_max.min(half_len_m));
    Some(depth_at(u0).min(depth_at(u1)))
}
