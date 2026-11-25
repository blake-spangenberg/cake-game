//! Bakes a repaired `.glb` model (from `tools/repair-model.py`) into the `.cake` file the game
//! loads: the render mesh, its texture, and its shading normals, plus the convex hulls of its
//! physics collider.
//! `--trim-overhangs` fits the hulls with [`HullFit::TrimOverhangs`].

mod collider;

use std::process::ExitCode;
use std::{env, fs};

use cake_core::cakes::{CREASE_RAD, CakeModel, Skin};
use cake_core::mesh::{CornerNormals, Mesh};
use collider::{HullFit, build_hulls};
use rapier3d::math::Vec3;

const USAGE: &str = "usage: bake-cake <model.glb> <out.cake> [--trim-overhangs]";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some((source, destination, fit)) = parse_args(&args) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    match bake(source, destination, fit) {
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

/// The model to read, the cake file to write, and how to fit its hulls, or `None` for arguments
/// [`USAGE`] does not allow.
fn parse_args(args: &[String]) -> Option<(&str, &str, HullFit)> {
    match args {
        [source, destination] => Some((source, destination, HullFit::Standard)),
        [source, destination, option] if option == "--trim-overhangs" => {
            Some((source, destination, HullFit::TrimOverhangs))
        }
        _ => None,
    }
}

fn bake(source: &str, destination: &str, fit: HullFit) -> Result<String, String> {
    let bytes = fs::read(source).map_err(|error| format!("cannot read the model: {error}"))?;
    let (mesh, skin) = fetch_mesh(&bytes)?;
    // Before the collapse, which removes no surface but would give V-HACD other hulls.
    let hulls = build_hulls(&mesh, fit)?;
    let (mesh, skin, collapsed_len) = collapse_flat_triangles(mesh, skin);
    let corner_normals = calc_corner_normals(&mesh)?;
    let model = CakeModel { mesh, skin, corner_normals, hulls };
    model.validate().map_err(|error| format!("not a playable cake: {error:?}"))?;
    fs::write(destination, model.encode()).map_err(|error| format!("cannot write {destination}: {error}"))?;
    let hull_sizes: Vec<usize> = model.hulls.iter().map(Vec::len).collect();
    let triangle_len = model.mesh.triangles.len();
    Ok(format!(
        "{triangle_len} triangles ({collapsed_len} collapsed away), {} hulls with {hull_sizes:?} vertices",
        model.hulls.len()
    ))
}

/// The mesh's shading normals, as the client once computed them when it loaded a cake.
fn calc_corner_normals(mesh: &Mesh) -> Result<CornerNormals, String> {
    CornerNormals::calc(mesh, CREASE_RAD).map_err(|error| format!("no corner normals: {error:?}"))
}

/// The mesh without its triangles of no area, which a corner normal cannot shade, and the skin's
/// UVs for the triangles left, plus how many went. Each such triangle's shortest edge collapses,
/// moving its higher-numbered vertex onto the other, which drops every triangle on that edge,
/// until none is left; decimation leaves such edges nanometers long. The moved vertex stays in
/// the positions unused, so the numbering scripts grab vertices by holds.
fn collapse_flat_triangles(mut mesh: Mesh, skin: Skin) -> (Mesh, Skin, usize) {
    let source_len = mesh.triangles.len();
    let mut source_idxs: Vec<usize> = (0..source_len).collect();
    let is_flat = |mesh: &Mesh, triangle: [u32; 3]| mesh.calc_face_normal(triangle) == Vec3::ZERO;
    while let Some(flat_idx) = mesh.triangles.iter().position(|&triangle| is_flat(&mesh, triangle)) {
        let [kept, moved] = find_shortest_edge(&mesh.positions, mesh.triangles[flat_idx]);
        for vertex in mesh.triangles.iter_mut().flatten().filter(|vertex| **vertex == moved) {
            *vertex = kept;
        }
        let is_kept: Vec<bool> = mesh.triangles.iter().map(|&[a, b, c]| a != b && b != c && c != a).collect();
        source_idxs = source_idxs.into_iter().zip(&is_kept).filter(|&(_, &keep)| keep).map(|(idx, _)| idx).collect();
        mesh.triangles = mesh.triangles.into_iter().zip(&is_kept).filter(|&(_, &keep)| keep).map(|(t, _)| t).collect();
    }
    let collapsed_len = source_len - source_idxs.len();
    let skin = match skin {
        Skin::Plain => Skin::Plain,
        Skin::Textured { corner_uvs, image } => {
            Skin::Textured { corner_uvs: source_idxs.iter().map(|&idx| corner_uvs[idx]).collect(), image }
        }
    };
    (mesh, skin, collapsed_len)
}

/// The ends of `triangle`'s shortest edge, the lower-numbered first.
fn find_shortest_edge(positions: &[Vec3], triangle: [u32; 3]) -> [u32; 2] {
    let edges = [0, 1, 2].map(|k| [triangle[k], triangle[(k + 1) % 3]]);
    let length = |[a, b]: [u32; 2]| positions[a as usize].distance(positions[b as usize]);
    let shortest = edges.into_iter().min_by(|&e, &f| length(e).total_cmp(&length(f))).expect("three edges");
    [shortest[0].min(shortest[1]), shortest[0].max(shortest[1])]
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
#[expect(clippy::float_cmp, reason = "copies of one solid vertex must agree to the bit")]
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
            position.map(Vec3::from_array).ok_or_else(|| format!("no vertex copies solid vertex {solid_id}"))
        })
        .collect()
}

#[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "rounds a coordinate in [0, u16::MAX]")]
fn quantize_uv(uv: [f32; 2]) -> [u16; 2] {
    uv.map(|coord| (coord.clamp(0.0, 1.0) * f32::from(u16::MAX)).round() as u16)
}

#[cfg(test)]
#[expect(clippy::cast_possible_truncation, reason = "the fixtures count a handful of elements")]
mod tests {
    use cake_core::cakes::MAX_HULL_VERTICES;
    use rapier3d::glamx::DVec3;

    use super::*;

    /// Two overlapping boxes merged into an L, 0.2 m long, made by `tools/repair-model.py`.
    const L_SHAPE_GLB: &[u8] = include_bytes!("../tests/l-shape.glb");

    #[test]
    fn l_shaped_model_bakes_into_a_playable_cake_with_a_hull_per_arm() {
        for fit in [HullFit::Standard, HullFit::TrimOverhangs] {
            let (mesh, skin) = fetch_mesh(L_SHAPE_GLB).unwrap();
            let hulls = build_hulls(&mesh, fit).unwrap();
            let corner_normals = calc_corner_normals(&mesh).unwrap();
            let model = CakeModel { mesh, skin, corner_normals, hulls };
            assert_eq!(model.validate(), Ok(()));
            assert!(model.hulls.len() >= 2, "{fit:?}: an L is concave, so one hull would fill its corner");
            assert!(model.hulls.iter().all(|hull| hull.len() <= MAX_HULL_VERTICES));
            assert_eq!(CakeModel::decode(&model.encode()), Ok(model));
        }
    }

    #[test]
    fn every_vertex_of_the_l_shaped_model_lies_within_the_slack_of_a_hull() {
        let (mesh, _) = fetch_mesh(L_SHAPE_GLB).unwrap();
        let hulls = build_hulls(&mesh, HullFit::Standard).unwrap();
        let planes: Vec<Vec<(DVec3, f64)>> = hulls.iter().map(|hull| build_supporting_planes(hull)).collect();
        let tolerance_m = collider::HULL_SLACK_M + 1e-6;
        for position in &mesh.positions {
            let p = position.as_dvec3();
            let is_covered =
                planes.iter().any(|hull| hull.iter().all(|&(normal, offset)| normal.dot(p) <= offset + tolerance_m));
            assert!(is_covered, "{position:?} lies outside every hull");
        }
    }

    /// Every plane through three of `hull`'s vertices with all the others on one side, facing out.
    fn build_supporting_planes(hull: &[Vec3]) -> Vec<(DVec3, f64)> {
        let points: Vec<DVec3> = hull.iter().map(|v| v.as_dvec3()).collect();
        let mut planes = Vec::new();
        for (idx_a, &a) in points.iter().enumerate() {
            for (idx_b, &b) in points.iter().enumerate().skip(idx_a + 1) {
                for &c in points.iter().skip(idx_b + 1) {
                    let Some(normal) = (b - a).cross(c - a).try_normalize() else { continue };
                    let sides: Vec<f64> = points.iter().map(|p| normal.dot(*p - a)).collect();
                    if sides.iter().all(|&side| side <= 1e-9) {
                        planes.push((normal, normal.dot(a)));
                    } else if sides.iter().all(|&side| side >= -1e-9) {
                        planes.push((-normal, -normal.dot(a)));
                    }
                }
            }
        }
        planes
    }

    #[test]
    fn trim_overhangs_after_the_two_paths_selects_the_trimming_fit() {
        let args = ["model.glb", "out.cake", "--trim-overhangs"].map(String::from);
        assert_eq!(parse_args(&args), Some(("model.glb", "out.cake", HullFit::TrimOverhangs)));
        assert_eq!(parse_args(&args[..2]), Some(("model.glb", "out.cake", HullFit::Standard)));
    }

    #[test]
    fn an_unknown_option_or_a_missing_path_is_refused() {
        assert_eq!(parse_args(&["model.glb", "out.cake", "--trim"].map(String::from)), None);
        assert_eq!(parse_args(&["--trim-overhangs", "model.glb", "out.cake"].map(String::from)), None);
        assert_eq!(parse_args(&["model.glb"].map(String::from)), None);
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
    fn a_vertex_split_by_flat_triangles_collapses_back_with_the_uvs_following() {
        // A closed tetrahedron whose vertex 3 has a copy, vertex 4, at the same place, joined to it
        // by two triangles of no area.
        let positions = vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z, Vec3::Z];
        let triangles = vec![[0, 2, 1], [0, 1, 3], [1, 2, 4], [1, 4, 3], [4, 2, 0], [3, 4, 0]];
        let mesh = Mesh { positions, triangles };
        assert_eq!(mesh.validate_closed(), Ok(()));
        let corner_uvs = (0..mesh.triangles.len() as u16).map(|idx| [[idx, 0]; 3]).collect();
        let skin = Skin::Textured { corner_uvs, image: b"RIFF....WEBP".to_vec() };
        let (mesh, skin, collapsed_len) = collapse_flat_triangles(mesh, skin);
        assert_eq!(mesh.triangles, vec![[0, 2, 1], [0, 1, 3], [1, 2, 3], [3, 2, 0]]);
        assert_eq!(mesh.validate_closed(), Ok(()));
        assert_eq!(collapsed_len, 2);
        let Skin::Textured { corner_uvs, .. } = skin else { panic!("the skin stays textured") };
        assert_eq!(corner_uvs.iter().map(|uvs| uvs[0][0]).collect::<Vec<_>>(), [0, 1, 2, 4]);
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
