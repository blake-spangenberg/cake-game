//! The hand that carries the cake: grabbing, steering, and stopping it against solids.

use rapier3d::glamx::DVec3;
use rapier3d::parry::query::ShapeCastOptions;
use rapier3d::prelude::*;

use super::{DT_S, FREE_ANGULAR_DAMPING, FREE_LINEAR_DAMPING, Fnv1a, World};
use crate::input::{HandInput, calc_meters};
use crate::table::{STAND_RADIUS_M, STAND_TOP_Y_M, TABLE_HALF_X_M, TABLE_HALF_Z_M};

const HAND_SPEED_M_PER_S: f64 = 1.5;
/// Fraction of the remaining distance the hand covers each tick, easing it into its target.
const HAND_EASE_PER_TICK: f64 = 0.1;
/// How far a carried cake's lowest point clears the surface beneath it, as a stand pickup lifts
/// it.
const CARRY_CLEARANCE_M: f64 = 0.01;
/// Across this gap between a carried cake and the stand's rim, most of the table, the height it
/// clears eases from the table's up to the stand's.
const STAND_APPROACH_M: f64 = 0.5;
/// How high the hand lifts a cake grabbed while resting on the stand, so the plate still bears
/// its far side and a gentle slide keeps its orientation.
const STAND_LIFT_M: f64 = 0.01;
/// How near the plate a cake's lowest point must be for a grab to count as taking it off the
/// stand.
const ON_STAND_TOLERANCE_M: f64 = 0.01;
/// Slack around the cake's bounding sphere within which a grab point is accepted.
const GRAB_TOLERANCE_M: f64 = 0.005;
const HELD_LINEAR_DAMPING: f32 = 4.0;
const HELD_ANGULAR_DAMPING: f32 = 6.0;
/// Gap a carried cake keeps from the knife, stand, or table when its hand is stopped short.
const HAND_SWEEP_CLEARANCE_M: f32 = 0.002;

pub(super) struct Grip {
    joint: ImpulseJointHandle,
    grab_at: [i32; 3],
    /// The hand's height while a cake taken off the stand still reaches over it.
    stand_hold_y_m: Option<f64>,
}

impl Grip {
    /// Hashes whether a cake is held, and where, so a divergence in grip shows the tick it happens.
    pub(super) fn write_hash(grip: Option<&Grip>, hash: &mut Fnv1a) {
        let Some(grip) = grip else {
            hash.write(&[0]);
            return;
        };
        hash.write(&[1]);
        for coordinate in grip.grab_at {
            hash.write(&coordinate.to_le_bytes());
        }
        match grip.stand_hold_y_m {
            None => hash.write(&[0]),
            Some(hold_y_m) => {
                hash.write(&[1]);
                hash.write(&hold_y_m.to_bits().to_le_bytes());
            }
        }
    }
}

impl World {
    pub(super) fn step_hand(&mut self, hand_input: Option<HandInput>) {
        let Some(hand_input) = hand_input else {
            self.release();
            return;
        };
        let is_same_grab = self.grip.as_ref().is_some_and(|grip| grip.grab_at == hand_input.grab_at);
        if !is_same_grab {
            self.release();
            self.try_grab(hand_input.grab_at);
        }
        self.release_stand_hold();
        let Some(target_y_m) = self.calc_hand_target_y_m() else {
            return;
        };
        let target_xz = hand_input.target_xz.map(calc_meters);
        let target = DVec3::new(
            target_xz[0].clamp(-TABLE_HALF_X_M, TABLE_HALF_X_M),
            target_y_m,
            target_xz[1].clamp(-TABLE_HALF_Z_M, TABLE_HALF_Z_M),
        );
        self.move_hand_toward(target);
    }

    pub(super) fn release(&mut self) {
        let Some(grip) = self.grip.take() else {
            return;
        };
        self.physics.remove_impulse_joint(grip.joint);
        set_damping(&mut self.physics.bodies[self.cake], FREE_LINEAR_DAMPING, FREE_ANGULAR_DAMPING);
        self.set_knife_solid(false);
    }

    /// Whether the cake sits on the stand's plate, judged by its shape rather than its contacts,
    /// which a sleeping cake does not report.
    pub(super) fn is_on_stand(&self) -> bool {
        self.is_over_stand() && (self.calc_lowest_y_m() - STAND_TOP_Y_M).abs() < ON_STAND_TOLERANCE_M
    }

    /// Ends a stand hold for good once the cake no longer reaches over the stand.
    fn release_stand_hold(&mut self) {
        if self.is_over_stand() {
            return;
        }
        if let Some(grip) = self.grip.as_mut() {
            grip.stand_hold_y_m = None;
        }
    }

    /// Height the hand steers to, or none with no grip. A cake taken off the stand is held just
    /// above where it was grabbed until it no longer reaches over the stand, and from then on is
    /// carried with its lowest point just clear of the surface beneath, eased up toward the
    /// stand's top as it nears.
    fn calc_hand_target_y_m(&self) -> Option<f64> {
        let grip = self.grip.as_ref()?;
        if let Some(hold_y_m) = grip.stand_hold_y_m {
            return Some(hold_y_m);
        }
        let hand_y_m = self.physics.bodies[self.hand].translation().y as f64;
        Some(hand_y_m + self.calc_support_y_m() + CARRY_CLEARANCE_M - self.calc_lowest_y_m())
    }

    /// Height of the surface beneath the cake: the stand's top where its bounding box reaches over
    /// the stand, easing down to the table's [`STAND_APPROACH_M`] away.
    fn calc_support_y_m(&self) -> f64 {
        STAND_TOP_Y_M * self.calc_stand_nearness(STAND_APPROACH_M)
    }

    /// How near the cake's bounding box is to the stand, along an S curve from 0 at `approach_m`
    /// outside the rim to 1 where it reaches the rim.
    pub(super) fn calc_stand_nearness(&self, approach_m: f64) -> f64 {
        let nearness = (1.0 - self.calc_stand_gap_m() / approach_m).clamp(0.0, 1.0);
        nearness * nearness * (3.0 - 2.0 * nearness)
    }

    fn calc_lowest_y_m(&self) -> f64 {
        self.cake_positions.iter().map(|p| p.y).fold(f64::INFINITY, f64::min)
    }

    /// Whether the cake's bounding box reaches over the stand's plate.
    fn is_over_stand(&self) -> bool {
        self.calc_stand_gap_m() <= 0.0
    }

    /// How far the cake's bounding box lies outside the stand's rim, seen from above, or a
    /// negative amount where it reaches over the plate.
    fn calc_stand_gap_m(&self) -> f64 {
        let (mut min, mut max) = (DVec3::INFINITY, DVec3::NEG_INFINITY);
        for p in &self.cake_positions {
            (min, max) = (min.min(*p), max.max(*p));
        }
        let nearest_x_m = 0.0_f64.clamp(min.x, max.x);
        let nearest_z_m = 0.0_f64.clamp(min.z, max.z);
        libm::sqrt(nearest_x_m * nearest_x_m + nearest_z_m * nearest_z_m) - STAND_RADIUS_M
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
        self.set_knife_solid(true);
        let stand_hold_y_m = self.is_on_stand().then_some(point.y + STAND_LIFT_M);
        self.grip = Some(Grip { joint, grab_at, stand_hold_y_m });
    }

    /// The knife collides only with a held cake: a free cake is never pushed by the blade,
    /// whose lowering already stops at the cake's surface.
    fn set_knife_solid(&mut self, is_solid: bool) {
        for &collider in &self.knife_colliders {
            self.physics.colliders[collider].set_enabled(is_solid);
        }
    }

    fn move_hand_toward(&mut self, target: DVec3) {
        let from = self.physics.bodies[self.hand].translation().as_dvec3();
        let step = (target - from) * HAND_EASE_PER_TICK;
        let max_step_m = HAND_SPEED_M_PER_S * DT_S;
        let step_len_m = step.length();
        let step = if step_len_m > max_step_m { step * (max_step_m / step_len_m) } else { step };
        self.set_hand_next(from + step);
    }

    /// Moves the hand next tick, stopping the carried cake at the knife, stand, or table and
    /// sliding it along them. The rigid grip would otherwise drag the cake into them, through
    /// the thin blade or clipping into the stand, since a kinematic hand outmuscles any contact.
    fn set_hand_next(&mut self, to: DVec3) {
        let from = self.physics.bodies[self.hand].translation().as_dvec3();
        let step = (to - from).as_vec3();
        let options = ShapeCastOptions {
            max_time_of_impact: 1.0,
            target_distance: HAND_SWEEP_CLEARANCE_M,
            stop_at_penetration: false,
            compute_impact_geometry_on_penetration: true,
        };
        let cake_pose = self.physics.bodies[self.cake].position();
        let hit = self.physics.cast_shape(
            cake_pose,
            step,
            &*self.cake_shape,
            options,
            QueryFilter::new().exclude_rigid_body(self.cake),
        );
        let step = match hit {
            None => step,
            Some((_, hit)) => {
                let allowed = step * hit.time_of_impact;
                let rest = step - allowed;
                allowed + rest - hit.normal1 * rest.dot(hit.normal1).min(0.0)
            }
        };
        self.physics.bodies[self.hand].set_next_kinematic_translation(from.as_vec3() + step);
    }
}

fn set_damping(body: &mut RigidBody, linear: f32, angular: f32) {
    body.set_linear_damping(linear);
    body.set_angular_damping(angular);
    body.wake_up(true);
}
