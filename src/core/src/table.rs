//! The fixed layout every round shares, in meters, with the table top at y = 0.

use rapier3d::glamx::DVec3;

/// The table centered on the origin.
pub const TABLE_HALF_X_M: f64 = 0.8;
pub const TABLE_HALF_Z_M: f64 = 0.5;
/// The cake stand's plate, centered on the origin and turned by the player. It is the cutting
/// region: every cake fits on it, and the blade spans it at every pose.
pub const STAND_RADIUS_M: f64 = 0.18;
pub const STAND_TOP_Y_M: f64 = 0.12;
pub const STAND_PLATE_THICKNESS_M: f64 = 0.012;
const STAND_COLUMN_RADIUS_M: f64 = 0.03;
const STAND_BASE_RADIUS_M: f64 = 0.09;
const STAND_BASE_THICKNESS_M: f64 = 0.012;

/// A cylinder of the stand, centered on its axis, from `y0_m` up to `y1_m`.
#[derive(Clone, Copy, Debug)]
pub struct StandPart {
    pub radius_m: f64,
    pub y0_m: f64,
    pub y1_m: f64,
}

/// The stand's plate, column, and base, from top to bottom.
pub const STAND_PARTS: [StandPart; 3] = [
    StandPart { radius_m: STAND_RADIUS_M, y0_m: STAND_TOP_Y_M - STAND_PLATE_THICKNESS_M, y1_m: STAND_TOP_Y_M },
    StandPart {
        radius_m: STAND_COLUMN_RADIUS_M,
        y0_m: STAND_BASE_THICKNESS_M,
        y1_m: STAND_TOP_Y_M - STAND_PLATE_THICKNESS_M,
    },
    StandPart { radius_m: STAND_BASE_RADIUS_M, y0_m: 0.0, y1_m: STAND_BASE_THICKNESS_M },
];
/// The longest any cake measures in any direction, so no resting pose stands taller.
pub const MAX_CAKE_EXTENT_M: f64 = 0.25;
/// How far the resting knife's edge clears the tallest the round's cake can stand on the stand,
/// with room for a carried cake's centimeter of hover.
const KNIFE_REST_CLEARANCE_M: f64 = 0.02;
/// The highest pivot, for the tallest cake allowed.
pub const MAX_PIVOT: DVec3 = DVec3::new(0.0, STAND_TOP_Y_M + MAX_CAKE_EXTENT_M + KNIFE_REST_CLEARANCE_M, 0.0);

/// The point every cut plane passes through in a round on a cake measuring `cake_extent_m` at
/// its longest, where the knife's edge rests: just above the tallest that cake can stand on the
/// stand in any pose, so positioning it never meets the knife, and no higher, so a round cake is
/// not left far below the blade.
pub fn calc_pivot(cake_extent_m: f64) -> DVec3 {
    let height_m = cake_extent_m.min(MAX_CAKE_EXTENT_M) + KNIFE_REST_CLEARANCE_M;
    DVec3::new(0.0, STAND_TOP_Y_M + height_m, 0.0)
}
/// Where the cake starts, on the table beside the stand so reaching it costs input distance.
pub const CAKE_SPAWN: DVec3 = DVec3::new(0.5, 0.0, 0.15);
/// A cake whose center falls below this has left the table and respawns.
pub const RESPAWN_BELOW_Y_M: f64 = -0.3;
