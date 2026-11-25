//! The WebSocket messages between the client (`src/client/src/net.ts`) and the server, which
//! change together. Control messages are JSON text frames tagged by `type`; the two per-tick
//! streams, inputs up and poses down, are little-endian binary frames.

use cake_core::input::{ENCODED_LEN, Input, InputError};
use cake_core::sim::{Phase, World};
use serde::{Deserialize, Serialize};

pub type PlayerId = u32;

/// An input frame: the tick the input steps the world from, then [`Input::encode`]'s values,
/// each a little-endian `i32`.
pub const INPUT_FRAME_SIZE: usize = 4 * (1 + ENCODED_LEN);
/// One player's record in a pose frame, which is these records back to back: id, tick, and
/// [`POSE_FLAG_HOLDING`] and [`POSE_FLAG_CUT`] as `u32`, then as `f32` the cake's translation and
/// rotation quaternion (x, y, z, qx, qy, qz, qw), the knife's tilt, lowering, and rest, and the
/// stand's angle, all little-endian.
pub const POSE_RECORD_SIZE: usize = 4 * 3 + 4 * 11;
pub const POSE_FLAG_HOLDING: u32 = 1 << 0;
pub const POSE_FLAG_CUT: u32 = 1 << 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// The first message on a connection, which makes it a player in the room.
    Join { name: String },
    /// The host starts a round on the cake with this id, or `random`.
    Start { cake: String },
    /// The round's cake is loaded, so the round may start.
    Ready,
    /// `Cut!` pressed with the world at `tick`, after the inputs before it.
    Cut { tick: u32 },
    /// [`World::calc_hash`] at `tick`, in decimal, since JSON numbers cannot hold every `u64`.
    Hash { tick: u32, hash: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// The joining connection's own id, sent once.
    Welcome { id: PlayerId },
    /// Who is in the room, sent whenever that, the host, or the phase changes.
    Room { phase: PhaseName, host: Option<PlayerId>, members: Vec<Member> },
    /// A round on `cake` for `players`: each loads it and answers [`ClientMessage::Ready`]. Anyone
    /// else spectates.
    Round { cake: String, round_ticks: u32, players: Vec<PlayerId> },
    /// Tick 0 of the round, `elapsed_ms` ago for a spectator joining late.
    Start { elapsed_ms: u64 },
    /// A player cut, with the frozen pose and lowerings the cut animation needs, but no score.
    Cut { id: PlayerId, tick: u32, pose: Pose, cut_lower_m: f64, cake_touch_lower_m: f64, is_cut_through: bool },
    /// Every player's cut ranked, once all have cut or time ran out.
    Results { standings: Vec<Standing> },
    /// The client's world hash at `tick` differs from the server's replay.
    Desync { tick: u32 },
    /// A message the room refused.
    Error { message: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseName {
    Lobby,
    Loading,
    Playing,
    Results,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Member {
    pub id: PlayerId,
    pub name: String,
}

/// A world's drawable state: the cake's translation and rotation quaternion, the knife's tilt
/// (radians), lowering below the pivot, and rest (meters), and the stand's angle (radians).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pose {
    pub cake: [f32; 7],
    pub knife: [f64; 3],
    pub stand_rad: f64,
}

/// One player's cut in the results. Equal ranks are shared: a higher unrounded score ranks first,
/// and of equal scores the earlier cut.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Standing {
    pub id: PlayerId,
    pub name: String,
    pub rank: u32,
    pub score: f64,
    pub above_m3: f64,
    pub below_m3: f64,
    pub tick: u32,
    /// Whether the player moved any control or pressed `Cut!`.
    pub has_touched: bool,
}

#[derive(Debug, PartialEq)]
pub enum FrameError {
    Size { size: usize },
    Input(InputError),
}

pub fn decode_input_frame(bytes: &[u8]) -> Result<(u32, Input), FrameError> {
    if bytes.len() != INPUT_FRAME_SIZE {
        return Err(FrameError::Size { size: bytes.len() });
    }
    let words: Vec<i32> = bytes.chunks_exact(4).map(|word| i32::from_le_bytes(word.try_into().unwrap())).collect();
    let (&tick, encoded) = words.split_first().unwrap();
    let input = Input::decode(encoded).map_err(FrameError::Input)?;
    Ok((tick as u32, input))
}

pub fn encode_input_frame(tick: u32, input: &Input) -> Vec<u8> {
    let words = [tick as i32].into_iter().chain(input.encode());
    words.flat_map(i32::to_le_bytes).collect()
}

pub fn build_pose(world: &World) -> Pose {
    let cake = world.get_cake_pose();
    let mut cake_values = [0.0; 7];
    cake_values[..3].copy_from_slice(&cake.translation.to_array());
    cake_values[3..].copy_from_slice(&cake.rotation.to_array());
    let knife = world.get_knife();
    Pose {
        cake: cake_values,
        knife: [knife.tilt_rad, knife.lower_m, knife.rest_m],
        stand_rad: world.get_stand_angle_rad(),
    }
}

/// Appends `world`'s record to a pose frame.
pub fn encode_pose_record(frame: &mut Vec<u8>, id: PlayerId, world: &World) {
    let is_cut = matches!(world.get_phase(), Phase::Cut { .. });
    let flags = if world.is_holding() { POSE_FLAG_HOLDING } else { 0 } | if is_cut { POSE_FLAG_CUT } else { 0 };
    for word in [id, world.get_tick(), flags] {
        frame.extend(word.to_le_bytes());
    }
    let pose = build_pose(world);
    let values = pose.cake.into_iter().chain(pose.knife.map(|value| value as f32)).chain([pose.stand_rad as f32]);
    for value in values {
        frame.extend(value.to_le_bytes());
    }
}
