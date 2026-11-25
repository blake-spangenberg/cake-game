//! The Standup Order's slot rule, on hand-picked rooms.

use cake_server::protocol::PlayerId;
use cake_server::standup::{Chooser, calc_standup_order, order_choosers};

const fn chooser(id: PlayerId, preference: f64) -> Chooser {
    Chooser { id, preference }
}

/// Reverses each tied group, an order a reader can predict in place of a random one.
const fn reverse(ids: &mut [PlayerId]) {
    ids.reverse();
}

#[test]
fn each_chooser_takes_the_open_slot_nearest_their_preference_in_choosing_order() {
    // Five slots, so a preference of 0.5 wants slot 2 and 0.8 wants slot 3.2.
    let choosers = [chooser(1, 0.5), chooser(2, 0.5), chooser(3, 0.8), chooser(4, 0.0), chooser(5, 1.0)];
    // 1 takes slot 2; 2 finds slots 1 and 3 equally near and takes the earlier, 1; 3 takes 3;
    // 4 takes 0; 5 takes 4.
    assert_eq!(calc_standup_order(&choosers), [4, 2, 1, 3, 5]);
}

#[test]
fn last_to_choose_takes_the_last_slot_left_whatever_their_preference() {
    let choosers = [chooser(1, 1.0), chooser(2, 1.0), chooser(3, 0.0)];
    assert_eq!(calc_standup_order(&choosers), [3, 2, 1]);
}

#[test]
fn everyone_wanting_to_go_first_goes_in_choosing_order() {
    let choosers = [chooser(7, 0.0), chooser(3, 0.0), chooser(9, 0.0)];
    assert_eq!(calc_standup_order(&choosers), [7, 3, 9]);
}

#[test]
fn a_lone_chooser_takes_the_only_slot() {
    assert_eq!(calc_standup_order(&[chooser(4, 0.3)]), [4]);
}

#[test]
fn players_choose_best_first_with_tied_ranks_shuffled_among_themselves_then_spectators_in_join_order() {
    let ranked = [(10, 1), (11, 2), (12, 2), (13, 2), (14, 5), (15, 6), (16, 6)];
    let spectators = [3, 8];
    assert_eq!(order_choosers(&ranked, &spectators, reverse), [10, 13, 12, 11, 14, 16, 15, 3, 8]);
}
