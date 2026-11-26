//! Simulated players a `?debug` page adds to the room, which play every round they are in.
//!
//! As a page would, each carries the cake onto the stand, sways the knife's tilt and the stand's
//! angle, and cuts at a random tick, its messages reaching the room a set latency late each way.

use std::f64::consts::TAU;
use std::hash::{BuildHasher, RandomState};

use cake_core::cakes::CakeModel;
use cake_core::input::{HandInput, Input, KnifeInput, quantize_meters, quantize_radians};
use cake_core::knife::MAX_TILT_RAD;
use cake_core::replay::calc_spawned_top_grab_at;

use crate::replay::Replay;

/// The ticks of a bot's play, from its first, during which it carries the cake.
const CARRY_TICKS: std::ops::Range<u32> = 20..240;
/// Where the carry steers, just past the stand's center so the cake lands on the plate.
const CARRY_TARGET_M: [f64; 2] = [0.0, 0.02];
/// The tick of a bot's play from which it sways the knife and the stand, having eased the knife
/// from its start tilt to upright until then.
const SWAY_START_TICK: u32 = 300;
const TILT_SWAY_RAD: f64 = 0.25;
const STAND_SWAY_RAD: f64 = 0.3;
/// The sways' periods in ticks, each bot's a little longer than the one before, so bots differ.
const TILT_PERIOD_TICKS: u32 = 180;
const TILT_PERIOD_STEP_TICKS: u32 = 30;
const STAND_PERIOD_TICKS: u32 = 400;
const STAND_PERIOD_STEP_TICKS: u32 = 50;
/// The shares of its time left at which a bot cuts, chosen at random between them.
const CUT_SHARE_RANGE: (f64, f64) = (0.5, 0.95);

/// One bot's play of a round, from the room's sending it the round to its cut.
pub struct BotPlay {
    /// When the bot's `ready` reaches the room, none once it has.
    ready_at_ms: Option<u64>,
    /// The tick the bot's script starts from, the first its inputs reach once it has loaded, and
    /// the tick it cuts at, none until it plays.
    script: Option<(u32, u32)>,
    /// The next tick the bot sends an input for.
    next_tick: u32,
    is_cut: bool,
    grab_at: [i32; 3],
}

impl BotPlay {
    /// A bot's play of a round on `model`, its `ready` reaching the room at `ready_at_ms`.
    #[must_use]
    pub fn create(model: &CakeModel, ready_at_ms: u64) -> Self {
        Self {
            ready_at_ms: Some(ready_at_ms),
            script: None,
            next_tick: 0,
            is_cut: false,
            grab_at: calc_spawned_top_grab_at(model),
        }
    }

    /// Whether the bot's `ready` reaches the room by `now_ms`, which the room takes once.
    pub fn try_take_ready(&mut self, now_ms: u64) -> bool {
        if self.ready_at_ms.is_none_or(|at_ms| now_ms < at_ms) {
            return false;
        }
        self.ready_at_ms = None;
        true
    }

    /// Queues in `replay` the inputs, and the cut, that bot `number` sent for every tick before
    /// `arrived_tick` of a round `round_ticks` long, those that have reached the room, but none for
    /// a tick more than `max_lead` ticks past `replay`'s world, so a bot that loaded late is not
    /// replayed far ahead in one step. Its script starts at the first tick it sends.
    pub fn push_arrived(
        &mut self,
        replay: &mut Replay,
        number: u32,
        arrived_tick: u32,
        round_ticks: u32,
        max_lead: u32,
    ) {
        if self.ready_at_ms.is_some() || self.is_cut {
            return;
        }
        let (start_tick, cut_tick) = *self.script.get_or_insert_with(|| {
            let ticks_left = round_ticks.saturating_sub(arrived_tick);
            (arrived_tick, arrived_tick + calc_cut_after_ticks(ticks_left))
        });
        self.next_tick = self.next_tick.max(start_tick);
        let end_tick = arrived_tick.min(replay.get_world().get_tick() + max_lead).min(round_ticks);
        while self.next_tick < end_tick {
            replay.push_input(self.next_tick, build_input(number, self.next_tick - start_tick, self.grab_at));
            self.next_tick += 1;
            if self.next_tick == cut_tick {
                replay.push_cut(cut_tick);
                self.is_cut = true;
                return;
            }
        }
    }
}

/// A bot's standup order preference, at random from 0 to 1.
#[must_use]
pub fn pick_standup_preference() -> f64 {
    pick_share()
}

/// How many ticks after its start a bot with `ticks_left` cuts, at random within [`CUT_SHARE_RANGE`].
#[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a share of a round's ticks")]
fn calc_cut_after_ticks(ticks_left: u32) -> u32 {
    let (low, high) = CUT_SHARE_RANGE;
    let share = low + (high - low) * pick_share();
    ((f64::from(ticks_left) * share) as u32).max(1)
}

/// A number at random from 0 up to 1.
#[expect(clippy::cast_precision_loss, reason = "the 53 bits a double holds exactly")]
fn pick_share() -> f64 {
    (RandomState::new().hash_one(()) >> 11) as f64 / (1_u64 << 53) as f64
}

/// Bot `number`'s input on tick `tick` of its play, grabbing the cake at `grab_at`.
fn build_input(number: u32, tick: u32, grab_at: [i32; 3]) -> Input {
    let hand =
        CARRY_TICKS.contains(&tick).then(|| HandInput { grab_at, target_xz: CARRY_TARGET_M.map(quantize_meters) });
    let sway_ticks = f64::from(tick.saturating_sub(SWAY_START_TICK));
    let lead = (f64::from(tick) / f64::from(SWAY_START_TICK)).min(1.0);
    let tilt_period = f64::from(TILT_PERIOD_TICKS + TILT_PERIOD_STEP_TICKS * number);
    let stand_period = f64::from(STAND_PERIOD_TICKS + STAND_PERIOD_STEP_TICKS * number);
    let tilt_rad = (1.0 - lead) * MAX_TILT_RAD + TILT_SWAY_RAD * (TAU * sway_ticks / tilt_period).sin();
    let stand_rad = STAND_SWAY_RAD * (TAU * sway_ticks / stand_period).sin();
    Input {
        hand,
        knife: KnifeInput { tilt: quantize_radians(tilt_rad), lower: 0 },
        stand_angle: quantize_radians(stand_rad),
    }
}
