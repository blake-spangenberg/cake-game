use rapier3d::glamx::DVec3;

/// A plane through `point` whose unit `normal` points to its above side.
#[derive(Clone, Copy, Debug)]
pub struct Plane {
    pub point: DVec3,
    pub normal: DVec3,
}

/// The volumes a plane divides a closed mesh into, cubic meters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Split {
    pub above_m3: f64,
    pub below_m3: f64,
}

/// Grams per cubic meter, uniform for every cake.
pub const DENSITY_G_PER_M3: f64 = 1.0e6;
pub const MAX_SCORE: f64 = 100.0;

impl Split {
    /// [`MAX_SCORE`] for halves of equal volume, falling linearly with the imbalance to 0 for a
    /// cut that misses: a percentage. Players see it rounded, and the unrounded value breaks ties.
    pub fn calc_score(&self) -> f64 {
        let total_m3 = self.above_m3 + self.below_m3;
        if total_m3 <= 0.0 {
            return 0.0;
        }
        MAX_SCORE * (1.0 - (self.above_m3 - self.below_m3).abs() / total_m3)
    }
}

/// Divides a closed mesh, given in world coordinates, by a plane. Each triangle is clipped to
/// each side and the pieces are summed as signed tetrahedra with their apex on the plane, so the
/// cut face contributes nothing and needs no triangulation.
pub fn calc_split(positions: &[DVec3], triangles: &[[u32; 3]], plane: &Plane) -> Split {
    let mut above_m3 = 0.0;
    let mut below_m3 = 0.0;
    for triangle in triangles {
        let corners = triangle.map(|i| positions[i as usize]);
        above_m3 += calc_clipped_cone_volume(&corners, plane, 1.0);
        below_m3 += calc_clipped_cone_volume(&corners, plane, -1.0);
    }
    Split { above_m3, below_m3 }
}

/// The segment where the triangle with `corners` crosses the plane, if it does, in world
/// coordinates.
pub(crate) fn find_section_segment(corners: &[DVec3; 3], plane: &Plane) -> Option<[DVec3; 2]> {
    let distances = corners.map(|p| (p - plane.point).dot(plane.normal));
    let mut crossings = [DVec3::ZERO; 2];
    let mut crossing_len = 0;
    for k in 0..3 {
        let (d0, d1) = (distances[k], distances[(k + 1) % 3]);
        if (d0 >= 0.0) == (d1 >= 0.0) {
            continue;
        }
        crossings[crossing_len] = calc_edge_crossing(corners[k], corners[(k + 1) % 3], d0, d1);
        crossing_len += 1;
    }
    (crossing_len == 2).then_some(crossings)
}

fn calc_clipped_cone_volume(corners: &[DVec3; 3], plane: &Plane, side: f64) -> f64 {
    let distances = corners.map(|p| side * (p - plane.point).dot(plane.normal));
    let mut polygon = [DVec3::ZERO; 4];
    let mut polygon_len = 0;
    for k in 0..3 {
        let next = (k + 1) % 3;
        if distances[k] >= 0.0 {
            polygon[polygon_len] = corners[k];
            polygon_len += 1;
        }
        if (distances[k] >= 0.0) != (distances[next] >= 0.0) {
            polygon[polygon_len] = calc_edge_crossing(corners[k], corners[next], distances[k], distances[next]);
            polygon_len += 1;
        }
    }
    let apex = plane.point;
    let mut six_volume = 0.0;
    for k in 1..polygon_len.saturating_sub(1) {
        let (a, b, c) = (polygon[0] - apex, polygon[k] - apex, polygon[k + 1] - apex);
        six_volume += a.dot(b.cross(c));
    }
    six_volume / 6.0
}

fn calc_edge_crossing(p0: DVec3, p1: DVec3, d0: f64, d1: f64) -> DVec3 {
    p0 + (p1 - p0) * (d0 / (d0 - d1))
}
