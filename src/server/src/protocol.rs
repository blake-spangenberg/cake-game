//! The WebSocket messages between the client (`src/client/src/net.ts`) and the server, which
//! change together.
//!
//! Control messages are JSON text frames tagged by `type`; the two per-tick
//! streams, inputs up and poses down, are little-endian binary frames.

use cake_core::input::{ENCODED_LEN, Input, InputError};
use cake_core::sim::{Phase, World};
use serde::{Deserialize, Serialize};

pub type PlayerId = u32;

/// The cake a host picks for one the server settles at random.
pub const RANDOM_CAKE: &str = "random";

/// An input frame: the tick the input steps the world from, then [`Input::encode`]'s values,
/// each a little-endian `i32`.
pub const INPUT_FRAME_SIZE: usize = 4 * (1 + ENCODED_LEN);
/// One player's record in a pose frame, which is these records back to back.
///
/// A record is id, tick, and [`POSE_FLAG_HOLDING`] and [`POSE_FLAG_CUT`] as `u32`, then as `f32` the cake's translation and
/// rotation quaternion (x, y, z, qx, qy, qz, qw), the knife's tilt, lowering, and rest, and the
/// stand's angle, all little-endian.
pub const POSE_RECORD_SIZE: usize = 4 * 3 + 4 * 11;
pub const POSE_FLAG_HOLDING: u32 = 1 << 0;
pub const POSE_FLAG_CUT: u32 = 1 << 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// The first message on a connection, which makes it a player in the room, with their standup
    /// order preference from 0 (first to give their update) to 1 (last), and the `seat` token an
    /// earlier connection of theirs was welcomed with, to reopen into that seat.
    Join {
        name: String,
        standup_preference: f64,
        #[serde(default)]
        seat: Option<String>,
    },
    /// Makes the sender host, given the host password, in the lobby or the results.
    ClaimHost { password: String },
    /// The host picks the next round's cake by id, or `random` for one the server settles at once.
    Pick { cake: String },
    /// The host starts a round on the picked cake.
    Start,
    /// The host ends the results, bringing everyone back to the lobby.
    ReturnToLobby,
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
    /// The joining connection's own id, sent once, and the token that reopens its seat while the
    /// player has been inactive under an hour.
    Welcome { id: PlayerId, seat: String },
    /// To a page reopened into a seat in the round: every tick its world has stepped, as runs of
    /// the same input in [`Input::encode`]'s values, and the tick it cut at, if it has.
    Resume { runs: Vec<Run>, cut_tick: Option<u32> },
    /// Who is in the room, sent whenever that, the host, or the phase changes.
    Room { phase: PhaseName, host: Option<PlayerId>, members: Vec<Member> },
    /// The next round's cake, which every page in the lobby downloads without showing it, and
    /// whether the host picked Random, sent on joining the lobby and whenever it changes.
    Pick { cake: String, is_random: bool },
    /// A host claim with the wrong password.
    ClaimRefused,
    /// A round on `cake` for `players`, named as the round started, so a page joining after one left
    /// can still label them: each loads it and answers [`ClientMessage::Ready`]. Anyone else spectates.
    Round { cake: String, round_ticks: u32, players: Vec<Member> },
    /// The round's `players` whose page had not loaded the cake when it started, sent just before
    /// [`ServerMessage::Start`]: they leave the round and spectate it.
    SitOut { players: Vec<PlayerId> },
    /// Tick 0 of the round, `elapsed_ms` ago for a spectator joining late.
    Start { elapsed_ms: u64 },
    /// A player cut, with the frozen pose and lowerings the cut animation needs, but no score.
    Cut { id: PlayerId, tick: u32, pose: Pose, cut_lower_m: f64, cake_touch_lower_m: f64, is_cut_through: bool },
    /// Every round player has cut, so the whistle blows. The judging follows once every page's
    /// opponent views have had time to play the last cut.
    PlayOver,
    /// The judging reveal, which every page shows in step from `elapsed_ms` ago: each round player
    /// in the random `order`, `scene_ms` each, judged by their standing. Every player's cut, ranked,
    /// with the pose of each in its [`ServerMessage::Cut`].
    Judging { order: Vec<PlayerId>, scene_ms: u64, elapsed_ms: u64, standings: Vec<Standing> },
    /// Every player's cut ranked, once the judging reveal ends, and the order everyone in the room,
    /// spectators included, gives their standup update in.
    Results { standings: Vec<Standing>, standup: Vec<Member> },
    /// The client's world hash at `tick` differs from the server's replay.
    Desync { tick: u32 },
    /// A message the room refused.
    Error { message: String },
}

/// `ticks` stepped with the same `input`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub ticks: u32,
    pub input: [i32; ENCODED_LEN],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseName {
    Lobby,
    Loading,
    Playing,
    Judging,
    Results,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, PartialEq, Eq)]
pub enum FrameError {
    Size { size: usize },
    Input(InputError),
}

/// # Errors
/// [`FrameError`] for a frame of the wrong size or holding an invalid input.
pub fn decode_input_frame(bytes: &[u8]) -> Result<(u32, Input), FrameError> {
    let size_error = FrameError::Size { size: bytes.len() };
    if bytes.len() != INPUT_FRAME_SIZE {
        return Err(size_error);
    }
    let words: Vec<i32> = bytes.as_chunks::<4>().0.iter().map(|&word| i32::from_le_bytes(word)).collect();
    let (&tick, encoded) = words.split_first().ok_or(size_error)?;
    let input = Input::decode(encoded).map_err(FrameError::Input)?;
    Ok((tick.cast_unsigned(), input))
}

#[must_use]
pub fn encode_input_frame(tick: u32, input: &Input) -> Vec<u8> {
    let words = std::iter::once(tick.cast_signed()).chain(input.encode());
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
#[expect(clippy::cast_possible_truncation, reason = "the pose frame's wire format is f32")]
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
