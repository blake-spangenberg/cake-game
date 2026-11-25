//! Bakes a repaired `.glb` model (from `tools/repair-model.py`) into the `.cake` file the game
//! loads: the render mesh and its texture plus the convex hulls of its physics collider.
//!
//! Usage: `bake-cake <model.glb> <out.cake>`

use std::process::ExitCode;
use std::{env, fs};

use cake_core::cakes::{CakeModel, MAX_HULL_VERTICES, Skin};
use cake_core::mesh::Mesh;
use rapier3d::math::Vec3;
use rapier3d::parry::transformation::convex_hull;
use rapier3d::parry::transformation::vhacd::{VHACD, VHACDParameters};

/// Voxels along the longest side when decomposing: finer finds thinner concavities.
const VHACD_RESOLUTION: u32 = 64;
/// How concave a part may stay, as a share of the model's size, before it is split.
const VHACD_CONCAVITY: f32 = 0.01;
const MAX_HULLS: u32 = 16;
/// A hull with more vertices than this many times [`MAX_HULL_VERTICES`] sheds vertices in
/// batches, since pricing every vertex's removal one drop at a time costs time cubic in the count.
const COARSE_STAGE_FACTOR: usize = 4;
/// A batch of the coarse stage sheds one in this many of a hull's vertices.
const COARSE_BATCH_SHARE: usize = 4;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let [source, destination] = args.as_slice() else {
        eprintln!("usage: bake-cake <model.glb> <out.cake>");
        return ExitCode::from(2);
    };
    match bake(source, destination) {
        Ok(summary) => {
            println!("{destination}: {summary}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("{source}: {message}");
            ExitCode::FAILURE
        }
    }
}

fn bake(source: &str, destination: &str) -> Result<String, String> {
    let bytes = fs::read(source).map_err(|error| format!("cannot read the model: {error}"))?;
    let (mesh, skin) = fetch_mesh(&bytes)?;
    let hulls = build_hulls(&mesh)?;
    let model = CakeModel { mesh, skin, hulls };
    model.validate().map_err(|error| format!("not a playable cake: {error:?}"))?;
    fs::write(destination, model.encode()).map_err(|error| format!("cannot write {destination}: {error}"))?;
    let hull_sizes: Vec<usize> = model.hulls.iter().map(Vec::len).collect();
    Ok(format!("{} triangles, {} hulls with {hull_sizes:?} vertices", model.mesh.triangles.len(), model.hulls.len()))
}

/// The glb vertex attribute holding, for each vertex, the index of the closed solid's vertex it
/// copies. `tools/repair-model.py` writes it as `_SOLID_VERTEX`, and `gltf` strips the leading `_`
/// that marks a custom attribute.
const SOLID_VERTEX_ATTRIBUTE: &str = "SOLID_VERTEX";

/// The one triangle mesh of a repaired glb, rejoined into the closed solid its texture seams split,
/// and its texture, in meters with the node transform ignored.
fn fetch_mesh(bytes: &[u8]) -> Result<(Mesh, Skin), String> {
    let glb = gltf::Gltf::from_slice(bytes).map_err(|error| format!("not a glb: {error}"))?;
    let blob = glb.blob.as_deref();
    let mut meshes = glb.meshes();
    let (Some(gltf_mesh), None) = (meshes.next(), meshes.next()) else {
        return Err("expected exactly one mesh; run tools/repair-model.py first".into());
    };
    let mut primitives = gltf_mesh.primitives();
    let (Some(primitive), None) = (primitives.next(), primitives.next()) else {
        return Err("expected exactly one primitive; run tools/repair-model.py first".into());
    };
    let reader = primitive.reader(|_| blob);
    let split_positions: Vec<[f32; 3]> = reader.read_positions().ok_or("the mesh has no positions")?.collect();
    let indices: Vec<u32> = reader.read_indices().ok_or("the mesh has no indices")?.into_u32().collect();
    let split_triangles = to_triangles(&indices, split_positions.len())?;
    let uvs: Vec<[f32; 2]> = reader.read_tex_coords(0).ok_or("the mesh has no UVs")?.into_f32().collect();
    let solid_accessor = primitive
        .get(&gltf::Semantic::Extras(SOLID_VERTEX_ATTRIBUTE.into()))
        .ok_or("the mesh has no solid vertex ids; run tools/repair-model.py first")?;
    let solid_ids: Vec<u32> = gltf::accessor::Iter::<u32>::new(solid_accessor, |_| blob)
        .ok_or("the solid vertex ids are unreadable")?
        .collect();
    if uvs.len() != split_positions.len() || solid_ids.len() != split_positions.len() {
        return Err("the mesh does not have one UV and one solid vertex id per vertex".into());
    }
    let image = find_image(&glb)?;
    let positions = rejoin(&split_positions, &solid_ids)?;
    let triangles = split_triangles.iter().map(|corners| corners.map(|idx| solid_ids[idx])).collect();
    let corner_uvs = split_triangles.iter().map(|corners| corners.map(|idx| quantize_uv(uvs[idx]))).collect();
    Ok((Mesh { positions, triangles }, Skin::Textured { corner_uvs, image: image.to_vec() }))
}

/// The glb's one image, which must be stored in its binary chunk.
fn find_image(glb: &gltf::Gltf) -> Result<&[u8], String> {
    let mut images = glb.images();
    let (Some(image), None) = (images.next(), images.next()) else {
        return Err("expected exactly one image; run tools/repair-model.py first".into());
    };
    let gltf::image::Source::View { view, .. } = image.source() else {
        return Err("the image is not stored in the glb".into());
    };
    let range = view.offset()..view.offset() + view.length();
    glb.blob.as_deref().and_then(|blob| blob.get(range)).ok_or_else(|| "the image is missing".into())
}

/// The triangles `indices` lists, three corners each, every corner one of `vertex_len` vertices.
fn to_triangles(indices: &[u32], vertex_len: usize) -> Result<Vec<[usize; 3]>, String> {
    let (triangles, []) = indices.as_chunks::<3>() else {
        return Err("the index count is not a multiple of 3".into());
    };
    if indices.iter().any(|&idx| idx as usize >= vertex_len) {
        return Err(format!("a triangle names a vertex past the mesh's {vertex_len}"));
    }
    Ok(triangles.iter().map(|corners| corners.map(|corner| corner as usize)).collect())
}

/// The solid's vertex positions, from the split vertices that copy them: `solid_ids` must name
/// every solid vertex, and copies of one must agree.
fn rejoin(split_positions: &[[f32; 3]], solid_ids: &[u32]) -> Result<Vec<Vec3>, String> {
    let solid_len = solid_ids.iter().max().map_or(0, |&max| max as usize + 1);
    let mut positions: Vec<Option<[f32; 3]>> = vec![None; solid_len];
    for (&solid_id, &position) in solid_ids.iter().zip(split_positions) {
        let slot = &mut positions[solid_id as usize];
        if slot.is_some_and(|known| known != position) {
            return Err(format!("the copies of solid vertex {solid_id} disagree on its position"));
        }
        *slot = Some(position);
    }
    positions
        .into_iter()
        .enumerate()
        .map(|(solid_id, position)| {
            position.map(Vec3::from_array).ok_or(format!("no vertex copies solid vertex {solid_id}"))
        })
        .collect()
}

fn quantize_uv(uv: [f32; 2]) -> [u16; 2] {
    uv.map(|coord| (coord.clamp(0.0, 1.0) * f32::from(u16::MAX)).round() as u16)
}

/// The collider: the mesh's approximate convex decomposition, each hull cut down to few vertices.
fn build_hulls(mesh: &Mesh) -> Result<Vec<Vec<Vec3>>, String> {
    let parameters = VHACDParameters {
        resolution: VHACD_RESOLUTION,
        concavity: VHACD_CONCAVITY,
        max_convex_hulls: MAX_HULLS,
        ..Default::default()
    };
    let decomposition = VHACD::decompose(&parameters, &mesh.positions, &mesh.triangles, true);
    let hulls = decomposition.compute_exact_convex_hulls(&mesh.positions, &mesh.triangles);
    hulls
        .into_iter()
        .filter(|(points, _)| points.len() >= 4)
        .map(|(points, _)| simplify_hull(&points))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| "a collider hull is flat".to_string())
}

/// The hull of `points` with the vertices whose removal costs the least volume dropped until at
/// most [`MAX_HULL_VERTICES`] remain. `None` when the points are flat.
fn simplify_hull(points: &[Vec3]) -> Option<Vec<Vec3>> {
    let mut vertices = convex_hull(points).0;
    if calc_hull_volume(&vertices)? == 0.0 {
        return None;
    }
    while vertices.len() > MAX_HULL_VERTICES {
        let is_coarse = vertices.len() > COARSE_STAGE_FACTOR * MAX_HULL_VERTICES;
        let drop_len = if is_coarse { vertices.len() / COARSE_BATCH_SHARE } else { 1 };
        let mut costs: Vec<(usize, f32)> = (0..vertices.len())
            .map(|idx| {
                let mut rest = vertices.clone();
                rest.swap_remove(idx);
                (idx, calc_hull_volume(&rest).unwrap_or(0.0))
            })
            .collect();
        costs.sort_by(|a, b| b.1.total_cmp(&a.1));
        let dropped: Vec<usize> = costs.iter().take(drop_len).map(|&(idx, _)| idx).collect();
        let rest: Vec<Vec3> =
            vertices.iter().enumerate().filter(|(idx, _)| !dropped.contains(idx)).map(|(_, &v)| v).collect();
        vertices = convex_hull(&rest).0;
    }
    Some(vertices)
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

    /// Two overlapping boxes merged into an L, 0.2 m long, made by `tools/repair-model.py`.
    const L_SHAPE_GLB: &[u8] = include_bytes!("../tests/l-shape.glb");

    #[test]
    fn l_shaped_model_bakes_into_a_playable_cake_with_a_hull_per_arm() {
        let (mesh, skin) = fetch_mesh(L_SHAPE_GLB).unwrap();
        let hulls = build_hulls(&mesh).unwrap();
        let model = CakeModel { mesh, skin, hulls };
        assert_eq!(model.validate(), Ok(()));
        assert!(model.hulls.len() >= 2, "an L is concave, so one hull would fill its corner");
        assert!(model.hulls.iter().all(|hull| hull.len() <= MAX_HULL_VERTICES));
        assert_eq!(CakeModel::decode(&model.encode()), Ok(model));
    }

    #[test]
    fn dense_hull_is_cut_to_the_vertex_limit_keeping_most_of_its_volume() {
        let point_len = 8 * MAX_HULL_VERTICES;
        let golden_angle = core::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
        let sphere: Vec<Vec3> = (0..point_len)
            .map(|idx| {
                let y = 1.0 - 2.0 * (idx as f32 + 0.5) / point_len as f32;
                let ring_radius = (1.0 - y * y).sqrt();
                let angle = golden_angle * idx as f32;
                Vec3::new(ring_radius * angle.cos(), y, ring_radius * angle.sin())
            })
            .collect();
        let simplified = simplify_hull(&sphere).unwrap();
        assert!(simplified.len() <= MAX_HULL_VERTICES);
        assert!(calc_hull_volume(&simplified).unwrap() > 0.8 * calc_hull_volume(&sphere).unwrap());
    }

    #[test]
    fn seam_copies_rejoin_into_the_solid_vertex_they_name() {
        let split_positions = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 0.0]];
        assert_eq!(rejoin(&split_positions, &[0, 1, 0]), Ok(vec![Vec3::ZERO, Vec3::X]));
    }

    #[test]
    fn two_sheets_at_one_place_stay_apart_when_they_name_different_solid_vertices() {
        let split_positions = [[0.0, 0.0, 0.0], [0.0, 0.0, 0.0]];
        assert_eq!(rejoin(&split_positions, &[0, 1]), Ok(vec![Vec3::ZERO, Vec3::ZERO]));
    }

    #[test]
    fn seam_copies_that_disagree_on_their_position_are_refused() {
        let error = rejoin(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]], &[0, 0]).unwrap_err();
        assert!(error.contains("disagree"), "{error}");
    }

    #[test]
    fn a_solid_vertex_no_copy_names_is_refused() {
        let error = rejoin(&[[0.0, 0.0, 0.0]], &[1]).unwrap_err();
        assert!(error.contains("solid vertex 0"), "{error}");
    }

    #[test]
    fn indices_group_into_triangles_of_existing_vertices() {
        assert_eq!(to_triangles(&[0, 1, 2, 2, 1, 0], 3), Ok(vec![[0, 1, 2], [2, 1, 0]]));
        assert!(to_triangles(&[0, 1], 3).is_err());
        assert!(to_triangles(&[0, 1, 3], 3).is_err(), "vertex 3 is past the end");
    }

    #[test]
    fn model_with_two_meshes_is_refused() {
        let error = fetch_mesh(&build_two_mesh_glb()).unwrap_err();
        assert!(error.contains("exactly one mesh"), "{error}");
    }

    fn build_two_mesh_glb() -> Vec<u8> {
        let json = br#"{"asset":{"version":"2.0"},"meshes":[{"primitives":[]},{"primitives":[]}]}"#;
        let padded_len = json.len().next_multiple_of(4);
        let mut bytes = b"glTF".to_vec();
        bytes.extend(2u32.to_le_bytes());
        bytes.extend((12 + 8 + padded_len as u32).to_le_bytes());
        bytes.extend((padded_len as u32).to_le_bytes());
        bytes.extend(b"JSON");
        bytes.extend(json);
        bytes.resize(12 + 8 + padded_len, b' ');
        bytes
    }
}
