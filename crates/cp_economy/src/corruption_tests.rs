use cp_content::CorruptionKnobs;

use crate::corruption::{CorruptionState, Edge};

fn k() -> CorruptionKnobs {
    CorruptionKnobs::default()
}

#[test]
fn a_new_city_starts_with_the_content_trust_and_climbs_to_the_ceiling() {
    let k = CorruptionKnobs {
        trust_start: 70.0,
        trust_recover_day: 10.0,
        ..k()
    };
    let mut s = CorruptionState::default();
    assert_eq!(s.trust_now(&k), 70.0, "before the first day");
    s.day(&k);
    assert!(s.started);
    assert_eq!(s.trust, 80.0);
    for _ in 0..10 {
        s.day(&k);
    }
    assert_eq!(s.trust, 100.0, "stops at the ceiling");
}

#[test]
fn corruption_decays_and_the_clan_edge_lowers_the_ceiling() {
    let k = CorruptionKnobs {
        trust_recover_day: 5.0,
        corruption_decay_day: 1.0,
        ..k()
    };
    let mut s = CorruptionState {
        corruption: 60.0,
        trust: 90.0,
        started: true,
        ..CorruptionState::default()
    };
    assert_eq!(s.edge(&k), Edge::Clan);
    assert_eq!(s.trust_ceiling(&k), k.trust_ceiling_clan);
    s.day(&k);
    assert_eq!(s.corruption, 59.0);
    // Above the clan ceiling: trust sinks to it.
    assert_eq!(s.trust, 85.0);
    for _ in 0..10 {
        s.day(&k);
    }
    assert_eq!(s.trust, 60.0);
    s.institutions = 100.0;
    assert_eq!(s.edge(&k), Edge::Law);
    s.corruption = 0.0;
    s.day(&k);
    assert_eq!(s.corruption, 0.0, "never below 0");
}

#[test]
fn low_trust_loses_tax_linearly() {
    let k = k();
    let at = |trust: f32| {
        CorruptionState {
            trust,
            started: true,
            ..CorruptionState::default()
        }
        .tax_collection(&k)
    };
    assert_eq!(at(100.0), 1.0);
    assert_eq!(at(50.0), 1.0);
    assert!((at(25.0) - 0.9).abs() < 1e-6, "{}", at(25.0));
    assert!((at(0.0) - 0.8).abs() < 1e-6, "{}", at(0.0));
    // A new city (trust_start 60) collects everything.
    assert_eq!(CorruptionState::default().tax_collection(&k), 1.0);
}
