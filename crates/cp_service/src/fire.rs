//! Fire (§16 M2 D "Пожар"): buildings catch fire by chance, fire stations send an engine when
//! one is free, and an unattended fire burns the building down.
//!
//! One system, [`update_fire`], on the 64-tick buckets of [`crate::garbage`]:
//! * an `Active` building without a fire ignites with `fire_base × (1 − fire_cover ×
//!   coverage(Fire))` per game day;
//! * a fire looks for the nearest `Active`, powered station with an engine free (engines out
//!   are counted in the station's `Facility::stored`, the station's `capacity` is how many
//!   there are). The engine arrives after the road distance at `fire_engine_kmh` and works
//!   `fire_extinguish_hours`: then the fire is out and the building stays `Active`;
//! * `fire_burn_hours` without an engine make the building `Burned` (its people leave); after
//!   `burned_days` more it is demolished and its cells freed;
//! * a burning building ignites one neighbour within `fire_spread_m` with chance
//!   `fire_spread` per game hour.
//!
//! The `FIRE` problem bit is set while an `Active` building burns. A burned building keeps its
//! `Fire` record (the time of ignition) so the demolition time survives a save.

use bevy_ecs::system::{Res, ResMut};
use cp_building::{BuildingGrid, BuildingId, BuildingState, burn, demolish_building};
use cp_content::{ContentDb, ServiceKind, SimContent};
use cp_core::time::{MOVE_SECONDS_PER_TICK, TICKS_PER_DAY, TICKS_PER_HOUR};
use cp_core::{SimRng, Tick};
use cp_net::NetGraph;
use cp_zone::ZoneGrid;

use crate::coverage::{COVERAGE_PERIOD, ServiceCoverage};
use crate::facility::nearest_facility_with;
use crate::state::{Fire, ServiceProblems, ServiceState};

/// The RNG stream of this system (`SimRng::fork`).
pub const FIRE_STREAM: u64 = 0xF14E;

const PASSES_PER_DAY: f32 = (TICKS_PER_DAY / COVERAGE_PERIOD) as f32;

/// Sends the nearest free engine to the burning building `id`; `true` when one went.
fn dispatch(
    net: &NetGraph,
    buildings: &BuildingGrid,
    db: &ContentDb,
    state: &mut ServiceState,
    id: BuildingId,
    now: Tick,
) -> bool {
    let bal = db.balance();
    let Some((station, metres)) = nearest_facility_with(
        net,
        buildings,
        db,
        state,
        ServiceKind::Fire,
        id,
        |used, svc| used.stored < svc.capacity,
    ) else {
        return false;
    };
    let speed = (bal.fire_engine_kmh / 3.6).max(0.1);
    let travel = (metres / speed / MOVE_SECONDS_PER_TICK).ceil() as u64;
    let work = (bal.fire_extinguish_hours * TICKS_PER_HOUR).round() as u64;
    state.facility_mut(station).stored += 1;
    let s = state.get_mut(id);
    if let Some(f) = s.fire.as_mut() {
        f.out_at = Some(Tick(now.0 + travel + work));
        f.station = Some(station);
    }
    true
}

fn ignite(state: &mut ServiceState, id: BuildingId, now: Tick) {
    let s = state.get_mut(id);
    s.fire = Some(Fire {
        since: now,
        out_at: None,
        station: None,
    });
    s.problems.set(ServiceProblems::FIRE, true);
}

/// Services phase, after the coverage rebuild and the daily reset.
#[allow(clippy::too_many_arguments)]
pub fn update_fire(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    net: Res<NetGraph>,
    coverage: Res<ServiceCoverage>,
    rng: Res<SimRng>,
    mut buildings: ResMut<BuildingGrid>,
    mut zones: ResMut<ZoneGrid>,
    mut state: ResMut<ServiceState>,
) {
    let Some(content) = content else { return };
    let db = &content.0;
    let bal = db.balance();
    let now = *tick;
    let bucket = now.0 % COVERAGE_PERIOD;
    let mut rng = rng.fork(FIRE_STREAM, now);
    let burn_after = (bal.fire_burn_hours * TICKS_PER_HOUR).round() as u64;
    let stand_for = burn_after + (bal.burned_days * TICKS_PER_DAY as f32).round() as u64;
    // Per pass: the daily chance spread over the day, the hourly one over the hour.
    let per_pass = COVERAGE_PERIOD as f32 / TICKS_PER_HOUR;
    let spread_p = 1.0 - (1.0 - bal.fire_spread.clamp(0.0, 1.0)).powf(per_pass);
    let ids: Vec<BuildingId> = buildings
        .iter()
        .filter(|(id, _)| u64::from(id.index) % COVERAGE_PERIOD == bucket)
        .map(|(id, _)| id)
        .collect();
    let mut spread: Vec<BuildingId> = Vec::new();
    for id in ids {
        let Some(b) = buildings.get(id) else { continue };
        let fire = state.get(id).and_then(|s| s.fire);
        match (b.state, fire) {
            (BuildingState::Active, None) => {
                let cover = coverage.coverage_of(ServiceKind::Fire, b).clamp(0.0, 1.0);
                let p = bal.fire_base * (1.0 - bal.fire_cover * cover) / PASSES_PER_DAY;
                if rng.next_f32() < p {
                    ignite(&mut state, id, now);
                    dispatch(&net, &buildings, db, &mut state, id, now);
                }
            }
            (BuildingState::Active, Some(f)) => {
                if let Some(out) = f.out_at {
                    if now >= out {
                        if let Some(st) = f.station
                            && state.facility(st).is_some()
                        {
                            let fac = state.facility_mut(st);
                            fac.stored = fac.stored.saturating_sub(1);
                        }
                        let s = state.get_mut(id);
                        s.fire = None;
                        s.problems.set(ServiceProblems::FIRE, false);
                        continue;
                    }
                } else if now.0 - f.since.0 >= burn_after {
                    burn(&mut buildings, id);
                    state.get_mut(id).problems.set(ServiceProblems::FIRE, false);
                    continue;
                } else {
                    dispatch(&net, &buildings, db, &mut state, id, now);
                }
                if rng.next_f32() < spread_p {
                    spread.push(id);
                }
            }
            (BuildingState::Burned, Some(f)) if now.0 - f.since.0 >= stand_for => {
                demolish_building(&mut buildings, &mut zones, id);
            }
            _ => {}
        }
    }
    // Neighbours catch fire after the loop, so the order of the pass does not matter.
    for id in spread {
        let Some(from) = buildings.get(id).map(|b| b.pos) else {
            continue;
        };
        let reach = bal.fire_spread_m;
        let next = buildings.iter().find(|(nid, n)| {
            *nid != id
                && n.state == BuildingState::Active
                && n.pos.distance(from) <= reach
                && state.get(*nid).is_none_or(|s| s.fire.is_none())
        });
        if let Some((nid, _)) = next.map(|(i, b)| (i, b.pos)) {
            ignite(&mut state, nid, now);
            dispatch(&net, &buildings, db, &mut state, nid, now);
        }
    }
}
