//! The Standup Order the results show beside the leaderboard: the order the room gives its scrum
//! updates in.
//!
//! Players choose slots in leaderboard order, best first, each taking the open slot
//! closest to their preference, and spectators choose after every player, in join order.

use std::hash::{BuildHasher, RandomState};

use crate::protocol::PlayerId;

/// Someone choosing a slot, with their preference from 0 (first to give their update) to 1 (last).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chooser {
    pub id: PlayerId,
    pub preference: f64,
}

/// The ids of `choosers`, given in the order they choose, listed in slot order.
///
/// Each takes the open slot nearest their preference spread across every slot, and of two equally
/// near the earlier, so the last to choose takes the last slot left.
#[must_use]
#[expect(clippy::cast_precision_loss, reason = "slot counts far below f64's whole-number range")]
#[expect(clippy::missing_panics_doc, reason = "there are as many slots as choosers")]
pub fn calc_standup_order(choosers: &[Chooser]) -> Vec<PlayerId> {
    let last_idx = choosers.len().saturating_sub(1) as f64;
    let mut slots: Vec<Option<PlayerId>> = vec![None; choosers.len()];
    for chooser in choosers {
        let wanted = chooser.preference * last_idx;
        let distance = |idx: usize| (idx as f64 - wanted).abs();
        // `min_by` keeps the first of equal elements, which is the earlier slot.
        let idx = (0..slots.len())
            .filter(|&idx| slots[idx].is_none())
            .min_by(|&a, &b| distance(a).total_cmp(&distance(b)))
            .expect("every chooser finds a slot open");
        slots[idx] = Some(chooser.id);
    }
    slots.into_iter().map(|slot| slot.expect("every slot is taken")).collect()
}

/// The order slots are chosen in: the `ranked` players, given best first as `(id, rank)`, then
/// the `spectators` as given. Players sharing a rank choose in the order `shuffle` leaves them in.
pub fn order_choosers(
    ranked: &[(PlayerId, u32)],
    spectators: &[PlayerId],
    mut shuffle: impl FnMut(&mut [PlayerId]),
) -> Vec<PlayerId> {
    debug_assert!(ranked.is_sorted_by_key(|&(_, rank)| rank));
    let mut order = Vec::with_capacity(ranked.len() + spectators.len());
    for tied in ranked.chunk_by(|a, b| a.1 == b.1) {
        let start = order.len();
        order.extend(tied.iter().map(|&(id, _)| id));
        shuffle(&mut order[start..]);
    }
    order.extend_from_slice(spectators);
    order
}

/// Shuffles `ids` into a random order, each order equally likely.
#[expect(clippy::cast_possible_truncation, reason = "keeps the random hash's low bits")]
pub fn shuffle_randomly(ids: &mut [PlayerId]) {
    let random = RandomState::new();
    for idx in (1..ids.len()).rev() {
        let other = random.hash_one(idx) as usize % (idx + 1);
        ids.swap(idx, other);
    }
}
