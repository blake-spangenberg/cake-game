use rapier3d::glamx::DVec3;
use rapier3d::math::Vec3;

use crate::triangle_tree::TriangleTree;

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
    #[expect(clippy::cast_possible_truncation, reason = "a mesh's counts fit its u32 indices")]
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
    ///
    /// # Errors
    /// [`MeshError`] for a triangle naming a missing vertex, else for the first bad edge in order
    /// of its vertex indices.
    #[expect(clippy::cast_possible_truncation, reason = "a mesh's counts fit its u32 indices")]
    pub fn validate_closed(&self) -> Result<(), MeshError> {
        let vertex_len = self.positions.len();
        let stray_idx = self.triangles.iter().position(|triangle| triangle.iter().any(|&i| i as usize >= vertex_len));
        if let Some(triangle_idx) = stray_idx {
            return Err(MeshError::IndexOutOfRange { triangle_idx });
        }
        let triangles = &self.triangles;
        let mut edge_ends =
            VertexGroups::build(vertex_len, triangles, |triangle_idx, k| triangles[triangle_idx][(k + 1) % 3]);
        edge_ends.sort_each();
        for from in 0..vertex_len as u32 {
            let ends = edge_ends.get(from);
            for (end_idx, &to) in ends.iter().enumerate() {
                if ends.get(end_idx + 1) == Some(&to) {
                    return Err(MeshError::InconsistentWinding { from, to });
                }
                match edge_ends.get(to).iter().filter(|&&back| back == from).count() {
                    0 => return Err(MeshError::OpenEdge { from, to }),
                    1 => {}
                    _ => return Err(MeshError::NonManifoldEdge { from, to }),
                }
            }
        }
        Ok(())
    }

    /// The enclosed volume, negative when the mesh is wound inside out.
    #[must_use]
    pub fn calc_signed_volume(&self) -> f64 {
        let at = |i: u32| self.positions[i as usize].as_dvec3();
        self.triangles.iter().map(|&[a, b, c]| at(a).dot(at(b).cross(at(c)))).sum::<f64>() / 6.0
    }

    pub fn calc_max_distance_from(&self, point: Vec3) -> f32 {
        self.positions.iter().map(|p| p.distance(point)).fold(0.0, f32::max)
    }

    /// Each triangle corner's unit normal, three per triangle in order, for drawing: the average of
    /// the normals of the triangles around its vertex that meet its own triangle's within
    /// `crease_rad`, so the mesh shades smoothly but for creases.
    #[must_use]
    #[expect(clippy::cast_possible_truncation, reason = "a mesh's counts fit its u32 indices")]
    pub fn calc_corner_normals(&self, crease_rad: f32) -> Vec<Vec3> {
        let face_normals: Vec<Vec3> = self.triangles.iter().map(|&triangle| self.calc_face_normal(triangle)).collect();
        let faces_around = VertexGroups::build(self.positions.len(), &self.triangles, |face_idx, _| face_idx as u32);
        let min_dot = libm::cosf(crease_rad);
        let calc_corner_normal = |vertex: u32, own: Vec3| {
            let near =
                faces_around.get(vertex).iter().map(|&idx| face_normals[idx as usize]).filter(|n| n.dot(own) > min_dot);
            near.sum::<Vec3>().normalize_or_zero()
        };
        self.triangles
            .iter()
            .zip(&face_normals)
            .flat_map(|(triangle, &own)| triangle.map(|vertex| calc_corner_normal(vertex, own)))
            .collect()
    }

    /// The unit normal `triangle` faces along, or zero for one with no area to face.
    #[must_use]
    pub fn calc_face_normal(&self, triangle: [u32; 3]) -> Vec3 {
        let [a, b, c] = triangle.map(|i| self.positions[i as usize]);
        (b - a).cross(c - a).normalize_or_zero()
    }

    /// An upper bound on the longest distance across the mesh: the diameter of the smallest sphere
    /// about its bounding box's center that holds every vertex.
    #[must_use]
    pub fn calc_extent_bound(&self) -> f32 {
        let Some(&first) = self.positions.first() else {
            return 0.0;
        };
        let (min, max) = self.positions.iter().fold((first, first), |(min, max), &p| (min.min(p), max.max(p)));
        2.0 * self.calc_max_distance_from((min + max) / 2.0)
    }

    /// The least and greatest of the mesh's widths along a fixed spread of directions: about how
    /// tall it stands in its flattest and its tallest pose. A `tree` built on this mesh gives the
    /// same result to the bit, sooner.
    #[must_use]
    pub fn calc_width_range_m(&self, tree: Option<&TriangleTree>) -> (f64, f64) {
        let loose_positions = match tree {
            None => self.positions.clone(),
            Some(_) => self.find_unreferenced_positions(),
        };
        calc_width_directions().fold((f64::INFINITY, f64::NEG_INFINITY), |(least_m, greatest_m), direction| {
            let (mut low_m, mut high_m) = calc_span_along(&loose_positions, direction);
            if let Some(tree) = tree {
                let (tree_low_m, tree_high_m) = tree.calc_span_along(&self.positions, &self.triangles, direction);
                (low_m, high_m) = (low_m.min(tree_low_m), high_m.max(tree_high_m));
            }
            let width_m = high_m - low_m;
            (least_m.min(width_m), greatest_m.max(width_m))
        })
    }

    /// The positions no triangle uses.
    fn find_unreferenced_positions(&self) -> Vec<Vec3> {
        let mut is_referenced = vec![false; self.positions.len()];
        for &idx in self.triangles.iter().flatten() {
            is_referenced[idx as usize] = true;
        }
        self.positions.iter().zip(is_referenced).filter(|&(_, is_referenced)| !is_referenced).map(|(&p, _)| p).collect()
    }
}

/// A mesh's corner normals as [`Mesh::calc_corner_normals`] gives them, compactly.
///
/// Each is rounded to an [`OctNormal`] and each distinct one at a vertex stored once: a vertex's
/// corners share one normal unless a crease parts them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CornerNormals {
    /// Each vertex's distinct corner normals, vertex by vertex, a vertex's in the order its corners
    /// first take them in triangle order.
    pub normals: Vec<OctNormal>,
    /// Which of its vertex's normals each triangle's corners take.
    pub picks: Vec<[u8; 3]>,
}

/// A unit vector folded onto an octahedron and unfolded into a square, each coordinate scaled to
/// the full `u16` range: within 1e-4 rad of the vector it rounds.
pub type OctNormal = [u16; 2];

#[derive(Debug, PartialEq, Eq)]
pub enum CornerNormalsError {
    /// A triangle has no area to face, so its corners have no normal.
    Degenerate { triangle_idx: usize },
    /// A vertex has more distinct normals than a pick can name.
    TooManyAtVertex { vertex: u32 },
}

impl CornerNormals {
    /// `mesh`'s corner normals, creased where faces meet at over `crease_rad`.
    ///
    /// # Errors
    /// [`CornerNormalsError`] for the first triangle with no area or vertex with too many normals.
    pub fn calc(mesh: &Mesh, crease_rad: f32) -> Result<Self, CornerNormalsError> {
        let corner_normals = mesh.calc_corner_normals(crease_rad);
        let mut vertex_normals: Vec<Vec<OctNormal>> = vec![Vec::new(); mesh.positions.len()];
        let mut picks = Vec::with_capacity(mesh.triangles.len());
        let triangles = mesh.triangles.iter().zip(corner_normals.as_chunks::<3>().0);
        for (triangle_idx, (triangle, normals)) in triangles.enumerate() {
            let mut triangle_picks = [0; 3];
            for ((pick, &vertex), &normal) in triangle_picks.iter_mut().zip(triangle).zip(normals) {
                if normal == Vec3::ZERO {
                    return Err(CornerNormalsError::Degenerate { triangle_idx });
                }
                let code = encode_oct(normal);
                let known = &mut vertex_normals[vertex as usize];
                let idx = known.iter().position(|&known_code| known_code == code).unwrap_or_else(|| {
                    known.push(code);
                    known.len() - 1
                });
                *pick = u8::try_from(idx).map_err(|_| CornerNormalsError::TooManyAtVertex { vertex })?;
            }
            picks.push(triangle_picks);
        }
        Ok(Self { normals: vertex_normals.concat(), picks })
    }

    /// Whether every corner of `mesh` picks a normal of its own vertex and every normal belongs to
    /// a vertex.
    #[must_use]
    pub fn is_valid_for(&self, mesh: &Mesh) -> bool {
        self.calc_run_starts(mesh).is_some()
    }

    /// Each triangle corner's unit normal, three per triangle in order, as
    /// [`Mesh::calc_corner_normals`] gave them to [`CornerNormals::calc`] but for the rounding.
    ///
    /// # Panics
    /// Unless [`CornerNormals::is_valid_for`] `mesh`.
    #[must_use]
    pub fn to_corners(&self, mesh: &Mesh) -> Vec<Vec3> {
        let starts = self.calc_run_starts(mesh).expect("corner normals that fit their mesh");
        let normals: Vec<Vec3> = self.normals.iter().map(|&code| decode_oct(code)).collect();
        let pick_normal = |vertex: u32, pick: u8| normals[(starts[vertex as usize] + u32::from(pick)) as usize];
        let corners = mesh.triangles.iter().zip(&self.picks);
        corners
            .flat_map(|(&[a, b, c], &[pick_a, pick_b, pick_c])| {
                [pick_normal(a, pick_a), pick_normal(b, pick_b), pick_normal(c, pick_c)]
            })
            .collect()
    }

    /// Where each vertex's run of normals starts in `normals`, or none when the picks do not fit
    /// `mesh`'s triangles or do not account for exactly `normals`.
    fn calc_run_starts(&self, mesh: &Mesh) -> Option<Vec<u32>> {
        if self.picks.len() != mesh.triangles.len() {
            return None;
        }
        let mut run_lens = vec![0u32; mesh.positions.len()];
        for (triangle, picks) in mesh.triangles.iter().zip(&self.picks) {
            for (&vertex, &pick) in triangle.iter().zip(picks) {
                let run_len = run_lens.get_mut(vertex as usize)?;
                *run_len = (*run_len).max(u32::from(pick) + 1);
            }
        }
        let mut next_start = 0;
        let starts = run_lens
            .iter()
            .map(|&run_len| {
                let start = next_start;
                next_start += run_len;
                start
            })
            .collect();
        (next_start as usize == self.normals.len()).then_some(starts)
    }
}

#[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "rounds a coordinate in [0, u16::MAX]")]
fn encode_oct(normal: Vec3) -> OctNormal {
    let folded = normal / normal.abs().element_sum();
    let square = if folded.z < 0.0 { fold_over(folded.x, folded.y) } else { [folded.x, folded.y] };
    square.map(|coord| ((coord * 0.5 + 0.5) * f32::from(u16::MAX)).round() as u16)
}

fn decode_oct(code: OctNormal) -> Vec3 {
    let [x, y] = code.map(|coord| f32::from(coord) / f32::from(u16::MAX) * 2.0 - 1.0);
    let z = 1.0 - x.abs() - y.abs();
    let [x, y] = if z < 0.0 { fold_over(x, y) } else { [x, y] };
    Vec3::new(x, y, z).normalize()
}

/// Moves a point of the octahedron's lower half across the square's diagonal edge, and back.
fn fold_over(x: f32, y: f32) -> [f32; 2] {
    [(1.0 - y.abs()) * sign_of(x), (1.0 - x.abs()) * sign_of(y)]
}

/// 1 or -1, counting -0.0 as positive where `signum` does not.
fn sign_of(coord: f32) -> f32 {
    if coord < 0.0 { -1.0 } else { 1.0 }
}

/// The least and greatest `p.dot(direction)` over `positions`.
fn calc_span_along(positions: &[Vec3], direction: DVec3) -> (f64, f64) {
    positions.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), p| {
        let along_m = p.as_dvec3().dot(direction);
        (low.min(along_m), high.max(along_m))
    })
}

/// Values filed under vertices, each vertex's a contiguous run in the order they were filed.
struct VertexGroups {
    /// Where each vertex's run starts in `values`, then the end of the last.
    starts: Vec<u32>,
    values: Vec<u32>,
}

impl VertexGroups {
    /// Files `value_at(triangle_idx, k)` under each triangle's `k`th corner's vertex, in triangle
    /// order.
    fn build(vertex_len: usize, triangles: &[[u32; 3]], value_at: impl Fn(usize, usize) -> u32) -> Self {
        let mut starts = vec![0u32; vertex_len + 1];
        for &vertex in triangles.iter().flatten() {
            starts[vertex as usize + 1] += 1;
        }
        for vertex in 0..vertex_len {
            starts[vertex + 1] += starts[vertex];
        }
        let mut next_slots = starts[..vertex_len].to_vec();
        let mut values = vec![0u32; 3 * triangles.len()];
        for (triangle_idx, triangle) in triangles.iter().enumerate() {
            for (k, &vertex) in triangle.iter().enumerate() {
                let slot = &mut next_slots[vertex as usize];
                values[*slot as usize] = value_at(triangle_idx, k);
                *slot += 1;
            }
        }
        Self { starts, values }
    }

    fn get(&self, vertex: u32) -> &[u32] {
        let vertex = vertex as usize;
        &self.values[self.starts[vertex] as usize..self.starts[vertex + 1] as usize]
    }

    fn sort_each(&mut self) {
        for vertex in 0..self.starts.len() - 1 {
            let (start, end) = (self.starts[vertex] as usize, self.starts[vertex + 1] as usize);
            self.values[start..end].sort_unstable();
        }
    }
}

/// Directions spread evenly over a hemisphere, which suffices since a width along a direction is
/// the width along its opposite, plus the axes, along which a model's flat faces usually lie.
fn calc_width_directions() -> impl Iterator<Item = DVec3> {
    let golden_angle_rad = core::f64::consts::PI * (3.0 - libm::sqrt(5.0));
    let spread = (0..WIDTH_DIRECTION_LEN).map(move |i| {
        let y = 1.0 - (f64::from(i) + 0.5) / f64::from(WIDTH_DIRECTION_LEN);
        let radius = libm::sqrt(1.0 - y * y);
        let angle_rad = golden_angle_rad * f64::from(i);
        DVec3::new(radius * libm::cos(angle_rad), y, radius * libm::sin(angle_rad))
    });
    [DVec3::X, DVec3::Y, DVec3::Z].into_iter().chain(spread)
}

/// `point_len` points evenly spaced along a circular arc from `angle0` to `angle1` inclusive,
/// counterclockwise seen from above, as `(x, z)`.
#[must_use]
#[expect(clippy::cast_precision_loss, reason = "a point count far below f32's whole-number range")]
pub fn calc_arc_outline(center: [f32; 2], radius: f32, angle0: f32, angle1: f32, point_len: u32) -> Vec<[f32; 2]> {
    (0..point_len)
        .map(|i| {
            let angle = angle0 + (angle1 - angle0) * i as f32 / (point_len - 1) as f32;
            [center[0] + radius * libm::cosf(angle), center[1] - radius * libm::sinf(angle)]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_oct_normal_lands_within_1e_4_rad_of_its_vector_in_every_direction() {
        let spread = calc_width_directions().flat_map(|d| [d, -d]);
        let edges = [Vec3::new(1.0, -1.0, 0.0), Vec3::new(-0.0, 0.0, -1.0), Vec3::new(1e-30, 0.0, -1.0)];
        for direction in spread.map(DVec3::as_vec3).chain(edges.map(Vec3::normalize)) {
            let decoded = decode_oct(encode_oct(direction));
            let error_rad = decoded.as_dvec3().angle_between(direction.as_dvec3());
            assert!(error_rad < 1e-4, "{direction:?} came back {decoded:?}, {error_rad} rad off");
        }
    }
}
