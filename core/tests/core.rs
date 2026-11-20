use cake_core::cakes::build_slice_missing_cake;
use cake_core::cut::{Plane, calc_split};
use cake_core::input::{HandInput, Input, InputError, KnifeInput};
use cake_core::knife::{Knife, START_KNIFE};
use cake_core::mesh::{Mesh, MeshError};
use cake_core::replay::{build_scripted_inputs, replay};
use cake_core::sim::{Phase, ROUND_TICKS, World};
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

#[test]
fn upright_knife_over_an_empty_table_lowers_to_the_table() {
    let knife = Knife::default();
    assert!((knife.calc_lower_limit_m(&[], &[]) - 0.2).abs() < 1e-12);
}

#[test]
fn upright_knife_stops_on_a_box_cake_under_its_edge() {
    let cake = build_box([-0.05, 0.0, -0.05], [0.05, 0.1, 0.05]);
    let limit_m = Knife::default().calc_lower_limit_m(&to_dvec3s(&cake), &cake.triangles);
    assert!((limit_m - 0.1).abs() < 1e-7, "{limit_m}");
}

#[test]
fn knife_ignores_a_cake_beyond_its_blade_end() {
    let cake = build_box([0.3, 0.0, -0.05], [0.4, 0.15, 0.05]);
    let limit_m = Knife::default().calc_lower_limit_m(&to_dvec3s(&cake), &cake.triangles);
    assert!((limit_m - 0.2).abs() < 1e-12, "{limit_m}");
}

#[test]
fn knife_stops_on_a_cake_reaching_under_its_blade_end() {
    let cake = build_box([0.2, 0.0, -0.05], [0.4, 0.15, 0.05]);
    let limit_m = Knife::default().calc_lower_limit_m(&to_dvec3s(&cake), &cake.triangles);
    assert!((limit_m - 0.05).abs() < 1e-7, "{limit_m}");
}

#[test]
fn input_survives_encoding() {
    let input = Input {
        hand: Some(HandInput { grab_at: [1, -2, 3], target_xz: [-4, 5] }),
        knife: KnifeInput { tilt: -600, heading: 7_000, is_lowering: true },
    };
    assert_eq!(Input::decode(&input.encode()), Ok(input));
}

#[test]
fn input_of_the_wrong_length_is_rejected() {
    assert_eq!(Input::decode(&[0; 7]), Err(InputError::Length { len: 7 }));
}

#[test]
fn input_with_unknown_flags_is_rejected() {
    assert_eq!(Input::decode(&[4, 0, 0, 0, 0, 0, 0, 0]), Err(InputError::UnknownFlags { flags: 4 }));
}

#[test]
fn untouched_round_cuts_itself_at_the_last_tick() {
    let mut world = World::create(build_slice_missing_cake(), START_KNIFE);
    for _ in 0..ROUND_TICKS + 5 {
        world.step(&Input::default());
    }
    assert!(matches!(world.get_phase(), Phase::Cut { tick: ROUND_TICKS, .. }));
    assert_eq!(world.get_tick(), ROUND_TICKS);
}

#[test]
fn carried_cake_lands_near_the_target() {
    let mut world = World::create(build_slice_missing_cake(), START_KNIFE);
    for input in &build_scripted_inputs()[..300] {
        world.step(input);
    }
    let pose = world.get_cake_pose();
    assert!(pose.translation.x.abs() < 0.05, "{pose:?}");
    assert!((pose.translation.z - 0.02).abs() < 0.05, "{pose:?}");
}

#[test]
fn grab_away_from_the_cake_is_ignored() {
    let mut world = World::create(build_slice_missing_cake(), START_KNIFE);
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
