mod hand;
mod mass;
mod setup;
mod squash;

use std::sync::Arc;

use rapier3d::glamx::{DQuat, DVec3};
use rapier3d::prelude::*;

use crate::cakes::CakeModel;
use crate::cut::{Split, calc_split};
use crate::input::{Input, KnifeInput, calc_radians};
use crate::knife::{BLADE_BACK_M, Knife, PosedTree, Solid, Turn, calc_rest_m, calc_sweep_reach};
use crate::table::{CAKE_SPAWN, RESPAWN_BELOW_Y_M, STAND_RADIUS_M, TABLE_HALF_X_M, TABLE_HALF_Z_M};
use hand::Grip;
pub use setup::Cake;
use setup::{StandPlate, insert_cake, insert_knife, insert_stand, insert_table, insert_under_plate_fill};

pub const TICKS_PER_S: u32 = 60;
/// A competitive round's length, at whose last tick the cut happens for anyone who has not cut.
pub const ROUND_TICKS: u32 = 30 * TICKS_PER_S;
const DT_S: f64 = 1.0 / TICKS_PER_S as f64;
#[expect(clippy::cast_possible_truncation, reason = "Rapier steps in f32")]
const RAPIER_DT_S: f32 = DT_S as f32;
/// Rapier scales its slop, contact prediction, and sleep thresholds by this, so they suit
/// objects a decimeter across instead of a meter.
const LENGTH_UNIT_M: f32 = 0.1;
/// How long a cake that fell off the table keeps falling before it respawns.
const RESPAWN_DELAY_TICKS: u32 = 3 * TICKS_PER_S;
const STAND_SPEED_RAD_PER_S: f64 = core::f64::consts::TAU;
const FREE_LINEAR_DAMPING: f32 = 0.5;
const FREE_ANGULAR_DAMPING: f32 = 2.0;
const CAKE_FRICTION: f32 = 0.9;
/// Spin a free cake touching the table or stand loses per second, standing in for the rolling
/// resistance of a soft cake squashing at its contact. Unlike damping, which only slows a roll
/// gravity keeps driving, it stops an off-balance cake rocking or rolling on its rim outright,
/// while tipping over still outpaces it.
const ROLL_RESISTANCE_RAD_PER_S2: f32 = 80.0;
/// A free cake whose every point stays within this of where it was for at least three
/// [`STILL_QUARTER_TICKS`], moving no further in the last of them than in the one before, is put to
/// sleep. Rapier's own sleep needs it slower than 5 mm/s at its farthest point throughout, which a
/// cake rocking in place on a single contact, such as the stand's rim, never is. A cake tipping
/// over from near balance creeps at first, since the rolling resistance takes off each tick's small
/// spin, but speeds up as it leans, which the comparison tells apart from rocking. The first
/// quarter is never compared, since it holds the jolt of landing.
const STILL_DRIFT_M: f64 = 0.005;
const STILL_QUARTER_TICKS: u32 = TICKS_PER_S / 4;
/// A free cake slower than this, spinning no faster than the rolling resistance stops in a tick,
/// has stopped moving but for its creep, and rests if its squash holds it.
const SQUASH_HOLD_SPEED_M_PER_S: f32 = 0.05;
/// How far a free cake rising into the pressed edge may raise the knife in one tick. A cake that
/// would raise it further moved into the blade from the side, and the blade stops it instead.
const MAX_CAKE_PUSH_M: f64 = 0.005;
/// Across this gap between the cake's bounding box and the stand's rim, the resting knife turns
/// from waiting over the cake's middle height to following its height as posed. It reaches past
/// where the cake spawns, so the knife there already partly follows, and leaves the table's far
/// ends and corners away. No gap the table allows lets the slow float rise clear of a cake carried
/// in at its tallest at the hand's full speed: this one clears such a carry at up to about 0.7 m/s
/// from anywhere on the table.
const REST_APPROACH_M: f64 = 0.32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    Playing,
    Cut { tick: u32, split: Split },
}

/// One player's cake, hand, stand, and knife on the shared table, advanced one fixed tick per
/// input.
pub struct World {
    physics: PhysicsWorld,
    cake: RigidBodyHandle,
    hand: RigidBodyHandle,
    grip: Option<Grip>,
    stand_angle_rad: f64,
    /// Whether the knife stopped the stand short of its input last tick.
    is_stand_blocked: bool,
    stand_plate: StandPlate,
    knife_body: RigidBodyHandle,
    /// What only a held cake collides with: the knife's blade and handles, and the space under the
    /// stand's plate.
    held_colliders: Vec<ColliderHandle>,
    /// What this world shares with every other on the same cake.
    shared: Arc<Cake>,
    bounding_radius_m: f64,
    /// Halfway between how tall the cake stands in its flattest and its tallest pose, which the
    /// knife rests over while the cake is away from the stand.
    waiting_height_m: f64,
    cake_positions: Vec<DVec3>,
    /// The cake's pose as of `cake_positions`.
    cake_positions_pose: Pose,
    /// The least and greatest corners of `cake_positions`' bounding box.
    cake_bounds: (DVec3, DVec3),
    /// Ticks since the cake fell off the table.
    fallen_ticks: u32,
    /// Where the awake free cake has stayed within [`STILL_DRIFT_M`] of for `still_ticks`.
    still_pose: Pose,
    /// The cake's poses at the last two [`STILL_QUARTER_TICKS`] boundaries since `still_pose`.
    still_marks: [Pose; 2],
    still_ticks: u32,
    /// Whether the pressed blade stopped the cake, which then rests against it until the knife
    /// moves.
    is_cake_against_blade: bool,
    knife: Knife,
    tick: u32,
    round_ticks: Option<u32>,
    phase: Phase,
}

impl World {
    /// A world whose knife rests at `knife_tilt_rad` over the cake, and that cuts itself at
    /// `round_ticks`, or never with `None`. Passing a [`CakeModel`] builds its [`Cake`] for this
    /// world alone.
    pub fn create(shared: impl Into<Arc<Cake>>, knife_tilt_rad: f64, round_ticks: Option<u32>) -> Self {
        let shared = shared.into();
        let knife = Knife { tilt_rad: knife_tilt_rad, rest_m: 0.0, rest_lead_m: 0.0, lower_m: 0.0 };
        let mut physics = PhysicsWorld::new();
        physics.integration_parameters.dt = RAPIER_DT_S;
        physics.integration_parameters.length_unit = LENGTH_UNIT_M;
        insert_table(&mut physics);
        let stand = insert_stand(&mut physics);
        let cake = insert_cake(&mut physics, &shared);
        let hand = physics.insert_body(RigidBodyBuilder::kinematic_position_based());
        let (knife_body, mut held_colliders) = insert_knife(&mut physics, &knife);
        held_colliders.push(insert_under_plate_fill(&mut physics, stand));
        let com = physics.bodies[cake].mass_properties().local_mprops.local_com;
        let bounding_radius_m = f64::from(shared.get_model().mesh.calc_max_distance_from(com));
        let (flattest_m, tallest_m) = shared.width_range_m;
        let mut world = Self {
            physics,
            cake,
            hand,
            grip: None,
            stand_angle_rad: 0.0,
            is_stand_blocked: false,
            stand_plate: StandPlate::build(),
            knife_body,
            held_colliders,
            shared,
            bounding_radius_m,
            waiting_height_m: f64::midpoint(flattest_m, tallest_m),
            cake_positions: Vec::new(),
            cake_positions_pose: Pose::IDENTITY,
            cake_bounds: (DVec3::ZERO, DVec3::ZERO),
            fallen_ticks: 0,
            still_pose: Pose::IDENTITY,
            still_marks: [Pose::IDENTITY; 2],
            still_ticks: 0,
            is_cake_against_blade: false,
            knife,
            tick: 0,
            round_ticks,
            phase: Phase::Playing,
        };
        world.update_cake_positions();
        let rest_m = calc_rest_m(world.calc_rest_height_m());
        world.knife = Knife { rest_m, rest_lead_m: rest_m, lower_m: rest_m, ..world.knife };
        world
    }

    pub fn step(&mut self, input: &Input) {
        if self.phase != Phase::Playing {
            return;
        }
        self.step_hand(input.hand);
        let is_carried = self.step_stand(input.stand_angle);
        self.step_knife(input.knife);
        self.resist_rolling();
        self.update_squash_hold(is_carried);
        let before_step = *self.physics.bodies[self.cake].position();
        self.physics.step();
        self.update_cake_positions();
        self.ensure_cake_outside_blade(before_step);
        self.ensure_still_cake_sleeps();
        self.ensure_cake_on_table();
        self.tick += 1;
        if self.round_ticks.is_some_and(|round_ticks| self.tick >= round_ticks) {
            self.cut();
        }
    }

    /// Freezes the world and cuts along the knife's plane as it is now, leaving the whole cake
    /// above a cut that misses. Later calls return the first cut.
    pub fn cut(&mut self) -> Split {
        if let Phase::Cut { split, .. } = self.phase {
            return split;
        }
        let triangles = &self.shared.get_model().mesh.triangles;
        let mut split = calc_split(&self.cake_positions, triangles, &self.knife.calc_plane());
        if !self.is_cut_through() {
            split = Split { above_m3: split.above_m3 + split.below_m3, below_m3: 0.0 };
        }
        self.phase = Phase::Cut { tick: self.tick, split };
        split
    }

    /// Whether a cut now would divide the cake, rather than miss it, stop on the stand above it,
    /// or end beside it.
    pub fn is_cut_through(&self) -> bool {
        self.knife.is_cut_through(self.as_cake_solid(), self.calc_cut_lower_m())
    }

    /// How far the knife would lower through the cake to the stand or table, for drawing the cut.
    pub fn calc_cut_lower_m(&self) -> f64 {
        self.knife.calc_lower_limit_m(&[self.stand_plate.as_solid()])
    }

    /// How far the knife lowers before its edge meets the cake, for keeping the halves together
    /// until the blade enters it.
    pub fn calc_cake_touch_lower_m(&self) -> f64 {
        self.knife.calc_lower_limit_m(&[self.as_cake_solid()])
    }

    pub const fn get_phase(&self) -> Phase {
        self.phase
    }

    pub const fn get_tick(&self) -> u32 {
        self.tick
    }

    pub const fn get_knife(&self) -> &Knife {
        &self.knife
    }

    pub fn get_model(&self) -> &CakeModel {
        self.shared.get_model()
    }

    pub fn get_cake_pose(&self) -> Pose {
        *self.physics.bodies[self.cake].position()
    }

    pub const fn get_stand_angle_rad(&self) -> f64 {
        self.stand_angle_rad
    }

    /// Whether the knife stopped the stand short of its input last tick, for the client to stop
    /// the drag there too.
    pub const fn is_stand_blocked(&self) -> bool {
        self.is_stand_blocked
    }

    pub const fn is_holding(&self) -> bool {
        self.grip.is_some()
    }

    /// The cake's mesh vertices in world coordinates as of the last tick.
    pub fn get_cake_positions(&self) -> &[DVec3] {
        &self.cake_positions
    }

    /// FNV-1a over the state that later ticks depend on, for comparing two simulations. State
    /// copied from earlier ticks' hashed state, such as the pose a still cake is measured from, is
    /// left out, since a divergence there showed in the hash when it was copied.
    pub fn calc_hash(&self) -> u64 {
        let cake = &self.physics.bodies[self.cake];
        let hand = &self.physics.bodies[self.hand];
        let mut hash = Fnv1a::new();
        hash.write(&self.tick.to_le_bytes());
        let pose = cake.position();
        for value in pose.translation.to_array().into_iter().chain(pose.rotation.to_array()) {
            hash.write(&value.to_bits().to_le_bytes());
        }
        for value in cake.linvel().to_array().into_iter().chain(cake.angvel().to_array()) {
            hash.write(&value.to_bits().to_le_bytes());
        }
        hash.write(&cake.gravity_scale().to_bits().to_le_bytes());
        for value in hand.translation().to_array() {
            hash.write(&value.to_bits().to_le_bytes());
        }
        let knife = &self.knife;
        for value in [knife.tilt_rad, knife.rest_m, knife.rest_lead_m, knife.lower_m, self.stand_angle_rad] {
            hash.write(&value.to_bits().to_le_bytes());
        }
        hash.write(&self.fallen_ticks.to_le_bytes());
        hash.write(&self.still_ticks.to_le_bytes());
        hash.write(&[u8::from(self.is_cake_against_blade)]);
        Grip::write_hash(self.grip.as_ref(), &mut hash);
        hash.finish()
    }

    /// Slows a free cake's spin while it touches the table or stand, neither of which spins in the
    /// physics.
    fn resist_rolling(&mut self) {
        if self.grip.is_some() || !self.is_cake_supported() {
            return;
        }
        let cake = &mut self.physics.bodies[self.cake];
        let spin = cake.angvel();
        let spin_len = spin.length();
        if spin_len == 0.0 {
            return;
        }
        let slowed_len = (spin_len - ROLL_RESISTANCE_RAD_PER_S2 * RAPIER_DT_S).max(0.0);
        cake.set_angvel(spin * (slowed_len / spin_len), false);
    }

    /// Puts a free cake its squash holds to sleep with gravity off, and gives a cake it no longer
    /// holds its gravity back. Gravity stays off because the stand's carry wakes the cake in the
    /// step, and gravity would then rock it off a pose only the squash holds. A sleeping cake the
    /// stand did not carry cannot have moved, and a held cake still slow moved only by the stand's
    /// turn about the vertical, which leaves its squash as it was, so both keep their hold unchecked.
    #[expect(clippy::float_cmp, reason = "the scale is only ever set to exactly 0 or 1")]
    fn update_squash_hold(&mut self, is_carried: bool) {
        let cake = &self.physics.bodies[self.cake];
        if cake.is_sleeping() && !is_carried {
            return;
        }
        let was_held = cake.gravity_scale() == 0.0;
        let is_held = self.grip.is_none() && (was_held && self.is_cake_slow() || self.is_held_by_squash());
        let cake = &mut self.physics.bodies[self.cake];
        let gravity_scale = if is_held { 0.0 } else { 1.0 };
        if cake.gravity_scale() != gravity_scale {
            cake.set_gravity_scale(gravity_scale, false);
        }
        if is_held {
            cake.sleep();
        }
    }

    /// Puts the free cake to sleep once it has stayed in place and stopped speeding up, as
    /// [`STILL_DRIFT_M`] describes, though it may still rock or creep there too slightly to leave it.
    fn ensure_still_cake_sleeps(&mut self) {
        let cake = &self.physics.bodies[self.cake];
        let pose = *cake.position();
        if self.grip.is_some() || cake.is_sleeping() || self.calc_drift_m(self.still_pose, pose) > STILL_DRIFT_M {
            self.still_pose = pose;
            self.still_marks = [pose; 2];
            self.still_ticks = 0;
            return;
        }
        self.still_ticks += 1;
        if !self.still_ticks.is_multiple_of(STILL_QUARTER_TICKS) {
            return;
        }
        let [earlier, later] = self.still_marks;
        let is_slowing = self.calc_drift_m(later, pose) <= self.calc_drift_m(earlier, later);
        if self.still_ticks >= 3 * STILL_QUARTER_TICKS && is_slowing {
            self.physics.bodies[self.cake].sleep();
            return;
        }
        self.still_marks = [later, pose];
    }

    /// Whether the slow cake rests on the plate or the table with its center of mass over the
    /// patch it squashes onto there, as the `squash` module describes.
    fn is_held_by_squash(&self) -> bool {
        if !self.is_cake_slow() || !self.is_cake_supported() {
            return false;
        }
        let cake = &self.physics.bodies[self.cake];
        let pose = cake.position();
        let com = pose.transform_point(cake.mass_properties().local_mprops.local_com).as_dvec3();
        let (rotation, translation) = (pose.rotation.as_dquat(), pose.translation.as_dvec3());
        let is_on_stand = self.is_on_stand();
        self.shared.hull_frame.is_over_squashed_patch(rotation, translation, com, |p| {
            if is_on_stand {
                return p.length() <= STAND_RADIUS_M;
            }
            p.x.abs() <= TABLE_HALF_X_M && p.y.abs() <= TABLE_HALF_Z_M
        })
    }

    /// Whether the cake spins no faster than the rolling resistance stops in a tick and moves slower
    /// than [`SQUASH_HOLD_SPEED_M_PER_S`].
    fn is_cake_slow(&self) -> bool {
        let cake = &self.physics.bodies[self.cake];
        cake.angvel().length() <= ROLL_RESISTANCE_RAD_PER_S2 * RAPIER_DT_S
            && cake.linvel().length() <= SQUASH_HOLD_SPEED_M_PER_S
    }

    /// At most how far any point of the cake moves from pose `a` to `b`.
    fn calc_drift_m(&self, a: Pose, b: Pose) -> f64 {
        let com = self.physics.bodies[self.cake].mass_properties().local_mprops.local_com;
        let shift_m = f64::from(a.transform_point(com).distance(b.transform_point(com)));
        let turn = b.rotation * a.rotation.inverse();
        // A unit quaternion's `xyz` is as long as the sine of half its turn, so this is the chord
        // the farthest point sweeps.
        let chord_m = 2.0 * f64::from(turn.xyz().length()) * self.bounding_radius_m;
        shift_m + chord_m
    }

    /// Whether the cake touches anything other than the knife, as of the last tick.
    fn is_cake_supported(&self) -> bool {
        self.physics.bodies[self.cake].colliders().iter().any(|&collider| {
            self.physics.contact_pairs_with(collider).any(|pair| {
                let other = if pair.collider1 == collider { pair.collider2 } else { pair.collider1 };
                pair.has_any_active_contact() && !self.held_colliders.contains(&other)
            })
        })
    }

    /// Turns the stand toward `stand_angle`, returning whether it carried the cake round.
    #[expect(clippy::float_cmp, reason = "a sweep that stopped short returns any other angle")]
    fn step_stand(&mut self, stand_angle: i32) -> bool {
        let from_rad = self.stand_angle_rad;
        let max_step_rad = STAND_SPEED_RAD_PER_S * DT_S;
        let step_rad = (calc_radians(stand_angle) - from_rad).clamp(-max_step_rad, max_step_rad);
        let wanted_rad = from_rad + step_rad;
        self.stand_angle_rad = self.calc_stand_reach_rad(wanted_rad);
        self.is_stand_blocked = self.stand_angle_rad != wanted_rad;
        if self.is_stand_blocked {
            // The knife holds the cake, which would otherwise spin on into the blade.
            let cake = &mut self.physics.bodies[self.cake];
            cake.set_linvel(Vector::ZERO, false);
            cake.set_angvel(Vector::ZERO, false);
        }
        let turn_rad = self.stand_angle_rad - from_rad;
        let is_carried = turn_rad != 0.0 && self.is_riding_stand();
        if is_carried {
            self.carry_cake_round(turn_rad);
        }
        is_carried
    }

    /// Whether the cake is free and sits on the stand, so that it turns with it.
    fn is_riding_stand(&self) -> bool {
        self.grip.is_none() && self.is_on_stand()
    }

    /// Turns the cake riding the stand with it by `turn_rad`, keeping its motion relative to the
    /// stand. A turning plate would carry it by friction alone, which Rapier's contacts let slip
    /// outward every tick, since they match velocities, not positions.
    fn carry_cake_round(&mut self, turn_rad: f64) {
        let half_rad = turn_rad / 2.0;
        let turn = DQuat::from_xyzw(0.0, libm::sin(half_rad), 0.0, libm::cos(half_rad)).as_quat();
        let cake = &mut self.physics.bodies[self.cake];
        let pose = cake.position();
        let turned = Pose::from_parts(turn * pose.translation, (turn * pose.rotation).normalize());
        let (linvel, angvel) = (turn * cake.linvel(), turn * cake.angvel());
        cake.set_position(turned, false);
        cake.set_linvel(linvel, false);
        cake.set_angvel(angvel, false);
    }

    /// How far the stand turns toward `to_rad` before the cake riding it would meet the lowered
    /// knife.
    fn calc_stand_reach_rad(&self, to_rad: f64) -> f64 {
        // A cake turning on the stand keeps its height, so it stays clear of the resting knife.
        if !self.is_riding_stand() || !self.knife.is_lowered() {
            return to_rad;
        }
        let from_rad = self.stand_angle_rad;
        let (cake, plate) = (self.as_cake_solid(), self.stand_plate.as_solid());
        calc_sweep_reach(from_rad, to_rad, |rad| {
            let turned = Solid { turn: Some(Turn::calc(rad - from_rad)), ..cake };
            self.knife.is_clear_of(&[turned, plate])
        })
    }

    /// The cake as it is posed now, for the knife to stop on.
    fn as_cake_solid(&self) -> Solid<'_> {
        let pose = self.cake_positions_pose;
        let (rotation, translation) = (pose.rotation.as_dquat(), pose.translation.as_dvec3());
        let tree = PosedTree { tree: &self.shared.tree, rotation, translation };
        Solid { tree: Some(tree), ..Solid::new(&self.cake_positions, &self.shared.get_model().mesh.triangles) }
    }

    fn step_knife(&mut self, input: KnifeInput) {
        let mut knife = self.knife;
        knife.float_over(self.calc_rest_height_m(), DT_S);
        let (cake, plate) = (self.as_cake_solid(), self.stand_plate.as_solid());
        // A held cake meets the knife's collider instead, so the blade holds its place against it.
        let solids: &[Solid] = if self.grip.is_some() { &[plate] } else { &[cake, plate] };
        knife.step(&input, solids);
        let is_moved = (knife.tilt_rad, knife.lower_m) != (self.knife.tilt_rad, self.knife.lower_m);
        if self.is_cake_against_blade && is_moved {
            self.is_cake_against_blade = false;
            self.physics.bodies[self.cake].wake_up(true);
        }
        self.knife = knife;
        self.physics.bodies[self.knife_body].set_next_kinematic_position(self.knife.calc_pose());
    }

    /// The cake height the knife rests over: the cake's as posed at the stand, easing to its
    /// waiting height [`REST_APPROACH_M`] away.
    fn calc_rest_height_m(&self) -> f64 {
        let nearness = self.calc_stand_nearness(REST_APPROACH_M);
        self.waiting_height_m + (self.calc_cake_height_m() - self.waiting_height_m) * nearness
    }

    /// How tall the cake stands as it is posed now.
    fn calc_cake_height_m(&self) -> f64 {
        let (min, max) = self.cake_bounds;
        max.y - min.y
    }

    /// Puts a free cake that moved into the pressed blade this tick back where it was before the
    /// step and stops it, as the blade would: the lowering limit sees only the blade's plane, so it
    /// would jump the knife onto the cake. One crossing the plane above the blade's back passes over
    /// it.
    fn ensure_cake_outside_blade(&mut self, before_step: Pose) {
        if self.grip.is_some() || !self.knife.is_lowered() {
            return;
        }
        let jump_m = self.knife.lower_m - self.calc_cake_touch_lower_m();
        if !(MAX_CAKE_PUSH_M..BLADE_BACK_M).contains(&jump_m) {
            return;
        }
        let cake = &mut self.physics.bodies[self.cake];
        cake.set_position(before_step, false);
        cake.set_linvel(Vector::ZERO, false);
        cake.set_angvel(Vector::ZERO, false);
        self.is_cake_against_blade = true;
        self.update_cake_positions();
    }

    /// Respawns a cake that fell off the table, once it has fallen for a while.
    fn ensure_cake_on_table(&mut self) {
        if f64::from(self.physics.bodies[self.cake].translation().y) >= RESPAWN_BELOW_Y_M {
            return;
        }
        self.fallen_ticks += 1;
        if self.fallen_ticks < RESPAWN_DELAY_TICKS {
            return;
        }
        self.fallen_ticks = 0;
        self.release();
        let cake = &mut self.physics.bodies[self.cake];
        cake.set_position(Pose::from_translation(CAKE_SPAWN.as_vec3()), true);
        cake.set_linvel(Vector::ZERO, true);
        cake.set_angvel(Vector::ZERO, true);
        self.update_cake_positions();
    }

    /// Turns the mesh into `cake_positions` and its bounding box, unless the cake has not moved, as
    /// it never does asleep, where a fresh copy would match the last to the bit.
    fn update_cake_positions(&mut self) {
        let pose = *self.physics.bodies[self.cake].position();
        if pose == self.cake_positions_pose && !self.cake_positions.is_empty() {
            return;
        }
        let rotation = pose.rotation.as_dquat();
        let translation = pose.translation.as_dvec3();
        self.cake_positions_pose = pose;
        self.cake_positions.clear();
        let positions = &self.shared.get_model().mesh.positions;
        self.cake_positions.extend(positions.iter().map(|p| rotation * p.as_dvec3() + translation));
        self.cake_bounds = self
            .cake_positions
            .iter()
            .fold((DVec3::INFINITY, DVec3::NEG_INFINITY), |(min, max), p| (min.min(*p), max.max(*p)));
    }
}

struct Fnv1a(u64);

impl Fnv1a {
    const fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }

    const fn finish(&self) -> u64 {
        self.0
    }
}
