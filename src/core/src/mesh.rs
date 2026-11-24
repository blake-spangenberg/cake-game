use std::collections::BTreeMap;

use rapier3d::glamx::DVec3;
use rapier3d::math::Vec3;

/// How many directions [`Mesh::calc_width_range_m`] spreads over a hemisphere, about 9 degrees
/// apart.
const WIDTH_DIRECTION_LEN: u32 = 256;

/// A triangle mesh in a cake's local frame, meters, counter-clockwise winding seen from outside.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum MeshError {
    IndexOutOfRange { triangle_idx: usize },
    OpenEdge { from: u32, to: u32 },
    NonManifoldEdge { from: u32, to: u32 },
    InconsistentWinding { from: u32, to: u32 },
}

impl Mesh {
    /// Adds a closed prism from `y0` up to `y1` over a convex outline of `(x, z)` points,
    /// counterclockwise seen from above.
    pub fn push_prism(&mut self, outline: &[[f32; 2]], y0: f32, y1: f32) {
        debug_assert!(outline.len() >= 3);
        let outline_len = outline.len() as u32;
        let base = self.positions.len() as u32;
        for &[x, z] in outline {
            self.positions.extend([Vec3::new(x, y0, z), Vec3::new(x, y1, z)]);
        }
        let bottom = |i: u32| base + 2 * (i % outline_len);
        let top = |i: u32| bottom(i) + 1;
        for i in 1..outline_len - 1 {
            self.triangles.extend([[top(0), top(i), top(i + 1)], [bottom(0), bottom(i + 1), bottom(i)]]);
        }
        for i in 0..outline_len {
            let (b0, t0, b1, t1) = (bottom(i), top(i), bottom(i + 1), top(i + 1));
            self.triangles.extend([[b0, b1, t1], [b0, t1, t0]]);
        }
    }

    /// Accepts the mesh only if it is closed: every edge borders exactly two triangles, which
    /// traverse it in opposite directions.
    pub fn validate_closed(&self) -> Result<(), MeshError> {
        let vertex_len = self.positions.len() as u32;
        let mut directed_edge_counts: BTreeMap<(u32, u32), u32> = BTreeMap::new();
        for (triangle_idx, triangle) in self.triangles.iter().enumerate() {
            if triangle.iter().any(|&i| i >= vertex_len) {
                return Err(MeshError::IndexOutOfRange { triangle_idx });
            }
            for k in 0..3 {
                let edge = (triangle[k], triangle[(k + 1) % 3]);
                *directed_edge_counts.entry(edge).or_default() += 1;
            }
        }
        for (&(from, to), &count) in &directed_edge_counts {
            if count > 1 {
                return Err(MeshError::InconsistentWinding { from, to });
            }
            match directed_edge_counts.get(&(to, from)) {
                None => return Err(MeshError::OpenEdge { from, to }),
                Some(&reverse_count) if reverse_count > 1 => {
                    return Err(MeshError::NonManifoldEdge { from, to });
                }
                Some(_) => {}
            }
        }
        Ok(())
    }

    /// The enclosed volume, negative when the mesh is wound inside out.
    pub fn calc_signed_volume(&self) -> f64 {
        let at = |i: u32| self.positions[i as usize].as_dvec3();
        self.triangles.iter().map(|&[a, b, c]| at(a).dot(at(b).cross(at(c)))).sum::<f64>() / 6.0
    }

    pub fn calc_max_distance_from(&self, point: Vec3) -> f32 {
        self.positions.iter().map(|p| p.distance(point)).fold(0.0, f32::max)
    }

    /// An upper bound on the longest distance across the mesh: the diameter of the smallest sphere
    /// about its bounding box's center that holds every vertex.
    pub fn calc_extent_bound(&self) -> f32 {
        let Some(&first) = self.positions.first() else {
            return 0.0;
        };
        let (min, max) = self.positions.iter().fold((first, first), |(min, max), &p| (min.min(p), max.max(p)));
        2.0 * self.calc_max_distance_from((min + max) / 2.0)
    }

    /// The least and greatest of the mesh's widths along a fixed spread of directions: about how
    /// tall it stands in its flattest and its tallest pose.
    pub(crate) fn calc_width_range_m(&self) -> (f64, f64) {
        calc_width_directions().fold((f64::INFINITY, f64::NEG_INFINITY), |(least_m, greatest_m), direction| {
            let width_m = self.calc_width_m(direction);
            (least_m.min(width_m), greatest_m.max(width_m))
        })
    }

    fn calc_width_m(&self, direction: DVec3) -> f64 {
        let (low_m, high_m) = self.positions.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), p| {
            let along_m = p.as_dvec3().dot(direction);
            (low.min(along_m), high.max(along_m))
        });
        high_m - low_m
    }
}

/// Directions spread evenly over a hemisphere, which suffices since a width along a direction is
/// the width along its opposite, plus the axes, along which a model's flat faces usually lie.
fn calc_width_directions() -> impl Iterator<Item = DVec3> {
    let golden_angle_rad = core::f64::consts::PI * (3.0 - libm::sqrt(5.0));
    let spread = (0..WIDTH_DIRECTION_LEN).map(move |i| {
        let y = 1.0 - (i as f64 + 0.5) / WIDTH_DIRECTION_LEN as f64;
        let radius = libm::sqrt(1.0 - y * y);
        let angle_rad = golden_angle_rad * i as f64;
        DVec3::new(radius * libm::cos(angle_rad), y, radius * libm::sin(angle_rad))
    });
    [DVec3::X, DVec3::Y, DVec3::Z].into_iter().chain(spread)
}

/// `point_len` points evenly spaced along a circular arc from `angle0` to `angle1` inclusive,
/// counterclockwise seen from above, as `(x, z)`.
pub fn calc_arc_outline(center: [f32; 2], radius: f32, angle0: f32, angle1: f32, point_len: u32) -> Vec<[f32; 2]> {
    (0..point_len)
        .map(|i| {
            let angle = angle0 + (angle1 - angle0) * i as f32 / (point_len - 1) as f32;
            [center[0] + radius * libm::cosf(angle), center[1] - radius * libm::sinf(angle)]
        })
        .collect()
}
