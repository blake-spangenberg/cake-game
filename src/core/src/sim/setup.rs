//! Builds the bodies and colliders of a fresh world.
#![expect(clippy::cast_possible_truncation, reason = "hands the core's f64 layout to Rapier, which works in f32")]

use std::sync::Arc;

use rapier3d::glamx::DVec3;
use rapier3d::prelude::*;

use super::mass::calc_mass_properties;
use super::squash::HullFrame;
use super::{CAKE_FRICTION, FREE_ANGULAR_DAMPING, FREE_LINEAR_DAMPING};
use crate::cakes::CakeModel;
use crate::knife::{HANDLE_LEN_M, HANDLE_RADIUS_M, Knife, Solid, build_blade_hull, calc_handle_centers};
use crate::mesh::{Mesh, calc_arc_outline};
use crate::table::{CAKE_SPAWN, STAND_PARTS, STAND_RADIUS_M, TABLE_HALF_X_M, TABLE_HALF_Z_M};
use crate::triangle_tree::TriangleTree;

const STAND_SEGMENTS: u32 = 48;
/// Every cake's mass density in the simulation, apart from the grams players see.
const CAKE_DENSITY_KG_PER_M3: f32 = 1000.0;

/// A cake model with what every world on it derives from the model alone, built once so that
/// worlds on the same cake, such as a round's on the server, share it through an `Arc`.
pub struct Cake {
    model: CakeModel,
    pub(super) tree: TriangleTree,
    /// How tall the cake stands in its flattest and its tallest pose, about.
    pub(super) width_range_m: (f64, f64),
    mass: MassProperties,
    hull_shapes: Vec<SharedShape>,
    /// The hulls as one shape, for sweeping the cake along the hand's moves.
    pub(super) shape: SharedShape,
    /// The hulls' corners and edges, for finding the patch a resting cake squashes onto.
    pub(super) hull_frame: HullFrame,
}

impl Cake {
    /// # Panics
    /// On a degenerate hull, which [`CakeModel::validate`] rejects.
    #[must_use]
    pub fn build(model: CakeModel) -> Self {
        let mesh = &model.mesh;
        let tree = TriangleTree::build(&mesh.positions, &mesh.triangles);
        let width_range_m = mesh.calc_width_range_m(Some(&tree));
        let mass = calc_mass_properties(CAKE_DENSITY_KG_PER_M3, &mesh.positions, &mesh.triangles);
        let hull_shapes: Vec<SharedShape> = model
            .hulls
            .iter()
            .map(|hull| SharedShape::convex_hull(hull).expect("a cake's hulls are not degenerate"))
            .collect();
        let shape = SharedShape::compound(hull_shapes.iter().map(|hull| (Pose::IDENTITY, hull.clone())).collect());
        let hull_frame = HullFrame::build(&hull_shapes);
        Self { model, tree, width_range_m, mass, hull_shapes, shape, hull_frame }
    }

    #[must_use]
    pub const fn get_model(&self) -> &CakeModel {
        &self.model
    }
}

impl From<CakeModel> for Arc<Cake> {
    fn from(model: CakeModel) -> Self {
        Self::new(Cake::build(model))
    }
}

/// The stand's plate as a closed mesh in world coordinates, for the knife's edge to stop on. Its
/// turning is ignored, since the plate is round.
pub(super) struct StandPlate {
    positions: Vec<DVec3>,
    triangles: Vec<[u32; 3]>,
}

impl StandPlate {
    #[expect(clippy::cast_precision_loss, reason = "a segment count far below f32's whole-number range")]
    pub(super) fn build() -> Self {
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
        Self { positions: mesh.positions.iter().map(|p| p.as_dvec3()).collect(), triangles: mesh.triangles }
    }

    pub(super) fn as_solid(&self) -> Solid<'_> {
        Solid::new(&self.positions, &self.triangles)
    }
}

pub(super) fn insert_table(physics: &mut PhysicsWorld) {
    let half_y_m = 0.02;
    physics.insert(
        RigidBodyBuilder::fixed().translation(Vector::new(0.0, -half_y_m, 0.0)),
        ColliderBuilder::cuboid(TABLE_HALF_X_M as f32, half_y_m, TABLE_HALF_Z_M as f32).friction(0.7),
    );
}

/// The stand's plate, column, and base as one fixed body. They are round, so the physics need not
/// turn them: the world carries a cake riding the stand round instead.
pub(super) fn insert_stand(physics: &mut PhysicsWorld) -> RigidBodyHandle {
    let stand = physics.insert_body(RigidBodyBuilder::fixed());
    for part in STAND_PARTS {
        physics.insert_collider(build_stand_cylinder(part.radius_m, part.y0_m, part.y1_m), Some(stand));
    }
    stand
}

/// The space under the stand's plate filled in as one solid cylinder, disabled until a cake is
/// held, so a carried cake swinging low cannot slip in under the plate and along the column.
pub(super) fn insert_under_plate_fill(physics: &mut PhysicsWorld, stand: RigidBodyHandle) -> ColliderHandle {
    let fill = build_stand_cylinder(STAND_RADIUS_M, 0.0, STAND_PARTS[0].y0_m).enabled(false);
    physics.insert_collider(fill, Some(stand))
}

fn build_stand_cylinder(radius_m: f64, y0_m: f64, y1_m: f64) -> ColliderBuilder {
    let half_height_m = ((y1_m - y0_m) / 2.0) as f32;
    ColliderBuilder::cylinder(half_height_m, radius_m as f32)
        .translation(Vector::new(0.0, (y0_m as f32) + half_height_m, 0.0))
        .friction(1.0)
}

pub(super) fn insert_cake(physics: &mut PhysicsWorld, shared: &Cake) -> RigidBodyHandle {
    let cake = physics.insert_body(
        RigidBodyBuilder::dynamic()
            .translation(CAKE_SPAWN.as_vec3())
            .additional_mass_properties(shared.mass)
            // The swinging cake otherwise tunnels through the thin blade, which the hand's
            // sweep alone cannot prevent because it only stops the cake's translation.
            .ccd_enabled(true)
            .linear_damping(FREE_LINEAR_DAMPING)
            .angular_damping(FREE_ANGULAR_DAMPING),
    );
    for hull in &shared.hull_shapes {
        let collider = ColliderBuilder::new(hull.clone()).density(0.0).friction(CAKE_FRICTION);
        physics.insert_collider(collider, Some(cake));
    }
    // Rapier folds the additional mass in only at the first step, so until then the center of
    // mass reads as the body's origin, which would size the grab's reach about the wrong point.
    physics.bodies[cake].recompute_mass_properties_from_colliders(&physics.colliders);
    cake
}

/// The knife's blade and handles, disabled until a cake is held.
pub(super) fn insert_knife(physics: &mut PhysicsWorld, knife: &Knife) -> (RigidBodyHandle, Vec<ColliderHandle>) {
    let body = physics.insert_body(RigidBodyBuilder::kinematic_position_based().pose(knife.calc_pose()));
    let blade = ColliderBuilder::convex_hull(&build_blade_hull()).expect("the blade hull is not degenerate");
    let handles = calc_handle_centers().map(|center| {
        ColliderBuilder::cylinder((HANDLE_LEN_M / 2.0) as f32, HANDLE_RADIUS_M as f32).translation(center)
    });
    let colliders = std::iter::once(blade)
        .chain(handles)
        .map(|collider| physics.insert_collider(collider.enabled(false), Some(body)))
        .collect();
    (body, colliders)
}
