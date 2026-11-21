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
pub const STAND_COLUMN_RADIUS_M: f64 = 0.03;
pub const STAND_BASE_RADIUS_M: f64 = 0.09;
pub const STAND_BASE_THICKNESS_M: f64 = 0.012;
/// The longest any cake measures in any direction, so no resting pose stands taller.
pub const MAX_CAKE_EXTENT_M: f64 = 0.25;
/// The point every cut plane passes through, just above the tallest a cake on the stand can be,
/// where the knife's edge rests.
pub const PIVOT: DVec3 = DVec3::new(0.0, STAND_TOP_Y_M + MAX_CAKE_EXTENT_M + 0.01, 0.0);
/// Where the cake starts, on the table beside the stand so reaching it costs input distance.
pub const CAKE_SPAWN: DVec3 = DVec3::new(0.5, 0.0, 0.15);
/// A cake whose center falls below this has left the table and respawns.
pub const RESPAWN_BELOW_Y_M: f64 = -0.3;
