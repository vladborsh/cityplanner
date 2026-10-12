//! `CityStats`: the city's summary for the UI panels (`architecture/system-design.md` §13.6).
//!
//! A **derived** snapshot: recomputed by [`update_city_stats`] in the `Stats` phase every
//! [`STATS_PERIOD`] = 64 ticks in one pass over the buildings, the zone blocks, the road
//! segments and the citizens, published as `SimView::city`. It is not part of `SaveData` and not
//! of the checksum, and no simulation system reads it; after a load it is filled at the next
//! multiple of 64 ticks and its history starts again.
//!
//! The history is a ring of [`HISTORY_HOURS`] hourly samples (7 game days); a sample is taken at
//! the first recompute of each game hour (`tick * 24 / TICKS_PER_DAY` changes; an hour is 170
//! ticks, so every hour has one), giving exactly 24 samples per game day.

use std::collections::VecDeque;

use bevy_ecs::prelude::*;
use cp_building::{BuildingFlags, BuildingGrid, BuildingState, Demand};
use cp_citizen::{CitizenGrid, TripBook, TripState};
use cp_content::{RoadDefId, ServiceKind, SimContent, UtilityKind, ZoneType};
use cp_core::time::TICKS_PER_DAY;
use cp_core::{Cost, Tick, Treasury};
use cp_economy::{EconomyState, LandGrids, segment_upkeep};
use cp_net::NetGraph;
use cp_traffic::Agents;
use cp_zone::{ZoneCellRef, ZoneGrid};

/// Ticks between two recomputes (the 64 buckets of §2).
pub const STATS_PERIOD: u64 = 64;
/// Hourly samples kept: 7 game days.
pub const HISTORY_HOURS: usize = 7 * 24;
/// A segment loaded above this share of its capacity counts as overloaded.
pub const OVERLOAD: f32 = 0.8;

/// The painted zone types, in the order of [`CityStats::zones`].
pub const ZONES: [ZoneType; 6] = [
    ZoneType::ResLow,
    ZoneType::ResHigh,
    ZoneType::ComLow,
    ZoneType::ComHigh,
    ZoneType::Industrial,
    ZoneType::Office,
];

/// Index of `zone` in [`ZONES`]; `None` for `ZoneType::None`.
pub fn zone_index(zone: ZoneType) -> Option<usize> {
    (zone as usize).checked_sub(1)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ZoneStats {
    /// Painted cells.
    pub painted: u32,
    /// Painted cells that are valid (free to build on).
    pub valid: u32,
    /// Painted cells covered by a building's lot.
    pub occupied: u32,
    /// Active and abandoned buildings of this zone by level 1..=5.
    pub by_level: [u32; 5],
    pub constructing: u32,
    pub abandoned: u32,
    /// Homes (households that fit) / jobs of the active buildings, and how many are free.
    pub homes: u32,
    pub homes_free: u32,
    pub jobs: u32,
    pub jobs_free: u32,
    /// Demand of the zone's group (R, C, I or O).
    pub demand: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BuildingStats {
    pub total: u32,
    pub constructing: u32,
    pub active: u32,
    pub abandoned: u32,
    pub burned: u32,
    pub no_water: u32,
    pub no_power: u32,
    pub no_sewage: u32,
    pub no_road: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TrafficStats {
    pub vehicles: u32,
    pub pedestrians: u32,
    /// Trips under way (not waiting for their departure).
    pub trips_in_transit: u32,
    /// Mean 2-hour load of all segments, `0..=1`.
    pub avg_load: f32,
    pub max_load: f32,
    /// Segments with a load above [`OVERLOAD`].
    pub overloaded: u32,
    pub despawned_stuck: u64,
    pub no_path: u64,
    pub abstract_trips: u64,
}

/// One utility network (water, sewage, power).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NetStats {
    /// Active source buildings.
    pub sources: u32,
    pub capacity: f32,
    /// What the active buildings consume.
    pub consumption: f32,
    /// Active buildings that lack this supply.
    pub unsupplied: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LandStats {
    pub avg_value: f32,
    pub avg_pollution: f32,
    pub max_pollution: f32,
    pub avg_noise: f32,
}

/// The roads of one road type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoadGroup {
    pub road: RoadDefId,
    pub segments: u32,
    pub meters: f32,
    /// Weekly upkeep at the current roads budget.
    pub upkeep_week: Cost,
}

/// One hourly sample for the sparklines.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HistorySample {
    pub tick: u64,
    pub population: u32,
    pub employed: u32,
    pub balance: Cost,
    pub avg_load: f32,
    pub pollution: f32,
    /// R, C, I, O.
    pub demand: [f32; 4],
}

#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct CityStats {
    /// Changes at every recompute (cache key of the published snapshot).
    pub generation: u64,
    /// Tick of the last recompute.
    pub tick: u64,
    pub population: u32,
    pub households: u32,
    pub adults: u32,
    pub employed: u32,
    pub unemployed: u32,
    pub zones: [ZoneStats; 6],
    pub buildings: BuildingStats,
    pub traffic: TrafficStats,
    /// Water, sewage, power.
    pub utilities: [NetStats; 3],
    pub land: LandStats,
    /// By road type, ascending `RoadDefId`; road types without segments are absent.
    pub roads: Vec<RoadGroup>,
    pub history: VecDeque<HistorySample>,
    /// Game hour (`tick * 24 / TICKS_PER_DAY`) of the newest history sample.
    last_hour: Option<u64>,
    /// The recompute in progress (between its first and last stage), not published.
    next: Option<Box<CityStats>>,
}

impl CityStats {
    /// Index of a utility in [`CityStats::utilities`].
    pub fn utility_index(kind: UtilityKind) -> usize {
        match kind {
            UtilityKind::Water => 0,
            UtilityKind::Sewage => 1,
            UtilityKind::Power => 2,
        }
    }
}

/// Ticks (of the 64-tick period) at which the stages run: citizens, traffic, roads and land;
/// zone cells; the even buildings; the odd buildings; publishing. Spread over the period so the
/// pass is four ~0.4 ms slices instead of one ~1.5 ms hitch at 100k citizens.
const STAGE_TICKS: [u64; 5] = [0, 1, 2, 3, 4];

/// `Stats`-phase system: recomputes [`CityStats`] every [`STATS_PERIOD`] ticks, in stages (see
/// [`STAGE_TICKS`]); the published value changes at the last stage only.
#[allow(clippy::too_many_arguments)]
pub fn update_city_stats(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    buildings: Res<BuildingGrid>,
    zones: Res<ZoneGrid>,
    citizens: Res<CitizenGrid>,
    net: Res<NetGraph>,
    agents: Res<Agents>,
    trips: Res<TripBook>,
    land: Res<LandGrids>,
    demand: Res<Demand>,
    treasury: Res<Treasury>,
    economy: Res<EconomyState>,
    mut stats: ResMut<CityStats>,
) {
    let phase = tick.0 % STATS_PERIOD;
    let Some(stage) = STAGE_TICKS.iter().position(|&t| t == phase) else {
        return;
    };
    let Some(content) = content else { return };
    let db = &*content.0;
    let demands = [demand.r, demand.c, demand.i, demand.o];
    if stage == 0 {
        let mut next = CityStats::default();
        stage_citizens_traffic_land(
            &mut next, db, &citizens, &net, &agents, &trips, &land, &economy, &demands,
        );
        stats.next = Some(Box::new(next));
        return;
    }
    let Some(mut next) = stats.next.take() else {
        return; // joined mid-period (after a load): wait for the next one
    };
    match stage {
        1 => stage_zone_cells(&mut next, &zones, &demands),
        2 | 3 => stage_buildings(
            &mut next,
            db,
            &buildings,
            &zones,
            &citizens,
            (stage - 2) as u64,
        ),
        _ => {
            // Publish.
            let hour = tick.0 * 24 / TICKS_PER_DAY;
            let mut history = std::mem::take(&mut stats.history);
            let mut last_hour = stats.last_hour;
            if last_hour != Some(hour) {
                last_hour = Some(hour);
                if history.len() == HISTORY_HOURS {
                    history.pop_front();
                }
                history.push_back(HistorySample {
                    tick: tick.0,
                    population: next.population,
                    employed: next.employed,
                    balance: treasury.balance,
                    avg_load: next.traffic.avg_load,
                    pollution: next.land.avg_pollution,
                    demand: demands,
                });
            }
            let generation = stats.generation + 1;
            *stats = *next;
            stats.generation = generation;
            stats.tick = tick.0;
            stats.history = history;
            stats.last_hour = last_hour;
            return;
        }
    }
    stats.next = Some(next);
}

/// Stage 0: citizens (one pass), traffic and roads (segments), land grids.
#[allow(clippy::too_many_arguments)]
fn stage_citizens_traffic_land(
    s: &mut CityStats,
    db: &cp_content::ContentDb,
    citizens: &CitizenGrid,
    net: &NetGraph,
    agents: &Agents,
    trips: &TripBook,
    land: &LandGrids,
    economy: &EconomyState,
    _demands: &[f32; 4],
) {
    s.population = citizens.citizen_count() as u32;
    s.households = citizens.household_count() as u32;
    for (_, c) in citizens.citizens() {
        if c.is_adult() {
            s.adults += 1;
            if c.work.is_some() {
                s.employed += 1;
            }
        }
    }
    s.unemployed = s.adults - s.employed;

    s.traffic.vehicles = agents.vehicle_count() as u32;
    s.traffic.pedestrians = agents.pedestrian_count() as u32;
    s.traffic.trips_in_transit = trips
        .iter()
        .filter(|(_, t)| !matches!(t.state, TripState::Scheduled { .. }))
        .count() as u32;
    s.traffic.despawned_stuck = agents.stats.despawned_stuck;
    s.traffic.no_path = agents.stats.no_path;
    s.traffic.abstract_trips = agents.stats.abstract_trips;
    let (mut load_sum, mut segs) = (0.0f32, 0u32);
    let roads_budget = economy.budgets.get(ServiceKind::Roads);
    let mut groups: Vec<Option<RoadGroup>> = Vec::new();
    for (id, seg) in net.segments() {
        let load = agents
            .load
            .ema
            .get(id.index as usize)
            .copied()
            .unwrap_or(0.0);
        load_sum += load;
        segs += 1;
        s.traffic.max_load = s.traffic.max_load.max(load);
        s.traffic.overloaded += u32::from(load > OVERLOAD);
        let i = seg.road.index();
        if groups.len() <= i {
            groups.resize(i + 1, None);
        }
        let g = groups[i].get_or_insert(RoadGroup {
            road: seg.road,
            segments: 0,
            meters: 0.0,
            upkeep_week: Cost::ZERO,
        });
        g.segments += 1;
        g.meters += seg.length;
        g.upkeep_week += segment_upkeep(db.road(seg.road), seg, roads_budget);
    }
    s.traffic.avg_load = if segs > 0 {
        load_sum / segs as f32
    } else {
        0.0
    };
    s.roads = groups.into_iter().flatten().collect();

    let n = land.land_value.len().max(1) as f32;
    s.land = LandStats {
        avg_value: land.land_value.iter().sum::<f32>() / n,
        avg_pollution: land.pollution.iter().sum::<f32>() / n,
        max_pollution: land.pollution.iter().copied().fold(0.0, f32::max),
        avg_noise: land.noise.iter().sum::<f32>() / n,
    };
}

/// Stage 1: painted and valid zone cells, the demand of each zone's group.
fn stage_zone_cells(s: &mut CityStats, zones: &ZoneGrid, demands: &[f32; 4]) {
    for (_, block) in zones.blocks() {
        for (i, &z) in block.zones.iter().enumerate() {
            if let Some(k) = zone_index(z) {
                s.zones[k].painted += 1;
                s.zones[k].valid += u32::from(block.valid[i]);
            }
        }
    }
    let group = |z: ZoneType| match z {
        ZoneType::ResLow | ZoneType::ResHigh => 0,
        ZoneType::ComLow | ZoneType::ComHigh => 1,
        ZoneType::Industrial => 2,
        _ => 3,
    };
    for (k, z) in ZONES.iter().enumerate() {
        s.zones[k].demand = demands[group(*z)];
    }
}

/// Stages 2 and 3: the buildings whose arena index has parity `half`.
fn stage_buildings(
    s: &mut CityStats,
    db: &cp_content::ContentDb,
    buildings: &BuildingGrid,
    zones: &ZoneGrid,
    citizens: &CitizenGrid,
    half: u64,
) {
    let sources_of = |name: &str| db.service_id(name).map(|id| db.service(id));
    for (id, b) in buildings.iter() {
        if u64::from(id.index) % 2 != half {
            continue;
        }
        let def = db.building(b.def);
        s.buildings.total += 1;
        match b.state {
            BuildingState::UnderConstruction => s.buildings.constructing += 1,
            BuildingState::Active => s.buildings.active += 1,
            BuildingState::Abandoned => s.buildings.abandoned += 1,
            BuildingState::Burned => s.buildings.burned += 1,
        }
        let f = b.flags;
        s.buildings.no_water += u32::from(f.has(BuildingFlags::NO_WATER));
        s.buildings.no_power += u32::from(f.has(BuildingFlags::NO_POWER));
        s.buildings.no_sewage += u32::from(f.has(BuildingFlags::NO_SEWAGE));
        s.buildings.no_road += u32::from(f.has(BuildingFlags::NO_ROAD));

        // Lot cells by the painted type under them.
        for cell in b.lot.cells() {
            if let Some(k) = painted_zone(zones, cell).and_then(zone_index) {
                s.zones[k].occupied += 1;
            }
        }
        if let Some(k) = zone_index(def.zone)
            && def.service.is_none()
        {
            let z = &mut s.zones[k];
            match b.state {
                BuildingState::UnderConstruction => z.constructing += 1,
                BuildingState::Abandoned => {
                    z.abandoned += 1;
                    z.by_level[usize::from(b.level.clamp(1, 5)) - 1] += 1;
                }
                BuildingState::Active => {
                    z.by_level[usize::from(b.level.clamp(1, 5)) - 1] += 1;
                    z.homes += u32::from(def.households);
                    z.homes_free +=
                        u32::from(def.households.saturating_sub(citizens.households_in(id)));
                    for l in 0..4 {
                        z.jobs += u32::from(def.jobs[l]);
                        z.jobs_free +=
                            u32::from(def.jobs[l].saturating_sub(citizens.jobs_filled(id, l)));
                    }
                }
                BuildingState::Burned => {}
            }
        }
        if b.state == BuildingState::Active {
            for (kind, v) in &def.consumption {
                s.utilities[CityStats::utility_index(*kind)].consumption += *v;
            }
            if let Some(svc) = def.service.as_deref().and_then(sources_of) {
                let k = match svc.kind {
                    ServiceKind::Water => Some(0),
                    ServiceKind::Sewage => Some(1),
                    ServiceKind::Power => Some(2),
                    _ => None,
                };
                if let Some(k) = k {
                    s.utilities[k].sources += 1;
                    s.utilities[k].capacity += svc.capacity as f32;
                }
            }
            s.utilities[0].unsupplied += u32::from(f.has(BuildingFlags::NO_WATER));
            s.utilities[1].unsupplied += u32::from(f.has(BuildingFlags::NO_SEWAGE));
            s.utilities[2].unsupplied += u32::from(f.has(BuildingFlags::NO_POWER));
        }
    }
}

/// The painted type of the zone cell `c`.
fn painted_zone(zones: &ZoneGrid, c: ZoneCellRef) -> Option<ZoneType> {
    zones
        .block(c.block)
        .and_then(|b| b.zones.get(usize::from(c.cell)))
        .copied()
}
