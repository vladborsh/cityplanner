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
//!   apartment (`BuildingDef.households` per building) and residential move-in demand > 0
//!   (`Demand::move_in`) receives a household with probability `1/MOVE_IN_ONE_IN`
//!   per visit. The household is created at once but its members stay `Moving` for
//!   [`FALLBACK_DELAY`] ticks, or — with a map — [`BASE_DELAY`] plus the time to drive from
//!   the nearest external connection at [`ARRIVAL_SPEED_MPS`] (15 m/s, on the movement clock of §2: `ceil(d / 15 / MOVE_SECONDS_PER_TICK)` ticks). Size 1–5; the first
//!   member is an adult, the others adults with probability 0.7; education of adults
//!   0/1/2/3 with 20/50/25/5 %; children have education 0; `Balance.car_ownership` (70 %) own a car.

use bevy_ecs::prelude::*;
use bevy_math::Vec2;
use cp_building::{BuildingGrid, BuildingId, BuildingState, Demand};
use cp_content::{ContentDb, SimContent, ZoneType};
use cp_core::time::{MOVE_SECONDS_PER_TICK, TICKS_PER_DAY};
use cp_core::{SimLimits, SimRng, Tick};
use cp_net::EntryReach;
use cp_terrain::SimTerrain;

use crate::grid::CitizenGrid;
use crate::model::{
    ADULT_AGE_DAYS, CitizenId, DAYS_PER_YEAR, HOUSEHOLD_MAX, HouseholdId, Location,
};
use crate::trips::{Trip, TripMode, TripPurpose, TripState};

/// Citizens and households are visited in 256 buckets (§2).
pub const CITIZEN_BUCKETS: u32 = 256;
/// Buildings are visited in 64 buckets (§2).
pub const BUILDING_BUCKETS: u32 = 64;
pub const MOVE_IN_STREAM: u64 = 0xC171;
/// A building with a free apartment receives a household with probability `1 / this` per visit.
/// User 2026-10-22: halved the arrival rate (was 2) — the game pace was too fast.
pub const MOVE_IN_ONE_IN: u32 = 4;
/// Arrival delay without a map, ticks (half a game day).
pub const FALLBACK_DELAY: u64 = 2048;
/// Fixed part of the arrival delay with a map, ticks.
pub const BASE_DELAY: u64 = 256;
/// Driving speed from the external connection: 15 m/s at 20 ticks/s.
pub const ARRIVAL_SPEED_MPS: f32 = 15.0;
/// A household with no employed adult for this many game days leaves.
pub const JOBLESS_LEAVE_DAYS: u64 = 20;

fn is_residential(z: ZoneType) -> bool {
    matches!(z, ZoneType::ResLow | ZoneType::ResHigh)
}

fn arrival_delay(terrain: Option<&SimTerrain>, reach: Option<&EntryReach>, at: Vec2) -> u64 {
    // With an entry highway the household drives in from its `Outside` node (§5.1).
    if let Some(d) = reach
        .filter(|r| r.is_active())
        .and_then(|r| r.distance_to_outside(at))
    {
        return BASE_DELAY + (d / ARRIVAL_SPEED_MPS / MOVE_SECONDS_PER_TICK).ceil() as u64;
    }
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
        Some(d) => BASE_DELAY + (d / ARRIVAL_SPEED_MPS / MOVE_SECONDS_PER_TICK).ceil() as u64,
        None => FALLBACK_DELAY,
    }
}

/// A building offering jobs, with its position, for the job search.
struct JobSite {
    id: BuildingId,
    pos: Vec2,
    slots: [u16; 4],
}

/// Side of a job-index cell, metres.
const JOB_CELL: f32 = 128.0;
/// Cells per side of the job index (the map is 10.24 km).
const JOB_CELLS: usize = (cp_terrain::MAP_SIZE / JOB_CELL) as usize + 1;

fn job_cell(p: Vec2) -> (usize, usize) {
    let c = |v: f32| ((v / JOB_CELL).max(0.0) as usize).min(JOB_CELLS - 1);
    (c(p.x), c(p.y))
}

/// The job search: the buildings offering jobs, binned in a grid with the number of free
/// slots per cell and level, so "nearest site with a free slot" looks at a few cells instead
/// of every workplace for every seeker. Built once per tick when someone looks for work and
/// kept exact while that tick assigns jobs ([`JobIndex::taken`]); the free counts are read from
/// the live occupancy at build time. Results equal a scan of all sites in id order: the
/// nearest free site by straight-line distance, ties to the lower building id.
/// A jobseeker takes a free job anywhere in the town, a cell's chance falling off as
/// `exp(-distance / COMMUTE_DECAY_M)` (BUG-066): typical commutes are one to three kilometres
/// instead of the building next door.
const COMMUTE_DECAY_M: f32 = 1500.0;

/// A uniform `0..1` from `h` (a splitmix step), for the deterministic choices by id.
pub(crate) fn unit(h: u64) -> f32 {
    let mut z = h.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32
}

struct JobIndex {
    sites: Vec<JobSite>,
    /// CSR layout: the sites of cell `c` are `items[start[c]..start[c + 1]]` (ascending index).
    start: Vec<u32>,
    items: Vec<u32>,
    free_cell: Vec<[u32; 4]>,
    free_total: [u32; 4],
    /// The cells that hold any site, ascending.
    cells: Vec<u32>,
}

impl JobIndex {
    fn build(buildings: &BuildingGrid, db: &ContentDb, grid: &CitizenGrid) -> Self {
        Self::from_sites(job_sites(buildings, db), &|id, l| grid.jobs_filled(id, l))
    }

    fn from_sites(sites: Vec<JobSite>, filled: &dyn Fn(BuildingId, usize) -> u16) -> Self {
        let cell_of = |s: &JobSite| {
            let (x, z) = job_cell(s.pos);
            z * JOB_CELLS + x
        };
        let mut start = vec![0u32; JOB_CELLS * JOB_CELLS + 1];
        for s in &sites {
            start[cell_of(s) + 1] += 1;
        }
        for c in 0..JOB_CELLS * JOB_CELLS {
            start[c + 1] += start[c];
        }
        let mut fill = start.clone();
        let mut items = vec![0u32; sites.len()];
        let mut free_cell = vec![[0u32; 4]; JOB_CELLS * JOB_CELLS];
        let mut free_total = [0u32; 4];
        for (i, s) in sites.iter().enumerate() {
            let c = cell_of(s);
            items[fill[c] as usize] = i as u32;
            fill[c] += 1;
            for l in 0..4 {
                let free = u32::from(s.slots[l].saturating_sub(filled(s.id, l)));
                free_cell[c][l] += free;
                free_total[l] += free;
            }
        }
        let cells = (0..JOB_CELLS * JOB_CELLS)
            .filter(|&c| start[c + 1] > start[c])
            .map(|c| c as u32)
            .collect();
        Self {
            sites,
            start,
            items,
            free_cell,
            free_total,
            cells,
        }
    }

    /// A job at site `i`, `level`, was given away.
    fn taken(&mut self, i: usize, level: u8) {
        let (x, z) = job_cell(self.sites[i].pos);
        self.free_cell[z * JOB_CELLS + x][usize::from(level)] -= 1;
        self.free_total[usize::from(level)] -= 1;
    }

    /// A free slot for a citizen of `education` living at `home`, chosen across the town: the
    /// highest level up to `education` with a free slot anywhere, then a cell with chance
    /// `free slots × exp(-distance / COMMUTE_DECAY_M)` (`pick` is the uniform draw), then the
    /// nearest free site of that cell. Deterministic.
    fn find_weighted(
        &self,
        filled: &dyn Fn(BuildingId, usize) -> u16,
        home: Vec2,
        education: u8,
        pick: f32,
    ) -> Option<(usize, u8)> {
        for level in (0..=education).rev() {
            let l = usize::from(level);
            if self.free_total[l] == 0 {
                continue;
            }
            let weight = |c: u32| {
                let free = self.free_cell[c as usize][l];
                if free == 0 {
                    return 0.0;
                }
                let (cx, cz) = (c as usize % JOB_CELLS, c as usize / JOB_CELLS);
                let centre = Vec2::new((cx as f32 + 0.5) * JOB_CELL, (cz as f32 + 0.5) * JOB_CELL);
                free as f32 * (-centre.distance(home) / COMMUTE_DECAY_M).exp()
            };
            let total: f32 = self.cells.iter().map(|&c| weight(c)).sum();
            let mut at = pick * total;
            let mut chosen = None;
            for &c in &self.cells {
                let w = weight(c);
                if w <= 0.0 {
                    continue;
                }
                chosen = Some(c as usize);
                if at < w {
                    break;
                }
                at -= w;
            }
            let c = chosen?;
            let mut best: Option<(f32, usize)> = None;
            for &i in &self.items[self.start[c] as usize..self.start[c + 1] as usize] {
                let st = &self.sites[i as usize];
                if st.slots[l] > filled(st.id, l) {
                    let d = st.pos.distance_squared(home);
                    if best.is_none_or(|(bd, bi)| d < bd || (d == bd && (i as usize) < bi)) {
                        best = Some((d, i as usize));
                    }
                }
            }
            if let Some((_, i)) = best {
                return Some((i, level));
            }
        }
        None
    }

    /// Best free slot for a citizen of `education` living at `home`: `(site index, level)` —
    /// the highest level up to `education` that has a free slot anywhere, and there the nearest
    /// site (ties: lower index).
    #[cfg(test)]
    fn find(
        &self,
        filled: &dyn Fn(BuildingId, usize) -> u16,
        home: Vec2,
        education: u8,
    ) -> Option<(usize, u8)> {
        self.find_among(filled, home, education, 1, 0)
    }

    /// The nearest of the `k` free sites ordered by distance picked by `pick % n`.
    #[cfg(test)]
    fn find_among(
        &self,
        filled: &dyn Fn(BuildingId, usize) -> u16,
        home: Vec2,
        education: u8,
        k: usize,
        pick: u32,
    ) -> Option<(usize, u8)> {
        for level in (0..=education).rev() {
            let l = usize::from(level);
            if self.free_total[l] == 0 {
                continue;
            }
            let (hx, hz) = job_cell(home);
            // The k nearest free sites so far, nearest first: `(distance², index)`.
            let mut best: Vec<(f32, usize)> = Vec::with_capacity(k + 1);
            for r in 0..JOB_CELLS {
                // Ring `r`: the cells at Chebyshev distance `r` from the home cell.
                let (x0, x1) = (hx as isize - r as isize, hx as isize + r as isize);
                let (z0, z1) = (hz as isize - r as isize, hz as isize + r as isize);
                for cz in z0.max(0)..=z1.min(JOB_CELLS as isize - 1) {
                    let edge_row = cz == z0 || cz == z1;
                    let step = if edge_row { 1 } else { (x1 - x0).max(1) };
                    let mut cx = x0;
                    while cx <= x1 {
                        if cx >= 0 && cx < JOB_CELLS as isize {
                            let c = cz as usize * JOB_CELLS + cx as usize;
                            if self.free_cell[c][l] > 0 {
                                for &i in
                                    &self.items[self.start[c] as usize..self.start[c + 1] as usize]
                                {
                                    let s = &self.sites[i as usize];
                                    if s.slots[l] > filled(s.id, l) {
                                        let d = s.pos.distance_squared(home);
                                        let at = best.partition_point(|&(bd, bi)| {
                                            bd < d || (bd == d && bi < i as usize)
                                        });
                                        if at < k {
                                            best.insert(at, (d, i as usize));
                                            best.truncate(k);
                                        }
                                    }
                                }
                            }
                        }
                        cx += step;
                    }
                }
                // Everything not yet looked at is at least `r` cells away.
                if best.len() == k {
                    let reach = r as f32 * JOB_CELL;
                    if best[k - 1].0 < reach * reach {
                        break;
                    }
                }
            }
            if !best.is_empty() {
                let (_, i) = best[pick as usize % best.len()];
                return Some((i, level));
            }
        }
        None
    }
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

#[allow(clippy::too_many_arguments)]
pub fn update_citizens(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    buildings: Res<BuildingGrid>,
    demand: Res<Demand>,
    terrain: Option<Res<SimTerrain>>,
    reach: Option<Res<EntryReach>>,
    rng: Res<SimRng>,
    limits: Res<SimLimits>,
    mut trips: Option<ResMut<crate::trips::TripBook>>,
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
        // Children whose adults have died leave the city (T-139).
        if arrived && adults == 0 {
            grid.remove_household(hid);
            continue;
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
    let mut sites: Option<JobIndex> = None;
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
        // A student (T-141) does not take a job.
        if work.is_some() || !c.is_adult() || c.school.is_some() || c.location == Location::Moving {
            continue;
        }
        let Some(home) = c.home.and_then(|h| buildings.get(h)) else {
            continue;
        };
        let index = sites.get_or_insert_with(|| JobIndex::build(&buildings, db, &grid));
        if let Some((i, level)) = index.find_weighted(
            &|id, l| grid.jobs_filled(id, l),
            home.pos,
            c.education,
            unit(u64::from(cid.index)),
        ) {
            let site = index.sites[i].id;
            grid.set_job(cid, site, level);
            index.taken(i, level);
        }
    }

    // --- move-in ---
    let mut rng = rng.fork(MOVE_IN_STREAM, *tick);
    let bucket = (now % u64::from(BUILDING_BUCKETS)) as u32;
    let reach = reach.as_deref();
    let entry_rule = reach.is_some_and(EntryReach::is_active);
    let visits: Vec<(BuildingId, u16, Vec2)> = buildings
        .iter_bucket(BUILDING_BUCKETS, bucket)
        .filter(|(_, b)| b.state == BuildingState::Active)
        // §5.1: with an entry highway only houses connected to it receive households.
        .filter(|(_, b)| {
            !entry_rule
                || b.access()
                    .is_some_and(|a| reach.is_some_and(|r| r.reaches(a.segment)))
        })
        .filter_map(|(id, b)| {
            let def = db.building(b.def);
            (is_residential(def.zone) && def.households > 0).then_some((id, def.households, b.pos))
        })
        .collect();
    let parks = cp_building::park_sites(db, &buildings);
    // The same draw as the old `below(100) < 70`, so 0.7 keeps the stream and the outcome.
    let owners = (db.balance().car_ownership * 100.0).round() as u32;
    for (id, apartments, pos) in visits {
        let gate = rng.below(MOVE_IN_ONE_IN);
        // A second draw every visit, so the stream does not depend on the outcome (T-237).
        let bonus = rng.below(100);
        if !cp_building::move_in_passes(
            gate,
            bonus,
            MOVE_IN_ONE_IN,
            cp_building::green_at(&parks, pos),
        ) || demand.move_in <= 0.0
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
            members.push((age, edu, rng.below(100) < owners));
        }
        let wealth = 50_000 + i64::from(rng.below(50_000));
        let arrive = Tick(now + arrival_delay(terrain.as_deref(), reach, pos));
        let hid = grid.add_household(id, wealth, arrive, &members);
        // With an entry highway the first member drives in from the Outside node (T-118);
        // `arrive` stays the abstract arrival if there is no route or no free agent.
        if entry_rule && let Some(book) = trips.as_deref_mut() {
            book.ensure_derived();
            let driver = grid.household(hid).and_then(|h| h.member_ids().next());
            if let Some(citizen) = driver {
                book.add(Trip {
                    citizen,
                    from: id,
                    to: id,
                    purpose: TripPurpose::MoveIn,
                    mode: TripMode::Car,
                    state: TripState::Planned,
                });
            }
        }
    }
}

/// T-252: the households of the homes in `gone` (demolished at the positions given, under a
/// service placed over them) move to the nearest vacant home in the city: an active residential
/// building with a free apartment, nearest first, then the lower id. Households are taken in id
/// order. Those that find none stay pointed at the demolished home and leave the city with the
/// purge of removed buildings (the bulldozer's path). Returns `(moved, left)` households.
pub fn rehome_evicted(
    grid: &mut CitizenGrid,
    buildings: &BuildingGrid,
    db: &ContentDb,
    gone: &[(BuildingId, Vec2)],
) -> (u32, u32) {
    if gone.is_empty() {
        return (0, 0);
    }
    grid.ensure_derived();
    let evicted: Vec<(HouseholdId, Vec2)> = grid
        .households()
        .filter_map(|(id, h)| {
            gone.iter()
                .find(|(b, _)| *b == h.home)
                .map(|(_, at)| (id, *at))
        })
        .collect();
    let homes: Vec<(BuildingId, u16, Vec2)> = buildings
        .iter()
        .filter(|(id, b)| b.state == BuildingState::Active && !gone.iter().any(|(g, _)| g == id))
        .filter_map(|(id, b)| {
            let def = db.building(b.def);
            (is_residential(def.zone) && def.households > 0).then_some((id, def.households, b.pos))
        })
        .collect();
    let (mut moved, mut left) = (0, 0);
    for (hid, at) in evicted {
        let to = homes
            .iter()
            .filter(|(id, apartments, _)| grid.households_in(*id) < *apartments)
            .min_by(|a, b| {
                a.2.distance_squared(at)
                    .total_cmp(&b.2.distance_squared(at))
                    .then(a.0.cmp(&b.0))
            })
            .map(|(id, ..)| *id);
        match to {
            Some(home) => {
                grid.move_household(hid, home);
                moved += 1;
            }
            None => left += 1,
        }
    }
    (moved, left)
}

#[cfg(test)]
mod job_index_tests {
    use super::*;

    /// The scan the index replaces: nearest free site of the highest level, ties to the lower
    /// index.
    fn brute(
        sites: &[JobSite],
        filled: &dyn Fn(BuildingId, usize) -> u16,
        home: Vec2,
        education: u8,
    ) -> Option<(usize, u8)> {
        for level in (0..=education).rev() {
            let l = usize::from(level);
            let mut best: Option<(f32, usize)> = None;
            for (i, s) in sites.iter().enumerate() {
                if s.slots[l] > filled(s.id, l) {
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

    #[test]
    fn the_weighted_search_commutes_across_the_town_and_is_deterministic() {
        // A row of sites every 100 m for 4 km, one slot of level 0 each.
        let sites: Vec<JobSite> = (0..40u32)
            .map(|i| JobSite {
                id: BuildingId::new(i, 0),
                pos: Vec2::new(100.0 + i as f32 * 100.0, 500.0),
                slots: [1, 0, 0, 0],
            })
            .collect();
        let get = |_: BuildingId, _: usize| 0u16;
        let index = JobIndex::from_sites(sites, &get);
        let home = Vec2::new(100.0, 500.0);
        let mut far = 0;
        for h in 0..400u64 {
            let (i, level) = index.find_weighted(&get, home, 0, unit(h)).unwrap();
            assert_eq!(level, 0);
            assert_eq!(
                index.find_weighted(&get, home, 0, unit(h)),
                Some((i, level))
            );
            far += usize::from(index.sites[i].pos.distance(home) > 400.0);
        }
        // Most seekers do not take the job next door.
        assert!(far > 250, "{far} of 400 commute more than 400 m");
        // No free slot anywhere: nothing.
        let full = |_: BuildingId, _: usize| 1u16;
        let empty = JobIndex::from_sites(
            vec![JobSite {
                id: BuildingId::new(0, 0),
                pos: home,
                slots: [1, 0, 0, 0],
            }],
            &full,
        );
        assert_eq!(empty.find_weighted(&full, home, 0, 0.5), None);
    }

    #[test]
    fn the_index_finds_what_a_full_scan_finds_while_slots_fill_up() {
        let mut rng = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move |n: u64| {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng % n
        };
        // Sites on a coarse lattice (many exact distance ties) and at random, a few slots each.
        let mut sites = Vec::new();
        for i in 0..600u32 {
            let pos = if i % 2 == 0 {
                Vec2::new(next(40) as f32 * 64.0, next(40) as f32 * 64.0)
            } else {
                Vec2::new(next(10_200) as f32, next(10_200) as f32)
            };
            sites.push(JobSite {
                id: BuildingId::new(i, 0),
                pos,
                slots: [
                    next(3) as u16,
                    next(3) as u16,
                    next(2) as u16,
                    (next(4) == 0) as u16,
                ],
            });
        }
        let filled = std::cell::RefCell::new(vec![[0u16; 4]; sites.len()]);
        let get = |id: BuildingId, l: usize| filled.borrow()[id.index as usize][l];
        let mut index = JobIndex::from_sites(
            sites
                .iter()
                .map(|s| JobSite {
                    id: s.id,
                    pos: s.pos,
                    slots: s.slots,
                })
                .collect(),
            &get,
        );
        let mut assigned = 0;
        for _ in 0..4000 {
            let home = Vec2::new(next(10_240) as f32, next(10_240) as f32);
            let edu = next(4) as u8;
            let want = brute(&sites, &get, home, edu);
            assert_eq!(index.find(&get, home, edu), want, "home {home:?} edu {edu}");
            if let Some((i, level)) = want {
                filled.borrow_mut()[i][usize::from(level)] += 1;
                index.taken(i, level);
                assigned += 1;
            }
        }
        assert!(assigned > 500, "{assigned} jobs given");
    }
}
