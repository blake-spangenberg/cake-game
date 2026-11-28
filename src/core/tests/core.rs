#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::similar_names,
    clippy::unwrap_used,
    reason = "test fixtures cast small literals, and x and z name the axes"
)]

use cake_core::cakes::{CREASE_RAD, CakeError, CakeModel, MAX_HULL_VERTICES, Skin, build_slice_missing_cake};
use cake_core::cut::{Plane, Split, calc_split};
use cake_core::input::{HandInput, Input, InputError, KnifeInput, POSITION_UNIT_M, quantize_meters, quantize_radians};
use cake_core::knife::{Knife, MAX_RIDE_M, MAX_TILT_RAD, START_TILT_RAD, Solid, calc_edge_rise_m};
use cake_core::mesh::{CornerNormals, CornerNormalsError, Mesh, MeshError};
use cake_core::replay::{build_placement_inputs, build_scripted_inputs, calc_spawned_top_grab_at, replay};
use cake_core::sim::{Phase, ROUND_TICKS, TICKS_PER_S, World};
use cake_core::table::{
    CAKE_SPAWN, MAX_CAKE_SIDE_M, PIVOT, STAND_PARTS, STAND_PLATE_THICKNESS_M, STAND_RADIUS_M, STAND_TOP_Y_M,
    TABLE_HALF_X_M, TABLE_HALF_Z_M,
};
use rapier3d::glamx::DVec3;
use rapier3d::math::{Rotation, Vec3};

/// A closed axis-aligned box from `min` to `max`, wound outward.
fn build_box(min: [f32; 3], max: [f32; 3]) -> Mesh {
    let corner = |i: usize| {
        Vec3::new(
            if i & 1 == 0 { min[0] } else { max[0] },
            if i & 2 == 0 { min[1] } else { max[1] },
            if i & 4 == 0 { min[2] } else { max[2] },
        )
    };
    let quads = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
    Mesh {
        positions: (0..8).map(corner).collect(),
        triangles: quads.iter().flat_map(|&[a, b, c, d]| [[a, b, c], [a, c, d]]).collect(),
    }
}

fn to_dvec3s(mesh: &Mesh) -> Vec<DVec3> {
    mesh.positions.iter().map(|p| p.as_dvec3()).collect()
}

#[test]
fn box_corner_normals_keep_its_square_edges() {
    let normals = build_box([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]).calc_corner_normals(std::f32::consts::FRAC_PI_3);
    // The first triangle lies on the box's z = 0 face.
    assert_eq!(normals[..3], [Vec3::NEG_Z; 3]);
}

#[test]
fn corner_normals_blend_faces_meeting_gently_only_where_they_meet() {
    // Two faces folded 23 degrees along the x axis, each rising by 0.2 over its width.
    let mesh = Mesh {
        positions: vec![Vec3::ZERO, Vec3::X, Vec3::new(0.5, 0.2, 1.0), Vec3::new(0.5, 0.2, -1.0)],
        triangles: vec![[1, 0, 2], [0, 1, 3]],
    };
    let normals = mesh.calc_corner_normals(std::f32::consts::FRAC_PI_3);
    let near = |a: Vec3, b: Vec3| a.distance(b) < 1e-6;
    assert!(near(normals[0], Vec3::Y), "the shared edge's corners take the average: {:?}", normals[0]);
    assert!(
        near(normals[2], Vec3::new(0.0, 1.0, -0.2).normalize()),
        "the far corner keeps its own face's: {:?}",
        normals[2]
    );
}

#[test]
fn placeholder_cake_is_closed() {
    assert_eq!(build_slice_missing_cake().mesh.validate_closed(), Ok(()));
}

/// A box resting on y = 0 and centered on its own origin, with a matching hull. A world spawns it
/// at `CAKE_SPAWN`, off the stand.
fn build_box_cake(half_x_m: f32, height_m: f32, half_z_m: f32) -> CakeModel {
    let mesh = build_box([-half_x_m, 0.0, -half_z_m], [half_x_m, height_m, half_z_m]);
    let hull = mesh.positions.clone();
    build_plain_cake(mesh, vec![hull])
}

fn build_plain_cake(mesh: Mesh, hulls: Vec<Vec<Vec3>>) -> CakeModel {
    let corner_normals = CornerNormals::calc(&mesh, CREASE_RAD).unwrap();
    CakeModel { mesh, skin: Skin::Plain, corner_normals, hulls }
}

#[test]
fn corner_normals_expand_to_the_computed_ones_but_for_rounding() {
    for mesh in [build_box([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]), build_slice_missing_cake().mesh] {
        let computed = mesh.calc_corner_normals(CREASE_RAD);
        let corner_normals = CornerNormals::calc(&mesh, CREASE_RAD).unwrap();
        let expanded = corner_normals.to_corners(&mesh);
        assert_eq!(expanded.len(), computed.len());
        for (a, b) in expanded.iter().zip(&computed) {
            assert!(a.as_dvec3().angle_between(b.as_dvec3()) < 1e-4, "{a:?} strays from {b:?}");
        }
        assert!(corner_normals.normals.len() < computed.len(), "each distinct normal at a vertex is kept once");
    }
}

#[test]
fn a_triangle_with_no_area_has_no_corner_normals() {
    let mut mesh = build_box([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
    mesh.positions.push(mesh.positions[0]);
    mesh.triangles.push([0, 8, 1]);
    assert_eq!(CornerNormals::calc(&mesh, CREASE_RAD), Err(CornerNormalsError::Degenerate { triangle_idx: 12 }));
}

#[test]
fn a_box_keeps_three_normals_at_each_corner() {
    let mesh = build_box([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
    assert_eq!(CornerNormals::calc(&mesh, CREASE_RAD).unwrap().normals.len(), 3 * mesh.positions.len());
}

#[test]
fn corner_normals_that_do_not_fit_the_mesh_are_refused() {
    let mut model = build_box_cake(0.05, 0.05, 0.05);
    model.corner_normals.normals.pop();
    assert_eq!(CakeModel::decode(&model.encode()), Err(CakeError::BadNormals));
    let mut model = build_box_cake(0.05, 0.05, 0.05);
    // A box corner has three normals, so a fourth pick names one past its own.
    model.corner_normals.picks[0][0] = 3;
    assert_eq!(model.validate(), Err(CakeError::BadNormals));
}

#[test]
fn placeholder_cake_validates() {
    assert_eq!(build_slice_missing_cake().validate(), Ok(()));
}

#[test]
fn cake_file_round_trips() {
    let model = build_slice_missing_cake();
    assert_eq!(CakeModel::decode(&model.encode()), Ok(model));
}

#[test]
fn cake_file_rejects_damaged_bytes() {
    let bytes = build_slice_missing_cake().encode();
    assert_eq!(CakeModel::decode(&bytes[..bytes.len() - 1]), Err(CakeError::BadFile));
    assert_eq!(CakeModel::decode(&[bytes.as_slice(), &[0]].concat()), Err(CakeError::BadFile));
    assert_eq!(CakeModel::decode(&bytes[1..]), Err(CakeError::BadFile));
    let mut wrong_version = bytes;
    wrong_version[4] ^= 1;
    assert_eq!(CakeModel::decode(&wrong_version), Err(CakeError::BadFile));
}

#[test]
fn cake_file_keeps_its_texture_and_needs_a_uv_triple_per_triangle() {
    let mut model = build_box_cake(0.05, 0.05, 0.05);
    let mut corner_uvs: Vec<[[u16; 2]; 3]> =
        (0..model.mesh.triangles.len() as u16).map(|idx| [[idx, 1], [2, u16::MAX], [3, idx]]).collect();
    model.skin = Skin::Textured { corner_uvs: corner_uvs.clone(), image: b"RIFF....WEBP".to_vec() };
    assert_eq!(CakeModel::decode(&model.encode()), Ok(model.clone()));
    corner_uvs.pop();
    model.skin = Skin::Textured { corner_uvs, image: b"RIFF....WEBP".to_vec() };
    assert_eq!(model.validate(), Err(CakeError::BadSkin));
}

#[test]
fn cake_file_rejects_an_unplayable_model() {
    let mut model = build_box_cake(0.05, 0.05, 0.05);
    model.mesh.triangles.pop();
    model.corner_normals.picks.pop();
    assert!(matches!(CakeModel::decode(&model.encode()), Err(CakeError::Mesh(MeshError::OpenEdge { .. }))));
}

#[test]
fn validate_accepts_a_box_and_names_what_is_wrong_otherwise() {
    assert_eq!(build_box_cake(0.05, 0.05, 0.05).validate(), Ok(()));

    let mut inside_out = build_box_cake(0.05, 0.05, 0.05);
    inside_out.mesh.triangles.iter_mut().for_each(|triangle| triangle.swap(1, 2));
    assert_eq!(inside_out.validate(), Err(CakeError::NotOutward));

    let too_long_m = MAX_CAKE_SIDE_M as f32 + 0.01;
    assert_eq!(build_box_cake(0.05, too_long_m, 0.05).validate(), Err(CakeError::TooLong));
    let too_long_corner_to_corner = build_box_cake(0.12, 0.24, 0.12);
    assert_eq!(too_long_corner_to_corner.validate(), Err(CakeError::TooLong));

    let mut floating = build_box_cake(0.05, 0.05, 0.05);
    floating.mesh.positions.iter_mut().for_each(|p| p.y += 0.01);
    assert_eq!(floating.validate(), Err(CakeError::OffPlate));
    let mut off_center = build_box_cake(0.05, 0.05, 0.05);
    off_center.mesh.positions.iter_mut().for_each(|p| p.x += STAND_RADIUS_M as f32);
    assert_eq!(off_center.validate(), Err(CakeError::OffPlate));

    let mut hull_free = build_box_cake(0.05, 0.05, 0.05);
    hull_free.hulls.clear();
    assert_eq!(hull_free.validate(), Err(CakeError::BadHull { hull_idx: 0 }));
    let mut flat_hull = build_box_cake(0.05, 0.05, 0.05);
    flat_hull.hulls[0] = vec![Vec3::ZERO; 4];
    assert_eq!(flat_hull.validate(), Err(CakeError::BadHull { hull_idx: 0 }));
    let mut fine_hull = build_box_cake(0.05, 0.05, 0.05);
    fine_hull.hulls[0] = (0..=(MAX_HULL_VERTICES as u32))
        .map(|i| {
            let angle = i as f32 * 0.3;
            Vec3::new(0.05 * angle.cos(), 0.05 + 0.01 * (i % 2) as f32, 0.05 * angle.sin())
        })
        .collect();
    assert_eq!(fine_hull.validate(), Err(CakeError::BadHull { hull_idx: 0 }));
}

#[test]
fn mesh_with_a_stray_index_is_rejected() {
    let mut mesh = build_box([0.0; 3], [1.0; 3]);
    mesh.triangles[3][1] = 99;
    assert_eq!(mesh.validate_closed(), Err(MeshError::IndexOutOfRange { triangle_idx: 3 }));
}

#[test]
fn mesh_with_three_triangles_on_an_edge_is_non_manifold() {
    let mesh = Mesh {
        positions: vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z, Vec3::NEG_Z],
        triangles: vec![[0, 1, 2], [1, 0, 3], [1, 0, 4]],
    };
    assert_eq!(mesh.validate_closed(), Err(MeshError::NonManifoldEdge { from: 0, to: 1 }));
}

#[test]
fn mesh_missing_a_face_is_open() {
    let mut mesh = build_box([0.0; 3], [1.0; 3]);
    mesh.triangles.pop();
    assert!(matches!(mesh.validate_closed(), Err(MeshError::OpenEdge { .. })));
}

#[test]
fn mesh_with_a_flipped_triangle_is_inconsistently_wound() {
    let mut mesh = build_box([0.0; 3], [1.0; 3]);
    mesh.triangles[0].swap(1, 2);
    assert!(matches!(mesh.validate_closed(), Err(MeshError::InconsistentWinding { .. })));
}

#[test]
fn plane_a_quarter_into_a_unit_cube_splits_it_three_to_one() {
    let cube = build_box([0.0; 3], [1.0; 3]);
    let plane = Plane { point: DVec3::new(0.25, 0.0, 0.0), normal: DVec3::X };
    let split = calc_split(&to_dvec3s(&cube), &cube.triangles, &plane);
    assert!((split.above_m3 - 0.75).abs() < 1e-12, "{split:?}");
    assert!((split.below_m3 - 0.25).abs() < 1e-12, "{split:?}");
}

#[test]
fn diagonal_plane_through_a_cube_center_halves_it() {
    let cube = build_box([0.0; 3], [1.0; 3]);
    let plane = Plane { point: DVec3::splat(0.5), normal: DVec3::new(1.0, 1.0, 0.0).normalize() };
    let split = calc_split(&to_dvec3s(&cube), &cube.triangles, &plane);
    assert!((split.above_m3 - 0.5).abs() < 1e-12, "{split:?}");
    assert!((split.below_m3 - 0.5).abs() < 1e-12, "{split:?}");
}

#[test]
fn plane_missing_the_mesh_leaves_it_whole_on_one_side() {
    let cube = build_box([0.0; 3], [1.0; 3]);
    let plane = Plane { point: DVec3::new(2.0, 0.0, 0.0), normal: DVec3::X };
    let split = calc_split(&to_dvec3s(&cube), &cube.triangles, &plane);
    assert_eq!(split.above_m3, 0.0);
    assert!((split.below_m3 - 1.0).abs() < 1e-12, "{split:?}");
}

/// An upright knife with its edge on the pivot.
const UPRIGHT_KNIFE: Knife = Knife { tilt_rad: 0.0, rest_m: 0.0, rest_lead_m: 0.0, lower_m: 0.0 };

fn calc_limit_m(knife: &Knife, mesh: &Mesh) -> f64 {
    let positions = to_dvec3s(mesh);
    knife.calc_lower_limit_m(&[Solid::new(&positions, &mesh.triangles)])
}

#[test]
fn upright_knife_over_an_empty_table_lowers_to_the_table() {
    assert!((UPRIGHT_KNIFE.calc_lower_limit_m(&[]) - PIVOT.y).abs() < 1e-12);
}

#[test]
fn upright_knife_stops_on_a_box_cake_under_its_edge_middle() {
    let cake = build_box([-0.05, 0.0, -0.01], [0.05, 0.1, 0.01]);
    let limit_m = calc_limit_m(&UPRIGHT_KNIFE, &cake);
    assert!((limit_m - (PIVOT.y - 0.1)).abs() < 1e-7, "{limit_m}");
}

#[test]
fn curved_edge_reaches_a_box_off_its_middle_later() {
    let cake = build_box([-0.05, 0.0, 0.1], [0.05, 0.1, 0.15]);
    let limit_m = calc_limit_m(&UPRIGHT_KNIFE, &cake);
    let expected_m = PIVOT.y - 0.1 + calc_edge_rise_m(0.1);
    assert!((limit_m - expected_m).abs() < 1e-7, "{limit_m} vs {expected_m}");
}

#[test]
fn curved_edge_meets_a_sloped_top_between_its_corners() {
    let cake = Mesh {
        positions: vec![
            Vec3::new(-0.01, 0.0, -0.1),
            Vec3::new(-0.01, 0.0, 0.1),
            Vec3::new(-0.01, 0.15, 0.1),
            Vec3::new(-0.01, 0.05, -0.1),
            Vec3::new(0.01, 0.0, -0.1),
            Vec3::new(0.01, 0.0, 0.1),
            Vec3::new(0.01, 0.15, 0.1),
            Vec3::new(0.01, 0.05, -0.1),
        ],
        triangles: Vec::new(),
    };
    let quads = [[0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4], [3, 7, 6, 2], [0, 4, 7, 3], [1, 2, 6, 5]];
    let cake = Mesh { triangles: quads.iter().flat_map(|&[a, b, c, d]| [[a, b, c], [a, c, d]]).collect(), ..cake };
    assert_eq!(cake.validate_closed(), Ok(()));
    let limit_m = calc_limit_m(&UPRIGHT_KNIFE, &cake);
    let brute_m = (0..=2000)
        .map(|i| {
            let u_m = -0.1 + 0.2 * f64::from(i) / 2000.0;
            let top_y_m = 0.1 + 0.5 * u_m;
            PIVOT.y - top_y_m + calc_edge_rise_m(u_m)
        })
        .fold(f64::INFINITY, f64::min);
    assert!((limit_m - brute_m).abs() < 1e-6, "{limit_m} vs {brute_m}");
}

#[test]
fn knife_ignores_a_cake_beyond_its_blade_end() {
    let cake = build_box([-0.05, 0.0, 0.3], [0.05, 0.15, 0.4]);
    let limit_m = calc_limit_m(&UPRIGHT_KNIFE, &cake);
    assert!((limit_m - PIVOT.y).abs() < 1e-12, "{limit_m}");
}

#[test]
fn lowered_knife_tilted_into_a_cake_wall_rides_up_it_at_most_the_ride_allowance_then_stops() {
    let cake = build_box([-0.15, 0.0, -0.1], [-0.05, 0.2, 0.1]);
    let positions = to_dvec3s(&cake);
    let solids = [Solid::new(&positions, &cake.triangles)];
    let lower = quantize_meters(0.39);
    let mut knife = UPRIGHT_KNIFE;
    knife.step(&KnifeInput { tilt: 0, lower }, &solids);
    assert!((knife.lower_m - 0.39).abs() < 1e-3, "{knife:?}");
    for _ in 0..10 {
        knife.step(&KnifeInput { tilt: quantize_radians(0.5), lower }, &solids);
    }
    assert!(knife.lower_m > 0.39 - MAX_RIDE_M - 1e-3, "{knife:?}");
    assert!((0.1..0.2).contains(&knife.tilt_rad), "{knife:?}");
}

/// Tilts a knife pressed 5 mm into a flat cake top at y = 0.2 toward a ridge of `ridge_height_m`
/// across its path, and returns how far it tilted.
fn tilt_across_a_ridge(ridge_height_m: f32) -> f64 {
    let slab = build_box([-0.15, 0.0, -0.1], [0.15, 0.2, 0.1]);
    let ridge = build_box([-0.07, 0.2, -0.1], [-0.06, 0.2 + ridge_height_m, 0.1]);
    let (slab_positions, ridge_positions) = (to_dvec3s(&slab), to_dvec3s(&ridge));
    let solids = [Solid::new(&slab_positions, &slab.triangles), Solid::new(&ridge_positions, &ridge.triangles)];
    let lower = quantize_meters(PIVOT.y - 0.2 + 0.005);
    let mut knife = UPRIGHT_KNIFE;
    knife.step(&KnifeInput { tilt: 0, lower }, &solids);
    for _ in 0..60 {
        knife.step(&KnifeInput { tilt: quantize_radians(0.6), lower }, &solids);
    }
    knife.tilt_rad
}

#[test]
fn tilting_knife_rides_over_a_low_bump_but_stops_at_a_high_step() {
    assert!(tilt_across_a_ridge(0.005) > 0.59, "{}", tilt_across_a_ridge(0.005));
    let blocked_rad = tilt_across_a_ridge(0.03);
    assert!((0.1..0.4).contains(&blocked_rad), "{blocked_rad}");
}

#[test]
fn knife_rotation_matches_its_frame() {
    let knife = Knife { tilt_rad: 0.3, ..UPRIGHT_KNIFE };
    let frame = knife.calc_frame();
    let rotation = knife.calc_rotation().as_dquat();
    assert!((rotation * DVec3::X).distance(frame.edge) < 1e-6);
    assert!((rotation * DVec3::Y).distance(-frame.down) < 1e-6);
}

#[test]
fn score_is_100_for_equal_halves_and_0_for_a_miss() {
    assert_eq!(Split { above_m3: 1.0, below_m3: 1.0 }.calc_score(), 100.0);
    assert_eq!(Split { above_m3: 0.0, below_m3: 1.0 }.calc_score(), 0.0);
    assert!((Split { above_m3: 0.6, below_m3: 0.4 }.calc_score() - 80.0).abs() < 1e-9);
}

#[test]
fn input_survives_encoding() {
    let input = Input {
        hand: Some(HandInput { grab_at: [1, -2, 3], target_xz: [-4, 5] }),
        knife: KnifeInput { tilt: -600, lower: 7_000 },
        stand_angle: -8,
    };
    assert_eq!(Input::decode(&input.encode()), Ok(input));
}

#[test]
fn input_of_the_wrong_length_is_rejected() {
    assert_eq!(Input::decode(&[0; 8]), Err(InputError::Length { len: 8 }));
}

#[test]
fn input_with_unknown_flags_is_rejected() {
    assert_eq!(Input::decode(&[2, 0, 0, 0, 0, 0, 0, 0, 0]), Err(InputError::UnknownFlags { flags: 2 }));
}

#[test]
fn untouched_round_cuts_itself_at_the_last_tick() {
    let mut world = World::create(build_slice_missing_cake(), START_TILT_RAD, Some(ROUND_TICKS));
    for _ in 0..ROUND_TICKS + 5 {
        world.step(&Input::default());
    }
    assert!(matches!(world.get_phase(), Phase::Cut { tick: ROUND_TICKS, .. }));
    assert_eq!(world.get_tick(), ROUND_TICKS);
}

#[test]
fn untimed_round_keeps_playing() {
    let mut world = World::create(build_slice_missing_cake(), START_TILT_RAD, None);
    for _ in 0..ROUND_TICKS + 5 {
        world.step(&Input::default());
    }
    assert_eq!(world.get_phase(), Phase::Playing);
}

#[test]
fn spawned_cake_is_beside_the_stand_out_of_the_cut() {
    let world = World::create(build_slice_missing_cake(), 0.0, None);
    assert!(!world.is_cut_through());
}

/// `model` moved by `shift` from where it spawns and settled on the table there, with the knife
/// held at `tilt_rad`.
fn settle_shifted_cake(mut model: CakeModel, shift: Vec3, tilt_rad: f64) -> World {
    for p in model.mesh.positions.iter_mut().chain(model.hulls.iter_mut().flatten()) {
        *p += shift;
    }
    let mut world = World::create(model, tilt_rad, None);
    let input = Input { knife: KnifeInput { tilt: quantize_radians(tilt_rad), lower: 0 }, ..Input::default() };
    for _ in 0..60 {
        world.step(&input);
    }
    world
}

/// A box cake settled on the table where it spawns, shifted along the knife's edge by `shift_z_m`,
/// with the knife tilted so its plane meets the table through the cake.
fn settle_box_on_the_table_in_the_knife_plane(shift_z_m: f32) -> World {
    settle_shifted_cake(build_box_cake(0.15, 0.1, 0.05), Vec3::new(0.0, 0.0, shift_z_m), core::f64::consts::FRAC_PI_4)
}

#[test]
fn cake_on_the_table_under_the_stands_rim_is_not_cut_since_the_blade_stops_on_the_plate() {
    // Centered 0.2 m out, the box reaches under the rim from 0.15 m out to 0.25 m.
    let to_under_rim = Vec3::new(0.2 - CAKE_SPAWN.x as f32, 0.0, 0.0);
    let mut world = settle_shifted_cake(build_box_cake(0.05, 0.1, 0.05), to_under_rim, -0.44);
    let plane = world.get_knife().calc_plane();
    let crossed = calc_split(world.get_cake_positions(), &world.get_model().mesh.triangles, &plane);
    assert!(crossed.above_m3 > 0.0 && crossed.below_m3 > 0.0, "{crossed:?}");
    assert!(!world.is_cut_through());
    assert_eq!(world.cut().calc_score(), 0.0);
}

#[test]
fn cake_on_the_table_under_the_blade_is_cut_through() {
    let mut world = settle_box_on_the_table_in_the_knife_plane(0.0);
    assert!(world.is_cut_through());
    assert!(world.cut().below_m3 > 0.0);
}

#[test]
fn cake_beyond_the_blade_end_is_not_cut_though_the_plane_crosses_it() {
    let mut world = settle_box_on_the_table_in_the_knife_plane(0.15);
    let plane = world.get_knife().calc_plane();
    let crossed = calc_split(world.get_cake_positions(), &world.get_model().mesh.triangles, &plane);
    assert!(crossed.above_m3 > 0.0 && crossed.below_m3 > 0.0, "{crossed:?}");
    assert!(!world.is_cut_through());
    let missed = world.cut();
    assert_eq!(missed.calc_score(), 0.0);
    assert_eq!(missed.below_m3, 0.0);
    assert!((missed.above_m3 - 0.003).abs() < 1e-9, "{missed:?}");
}

#[test]
fn cake_reaching_past_the_blade_end_is_cut_whole_along_the_plane() {
    let mut world = settle_box_on_the_table_in_the_knife_plane(0.07);
    let plane = world.get_knife().calc_plane();
    let crossed = calc_split(world.get_cake_positions(), &world.get_model().mesh.triangles, &plane);
    assert!(world.is_cut_through());
    assert_eq!(world.cut(), crossed);
    assert!(crossed.above_m3 > 0.0 && crossed.below_m3 > 0.0, "{crossed:?}");
    assert!((crossed.above_m3 + crossed.below_m3 - 0.003).abs() < 1e-9, "{crossed:?}");
}

#[test]
fn cake_on_the_stand_under_the_resting_knife_is_cut_through_once_the_edge_enters_it() {
    let mut world = place_cake_on_stand();
    for _ in 0..120 {
        world.step(&Input::default());
    }
    assert!(world.is_cut_through());
    let touch_m = world.calc_cake_touch_lower_m();
    assert!((0.0..world.calc_cut_lower_m()).contains(&touch_m), "{touch_m}");
}

/// The scripted round up to the moment the carried cake has been placed on the stand and let go.
fn place_cake_on_stand() -> World {
    let mut world = World::create(build_slice_missing_cake(), START_TILT_RAD, None);
    for input in &build_placement_inputs() {
        world.step(input);
    }
    world
}

#[test]
fn carried_cake_is_placed_on_the_stand_and_let_go() {
    let world = place_cake_on_stand();
    assert!(!world.is_holding());
    let pose = world.get_cake_pose();
    assert!(pose.translation.x.abs() < 0.08, "{pose:?}");
    assert!(pose.translation.z.abs() < 0.08, "{pose:?}");
    assert!((f64::from(pose.translation.y) - STAND_TOP_Y_M).abs() < 0.01, "{pose:?}");
}

#[test]
fn turning_stand_carries_the_cake_around() {
    let mut world = place_cake_on_stand();
    let before = world.get_cake_pose().rotation;
    let quarter_turn = quantize_radians(core::f64::consts::FRAC_PI_2);
    let last_placement_input = build_placement_inputs()[299];
    for _ in 0..60 {
        world.step(&Input { stand_angle: quarter_turn, ..last_placement_input });
    }
    let turned_rad = f64::from(world.get_cake_pose().rotation.angle_between(before));
    assert!((turned_rad - core::f64::consts::FRAC_PI_2).abs() < 0.1, "{turned_rad}");
}

#[test]
fn cake_riding_the_stand_keeps_its_distance_from_the_axis_through_eight_quarter_turns() {
    let mut world = place_cake_on_stand();
    for _ in 0..120 {
        world.step(&Input::default());
    }
    let off_axis_m = |world: &World| world.get_cake_pose().translation.with_y(0.0).length();
    let before_m = off_axis_m(&world);
    for quarter_idx in 1..=8 {
        let stand_angle = quantize_radians(f64::from(quarter_idx) * core::f64::consts::FRAC_PI_2);
        for _ in 0..60 {
            world.step(&Input { stand_angle, ..Input::default() });
        }
    }
    let after_m = off_axis_m(&world);
    assert!((after_m - before_m).abs() < 0.001, "{before_m} m off the axis became {after_m} m");
}

/// The placeholder carried by its top until it hangs 22 cm from the stand's axis, half over the
/// rim, and let go, so it lands leaning against the rim.
#[test]
fn cake_let_go_leaning_on_the_stands_rim_comes_to_rest_within_3_s() {
    let mut world = World::create(build_slice_missing_cake(), START_TILT_RAD, None);
    let grab_at = calc_spawned_top_grab_at(&build_slice_missing_cake());
    let [grab_x, _, grab_z] = grab_at.map(|unit| f64::from(unit) * POSITION_UNIT_M);
    for tick in 0..300 {
        let progress = f64::from(tick.min(150)) / 150.0;
        let target_xz = [grab_x + (-0.22 - grab_x) * progress, grab_z * (1.0 - progress)].map(quantize_meters);
        world.step(&Input { hand: Some(HandInput { grab_at, target_xz }), ..Input::default() });
    }
    for _ in 0..3 * TICKS_PER_S {
        world.step(&Input::default());
    }
    let rested = world.get_cake_pose();
    let lean_rad = (rested.rotation * Vec3::Y).angle_between(Vec3::Y);
    assert!((0.5..0.7).contains(&lean_rad), "it lies leaning {lean_rad} rad rather than against the rim");
    for _ in 0..TICKS_PER_S {
        world.step(&Input::default());
    }
    assert_eq!(world.get_cake_pose(), rested);
}

/// A square box cake `2 * half_m` across and `height_m` tall, set down on its bottom edge at
/// x = `half_m` leaning `past_balance_rad` past balance toward +x, then moved by `shift`, with how far
/// it leans from upright.
fn build_leaning_box_cake(half_m: f32, height_m: f32, past_balance_rad: f32, shift: Vec3) -> (CakeModel, f32) {
    let mut model = build_box_cake(half_m, height_m, half_m);
    let lean_rad = half_m.atan2(height_m / 2.0) + past_balance_rad;
    let lean = Rotation::from_rotation_z(-lean_rad);
    let edge = Vec3::new(half_m, 0.0, 0.0);
    for p in model.mesh.positions.iter_mut().chain(model.hulls.iter_mut().flatten()) {
        *p = lean * (*p - edge) + edge + shift;
    }
    (model, lean_rad)
}

/// How far a box 6 cm wide and 10 cm tall, set down on one bottom edge leaning `past_balance_rad`
/// past balance, has turned 5 s later, and how far it would turn to lie on its side.
fn calc_leaning_box_turn_rad(past_balance_rad: f32) -> (f32, f32) {
    let (model, start_lean_rad) = build_leaning_box_cake(0.03, 0.1, past_balance_rad, Vec3::ZERO);
    let mut world = World::create(model, START_TILT_RAD, None);
    for _ in 0..5 * TICKS_PER_S {
        world.step(&Input::default());
    }
    // From the release, lying on its side is a further quarter turn less the lean it started with.
    let turned_rad = world.get_cake_pose().rotation.angle_between(Rotation::IDENTITY);
    (turned_rad, core::f32::consts::FRAC_PI_2 - start_lean_rad)
}

/// 30 mrad past balance its center of mass stands 1.7 mm past the edge, beyond the 1.2 mm the box's
/// squash flattens there, so the rolling resistance holds it only to a creep that speeds up as it
/// leans, and it tips rather than freezing.
#[test]
fn box_set_down_past_balance_on_an_edge_tips_onto_its_side_rather_than_freezing() {
    let (turned_rad, on_its_side_rad) = calc_leaning_box_turn_rad(0.03);
    assert!((turned_rad - on_its_side_rad).abs() < 0.05, "it turned {turned_rad} rad, not {on_its_side_rad}");
}

/// 4 mrad past balance its center of mass stands 0.2 mm past the edge, over the patch the squash
/// flattens, so it rests balanced as a soft cake would.
#[test]
fn box_set_down_a_hair_past_balance_on_an_edge_rests_on_its_squash() {
    let (turned_rad, _) = calc_leaning_box_turn_rad(0.004);
    assert!(turned_rad < 0.01, "it turned {turned_rad} rad");
}

/// Turning the stand wakes the cake it carries, and gravity would rock one only its squash holds a
/// little off its pose each tick.
#[test]
fn box_resting_on_its_squash_on_the_stand_keeps_its_pose_through_eight_quarter_turns() {
    let onto_plate = Vec3::new(0.05 - CAKE_SPAWN.x as f32, STAND_TOP_Y_M as f32, -CAKE_SPAWN.z as f32);
    let (model, _) = build_leaning_box_cake(0.03, 0.1, 0.004, onto_plate);
    let mut world = World::create(model, START_TILT_RAD, None);
    for _ in 0..TICKS_PER_S {
        world.step(&Input::default());
    }
    let rested = world.get_cake_pose();
    for quarter_idx in 1..=8 {
        let stand_angle = quantize_radians(f64::from(quarter_idx) * core::f64::consts::FRAC_PI_2);
        for _ in 0..60 {
            world.step(&Input { stand_angle, ..Input::default() });
        }
    }
    // Eight quarter turns bring the stand round twice, back where it started.
    let after = world.get_cake_pose();
    let moved_m = after.translation.distance(rested.translation);
    let turned_rad = after.rotation.angle_between(rested.rotation);
    assert!(moved_m < 0.001 && turned_rad < 0.01, "it moved {moved_m} m and turned {turned_rad} rad");
}

#[test]
fn cake_grabbed_on_the_stand_by_its_rim_lifts_barely_and_keeps_its_orientation() {
    let mut world = place_cake_on_stand();
    for _ in 0..120 {
        world.step(&Input::default());
    }
    let before = world.get_cake_pose().rotation;
    let rim =
        world.get_cake_positions().iter().copied().fold(DVec3::NEG_INFINITY, |a, b| if b.x > a.x { b } else { a });
    let grab_at = rim.to_array().map(quantize_meters);
    let hand = HandInput { grab_at, target_xz: [grab_at[0], grab_at[2]] };
    for _ in 0..120 {
        world.step(&Input { hand: Some(hand), ..Input::default() });
    }
    let lowest_y_m = world.get_cake_positions().iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
    assert!(lowest_y_m - STAND_TOP_Y_M < 0.02, "{lowest_y_m}");
    let slide_m = 0.05;
    for tick in 1..=60 {
        let target_x = grab_at[0] - quantize_meters(slide_m * f64::from(tick) / 60.0);
        world.step(&Input { hand: Some(HandInput { target_xz: [target_x, grab_at[2]], ..hand }), ..Input::default() });
    }
    for _ in 0..120 {
        world.step(&Input::default());
    }
    let up = |rotation: Rotation| rotation * Vec3::Y;
    let tilt_rad = up(world.get_cake_pose().rotation).angle_between(up(before));
    assert!(tilt_rad < 0.05, "{tilt_rad}");
    assert!(world.get_cake_pose().translation.x < rim.x as f32 - 0.11 - slide_m as f32 / 2.0);
}

#[test]
fn turning_the_stand_stops_where_the_cake_meets_the_lowered_knife() {
    // The placeholder on the stand 3 cm off its middle, which puts its missing slice under the knife.
    let onto_stand = Vec3::new(0.03 - CAKE_SPAWN.x as f32, STAND_TOP_Y_M as f32, -CAKE_SPAWN.z as f32);
    let mut world = settle_shifted_cake(build_slice_missing_cake(), onto_stand, 0.3);
    let tilt = quantize_radians(0.3);
    let mut input = Input { knife: KnifeInput { tilt, lower: quantize_meters(0.3) }, ..Input::default() };
    for _ in 0..120 {
        world.step(&input);
    }
    let rested_m = world.get_knife().lower_m;
    assert!(rested_m < world.get_knife().rest_m + 0.3, "the knife missed the cake");
    input.stand_angle = quantize_radians(core::f64::consts::PI);
    let mut was_blocked = false;
    for _ in 0..120 {
        world.step(&input);
        was_blocked |= world.is_stand_blocked();
        assert!(world.get_knife().lower_m > rested_m - 0.001, "{} after {rested_m}", world.get_knife().lower_m);
    }
    assert!(was_blocked);
    assert!(world.get_stand_angle_rad() < 2.0, "{}", world.get_stand_angle_rad());
}

/// A 6 cm box on the stand 10 cm off its middle, straddling the plane of an upright knife pressed
/// down onto it, is turned at full speed out from under the blade, which drops to the plate, and
/// on round into the blade's other side.
#[test]
fn stand_turned_into_a_lowered_blade_stops_at_the_cake_and_the_knife_never_climbs_onto_it() {
    let onto_stand = Vec3::new(-CAKE_SPAWN.x as f32, STAND_TOP_Y_M as f32, -0.1 - CAKE_SPAWN.z as f32);
    let mut world = settle_shifted_cake(build_box_cake(0.03, 0.06, 0.03), onto_stand, 0.0);
    let press = KnifeInput { tilt: 0, lower: quantize_meters(0.3) };
    for _ in 0..120 {
        world.step(&Input { knife: press, ..Input::default() });
    }
    let mut deepest_lower_m = world.get_knife().lower_m;
    let half_turn = Input { knife: press, stand_angle: quantize_radians(core::f64::consts::PI), ..Input::default() };
    for _ in 0..300 {
        world.step(&half_turn);
        let lower_m = world.get_knife().lower_m;
        assert!(lower_m > deepest_lower_m - 1e-3, "the knife rose to {lower_m} from {deepest_lower_m}");
        deepest_lower_m = deepest_lower_m.max(lower_m);
    }
    assert!(world.is_stand_blocked());
    // The corner at (-0.03, -0.07) reaches the plane x = 0 after a turn of pi - atan(3 / 7), and the
    // stand stops 5 milliradians short of it.
    let contact_rad = core::f64::consts::PI - libm::atan(0.03 / 0.07);
    let stopped_rad = world.get_stand_angle_rad();
    assert!((stopped_rad - (contact_rad - 0.005)).abs() < 0.002, "{stopped_rad} vs contact at {contact_rad}");
}

/// A box 6 cm wide and 12 cm tall standing on the stand 8 cm from the plane of an upright knife
/// pressed down to the plate, set down leaning 0.2 rad past balance toward the blade, so it tips
/// over into it. Its far top corner meets the plane 9 cm above the plate, below the blade's back,
/// and lying down it would reach 4 cm past the plane.
#[test]
fn cake_tipping_into_a_pressed_blade_rests_against_it_until_the_knife_rises_and_the_knife_never_climbs_onto_it() {
    let onto_stand = Vec3::new(-0.11 - CAKE_SPAWN.x as f32, STAND_TOP_Y_M as f32, -CAKE_SPAWN.z as f32);
    let (model, start_lean_rad) = build_leaning_box_cake(0.03, 0.12, 0.2, onto_stand);
    let mut world = World::create(model, 0.0, None);
    let press = Input { knife: KnifeInput { tilt: 0, lower: quantize_meters(0.3) }, ..Input::default() };
    world.step(&press);
    let pressed_lower_m = world.get_knife().lower_m;
    for _ in 0..2 * TICKS_PER_S {
        world.step(&press);
        let lower_m = world.get_knife().lower_m;
        assert!(lower_m > pressed_lower_m - 1e-3, "the knife rose to {lower_m} from {pressed_lower_m}");
    }
    let crossing_m = world.get_cake_positions().iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max);
    assert!(crossing_m < 0.0, "the box reaches {crossing_m} m past the blade's plane");
    for _ in 0..2 * TICKS_PER_S {
        world.step(&Input::default());
    }
    let turned_rad = world.get_cake_pose().rotation.angle_between(Rotation::IDENTITY);
    let on_its_side_rad = core::f32::consts::FRAC_PI_2 - start_lean_rad;
    assert!((turned_rad - on_its_side_rad).abs() < 0.05, "it turned {turned_rad} rad, not {on_its_side_rad}");
}

/// How deep the cake's deepest vertex lies inside the stand's plate, column, or base.
fn calc_stand_depth_m(world: &World) -> f64 {
    let depths = world.get_cake_positions().iter().flat_map(|p| {
        STAND_PARTS.map(|part| (part.radius_m - p.with_y(0.0).length()).min(p.y - part.y0_m).min(part.y1_m - p.y))
    });
    depths.fold(0.0, f64::max)
}

/// A box 8 cm across and 5 cm tall, short enough to fit under the stand's plate, grabbed by a top
/// corner and carried at full speed from where it spawns to past the stand's far side.
#[test]
fn short_cake_carried_low_at_the_stand_rises_over_the_plate_rather_than_slipping_under_it() {
    let model = build_box_cake(0.04, 0.05, 0.04);
    let grab_at = calc_spawned_top_grab_at(&model);
    let mut world = World::create(model, START_TILT_RAD, None);
    let across = [0.3, 0.0].map(quantize_meters);
    for tick in 0..120 {
        world.step(&Input { hand: Some(HandInput { grab_at, target_xz: across }), ..Input::default() });
        let depth_m = calc_stand_depth_m(&world);
        assert!(depth_m < 0.002, "at tick {tick} a corner sank {depth_m} m into the stand");
    }
}

#[test]
fn held_cake_dragged_through_the_stand_does_not_clip_into_it() {
    let (mut world, hand) = hold_cake_by_its_rim();
    let across = [0.4, 0.0].map(quantize_meters);
    let plate_bottom_y_m = STAND_TOP_Y_M - STAND_PLATE_THICKNESS_M;
    for _ in 0..240 {
        world.step(&Input { hand: Some(HandInput { target_xz: across, ..hand }), ..Input::default() });
        let depth_m = world
            .get_cake_positions()
            .iter()
            .filter(|p| (plate_bottom_y_m..STAND_TOP_Y_M - 0.003).contains(&p.y))
            .map(|p| STAND_RADIUS_M - p.with_y(0.0).length())
            .fold(0.0, f64::max);
        assert!(depth_m < 0.01, "a cake corner sank {depth_m} m into the plate");
    }
}

#[test]
fn cake_falling_off_the_table_respawns() {
    let mut world = World::create(build_slice_missing_cake(), START_TILT_RAD, None);
    let grab_at = [CAKE_SPAWN.x - 0.11, 0.035, CAKE_SPAWN.z].map(quantize_meters);
    let edge = [-TABLE_HALF_X_M, TABLE_HALF_Z_M].map(quantize_meters);
    let carry = Input { hand: Some(HandInput { grab_at, target_xz: edge }), ..Input::default() };
    let mut has_left_spawn = false;
    for tick in 0..900 {
        world.step(&if tick < 200 { carry } else { Input::default() });
        has_left_spawn |= world.get_cake_pose().translation.distance(CAKE_SPAWN.as_vec3()) > 0.2;
    }
    assert!(has_left_spawn);
    assert!(world.get_cake_pose().translation.distance(CAKE_SPAWN.as_vec3()) < 0.05, "{:?}", world.get_cake_pose());
}

#[test]
fn held_cake_dragged_into_the_lowered_knife_is_not_pulled_through_the_blade() {
    let mut world = World::create(build_slice_missing_cake(), START_TILT_RAD, None);
    let grab_at = [CAKE_SPAWN.x, 0.12, CAKE_SPAWN.z].map(quantize_meters);
    let across = [0.3, 0.0].map(quantize_meters);
    let knife = KnifeInput { tilt: 0, lower: quantize_meters(0.2) };
    // Lifted in place first, so the drag carries it high enough to clear the stand's rim and meet
    // the blade, rather than catching on the rim and sliding around the blade's end.
    let lift = Input { hand: Some(HandInput { grab_at, target_xz: [grab_at[0], grab_at[2]] }), knife, stand_angle: 0 };
    let carry = Input { hand: Some(HandInput { grab_at, target_xz: across }), knife, stand_angle: 0 };
    for tick in 0..300 {
        world.step(if tick < 60 { &lift } else { &carry });
    }
    let knife = world.get_knife();
    assert!((knife.lower_m - knife.rest_m - 0.2).abs() < 1e-9, "{knife:?}");
    let min_x_m = world.get_cake_positions().iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
    assert!(min_x_m < -0.02, "the cake passed wholly through the blade, to x >= {min_x_m}");
}

/// With `placeholder_cake_validates`, which holds the bound within `MAX_CAKE_EXTENT_M`, this keeps
/// the placeholder below the pivot in any pose on the stand.
#[test]
fn extent_bound_is_at_least_the_placeholders_longest_distance_across() {
    let mesh = build_slice_missing_cake().mesh;
    let positions = &mesh.positions;
    let extent_m = positions.iter().flat_map(|a| positions.iter().map(move |b| a.distance(*b))).fold(0.0, f32::max);
    assert!(extent_m <= mesh.calc_extent_bound(), "{extent_m} vs {}", mesh.calc_extent_bound());
}

/// How high the upright knife's edge rests over `model` as it spawns moved by `shift` from where
/// it usually spawns.
fn calc_resting_edge_y_m(mut model: CakeModel, shift: DVec3) -> f64 {
    for p in model.mesh.positions.iter_mut().chain(model.hulls.iter_mut().flatten()) {
        *p += shift.as_vec3();
    }
    PIVOT.y - World::create(model, 0.0, None).get_knife().lower_m
}

/// Moves a cake from where it spawns onto the middle of the stand.
const ONTO_STAND: DVec3 = DVec3::new(-CAKE_SPAWN.x, STAND_TOP_Y_M, -CAKE_SPAWN.z);
/// Moves a cake up to 0.2 m across from where it spawns to the table's corner farthest from the
/// stand, where it is away from the stand.
const TO_FAR_CORNER: DVec3 = DVec3::new(TABLE_HALF_X_M - 0.1 - CAKE_SPAWN.x, 0.0, TABLE_HALF_Z_M - 0.1 - CAKE_SPAWN.z);

#[test]
fn knife_rests_5_cm_above_the_top_of_a_cake_on_the_stand_as_it_is_posed() {
    let flat_y_m = calc_resting_edge_y_m(build_box_cake(0.1, 0.04, 0.1), ONTO_STAND);
    assert!((flat_y_m - 0.21).abs() < 1e-6, "{flat_y_m}");
    let tall_y_m = calc_resting_edge_y_m(build_box_cake(0.02, 0.2, 0.02), ONTO_STAND);
    assert!((tall_y_m - 0.37).abs() < 1e-6, "{tall_y_m}");
}

/// The box's flattest pose stands 0.04 m tall and its tallest, on a corner, 0.230 m, so the edge
/// waits 5 cm above 0.135 m over the stand's top. The tallest pose is found along sampled
/// directions, hence the millimeter's slack.
#[test]
fn knife_waits_over_the_middle_of_a_cakes_flattest_and_tallest_heights_away_from_the_stand() {
    let edge_y_m = calc_resting_edge_y_m(build_box_cake(0.08, 0.04, 0.08), TO_FAR_CORNER);
    assert!((edge_y_m - 0.305).abs() < 1e-3, "{edge_y_m}");
}

#[test]
fn resting_knife_floats_halfway_in_0_7_s_and_never_passes_where_it_rests() {
    let mut knife = UPRIGHT_KNIFE;
    let mut rests_m = Vec::new();
    for _ in 0..10 * TICKS_PER_S {
        knife.float_over(0.1, 1.0 / f64::from(TICKS_PER_S));
        rests_m.push(knife.rest_m);
    }
    // Over a cake 0.1 m tall on the stand the edge rests 5 cm above it, 0.21 m below the pivot.
    let rest_m = 0.21;
    assert!(rests_m[0] < 0.01 * rest_m, "jumped to {}", rests_m[0]);
    let at_0_7_s_m = rests_m[TICKS_PER_S as usize * 7 / 10 - 1];
    assert!((0.45 * rest_m..0.55 * rest_m).contains(&at_0_7_s_m), "{at_0_7_s_m}");
    assert!(rests_m.windows(2).all(|pair| pair[0] <= pair[1]), "turned back");
    assert!(rests_m.iter().all(|&m| m <= rest_m), "passed {rest_m}");
    assert!((rests_m.last().unwrap() - rest_m).abs() < 1e-6, "{rests_m:?}");
    assert_eq!(knife.lower_m, 0.0);
}

/// Hung on its side the placeholder stands much taller than lying flat, yet carried to the table's
/// far corner the knife waits where it rests over the cake lying flat there.
#[test]
fn resting_knife_waits_over_a_held_cake_on_its_side_until_it_is_carried_onto_the_stand() {
    let flat_edge_y_m = calc_resting_edge_y_m(build_slice_missing_cake(), TO_FAR_CORNER);
    let (mut world, hand) = hold_cake_by_its_rim();
    let far_corner_xz = [TABLE_HALF_X_M, TABLE_HALF_Z_M].map(quantize_meters);
    for _ in 0..3 * TICKS_PER_S {
        world.step(&Input { hand: Some(HandInput { target_xz: far_corner_xz, ..hand }), ..Input::default() });
    }
    let waiting_rest_m = world.get_knife().rest_m;
    let waiting_edge_y_m = PIVOT.y - waiting_rest_m;
    assert!((waiting_edge_y_m - flat_edge_y_m).abs() < 0.005, "{waiting_edge_y_m} vs {flat_edge_y_m}");
    for _ in 0..3 * TICKS_PER_S {
        world.step(&Input { hand: Some(HandInput { target_xz: [0, 0], ..hand }), ..Input::default() });
    }
    let knife = world.get_knife();
    assert!(knife.rest_m < waiting_rest_m - 0.04, "{knife:?} vs {waiting_rest_m}");
    assert_eq!(knife.lower_m, knife.rest_m);
}

/// How far into the cake the blade's edge would reach were the cake moved just across the
/// blade's plane, or 0 where it would clear the cake or the cake lies over 2 cm off the plane. A
/// held cake the blade stops short of its plane counts as in the way too.
fn calc_blade_overlap_m(world: &World) -> f64 {
    let (near_blade_m, across_m) = (0.02, 0.002);
    let knife = world.get_knife();
    let plane = knife.calc_plane();
    let positions = world.get_cake_positions();
    let offsets_m = positions.iter().map(|p| (*p - plane.point).dot(plane.normal));
    let (low_m, high_m) =
        offsets_m.fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), m| (low.min(m), high.max(m)));
    let shift_m = if low_m > 0.0 {
        -low_m - across_m
    } else if high_m < 0.0 {
        across_m - high_m
    } else {
        0.0
    };
    if shift_m.abs() > near_blade_m + across_m {
        return 0.0;
    }
    let shifted: Vec<DVec3> = positions.iter().map(|p| *p + plane.normal * shift_m).collect();
    let cake = Solid::new(&shifted, &world.get_model().mesh.triangles);
    (knife.lower_m - knife.calc_lower_limit_m(&[cake])).max(0.0)
}

/// A pole standing at its tallest, picked up by its top, held for 40 ticks, then carried 0.44 m
/// from where it spawns onto the stand's middle at 1 m/s under a blade tilted toward it.
#[test]
fn resting_knife_rises_clear_of_a_tall_cake_carried_onto_the_stand() {
    let mut world = World::create(build_box_cake(0.025, 0.25, 0.025), MAX_TILT_RAD, None);
    let grab_at = [CAKE_SPAWN.x, 0.25, CAKE_SPAWN.z].map(quantize_meters);
    let knife = KnifeInput { tilt: quantize_radians(MAX_TILT_RAD), lower: 0 };
    let (hold_ticks, carry_ticks) = (40, 26);
    for tick in 0..5 * TICKS_PER_S as i32 {
        let progress = f64::from((tick - hold_ticks).clamp(0, carry_ticks)) / f64::from(carry_ticks);
        let target_xz = [CAKE_SPAWN.x, CAKE_SPAWN.z].map(|m| quantize_meters(m * (1.0 - progress)));
        world.step(&Input { hand: Some(HandInput { grab_at, target_xz }), knife, stand_angle: 0 });
        let overlap_m = calc_blade_overlap_m(&world);
        assert_eq!(overlap_m, 0.0, "the blade reached {overlap_m} m into the cake at tick {tick}");
    }
    let translation = world.get_cake_pose().translation;
    assert!(translation.with_y(0.0).length() < 0.01, "{translation:?}");
}

/// Grabs the placeholder's bottom tier at its rim away from the stand, which hangs it on its
/// side, and holds it still above the table for a second.
fn hold_cake_by_its_rim() -> (World, HandInput) {
    let mut world = World::create(build_slice_missing_cake(), START_TILT_RAD, None);
    let grab_at = [CAKE_SPAWN.x - 0.11, 0.035, CAKE_SPAWN.z].map(quantize_meters);
    let hand = HandInput { grab_at, target_xz: [grab_at[0], grab_at[2]] };
    for _ in 0..60 {
        world.step(&Input { hand: Some(hand), ..Input::default() });
    }
    (world, hand)
}

#[test]
fn carried_cake_far_from_the_stand_clears_the_table_just_barely() {
    let (world, _) = hold_cake_by_its_rim();
    let lowest_y_m = world.get_cake_positions().iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
    assert!((0.0..STAND_TOP_Y_M / 2.0).contains(&lowest_y_m), "{lowest_y_m}");
}

#[test]
fn dropped_cake_comes_to_rest_without_rolling_away() {
    let (mut world, _) = hold_cake_by_its_rim();
    let calc_centroid = |world: &World| {
        let positions = world.get_cake_positions();
        positions.iter().copied().sum::<DVec3>() / positions.len() as f64
    };
    let dropped_at = calc_centroid(&world);
    for _ in 0..180 {
        world.step(&Input::default());
    }
    let before = world.get_cake_pose();
    world.step(&Input::default());
    assert_eq!(world.get_cake_pose(), before, "still moving 3 s after the drop");
    let travel_m = (calc_centroid(&world) - dropped_at).with_y(0.0).length();
    assert!(travel_m < 0.15, "{travel_m}");
}

#[test]
fn grab_away_from_the_cake_is_ignored() {
    let mut world = World::create(build_slice_missing_cake(), START_TILT_RAD, None);
    let start = world.get_cake_pose().translation;
    let input = Input { hand: Some(HandInput { grab_at: [40_000, 1_000, 0], target_xz: [0, 0] }), ..Input::default() };
    for _ in 0..60 {
        world.step(&input);
    }
    assert!(world.get_cake_pose().translation.distance(start) < 1e-3);
}

#[test]
fn settled_lopsided_cake_grabbed_at_its_far_light_end_is_carried() {
    // A thin bar with a heavy block on one end, like a guitar's neck and body, so the far end of the
    // bar lies much farther from the center of mass than from the mesh's origin.
    let bar = build_box([-0.12, 0.0, -0.01], [0.04, 0.01, 0.01]);
    let block = build_box([0.04, 0.0, -0.04], [0.12, 0.06, 0.04]);
    let hulls = vec![bar.positions.clone(), block.positions.clone()];
    let offset = bar.positions.len() as u32;
    let triangles =
        bar.triangles.iter().copied().chain(block.triangles.iter().map(|t| t.map(|i| i + offset))).collect();
    let mesh = Mesh { positions: [bar.positions, block.positions].concat(), triangles };
    let mut world = World::create(build_plain_cake(mesh, hulls), START_TILT_RAD, None);
    for _ in 0..60 {
        world.step(&Input::default());
    }
    let tip = world.get_cake_pose().transform_point(Vec3::new(-0.115, 0.01, 0.0));
    let start = world.get_cake_pose().translation;
    let grab_at = [tip.x, tip.y, tip.z].map(|m| quantize_meters(f64::from(m)));
    let input = Input { hand: Some(HandInput { grab_at, target_xz: [0, 0] }), ..Input::default() };
    for _ in 0..60 {
        world.step(&input);
    }
    assert!(world.is_holding());
    assert!(world.get_cake_pose().translation.distance(start) > 0.1);
}

#[test]
fn replaying_the_same_inputs_gives_the_same_hashes() {
    let inputs = build_scripted_inputs();
    assert_eq!(replay(&inputs), replay(&inputs));
}

#[test]
fn quantizing_rounds_to_the_nearest_unit() {
    assert_eq!(quantize_meters(0.123_456), 12_346);
    assert_eq!(quantize_meters(-0.000_014), -1);
    assert_eq!(quantize_radians(1.0), 100_000);
    assert_eq!(quantize_radians(-0.000_004), 0);
}

#[test]
fn stand_parts_stack_from_the_table_to_the_plate_top() {
    let [plate, column, base] = STAND_PARTS;
    assert_eq!(base.y0_m, 0.0);
    assert_eq!(base.y1_m, column.y0_m);
    assert_eq!(column.y1_m, plate.y0_m);
    assert_eq!(plate.y1_m, STAND_TOP_Y_M);
    assert_eq!(plate.radius_m, STAND_RADIUS_M);
    assert!((plate.y1_m - plate.y0_m - STAND_PLATE_THICKNESS_M).abs() < 1e-12);
}

#[test]
fn scripted_round_ends_with_the_cake_carried_off_the_stand_and_dropped() {
    let mut world = World::create(build_slice_missing_cake(), START_TILT_RAD, None);
    for input in &build_scripted_inputs() {
        world.step(input);
    }
    let translation = world.get_cake_pose().translation;
    assert!(!world.is_holding());
    assert!(translation.with_y(0.0).length() > 0.3, "{translation:?}");
    assert!(translation.y < 0.1, "{translation:?}");
}
