use cake_core::cakes::{CakeError, CakeModel, MAX_HULL_VERTICES, Skin, build_slice_missing_cake};
use cake_core::cut::{Plane, Split, calc_split};
use cake_core::input::{HandInput, Input, InputError, KnifeInput, quantize_meters, quantize_radians};
use cake_core::knife::{Knife, MAX_RIDE_M, START_TILT_RAD, Solid, calc_edge_rise_m};
use cake_core::mesh::{Mesh, MeshError};
use cake_core::replay::{build_placement_inputs, build_scripted_inputs, replay};
use cake_core::sim::{Phase, ROUND_TICKS, World};
use cake_core::table::{
    CAKE_SPAWN, MAX_CAKE_EXTENT_M, MAX_PIVOT, STAND_PARTS, STAND_PLATE_THICKNESS_M, STAND_RADIUS_M, STAND_TOP_Y_M,
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
fn placeholder_cake_is_closed() {
    assert_eq!(build_slice_missing_cake().mesh.validate_closed(), Ok(()));
}

/// A box resting on y = 0 and centered on the stand, with a matching hull.
fn build_box_cake(half_x_m: f32, height_m: f32, half_z_m: f32) -> CakeModel {
    let mesh = build_box([-half_x_m, 0.0, -half_z_m], [half_x_m, height_m, half_z_m]);
    let hull = mesh.positions.clone();
    CakeModel { mesh, skin: Skin::Plain, hulls: vec![hull] }
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
    let mut wrong_version = bytes.clone();
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
    assert!(matches!(CakeModel::decode(&model.encode()), Err(CakeError::Mesh(MeshError::OpenEdge { .. }))));
}

#[test]
fn validate_accepts_a_box_and_names_what_is_wrong_otherwise() {
    assert_eq!(build_box_cake(0.05, 0.05, 0.05).validate(), Ok(()));

    let mut inside_out = build_box_cake(0.05, 0.05, 0.05);
    inside_out.mesh.triangles.iter_mut().for_each(|triangle| triangle.swap(1, 2));
    assert_eq!(inside_out.validate(), Err(CakeError::NotOutward));

    let too_long_m = MAX_CAKE_EXTENT_M as f32 + 0.01;
    assert_eq!(build_box_cake(0.05, too_long_m, 0.05).validate(), Err(CakeError::TooLong));

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
    fine_hull.hulls[0] = (0..MAX_HULL_VERTICES as u32 + 1)
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

/// An upright knife resting over the tallest cake's pivot.
const UPRIGHT_KNIFE: Knife = Knife { pivot: MAX_PIVOT, tilt_rad: 0.0, lower_m: 0.0 };

fn calc_limit_m(knife: &Knife, mesh: &Mesh) -> f64 {
    let positions = to_dvec3s(mesh);
    knife.calc_lower_limit_m(&[Solid { positions: &positions, triangles: &mesh.triangles }])
}

#[test]
fn upright_knife_over_an_empty_table_lowers_to_the_table() {
    assert!((UPRIGHT_KNIFE.calc_lower_limit_m(&[]) - MAX_PIVOT.y).abs() < 1e-12);
}

#[test]
fn upright_knife_stops_on_a_box_cake_under_its_edge_middle() {
    let cake = build_box([-0.05, 0.0, -0.01], [0.05, 0.1, 0.01]);
    let limit_m = calc_limit_m(&UPRIGHT_KNIFE, &cake);
    assert!((limit_m - (MAX_PIVOT.y - 0.1)).abs() < 1e-7, "{limit_m}");
}

#[test]
fn curved_edge_reaches_a_box_off_its_middle_later() {
    let cake = build_box([-0.05, 0.0, 0.1], [0.05, 0.1, 0.15]);
    let limit_m = calc_limit_m(&UPRIGHT_KNIFE, &cake);
    let expected_m = MAX_PIVOT.y - 0.1 + calc_edge_rise_m(0.1);
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
            let u_m = -0.1 + 0.2 * i as f64 / 2000.0;
            let top_y_m = 0.1 + 0.5 * u_m;
            MAX_PIVOT.y - top_y_m + calc_edge_rise_m(u_m)
        })
        .fold(f64::INFINITY, f64::min);
    assert!((limit_m - brute_m).abs() < 1e-6, "{limit_m} vs {brute_m}");
}

#[test]
fn knife_ignores_a_cake_beyond_its_blade_end() {
    let cake = build_box([-0.05, 0.0, 0.3], [0.05, 0.15, 0.4]);
    let limit_m = calc_limit_m(&UPRIGHT_KNIFE, &cake);
    assert!((limit_m - MAX_PIVOT.y).abs() < 1e-12, "{limit_m}");
}

#[test]
fn lowered_knife_tilted_into_a_cake_wall_rides_up_it_at_most_the_ride_allowance_then_stops() {
    let cake = build_box([-0.15, 0.0, -0.1], [-0.05, 0.2, 0.1]);
    let positions = to_dvec3s(&cake);
    let solids = [Solid { positions: &positions, triangles: &cake.triangles }];
    let lower = quantize_meters(0.3);
    let mut knife = UPRIGHT_KNIFE;
    knife.step(&KnifeInput { tilt: 0, lower }, &solids);
    assert!((knife.lower_m - 0.3).abs() < 1e-3, "{knife:?}");
    for _ in 0..10 {
        knife.step(&KnifeInput { tilt: quantize_radians(0.5), lower }, &solids);
    }
    assert!(knife.lower_m > 0.3 - MAX_RIDE_M - 1e-3, "{knife:?}");
    assert!((0.1..0.2).contains(&knife.tilt_rad), "{knife:?}");
}

/// Tilts a knife pressed 5 mm into a flat cake top at y = 0.2 toward a ridge of `ridge_height_m`
/// across its path, and returns how far it tilted.
fn tilt_across_a_ridge(ridge_height_m: f32) -> f64 {
    let slab = build_box([-0.15, 0.0, -0.1], [0.15, 0.2, 0.1]);
    let ridge = build_box([-0.07, 0.2, -0.1], [-0.06, 0.2 + ridge_height_m, 0.1]);
    let (slab_positions, ridge_positions) = (to_dvec3s(&slab), to_dvec3s(&ridge));
    let solids = [
        Solid { positions: &slab_positions, triangles: &slab.triangles },
        Solid { positions: &ridge_positions, triangles: &ridge.triangles },
    ];
    let lower = quantize_meters(MAX_PIVOT.y - 0.2 + 0.005);
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

/// A box cake settled on the table where it spawns, shifted along the knife's edge by `shift_z_m`,
/// with the knife tilted so its plane meets the table through the cake.
fn settle_box_on_the_table_in_the_knife_plane(shift_z_m: f32) -> World {
    let mut model = build_box_cake(0.15, 0.1, 0.05);
    for p in model.mesh.positions.iter_mut().chain(model.hulls.iter_mut().flatten()) {
        p.z += shift_z_m;
    }
    let tilt_rad = -core::f64::consts::FRAC_PI_4;
    let mut world = World::create(model, tilt_rad, None);
    let input = Input { knife: KnifeInput { tilt: quantize_radians(tilt_rad), lower: 0 }, ..Input::default() };
    for _ in 0..60 {
        world.step(&input);
    }
    world
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
    assert_eq!(world.cut().calc_score(), 0.0);
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
    assert!((pose.translation.y as f64 - STAND_TOP_Y_M).abs() < 0.01, "{pose:?}");
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
    let turned_rad = world.get_cake_pose().rotation.angle_between(before) as f64;
    assert!((turned_rad - core::f64::consts::FRAC_PI_2).abs() < 0.1, "{turned_rad}");
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
        let target_x = grab_at[0] - quantize_meters(slide_m * tick as f64 / 60.0);
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
    let mut world = place_cake_on_stand();
    let tilt = quantize_radians(0.3);
    let mut input = Input { knife: KnifeInput { tilt, lower: quantize_meters(0.3) }, ..Input::default() };
    for _ in 0..120 {
        world.step(&input);
    }
    let rested_m = world.get_knife().lower_m;
    assert!(rested_m < 0.3, "the knife missed the cake");
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

#[test]
fn held_cake_dragged_through_the_stand_does_not_clip_into_it() {
    let (mut world, hand) = hold_cake_by_its_rim();
    let across = [-0.4, 0.0].map(quantize_meters);
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
    let grab_at = [CAKE_SPAWN.x + 0.11, 0.035, CAKE_SPAWN.z].map(quantize_meters);
    let edge = [TABLE_HALF_X_M, TABLE_HALF_Z_M].map(quantize_meters);
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
    let across = [-0.3, 0.0].map(quantize_meters);
    let knife = KnifeInput { tilt: 0, lower: quantize_meters(0.2) };
    let carry = Input { hand: Some(HandInput { grab_at, target_xz: across }), knife, stand_angle: 0 };
    for _ in 0..240 {
        world.step(&carry);
    }
    assert!((world.get_knife().lower_m - 0.2).abs() < 1e-9, "{:?}", world.get_knife());
    let max_x_m = world.get_cake_positions().iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max);
    assert!(max_x_m > 0.02, "the cake passed wholly through the blade, to x <= {max_x_m}");
}

#[test]
fn placeholder_cake_fits_under_the_resting_knife_in_any_pose() {
    let positions = &build_slice_missing_cake().mesh.positions;
    let extent_m = positions.iter().flat_map(|a| positions.iter().map(move |b| a.distance(*b))).fold(0.0, f32::max);
    assert!(extent_m as f64 <= MAX_CAKE_EXTENT_M, "{extent_m}");
}

#[test]
fn knife_rests_just_above_the_cake_standing_on_its_longest_extent() {
    let pivot_y_m = |model: CakeModel| World::create(model, START_TILT_RAD, None).get_knife().pivot.y;
    let cube_m = 0.08_f64;
    let cube_pivot_y_m = pivot_y_m(build_box_cake(cube_m as f32 / 2.0, cube_m as f32, cube_m as f32 / 2.0));
    let cube_diagonal_m = cube_m * 3.0_f64.sqrt();
    assert!(cube_pivot_y_m > STAND_TOP_Y_M + cube_diagonal_m, "{cube_pivot_y_m}");
    assert!(cube_pivot_y_m < STAND_TOP_Y_M + cube_diagonal_m + 0.03, "{cube_pivot_y_m}");
    let log_pivot_y_m = pivot_y_m(build_box_cake(0.1, 0.04, 0.02));
    assert!(log_pivot_y_m > cube_pivot_y_m, "{log_pivot_y_m} vs {cube_pivot_y_m}");
    assert!(pivot_y_m(build_slice_missing_cake()) <= MAX_PIVOT.y);
}

/// Grabs the placeholder's bottom tier at its rim away from the stand, which hangs it on its
/// side, and holds it still above the table for a second.
fn hold_cake_by_its_rim() -> (World, HandInput) {
    let mut world = World::create(build_slice_missing_cake(), START_TILT_RAD, None);
    let grab_at = [CAKE_SPAWN.x + 0.11, 0.035, CAKE_SPAWN.z].map(quantize_meters);
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
    let input = Input { hand: Some(HandInput { grab_at: [-40_000, 1_000, 0], target_xz: [0, 0] }), ..Input::default() };
    for _ in 0..60 {
        world.step(&input);
    }
    assert!(world.get_cake_pose().translation.distance(start) < 1e-3);
}

#[test]
fn replaying_the_same_inputs_gives_the_same_hashes() {
    let inputs = build_scripted_inputs();
    assert_eq!(replay(&inputs), replay(&inputs));
}

#[test]
fn quantizing_rounds_to_the_nearest_unit() {
    assert_eq!(quantize_meters(0.123456), 12_346);
    assert_eq!(quantize_meters(-0.000014), -1);
    assert_eq!(quantize_radians(1.0), 100_000);
    assert_eq!(quantize_radians(-0.000004), 0);
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
