//! The fixed layout every round shares, in meters, with the table top at y = 0.

use rapier3d::glamx::DVec3;

/// The table centered on the origin.
pub const TABLE_HALF_X_M: f64 = 0.6;
pub const TABLE_HALF_Z_M: f64 = 0.4;
/// Every cake fits within this circle around the origin, and the blade spans it at every pose.
pub const CUTTING_REGION_RADIUS_M: f64 = 0.25;
/// The point every cut plane passes through, above the cutting region's center and every cake.
pub const PIVOT: DVec3 = DVec3::new(0.0, 0.2, 0.0);
/// Where the cake starts, off to one side so reaching the knife costs input distance.
pub const CAKE_SPAWN: DVec3 = DVec3::new(0.42, 0.0, 0.0);
