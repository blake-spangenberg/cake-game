//! Bakes a repaired `.glb` model (from `tools/repair-model.py`) into the `.cake` file the game
//! loads: the render mesh plus the convex hulls of its physics collider.
//!
//! Usage: `bake-cake <model.glb> <out.cake>`

use std::process::ExitCode;
use std::{env, fs};

use cake_core::cakes::{CakeModel, MAX_HULL_VERTICES};
use cake_core::mesh::Mesh;
use rapier3d::math::Vec3;
use rapier3d::parry::transformation::convex_hull;
use rapier3d::parry::transformation::vhacd::{VHACD, VHACDParameters};

/// Voxels along the longest side when decomposing: finer finds thinner concavities.
const VHACD_RESOLUTION: u32 = 64;
/// How concave a part may stay, as a share of the model's size, before it is split.
const VHACD_CONCAVITY: f32 = 0.01;
const MAX_HULLS: u32 = 16;

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
    let mesh = fetch_mesh(&bytes)?;
    let hulls = build_hulls(&mesh)?;
    let model = CakeModel { mesh, hulls };
    model.validate().map_err(|error| format!("not a playable cake: {error:?}"))?;
    fs::write(destination, model.encode()).map_err(|error| format!("cannot write {destination}: {error}"))?;
    let hull_sizes: Vec<usize> = model.hulls.iter().map(Vec::len).collect();
    Ok(format!("{} triangles, {} hulls with {hull_sizes:?} vertices", model.mesh.triangles.len(), model.hulls.len()))
}

/// The one triangle mesh of a repaired glb, in meters with the node transform ignored.
fn fetch_mesh(bytes: &[u8]) -> Result<Mesh, String> {
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
    let positions = reader.read_positions().ok_or("the mesh has no positions")?;
    let indices = reader.read_indices().ok_or("the mesh has no indices")?;
    let indices: Vec<u32> = indices.into_u32().collect();
    if !indices.len().is_multiple_of(3) {
        return Err("the index count is not a multiple of 3".into());
    }
    Ok(Mesh {
        positions: positions.map(Vec3::from_array).collect(),
        triangles: indices.chunks_exact(3).map(|triangle| [triangle[0], triangle[1], triangle[2]]).collect(),
    })
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
        let (best_idx, _) = (0..vertices.len())
            .map(|idx| {
                let mut rest = vertices.clone();
                rest.swap_remove(idx);
                (idx, calc_hull_volume(&rest).unwrap_or(0.0))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))?;
        let rest: Vec<Vec3> =
            vertices.iter().enumerate().filter(|(idx, _)| *idx != best_idx).map(|(_, &v)| v).collect();
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
        let mesh = fetch_mesh(L_SHAPE_GLB).unwrap();
        let hulls = build_hulls(&mesh).unwrap();
        let model = CakeModel { mesh, hulls };
        assert_eq!(model.validate(), Ok(()));
        assert!(model.hulls.len() >= 2, "an L is concave, so one hull would fill its corner");
        assert!(model.hulls.iter().all(|hull| hull.len() <= MAX_HULL_VERTICES));
        assert_eq!(CakeModel::decode(&model.encode()), Ok(model));
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
