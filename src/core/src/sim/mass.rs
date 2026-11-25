//! A closed mesh's mass properties, the same on every platform.

use rapier3d::glamx::{DMat3, DVec3, Mat3, Quat};
use rapier3d::parry::mass_properties::details::{
    tetrahedron_unit_inertia_tensor_wrt_point, trimesh_signed_volume_and_center_of_mass,
};
use rapier3d::parry::shape::Tetrahedron;
use rapier3d::prelude::*;

/// Jacobi sweeps after which the inertia is diagonal to f64 rounding: each sweep squares the
/// off-diagonal error, so a handful suffice, and a fixed count keeps the result deterministic.
const JACOBI_SWEEPS: u32 = 12;

/// What [`MassProperties::from_trimesh`] computes, but with the inertia diagonalized by Jacobi
/// rotations, which need only arithmetic and square roots. Parry diagonalizes through glamx's
/// closed-form solver, whose `acos` and `cos` come from the platform's math library and so
/// differ by an ulp between native code and WASM on some cakes, after which their worlds part.
pub(super) fn calc_mass_properties(
    density_kg_per_m3: f32,
    positions: &[Vector],
    triangles: &[[u32; 3]],
) -> MassProperties {
    let (volume, com) = trimesh_signed_volume_and_center_of_mass(positions, triangles);
    debug_assert!(volume != 0.0, "CakeModel::validate rejects a mesh enclosing nothing");
    let mut inertia = Mat3::ZERO;
    for triangle in triangles {
        let [a, b, c] = triangle.map(|i| positions[i as usize]);
        let tetrahedron_volume = Tetrahedron::new(com, a, b, c).signed_volume();
        inertia += tetrahedron_unit_inertia_tensor_wrt_point(com, com, a, b, c) * tetrahedron_volume;
    }
    let sign = volume.signum();
    let (principal, frame) = calc_principal_axes((inertia * density_kg_per_m3 * sign).as_dmat3());
    MassProperties::with_principal_inertia_frame(com, volume * density_kg_per_m3 * sign, principal, frame)
}

/// The eigenvalues of the symmetric `inertia` and the rotation whose columns are their
/// eigenvectors, negative eigenvalues dropped to 0 as parry does.
fn calc_principal_axes(inertia: DMat3) -> (Vector, Quat) {
    let mut a = inertia.to_cols_array_2d();
    let mut axes = DMat3::IDENTITY.to_cols_array_2d();
    for _ in 0..JACOBI_SWEEPS {
        for (p, q) in [(0, 1), (0, 2), (1, 2)] {
            rotate_away(&mut a, &mut axes, p, q);
        }
    }
    let mut axes = DMat3::from_cols_array_2d(&axes);
    if axes.determinant() < 0.0 {
        axes.z_axis = -axes.z_axis;
    }
    let principal = DVec3::new(a[0][0], a[1][1], a[2][2]).max(DVec3::ZERO).as_vec3();
    (principal, Quat::from_mat3(&axes.as_mat3()).normalize())
}

/// One Jacobi rotation in the `p`-`q` plane, `a` turned to `R^T a R` so that `a[p][q]` becomes 0,
/// and the columns of `axes` turned by `R` with it.
fn rotate_away(a: &mut [[f64; 3]; 3], axes: &mut [[f64; 3]; 3], p: usize, q: usize) {
    if a[p][q] == 0.0 {
        return;
    }
    let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
    let t = theta.signum() / (theta.abs() + libm::sqrt(theta * theta + 1.0));
    let cos = 1.0 / libm::sqrt(t * t + 1.0);
    let sin = t * cos;
    for row in a.iter_mut() {
        (row[p], row[q]) = (cos * row[p] - sin * row[q], sin * row[p] + cos * row[q]);
    }
    (a[p], a[q]) = calc_turned_pair(a[p], a[q], cos, sin);
    (axes[p], axes[q]) = calc_turned_pair(axes[p], axes[q], cos, sin);
}

fn calc_turned_pair(u: [f64; 3], v: [f64; 3], cos: f64, sin: f64) -> ([f64; 3], [f64; 3]) {
    (core::array::from_fn(|k| cos * u[k] - sin * v[k]), core::array::from_fn(|k| sin * u[k] + cos * v[k]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mass_properties_match_parrys_on_a_box_turned_off_every_axis() {
        let turn = Quat::from_xyzw(0.3, -0.5, 0.2, 0.78).normalize();
        let corners = (0..8).map(|i| {
            let corner = Vector::new([0.0, 0.1][i & 1], [0.0, 0.04][(i >> 1) & 1], [0.0, 0.07][(i >> 2) & 1]);
            turn * corner + Vector::new(0.4, 0.0, 0.1)
        });
        let positions: Vec<Vector> = corners.collect();
        let quads = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
        let triangles: Vec<[u32; 3]> = quads.iter().flat_map(|&[a, b, c, d]| [[a, b, c], [a, c, d]]).collect();
        let ours = calc_mass_properties(1000.0, &positions, &triangles);
        let parrys = MassProperties::from_trimesh(1000.0, &positions, &triangles);
        assert_eq!(ours.mass(), parrys.mass());
        assert_eq!(ours.local_com, parrys.local_com);
        let (ours, parrys) = (ours.reconstruct_inertia_matrix(), parrys.reconstruct_inertia_matrix());
        let scale = parrys.abs().to_cols_array().into_iter().fold(0.0, f32::max);
        assert!((ours - parrys).abs().to_cols_array().iter().all(|d| *d < scale * 1e-5), "{ours} vs {parrys}");
    }
}
