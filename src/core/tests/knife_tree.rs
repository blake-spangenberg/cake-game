//! The cake's triangle tree only skips work: on real cakes in many poses, the knife lowers,
//! tilts, rides, stops, and cuts exactly as it does checking every triangle, and the cake's width
//! range matches checking every vertex, to the bit.

use std::io::Read;
use std::path::Path;

use cake_core::cakes::{CakeModel, build_slice_missing_cake};
use cake_core::input::{KnifeInput, quantize_meters, quantize_radians};
use cake_core::knife::{Knife, MAX_TILT_RAD, PosedTree, Solid, Turn};
use cake_core::table::STAND_TOP_Y_M;
use cake_core::triangle_tree::TriangleTree;
use rapier3d::glamx::{DQuat, DVec3};
use rapier3d::math::{Rotation, Vec3};

/// The Yellow Sneaker and the Garden Scarecrow, two of the largest cakes at about 40k triangles,
/// and the Leather Sofa, whose broad flat faces lay many triangles in one plane.
const CAKE_IDS: [&str; 3] =
    ["89b7408463b144d0bd157cc788dd496d", "b400281a14bf4f3a87bc6a053a19581a", "ccc3f77d27f94ae5acc77992b90b146b"];
const POSE_LEN: u32 = 24;
/// Every this many poses also steps a pressed knife, which costs many scans of every triangle.
const STEPPED_POSE_INTERVAL: u32 = 8;
const STEP_LEN: u32 = 3;
/// Tilted no further, the knife's plane crosses most cakes on the stand.
const MAX_AIMED_TILT_RAD: f64 = 0.35;
/// Far enough for a turn to sweep several of the stand's probes.
const MAX_STAND_TURN_RAD: f64 = 0.1;
const PRESSED_KNIFE: Knife = Knife { tilt_rad: 0.0, rest_m: 0.0, rest_lead_m: 0.0, lower_m: 0.0 };

#[test]
fn knife_stops_and_cuts_on_real_cakes_exactly_as_checking_every_triangle_does() {
    let mut random = SplitMix64(0x5eed);
    let (mut hit_count, mut cut_count) = (0, 0);
    for id in CAKE_IDS {
        let model = fetch_cake(id);
        let triangles = &model.mesh.triangles;
        let tree = TriangleTree::build(&model.mesh.positions, triangles);
        for pose_idx in 0..POSE_LEN {
            let cake = PosedCake::place(&model, &mut random);
            let posed_tree = PosedTree { tree: &tree, rotation: cake.rotation, translation: cake.translation };
            let brute = Solid::new(&cake.positions, triangles);
            let fast = Solid { tree: Some(posed_tree), ..brute };
            let knife = Knife { tilt_rad: random.pick_in(-MAX_AIMED_TILT_RAD, MAX_AIMED_TILT_RAD), ..PRESSED_KNIFE };
            let limit_m = knife.calc_lower_limit_m(&[brute]);
            assert_eq!(knife.calc_lower_limit_m(&[fast]), limit_m, "{id} pose {pose_idx}: {knife:?}");
            hit_count += (limit_m < knife.calc_lower_limit_m(&[])) as u32;

            let stop_m = limit_m + random.pick_in(-0.03, 0.03);
            let is_cut = knife.is_cut_through(brute, stop_m);
            assert_eq!(knife.is_cut_through(fast, stop_m), is_cut, "{id} pose {pose_idx}: {knife:?} to {stop_m}");
            cut_count += is_cut as u32;

            let turn_rad = random.pick_in(-MAX_STAND_TURN_RAD, MAX_STAND_TURN_RAD);
            let turned_positions = calc_turned_as_the_stand_sweep_did(&cake.positions, turn_rad);
            let turned_brute = Solid::new(&turned_positions, triangles);
            let turned_fast = Solid { turn: Some(Turn::calc(turn_rad)), ..fast };
            let turned_limit_m = knife.calc_lower_limit_m(&[turned_brute]);
            let fast_turned_limit_m = knife.calc_lower_limit_m(&[turned_fast]);
            assert_eq!(fast_turned_limit_m, turned_limit_m, "{id} pose {pose_idx} turn {turn_rad}");

            if pose_idx % STEPPED_POSE_INTERVAL == 0 {
                assert_presses_alike(knife, limit_m, &mut random, brute, fast);
                assert_presses_alike(knife, turned_limit_m, &mut random, turned_brute, turned_fast);
            }
        }
    }
    let sample_count = CAKE_IDS.len() as u32 * POSE_LEN;
    let miss_count = sample_count - cut_count;
    assert!(hit_count * 2 > sample_count, "only {hit_count} of {sample_count} planes met a cake");
    assert!(cut_count * 4 > sample_count && miss_count * 4 > sample_count, "{cut_count} cuts and {miss_count} misses");
}

#[test]
fn width_range_on_real_cakes_matches_checking_every_vertex() {
    for id in CAKE_IDS {
        let mesh = fetch_cake(id).mesh;
        let tree = TriangleTree::build(&mesh.positions, &mesh.triangles);
        assert_eq!(mesh.calc_width_range_m(Some(&tree)), mesh.calc_width_range_m(None), "{id}");
    }
}

#[test]
fn width_range_counts_a_vertex_no_triangle_uses() {
    let mut mesh = build_slice_missing_cake().mesh;
    mesh.positions.push(Vec3::new(0.0, 1.0, 0.0));
    let tree = TriangleTree::build(&mesh.positions, &mesh.triangles);
    let (_, tallest_m) = mesh.calc_width_range_m(Some(&tree));
    assert!(tallest_m >= 1.0, "{tallest_m}");
    assert_eq!(mesh.calc_width_range_m(Some(&tree)), mesh.calc_width_range_m(None));
}

/// Presses `knife` a centimeter past where it meets the cake at `limit_m`, then tilts it a few
/// probes' worth each step, so it lowers, rides, and stops against the cake's surface.
fn assert_presses_alike(knife: Knife, limit_m: f64, random: &mut SplitMix64, brute: Solid, fast: Solid) {
    let lower = quantize_meters(limit_m + 0.01);
    let (mut brute_knife, mut fast_knife) = (knife, knife);
    let mut tilt_rad = knife.tilt_rad;
    for step_idx in 0..STEP_LEN {
        let input = KnifeInput { tilt: quantize_radians(tilt_rad), lower };
        brute_knife.step(&input, &[brute]);
        fast_knife.step(&input, &[fast]);
        assert_eq!(fast_knife, brute_knife, "step {step_idx} from {knife:?} with {input:?}");
        tilt_rad = (tilt_rad + random.pick_in(-0.05, 0.05)).clamp(-MAX_TILT_RAD, MAX_TILT_RAD);
    }
}

/// The cake's vertices placed as the world places them from its body's pose.
struct PosedCake {
    positions: Vec<DVec3>,
    rotation: DQuat,
    translation: DVec3,
}

impl PosedCake {
    /// Turned any way and lying about the stand's middle, under the pivot.
    fn place(model: &CakeModel, random: &mut SplitMix64) -> PosedCake {
        let axis = Vec3::new(random.pick_in(-1.0, 1.0) as f32, random.pick_in(-1.0, 1.0) as f32, 0.5).normalize();
        let pose_rotation = Rotation::from_axis_angle(axis, random.pick_in(-3.0, 3.0) as f32);
        let pose_translation = Vec3::new(
            random.pick_in(-0.03, 0.03) as f32,
            (STAND_TOP_Y_M + random.pick_in(0.0, 0.08)) as f32,
            random.pick_in(-0.03, 0.03) as f32,
        );
        let (rotation, translation) = (pose_rotation.as_dquat(), pose_translation.as_dvec3());
        let positions = model.mesh.positions.iter().map(|p| rotation * p.as_dvec3() + translation).collect();
        PosedCake { positions, rotation, translation }
    }
}

/// The vertices turned about the vertical axis by `turn_rad` with the arithmetic the stand's
/// sweep used before the triangle tree, one turned copy of the cake per probe.
fn calc_turned_as_the_stand_sweep_did(positions: &[DVec3], turn_rad: f64) -> Vec<DVec3> {
    let (sin, cos) = (libm::sin(turn_rad), libm::cos(turn_rad));
    positions.iter().map(|p| DVec3::new(p.x * cos + p.z * sin, p.y, p.z * cos - p.x * sin)).collect()
}

fn fetch_cake(id: &str) -> CakeModel {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../assets/cakes/{id}.cake.br"));
    let packed = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let mut bytes = Vec::new();
    brotli_decompressor::Decompressor::new(packed.as_slice(), 4096)
        .read_to_end(&mut bytes)
        .unwrap_or_else(|error| panic!("cannot unpack {}: {error}", path.display()));
    CakeModel::decode(&bytes).unwrap_or_else(|error| panic!("{} is not a playable cake: {error:?}", path.display()))
}

/// A seeded generator, so every run tries the same poses.
struct SplitMix64(u64);

impl SplitMix64 {
    fn pick_in(&mut self, low: f64, high: f64) -> f64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        low + (high - low) * (z >> 11) as f64 / (1u64 << 53) as f64
    }
}
