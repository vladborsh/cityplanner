//! Move-in, job assignment and move-out (`architecture/system-design.md` §9.1).
//!
//! One `Citizens`-phase system, three bucketed passes per tick (§2), all deterministic
//! (arena order; RNG from `SimRng::fork(MOVE_IN_STREAM, tick)`):
//!
//! * **Households** (1/256 per tick): the home must still exist and be `Active`, else the
//!   household is evicted (apartment and jobs freed). Arrival: once `arrive_at` has passed
//!   the members go from `Moving` to `Home`. If no adult has had a job for
//!   [`JOBLESS_LEAVE_DAYS`] game days the household leaves the city.
//! * **Citizens** (1/256 per tick): a job whose building is gone or not `Active` is lost;
//!   an adult at home without a job takes the best free slot: **education match first**
//!   (a slot of exactly their level), then — if none is free — the highest lower level
//!   (overqualified), and among candidates of a level the **nearest by straight-line
//!   distance** between the home and the workplace (pathfinding does not exist yet; the
//!   traffic task can switch this to path cost). Ties go to the lower building id.
//! * **Move-in** (buildings, 1/64 per tick): an `Active` residential building with a free
//!   apartment (`BuildingDef.households` per building) and residential demand > 0
//!   (`DemandStub`, temporary) receives a household with probability `1/MOVE_IN_ONE_IN`
//!   per visit. The household is created at once but its members stay `Moving` for
//!   [`FALLBACK_DELAY`] ticks, or — with a map — [`BASE_DELAY`] plus the time to drive from
//!   the nearest external connection at [`ARRIVAL_M_PER_TICK`] (15 m/s). Size 1–5; the first
//!   member is an adult, the others adults with probability 0.7; education of adults
//!   0/1/2/3 with 20/50/25/5 %; children have education 0; 70 % own a car.

use bevy_ecs::prelude::*;
use bevy_math::Vec2;
use cp_building::{BuildingGrid, BuildingId, BuildingState, DemandStub};
use cp_content::{ContentDb, SimContent, ZoneType};
use cp_core::time::TICKS_PER_DAY;
use cp_core::{SimLimits, SimRng, Tick};
use cp_terrain::SimTerrain;

use crate::grid::CitizenGrid;
use crate::model::{
    ADULT_AGE_DAYS, CitizenId, DAYS_PER_YEAR, HOUSEHOLD_MAX, HouseholdId, Location,
};

/// Citizens and households are visited in 256 buckets (§2).
pub const CITIZEN_BUCKETS: u32 = 256;
/// Buildings are visited in 64 buckets (§2).
pub const BUILDING_BUCKETS: u32 = 64;
pub const MOVE_IN_STREAM: u64 = 0xC171;
/// A building with a free apartment receives a household with probability `1 / this` per visit.
pub const MOVE_IN_ONE_IN: u32 = 2;
/// Arrival delay without a map, ticks (half a game day).
pub const FALLBACK_DELAY: u64 = 2048;
/// Fixed part of the arrival delay with a map, ticks.
pub const BASE_DELAY: u64 = 256;
/// Driving speed from the external connection: 15 m/s at 20 ticks/s.
pub const ARRIVAL_M_PER_TICK: f32 = 0.75;
/// A household with no employed adult for this many game days leaves.
pub const JOBLESS_LEAVE_DAYS: u64 = 20;

fn is_residential(z: ZoneType) -> bool {
    matches!(z, ZoneType::ResLow | ZoneType::ResHigh)
}

fn arrival_delay(terrain: Option<&SimTerrain>, at: Vec2) -> u64 {
    let Some(t) = terrain else {
        return FALLBACK_DELAY;
    };
    let nearest =
        t.0.meta()
            .external_connections
            .iter()
            .map(|&(x, z)| Vec2::new(x, z).distance(at))
            .min_by(f32::total_cmp);
    match nearest {
        Some(d) => BASE_DELAY + (d / ARRIVAL_M_PER_TICK).ceil() as u64,
        None => FALLBACK_DELAY,
    }
}

/// A building offering jobs, with its position, for the job search.
struct JobSite {
    id: BuildingId,
    pos: Vec2,
    slots: [u16; 4],
}

fn job_sites(buildings: &BuildingGrid, db: &ContentDb) -> Vec<JobSite> {
    buildings
        .iter()
        .filter(|(_, b)| b.state == BuildingState::Active)
        .filter_map(|(id, b)| {
            let slots = db.building(b.def).jobs;
            (slots.iter().any(|&s| s > 0)).then_some(JobSite {
                id,
                pos: b.pos,
                slots,
            })
        })
        .collect()
}

/// Best free slot for a citizen of `education` living at `home`: `(site index, level)`.
fn find_job(
    sites: &[JobSite],
    grid: &CitizenGrid,
    home: Vec2,
    education: u8,
) -> Option<(usize, u8)> {
    for level in (0..=education).rev() {
        let l = usize::from(level);
        let mut best: Option<(f32, usize)> = None;
        for (i, s) in sites.iter().enumerate() {
            if s.slots[l] > grid.jobs_filled(s.id, l) {
                let d = s.pos.distance_squared(home);
                if best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((d, i));
                }
            }
        }
        if let Some((_, i)) = best {
            return Some((i, level));
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
pub fn update_citizens(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    buildings: Res<BuildingGrid>,
    demand: Res<DemandStub>,
    terrain: Option<Res<SimTerrain>>,
    rng: Res<SimRng>,
    limits: Res<SimLimits>,
    trips: Option<Res<crate::trips::TripBook>>,
    mut grid: ResMut<CitizenGrid>,
) {
    let Some(content) = content else { return };
    let db = &content.0;
    let now = tick.0;
    grid.ensure_derived();

    // --- households: eviction, arrival, long unemployment ---
    let bucket = (now % u64::from(CITIZEN_BUCKETS)) as u32;
    let households: Vec<HouseholdId> = grid
        .households_bucket(CITIZEN_BUCKETS, bucket)
        .map(|(id, _)| id)
        .collect();
    for hid in households {
        let Some(h) = grid.household(hid).cloned() else {
            continue;
        };
        let home_ok = buildings
            .get(h.home)
            .is_some_and(|b| b.state == BuildingState::Active);
        if !home_ok {
            grid.remove_household(hid);
            continue;
        }
        let arrived = now >= h.arrive_at.0;
        if arrived {
            for cid in h.member_ids() {
                // A member on a trip is `Moving` for another reason; leave them be.
                let on_trip = trips.as_ref().is_some_and(|t| t.active_of(cid).is_some());
                if !on_trip
                    && let Some(c) = grid.citizen_mut(cid)
                    && c.location == Location::Moving
                {
                    c.location = Location::Home;
                }
            }
        }
        let (mut adults, mut employed) = (0, 0);
        for cid in h.member_ids() {
            if let Some(c) = grid.citizen(cid)
                && c.is_adult()
            {
                adults += 1;
                employed += usize::from(c.work.is_some());
            }
        }
        let since = if arrived && adults > 0 && employed == 0 {
            Some(h.jobless_since.unwrap_or(Tick(now)))
        } else {
            None
        };
        if let Some(s) = since
            && now - s.0 >= JOBLESS_LEAVE_DAYS * TICKS_PER_DAY
        {
            grid.remove_household(hid);
            continue;
        }
        if since != h.jobless_since
            && let Some(hh) = grid.household_mut(hid)
        {
            hh.jobless_since = since;
        }
    }

    // --- citizens: lost jobs, job search ---
    let citizens: Vec<CitizenId> = grid
        .citizens_bucket(CITIZEN_BUCKETS, bucket)
        .map(|(id, _)| id)
        .collect();
    let mut sites: Option<Vec<JobSite>> = None;
    for cid in citizens {
        let Some(c) = grid.citizen(cid).cloned() else {
            continue;
        };
        if let Some(w) = c.work
            && !buildings
                .get(w)
                .is_some_and(|b| b.state == BuildingState::Active)
        {
            grid.clear_job(cid);
        }
        let work = grid.citizen(cid).and_then(|c| c.work);
        if work.is_some() || !c.is_adult() || c.location == Location::Moving {
            continue;
        }
        let Some(home) = c.home.and_then(|h| buildings.get(h)) else {
            continue;
        };
        let sites = sites.get_or_insert_with(|| job_sites(&buildings, db));
        if let Some((i, level)) = find_job(sites, &grid, home.pos, c.education) {
            let site = sites[i].id;
            grid.set_job(cid, site, level);
        }
    }

    // --- move-in ---
    let mut rng = rng.fork(MOVE_IN_STREAM, *tick);
    let bucket = (now % u64::from(BUILDING_BUCKETS)) as u32;
    let visits: Vec<(BuildingId, ZoneType, u16, Vec2)> = buildings
        .iter_bucket(BUILDING_BUCKETS, bucket)
        .filter(|(_, b)| b.state == BuildingState::Active)
        .filter_map(|(id, b)| {
            let def = db.building(b.def);
            (is_residential(def.zone) && def.households > 0).then_some((
                id,
                def.zone,
                def.households,
                b.pos,
            ))
        })
        .collect();
    for (id, zone, apartments, pos) in visits {
        let gate = rng.below(MOVE_IN_ONE_IN);
        if gate != 0
            || demand.get(zone) <= 0.0
            || grid.households_in(id) >= apartments
            || grid.household_count() >= limits.households as usize
            || grid.citizen_count() + HOUSEHOLD_MAX > limits.citizens as usize
        {
            continue;
        }
        let size = 1 + rng.below(HOUSEHOLD_MAX as u32) as usize;
        let mut members = Vec::with_capacity(size);
        for i in 0..size {
            let adult = i == 0 || rng.below(10) < 7;
            let (age, edu) = if adult {
                let age = ADULT_AGE_DAYS + rng.below(40 * DAYS_PER_YEAR);
                let edu = match rng.below(100) {
                    0..20 => 0,
                    20..70 => 1,
                    70..95 => 2,
                    _ => 3,
                };
                (age, edu)
            } else {
                (rng.below(ADULT_AGE_DAYS), 0)
            };
            members.push((age, edu, rng.below(100) < 70));
        }
        let wealth = 50_000 + i64::from(rng.below(50_000));
        let arrive = Tick(now + arrival_delay(terrain.as_deref(), pos));
        grid.add_household(id, wealth, arrive, &members);
    }
}
