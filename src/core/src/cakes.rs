use core::f32::consts::TAU;

use rapier3d::math::Vec3;
use rapier3d::prelude::SharedShape;

use crate::mesh::{Mesh, MeshError, calc_arc_outline};
use crate::table::{MAX_CAKE_EXTENT_M, MAX_CAKE_SIDE_M, STAND_RADIUS_M};

/// A cake's render mesh, how its surface is drawn, and the convex pieces its collider is built
/// from, in its local frame with the serving side down at y = 0.
#[derive(Clone, Debug, PartialEq)]
pub struct CakeModel {
    pub mesh: Mesh,
    /// Drawing only: weight and collision ignore it.
    pub skin: Skin,
    pub hulls: Vec<Vec<Vec3>>,
}

/// How a cake's surface is drawn.
#[derive(Clone, Debug, PartialEq)]
pub enum Skin {
    /// One frosting color, for a cake with no sourced coloring.
    Plain,
    /// An image wrapped over the mesh.
    Textured {
        /// Each triangle's corners' places in the image, in the mesh's triangle order, as glTF
        /// UVs from the image's top left scaled to the full `u16` range.
        corner_uvs: Vec<[[u16; 2]; 3]>,
        /// The image file as encoded, a WebP from `tools/repair-model.py`, decoded only by the client.
        image: Vec<u8>,
    },
}

/// The most vertices a collider hull may have: a hull face with many vertices jitters on the
/// table and never sleeps, so baked hulls stay coarse.
pub const MAX_HULL_VERTICES: usize = 32;
/// How far below the table or beyond the stand's rim a model's mesh may stray, absorbing float
/// rounding in the baked file.
const BOUNDS_SLACK_M: f32 = 1e-4;

const FILE_MAGIC: [u8; 4] = *b"CAKE";
const FILE_VERSION: u32 = 3;

#[derive(Debug, PartialEq)]
pub enum CakeError {
    Mesh(MeshError),
    /// The textured model does not have one UV triple per triangle, or has no image.
    BadSkin,
    /// The mesh is wound inside out, or encloses no volume.
    NotOutward,
    /// The model's bounding box is longer than [`MAX_CAKE_SIDE_M`] along an axis, or its extent
    /// bound exceeds [`MAX_CAKE_EXTENT_M`].
    TooLong,
    /// The model does not rest on the table's top at y = 0, or reaches past the stand's plate
    /// when centered on it.
    OffPlate,
    /// The hull at this index is missing, too coarse to hold, or has too many vertices.
    BadHull {
        hull_idx: usize,
    },
    /// The bytes are not a baked cake of this version, or end early.
    BadFile,
}

/// Smoothness of the drawn and weighed mesh.
const MESH_SEGMENTS_PER_TURN: f32 = 48.0;
/// Coarser for the collider: a hull face with many vertices resting on the table jitters in
/// Rapier and never sleeps, and facets keep a cake on its side from rolling far.
const HULL_SEGMENTS_PER_TURN: f32 = 16.0;

impl CakeModel {
    /// Accepts the model only if the game can play it: a closed, outward-wound mesh, textured with
    /// a UV triple per triangle if at all, that rests on y = 0, fits the stand's plate and the
    /// extent limit, with coarse, solid hulls.
    pub fn validate(&self) -> Result<(), CakeError> {
        self.mesh.validate_closed().map_err(CakeError::Mesh)?;
        if let Skin::Textured { corner_uvs, image } = &self.skin
            && (corner_uvs.len() != self.mesh.triangles.len() || image.is_empty())
        {
            return Err(CakeError::BadSkin);
        }
        if self.mesh.calc_signed_volume() <= 0.0 {
            return Err(CakeError::NotOutward);
        }
        let positions = &self.mesh.positions;
        let (min, max) = positions.iter().fold((positions[0], positions[0]), |(min, max), &p| (min.min(p), max.max(p)));
        if (max - min).max_element() > MAX_CAKE_SIDE_M as f32
            || self.mesh.calc_extent_bound() > MAX_CAKE_EXTENT_M as f32
        {
            return Err(CakeError::TooLong);
        }
        let radius_m = positions.iter().map(|p| libm::hypotf(p.x, p.z)).fold(0.0, f32::max);
        if min.y.abs() > BOUNDS_SLACK_M || radius_m > STAND_RADIUS_M as f32 + BOUNDS_SLACK_M {
            return Err(CakeError::OffPlate);
        }
        if self.hulls.is_empty() {
            return Err(CakeError::BadHull { hull_idx: 0 });
        }
        for (hull_idx, hull) in self.hulls.iter().enumerate() {
            if hull.len() > MAX_HULL_VERTICES || SharedShape::convex_hull(hull).is_none() {
                return Err(CakeError::BadHull { hull_idx });
            }
        }
        Ok(())
    }

    /// The model as a baked cake file: little-endian, a magic and version, the counts and the
    /// image's byte length (0 for a plain skin), then the mesh's positions, each hull's vertices,
    /// the triangles, and for a textured skin the corner UVs and the image.
    pub fn encode(&self) -> Vec<u8> {
        let (corner_uvs, image): (&[[[u16; 2]; 3]], &[u8]) = match &self.skin {
            Skin::Plain => (&[], &[]),
            Skin::Textured { corner_uvs, image } => (corner_uvs, image),
        };
        let lens = [self.mesh.positions.len(), self.mesh.triangles.len(), self.hulls.len(), image.len()];
        let header = [FILE_VERSION]
            .into_iter()
            .chain(lens.map(|len| len as u32))
            .chain(self.hulls.iter().map(|h| h.len() as u32));
        let points = self.mesh.positions.iter().chain(self.hulls.iter().flatten());
        let mut bytes = FILE_MAGIC.to_vec();
        bytes.extend(header.flat_map(u32::to_le_bytes));
        bytes.extend(points.flat_map(|p| p.to_array()).flat_map(f32::to_le_bytes));
        bytes.extend(self.mesh.triangles.iter().flatten().flat_map(|index| index.to_le_bytes()));
        bytes.extend(corner_uvs.iter().flatten().flatten().flat_map(|coord| coord.to_le_bytes()));
        bytes.extend(image);
        bytes
    }

    /// Reads a baked cake file and validates it, so a model that decodes is playable.
    pub fn decode(bytes: &[u8]) -> Result<CakeModel, CakeError> {
        let mut reader = Reader { bytes };
        if reader.take::<4>()? != FILE_MAGIC || reader.read_u32()? != FILE_VERSION {
            return Err(CakeError::BadFile);
        }
        let vertex_len = reader.read_len()?;
        let triangle_len = reader.read_len()?;
        let hull_len = reader.read_len()?;
        let image_len = reader.read_len()?;
        let hull_lens = (0..hull_len).map(|_| reader.read_len()).collect::<Result<Vec<_>, _>>()?;
        let positions = reader.read_vecs(vertex_len)?;
        let hulls = hull_lens.into_iter().map(|len| reader.read_vecs(len)).collect::<Result<Vec<_>, _>>()?;
        let triangles = (0..triangle_len)
            .map(|_| Ok([reader.read_u32()?, reader.read_u32()?, reader.read_u32()?]))
            .collect::<Result<Vec<_>, CakeError>>()?;
        let skin = if image_len == 0 {
            Skin::Plain
        } else {
            let mut read_uv = || Ok([reader.read_u16()?, reader.read_u16()?]);
            let corner_uvs = (0..triangle_len)
                .map(|_| Ok([read_uv()?, read_uv()?, read_uv()?]))
                .collect::<Result<Vec<_>, CakeError>>()?;
            let image = reader.bytes.get(..image_len).ok_or(CakeError::BadFile)?.to_vec();
            reader.bytes = &reader.bytes[image_len..];
            Skin::Textured { corner_uvs, image }
        };
        if !reader.bytes.is_empty() {
            return Err(CakeError::BadFile);
        }
        let model = CakeModel { mesh: Mesh { positions, triangles }, skin, hulls };
        model.validate()?;
        Ok(model)
    }
}

/// A cursor over a baked file's bytes.
struct Reader<'a> {
    bytes: &'a [u8],
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], CakeError> {
        let (head, tail) = self.bytes.split_first_chunk::<N>().ok_or(CakeError::BadFile)?;
        self.bytes = tail;
        Ok(*head)
    }

    fn read_u16(&mut self) -> Result<u16, CakeError> {
        self.take().map(u16::from_le_bytes)
    }

    fn read_u32(&mut self) -> Result<u32, CakeError> {
        self.take().map(u32::from_le_bytes)
    }

    fn read_len(&mut self) -> Result<usize, CakeError> {
        self.read_u32().map(|len| len as usize)
    }

    fn read_vecs(&mut self, len: usize) -> Result<Vec<Vec3>, CakeError> {
        let mut read_f32 = || self.take().map(f32::from_le_bytes);
        (0..len).map(|_| Ok(Vec3::new(read_f32()?, read_f32()?, read_f32()?))).collect()
    }
}

/// A placeholder until sourced models arrive: a round cake with a slice missing, under a smaller
/// tier set off center.
pub fn build_slice_missing_cake() -> CakeModel {
    let mut model = CakeModel { mesh: Mesh::default(), skin: Skin::Plain, hulls: Vec::new() };
    let missing_slice_start = 290.0_f32.to_radians();
    let split = missing_slice_start / 2.0;
    for (angle0, angle1) in [(0.0, split), (split, missing_slice_start)] {
        let build_outline = |segments_per_turn: f32| {
            let point_len = libm::ceilf(segments_per_turn * (angle1 - angle0) / TAU) as u32 + 1;
            let mut outline = vec![[0.0, 0.0]];
            outline.extend(calc_arc_outline([0.0, 0.0], 0.11, angle0, angle1, point_len));
            outline
        };
        add_prism(&mut model, build_outline, 0.0, 0.07);
    }
    let top_offset_angle = 145.0_f32.to_radians();
    let top_center = [0.03 * libm::cosf(top_offset_angle), -0.03 * libm::sinf(top_offset_angle)];
    let build_top_outline = |segments_per_turn: f32| {
        let point_len = segments_per_turn as u32;
        calc_arc_outline(top_center, 0.055, 0.0, TAU * (1.0 - 1.0 / segments_per_turn), point_len)
    };
    add_prism(&mut model, build_top_outline, 0.07, 0.12);
    model
}

/// Adds a convex prism to the mesh, and a coarser copy as one of the collider's hulls.
fn add_prism(model: &mut CakeModel, build_outline: impl Fn(f32) -> Vec<[f32; 2]>, y0: f32, y1: f32) {
    model.mesh.push_prism(&build_outline(MESH_SEGMENTS_PER_TURN), y0, y1);
    let hull =
        build_outline(HULL_SEGMENTS_PER_TURN).into_iter().flat_map(|[x, z]| [Vec3::new(x, y0, z), Vec3::new(x, y1, z)]);
    model.hulls.push(hull.collect());
}
