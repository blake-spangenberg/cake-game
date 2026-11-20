//! Player input for one tick, quantized to integers so every simulation sees identical values.

pub const POSITION_UNIT_M: f64 = 1.0e-5;
pub const ANGLE_UNIT_RAD: f64 = 1.0e-5;

/// Length of [`Input::encode`]'s output, the wire format shared with the client.
pub const ENCODED_LEN: usize = 8;

const FLAG_HOLDING: i32 = 1 << 0;
const FLAG_LOWERING: i32 = 1 << 1;
const FLAGS_KNOWN: i32 = FLAG_HOLDING | FLAG_LOWERING;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Input {
    pub hand: Option<HandInput>,
    pub knife: KnifeInput,
}

/// The cake held at `grab_at`, a world point on its surface where the grab began, with the hand
/// steering toward the table point `target_xz`. A new `grab_at` while holding regrabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HandInput {
    pub grab_at: [i32; 3],
    pub target_xz: [i32; 2],
}

/// Knife angles in [`ANGLE_UNIT_RAD`]: tilt from vertical about the blade's edge, and heading
/// of the edge about the vertical axis. Tilt is clamped by the simulation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KnifeInput {
    pub tilt: i32,
    pub heading: i32,
    pub is_lowering: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum InputError {
    Length { len: usize },
    UnknownFlags { flags: i32 },
}

impl Input {
    pub fn encode(&self) -> [i32; ENCODED_LEN] {
        let mut flags = 0;
        if self.knife.is_lowering {
            flags |= FLAG_LOWERING;
        }
        let (grab_at, target_xz) = match self.hand {
            Some(hand) => {
                flags |= FLAG_HOLDING;
                (hand.grab_at, hand.target_xz)
            }
            None => ([0; 3], [0; 2]),
        };
        [flags, grab_at[0], grab_at[1], grab_at[2], target_xz[0], target_xz[1], self.knife.tilt, self.knife.heading]
    }

    pub fn decode(encoded: &[i32]) -> Result<Input, InputError> {
        let &[flags, grab_x, grab_y, grab_z, target_x, target_z, tilt, heading] = encoded else {
            return Err(InputError::Length { len: encoded.len() });
        };
        if flags & !FLAGS_KNOWN != 0 {
            return Err(InputError::UnknownFlags { flags });
        }
        let hand = (flags & FLAG_HOLDING != 0)
            .then_some(HandInput { grab_at: [grab_x, grab_y, grab_z], target_xz: [target_x, target_z] });
        Ok(Input { hand, knife: KnifeInput { tilt, heading, is_lowering: flags & FLAG_LOWERING != 0 } })
    }
}

pub fn calc_meters(quantized: i32) -> f64 {
    quantized as f64 * POSITION_UNIT_M
}

pub fn calc_radians(quantized: i32) -> f64 {
    quantized as f64 * ANGLE_UNIT_RAD
}
