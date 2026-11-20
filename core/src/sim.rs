use rapier3d::glamx::DVec3;
use rapier3d::prelude::*;

use crate::cakes::CakeModel;
use crate::cut::{Split, calc_split};
use crate::input::{HandInput, Input, calc_meters};
use crate::knife::Knife;
use crate::table::{CAKE_SPAWN, TABLE_HALF_X_M, TABLE_HALF_Z_M};

pub const TICKS_PER_S: u32 = 60;
/// The round's last tick, at which the cut happens for anyone who has not cut yet.
pub const ROUND_TICKS: u32 = 30 * TICKS_PER_S;
const DT_S: f64 = 1.0 / TICKS_PER_S as f64;
const HAND_SPEED_M_PER_S: f64 = 1.0;
/// Height the held cake's lowest point hovers above the table.
const HOLD_CLEARANCE_M: f64 = 0.01;
/// Slack around the cake's bounding sphere within which a grab point is accepted.
const GRAB_TOLERANCE_M: f64 = 0.005;
const HELD_LINEAR_DAMPING: f32 = 2.0;
const HELD_ANGULAR_DAMPING: f32 = 3.0;
const FREE_DAMPING: f32 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    Playing,
    Cut { tick: u32, split: Split },
}

struct Grip {
    joint: ImpulseJointHandle,
    grab_at: [i32; 3],
}

/// One player's cake, hand, and knife on the shared table, advanced one fixed tick per input.
pub struct World {
    physics: PhysicsWorld,
    cake: RigidBodyHandle,
    hand: RigidBodyHandle,
    grip: Option<Grip>,
    model: CakeModel,
    bounding_radius_m: f64,
    cake_positions: Vec<DVec3>,
    knife: Knife,
    tick: u32,
    phase: Phase,
}

impl World {
    pub fn create(model: CakeModel, knife: Knife) -> World {
        let mut physics = PhysicsWorld::new();
        physics.integration_parameters.dt = DT_S as f32;
        let table_half_y_m = 0.05;
        physics.insert(
            RigidBodyBuilder::fixed().translation(Vector::new(0.0, -table_half_y_m, 0.0)),
            ColliderBuilder::cuboid(TABLE_HALF_X_M as f32, table_half_y_m, TABLE_HALF_Z_M as f32),
        );
        let mass = MassProperties::from_trimesh(1.0, &model.mesh.positions, &model.mesh.triangles);
        let cake = physics.insert_body(
            RigidBodyBuilder::dynamic()
                .translation(CAKE_SPAWN.as_vec3())
                .additional_mass_properties(mass)
                .linear_damping(FREE_DAMPING)
                .angular_damping(FREE_DAMPING),
        );
        for hull in &model.hulls {
            let collider = ColliderBuilder::convex_hull(hull).expect("placeholder hulls are not degenerate");
            physics.insert_collider(collider.density(0.0), Some(cake));
        }
        let hand = physics.insert_body(RigidBodyBuilder::kinematic_position_based());
        let bounding_radius_m = model.mesh.calc_max_distance_from(mass.local_com) as f64;
        let mut world = World {
            physics,
            cake,
            hand,
            grip: None,
            model,
            bounding_radius_m,
            cake_positions: Vec::new(),
            knife,
            tick: 0,
            phase: Phase::Playing,
        };
        world.update_cake_positions();
        world
    }

    pub fn step(&mut self, input: &Input) {
        if self.phase != Phase::Playing {
            return;
        }
        self.step_hand(input.hand);
        self.physics.step();
        self.update_cake_positions();
        self.knife.step(&input.knife, &self.cake_positions, &self.model.mesh.triangles, DT_S);
        self.tick += 1;
        if self.tick >= ROUND_TICKS {
            self.cut();
        }
    }

    /// Freezes the world and cuts along the knife's plane as it is now. Later calls return the
    /// first cut.
    pub fn cut(&mut self) -> Split {
        if let Phase::Cut { split, .. } = self.phase {
            return split;
        }
        let split = calc_split(&self.cake_positions, &self.model.mesh.triangles, &self.knife.calc_plane());
        self.phase = Phase::Cut { tick: self.tick, split };
        split
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
        for value in [self.knife.tilt_rad, self.knife.heading_rad, self.knife.lower_m] {
            hash.write(&value.to_bits().to_le_bytes());
        }
        hash.finish()
    }

    fn step_hand(&mut self, hand_input: Option<HandInput>) {
        let Some(hand_input) = hand_input else {
            self.release();
            return;
        };
        let is_same_grab = self.grip.as_ref().is_some_and(|grip| grip.grab_at == hand_input.grab_at);
        if !is_same_grab {
            self.release();
            self.try_grab(hand_input.grab_at);
        }
        if self.grip.is_none() {
            return;
        }
        let target_xz = hand_input.target_xz.map(calc_meters);
        self.move_hand_toward(target_xz);
    }

    /// Grabs at `grab_at`, ignored where it lies beyond the cake, since a client may send any
    /// point.
    fn try_grab(&mut self, grab_at: [i32; 3]) {
        let point = DVec3::from_array(grab_at.map(calc_meters));
        let cake = &self.physics.bodies[self.cake];
        let local_point = cake.position().inverse_transform_point(point.as_vec3());
        let com = cake.mass_properties().local_mprops.local_com;
        if local_point.distance(com) as f64 > self.bounding_radius_m + GRAB_TOLERANCE_M {
            return;
        }
        let hand = &mut self.physics.bodies[self.hand];
        hand.set_translation(point.as_vec3(), true);
        hand.set_next_kinematic_translation(point.as_vec3());
        let joint = SphericalJointBuilder::new().local_anchor1(Vector::ZERO).local_anchor2(local_point);
        let joint = self.physics.insert_impulse_joint(self.hand, self.cake, joint);
        set_damping(&mut self.physics.bodies[self.cake], HELD_LINEAR_DAMPING, HELD_ANGULAR_DAMPING);
        self.grip = Some(Grip { joint, grab_at });
    }

    fn release(&mut self) {
        let Some(grip) = self.grip.take() else {
            return;
        };
        self.physics.remove_impulse_joint(grip.joint);
        set_damping(&mut self.physics.bodies[self.cake], FREE_DAMPING, FREE_DAMPING);
    }

    /// Steers the hand toward the target across the table, and up or down so the hanging cake's
    /// lowest point clears the table by [`HOLD_CLEARANCE_M`].
    fn move_hand_toward(&mut self, target_xz: [f64; 2]) {
        let lowest_y_m = self.cake_positions.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
        let hand = &mut self.physics.bodies[self.hand];
        let from = hand.translation().as_dvec3();
        let target = DVec3::new(
            target_xz[0].clamp(-TABLE_HALF_X_M, TABLE_HALF_X_M),
            from.y + HOLD_CLEARANCE_M - lowest_y_m,
            target_xz[1].clamp(-TABLE_HALF_Z_M, TABLE_HALF_Z_M),
        );
        let step = target - from;
        let max_step_m = HAND_SPEED_M_PER_S * DT_S;
        let step_len_m = step.length();
        let step = if step_len_m > max_step_m { step * (max_step_m / step_len_m) } else { step };
        hand.set_next_kinematic_translation((from + step).as_vec3());
    }

    fn update_cake_positions(&mut self) {
        let pose = *self.physics.bodies[self.cake].position();
        let rotation = pose.rotation.as_dquat();
        let translation = pose.translation.as_dvec3();
        self.cake_positions.clear();
        self.cake_positions.extend(self.model.mesh.positions.iter().map(|p| rotation * p.as_dvec3() + translation));
    }
}

fn set_damping(body: &mut RigidBody, linear: f32, angular: f32) {
    body.set_linear_damping(linear);
    body.set_angular_damping(angular);
    body.wake_up(true);
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
