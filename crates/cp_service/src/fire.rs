//! Fire (§16 M2 D "Пожар"): buildings catch fire by chance, fire stations send an engine when
//! one is free, and an unattended fire burns the building down.
//!
//! One system, [`update_fire`], on the 64-tick buckets of [`crate::garbage`]:
//! * an `Active` building without a fire ignites with `fire_base × (1 − fire_cover ×
//!   coverage(Fire))` per game day;
//! * (T-151) with `cp_traffic`'s [`ServiceRuns`] present the engine is a real vehicle: dispatch
//!   starts a run from the station and the fire is out `fire_extinguish_hours` after the run
//!   arrives (by driving, or abstractly when the agent pool is full); a failed run falls back
//!   to extinguishing at once (no travel time). Without `ServiceRuns` only the estimate exists;
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
use cp_content::{ContentDb, PolicyKind, ServiceKind, ServiceUnlocks, SimContent, VehicleKind};
use cp_core::time::{MOVE_SECONDS_PER_TICK, TICKS_PER_DAY, TICKS_PER_HOUR};
use cp_core::{SimRng, Tick};
use cp_district::PolicyParams;
use cp_net::NetGraph;
use cp_traffic::{RunState, ServiceRuns};
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
    runs: Option<&mut ServiceRuns>,
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
    // A real engine: the fire goes out when its run arrives (`attend`); else the estimate.
    let out_at = match runs {
        Some(runs) => {
            runs.start(VehicleKind::Emergency, station, id);
            None
        }
        None => Some(Tick(now.0 + travel + work)),
    };
    let s = state.get_mut(id);
    if let Some(f) = s.fire.as_mut() {
        f.out_at = out_at;
        f.station = Some(station);
    }
    true
}

/// Follows the engine of the fire in `id` (dispatched, no `out_at` yet): when its run has
/// arrived the extinguishing starts then; when it failed — or is gone — the road-distance
/// estimate from now on is used.
fn attend(
    state: &mut ServiceState,
    runs: &mut ServiceRuns,
    db: &ContentDb,
    id: BuildingId,
    station: BuildingId,
    now: Tick,
) {
    let work = (db.balance().fire_extinguish_hours * TICKS_PER_HOUR).round() as u64;
    let out_at = match runs.find(VehicleKind::Emergency, id) {
        Some((_, run)) => match run.state {
            RunState::Arrived { at } => at.0 + work,
            RunState::Failed => now.0 + work,
            _ => return,
        },
        None => now.0 + work,
    };
    if let Some((run, _)) = runs.find(VehicleKind::Emergency, id) {
        runs.remove(run);
    }
    if let Some(f) = state.get_mut(id).fire.as_mut() {
        f.out_at = Some(Tick(out_at));
        f.station = Some(station);
    }
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
    mut runs: Option<ResMut<ServiceRuns>>,
    policies: PolicyParams,
    unlocks: Option<Res<ServiceUnlocks>>,
) {
    let Some(content) = content else { return };
    // Worlds built without the resource have every service open.
    let unlocks = unlocks.map(|u| *u).unwrap_or_default();
    let db = &content.0;
    let bal = db.balance();
    let policies = policies.get();
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
                // No fire service is open yet: nothing catches fire (BUG-051).
                if !unlocks.open(ServiceKind::Fire) {
                    continue;
                }
                let cover = coverage.coverage_of(ServiceKind::Fire, b).clamp(0.0, 1.0);
                // SmokeDetectors (T-192) cut the chance.
                let detectors = policies
                    .as_ref()
                    .map_or(1.0, |p| p.factor_at(db, b.pos, PolicyKind::SmokeDetectors));
                let p = bal.fire_base * (1.0 - bal.fire_cover * cover) * detectors / PASSES_PER_DAY;
                if rng.next_f32() < p {
                    ignite(&mut state, id, now);
                    dispatch(
                        &net,
                        &buildings,
                        db,
                        &mut state,
                        runs.as_deref_mut(),
                        id,
                        now,
                    );
                }
            }
            (BuildingState::Active, Some(mut f)) => {
                // An engine on its way: its arrival starts the extinguishing.
                if f.out_at.is_none()
                    && let (Some(station), Some(runs)) = (f.station, runs.as_deref_mut())
                {
                    attend(&mut state, runs, db, id, station, now);
                    f = state.get(id).and_then(|s| s.fire).unwrap_or(f);
                }
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
                    // The engine that never came: free it and call off its run.
                    if let Some(st) = f.station
                        && state.facility(st).is_some()
                    {
                        let fac = state.facility_mut(st);
                        fac.stored = fac.stored.saturating_sub(1);
                    }
                    if let Some(runs) = runs.as_deref_mut()
                        && let Some((run, _)) = runs.find(VehicleKind::Emergency, id)
                    {
                        runs.remove(run);
                    }
                    burn(&mut buildings, id);
                    state.get_mut(id).problems.set(ServiceProblems::FIRE, false);
                    continue;
                } else if f.station.is_none() {
                    dispatch(
                        &net,
                        &buildings,
                        db,
                        &mut state,
                        runs.as_deref_mut(),
                        id,
                        now,
                    );
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
            dispatch(
                &net,
                &buildings,
                db,
                &mut state,
                runs.as_deref_mut(),
                nid,
                now,
            );
        }
    }
}
