//! A bounding volume hierarchy over a mesh's triangles, for finding the few near a plane without
//! checking every one.

use core::ops::ControlFlow;

use rapier3d::glamx::DVec3;
use rapier3d::math::Vec3;

const MAX_LEAF_LEN: usize = 4;
/// More than the search's stack ever holds, one node per level of a tree halving `u32` many
/// triangles at each level plus the one being searched.
const MAX_STACK_LEN: usize = 64;
/// The largest cell index along each axis of the grid whose Morton order groups nearby triangles.
const MORTON_MAX_CELL: f32 = 1023.0;
/// Far beyond the rounding between a box's bound and a corner's `p.dot(direction)` inside it,
/// about 1e-16 m on a cake.
const SPAN_MARGIN_M: f64 = 1e-9;

/// Boxes around ever smaller groups of a mesh's triangles, in the mesh's own coordinates.
#[derive(Clone, Debug)]
pub struct TriangleTree {
    /// Depth first, so each branch's first child directly follows it.
    nodes: Vec<Node>,
    /// Indices into the mesh's triangles, each leaf's a contiguous run.
    triangle_idxs: Vec<u32>,
}

/// The signed distance `p.dot(normal) + offset` of a point `p` in the tree's coordinates.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SignedDistance {
    pub normal: DVec3,
    pub offset: f64,
}

#[derive(Clone, Copy, Debug)]
struct Node {
    center: DVec3,
    half_extent: DVec3,
    children: Children,
}

#[derive(Clone, Copy, Debug)]
enum Children {
    Leaf { start: u32, end: u32 },
    Branch { second_idx: u32 },
}

/// A box in the mesh's coordinates as its least and greatest corners.
type Bounds = (Vec3, Vec3);

impl TriangleTree {
    pub fn build(positions: &[Vec3], triangles: &[[u32; 3]]) -> TriangleTree {
        let mut tree = TriangleTree { nodes: Vec::new(), triangle_idxs: Vec::new() };
        let triangle_bounds: Vec<Bounds> = triangles
            .iter()
            .map(|triangle| {
                let [a, b, c] = triangle.map(|i| positions[i as usize]);
                (a.min(b).min(c), a.max(b).max(c))
            })
            .collect();
        let Some(mesh_bounds) = calc_union(triangle_bounds.iter().copied()) else {
            return tree;
        };
        // Each triangle's Morton code above its index, so one sort of plain integers orders them.
        let mut keys: Vec<u64> = triangle_bounds
            .iter()
            .enumerate()
            .map(|(idx, &bounds)| ((calc_morton_code(bounds, mesh_bounds) as u64) << 32) | idx as u64)
            .collect();
        keys.sort_unstable();
        tree.triangle_idxs = keys.into_iter().map(|key| key as u32).collect();
        // Halving leaves every leaf below the root at least two triangles, so no more nodes than
        // triangles.
        tree.nodes.reserve_exact(triangles.len());
        tree.push_node(&triangle_bounds, 0, triangles.len());
        tree
    }

    /// Calls `on_triangle` with every triangle that might cross the plane where `plane` is 0 and
    /// reach no deeper than `max_depth` along `depth`, give or take `margin`, which includes every
    /// one that does. `on_triangle` may lower `max_depth`, or break to stop the search, which then
    /// breaks too.
    pub(crate) fn search(
        &self,
        plane: SignedDistance,
        depth: SignedDistance,
        max_depth: &mut f64,
        margin: f64,
        mut on_triangle: impl FnMut(u32, &mut f64) -> ControlFlow<()>,
    ) -> ControlFlow<()> {
        if self.nodes.is_empty() {
            return ControlFlow::Continue(());
        }
        let (abs_plane_normal, abs_depth_normal) = (plane.normal.abs(), depth.normal.abs());
        let calc_least_depth =
            |node: &Node| node.center.dot(depth.normal) + depth.offset - node.half_extent.dot(abs_depth_normal);
        let mut stack = [0u32; MAX_STACK_LEN];
        let mut stack_len = 1;
        while stack_len > 0 {
            stack_len -= 1;
            let node_idx = stack[stack_len];
            let node = &self.nodes[node_idx as usize];
            let plane_distance = node.center.dot(plane.normal) + plane.offset;
            if plane_distance.abs() > node.half_extent.dot(abs_plane_normal) + margin {
                continue;
            }
            if calc_least_depth(node) > *max_depth + margin {
                continue;
            }
            match node.children {
                Children::Leaf { start, end } => {
                    for &idx in &self.triangle_idxs[start as usize..end as usize] {
                        on_triangle(idx, max_depth)?;
                    }
                }
                Children::Branch { second_idx } => {
                    // The shallower child pops first, so it can lower `max_depth` before the other.
                    let mut children = [node_idx + 1, second_idx];
                    if calc_least_depth(&self.nodes[children[0] as usize])
                        < calc_least_depth(&self.nodes[children[1] as usize])
                    {
                        children.reverse();
                    }
                    stack[stack_len..stack_len + 2].copy_from_slice(&children);
                    stack_len += 2;
                }
            }
        }
        ControlFlow::Continue(())
    }

    /// The least and greatest `p.dot(direction)` over the corners of `triangles`, which with
    /// `positions` must be the mesh the tree was built on. The same to the bit as checking every
    /// corner, since a box is skipped only when its bound misses the best so far by more than
    /// [`SPAN_MARGIN_M`].
    pub(crate) fn calc_span_along(&self, positions: &[Vec3], triangles: &[[u32; 3]], direction: DVec3) -> (f64, f64) {
        let low = -self.calc_greatest_along(positions, triangles, -direction);
        (low, self.calc_greatest_along(positions, triangles, direction))
    }

    fn calc_greatest_along(&self, positions: &[Vec3], triangles: &[[u32; 3]], direction: DVec3) -> f64 {
        let mut greatest = f64::NEG_INFINITY;
        if self.nodes.is_empty() {
            return greatest;
        }
        let abs_direction = direction.abs();
        let calc_bound = |node_idx: u32| {
            let node = &self.nodes[node_idx as usize];
            (node_idx, node.center.dot(direction) + node.half_extent.dot(abs_direction))
        };
        let mut stack = [(0u32, 0.0f64); MAX_STACK_LEN];
        stack[0] = calc_bound(0);
        let mut stack_len = 1;
        while stack_len > 0 {
            stack_len -= 1;
            let (node_idx, bound) = stack[stack_len];
            if bound + SPAN_MARGIN_M < greatest {
                continue;
            }
            match self.nodes[node_idx as usize].children {
                Children::Leaf { start, end } => {
                    let corners = self.triangle_idxs[start as usize..end as usize]
                        .iter()
                        .flat_map(|&idx| triangles[idx as usize]);
                    for corner in corners {
                        greatest = greatest.max(positions[corner as usize].as_dvec3().dot(direction));
                    }
                }
                Children::Branch { second_idx } => {
                    // The child reaching further pops first, so it can raise `greatest` before the other.
                    let mut children = [calc_bound(node_idx + 1), calc_bound(second_idx)];
                    if children[0].1 > children[1].1 {
                        children.reverse();
                    }
                    for child in children.into_iter().filter(|&(_, bound)| bound + SPAN_MARGIN_M >= greatest) {
                        stack[stack_len] = child;
                        stack_len += 1;
                    }
                }
            }
        }
        greatest
    }

    /// Pushes the node over `triangle_idxs[start..end]` and, depth first, those below it, each
    /// branch halving its triangles in their Morton order. Returns the node's bounds.
    fn push_node(&mut self, triangle_bounds: &[Bounds], start: usize, end: usize) -> Bounds {
        let node_idx = self.nodes.len();
        let leaf = Children::Leaf { start: start as u32, end: end as u32 };
        self.nodes.push(Node { center: DVec3::ZERO, half_extent: DVec3::ZERO, children: leaf });
        let (min, max) = if end - start <= MAX_LEAF_LEN {
            let idxs = &self.triangle_idxs[start..end];
            calc_union(idxs.iter().map(|&idx| triangle_bounds[idx as usize])).unwrap()
        } else {
            let middle = start + (end - start) / 2;
            let first = self.push_node(triangle_bounds, start, middle);
            let second_idx = self.nodes.len() as u32;
            let second = self.push_node(triangle_bounds, middle, end);
            self.nodes[node_idx].children = Children::Branch { second_idx };
            (first.0.min(second.0), first.1.max(second.1))
        };
        let (min_m, max_m) = (min.as_dvec3(), max.as_dvec3());
        let node = &mut self.nodes[node_idx];
        (node.center, node.half_extent) = ((min_m + max_m) / 2.0, (max_m - min_m) / 2.0);
        (min, max)
    }
}

fn calc_union(mut boxes: impl Iterator<Item = Bounds>) -> Option<Bounds> {
    let first = boxes.next()?;
    Some(boxes.fold(first, |(min, max), (box_min, box_max)| (min.min(box_min), max.max(box_max))))
}

/// The Morton code of the box's center within `mesh_bounds`: its grid cell's coordinates with
/// their bits interleaved, so boxes near one another mostly get codes near one another.
fn calc_morton_code((min, max): Bounds, (mesh_min, mesh_max): Bounds) -> u32 {
    let extent = (mesh_max - mesh_min).max(Vec3::splat(f32::MIN_POSITIVE));
    let cell = ((min + max) / 2.0 - mesh_min) / extent * MORTON_MAX_CELL;
    let [x, y, z] = cell.to_array().map(|coord| spread_bits(coord.clamp(0.0, MORTON_MAX_CELL) as u32));
    x | (y << 1) | (z << 2)
}

/// The low 10 bits of `value` spread to every third bit.
fn spread_bits(value: u32) -> u32 {
    let mut bits = value & 0x3ff;
    bits = (bits | (bits << 16)) & 0x0300_00ff;
    bits = (bits | (bits << 8)) & 0x0300_f00f;
    bits = (bits | (bits << 4)) & 0x030c_30c3;
    (bits | (bits << 2)) & 0x0924_9249
}
