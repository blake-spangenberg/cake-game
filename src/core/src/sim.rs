use rapier3d::glamx::DVec3;
use rapier3d::parry::query::ShapeCastOptions;
use rapier3d::prelude::*;

use crate::cakes::CakeModel;
use crate::cut::{DENSITY_G_PER_M3, Split, calc_split, is_crossing};
use crate::input::{HandInput, Input, KnifeInput, calc_meters, calc_radians};
use crate::knife::{HANDLE_LEN_M, HANDLE_RADIUS_M, Knife, Solid, build_blade_hull, calc_handle_centers};
use crate::mesh::{Mesh, calc_arc_outline};
use crate::table::{
    CAKE_SPAWN, RESPAWN_BELOW_Y_M, STAND_BASE_RADIUS_M, STAND_BASE_THICKNESS_M, STAND_COLUMN_RADIUS_M,
    STAND_PLATE_THICKNESS_M, STAND_RADIUS_M, STAND_TOP_Y_M, TABLE_HALF_X_M, TABLE_HALF_Z_M,
};

pub const TICKS_PER_S: u32 = 60;
/// A competitive round's length, at whose last tick the cut happens for anyone who has not cut.
pub const ROUND_TICKS: u32 = 30 * TICKS_PER_S;
const DT_S: f64 = 1.0 / TICKS_PER_S as f64;
/// Rapier scales its slop, contact prediction, and sleep thresholds by this, so they suit
/// objects a decimeter across instead of a meter.
const LENGTH_UNIT_M: f32 = 0.1;
const HAND_SPEED_M_PER_S: f64 = 1.5;
/// Fraction of the remaining distance the hand covers each tick, easing it into its target.
const HAND_EASE_PER_TICK: f64 = 0.2;
/// How far a carried cake's lowest point clears the table or stand beneath it.
const CARRY_CLEARANCE_M: f64 = 0.02;
/// Across this gap between a carried cake and the stand's rim, the height it clears blends from
/// the table's to the stand's.
const STAND_APPROACH_M: f64 = 0.15;
const STAND_SPEED_RAD_PER_S: f64 = core::f64::consts::TAU;
/// Slack around the cake's bounding sphere within which a grab point is accepted.
const GRAB_TOLERANCE_M: f64 = 0.005;
const HELD_LINEAR_DAMPING: f32 = 4.0;
const HELD_ANGULAR_DAMPING: f32 = 6.0;
const FREE_LINEAR_DAMPING: f32 = 0.5;
const FREE_ANGULAR_DAMPING: f32 = 2.0;
const CAKE_FRICTION: f32 = 0.9;
/// Spin a free cake touching the table or stand loses per second, standing in for the rolling
/// resistance of a soft cake squashing at its contact. Unlike damping, which only slows a roll
/// gravity keeps driving, it stops an off-balance cake rocking or rolling on its rim outright,
/// while tipping over still outpaces it.
const ROLL_RESISTANCE_RAD_PER_S2: f32 = 80.0;
/// Gap a carried cake keeps from the knife when its hand is stopped short.
const KNIFE_CLEARANCE_M: f32 = 0.002;
const STAND_SEGMENTS: u32 = 48;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    Playing,
    Cut { tick: u32, split: Split },
}

struct Grip {
    joint: ImpulseJointHandle,
    grab_at: [i32; 3],
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
    stand_plate_positions: Vec<DVec3>,
    stand_plate_triangles: Vec<[u32; 3]>,
    knife_body: RigidBodyHandle,
    knife_colliders: Vec<ColliderHandle>,
    model: CakeModel,
    bounding_radius_m: f64,
    cake_positions: Vec<DVec3>,
    knife: Knife,
    tick: u32,
    round_ticks: Option<u32>,
    phase: Phase,
}

impl World {
    /// A world that cuts itself at `round_ticks`, or never with `None`.
    pub fn create(model: CakeModel, knife: Knife, round_ticks: Option<u32>) -> World {
        let mut physics = PhysicsWorld::new();
        physics.integration_parameters.dt = DT_S as f32;
        physics.integration_parameters.length_unit = LENGTH_UNIT_M;
        let table_half_y_m = 0.02;
        physics.insert(
            RigidBodyBuilder::fixed().translation(Vector::new(0.0, -table_half_y_m, 0.0)),
            ColliderBuilder::cuboid(TABLE_HALF_X_M as f32, table_half_y_m, TABLE_HALF_Z_M as f32).friction(0.7),
        );
        let stand = insert_stand(&mut physics);
        let cake = insert_cake(&mut physics, &model);
        let hand = physics.insert_body(RigidBodyBuilder::kinematic_position_based());
        let (knife_body, knife_colliders) = insert_knife(&mut physics, &knife);
        let com = physics.bodies[cake].mass_properties().local_mprops.local_com;
        let bounding_radius_m = model.mesh.calc_max_distance_from(com) as f64;
        let plate = build_stand_plate();
        let cake_shape = SharedShape::compound(
            model
                .hulls
                .iter()
                .map(|hull| {
                    (Pose::IDENTITY, SharedShape::convex_hull(hull).expect("placeholder hulls are not degenerate"))
                })
                .collect(),
        );
        let mut world = World {
            physics,
            cake,
            cake_shape,
            hand,
            grip: None,
            stand,
            stand_angle_rad: 0.0,
            stand_plate_positions: plate.positions.iter().map(|p| p.as_dvec3()).collect(),
            stand_plate_triangles: plate.triangles,
            knife_body,
            knife_colliders,
            model,
            bounding_radius_m,
            cake_positions: Vec::new(),
            knife,
            tick: 0,
            round_ticks,
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

    /// Whether a cut now would divide the cake rather than miss it.
    pub fn is_cut_through(&self) -> bool {
        is_crossing(&self.cake_positions, &self.knife.calc_plane())
    }

    /// How far the knife would lower through the cake to the stand or table, for drawing the cut.
    pub fn calc_cut_lower_m(&self) -> f64 {
        self.knife.calc_lower_limit_m(&[self.get_stand_plate()])
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
        for value in [self.knife.tilt_rad, self.knife.lower_m, self.stand_angle_rad] {
            hash.write(&value.to_bits().to_le_bytes());
        }
        hash.finish()
    }

    fn get_stand_plate(&self) -> Solid<'_> {
        Solid { positions: &self.stand_plate_positions, triangles: &self.stand_plate_triangles }
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
        let lowest_y_m = self.cake_positions.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
        let hand_y_m = self.physics.bodies[self.hand].translation().y as f64;
        let target = DVec3::new(
            target_xz[0].clamp(-TABLE_HALF_X_M, TABLE_HALF_X_M),
            hand_y_m + self.calc_support_y_m() + CARRY_CLEARANCE_M - lowest_y_m,
            target_xz[1].clamp(-TABLE_HALF_Z_M, TABLE_HALF_Z_M),
        );
        self.move_hand_toward(target);
    }

    /// Height of the surface beneath the cake: the stand's top where the cake's bounding box
    /// reaches over the stand, the table's well away from it, and a blend between.
    fn calc_support_y_m(&self) -> f64 {
        let (mut min, mut max) = (DVec3::INFINITY, DVec3::NEG_INFINITY);
        for p in &self.cake_positions {
            (min, max) = (min.min(*p), max.max(*p));
        }
        let nearest_x_m = 0.0_f64.clamp(min.x, max.x);
        let nearest_z_m = 0.0_f64.clamp(min.z, max.z);
        let gap_m = libm::sqrt(nearest_x_m * nearest_x_m + nearest_z_m * nearest_z_m) - STAND_RADIUS_M;
        STAND_TOP_Y_M * (1.0 - gap_m / STAND_APPROACH_M).clamp(0.0, 1.0)
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
        self.grip = Some(Grip { joint, grab_at });
    }

    fn release(&mut self) {
        let Some(grip) = self.grip.take() else {
            return;
        };
        self.physics.remove_impulse_joint(grip.joint);
        set_damping(&mut self.physics.bodies[self.cake], FREE_LINEAR_DAMPING, FREE_ANGULAR_DAMPING);
        self.set_knife_solid(false);
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

    /// Moves the hand next tick, stopping the carried cake at the knife and sliding it along the
    /// blade. The rigid grip would otherwise drag the cake through the thin blade, since a
    /// kinematic hand outmuscles any contact.
    fn set_hand_next(&mut self, to: DVec3) {
        let from = self.physics.bodies[self.hand].translation().as_dvec3();
        let step = (to - from).as_vec3();
        let knife_colliders = &self.knife_colliders;
        let is_knife = |collider: ColliderHandle, _: &Collider| knife_colliders.contains(&collider);
        let options = ShapeCastOptions {
            max_time_of_impact: 1.0,
            target_distance: KNIFE_CLEARANCE_M,
            stop_at_penetration: false,
            compute_impact_geometry_on_penetration: true,
        };
        let cake_pose = self.physics.bodies[self.cake].position();
        let hit = self.physics.cast_shape(
            cake_pose,
            step,
            &*self.cake_shape,
            options,
            QueryFilter::new().predicate(&is_knife),
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

    fn step_stand(&mut self, stand_angle: i32) {
        let max_step_rad = STAND_SPEED_RAD_PER_S * DT_S;
        self.stand_angle_rad += (calc_radians(stand_angle) - self.stand_angle_rad).clamp(-max_step_rad, max_step_rad);
        let half_rad = self.stand_angle_rad / 2.0;
        let rotation = Rotation::from_xyzw(0.0, libm::sin(half_rad) as f32, 0.0, libm::cos(half_rad) as f32);
        self.physics.bodies[self.stand].set_next_kinematic_rotation(rotation);
    }

    fn step_knife(&mut self, input: &KnifeInput) {
        let cake = Solid { positions: &self.cake_positions, triangles: &self.model.mesh.triangles };
        let plate = Solid { positions: &self.stand_plate_positions, triangles: &self.stand_plate_triangles };
        // A held cake meets the knife's collider instead, so the blade holds its place against it.
        let solids: &[Solid] = if self.grip.is_some() { &[plate] } else { &[cake, plate] };
        self.knife.step(input, solids);
        let pose = Pose::from_parts(self.knife.calc_edge_middle().as_vec3(), self.knife.calc_rotation());
        self.physics.bodies[self.knife_body].set_next_kinematic_position(pose);
    }

    /// Respawns a cake that fell off the table.
    fn ensure_cake_on_table(&mut self) {
        if self.physics.bodies[self.cake].translation().y as f64 >= RESPAWN_BELOW_Y_M {
            return;
        }
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

/// The stand's plate, column, and base as one kinematic body turning about the vertical axis.
fn insert_stand(physics: &mut PhysicsWorld) -> RigidBodyHandle {
    let stand = physics.insert_body(RigidBodyBuilder::kinematic_position_based());
    let column_bottom_m = STAND_BASE_THICKNESS_M;
    let column_top_m = STAND_TOP_Y_M - STAND_PLATE_THICKNESS_M;
    let parts = [
        (STAND_RADIUS_M, column_top_m, STAND_TOP_Y_M),
        (STAND_COLUMN_RADIUS_M, column_bottom_m, column_top_m),
        (STAND_BASE_RADIUS_M, 0.0, column_bottom_m),
    ];
    for (radius_m, y0_m, y1_m) in parts {
        let half_height_m = ((y1_m - y0_m) / 2.0) as f32;
        let collider = ColliderBuilder::cylinder(half_height_m, radius_m as f32)
            .translation(Vector::new(0.0, (y0_m as f32) + half_height_m, 0.0))
            .friction(1.0);
        physics.insert_collider(collider, Some(stand));
    }
    stand
}

fn insert_cake(physics: &mut PhysicsWorld, model: &CakeModel) -> RigidBodyHandle {
    let density_kg_per_m3 = (DENSITY_G_PER_M3 / 1000.0) as f32;
    let mass = MassProperties::from_trimesh(density_kg_per_m3, &model.mesh.positions, &model.mesh.triangles);
    let cake = physics.insert_body(
        RigidBodyBuilder::dynamic()
            .translation(CAKE_SPAWN.as_vec3())
            .additional_mass_properties(mass)
            // The swinging cake otherwise tunnels through the thin blade, which the hand's
            // sweep alone cannot prevent because it only stops the cake's translation.
            .ccd_enabled(true)
            .linear_damping(FREE_LINEAR_DAMPING)
            .angular_damping(FREE_ANGULAR_DAMPING),
    );
    for hull in &model.hulls {
        let collider = ColliderBuilder::convex_hull(hull).expect("placeholder hulls are not degenerate");
        physics.insert_collider(collider.density(0.0).friction(CAKE_FRICTION), Some(cake));
    }
    cake
}

/// The knife's blade and handles, disabled until a cake is held.
fn insert_knife(physics: &mut PhysicsWorld, knife: &Knife) -> (RigidBodyHandle, Vec<ColliderHandle>) {
    let pose = Pose::from_parts(knife.calc_edge_middle().as_vec3(), knife.calc_rotation());
    let body = physics.insert_body(RigidBodyBuilder::kinematic_position_based().pose(pose));
    let blade = ColliderBuilder::convex_hull(&build_blade_hull()).expect("the blade hull is not degenerate");
    let handles = calc_handle_centers().map(|center| {
        ColliderBuilder::cylinder((HANDLE_LEN_M / 2.0) as f32, HANDLE_RADIUS_M as f32).translation(center)
    });
    let colliders = [blade]
        .into_iter()
        .chain(handles)
        .map(|collider| physics.insert_collider(collider.enabled(false), Some(body)))
        .collect();
    (body, colliders)
}

/// The stand's plate as a closed mesh, for the knife's edge to stop on. Its turning is ignored,
/// since the plate is round.
fn build_stand_plate() -> Mesh {
    let mut plate = Mesh::default();
    let outline = calc_arc_outline(
        [0.0, 0.0],
        STAND_RADIUS_M as f32,
        0.0,
        core::f32::consts::TAU * (1.0 - 1.0 / STAND_SEGMENTS as f32),
        STAND_SEGMENTS,
    );
    plate.push_prism(&outline, (STAND_TOP_Y_M - STAND_PLATE_THICKNESS_M) as f32, STAND_TOP_Y_M as f32);
    plate
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
