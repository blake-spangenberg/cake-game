//! One player's replay, fed messages directly.

use cake_core::cakes::build_slice_missing_cake;
use cake_server::replay::{Note, Replay, calc_idle_input};

const ROUND_TICKS: u32 = 900;

#[test]
fn a_burst_of_stale_inputs_makes_one_note_and_holding_steps_to_the_tick_asked() {
    let mut replay = Replay::create(build_slice_missing_cake().into(), ROUND_TICKS);
    replay.hold_to(120);
    assert_eq!(replay.get_world().get_tick(), 120);
    for tick in 10..60 {
        replay.push_input(tick, calc_idle_input());
    }
    replay.push_hash(30, 0);
    replay.push_input(60, calc_idle_input());

    let expected = [
        Note::OutOfStep { kind: "input", first_tick: 10, last_tick: 60, len: 51, at_tick: 120 },
        Note::OutOfStep { kind: "hash", first_tick: 30, last_tick: 30, len: 1, at_tick: 120 },
    ];
    assert_eq!(replay.drain(), expected);
    assert_eq!(replay.get_world().get_tick(), 120);
    replay.hold_to(ROUND_TICKS + 10);
    assert_eq!(replay.find_cut().map(|(tick, _)| tick), Some(ROUND_TICKS));
}

#[test]
fn refusals_between_two_drains_make_one_note_per_kind() {
    let mut replay = Replay::create(build_slice_missing_cake().into(), ROUND_TICKS);
    for tick in 500..600 {
        replay.note_refused("input", tick, 10 + tick / 50);
    }
    replay.note_refused("cut", 700, 21);

    let expected = [
        Note::Refused { kind: "input", first_tick: 500, last_tick: 599, len: 100, clock_tick: 21 },
        Note::Refused { kind: "cut", first_tick: 700, last_tick: 700, len: 1, clock_tick: 21 },
    ];
    assert_eq!(replay.drain(), expected);
    assert_eq!(replay.drain(), []);
}
