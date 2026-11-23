use std::collections::BTreeMap;

use rapier3d::math::Vec3;

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
