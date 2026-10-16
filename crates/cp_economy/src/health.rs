//! Health (§16 M2 D "Здоровье"): citizens fall ill at home (more where the air is dirty),
//! clinics and hospitals within reach treat the sick by their daily capacity, and a home with
//! sick people nobody treats has the SICK problem. The sick stay home from work (the routine).
//!
//! * **Falling ill** (each tick, the citizen bucket `id % 256 == tick % 256`, 16 visits a game
//!   day): a healthy citizen at home gets `sick_since` with chance `(sick_base + sick_pollution ×
//!   pollution at the home) / 16`.
//! * **Treatment** (each tick, the building bucket `index % 64 == tick % 64`, as police): the
//!   sick of a building with Health coverage are cured one capacity unit each by the nearest
//!   powered clinic or hospital with room; the others keep SICK on the building.

use bevy_ecs::system::{Res, ResMut};
use cp_building::{BuildingGrid, BuildingState};
use cp_citizen::CitizenGrid;
use cp_citizen::system::CITIZEN_BUCKETS;
use cp_content::{ServiceKind, SimContent};
use cp_core::time::TICKS_PER_DAY;
use cp_core::{SimRng, Tick};
use cp_net::NetGraph;
use cp_service::{
    COVERAGE_PERIOD, ServiceCoverage, ServiceProblems, ServiceState, nearest_facility,
};

use crate::land::LandGrids;

/// The RNG stream of this system (`SimRng::fork`).
pub const HEALTH_STREAM: u64 = 0x4EA1;

const VISITS_PER_DAY: f32 = (TICKS_PER_DAY / CITIZEN_BUCKETS as u64) as f32;

/// Economy phase, after `update_police`.
#[allow(clippy::too_many_arguments)]
pub fn update_health(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    net: Res<NetGraph>,
    buildings: Res<BuildingGrid>,
    land: Res<LandGrids>,
    coverage: Res<ServiceCoverage>,
    rng: Res<SimRng>,
    mut grid: ResMut<CitizenGrid>,
    mut state: ResMut<ServiceState>,
) {
    let Some(content) = content else { return };
    let db = &content.0;
    let bal = db.balance();
    grid.ensure_derived();

    // --- falling ill ---
    let mut rng = rng.fork(HEALTH_STREAM, *tick);
    let bucket = (tick.0 % u64::from(CITIZEN_BUCKETS)) as u32;
    let healthy: Vec<_> = grid
        .citizens_bucket(CITIZEN_BUCKETS, bucket)
        .filter(|(_, c)| c.sick_since.is_none())
        .filter_map(|(id, c)| Some((id, c.home?)))
        .collect();
    for (id, home) in healthy {
        let Some(b) = buildings.get(home) else {
            continue;
        };
        let p = (bal.sick_base + bal.sick_pollution * land.pollution_at(b.pos)) / VISITS_PER_DAY;
        if rng.next_f32() < p {
            grid.set_sick(id, Some(*tick));
        }
    }

    // --- treatment ---
    let bucket = tick.0 % COVERAGE_PERIOD;
    for (id, b) in buildings.iter() {
        if u64::from(id.index) % COVERAGE_PERIOD != bucket || b.state != BuildingState::Active {
            continue;
        }
        let sick: Vec<_> = grid.sick_in(id).collect();
        let mut untreated = sick.len();
        if untreated > 0 && coverage.coverage_of(ServiceKind::Health, b) > 0.0 {
            let mut station = None;
            for cid in sick {
                if station.is_none() {
                    station =
                        nearest_facility(&net, &buildings, db, &state, ServiceKind::Health, id);
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
    }
}
