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

fn content() -> cp_content::ContentDb {
    cp_content::ContentDb::load_dir(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content"),
    )
    .unwrap()
}

#[test]
fn offers_come_from_the_population_threshold_sooner_in_a_bigger_city_and_lapse() {
    use crate::corruption::{DayInputs, offer_interval};
    use cp_core::time::TICKS_PER_DAY;
    let db = content();
    let k = db.balance().corruption.clone();
    assert_eq!(offer_interval(&k, 1000, 0.0), 12);
    assert!(offer_interval(&k, 20_000, 50.0) < offer_interval(&k, 2000, 50.0));
    assert_eq!(
        offer_interval(&k, 1_000_000, 100.0),
        3,
        "never below offer_days_min"
    );
    let buildings = cp_building::BuildingGrid::default();
    let rng = cp_core::SimRng::seed_from_u64(7);
    let at = |day: u64, population: u32| DayInputs {
        db: &db,
        tick: cp_core::Tick(day * TICKS_PER_DAY),
        rng: &rng,
        population,
        land: 0.0,
        buildings: &buildings,
        districts: None,
    };
    let mut s = CorruptionState {
        trust: 60.0,
        started: true,
        ..CorruptionState::default()
    };
    s.deals_day(&at(1, 999));
    assert!(s.inbox.is_empty(), "too small a city");
    s.deals_day(&at(2, 1000));
    assert_eq!(s.inbox.len(), 1);
    let o = s.inbox[0].clone();
    // Only the deals that need no building or district can come to an empty city.
    assert!(
        ["kickback", "land", "sponsor"].contains(&o.deal.as_str()),
        "{o:?}"
    );
    assert_eq!(o.target, None);
    assert_eq!(s.next_offer_day, 2 + 12);
    s.deals_day(&at(3, 1000));
    assert_eq!(s.inbox.len(), 1, "the next one waits its interval");
    // The same day and seed draw the same offer.
    let mut again = CorruptionState {
        trust: 60.0,
        started: true,
        ..CorruptionState::default()
    };
    again.deals_day(&at(2, 1000));
    assert_eq!(again.inbox, vec![o.clone()]);
    // Unanswered, it lapses on its day (and the next one comes when due).
    s.deals_day(&at(u64::from(o.expires_day), 1000));
    assert!(s.inbox.iter().all(|x| x != &o));
    for d in 4..40 {
        s.deals_day(&at(d, 1000));
    }
    assert!(s.inbox.len() <= 3 && !s.inbox.is_empty());
    let mut ids: Vec<&str> = s.inbox.iter().map(|o| o.deal.as_str()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), s.inbox.len(), "one offer of a deal at a time");
}

#[test]
fn the_contents_six_deals_load() {
    let db = content();
    let mut ids: Vec<&str> = db.deals().iter().map(|d| d.id.as_str()).collect();
    ids.sort();
    assert_eq!(
        ids,
        [
            "casino",
            "developer",
            "factory_emissions",
            "kickback",
            "land",
            "sponsor"
        ]
    );
}
