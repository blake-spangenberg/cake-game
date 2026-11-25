//! Player input for one tick, quantized to integers so every simulation sees identical values.

pub const POSITION_UNIT_M: f64 = 1.0e-5;
pub const ANGLE_UNIT_RAD: f64 = 1.0e-5;

/// Length of [`Input::encode`]'s output, the wire format shared with the client.
pub const ENCODED_LEN: usize = 9;

const FLAG_HOLDING: i32 = 1 << 0;
const FLAGS_KNOWN: i32 = FLAG_HOLDING;

/// `stand_angle` turns the cake stand about the vertical axis, in [`ANGLE_UNIT_RAD`]. The
/// simulation limits how fast the stand follows it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Input {
    pub hand: Option<HandInput>,
    pub knife: KnifeInput,
    pub stand_angle: i32,
}

/// The cake held at `grab_at`, a world point on its surface where the grab began, with the hand
/// steering toward the table point `target_xz`. A new `grab_at` while holding regrabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HandInput {
    pub grab_at: [i32; 3],
    pub target_xz: [i32; 2],
}

/// Knife tilt from vertical about the blade's edge in [`ANGLE_UNIT_RAD`], and how far to lower
/// it in [`POSITION_UNIT_M`]. The simulation clamps both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KnifeInput {
    pub tilt: i32,
    pub lower: i32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum InputError {
    Length { len: usize },
    UnknownFlags { flags: i32 },
}

impl Input {
    #[must_use]
    pub const fn encode(&self) -> [i32; ENCODED_LEN] {
        let (flags, [grab_x, grab_y, grab_z], [target_x, target_z]) = match self.hand {
            Some(hand) => (FLAG_HOLDING, hand.grab_at, hand.target_xz),
            None => (0, [0; 3], [0; 2]),
        };
        [flags, grab_x, grab_y, grab_z, target_x, target_z, self.knife.tilt, self.knife.lower, self.stand_angle]
    }

    /// # Errors
    /// [`InputError`] for a slice of the wrong length or with flags this version does not know.
    pub fn decode(encoded: &[i32]) -> Result<Self, InputError> {
        let &[flags, grab_x, grab_y, grab_z, target_x, target_z, tilt, lower, stand_angle] = encoded else {
            return Err(InputError::Length { len: encoded.len() });
        };
        if flags & !FLAGS_KNOWN != 0 {
            return Err(InputError::UnknownFlags { flags });
        }
        let hand = (flags & FLAG_HOLDING != 0)
            .then_some(HandInput { grab_at: [grab_x, grab_y, grab_z], target_xz: [target_x, target_z] });
        Ok(Self { hand, knife: KnifeInput { tilt, lower }, stand_angle })
    }
}

#[must_use]
pub fn calc_meters(quantized: i32) -> f64 {
    f64::from(quantized) * POSITION_UNIT_M
}

#[must_use]
pub fn calc_radians(quantized: i32) -> f64 {
    f64::from(quantized) * ANGLE_UNIT_RAD
}

#[must_use]
#[expect(clippy::cast_possible_truncation, reason = "the wire format's saturating quantization")]
pub fn quantize_meters(meters: f64) -> i32 {
    (meters / POSITION_UNIT_M).round() as i32
}

#[must_use]
#[expect(clippy::cast_possible_truncation, reason = "the wire format's saturating quantization")]
pub fn quantize_radians(radians: f64) -> i32 {
    (radians / ANGLE_UNIT_RAD).round() as i32
}
