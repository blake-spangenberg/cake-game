use std::collections::BTreeMap;

use rapier3d::math::Vec3;

/// A triangle mesh in a cake's local frame, meters, counter-clockwise winding seen from outside.
#[derive(Clone, Debug)]
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

    pub fn calc_max_distance_from(&self, point: Vec3) -> f32 {
        self.positions.iter().map(|p| p.distance(point)).fold(0.0, f32::max)
    }
}
