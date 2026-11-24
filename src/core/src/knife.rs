use rapier3d::glamx::DVec3;
use rapier3d::math::{Rotation, Vec3};

use crate::cut::{Plane, for_each_section_segment};
use crate::input::{KnifeInput, calc_meters, calc_radians};
use crate::table::{PIVOT, STAND_RADIUS_M, STAND_TOP_Y_M};

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
/// The tilt every round starts at, leaned away from any useful cut.
pub const START_TILT_RAD: f64 = MAX_TILT_RAD;
/// The edge's fixed direction: the cake turns on its stand instead of the knife turning.
const EDGE: DVec3 = DVec3::Z;
const BLADE_HULL_SEGMENTS: u32 = 16;
const BLADE_COLLIDER_THICKNESS_M: f64 = 0.01;
/// Largest move tried at once when sweeping a turn for where it meets a solid, small enough not
/// to skip through a thin part.
const SWEEP_PROBE_RAD: f64 = 0.01;
const SWEEP_BISECTIONS: u32 = 16;
/// How far short of a solid a sweep stops, so a cake settling a hair after the stop does not
/// cross the blade's plane, where the lowering limit jumps from a wall's foot to its top.
const SWEEP_MARGIN_RAD: f64 = 0.005;
/// How far the edge may sink into a solid it rests on and still count as clear, so a cake
/// settling under a resting knife does not block every turn.
const TOUCH_TOLERANCE_M: f64 = 0.0005;
/// How far above the player's lowering a tilt may lift the edge to ride over a bump, such as a
/// mitten's knit. The client lets the player press a few millimeters past where the knife stops,
/// and those come off the highest bump ridden.
pub const MAX_RIDE_M: f64 = 0.015;
/// How far past the cut's stop a cake may reach and still lie in its path, so a cake settled a
/// hair into the plate it rests on is still cut.
const CUT_DEPTH_TOLERANCE_M: f64 = 0.005;
/// How far the resting edge clears the top of the cake as it is posed now, were it standing on
/// the stand.
const REST_HEADROOM_M: f64 = 0.05;
/// Half-life of each of the rest's two easing stages, which together cover half of a change in
/// the cake's height in about 0.5 s.
const REST_STAGE_HALF_LIFE_S: f64 = 0.2;

/// The knife's cut plane always passes through [`PIVOT`], tilting about the edge's direction.
/// `lower_m` is how far the edge's lowest point lies below the pivot within the plane. The player
/// lowers it from `rest_m`, which floats with the cake's height by sliding the knife along its
/// blade, so the cut line stays in place.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Knife {
    pub tilt_rad: f64,
    pub rest_m: f64,
    /// What `rest_m` eases toward, itself easing toward where the knife rests over the cake. The
    /// two stages start the float smoothly and stop it without passing its target.
    pub rest_lead_m: f64,
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

impl KnifeFrame {
    fn calc_plane(&self) -> Plane {
        Plane { point: PIVOT, normal: self.normal }
    }

    /// Where the in-plane point `p` lies: how far along the edge from the pivot, then how far down.
    fn calc_plane_coords(&self, p: DVec3) -> (f64, f64) {
        ((p - PIVOT).dot(self.edge), (p - PIVOT).dot(self.down))
    }
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
        self.calc_frame().calc_plane()
    }

    /// The edge's lowest point, the origin of the knife's local frame.
    pub(crate) fn calc_edge_middle(&self) -> DVec3 {
        PIVOT + self.calc_frame().down * self.lower_m
    }

    /// Eases the rest over `dt_s` toward where it sits over a cake `cake_height_m` tall, never
    /// leaving the range between where it was and the targets it has been given. The next `step`
    /// lowers the knife from there.
    pub fn float_over(&mut self, cake_height_m: f64, dt_s: f64) {
        let ease = 1.0 - libm::exp2(-dt_s / REST_STAGE_HALF_LIFE_S);
        self.rest_lead_m += (calc_rest_m(cake_height_m) - self.rest_lead_m) * ease;
        self.rest_m += (self.rest_lead_m - self.rest_m) * ease;
    }

    /// Rotation from the knife's local frame (x along the edge, y up the blade) to the world.
    pub fn calc_rotation(&self) -> Rotation {
        let half_sqrt = core::f64::consts::FRAC_1_SQRT_2;
        let (sin_half, cos_half) = (libm::sin(-self.tilt_rad / 2.0), libm::cos(-self.tilt_rad / 2.0));
        let [x, y, z, w] = [sin_half * half_sqrt, -cos_half * half_sqrt, sin_half * half_sqrt, cos_half * half_sqrt];
        Rotation::from_xyzw(x as f32, y as f32, z as f32, w as f32)
    }

    /// Takes the player's lowering, then tilt, then lowering again at the new tilt. Lowering
    /// stops where the edge meets a solid. Tilting rides the edge up over a bump in a solid, but
    /// stops where it would have to rise more than [`MAX_RIDE_M`] above the player's lowering.
    /// The blade never pushes a solid; one rising into it raises it instead.
    pub fn step(&mut self, input: &KnifeInput, solids: &[Solid]) {
        let wanted_lower_m = self.rest_m + calc_meters(input.lower).max(0.0);
        self.lower_toward(wanted_lower_m, solids);
        let ride_lower_m = self.lower_m.min(wanted_lower_m - MAX_RIDE_M);
        let target_tilt_rad = calc_radians(input.tilt).clamp(-MAX_TILT_RAD, MAX_TILT_RAD);
        self.tilt_rad = calc_sweep_reach(self.tilt_rad, target_tilt_rad, |tilt_rad| {
            Knife { tilt_rad, lower_m: ride_lower_m, ..*self }.is_clear_of(solids)
        });
        self.lower_toward(wanted_lower_m, solids);
    }

    fn lower_toward(&mut self, wanted_lower_m: f64, solids: &[Solid]) {
        self.lower_m = wanted_lower_m.min(self.calc_lower_limit_m(solids)).max(0.0);
    }

    /// Whether the edge at its lowering stays out of every solid, give or take
    /// [`TOUCH_TOLERANCE_M`].
    pub(crate) fn is_clear_of(&self, solids: &[Solid]) -> bool {
        self.calc_lower_limit_m(solids) >= self.lower_m - TOUCH_TOLERANCE_M
    }

    /// Whether a cut that stops at the lowering `stop_lower_m` divides `cake`: any of its section
    /// lies within the blade's span and no deeper than the stop, since the cake splits whole along
    /// the plane. The cut rocks the curved edge along its stop, so it reaches that depth across the
    /// span, not only at the edge's middle.
    pub(crate) fn is_cut_through(&self, cake: Solid, stop_lower_m: f64) -> bool {
        let frame = self.calc_frame();
        let plane = frame.calc_plane();
        let mut is_reached = false;
        for_each_section_segment(cake.positions, cake.triangles, &plane, |a, b| {
            is_reached |= calc_segment_least_depth_m(&frame, a, b)
                .is_some_and(|depth_m| depth_m <= stop_lower_m + CUT_DEPTH_TOLERANCE_M);
        });
        is_reached
    }

    /// How far the edge can lower before it touches one of `solids` or the table.
    pub fn calc_lower_limit_m(&self, solids: &[Solid]) -> f64 {
        let frame = self.calc_frame();
        let table_limit_m = PIVOT.y / -frame.down.y;
        let plane = frame.calc_plane();
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

/// How far a turn from `from_rad` toward `to_rad` gets while `is_clear` holds, stopping
/// [`SWEEP_MARGIN_RAD`] short of where it first fails but never turning back.
pub(crate) fn calc_sweep_reach(from_rad: f64, to_rad: f64, is_clear: impl Fn(f64) -> bool) -> f64 {
    if to_rad == from_rad {
        return to_rad;
    }
    let direction = (to_rad - from_rad).signum();
    let probe_to_rad = to_rad + SWEEP_MARGIN_RAD * direction;
    let step_len = libm::ceil((probe_to_rad - from_rad).abs() / SWEEP_PROBE_RAD) as u32;
    let mut clear_rad = from_rad;
    for i in 1..=step_len {
        let probe_rad = from_rad + (probe_to_rad - from_rad) * i as f64 / step_len as f64;
        if !is_clear(probe_rad) {
            let blocked_rad = calc_bisected_reach(clear_rad, probe_rad, is_clear);
            let reach_rad = ((blocked_rad - from_rad).abs() - SWEEP_MARGIN_RAD).max(0.0);
            return from_rad + reach_rad * direction;
        }
        clear_rad = probe_rad;
    }
    to_rad
}

fn calc_bisected_reach(mut clear_rad: f64, mut blocked_rad: f64, is_clear: impl Fn(f64) -> bool) -> f64 {
    for _ in 0..SWEEP_BISECTIONS {
        let middle_rad = (clear_rad + blocked_rad) / 2.0;
        if is_clear(middle_rad) {
            clear_rad = middle_rad;
        } else {
            blocked_rad = middle_rad;
        }
    }
    clear_rad
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
pub(crate) fn build_blade_hull() -> Vec<Vec3> {
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
    let ((ua, sa), (ub, sb)) = (frame.calc_plane_coords(a), frame.calc_plane_coords(b));
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

/// The least depth below the pivot of the in-plane segment `a`-`b` where it lies within the
/// blade's span, or `None` where it lies wholly beyond the blade's ends.
fn calc_segment_least_depth_m(frame: &KnifeFrame, a: DVec3, b: DVec3) -> Option<f64> {
    let ((ua, sa), (ub, sb)) = (frame.calc_plane_coords(a), frame.calc_plane_coords(b));
    let (u0, u1) = (ua.min(ub).max(-BLADE_HALF_SPAN_M), ua.max(ub).min(BLADE_HALF_SPAN_M));
    if u0 > u1 {
        return None;
    }
    if ua == ub {
        return Some(sa.min(sb));
    }
    let depth_at = |u: f64| sa + (sb - sa) * (u - ua) / (ub - ua);
    Some(depth_at(u0).min(depth_at(u1)))
}

/// The lowering at which the edge rests over a cake `cake_height_m` tall standing on the stand,
/// [`REST_HEADROOM_M`] above its top, and never above the pivot.
pub(crate) fn calc_rest_m(cake_height_m: f64) -> f64 {
    (PIVOT.y - STAND_TOP_Y_M - cake_height_m - REST_HEADROOM_M).max(0.0)
}
