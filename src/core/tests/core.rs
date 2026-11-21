use cake_core::cakes::build_slice_missing_cake;
use cake_core::cut::{Plane, Split, calc_split};
use cake_core::input::{ANGLE_UNIT_RAD, HandInput, Input, InputError, KnifeInput, POSITION_UNIT_M};
use cake_core::knife::{Knife, START_KNIFE, Solid, calc_edge_rise_m};
use cake_core::mesh::{Mesh, MeshError};
use cake_core::replay::{build_scripted_inputs, replay};
use cake_core::sim::{Phase, ROUND_TICKS, World};
use cake_core::table::{CAKE_SPAWN, MAX_CAKE_EXTENT_M, PIVOT, STAND_TOP_Y_M, TABLE_HALF_X_M, TABLE_HALF_Z_M};
use rapier3d::glamx::DVec3;
use rapier3d::math::Vec3;

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

fn calc_limit_m(knife: &Knife, mesh: &Mesh) -> f64 {
    let positions = to_dvec3s(mesh);
    knife.calc_lower_limit_m(&[Solid { positions: &positions, triangles: &mesh.triangles }])
}

#[test]
fn upright_knife_over_an_empty_table_lowers_to_the_table() {
    assert!((Knife::default().calc_lower_limit_m(&[]) - PIVOT.y).abs() < 1e-12);
}

#[test]
fn upright_knife_stops_on_a_box_cake_under_its_edge_middle() {
    let cake = build_box([-0.05, 0.0, -0.01], [0.05, 0.1, 0.01]);
    let limit_m = calc_limit_m(&Knife::default(), &cake);
    assert!((limit_m - (PIVOT.y - 0.1)).abs() < 1e-7, "{limit_m}");
}

#[test]
fn curved_edge_reaches_a_box_off_its_middle_later() {
    let cake = build_box([-0.05, 0.0, 0.1], [0.05, 0.1, 0.15]);
    let limit_m = calc_limit_m(&Knife::default(), &cake);
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
    let limit_m = calc_limit_m(&Knife::default(), &cake);
    let brute_m = (0..=2000)
        .map(|i| {
            let u_m = -0.1 + 0.2 * i as f64 / 2000.0;
            let top_y_m = 0.1 + 0.5 * u_m;
            PIVOT.y - top_y_m + calc_edge_rise_m(u_m)
        })
        .fold(f64::INFINITY, f64::min);
    assert!((limit_m - brute_m).abs() < 1e-6, "{limit_m} vs {brute_m}");
}

#[test]
fn knife_ignores_a_cake_beyond_its_blade_end() {
    let cake = build_box([-0.05, 0.0, 0.3], [0.05, 0.15, 0.4]);
    let limit_m = calc_limit_m(&Knife::default(), &cake);
    assert!((limit_m - PIVOT.y).abs() < 1e-12, "{limit_m}");
}

#[test]
fn knife_rotation_matches_its_frame() {
    let knife = Knife { tilt_rad: 0.3, lower_m: 0.0 };
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
    let mut world = World::create(build_slice_missing_cake(), START_KNIFE, Some(ROUND_TICKS));
    for _ in 0..ROUND_TICKS + 5 {
        world.step(&Input::default());
    }
    assert!(matches!(world.get_phase(), Phase::Cut { tick: ROUND_TICKS, .. }));
    assert_eq!(world.get_tick(), ROUND_TICKS);
}

#[test]
fn untimed_round_keeps_playing() {
    let mut world = World::create(build_slice_missing_cake(), START_KNIFE, None);
    for _ in 0..ROUND_TICKS + 5 {
        world.step(&Input::default());
    }
    assert_eq!(world.get_phase(), Phase::Playing);
}

#[test]
fn spawned_cake_is_beside_the_stand_out_of_the_cut() {
    let world = World::create(build_slice_missing_cake(), Knife::default(), None);
    assert!(!world.is_cut_through());
}

#[test]
fn carried_cake_is_placed_on_the_stand_and_let_go() {
    let mut world = World::create(build_slice_missing_cake(), START_KNIFE, None);
    for input in &build_scripted_inputs()[..300] {
        world.step(input);
    }
    assert!(!world.is_holding());
    let pose = world.get_cake_pose();
    assert!(pose.translation.x.abs() < 0.08, "{pose:?}");
    assert!(pose.translation.z.abs() < 0.08, "{pose:?}");
    assert!((pose.translation.y as f64 - STAND_TOP_Y_M).abs() < 0.01, "{pose:?}");
}

#[test]
fn turning_stand_carries_the_cake_around() {
    let mut world = World::create(build_slice_missing_cake(), START_KNIFE, None);
    let inputs = build_scripted_inputs();
    for input in &inputs[..300] {
        world.step(input);
    }
    let before = world.get_cake_pose().rotation;
    let quarter_turn = (core::f64::consts::FRAC_PI_2 / ANGLE_UNIT_RAD) as i32;
    for _ in 0..60 {
        world.step(&Input { stand_angle: quarter_turn, ..inputs[299] });
    }
    let turned_rad = world.get_cake_pose().rotation.angle_between(before) as f64;
    assert!((turned_rad - core::f64::consts::FRAC_PI_2).abs() < 0.1, "{turned_rad}");
}

#[test]
fn cake_falling_off_the_table_respawns() {
    let mut world = World::create(build_slice_missing_cake(), START_KNIFE, None);
    let grab_at = [CAKE_SPAWN.x, 0.05, CAKE_SPAWN.z].map(|m| (m / POSITION_UNIT_M).round() as i32);
    let edge = [TABLE_HALF_X_M, TABLE_HALF_Z_M].map(|m| (m / POSITION_UNIT_M).round() as i32);
    let carry = Input { hand: Some(HandInput { grab_at, target_xz: edge }), ..Input::default() };
    let mut has_left_spawn = false;
    for tick in 0..900 {
        world.step(&if tick < 200 { carry } else { Input::default() });
        has_left_spawn |= world.get_cake_pose().translation.distance(CAKE_SPAWN.as_vec3()) > 0.2;
    }
    assert!(has_left_spawn);
    assert!(world.get_cake_pose().translation.distance(CAKE_SPAWN.as_vec3()) < 0.05);
}

#[test]
fn held_cake_dragged_into_the_lowered_knife_is_not_pulled_through_the_blade() {
    let mut world = World::create(build_slice_missing_cake(), START_KNIFE, None);
    let grab_at = [CAKE_SPAWN.x, 0.12, CAKE_SPAWN.z].map(|m| (m / POSITION_UNIT_M).round() as i32);
    let across = [-0.3, 0.0].map(|m| (m / POSITION_UNIT_M).round() as i32);
    let knife = KnifeInput { tilt: 0, lower: (0.2 / POSITION_UNIT_M) as i32 };
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

/// Grabs the placeholder's bottom tier at its rim away from the stand, which hangs it on its
/// side, and holds it still above the table for a second.
fn hold_cake_by_its_rim() -> (World, HandInput) {
    let mut world = World::create(build_slice_missing_cake(), START_KNIFE, None);
    let grab_at = [CAKE_SPAWN.x + 0.11, 0.035, CAKE_SPAWN.z].map(|m| (m / POSITION_UNIT_M).round() as i32);
    let hand = HandInput { grab_at, target_xz: [grab_at[0], grab_at[2]] };
    for _ in 0..60 {
        world.step(&Input { hand: Some(hand), ..Input::default() });
    }
    (world, hand)
}

#[test]
fn carried_cake_over_the_table_clears_it_just_barely() {
    let (world, _) = hold_cake_by_its_rim();
    let lowest_y_m = world.get_cake_positions().iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
    assert!((0.005..0.05).contains(&lowest_y_m), "{lowest_y_m}");
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
    let mut world = World::create(build_slice_missing_cake(), START_KNIFE, None);
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
