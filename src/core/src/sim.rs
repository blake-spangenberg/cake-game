mod hand;
mod setup;

use rapier3d::glamx::DVec3;
use rapier3d::prelude::*;

use crate::cakes::CakeModel;
use crate::cut::{Split, calc_split};
use crate::input::{Input, KnifeInput, calc_radians};
use crate::knife::{Knife, Solid, calc_rest_m, calc_sweep_reach};
use crate::table::{CAKE_SPAWN, RESPAWN_BELOW_Y_M};
use hand::Grip;
use setup::{StandPlate, build_cake_shape, insert_cake, insert_knife, insert_stand, insert_table};

pub const TICKS_PER_S: u32 = 60;
/// A competitive round's length, at whose last tick the cut happens for anyone who has not cut.
pub const ROUND_TICKS: u32 = 30 * TICKS_PER_S;
const DT_S: f64 = 1.0 / TICKS_PER_S as f64;
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
/// Across this gap between the cake's bounding box and the stand's rim, the resting knife turns
/// from waiting over the cake's middle height to following its height as posed. It reaches past
/// where the cake spawns, so the knife there already partly follows, and leaves only the table's
/// far ends and corners away. No gap the table allows lets the slow float rise clear of a cake
/// carried in at its tallest at the hand's full speed. This one clears such a carry at up to
/// about 0.5 m/s from anywhere on the table, and at 1 m/s from where the cake spawns, which at
/// 0.35 m meets the blade.
const REST_APPROACH_M: f64 = 0.4;

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
    /// The cake's hulls as one shape, for sweeping it along the hand's moves.
    cake_shape: SharedShape,
    hand: RigidBodyHandle,
    grip: Option<Grip>,
    stand: RigidBodyHandle,
    stand_angle_rad: f64,
    /// Whether the knife stopped the stand short of its input last tick.
    is_stand_blocked: bool,
    stand_plate: StandPlate,
    knife_body: RigidBodyHandle,
    knife_colliders: Vec<ColliderHandle>,
    model: CakeModel,
    bounding_radius_m: f64,
    /// Halfway between how tall the cake stands in its flattest and its tallest pose, which the
    /// knife rests over while the cake is away from the stand.
    waiting_height_m: f64,
    cake_positions: Vec<DVec3>,
    /// Ticks since the cake fell off the table.
    fallen_ticks: u32,
    knife: Knife,
    tick: u32,
    round_ticks: Option<u32>,
    phase: Phase,
}

impl World {
    /// A world whose knife rests at `knife_tilt_rad` over the cake, and that cuts itself at
    /// `round_ticks`, or never with `None`.
    pub fn create(model: CakeModel, knife_tilt_rad: f64, round_ticks: Option<u32>) -> World {
        let knife = Knife { tilt_rad: knife_tilt_rad, rest_m: 0.0, rest_lead_m: 0.0, lower_m: 0.0 };
        let mut physics = PhysicsWorld::new();
        physics.integration_parameters.dt = DT_S as f32;
        physics.integration_parameters.length_unit = LENGTH_UNIT_M;
        insert_table(&mut physics);
        let stand = insert_stand(&mut physics);
        let cake = insert_cake(&mut physics, &model);
        let hand = physics.insert_body(RigidBodyBuilder::kinematic_position_based());
        let (knife_body, knife_colliders) = insert_knife(&mut physics, &knife);
        let com = physics.bodies[cake].mass_properties().local_mprops.local_com;
        let bounding_radius_m = model.mesh.calc_max_distance_from(com) as f64;
        let (flattest_m, tallest_m) = model.mesh.calc_width_range_m();
        let mut world = World {
            physics,
            cake,
            cake_shape: build_cake_shape(&model),
            hand,
            grip: None,
            stand,
            stand_angle_rad: 0.0,
            is_stand_blocked: false,
            stand_plate: StandPlate::build(),
            knife_body,
            knife_colliders,
            model,
            bounding_radius_m,
            waiting_height_m: (flattest_m + tallest_m) / 2.0,
            cake_positions: Vec::new(),
            fallen_ticks: 0,
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
        self.step_stand(input.stand_angle);
        self.step_knife(&input.knife);
        self.resist_rolling();
        self.physics.step();
        self.update_cake_positions();
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
        let mut split = calc_split(&self.cake_positions, &self.model.mesh.triangles, &self.knife.calc_plane());
        if !self.is_cut_through() {
            split = Split { above_m3: split.above_m3 + split.below_m3, below_m3: 0.0 };
        }
        self.phase = Phase::Cut { tick: self.tick, split };
        split
    }

    /// Whether a cut now would divide the cake, rather than miss it, stop on the stand above it,
    /// or end beside it.
    pub fn is_cut_through(&self) -> bool {
        let cake = Solid { positions: &self.cake_positions, triangles: &self.model.mesh.triangles };
        self.knife.is_cut_through(cake, self.calc_cut_lower_m())
    }

    /// How far the knife would lower through the cake to the stand or table, for drawing the cut.
    pub fn calc_cut_lower_m(&self) -> f64 {
        self.knife.calc_lower_limit_m(&[self.stand_plate.as_solid()])
    }

    /// How far the knife lowers before its edge meets the cake, for keeping the halves together
    /// until the blade enters it.
    pub fn calc_cake_touch_lower_m(&self) -> f64 {
        let cake = Solid { positions: &self.cake_positions, triangles: &self.model.mesh.triangles };
        self.knife.calc_lower_limit_m(&[cake])
    }

    pub fn get_phase(&self) -> Phase {
        self.phase
    }

    pub fn get_tick(&self) -> u32 {
        self.tick
    }

    pub fn get_knife(&self) -> &Knife {
        &self.knife
    }

    pub fn get_model(&self) -> &CakeModel {
        &self.model
    }

    pub fn get_cake_pose(&self) -> Pose {
        *self.physics.bodies[self.cake].position()
    }

    pub fn get_stand_angle_rad(&self) -> f64 {
        self.stand_angle_rad
    }

    /// Whether the knife stopped the stand short of its input last tick, for the client to stop
    /// the drag there too.
    pub fn is_stand_blocked(&self) -> bool {
        self.is_stand_blocked
    }

    pub fn is_holding(&self) -> bool {
        self.grip.is_some()
    }

    /// The cake's mesh vertices in world coordinates as of the last tick.
    pub fn get_cake_positions(&self) -> &[DVec3] {
        &self.cake_positions
    }

    /// FNV-1a over the state that later ticks depend on, for comparing two simulations.
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
        for value in hand.translation().to_array() {
            hash.write(&value.to_bits().to_le_bytes());
        }
        let knife = &self.knife;
        for value in [knife.tilt_rad, knife.rest_m, knife.rest_lead_m, knife.lower_m, self.stand_angle_rad] {
            hash.write(&value.to_bits().to_le_bytes());
        }
        hash.write(&self.fallen_ticks.to_le_bytes());
        Grip::write_hash(self.grip.as_ref(), &mut hash);
        hash.finish()
    }

    /// Slows a free cake's spin relative to the table or stand it rests on, so the turning
    /// stand still carries it round.
    fn resist_rolling(&mut self) {
        if self.grip.is_some() {
            return;
        }
        let Some(support) = self.find_cake_support() else {
            return;
        };
        let support_spin = self.physics.bodies[support].angvel();
        let cake = &mut self.physics.bodies[self.cake];
        let spin = cake.angvel() - support_spin;
        let spin_len = spin.length();
        if spin_len == 0.0 {
            return;
        }
        let slowed_len = (spin_len - ROLL_RESISTANCE_RAD_PER_S2 * DT_S as f32).max(0.0);
        cake.set_angvel(support_spin + spin * (slowed_len / spin_len), false);
    }

    /// The body the cake touches other than the knife, as of the last tick, if any.
    fn find_cake_support(&self) -> Option<RigidBodyHandle> {
        let colliders = &self.physics.colliders;
        self.physics.bodies[self.cake].colliders().iter().find_map(|&collider| {
            self.physics.contact_pairs_with(collider).find_map(|pair| {
                let other = if pair.collider1 == collider { pair.collider2 } else { pair.collider1 };
                let is_support = pair.has_any_active_contact() && !self.knife_colliders.contains(&other);
                is_support.then(|| colliders[other].parent()).flatten()
            })
        })
    }

    fn step_stand(&mut self, stand_angle: i32) {
        let max_step_rad = STAND_SPEED_RAD_PER_S * DT_S;
        let step_rad = (calc_radians(stand_angle) - self.stand_angle_rad).clamp(-max_step_rad, max_step_rad);
        let wanted_rad = self.stand_angle_rad + step_rad;
        self.stand_angle_rad = self.calc_stand_reach_rad(wanted_rad);
        self.is_stand_blocked = self.stand_angle_rad != wanted_rad;
        if self.is_stand_blocked {
            // The knife holds the cake, which would otherwise spin on into the blade.
            let cake = &mut self.physics.bodies[self.cake];
            cake.set_linvel(Vector::ZERO, true);
            cake.set_angvel(Vector::ZERO, true);
        }
        let half_rad = self.stand_angle_rad / 2.0;
        let rotation = Rotation::from_xyzw(0.0, libm::sin(half_rad) as f32, 0.0, libm::cos(half_rad) as f32);
        self.physics.bodies[self.stand].set_next_kinematic_rotation(rotation);
    }

    /// How far the stand turns toward `to_rad` before the cake riding it would meet the lowered
    /// knife.
    fn calc_stand_reach_rad(&self, to_rad: f64) -> f64 {
        let is_riding = self.grip.is_none() && self.is_on_stand();
        // A cake turning on the stand keeps its height, so it stays clear of the resting knife.
        if !is_riding || self.knife.lower_m <= self.knife.rest_m {
            return to_rad;
        }
        let from_rad = self.stand_angle_rad;
        let plate = self.stand_plate.as_solid();
        calc_sweep_reach(from_rad, to_rad, |rad| {
            let (sin, cos) = (libm::sin(rad - from_rad), libm::cos(rad - from_rad));
            let turned: Vec<DVec3> = self
                .cake_positions
                .iter()
                .map(|p| DVec3::new(p.x * cos + p.z * sin, p.y, p.z * cos - p.x * sin))
                .collect();
            let cake = Solid { positions: &turned, triangles: &self.model.mesh.triangles };
            self.knife.is_clear_of(&[cake, plate])
        })
    }

    fn step_knife(&mut self, input: &KnifeInput) {
        let cake = Solid { positions: &self.cake_positions, triangles: &self.model.mesh.triangles };
        let plate = self.stand_plate.as_solid();
        // A held cake meets the knife's collider instead, so the blade holds its place against it.
        let solids: &[Solid] = if self.grip.is_some() { &[plate] } else { &[cake, plate] };
        self.knife.float_over(self.calc_rest_height_m(), DT_S);
        self.knife.step(input, solids);
        let pose = Pose::from_parts(self.knife.calc_edge_middle().as_vec3(), self.knife.calc_rotation());
        self.physics.bodies[self.knife_body].set_next_kinematic_position(pose);
    }

    /// The cake height the knife rests over: the cake's as posed at the stand, easing to its
    /// waiting height [`REST_APPROACH_M`] away.
    fn calc_rest_height_m(&self) -> f64 {
        let nearness = self.calc_stand_nearness(REST_APPROACH_M);
        self.waiting_height_m + (self.calc_cake_height_m() - self.waiting_height_m) * nearness
    }

    /// How tall the cake stands as it is posed now.
    fn calc_cake_height_m(&self) -> f64 {
        let (low_m, high_m) = self
            .cake_positions
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), p| (low.min(p.y), high.max(p.y)));
        high_m - low_m
    }

    /// Respawns a cake that fell off the table, once it has fallen for a while.
    fn ensure_cake_on_table(&mut self) {
        if self.physics.bodies[self.cake].translation().y as f64 >= RESPAWN_BELOW_Y_M {
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

    fn update_cake_positions(&mut self) {
        let pose = *self.physics.bodies[self.cake].position();
        let rotation = pose.rotation.as_dquat();
        let translation = pose.translation.as_dvec3();
        self.cake_positions.clear();
        self.cake_positions.extend(self.model.mesh.positions.iter().map(|p| rotation * p.as_dvec3() + translation));
    }
}

struct Fnv1a(u64);

impl Fnv1a {
    fn new() -> Self {
        Fnv1a(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0 ^ byte as u64).wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}
