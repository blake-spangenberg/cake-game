//! Builds the bodies and colliders of a fresh world.

use rapier3d::glamx::DVec3;
use rapier3d::prelude::*;

use super::{CAKE_FRICTION, FREE_ANGULAR_DAMPING, FREE_LINEAR_DAMPING};
use crate::cakes::CakeModel;
use crate::cut::DENSITY_G_PER_M3;
use crate::knife::{HANDLE_LEN_M, HANDLE_RADIUS_M, Knife, Solid, build_blade_hull, calc_handle_centers};
use crate::mesh::{Mesh, calc_arc_outline};
use crate::table::{CAKE_SPAWN, STAND_PARTS, TABLE_HALF_X_M, TABLE_HALF_Z_M};

const STAND_SEGMENTS: u32 = 48;

/// The stand's plate as a closed mesh in world coordinates, for the knife's edge to stop on. Its
/// turning is ignored, since the plate is round.
pub(super) struct StandPlate {
    positions: Vec<DVec3>,
    triangles: Vec<[u32; 3]>,
}

impl StandPlate {
    pub(super) fn build() -> StandPlate {
        let plate = STAND_PARTS[0];
        let outline = calc_arc_outline(
            [0.0, 0.0],
            plate.radius_m as f32,
            0.0,
            core::f32::consts::TAU * (1.0 - 1.0 / STAND_SEGMENTS as f32),
            STAND_SEGMENTS,
        );
        let mut mesh = Mesh::default();
        mesh.push_prism(&outline, plate.y0_m as f32, plate.y1_m as f32);
        StandPlate { positions: mesh.positions.iter().map(|p| p.as_dvec3()).collect(), triangles: mesh.triangles }
    }

    pub(super) fn as_solid(&self) -> Solid<'_> {
        Solid { positions: &self.positions, triangles: &self.triangles }
    }
}

pub(super) fn insert_table(physics: &mut PhysicsWorld) {
    let half_y_m = 0.02;
    physics.insert(
        RigidBodyBuilder::fixed().translation(Vector::new(0.0, -half_y_m, 0.0)),
        ColliderBuilder::cuboid(TABLE_HALF_X_M as f32, half_y_m, TABLE_HALF_Z_M as f32).friction(0.7),
    );
}

/// The stand's plate, column, and base as one kinematic body turning about the vertical axis.
pub(super) fn insert_stand(physics: &mut PhysicsWorld) -> RigidBodyHandle {
    let stand = physics.insert_body(RigidBodyBuilder::kinematic_position_based());
    for part in STAND_PARTS {
        let half_height_m = ((part.y1_m - part.y0_m) / 2.0) as f32;
        let collider = ColliderBuilder::cylinder(half_height_m, part.radius_m as f32)
            .translation(Vector::new(0.0, (part.y0_m as f32) + half_height_m, 0.0))
            .friction(1.0);
        physics.insert_collider(collider, Some(stand));
    }
    stand
}

pub(super) fn insert_cake(physics: &mut PhysicsWorld, model: &CakeModel) -> RigidBodyHandle {
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

/// The cake's hulls as one shape, for sweeping it along the hand's moves.
pub(super) fn build_cake_shape(model: &CakeModel) -> SharedShape {
    let hulls = model
        .hulls
        .iter()
        .map(|hull| (Pose::IDENTITY, SharedShape::convex_hull(hull).expect("placeholder hulls are not degenerate")));
    SharedShape::compound(hulls.collect())
}

/// The knife's blade and handles, disabled until a cake is held.
pub(super) fn insert_knife(physics: &mut PhysicsWorld, knife: &Knife) -> (RigidBodyHandle, Vec<ColliderHandle>) {
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
