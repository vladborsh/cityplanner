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

fn evidence(deals: &[(&str, f32)]) -> CorruptionState {
    CorruptionState {
        corruption: 20.0,
        trust: 70.0,
        started: true,
        evidence: deals
            .iter()
            .map(|&(d, w)| crate::Evidence {
                deal: d.into(),
                accepted_day: 0,
                weight: w,
            })
            .collect(),
        ..CorruptionState::default()
    }
}

/// The first day (from day 1) a scandal breaks, and the deal it reveals.
fn first_scandal(
    k: &CorruptionKnobs,
    mut s: CorruptionState,
    seed: u64,
) -> Option<(u32, String, CorruptionState)> {
    let rng = cp_core::SimRng::seed_from_u64(seed);
    for day in 1..2000u64 {
        let tick = cp_core::Tick(day * cp_core::time::TICKS_PER_DAY);
        if s.scandal_day(k, &rng, tick).is_some() {
            let d = s.scandals.last().unwrap();
            return Some((d.day, d.deal.clone(), s));
        }
    }
    None
}

#[test]
fn evidence_breaks_a_scandal_deterministically_by_the_seed_heavier_evidence_likelier() {
    let k = k();
    let s = evidence(&[("kickback", 2.0), ("land", 4.0)]);
    let a = first_scandal(&k, s.clone(), 7).expect("a scandal comes");
    let b = first_scandal(&k, s.clone(), 7).unwrap();
    assert_eq!((a.0, &a.1), (b.0, &b.1), "same seed, same scandal");
    assert_eq!(a.2, b.2);
    let (_, deal, after) = a;
    let scandal = after.scandals.last().unwrap();
    let w = if deal == "land" { 4.0 } else { 2.0 };
    assert_eq!(scandal.weight, w);
    assert_eq!(scandal.trust, k.scandal_trust * w);
    assert_eq!(after.trust, 70.0 - k.scandal_trust * w);
    assert_eq!(scandal.fine, (k.scandal_fine * w) as i64);
    assert_eq!(after.evidence.len(), 1, "the revealed evidence goes");
    assert!(!scandal.undone, "no institutions, the deal stands");

    // Over many seeds the heavier deal comes out about twice as often.
    let land = (0..200)
        .filter(|&seed| first_scandal(&k, s.clone(), seed).unwrap().1 == "land")
        .count();
    assert!((110..=160).contains(&land), "land revealed {land} of 200");

    // No evidence, no scandal; more corruption hides it, the captured stations cover it.
    assert!(first_scandal(&k, evidence(&[]), 7).is_none());
    let mut hidden = s.clone();
    hidden.corruption = 90.0;
    let mut roofed = s.clone();
    roofed.captured = (0..4)
        .map(|i| cp_core::Id::<cp_building::Building>::new(i, 0))
        .collect();
    assert!(hidden.scandal_chance(&k) < s.scandal_chance(&k));
    assert_eq!(roofed.roof(&k), k.roof_max);
    assert!((roofed.scandal_chance(&k) - s.scandal_chance(&k) * (1.0 - k.roof_max)).abs() < 1e-6);
}

#[test]
fn one_scandal_at_a_time_and_strong_institutions_undo_the_deal() {
    let k = CorruptionKnobs {
        scandal_day: 1.0,
        scandal_exposure: 1.0,
        scandal_c_damp: 0.0,
        ..k()
    };
    let mut s = evidence(&[("developer", 2.0), ("land", 4.0)]);
    s.institutions = k.scandal_undo_i;
    s.active.push(crate::ActiveDeal {
        deal: "developer".into(),
        target: None,
        accepted_day: 0,
        until_day: 0,
    });
    let rng = cp_core::SimRng::seed_from_u64(1);
    let at = |d: u64| cp_core::Tick(d * cp_core::time::TICKS_PER_DAY);
    let mut broke = Vec::new();
    for d in 1..=30 {
        if let Some((fine, undone)) = s.scandal_day(&k, &rng, at(d)) {
            broke.push((d, fine, undone.map(|a| a.deal)));
        }
    }
    assert_eq!(broke.len(), 2, "{broke:?}");
    assert_eq!(
        broke[1].0 - broke[0].0,
        k.scandal_pause as u64,
        "a pause between"
    );
    assert!(s.evidence.is_empty());
    assert!(s.active.is_empty(), "the developer deal is undone");
    assert!(s.scandals.iter().all(|x| x.undone));
    let developer = broke.iter().find(|b| b.2.is_some()).unwrap();
    assert_eq!(developer.1, cp_core::Cost::from_units(k.scandal_fine * 2.0));
}

#[test]
fn stations_get_captured_more_with_corruption_and_a_low_police_budget() {
    let k = k();
    let mut s = CorruptionState {
        corruption: 50.0,
        ..CorruptionState::default()
    };
    assert_eq!(s.capture_chance(&k, 100), k.capture_day * 0.5);
    assert!(s.capture_chance(&k, 50) > s.capture_chance(&k, 100));
    assert!(s.capture_chance(&k, 150) < s.capture_chance(&k, 100));
    assert_eq!(
        CorruptionState::default().capture_chance(&k, 50),
        0.0,
        "no C, no capture"
    );

    let stations: Vec<_> = (0..20)
        .map(|i| cp_core::Id::<cp_building::Building>::new(i, 0))
        .collect();
    let rng = cp_core::SimRng::seed_from_u64(3);
    s.corruption = 100.0;
    for d in 1..=30u64 {
        s.capture_day(
            &k,
            &stations,
            50,
            &rng,
            cp_core::Tick(d * cp_core::time::TICKS_PER_DAY),
        );
    }
    assert!(!s.captured.is_empty() && s.captured.len() < stations.len());
    assert!(s.captured.is_sorted());
    assert!(s.is_captured(s.captured[0]));
    // A demolished station leaves the list.
    let gone = s.captured[0];
    let left: Vec<_> = stations.iter().copied().filter(|&b| b != gone).collect();
    s.corruption = 0.0;
    s.capture_day(
        &k,
        &left,
        100,
        &rng,
        cp_core::Tick(31 * cp_core::time::TICKS_PER_DAY),
    );
    assert!(!s.is_captured(gone));
}

#[test]
fn institutions_raise_i_and_each_does_its_part() {
    use cp_content::InstitutionKind as K;
    let k = k();
    let mut s = CorruptionState {
        corruption: 30.0,
        trust: 50.0,
        started: true,
        ..CorruptionState::default()
    };
    let all = K::Audit.bit() | K::Press.bit() | K::Court.bit();
    s.institutions_day(&k, all, true, 0.0);
    assert_eq!(s.institutions_target(&k), 75.0);
    assert_eq!(s.institutions, k.i_follow_day);
    for _ in 0..100 {
        s.institutions_day(&k, all, true, 0.0);
    }
    assert_eq!(s.institutions, 75.0);
    // Exposure: base 0.5 + 3 × 0.5 → four times the scandal chance of a city without them.
    let mut bare = s.clone();
    bare.present = 0;
    s.evidence.push(crate::Evidence {
        deal: "land".into(),
        accepted_day: 0,
        weight: 4.0,
    });
    bare.evidence = s.evidence.clone();
    assert!((s.scandal_chance(&k) - 4.0 * bare.scandal_chance(&k)).abs() < 1e-6);
    // The court: C falls faster, crime grows less; the press: trust recovers twice as fast.
    let (c, t) = (s.corruption, s.trust);
    s.day(&k);
    assert!((c - s.corruption - (k.corruption_decay_day + k.court_decay_day)).abs() < 1e-5);
    assert!((s.trust - t - k.trust_recover_day * k.press_trust_recover).abs() < 1e-5);
    assert!(s.crime_factor(&k) < 1.0);
    // The press reports the city's problems and praises a declined deal.
    let t = s.trust;
    s.institutions_day(&k, all, true, 0.5);
    assert!((t - s.trust - k.press_problem_trust * 0.5).abs() < 1e-5);
    let t = s.trust;
    s.declined(&k);
    assert_eq!(s.trust, t + k.press_decline_trust);
    // Tenders make building dearer.
    assert_eq!(s.build_surcharge(&k), k.tenders_build);
}

#[test]
fn the_court_frees_captured_stations() {
    let k = CorruptionKnobs {
        court_free_day: 0.5,
        ..k()
    };
    let stations: Vec<_> = (0..20)
        .map(|i| cp_core::Id::<cp_building::Building>::new(i, 0))
        .collect();
    let mut s = CorruptionState {
        captured: stations.clone(),
        present: cp_content::InstitutionKind::Court.bit(),
        ..CorruptionState::default()
    };
    let rng = cp_core::SimRng::seed_from_u64(9);
    s.capture_day(
        &k,
        &stations,
        100,
        &rng,
        cp_core::Tick(cp_core::time::TICKS_PER_DAY),
    );
    assert!(
        !s.captured.is_empty() && s.captured.len() < 20,
        "{}",
        s.captured.len()
    );
}

#[test]
fn the_edges_change_offers_money_tax_crime_land_and_upkeep() {
    let k = k();
    let clan = CorruptionState {
        corruption: 80.0,
        trust: 100.0,
        started: true,
        ..CorruptionState::default()
    };
    let law = CorruptionState {
        institutions: 80.0,
        trust: 100.0,
        started: true,
        ..CorruptionState::default()
    };
    let middle = CorruptionState {
        trust: 100.0,
        started: true,
        ..CorruptionState::default()
    };
    assert_eq!(clan.offer_factor(&k), k.clan_offers);
    assert_eq!(law.offer_factor(&k), k.law_offers);
    assert_eq!(middle.offer_factor(&k), 1.0);
    assert_eq!(clan.money_factor(&k), k.clan_money);
    assert_eq!(clan.crime_factor(&k), 1.0 + k.clan_crime);
    assert_eq!(middle.tax_collection(&k), 1.0);
    assert!((law.tax_collection(&k) - (1.0 + f64::from(k.law_tax))).abs() < 1e-9);
    assert_eq!(law.land_factor(&k), 1.0 + k.law_land);
    assert!((law.institution_upkeep(&k) - (1.0 + f64::from(k.law_upkeep))).abs() < 1e-9);
    assert_eq!(middle.land_factor(&k), 1.0);
}

#[test]
fn transparent_tenders_stop_the_kickback_and_its_corruption() {
    let db = content();
    let mut s = CorruptionState {
        tenders: true,
        ..CorruptionState::default()
    };
    let buildings = cp_building::BuildingGrid::default();
    let rng = cp_core::SimRng::seed_from_u64(1);
    for day in 1..200u64 {
        s.inbox.clear();
        s.next_offer_day = 0;
        s.deals_day(&crate::corruption::DayInputs {
            db: &db,
            tick: cp_core::Tick(day * cp_core::time::TICKS_PER_DAY),
            rng: &rng,
            population: 5000,
            land: 50.0,
            buildings: &buildings,
            districts: None,
        });
        assert!(s.inbox.iter().all(|o| o.deal != "kickback"), "day {day}");
    }
}
