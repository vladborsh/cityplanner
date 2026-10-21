//! Health (§16 M2 D "Здоровье"): citizens fall ill at home (more where the air is dirty),
//! clinics and hospitals within reach treat the sick by their daily capacity, and a home with
//! sick people nobody treats has the SICK problem. The sick stay home from work (the routine).
//!
//! * **Falling ill** (each tick, the citizen bucket `id % 256 == tick % 256`, so
//!   `TICKS_PER_DAY / 256` visits a game day): a healthy citizen at home gets `sick_since` with
//!   the chance per day `(sick_base + sick_pollution × G) × (1 − GREEN_SICK_CUT × green) +
//!   sick_noise × N + sick_water × W` (T-300, §11.1), shared out over the visits; `G` is the
//!   ground pollution and `N` the noise of the home's 16 m cell, `W` the dirt of the water it
//!   drinks (`Emissions::water_dirt`). Parks soften the base and the pollution only.
//! * **Bad water** (with the treatment, building bucket `index % 64`): a building whose water
//!   is at least `bad_water_at` dirty has the BAD_WATER problem; clean or no water clears it.
//! * **Treatment** (each tick, the building bucket `index % 64 == tick % 64`, as police): the
//!   sick of a building with Health coverage are cured one capacity unit each by the nearest
//!   powered clinic or hospital with room; the others keep SICK on the building.

use bevy_ecs::system::{Res, ResMut};
use cp_building::{BuildingGrid, BuildingId, BuildingState};
use cp_citizen::CitizenGrid;
use cp_citizen::system::CITIZEN_BUCKETS;
use cp_content::{ServiceKind, ServiceUnlocks, SimContent};
use cp_core::time::TICKS_PER_DAY;
use cp_core::{SimRng, Tick};
use cp_service::{COVERAGE_PERIOD, FacilityFinder, ServiceCoverage, ServiceProblems, ServiceState};

use crate::emissions::Emissions;

/// The RNG stream of this system (`SimRng::fork`).
pub const HEALTH_STREAM: u64 = 0x4EA1;

const VISITS_PER_DAY: f32 = (TICKS_PER_DAY / CITIZEN_BUCKETS as u64) as f32;

/// Economy phase, after `update_police`.
#[allow(clippy::too_many_arguments)]
pub fn update_health(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    net: cp_service::ServiceNet,
    buildings: Res<BuildingGrid>,
    emissions: Res<Emissions>,
    coverage: Res<ServiceCoverage>,
    rng: Res<SimRng>,
    mut grid: ResMut<CitizenGrid>,
    mut state: ResMut<ServiceState>,
    unlocks: Option<Res<ServiceUnlocks>>,
) {
    let Some(content) = content else { return };
    // Worlds built without the resource have every service open.
    let unlocks = unlocks.map(|u| *u).unwrap_or_default();
    let db = &content.0;
    let bal = db.balance();
    grid.ensure_derived();

    // No clinic or hospital is open yet: nobody falls ill, and the sick of an older save are
    // well again (BUG-051).
    if !unlocks.open(ServiceKind::Health) {
        let bucket = tick.0 % COVERAGE_PERIOD;
        for (id, _) in buildings.iter_bucket(COVERAGE_PERIOD as u32, bucket as u32) {
            let sick: Vec<_> = grid.sick_in(id).collect();
            for cid in sick {
                grid.set_sick(cid, None);
            }
            if state.problems_of(id).has(ServiceProblems::SICK) {
                state.get_mut(id).problems.set(ServiceProblems::SICK, false);
            }
            mark_bad_water(&mut state, &emissions, id, bal.bad_water_at);
        }
        return;
    }

    // --- falling ill ---
    let mut rng = rng.fork(HEALTH_STREAM, *tick);
    let bucket = (tick.0 % u64::from(CITIZEN_BUCKETS)) as u32;
    let parks = cp_building::park_sites(db, &buildings);
    let healthy: Vec<_> = grid
        .citizens_bucket(CITIZEN_BUCKETS, bucket)
        .filter(|(_, c)| c.sick_since.is_none())
        .filter_map(|(id, c)| Some((id, c.home?)))
        .collect();
    for (id, home) in healthy {
        let Some(b) = buildings.get(home) else {
            continue;
        };
        let water = emissions.water_dirt_of(home);
        let p = (cp_building::sick_chance(
            bal.sick_base,
            bal.sick_pollution * emissions.ground_at(b.pos),
            cp_building::green_at(&parks, b.pos),
        ) + bal.sick_noise * emissions.noise_at(b.pos)
            + bal.sick_water * water)
            / VISITS_PER_DAY;
        if rng.next_f32() < p {
            grid.set_sick(id, Some(*tick));
        }
    }

    // --- treatment ---
    let roads = net.graph();
    let mut finder = None;
    let bucket = tick.0 % COVERAGE_PERIOD;
    for (id, b) in buildings.iter_bucket(COVERAGE_PERIOD as u32, bucket as u32) {
        if b.state != BuildingState::Active {
            continue;
        }
        let sick: Vec<_> = grid.sick_in(id).collect();
        let mut untreated = sick.len();
        if untreated > 0 && coverage.coverage_of(ServiceKind::Health, b) > 0.0 {
            let mut station = None;
            for cid in sick {
                if station.is_none() {
                    station = finder
                        .get_or_insert_with(|| {
                            FacilityFinder::new(&roads, &buildings, db, ServiceKind::Health)
                        })
                        .nearest(&state, id);
                }
                let Some(s) = station else { break };
                state.facility_mut(s).used_today += 1;
                grid.set_sick(cid, None);
                untreated -= 1;
                // The facility may be full now: look for the next one on the next sick.
                station = None;
            }
        }
        let problem = untreated > 0;
        if problem != state.problems_of(id).has(ServiceProblems::SICK) {
            state
                .get_mut(id)
                .problems
                .set(ServiceProblems::SICK, problem);
        }
        mark_bad_water(&mut state, &emissions, id, bal.bad_water_at);
    }
}

/// Sets or clears BAD_WATER on `id` by the dirt of the water it drinks (T-300). Whether a clinic
/// is open or not: the problem is the water's.
fn mark_bad_water(state: &mut ServiceState, emissions: &Emissions, id: BuildingId, at: f32) {
    let bad = emissions.water_dirt_of(id) >= at;
    if bad != state.problems_of(id).has(ServiceProblems::BAD_WATER) {
        state
            .get_mut(id)
            .problems
            .set(ServiceProblems::BAD_WATER, bad);
    }
}
