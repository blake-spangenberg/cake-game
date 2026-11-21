use rapier3d::glamx::DVec3;
use rapier3d::math::{Rotation, Vec3};

use crate::cut::{Plane, for_each_section_segment};
use crate::input::{KnifeInput, calc_meters, calc_radians};
use crate::table::{PIVOT, STAND_RADIUS_M};

/// Tilt limit either side of vertical, so every cut still looks like slicing a cake.
pub const MAX_TILT_RAD: f64 = core::f64::consts::FRAC_PI_4;
/// The mezzaluna's edge is an arc of this radius, lowest at its middle.
pub const BLADE_ARC_RADIUS_M: f64 = 0.4;
/// How far the edge reaches either side of its middle, past the stand's rim so the blade spans
/// the cutting region at every pose.
pub const BLADE_HALF_SPAN_M: f64 = STAND_RADIUS_M + 0.04;
/// Height of the blade's straight back above the edge's lowest point.
pub const BLADE_BACK_M: f64 = 0.1;
pub const BLADE_THICKNESS_M: f64 = 0.002;
/// The two upright handles rise from the back's ends.
pub const HANDLE_RADIUS_M: f64 = 0.014;
pub const HANDLE_LEN_M: f64 = 0.09;
/// The pose every round starts in, leaned away from any useful cut.
pub const START_KNIFE: Knife = Knife { tilt_rad: MAX_TILT_RAD, lower_m: 0.0 };
/// The edge's fixed direction: the cake turns on its stand instead of the knife turning.
const EDGE: DVec3 = DVec3::Z;
const BLADE_HULL_SEGMENTS: u32 = 16;
const BLADE_COLLIDER_THICKNESS_M: f64 = 0.01;

/// The knife's cut plane always passes through [`PIVOT`], tilting about the edge's direction.
/// At rest the edge's lowest point lies on the pivot, and lowering slides it down within its
/// plane by `lower_m`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Knife {
    pub tilt_rad: f64,
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

/// A closed mesh in world coordinates that the knife's edge stops on.
#[derive(Clone, Copy)]
pub struct Solid<'a> {
    pub positions: &'a [DVec3],
    pub triangles: &'a [[u32; 3]],
}

impl Knife {
    pub fn calc_frame(&self) -> KnifeFrame {
        let (sin_tilt, cos_tilt) = (libm::sin(self.tilt_rad), libm::cos(self.tilt_rad));
        let level_normal = EDGE.cross(DVec3::Y);
        let down = level_normal * sin_tilt - DVec3::Y * cos_tilt;
        KnifeFrame { edge: EDGE, down, normal: EDGE.cross(down) }
    }

    pub fn calc_plane(&self) -> Plane {
        Plane { point: PIVOT, normal: self.calc_frame().normal }
    }

    /// The edge's lowest point, the origin of the knife's local frame.
    pub fn calc_edge_middle(&self) -> DVec3 {
        PIVOT + self.calc_frame().down * self.lower_m
    }

    /// Rotation from the knife's local frame (x along the edge, y up the blade) to the world.
    pub fn calc_rotation(&self) -> Rotation {
        let half_sqrt = core::f64::consts::FRAC_1_SQRT_2;
        let (sin_half, cos_half) = (libm::sin(-self.tilt_rad / 2.0), libm::cos(-self.tilt_rad / 2.0));
        let [x, y, z, w] = [sin_half * half_sqrt, -cos_half * half_sqrt, sin_half * half_sqrt, cos_half * half_sqrt];
        Rotation::from_xyzw(x as f32, y as f32, z as f32, w as f32)
    }

    /// Takes the player's tilt and lowering, lowering only until the edge meets a solid. The
    /// blade never pushes a solid; one rising into it raises it instead.
    pub fn step(&mut self, input: &KnifeInput, solids: &[Solid]) {
        self.tilt_rad = calc_radians(input.tilt).clamp(-MAX_TILT_RAD, MAX_TILT_RAD);
        let limit_m = self.calc_lower_limit_m(solids);
        self.lower_m = calc_meters(input.lower).min(limit_m).max(0.0);
    }

    /// How far the edge can lower before it touches one of `solids` or the table.
    pub fn calc_lower_limit_m(&self, solids: &[Solid]) -> f64 {
        let frame = self.calc_frame();
        let table_limit_m = PIVOT.y / -frame.down.y;
        let plane = Plane { point: PIVOT, normal: frame.normal };
        let mut limit_m = table_limit_m;
        for solid in solids {
            for_each_section_segment(solid.positions, solid.triangles, &plane, |a, b| {
                if let Some(depth_m) = calc_segment_depth_under_edge(&frame, a, b) {
                    limit_m = limit_m.min(depth_m);
                }
            });
        }
        limit_m
    }
}

/// Height of the edge above its lowest point at `u_m` along it.
pub fn calc_edge_rise_m(u_m: f64) -> f64 {
    BLADE_ARC_RADIUS_M - libm::sqrt(BLADE_ARC_RADIUS_M * BLADE_ARC_RADIUS_M - u_m * u_m)
}

/// The blade's convex outline as `(x, y)` in the knife's local frame: along the curved edge in
/// `edge_segments` pieces, then back along its straight back.
pub fn calc_blade_outline(edge_segments: u32) -> Vec<[f64; 2]> {
    let edge = (0..=edge_segments).map(|i| {
        let u_m = BLADE_HALF_SPAN_M * (2.0 * i as f64 / edge_segments as f64 - 1.0);
        [u_m, calc_edge_rise_m(u_m)]
    });
    edge.chain([[BLADE_HALF_SPAN_M, BLADE_BACK_M], [-BLADE_HALF_SPAN_M, BLADE_BACK_M]]).collect()
}

/// The blade's outline on both faces, for its collider. The collider is thicker than the drawn
/// blade so a carried cake cannot pass through it between ticks.
pub fn build_blade_hull() -> Vec<Vec3> {
    let half_thickness_m = BLADE_COLLIDER_THICKNESS_M / 2.0;
    calc_blade_outline(BLADE_HULL_SEGMENTS)
        .into_iter()
        .flat_map(|[x_m, y_m]| {
            [-half_thickness_m, half_thickness_m].map(|z_m| Vec3::new(x_m as f32, y_m as f32, z_m as f32))
        })
        .collect()
}

/// Centers of the two handles in the knife's local frame, each upright along local y.
pub fn calc_handle_centers() -> [Vec3; 2] {
    let x_m = (BLADE_HALF_SPAN_M - HANDLE_RADIUS_M) as f32;
    let y_m = (BLADE_BACK_M + HANDLE_LEN_M / 2.0) as f32;
    [Vec3::new(-x_m, y_m, 0.0), Vec3::new(x_m, y_m, 0.0)]
}

/// The least lowering at which the curved edge reaches the in-plane segment `a`-`b`, or `None`
/// where the segment lies beyond the blade's ends. A point `u` along the edge and `s` down from
/// the pivot is reached at lowering `s + rise(u)`, convex in `u`, so its least value on the
/// segment is at the clamped stationary point.
fn calc_segment_depth_under_edge(frame: &KnifeFrame, a: DVec3, b: DVec3) -> Option<f64> {
    let (ua, ub) = ((a - PIVOT).dot(frame.edge), (b - PIVOT).dot(frame.edge));
    let (sa, sb) = ((a - PIVOT).dot(frame.down), (b - PIVOT).dot(frame.down));
    let (u_min, u_max) = (ua.min(ub), ua.max(ub));
    if u_max < -BLADE_HALF_SPAN_M || u_min > BLADE_HALF_SPAN_M {
        return None;
    }
    if ua == ub {
        return Some(sa.min(sb) + calc_edge_rise_m(ua));
    }
    let slope = (sb - sa) / (ub - ua);
    let stationary_u = -slope * BLADE_ARC_RADIUS_M / libm::sqrt(1.0 + slope * slope);
    let u = stationary_u.clamp(u_min.max(-BLADE_HALF_SPAN_M), u_max.min(BLADE_HALF_SPAN_M));
    Some(sa + slope * (u - ua) + calc_edge_rise_m(u))
}
