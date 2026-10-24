//! T-324/T-325 (`architecture/corruption.md` §12): the corruption state on a real city. Trust sets the
//! share of the tax the city collects and is the trust factor of the home contentment (through
//! which it pulls on move-in, §19.4); the daily update seeds and moves it; two runs are identical.

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec2;
use cp_content::ContentDb;
use cp_core::time::TICKS_PER_DAY;
use cp_core::{Cost, Treasury};
use cp_district::{District, Districts};
use cp_economy::contentment::F_TRUST;
use cp_economy::{
    ActiveDeal, CityContentment, CorruptionCommand, CorruptionState, DealTarget, MoneyFlow, Offer,
};
use cp_net::{ElevationMode, NetCommand, RoadPath};
use cp_sim::scenario::{GridCityConfig, grid_city};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn city() -> SimRunner {
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let (sim, report) = grid_city(400, 1, terrain, content, &GridCityConfig::default());
    assert!(report.rejected.is_empty(), "{:?}", report.rejected);
    sim
}

fn with_trust(trust: f32) -> SimRunner {
    let mut sim = city();
    *sim.world_mut().resource_mut::<CorruptionState>() = CorruptionState {
        trust,
        started: true,
        ..CorruptionState::default()
    };
    sim
}

fn run(sim: &mut SimRunner, n: u64) {
    for _ in 0..n {
        sim.tick_once();
    }
}

/// The tax booked in the game hours after `hour`.
fn tax_since(sim: &SimRunner, hour: u64) -> i64 {
    sim.world()
        .resource::<MoneyFlow>()
        .hours
        .iter()
        .filter(|(h, _)| *h > hour)
        .map(|(_, f)| f.income_by_zone.iter().map(|c| c.0).sum::<i64>())
        .sum()
}

#[test]
fn low_trust_collects_less_tax_and_makes_homes_distrust() {
    // Three contentment passes (a daily update on the way moves trust by a fraction of a point).
    let n = 3 * cp_economy::CITY_PERIOD;
    let mut high = with_trust(100.0);
    let mut low = with_trust(0.0);
    let hour = cp_economy::game_hour(high.tick().0);
    run(&mut high, n);
    run(&mut low, n);
    let (th, tl) = (tax_since(&high, hour), tax_since(&low, hour));
    assert!(th > 0, "the city pays tax: {th}");
    // No trust: 20% of the tax is lost (tax_loss_max); the rest is the cities drifting apart.
    let ratio = tl as f64 / th as f64;
    assert!((0.75..0.85).contains(&ratio), "{tl} / {th} = {ratio}");
    let ch = high.world().resource::<CityContentment>().clone();
    let cl = low.world().resource::<CityContentment>().clone();
    assert_eq!(ch.factors[F_TRUST], 100);
    assert_eq!(cl.factors[F_TRUST], 0);
    assert!(cl.city < ch.city, "{} vs {}", cl.city, ch.city);
    assert!(
        cl.complaints
            .iter()
            .any(|&(c, n)| c == cp_building::Complaint::Distrust && n > 0),
        "{:?}",
        cl.complaints
    );
}

#[test]
fn the_first_day_seeds_trust_from_the_content_and_days_move_it() {
    let mut sim = city();
    let k = sim
        .world()
        .resource::<cp_content::SimContent>()
        .0
        .balance()
        .corruption
        .clone();
    // A city whose first day is still to come.
    *sim.world_mut().resource_mut::<CorruptionState>() = CorruptionState::default();
    // Through the next tick a day starts at (the systems see the tick before it advances).
    let to_day = TICKS_PER_DAY - sim.tick().0 % TICKS_PER_DAY;
    run(&mut sim, to_day + 1);
    let s = sim.world().resource::<CorruptionState>().clone();
    assert!(s.started);
    assert!((s.trust - (k.trust_start + k.trust_recover_day)).abs() < 1e-4);
    sim.world_mut().resource_mut::<CorruptionState>().corruption = 10.0;
    run(&mut sim, TICKS_PER_DAY);
    let s = sim.world().resource::<CorruptionState>().clone();
    assert!((s.corruption - (10.0 - k.corruption_decay_day)).abs() < 1e-4);
    assert!((s.trust - (k.trust_start + 2.0 * k.trust_recover_day)).abs() < 1e-4);
}

#[test]
fn two_runs_are_identical() {
    let go = || {
        let mut sim = with_trust(30.0);
        sim.world_mut().resource_mut::<CorruptionState>().corruption = 50.0;
        run(&mut sim, TICKS_PER_DAY + 10);
        (
            sim.world().resource::<CorruptionState>().clone(),
            sim.checksum(),
        )
    };
    assert_eq!(go(), go());
}

fn offer(sim: &mut SimRunner, deal: &str, target: Option<DealTarget>) {
    let day = cp_economy::corruption::day_of(sim.tick());
    sim.world_mut()
        .resource_mut::<CorruptionState>()
        .inbox
        .push(Offer {
            deal: deal.into(),
            target,
            expires_day: day + 5,
        });
}

fn answer(sim: &mut SimRunner, cmd: CorruptionCommand) {
    let t = sim.tick();
    sim.push_command(SimCommand::Corruption(cmd), t);
    sim.apply_due_commands();
}

fn accept(deal: &str) -> CorruptionCommand {
    CorruptionCommand::Accept { deal: deal.into() }
}

#[test]
fn accepting_pays_now_adds_corruption_and_evidence_and_lifts_the_policy() {
    let mut sim = with_trust(70.0);
    let ban = cp_content::PolicyKind::HighRiseBan.bit();
    let district = {
        let mut ds = sim.world_mut().resource_mut::<Districts>();
        ds.list.push(Some(District {
            name: "Old Town".into(),
            color: 0,
            tax: [None; 6],
            policies: ban,
            spec: [None; 6],
        }));
        cp_district::DistrictId(ds.list.len() as u8)
    };
    offer(&mut sim, "land", None);
    offer(&mut sim, "developer", Some(DealTarget::District(district)));
    let before = sim.world().resource::<Treasury>().balance;
    answer(&mut sim, accept("land"));
    answer(&mut sim, accept("developer"));
    let paid = sim.world().resource::<Treasury>().balance.0 - before.0;
    assert_eq!(
        paid,
        Cost::from_units(65_000.0).0,
        "40k for the land, 25k from the developer"
    );
    let s = sim.world().resource::<CorruptionState>().clone();
    assert!(s.inbox.is_empty());
    assert!((s.corruption - 17.0).abs() < 1e-4, "{}", s.corruption);
    assert_eq!(s.evidence.len(), 2);
    assert!((s.evidence.iter().map(|e| e.weight).sum::<f32>() - 6.0).abs() < 1e-4);
    assert!(
        (s.trust - 68.0).abs() < 1e-4,
        "the developer's neighbours: {}",
        s.trust
    );
    assert_eq!(
        s.active.len(),
        1,
        "the lifted ban runs on, the land sale does not"
    );
    let ds = sim.world().resource::<Districts>();
    assert_eq!(
        ds.get(district).unwrap().policies & ban,
        0,
        "the ban is lifted"
    );
    // No such offer any more.
    let t = sim.tick();
    sim.push_command(SimCommand::Corruption(accept("land")), t);
    sim.apply_due_commands();
    assert!((sim.world().resource::<CorruptionState>().corruption - 17.0).abs() < 1e-4);
}

#[test]
fn declining_is_neutral_in_the_middle_and_costs_trust_on_the_clan_edge_and_offers_lapse() {
    let mut sim = with_trust(70.0);
    offer(&mut sim, "sponsor", None);
    answer(
        &mut sim,
        CorruptionCommand::Decline {
            deal: "sponsor".into(),
        },
    );
    let s = sim.world().resource::<CorruptionState>().clone();
    assert!(s.inbox.is_empty() && s.evidence.is_empty());
    assert_eq!((s.trust, s.corruption), (70.0, 0.0));

    sim.world_mut().resource_mut::<CorruptionState>().corruption = 80.0;
    offer(&mut sim, "sponsor", None);
    answer(
        &mut sim,
        CorruptionCommand::Decline {
            deal: "sponsor".into(),
        },
    );
    assert!((sim.world().resource::<CorruptionState>().trust - 69.0).abs() < 1e-4);

    // Left unanswered past its day, an offer is declined.
    offer(&mut sim, "land", None);
    let to_lapse = 6 * TICKS_PER_DAY;
    run(&mut sim, to_lapse);
    let s = sim.world().resource::<CorruptionState>().clone();
    assert!(s.inbox.iter().all(|o| o.deal != "land"), "{:?}", s.inbox);
}

#[test]
fn a_running_deal_pays_weekly_and_boosts_the_tourists_until_it_ends() {
    let mut sim = with_trust(70.0);
    let shop = {
        let db = sim.world().resource::<cp_content::SimContent>().0.clone();
        sim.world()
            .resource::<cp_building::BuildingGrid>()
            .iter()
            .find(|(_, b)| {
                matches!(
                    db.building(b.def).zone,
                    cp_content::ZoneType::ComLow | cp_content::ZoneType::ComHigh
                )
            })
            .map(|(id, _)| id)
            .expect("a shop")
    };
    offer(&mut sim, "casino", Some(DealTarget::Building(shop)));
    answer(&mut sim, accept("casino"));
    let s = sim.world().resource::<CorruptionState>().clone();
    assert_eq!(
        s.active,
        vec![ActiveDeal {
            deal: "casino".into(),
            target: Some(DealTarget::Building(shop)),
            accepted_day: cp_economy::corruption::day_of(sim.tick()),
            until_day: 0,
        }]
    );
    // Through the first day: the tourists come 15% more.
    let to_day = TICKS_PER_DAY - sim.tick().0 % TICKS_PER_DAY;
    run(&mut sim, to_day + 1);
    assert_eq!(sim.world().resource::<cp_transit::Tourists>().boost_pct, 15);
    // The weekly cut is booked on the 7th day after it was accepted.
    let k = cp_economy::corruption::day_of(sim.tick());
    let accepted = s.active[0].accepted_day;
    let week_day = accepted + 7;
    run(&mut sim, u64::from(week_day - k) * TICKS_PER_DAY - 2);
    let before = sim.world().resource::<Treasury>().balance.0;
    let cs_before = sim.world().resource::<CorruptionState>().clone();
    run(&mut sim, 3);
    // Everything else the city books in those ticks is small next to the cut.
    let delta = sim.world().resource::<Treasury>().balance.0 - before;
    assert_eq!(cs_before.active.len(), 1);
    assert!(
        (delta - Cost::from_units(2000.0).0).abs() < Cost::from_units(500.0).0,
        "{delta}"
    );
    // Once the deal is over the boost goes with it.
    sim.world_mut()
        .resource_mut::<CorruptionState>()
        .active
        .clear();
    run(&mut sim, TICKS_PER_DAY);
    assert_eq!(sim.world().resource::<cp_transit::Tourists>().boost_pct, 0);
}

/// The money a straight road of 200 m costs, with `active` deals running.
fn road_cost(active: Vec<ActiveDeal>) -> i64 {
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::new(5);
    sim.set_terrain(terrain);
    sim.set_content(Arc::clone(&content));
    sim.world_mut().resource_mut::<CorruptionState>().active = active;
    let before = sim.world().resource::<Treasury>().balance.0;
    let t = sim.tick();
    sim.push_command(
        SimCommand::Net(NetCommand::BuildRoad {
            road: content.road_id("basic_2l").unwrap(),
            path: RoadPath::Straight {
                from: Vec2::new(5000.0, 5000.0),
                to: Vec2::new(5200.0, 5000.0),
            },
            elevation: ElevationMode::Ground,
        }),
        t,
    );
    sim.apply_due_commands();
    before - sim.world().resource::<Treasury>().balance.0
}

#[test]
fn the_kickback_makes_building_cheaper_while_it_runs() {
    let full = road_cost(Vec::new());
    let cheap = road_cost(vec![ActiveDeal {
        deal: "kickback".into(),
        target: None,
        accepted_day: 0,
        until_day: 30,
    }]);
    assert!(full > 0);
    assert_eq!(
        cheap,
        full - (full as f64 * 0.2).round() as i64,
        "{cheap} of {full}"
    );
}
