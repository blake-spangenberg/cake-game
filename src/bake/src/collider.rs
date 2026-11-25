//! The cake's collider: convex hulls that between them reach to within [`HULL_SLACK_M`] of every
//! point of the drawn mesh, so the cake draws no deeper than that inside the stand, the table, or
//! the blade it rests or slides against. [`HullFit::TrimOverhangs`] trades some of that reach for
//! hulls that stand less far outside the mesh.

use cake_core::cakes::MAX_HULL_VERTICES;
use cake_core::mesh::Mesh;
use rapier3d::glamx::DVec3;
use rapier3d::math::Vec3;
use rapier3d::parry::query::PointQuery;
use rapier3d::parry::shape::{TriMesh, TriMeshFlags};
use rapier3d::parry::transformation::convex_hull;
use rapier3d::parry::transformation::vhacd::{VHACD, VHACDParameters};

/// Voxels along the longest side when decomposing: finer finds thinner concavities.
const VHACD_RESOLUTION: u32 = 64;
/// How concave a part may stay, as a share of the model's size, before it is split.
const VHACD_CONCAVITY: f32 = 0.01;
/// Sets how deep V-HACD splits: up to twice the next power of two at or above it in parts.
const MAX_HULLS: u32 = 16;
/// A hull with more vertices than this many times [`MAX_HULL_VERTICES`] sheds vertices in
/// batches by volume, since pricing every vertex's removal one drop at a time costs time cubic in
/// the count.
const COARSE_STAGE_FACTOR: usize = 4;
/// A batch of the coarse stage sheds one in this many of a hull's vertices.
const COARSE_BATCH_SHARE: usize = 4;
/// Directions along which a dense hull's support points stand in for it, spread over the sphere.
const SUPPORT_DIRECTION_LEN: usize = 512;
/// How far the mesh may reach outside its hulls. Holding every point would float a cake resting
/// on a rounded side, since a hull of [`MAX_HULL_VERTICES`] can only hug a curve from outside with
/// faces that stand off it between their corners.
pub const HULL_SLACK_M: f64 = 1.5e-3;
/// How many times a hull's vertices are pushed out to cover what the last push left uncovered.
const PUSH_ROUNDS: usize = 3;
/// A vertex pushed out along the mean of its faces' normals moves at most 1 over this as far as
/// its faces need, which bounds how far a sharp corner overshoots.
const MIN_PUSH_COS: f64 = 0.7;
/// The farthest a pushed vertex may stand past the part along its push.
const MAX_FLOAT_M: f64 = 2e-3;
/// A part whose hull reaches more than this outside the mesh, spanning a concavity such as the gap
/// between a piano's keyboard and its pedal lyre, is split in two where that helps.
const SPLIT_OVERHANG_M: f64 = 0.01;
/// The most hulls splitting may bring a cake to; each hull adds to every tick's contact work.
const MAX_SPLIT_HULLS: usize = 24;
/// Where along a part's span along each axis a split plane is tried.
const SPLIT_SHARES: [f32; 3] = [0.25, 0.5, 0.75];
/// How far past a split plane each half keeps the other's points.
const SPLIT_OVERLAP_M: f64 = 2e-3;
/// Samples along each side of a fitted hull's triangles when measuring how far it reaches out.
const OVERHANG_FIT_STEPS: u32 = 6;
/// Samples along each side of a candidate half's dense hull triangles when choosing a split.
const OVERHANG_SPLIT_STEPS: u32 = 2;

/// How a cake's hulls are fitted, a per-cake bake option.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HullFit {
    /// Each decomposed part gets one hull, pushed out as far as covering the mesh needs.
    Standard,
    /// Pushes stop [`MAX_FLOAT_M`] past the mesh, so no corner floats the cake on the side it rests
    /// on, and a hull bridging a concavity more than [`SPLIT_OVERHANG_M`] outside the mesh, such as
    /// the gap under a piano's keyboard, is split, at the cost of more hulls and wider gaps.
    TrimOverhangs,
}

/// The mesh's approximate convex decomposition, each part fitted with a hull of at most
/// [`MAX_HULL_VERTICES`] vertices that reaches to within about [`HULL_SLACK_M`] of every point of
/// the mesh's surface in that part, and no lower than the mesh, as `fit` bounds it.
pub fn build_hulls(mesh: &Mesh, fit: HullFit) -> Result<Vec<Vec<Vec3>>, String> {
    let parameters = VHACDParameters {
        resolution: VHACD_RESOLUTION,
        concavity: VHACD_CONCAVITY,
        max_convex_hulls: MAX_HULLS,
        ..Default::default()
    };
    let decomposition = VHACD::decompose(&parameters, &mesh.positions, &mesh.triangles, true);
    let floor_y_m = mesh.positions.iter().map(|p| f64::from(p.y)).fold(f64::INFINITY, f64::min);
    let parts: Vec<Vec<Vec3>> = decomposition
        .compute_primitive_intersections(&mesh.positions, &mesh.triangles)
        .into_iter()
        .filter(|surface| !surface.is_empty())
        .collect();
    match fit {
        HullFit::Standard => parts
            .iter()
            .map(|points| fit_hull(points, floor_y_m, fit).ok_or_else(|| "a collider hull is flat".to_string()))
            .collect(),
        HullFit::TrimOverhangs => fit_splitting_overhangs(parts, mesh, floor_y_m),
    }
}

/// `parts` fitted as [`HullFit::TrimOverhangs`] does, splitting the part whose hull reaches
/// farthest outside `mesh` first, while one reaches more than [`SPLIT_OVERHANG_M`] and the cake
/// has fewer than [`MAX_SPLIT_HULLS`] hulls.
fn fit_splitting_overhangs(parts: Vec<Vec<Vec3>>, mesh: &Mesh, floor_y_m: f64) -> Result<Vec<Vec<Vec3>>, String> {
    let solid = TriMesh::with_flags(mesh.positions.clone(), mesh.triangles.clone(), TriMeshFlags::ORIENTED)
        .map_err(|error| format!("cannot measure the hulls against the mesh: {error:?}"))?;
    let context = FitContext { floor_y_m, solid };
    let mut fitted =
        parts.into_iter().map(|points| FittedPart::fit(points, &context)).collect::<Result<Vec<_>, _>>()?;
    while fitted.len() < MAX_SPLIT_HULLS {
        let Some(worst_idx) = (0..fitted.len())
            .filter(|&idx| fitted[idx].is_splittable && fitted[idx].overhang_m > SPLIT_OVERHANG_M)
            .max_by(|&a, &b| fitted[a].overhang_m.total_cmp(&fitted[b].overhang_m))
        else {
            break;
        };
        let worst = &fitted[worst_idx];
        let worst_overhang_m = worst.overhang_m;
        let halves = match split_part(&worst.points, worst.overhang_at, &context.solid) {
            Some((below, above)) => Some([FittedPart::fit(below, &context)?, FittedPart::fit(above, &context)?]),
            None => None,
        };
        match halves {
            Some([below, above]) if below.overhang_m.max(above.overhang_m) < worst_overhang_m => {
                fitted[worst_idx] = below;
                fitted.push(above);
            }
            _ => fitted[worst_idx].is_splittable = false,
        }
    }
    Ok(fitted.into_iter().map(|part| part.hull).collect())
}

/// What fitting a part's hull needs to know about the whole mesh.
struct FitContext {
    /// The mesh's lowest height, which no hull may reach below.
    floor_y_m: f64,
    /// The mesh as a solid, to measure how far a hull reaches outside it.
    solid: TriMesh,
}

/// A decomposed part with its fitted hull and how far that hull reaches outside the mesh.
struct FittedPart {
    points: Vec<Vec3>,
    hull: Vec<Vec3>,
    overhang_m: f64,
    /// The point on the hull's surface farthest outside the mesh.
    overhang_at: Vec3,
    /// False once a split has failed to reach out less.
    is_splittable: bool,
}

impl FittedPart {
    fn fit(points: Vec<Vec3>, context: &FitContext) -> Result<Self, String> {
        let hull = fit_hull(&points, context.floor_y_m, HullFit::TrimOverhangs).ok_or("a collider hull is flat")?;
        let (overhang_m, overhang_at) = calc_overhang_m(&hull, &context.solid, OVERHANG_FIT_STEPS);
        Ok(Self { points, hull, overhang_m, overhang_at, is_splittable: true })
    }
}

/// `points` split in two by the axis-aligned plane whose halves' convex hulls reach least far
/// outside `solid`, tried through `overhang_at`, where the part's hull reaches farthest out, and
/// through a quarter, half, and three quarters of the points' span along each axis. Points within
/// [`SPLIT_OVERLAP_M`] of the plane go to both halves, so their hulls meet without a seam. `None`
/// when no plane leaves both halves solid.
fn split_part(points: &[Vec3], overhang_at: Vec3, solid: &TriMesh) -> Option<(Vec<Vec3>, Vec<Vec3>)> {
    let mut best: Option<(f64, Vec<Vec3>, Vec<Vec3>)> = None;
    #[expect(clippy::cast_possible_truncation, reason = "the points are f32")]
    let overlap_m = SPLIT_OVERLAP_M as f32;
    for axis in 0..3 {
        let low = points.iter().map(|p| p[axis]).fold(f32::INFINITY, f32::min);
        let high = points.iter().map(|p| p[axis]).fold(f32::NEG_INFINITY, f32::max);
        let cuts = SPLIT_SHARES.iter().map(|share| low + (high - low) * share).chain([overhang_at[axis]]);
        for cut in cuts {
            let below: Vec<Vec3> = points.iter().copied().filter(|p| p[axis] <= cut + overlap_m).collect();
            let above: Vec<Vec3> = points.iter().copied().filter(|p| p[axis] >= cut - overlap_m).collect();
            let halves = [&below, &above].map(|half| {
                let hull = convex_hull(half).0;
                calc_hull_volume(&hull).filter(|volume| *volume > 0.0).map(|_| hull)
            });
            let [Some(below_hull), Some(above_hull)] = halves else { continue };
            let overhang_m = calc_overhang_m(&below_hull, solid, OVERHANG_SPLIT_STEPS)
                .0
                .max(calc_overhang_m(&above_hull, solid, OVERHANG_SPLIT_STEPS).0);
            if best.as_ref().is_none_or(|(least, _, _)| overhang_m < *least) {
                best = Some((overhang_m, below, above));
            }
        }
    }
    best.map(|(_, below, above)| (below, above))
}

/// How far the convex hull of `vertices` reaches outside `solid`, and where, sampled at `steps`
/// points along each side of each hull triangle.
#[expect(clippy::cast_precision_loss, reason = "sample counts far below f32's whole-number range")]
fn calc_overhang_m(vertices: &[Vec3], solid: &TriMesh, steps: u32) -> (f64, Vec3) {
    let (hull_vertices, triangles) = convex_hull(vertices);
    let mut farthest = (0.0f64, Vec3::ZERO);
    for [a, b, c] in triangles.iter().map(|corners| corners.map(|idx| hull_vertices[idx as usize])) {
        for i in 0..=steps {
            for j in 0..=steps - i {
                let (u, v) = (i as f32 / steps as f32, j as f32 / steps as f32);
                let sample = a * (1.0 - u - v) + b * u + c * v;
                let projection = solid.project_local_point(sample, true);
                let distance_m = f64::from(sample.distance(projection.point));
                if !projection.is_inside && distance_m > farthest.0 {
                    farthest = (distance_m, sample);
                }
            }
        }
    }
    farthest
}

/// The hull of `points` cut to at most [`MAX_HULL_VERTICES`] of its own vertices, each drop the
/// one that leaves the smallest worst gap to the part's extreme points, then pushed out to bring
/// those within [`HULL_SLACK_M`], never below `floor_y_m`, as `fit` bounds the push. `None` when
/// the points are flat.
fn fit_hull(points: &[Vec3], floor_y_m: f64, fit: HullFit) -> Option<Vec<Vec3>> {
    let mut vertices = reduce_to_support(convex_hull(points).0);
    calc_hull_volume(&vertices).filter(|volume| *volume > 0.0)?;
    // The part's points lie in or next to the hull of these, and the distance to a convex hull is
    // convex, so the farthest of these outside a hull bounds how far the part reaches out of it.
    let extremes: Vec<DVec3> = vertices.iter().map(|v| v.as_dvec3()).collect();
    while vertices.len() > COARSE_STAGE_FACTOR * MAX_HULL_VERTICES {
        vertices = drop_cheapest(&vertices)?;
    }
    while vertices.len() > MAX_HULL_VERTICES {
        vertices = drop_least_gap(&vertices, &extremes)?;
    }
    for _ in 0..PUSH_ROUNDS {
        vertices = push_out(&vertices, &extremes, floor_y_m, fit);
    }
    Some(vertices)
}

/// [`SUPPORT_DIRECTION_LEN`] unit directions spread evenly over the sphere.
#[expect(clippy::cast_precision_loss, reason = "direction counts far below f32's whole-number range")]
fn build_sphere_directions() -> impl Iterator<Item = Vec3> {
    let golden_angle = core::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
    (0..SUPPORT_DIRECTION_LEN).map(move |idx| {
        let y = 1.0 - 2.0 * (idx as f32 + 0.5) / SUPPORT_DIRECTION_LEN as f32;
        let ring_radius = (1.0 - y * y).sqrt();
        let angle = golden_angle * idx as f32;
        Vec3::new(ring_radius * angle.cos(), y, ring_radius * angle.sin())
    })
}

/// A dense hull's support points along [`SUPPORT_DIRECTION_LEN`] directions, the hull itself
/// when it has few vertices.
fn reduce_to_support(vertices: Vec<Vec3>) -> Vec<Vec3> {
    if vertices.len() <= COARSE_STAGE_FACTOR * MAX_HULL_VERTICES {
        return vertices;
    }
    let mut support: Vec<Vec3> = build_sphere_directions()
        .map(|direction| {
            vertices.iter().copied().max_by(|a, b| a.dot(direction).total_cmp(&b.dot(direction))).unwrap_or_default()
        })
        .collect();
    support.sort_by(|a, b| a.to_array().partial_cmp(&b.to_array()).unwrap_or(core::cmp::Ordering::Equal));
    support.dedup();
    convex_hull(&support).0
}

/// The hull's faces as outward unit normals and offsets, with the vertices each spans.
fn build_faces(vertices: &[Vec3], triangles: &[[u32; 3]]) -> Vec<([usize; 3], DVec3, f64)> {
    triangles
        .iter()
        .filter_map(|&[a, b, c]| {
            let corners = [a, b, c].map(|corner| corner as usize);
            let [pa, pb, pc] = corners.map(|corner| vertices[corner].as_dvec3());
            let normal = (pb - pa).cross(pc - pa).try_normalize()?;
            Some((corners, normal, normal.dot(pa)))
        })
        .collect()
}

/// How far `point` lies outside the farthest of `faces`' planes, at most its distance from their
/// hull, and 0 inside.
fn calc_plane_gap_m(faces: &[([usize; 3], DVec3, f64)], point: DVec3) -> f64 {
    faces.iter().map(|(_, normal, offset)| normal.dot(point) - offset).fold(0.0, f64::max)
}

/// `vertices` with the one dropped whose removal leaves the hull's worst gap to `extremes` least.
fn drop_least_gap(vertices: &[Vec3], extremes: &[DVec3]) -> Option<Vec<Vec3>> {
    let mut best: Option<(f64, Vec<Vec3>)> = None;
    for idx in 0..vertices.len() {
        let mut rest = vertices.to_vec();
        rest.swap_remove(idx);
        let (hull_vertices, triangles) = convex_hull(&rest);
        let faces = build_faces(&hull_vertices, &triangles);
        if faces.len() < 4 {
            continue;
        }
        let gap = extremes.iter().map(|p| calc_plane_gap_m(&faces, *p)).fold(0.0, f64::max);
        if best.as_ref().is_none_or(|(least, _)| gap < *least) {
            best = Some((gap, hull_vertices));
        }
    }
    best.map(|(_, hull)| hull)
}

/// `vertices`' hull with each vertex pushed out along the mean of its faces' normals, as far as
/// brings every one of `extremes` to within [`HULL_SLACK_M`] of the faces around it, and kept no
/// lower than `floor_y_m`. With [`HullFit::TrimOverhangs`] a step instead stops where it would
/// pass below `floor_y_m`, or [`MAX_FLOAT_M`] past `extremes` along the step. Each extreme point
/// counts against the face it lies farthest outside.
fn push_out(vertices: &[Vec3], extremes: &[DVec3], floor_y_m: f64, fit: HullFit) -> Vec<Vec3> {
    let (hull_vertices, triangles) = convex_hull(vertices);
    let faces = build_faces(&hull_vertices, &triangles);
    let mut needs = vec![0.0f64; faces.len()];
    for point in extremes {
        let farthest = faces
            .iter()
            .enumerate()
            .map(|(idx, (_, normal, offset))| (idx, normal.dot(*point) - offset))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((face_idx, gap_m)) = farthest {
            needs[face_idx] = needs[face_idx].max(gap_m - HULL_SLACK_M);
        }
    }
    (0..hull_vertices.len())
        .map(|vertex| {
            let point = hull_vertices[vertex].as_dvec3();
            let around: Vec<(DVec3, f64)> = faces
                .iter()
                .zip(&needs)
                .filter(|((corners, _, _), _)| corners.contains(&vertex))
                .map(|((_, normal, _), need)| (*normal, *need))
                .collect();
            let need = around.iter().map(|(_, need)| *need).fold(0.0, f64::max);
            let Some(direction) = around.iter().map(|(normal, _)| *normal).sum::<DVec3>().try_normalize() else {
                return point.as_vec3();
            };
            let least_cos =
                around.iter().map(|(normal, _)| normal.dot(direction)).fold(1.0, f64::min).max(MIN_PUSH_COS);
            let mut step = need / least_cos;
            if fit == HullFit::TrimOverhangs {
                // A step cut short at the floor, rather than flattened onto it, keeps a corner on
                // the bottom edge from sliding sideways.
                if direction.y < 0.0 {
                    step = step.min((point.y - floor_y_m).max(0.0) / -direction.y);
                }
                let reach = extremes.iter().map(|p| p.dot(direction)).fold(f64::NEG_INFINITY, f64::max);
                step = step.min((reach + MAX_FLOAT_M - point.dot(direction)).max(0.0));
            }
            let pushed = point + direction * step;
            pushed.with_y(pushed.y.max(floor_y_m)).as_vec3()
        })
        .collect()
}

/// `vertices`' hull without the [`COARSE_BATCH_SHARE`] share of them whose removal each costs it
/// the least volume, or `None` where that leaves it flat.
fn drop_cheapest(vertices: &[Vec3]) -> Option<Vec<Vec3>> {
    let drop_len = vertices.len() / COARSE_BATCH_SHARE;
    let mut costs: Vec<(usize, f32)> = (0..vertices.len())
        .map(|idx| {
            let mut rest = vertices.to_vec();
            rest.swap_remove(idx);
            (idx, calc_hull_volume(&rest).unwrap_or(0.0))
        })
        .collect();
    costs.sort_by(|a, b| b.1.total_cmp(&a.1));
    let dropped: Vec<usize> = costs.iter().take(drop_len).map(|&(idx, _)| idx).collect();
    let rest: Vec<Vec3> =
        vertices.iter().enumerate().filter(|(idx, _)| !dropped.contains(idx)).map(|(_, &v)| v).collect();
    let rest = convex_hull(&rest).0;
    (calc_hull_volume(&rest)? > 0.0).then_some(rest)
}

/// The volume of the points' convex hull, or `None` when there are too few to span one.
fn calc_hull_volume(points: &[Vec3]) -> Option<f32> {
    if points.len() < 4 {
        return None;
    }
    let (vertices, triangles) = convex_hull(points);
    let volume = triangles
        .iter()
        .map(|&[a, b, c]| vertices[a as usize].dot(vertices[b as usize].cross(vertices[c as usize])))
        .sum::<f32>()
        / 6.0;
    Some(volume.abs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[expect(clippy::cast_precision_loss, reason = "point counts far below f32's whole-number range")]
    fn build_sphere_points(point_len: usize, radius: f32) -> Vec<Vec3> {
        let golden_angle = core::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
        (0..point_len)
            .map(|idx| {
                let y = 1.0 - 2.0 * (idx as f32 + 0.5) / point_len as f32;
                let ring_radius = (1.0 - y * y).sqrt();
                let angle = golden_angle * idx as f32;
                Vec3::new(ring_radius * angle.cos(), y + 1.0, ring_radius * angle.sin()) * radius
            })
            .collect()
    }

    #[test]
    fn a_dense_round_part_fits_a_hull_within_the_slack_of_every_point() {
        let radius_m = 0.06;
        let points = build_sphere_points(16 * MAX_HULL_VERTICES, radius_m);
        let hull = fit_hull(&points, f64::NEG_INFINITY, HullFit::Standard).unwrap();
        assert!(hull.len() <= MAX_HULL_VERTICES);
        let (vertices, triangles) = convex_hull(&hull);
        let faces = build_faces(&vertices, &triangles);
        let worst_gap_m = points.iter().map(|p| calc_plane_gap_m(&faces, p.as_dvec3())).fold(0.0, f64::max);
        assert!(worst_gap_m <= HULL_SLACK_M + 1e-6, "{worst_gap_m} m out");
        let farthest_m = hull.iter().map(|v| f64::from(v.distance(Vec3::Y * radius_m))).fold(0.0, f64::max);
        assert!(farthest_m < f64::from(radius_m) + 6.0 * HULL_SLACK_M, "a corner stands {farthest_m} m off center");
    }

    #[test]
    fn a_dense_round_part_trimmed_of_overhangs_fits_a_hull_whose_corners_stand_close_off_it() {
        let radius_m = 0.06;
        let points = build_sphere_points(16 * MAX_HULL_VERTICES, radius_m);
        let hull = fit_hull(&points, f64::NEG_INFINITY, HullFit::TrimOverhangs).unwrap();
        assert!(hull.len() <= MAX_HULL_VERTICES);
        let farthest_m = hull.iter().map(|v| f64::from(v.distance(Vec3::Y * radius_m))).fold(0.0, f64::max);
        // Each round caps its own step, and the step's direction shifts between rounds.
        assert!(farthest_m < f64::from(radius_m) + 2.0 * MAX_FLOAT_M, "a corner stands {farthest_m} m off center");
        let (vertices, triangles) = convex_hull(&hull);
        let faces = build_faces(&vertices, &triangles);
        let worst_gap_m = points.iter().map(|p| calc_plane_gap_m(&faces, p.as_dvec3())).fold(0.0, f64::max);
        assert!(worst_gap_m < 0.12 * f64::from(radius_m), "{worst_gap_m} m out");
    }

    #[test]
    fn a_part_resting_on_the_floor_fits_a_hull_reaching_no_lower() {
        let points = build_sphere_points(16 * MAX_HULL_VERTICES, 0.06);
        let floor_y_m = points.iter().map(|p| f64::from(p.y)).fold(f64::INFINITY, f64::min);
        for fit in [HullFit::Standard, HullFit::TrimOverhangs] {
            let hull = fit_hull(&points, floor_y_m, fit).unwrap();
            assert!(
                hull.iter().all(|v| f64::from(v.y) >= floor_y_m - 1e-7),
                "{fit:?}: a hull vertex lies below the floor"
            );
        }
    }

    /// An L-shaped slab 5 cm thick, its arms 10 cm wide and 20 cm long on their outer sides, with
    /// one part's points on a 1 cm grid through it. That part's hull bridges the 10 cm square notch
    /// between the arms, reaching 5 cm outside the slab at the notch's middle.
    #[test]
    fn a_part_bridging_a_notch_splits_into_halves_reaching_less_far_outside_the_mesh() {
        let outline = [[0.1, 0.1], [0.2, 0.1], [0.2, 0.0], [0.0, 0.0], [0.0, 0.2], [0.1, 0.2]];
        let mut mesh = Mesh::default();
        mesh.push_prism(&outline, 0.0, 0.05);
        let solid = TriMesh::with_flags(mesh.positions, mesh.triangles, TriMeshFlags::ORIENTED).unwrap();
        let grid = (0..=20u8).flat_map(|x| (0..=20u8).flat_map(move |z| [0.0f32, 0.05].map(move |y| (x, z, y))));
        let points: Vec<Vec3> = grid
            .filter(|&(x, z, _)| x <= 10 || z <= 10)
            .map(|(x, z, y)| Vec3::new(f32::from(x) * 0.01, y, f32::from(z) * 0.01))
            .collect();
        let (whole_m, overhang_at) = calc_overhang_m(&points, &solid, OVERHANG_FIT_STEPS);
        assert!((whole_m - 0.05).abs() < 1e-3, "the part's hull reaches {whole_m} m out");
        let halves: [Vec<Vec3>; 2] = split_part(&points, overhang_at, &solid).unwrap().into();
        let halves_m = halves.map(|half| calc_overhang_m(&half, &solid, OVERHANG_FIT_STEPS).0);
        // The best plane crosses an arm 15 cm along, so one half still spans the notch from that
        // arm's inner corner to its middle, a third of the notch's width out at worst.
        let farther_m = halves_m[0].max(halves_m[1]);
        assert!((farther_m - 0.1 / 3.0).abs() < 2e-3, "the halves' hulls reach {halves_m:?} m out");
    }

    #[test]
    fn a_dense_hull_stands_in_for_by_its_support_points() {
        let points = build_sphere_points(4000, 1.0);
        let support = reduce_to_support(convex_hull(&points).0);
        assert!(support.len() <= SUPPORT_DIRECTION_LEN);
        assert!(support.len() > COARSE_STAGE_FACTOR * MAX_HULL_VERTICES);
    }
}
