//! Whether a cake resting on a flat surface stays put the way a soft cake would.
//!
//! A rigid body rests only where its center of mass stands over what touches the surface, so on a
//! rounded side it rolls on until it finds a flat one. A cake squashes, and rests on the patch its
//! squash flattens. Here the patch is every part of the collider hulls within [`SQUASH_M`] of the
//! surface, which equals the hulls having a face cut that deep wherever they touch, so a cake
//! rests where hulls with such a face would, without a hull face reaching more than that inside
//! the drawn mesh. On a gently curved side the patch is wide; across a sharp edge it is narrow, so a
//! cake still tips off an edge unless set within a hair of balance.

use rapier3d::glamx::{DQuat, DVec2, DVec3};
use rapier3d::prelude::*;

/// How deep a resting cake squashes. It matches how far a hull may sit inside the drawn mesh before
/// a resting cake looks sunk.
pub(super) const SQUASH_M: f64 = 0.002;

/// The collider hulls' corners and edges, in the cake's own coordinates.
pub(super) struct HullFrame {
    corners: Vec<DVec3>,
    edges: Vec<[usize; 2]>,
}

impl HullFrame {
    /// # Panics
    /// On a shape that is not a convex polyhedron, which [`super::Cake`] never builds.
    pub(super) fn build(hull_shapes: &[SharedShape]) -> Self {
        let mut frame = Self { corners: Vec::new(), edges: Vec::new() };
        for shape in hull_shapes {
            let hull = shape.as_convex_polyhedron().expect("a cake's hulls are convex polyhedra");
            let first = frame.corners.len();
            frame.corners.extend(hull.points().iter().map(|p| p.as_dvec3()));
            frame.edges.extend(hull.edges().iter().map(|edge| edge.vertices.map(|idx| first + idx as usize)));
        }
        frame
    }

    /// Whether `com`, the center of mass, stands over the patch of the hulls posed at `rotation`
    /// and `translation` that lies within [`SQUASH_M`] of their lowest point and inside `is_supported`'s
    /// reach, as seen from above.
    pub(super) fn is_over_squashed_patch(
        &self,
        rotation: DQuat,
        translation: DVec3,
        com: DVec3,
        is_supported: impl Fn(DVec2) -> bool,
    ) -> bool {
        let corners: Vec<DVec3> = self.corners.iter().map(|&p| rotation * p + translation).collect();
        let low_y_m = corners.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
        let cut_y_m = low_y_m + SQUASH_M;
        let below = corners.iter().filter(|p| p.y <= cut_y_m).map(|&p| as_plan(p));
        let crossings = self.edges.iter().filter_map(|&[a, b]| {
            let (a, b) = (corners[a], corners[b]);
            if (a.y <= cut_y_m) == (b.y <= cut_y_m) {
                return None;
            }
            let share = (cut_y_m - a.y) / (b.y - a.y);
            Some(as_plan(a + (b - a) * share))
        });
        let patch: Vec<DVec2> = below.chain(crossings).filter(|&p| is_supported(p)).collect();
        is_inside_hull_2d(&patch, as_plan(com))
    }
}

/// Where `p` lies seen from above, as (x, z).
const fn as_plan(p: DVec3) -> DVec2 {
    DVec2::new(p.x, p.z)
}

/// Whether `point` lies inside the convex hull of `points`, by Andrew's monotone chain, which needs
/// only products and so gives the same answer on every platform.
fn is_inside_hull_2d(points: &[DVec2], point: DVec2) -> bool {
    let mut sorted = points.to_vec();
    sorted.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    let mut hull: Vec<DVec2> = Vec::with_capacity(sorted.len() + 1);
    push_chain(&mut hull, sorted.iter().copied());
    push_chain(&mut hull, sorted.iter().rev().copied());
    if hull.len() < 3 {
        return false;
    }
    (0..hull.len()).all(|idx| calc_turn(hull[idx], hull[(idx + 1) % hull.len()], point) >= 0.0)
}

/// Appends to `hull` the chain through `points` that turns only counterclockwise, keeping what it
/// already holds, then drops the chain's last point, with which the next chain starts.
fn push_chain(hull: &mut Vec<DVec2>, points: impl Iterator<Item = DVec2>) {
    let floor = hull.len();
    for p in points {
        while hull.len() >= floor + 2 && calc_turn(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
            hull.pop();
        }
        hull.push(p);
    }
    hull.pop();
}

/// Twice the signed area of the triangle `a`, `b`, `c`: positive when it turns counterclockwise.
fn calc_turn(a: DVec2, b: DVec2, c: DVec2) -> f64 {
    (b - a).perp_dot(c - a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_inside_a_square_of_points_is_inside_and_one_beyond_its_edge_is_not() {
        let square = [
            DVec2::new(0.0, 0.0),
            DVec2::new(1.0, 0.0),
            DVec2::new(1.0, 1.0),
            DVec2::new(0.0, 1.0),
            DVec2::new(0.5, 0.5),
        ];
        assert!(is_inside_hull_2d(&square, DVec2::new(0.9, 0.2)));
        assert!(!is_inside_hull_2d(&square, DVec2::new(1.01, 0.5)));
        assert!(!is_inside_hull_2d(&square[..2], DVec2::new(0.5, 0.0)));
    }
}
