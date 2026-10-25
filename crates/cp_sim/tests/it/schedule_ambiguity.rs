//! T-257: every pair of systems of the sim schedule that touches the same data has an explicit
//! order. Bevy's ambiguity detection lists the pairs whose order the schedule graph leaves open;
//! the order of such a pair follows the graph, so an unrelated new system could flip it and move
//! the results (the m1_traffic pin moved when T-256 added one system to the Network phase).
//!
//! A new system that conflicts with another one fails this test: give the pair an order
//! (`.before` / `.after` in `SimRunner::new`, following the phase design in
//! `architecture/system-design.md` §2), or, for a pair that is truly independent although the
//! detector sees a conflict, `.ambiguous_with` with a comment saying why.

use cp_sim::SimRunner;

#[test]
fn every_pair_of_systems_that_touches_the_same_data_has_an_order() {
    let mut sim = SimRunner::new(1);
    let pairs = sim.schedule_ambiguities();
    let list: Vec<String> = pairs
        .iter()
        .map(|(a, b, on)| format!("{a} <> {b} on {}", on.join(", ")))
        .collect();
    assert!(
        pairs.is_empty(),
        "{} unordered pair(s) of systems:\n{}",
        pairs.len(),
        list.join("\n")
    );
}
