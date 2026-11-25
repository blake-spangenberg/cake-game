//! One player's world on the server, replaying the tick-stamped messages their client sent in
//! the order it sent them.

use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;

use cake_core::cut::Split;
use cake_core::input::{Input, KnifeInput, quantize_radians};
use cake_core::knife::START_TILT_RAD;
use cake_core::sim::{Cake, Phase, TICKS_PER_S, World};

enum Entry {
    Input { tick: u32, input: Input },
    Cut { tick: u32 },
    Hash { tick: u32, hash: u64 },
}

/// The fewest ticks between two resyncs of one page.
pub const MIN_RESYNC_TICKS: u32 = TICKS_PER_S;

/// A message the replay could not take as stamped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Note {
    /// The client's world hash at `tick` differs from the replay's.
    Desync { tick: u32 },
    /// `len` messages of one `kind`, stamped for ticks from `first_tick` to `last_tick`, arrived
    /// with the replay at `at_tick`. An input or cut stamped later is reached by repeating the
    /// last input, and an earlier input is dropped while an earlier cut happens at once. A hash
    /// stamped for any other tick goes unchecked. One note covers every such message of a kind
    /// at the same `at_tick` in a drain, so a burst of stale inputs logs once.
    OutOfStep { kind: &'static str, first_tick: u32, last_tick: u32, len: u32, at_tick: u32 },
    /// `len` messages of one `kind`, stamped for ticks from `first_tick` to `last_tick`, were
    /// refused as stamped too far past the round's clock, which read `clock_tick` at the last.
    /// One note covers every such message of a kind between two drains.
    Refused { kind: &'static str, first_tick: u32, last_tick: u32, len: u32, clock_tick: u32 },
    /// An input stamped for `tick`, which the replay had already stepped with another input
    /// since the page last rebuilt its world, arrived: the page's world has parted from the
    /// replay's there. Noted once per drain, for the first such tick.
    Parted { tick: u32 },
}

impl fmt::Display for Note {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Self::Desync { tick } => write!(f, "world hash differs at tick {tick}"),
            Self::OutOfStep { kind, first_tick, len: 1, at_tick, .. } => {
                write!(f, "{kind} stamped tick {first_tick} arrived at tick {at_tick}")
            }
            Self::OutOfStep { kind, first_tick, last_tick, len, at_tick } => {
                write!(f, "{len} of {kind} stamped ticks {first_tick} to {last_tick} arrived at tick {at_tick}")
            }
            Self::Refused { kind, first_tick, len: 1, clock_tick, .. } => {
                write!(f, "{kind} stamped tick {first_tick} refused at round tick {clock_tick}")
            }
            Self::Refused { kind, first_tick, last_tick, len, clock_tick } => {
                write!(
                    f,
                    "{len} of {kind} stamped ticks {first_tick} to {last_tick} refused by round tick {clock_tick}"
                )
            }
            Self::Parted { tick } => write!(f, "world parted from the page's at tick {tick}"),
        }
    }
}

/// Notes that a `kind` of message stamped for `tick` arrived with the replay at `at_tick`, in the
/// note already made for that kind and `at_tick` if there is one.
fn note_out_of_step(notes: &mut Vec<Note>, kind: &'static str, tick: u32, at_tick: u32) {
    let same = notes.iter_mut().rev().find(|note| {
        matches!(note, Note::OutOfStep { kind: noted_kind, at_tick: noted_at, .. } if *noted_kind == kind && *noted_at == at_tick)
    });
    let Some(Note::OutOfStep { first_tick, last_tick, len, .. }) = same else {
        notes.push(Note::OutOfStep { kind, first_tick: tick, last_tick: tick, len: 1, at_tick });
        return;
    };
    *first_tick = (*first_tick).min(tick);
    *last_tick = (*last_tick).max(tick);
    *len += 1;
}

pub struct Replay {
    world: World,
    queue: VecDeque<Entry>,
    last_input: Input,
    /// The newest input queued and the tick it was stamped for.
    last_pushed: Option<(u32, Input)>,
    /// The input each tick so far stepped with, so a page reopened mid-round can rebuild the world.
    history: Vec<Input>,
    has_touched: bool,
    /// Notes made since the last drain, which it returns first.
    pending_notes: Vec<Note>,
    /// The tick at which the page was last sent the steps so far to rebuild its world from. An
    /// input for an earlier tick still arriving left the page before it rebuilt, so it parts nothing.
    resynced_tick: u32,
    /// Whether the page's world has parted from this one since the last resync.
    is_parted: bool,
}

/// A run of ticks stepped with the same input, as a reopened page replays them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputRun {
    pub ticks: u32,
    pub input: Input,
}

impl Replay {
    /// A fresh world on `cake` that cuts itself at `round_ticks`.
    #[must_use]
    pub fn create(cake: Arc<Cake>, round_ticks: u32) -> Self {
        Self {
            world: World::create(cake, START_TILT_RAD, Some(round_ticks)),
            queue: VecDeque::new(),
            last_input: calc_idle_input(),
            last_pushed: None,
            history: Vec::new(),
            has_touched: false,
            pending_notes: Vec::new(),
            resynced_tick: 0,
            is_parted: false,
        }
    }

    /// Whether the page's world has parted from this one since its last resync, and the page is
    /// to be sent every step so far to rebuild its world from, as [`Replay::build_input_runs`]
    /// gives them, which this notes as its resync. A parting within [`MIN_RESYNC_TICKS`] of the
    /// last resync stays due until they have passed, so a page whose network stays slow is not
    /// flooded, yet is always told. A cut world is never due, since the results settle it.
    pub const fn try_note_resync(&mut self) -> bool {
        let tick = self.world.get_tick();
        let is_too_soon = self.resynced_tick > 0 && tick < self.resynced_tick + MIN_RESYNC_TICKS;
        if !self.is_parted || self.is_cut() || is_too_soon {
            return false;
        }
        self.is_parted = false;
        self.resynced_tick = tick;
        true
    }

    /// Notes that a `kind` of message stamped for `tick` was refused with the round's clock at
    /// `clock_tick`, in the note already made for that kind since the last drain if there is one.
    pub fn note_refused(&mut self, kind: &'static str, tick: u32, clock_tick: u32) {
        let same = self
            .pending_notes
            .iter_mut()
            .find(|note| matches!(note, Note::Refused { kind: noted_kind, .. } if *noted_kind == kind));
        let Some(Note::Refused { first_tick, last_tick, len, clock_tick: noted_clock_tick, .. }) = same else {
            self.pending_notes.push(Note::Refused { kind, first_tick: tick, last_tick: tick, len: 1, clock_tick });
            return;
        };
        *first_tick = (*first_tick).min(tick);
        *last_tick = (*last_tick).max(tick);
        *len += 1;
        *noted_clock_tick = clock_tick;
    }

    pub fn push_input(&mut self, tick: u32, input: Input) {
        self.last_pushed = Some((tick, input));
        self.queue.push_back(Entry::Input { tick, input });
    }

    /// The newest input queued, if any has been.
    pub fn find_last_pushed_input(&self) -> Option<Input> {
        self.last_pushed.map(|(_, input)| input)
    }

    /// Lets go of the cake and the knife after the newest input, as a released mouse does, for a
    /// player whose page has gone. The world then holds the released input.
    pub fn push_release(&mut self) {
        let (tick, input) = match self.last_pushed {
            Some((tick, input)) => (tick + 1, input),
            None => (0, self.last_input),
        };
        self.push_input(tick.max(self.world.get_tick()), calc_released_input(input));
    }

    /// Every tick stepped so far, as runs of the same input.
    pub fn build_input_runs(&self) -> Vec<InputRun> {
        let mut runs: Vec<InputRun> = Vec::new();
        for &input in &self.history {
            match runs.last_mut() {
                Some(run) if run.input == input => run.ticks += 1,
                _ => runs.push(InputRun { ticks: 1, input }),
            }
        }
        runs
    }

    pub fn push_cut(&mut self, tick: u32) {
        self.queue.push_back(Entry::Cut { tick });
    }

    pub fn push_hash(&mut self, tick: u32, hash: u64) {
        self.queue.push_back(Entry::Hash { tick, hash });
    }

    /// Replays every queued message, returning the notes made since the last drain. Inputs after
    /// the cut are dropped.
    pub fn drain(&mut self) -> Vec<Note> {
        let mut notes = std::mem::take(&mut self.pending_notes);
        while let Some(entry) = self.queue.pop_front() {
            match entry {
                Entry::Input { tick, input } => self.replay_input(tick, input, &mut notes),
                Entry::Cut { tick } => self.replay_cut(tick, &mut notes),
                Entry::Hash { tick, hash } => self.check_hash(tick, hash, &mut notes),
            }
        }
        notes
    }

    /// Steps with the last input until the world reaches `tick` or cuts itself at the round's end,
    /// as for ticks a client has not sent in time. Inputs for those ticks arriving later are stale.
    pub fn hold_to(&mut self, tick: u32) {
        while self.world.get_tick() < tick && !self.is_cut() {
            self.step(self.last_input);
        }
    }

    pub const fn get_world(&self) -> &World {
        &self.world
    }

    /// The cut's tick and volumes, once cut.
    pub const fn find_cut(&self) -> Option<(u32, Split)> {
        match self.world.get_phase() {
            Phase::Playing => None,
            Phase::Cut { tick, split } => Some((tick, split)),
        }
    }

    pub const fn is_cut(&self) -> bool {
        self.find_cut().is_some()
    }

    /// Whether any input moved a control, or the player pressed `Cut!`.
    pub const fn has_touched(&self) -> bool {
        self.has_touched
    }

    fn replay_input(&mut self, tick: u32, input: Input, notes: &mut Vec<Note>) {
        if self.is_cut() {
            return;
        }
        let at_tick = self.world.get_tick();
        if tick != at_tick {
            note_out_of_step(notes, "input", tick, at_tick);
        }
        if tick < at_tick {
            let is_parted = tick >= self.resynced_tick && self.history[tick as usize] != input;
            if is_parted && !notes.iter().any(|note| matches!(note, Note::Parted { .. })) {
                notes.push(Note::Parted { tick });
            }
            self.is_parted |= is_parted;
            return;
        }
        self.hold_to(tick);
        if self.is_cut() {
            return;
        }
        self.has_touched |= input != calc_idle_input();
        self.last_input = input;
        self.step(input);
    }

    fn step(&mut self, input: Input) {
        self.history.push(input);
        self.world.step(&input);
    }

    fn replay_cut(&mut self, tick: u32, notes: &mut Vec<Note>) {
        if self.is_cut() {
            return;
        }
        let at_tick = self.world.get_tick();
        if tick != at_tick {
            note_out_of_step(notes, "cut", tick, at_tick);
        }
        self.hold_to(tick);
        self.has_touched = true;
        self.world.cut();
    }

    fn check_hash(&self, tick: u32, hash: u64, notes: &mut Vec<Note>) {
        let at_tick = self.world.get_tick();
        if tick != at_tick {
            note_out_of_step(notes, "hash", tick, at_tick);
            return;
        }
        if hash != self.world.calc_hash() {
            notes.push(Note::Desync { tick });
        }
    }
}

/// `input` with the hand let go and the knife eased back up to its rest, keeping its tilt and the stand's angle.
#[must_use]
pub const fn calc_released_input(input: Input) -> Input {
    Input { hand: None, knife: KnifeInput { lower: 0, ..input.knife }, stand_angle: input.stand_angle }
}

/// What the client sends before the player moves anything: no hand, and the knife and stand
/// where they start.
#[must_use]
pub fn calc_idle_input() -> Input {
    Input { hand: None, knife: KnifeInput { tilt: quantize_radians(START_TILT_RAD), lower: 0 }, stand_angle: 0 }
}
