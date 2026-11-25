//! One player's world on the server, replaying the tick-stamped messages their client sent in
//! the order it sent them.

use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;

use cake_core::cut::Split;
use cake_core::input::{Input, KnifeInput, quantize_radians};
use cake_core::knife::START_TILT_RAD;
use cake_core::sim::{Cake, Phase, World};

enum Entry {
    Input { tick: u32, input: Input },
    Cut { tick: u32 },
    Hash { tick: u32, hash: u64 },
}

/// A message the replay could not take as stamped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Note {
    /// The client's world hash at `tick` differs from the replay's.
    Desync { tick: u32 },
    /// A message stamped for `tick` arrived with the replay at `at_tick`. An input or cut stamped
    /// later is reached by repeating the last input, and an earlier input is dropped while an
    /// earlier cut happens at once. A hash stamped for any other tick goes unchecked.
    OutOfStep { kind: &'static str, tick: u32, at_tick: u32 },
}

impl fmt::Display for Note {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Note::Desync { tick } => write!(f, "world hash differs at tick {tick}"),
            Note::OutOfStep { kind, tick, at_tick } => {
                write!(f, "{kind} stamped tick {tick} arrived at tick {at_tick}")
            }
        }
    }
}

pub struct Replay {
    world: World,
    queue: VecDeque<Entry>,
    last_input: Input,
    has_touched: bool,
}

impl Replay {
    /// A fresh world on `cake` that cuts itself at `round_ticks`.
    pub fn create(cake: Arc<Cake>, round_ticks: u32) -> Replay {
        Replay {
            world: World::create(cake, START_TILT_RAD, Some(round_ticks)),
            queue: VecDeque::new(),
            last_input: calc_idle_input(),
            has_touched: false,
        }
    }

    pub fn push_input(&mut self, tick: u32, input: Input) {
        self.queue.push_back(Entry::Input { tick, input });
    }

    pub fn push_cut(&mut self, tick: u32) {
        self.queue.push_back(Entry::Cut { tick });
    }

    pub fn push_hash(&mut self, tick: u32, hash: u64) {
        self.queue.push_back(Entry::Hash { tick, hash });
    }

    /// Replays every queued message. Inputs after the cut are dropped.
    pub fn drain(&mut self) -> Vec<Note> {
        let mut notes = Vec::new();
        while let Some(entry) = self.queue.pop_front() {
            match entry {
                Entry::Input { tick, input } => self.replay_input(tick, input, &mut notes),
                Entry::Cut { tick } => self.replay_cut(tick, &mut notes),
                Entry::Hash { tick, hash } => self.check_hash(tick, hash, &mut notes),
            }
        }
        notes
    }

    /// Ends the round for a player who has not cut: replays what is queued, then holds the last
    /// input until the round's last tick cuts.
    pub fn finish(&mut self) -> Vec<Note> {
        let notes = self.drain();
        while !self.is_cut() {
            self.world.step(&self.last_input);
        }
        notes
    }

    pub fn get_world(&self) -> &World {
        &self.world
    }

    /// The cut's tick and volumes, once cut.
    pub fn find_cut(&self) -> Option<(u32, Split)> {
        match self.world.get_phase() {
            Phase::Playing => None,
            Phase::Cut { tick, split } => Some((tick, split)),
        }
    }

    pub fn is_cut(&self) -> bool {
        self.find_cut().is_some()
    }

    /// Whether any input moved a control, or the player pressed `Cut!`.
    pub fn has_touched(&self) -> bool {
        self.has_touched
    }

    fn replay_input(&mut self, tick: u32, input: Input, notes: &mut Vec<Note>) {
        if self.is_cut() {
            return;
        }
        let at_tick = self.world.get_tick();
        if tick != at_tick {
            notes.push(Note::OutOfStep { kind: "input", tick, at_tick });
        }
        if tick < at_tick {
            return;
        }
        self.hold_last_input_to(tick);
        if self.is_cut() {
            return;
        }
        self.has_touched |= input != calc_idle_input();
        self.last_input = input;
        self.world.step(&input);
    }

    fn replay_cut(&mut self, tick: u32, notes: &mut Vec<Note>) {
        if self.is_cut() {
            return;
        }
        let at_tick = self.world.get_tick();
        if tick != at_tick {
            notes.push(Note::OutOfStep { kind: "cut", tick, at_tick });
        }
        self.hold_last_input_to(tick);
        self.has_touched = true;
        self.world.cut();
    }

    fn check_hash(&self, tick: u32, hash: u64, notes: &mut Vec<Note>) {
        let at_tick = self.world.get_tick();
        if tick != at_tick {
            notes.push(Note::OutOfStep { kind: "hash", tick, at_tick });
            return;
        }
        if hash != self.world.calc_hash() {
            notes.push(Note::Desync { tick });
        }
    }

    /// Steps with the last input until the world reaches `tick` or cuts itself at the round's end.
    fn hold_last_input_to(&mut self, tick: u32) {
        while self.world.get_tick() < tick && !self.is_cut() {
            self.world.step(&self.last_input);
        }
    }
}

/// What the client sends before the player moves anything: no hand, and the knife and stand
/// where they start.
pub fn calc_idle_input() -> Input {
    Input { hand: None, knife: KnifeInput { tilt: quantize_radians(START_TILT_RAD), lower: 0 }, stand_angle: 0 }
}
