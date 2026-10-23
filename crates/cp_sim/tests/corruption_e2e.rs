//! T-324 (`architecture/corruption.md` §12): the corruption state on a real city. Trust sets the
//! share of the tax the city collects and is the trust factor of the home contentment (through
//! which it pulls on move-in, §19.4); the daily update seeds and moves it; two runs are identical.

use std::path::Path;
use std::sync::Arc;

use cp_content::ContentDb;
use cp_core::time::TICKS_PER_DAY;
use cp_economy::contentment::F_TRUST;
use cp_economy::{CityContentment, CorruptionState, MoneyFlow};
use cp_sim::SimRunner;
use cp_sim::scenario::{GridCityConfig, grid_city};
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
