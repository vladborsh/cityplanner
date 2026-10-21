//! The citizen and household arenas, plus derived occupancy tables.

use std::collections::{BTreeMap, BTreeSet};

use bevy_ecs::resource::Resource;
use cp_building::BuildingId;
use cp_core::{Arena, Tick};
use serde::{Deserialize, Serialize};

use crate::model::{Citizen, CitizenId, HOUSEHOLD_MAX, Household, HouseholdId, Location};

/// A table keyed by `BuildingId` stored densely by arena index; an entry carries the generation
/// of the building it belongs to, so a building that reuses a removed building's slot starts
/// empty (the key is the full id: index **and generation**).
#[derive(Clone, Debug, Default)]
struct PerBuilding<T> {
    slots: Vec<Option<(u32, T)>>,
}

impl<T: Copy + Default + PartialEq> PerBuilding<T> {
    fn get(&self, id: BuildingId) -> Option<&T> {
        match self.slots.get(id.index as usize) {
            Some(Some((g, v))) if *g == id.generation => Some(v),
            _ => None,
        }
    }

    fn get_mut(&mut self, id: BuildingId) -> Option<&mut T> {
        match self.slots.get_mut(id.index as usize) {
            Some(Some((g, v))) if *g == id.generation => Some(v),
            _ => None,
        }
    }

    /// The entry of `id`, created (default) if absent or left over from an older generation.
    fn entry(&mut self, id: BuildingId) -> &mut T {
        let i = id.index as usize;
        if self.slots.len() <= i {
            self.slots.resize(i + 1, None);
        }
        let slot = &mut self.slots[i];
        if !matches!(slot, Some((g, _)) if *g == id.generation) {
            *slot = Some((id.generation, T::default()));
        }
        &mut slot.as_mut().expect("just set").1
    }

    fn remove(&mut self, id: BuildingId) {
        if let Some(Some((g, _))) = self.slots.get(id.index as usize)
            && *g == id.generation
        {
            self.slots[id.index as usize] = None;
        }
    }
}

/// Per-building counts derived from the arenas (see [`PerBuilding`]).
#[derive(Clone, Debug, Default)]
struct Derived {
    ready: bool,
    /// Households living in the building.
    homes: PerBuilding<u16>,
    /// Filled job slots per education level.
    jobs: PerBuilding<[u16; 4]>,
    /// The sick citizens by the building they live in (T-139).
    sick: BTreeMap<BuildingId, BTreeSet<CitizenId>>,
    /// The pupils of each school (T-291: the education phase counted them over every citizen
    /// every tick).
    pupils: PerBuilding<u32>,
}

impl Derived {
    fn add_home(&mut self, b: BuildingId) {
        *self.homes.entry(b) += 1;
    }

    fn remove_home(&mut self, b: BuildingId) {
        if let Some(n) = self.homes.get_mut(b) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.homes.remove(b);
            }
        }
    }

    fn add_sick(&mut self, home: BuildingId, id: CitizenId) {
        self.sick.entry(home).or_default().insert(id);
    }

    fn remove_sick(&mut self, home: BuildingId, id: CitizenId) {
        if let Some(set) = self.sick.get_mut(&home) {
            set.remove(&id);
            if set.is_empty() {
                self.sick.remove(&home);
            }
        }
    }

    fn add_pupil(&mut self, school: BuildingId) {
        *self.pupils.entry(school) += 1;
    }

    fn remove_pupil(&mut self, school: BuildingId) {
        if let Some(n) = self.pupils.get_mut(school) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.pupils.remove(school);
            }
        }
    }

    fn add_job(&mut self, b: BuildingId, level: usize) {
        self.jobs.entry(b)[level] += 1;
    }

    fn remove_job(&mut self, b: BuildingId, level: usize) {
        if let Some(j) = self.jobs.get_mut(b) {
            j[level] = j[level].saturating_sub(1);
            if *j == [0; 4] {
                self.jobs.remove(b);
            }
        }
    }
}

/// Running totals of the people who came, were born, left and died (T-179, the top bar's
/// population trend). Derived bookkeeping: not saved, not part of equality or the checksum; it
/// starts again from zero after a load.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PopFlow {
    pub arrived: u64,
    pub born: u64,
    pub left: u64,
    pub died: u64,
}

/// All citizens and households.
#[derive(Resource, Clone, Debug, Default, Serialize, Deserialize)]
pub struct CitizenGrid {
    citizens: Arena<Citizen>,
    households: Arena<Household>,
    /// Bodies waiting at each deathcare building (T-135); the deathcare row fills it.
    pub dead: BTreeMap<BuildingId, u16>,
    #[serde(skip)]
    derived: Derived,
    /// Who came, was born, left and died since the start of this run.
    #[serde(skip)]
    pub flow: PopFlow,
}

impl PartialEq for CitizenGrid {
    fn eq(&self, other: &Self) -> bool {
        self.citizens == other.citizens
            && self.households == other.households
            && self.dead == other.dead
    }
}

impl CitizenGrid {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drops the school of every citizen whose school `gone` says is demolished.
    pub fn clear_schools(&mut self, gone: impl Fn(BuildingId) -> bool) {
        for (_, c) in self.citizens.iter_mut() {
            if let Some(s) = c.school.filter(|s| gone(*s)) {
                c.school = None;
                self.derived.remove_pupil(s);
            }
        }
    }

    /// The arenas as saved, for the frozen save types.
    pub fn parts(&self) -> (&Arena<Citizen>, &Arena<Household>) {
        (&self.citizens, &self.households)
    }

    /// A grid from saved arenas; the occupancy tables are rebuilt on demand.
    pub fn from_parts(
        citizens: Arena<Citizen>,
        households: Arena<Household>,
        dead: BTreeMap<BuildingId, u16>,
    ) -> Self {
        Self {
            citizens,
            households,
            dead,
            derived: Derived::default(),
            flow: PopFlow::default(),
        }
    }

    pub fn citizen(&self, id: CitizenId) -> Option<&Citizen> {
        self.citizens.get(id)
    }

    pub fn household(&self, id: HouseholdId) -> Option<&Household> {
        self.households.get(id)
    }

    pub fn citizens(&self) -> impl Iterator<Item = (CitizenId, &Citizen)> {
        self.citizens.iter()
    }

    pub fn households(&self) -> impl Iterator<Item = (HouseholdId, &Household)> {
        self.households.iter()
    }

    pub fn citizens_bucket(
        &self,
        buckets: u32,
        bucket: u32,
    ) -> impl Iterator<Item = (CitizenId, &Citizen)> {
        self.citizens.iter_bucket(buckets, bucket)
    }

    pub fn households_bucket(
        &self,
        buckets: u32,
        bucket: u32,
    ) -> impl Iterator<Item = (HouseholdId, &Household)> {
        self.households.iter_bucket(buckets, bucket)
    }

    pub fn citizen_count(&self) -> usize {
        self.citizens.len()
    }

    pub fn household_count(&self) -> usize {
        self.households.len()
    }

    pub fn adults(&self) -> usize {
        self.citizens.iter().filter(|(_, c)| c.is_adult()).count()
    }

    pub fn employed(&self) -> usize {
        self.citizens
            .iter()
            .filter(|(_, c)| c.work.is_some())
            .count()
    }

    /// Adults without a job.
    pub fn unemployed(&self) -> usize {
        self.citizens
            .iter()
            .filter(|(_, c)| c.is_adult() && c.work.is_none())
            .count()
    }

    /// Rebuilds the occupancy tables if they are stale (after a load).
    pub fn ensure_derived(&mut self) {
        if self.derived.ready {
            return;
        }
        let mut d = Derived::default();
        for (_, h) in self.households.iter() {
            d.add_home(h.home);
        }
        for (id, c) in self.citizens.iter() {
            if let Some(w) = c.work {
                d.add_job(w, usize::from(c.work_level));
            }
            if c.sick_since.is_some()
                && let Some(h) = c.home
            {
                d.add_sick(h, id);
            }
            if let Some(s) = c.school {
                d.add_pupil(s);
            }
        }
        d.ready = true;
        self.derived = d;
    }

    /// Households living in `building` (needs [`Self::ensure_derived`]).
    pub fn households_in(&self, building: BuildingId) -> u16 {
        self.derived.homes.get(building).copied().unwrap_or(0)
    }

    /// The sick citizens living in `building`, in id order (needs [`Self::ensure_derived`]).
    pub fn sick_in(&self, building: BuildingId) -> impl Iterator<Item = CitizenId> + '_ {
        self.derived
            .sick
            .get(&building)
            .into_iter()
            .flatten()
            .copied()
    }

    /// Sets or cures the sickness of a citizen (`since = None` cures).
    pub fn set_sick(&mut self, id: CitizenId, since: Option<Tick>) {
        let Some(c) = self.citizens.get_mut(id) else {
            return;
        };
        if let Some(h) = c.home {
            match (c.sick_since.is_some(), since.is_some()) {
                (false, true) => self.derived.add_sick(h, id),
                (true, false) => self.derived.remove_sick(h, id),
                _ => {}
            }
        }
        c.sick_since = since;
    }

    /// Enrols the citizen in `school`, or takes them out of it (`None`).
    pub fn set_school(&mut self, id: CitizenId, school: Option<BuildingId>) {
        if let Some(c) = self.citizens.get_mut(id) {
            if let Some(old) = std::mem::replace(&mut c.school, school) {
                self.derived.remove_pupil(old);
            }
            if let Some(new) = school {
                self.derived.add_pupil(new);
            }
        }
    }

    /// The citizens enrolled in `school` (needs [`Self::ensure_derived`]).
    pub fn pupils_in(&self, school: BuildingId) -> u32 {
        self.derived.pupils.get(school).copied().unwrap_or(0)
    }

    /// Sets a citizen's age in game days (tests and scenarios).
    pub fn set_age_days(&mut self, id: CitizenId, age_days: u32) {
        if let Some(c) = self.citizens.get_mut(id) {
            c.age_days = age_days;
        }
    }

    /// Sets the education of a citizen (a graduation).
    pub fn set_education(&mut self, id: CitizenId, education: u8) {
        if let Some(c) = self.citizens.get_mut(id) {
            c.education = education;
        }
    }

    /// Filled job slots of `level` in `building`.
    pub fn jobs_filled(&self, building: BuildingId, level: usize) -> u16 {
        self.derived.jobs.get(building).map_or(0, |j| j[level])
    }

    /// Creates a household and its members. `members` are `(age_days, education, has_car)`
    /// plus wellbeing; at most [`HOUSEHOLD_MAX`]. They start `Moving` until `arrive_at`.
    pub fn add_household(
        &mut self,
        home: BuildingId,
        wealth: i64,
        arrive_at: Tick,
        members: &[(u32, u8, bool)],
    ) -> HouseholdId {
        debug_assert!(self.derived.ready);
        let hid = self.households.insert(Household {
            members: [None; HOUSEHOLD_MAX],
            home,
            wealth,
            arrive_at,
            jobless_since: None,
        });
        let mut slots = [None; HOUSEHOLD_MAX];
        for (i, &(age_days, education, has_car)) in members.iter().take(HOUSEHOLD_MAX).enumerate() {
            slots[i] = Some(self.citizens.insert(Citizen {
                age_days,
                education,
                household: hid,
                home: Some(home),
                work: None,
                work_level: 0,
                location: Location::Moving,
                wellbeing: 50,
                has_car,
                sick_since: None,
                school: None,
            }));
        }
        self.households.get_mut(hid).expect("just inserted").members = slots;
        self.flow.arrived += slots.iter().flatten().count() as u64;
        self.derived.add_home(home);
        hid
    }

    /// A household's car has driven in (T-118): the whole household of `citizen` is at home now.
    pub fn household_arrived(&mut self, citizen: CitizenId, now: u64) {
        let Some(hid) = self.citizen(citizen).map(|c| c.household) else {
            return;
        };
        let Some(h) = self.household_mut(hid) else {
            return;
        };
        h.arrive_at = Tick(h.arrive_at.0.min(now));
        let members: Vec<CitizenId> = h.member_ids().collect();
        for cid in members {
            if let Some(c) = self.citizen_mut(cid) {
                c.location = Location::Home;
            }
        }
    }

    /// Puts the arrival of the household of `citizen` off to `until` at the earliest (T-167: it
    /// comes by train). Never brings it forward.
    pub fn delay_household(&mut self, citizen: CitizenId, until: Tick) {
        let Some(hid) = self.citizen(citizen).map(|c| c.household) else {
            return;
        };
        if let Some(h) = self.household_mut(hid) {
            h.arrive_at = Tick(h.arrive_at.0.max(until.0));
        }
    }

    /// Removes a household and all its members, freeing the apartment and their jobs.
    pub fn remove_household(&mut self, id: HouseholdId) -> bool {
        let Some(h) = self.households.remove(id) else {
            return false;
        };
        for cid in h.members.into_iter().flatten() {
            if let Some(c) = self.citizens.remove(cid) {
                self.flow.left += 1;
                if let Some(w) = c.work {
                    self.derived.remove_job(w, usize::from(c.work_level));
                }
                if let (Some(_), Some(home)) = (c.sick_since, c.home) {
                    self.derived.remove_sick(home, cid);
                }
                if let Some(s) = c.school {
                    self.derived.remove_pupil(s);
                }
            }
        }
        self.derived.remove_home(h.home);
        true
    }

    /// Moves a household and its members to `home` (T-252: evicted from a demolished house), with
    /// their sickness bookkeeping; jobs, schools and where they are stay as they are.
    pub fn move_household(&mut self, id: HouseholdId, home: BuildingId) -> bool {
        let Some(h) = self.households.get_mut(id) else {
            return false;
        };
        let old = std::mem::replace(&mut h.home, home);
        let members: Vec<CitizenId> = h.member_ids().collect();
        for cid in members {
            if let Some(c) = self.citizens.get_mut(cid) {
                c.home = Some(home);
                if c.sick_since.is_some() {
                    self.derived.remove_sick(old, cid);
                    self.derived.add_sick(home, cid);
                }
            }
        }
        self.derived.remove_home(old);
        self.derived.add_home(home);
        true
    }

    /// Removes one citizen (death): frees their job and their place in the household, and the
    /// household too when nobody is left. Returns the citizen.
    pub fn remove_citizen(&mut self, id: CitizenId) -> Option<Citizen> {
        let c = self.citizens.remove(id)?;
        self.flow.died += 1;
        if let Some(w) = c.work {
            self.derived.remove_job(w, usize::from(c.work_level));
        }
        if let (Some(_), Some(home)) = (c.sick_since, c.home) {
            self.derived.remove_sick(home, id);
        }
        if let Some(s) = c.school {
            self.derived.remove_pupil(s);
        }
        let mut empty = false;
        if let Some(h) = self.households.get_mut(c.household) {
            for slot in h.members.iter_mut().filter(|m| **m == Some(id)) {
                *slot = None;
            }
            empty = h.members.iter().all(Option::is_none);
        }
        if empty {
            self.remove_household(c.household);
        }
        Some(c)
    }

    /// Adds a newborn (0 days, education 0) to a household with a free place, at home.
    pub fn add_newborn(&mut self, hid: HouseholdId) -> Option<CitizenId> {
        let h = self.households.get(hid)?;
        let slot = h.members.iter().position(Option::is_none)?;
        let home = h.home;
        let cid = self.citizens.insert(Citizen {
            age_days: 0,
            education: 0,
            household: hid,
            home: Some(home),
            work: None,
            work_level: 0,
            location: Location::Home,
            wellbeing: 50,
            has_car: false,
            sick_since: None,
            school: None,
        });
        self.households.get_mut(hid)?.members[slot] = Some(cid);
        self.flow.born += 1;
        Some(cid)
    }

    /// Gives a citizen a job slot of `level` in `building`.
    pub fn set_job(&mut self, id: CitizenId, building: BuildingId, level: u8) {
        self.clear_job(id);
        if let Some(c) = self.citizens.get_mut(id) {
            c.work = Some(building);
            c.work_level = level;
            self.derived.add_job(building, usize::from(level));
        }
    }

    pub fn clear_job(&mut self, id: CitizenId) {
        let Some(c) = self.citizens.get_mut(id) else {
            return;
        };
        if let Some(w) = c.work.take() {
            self.derived.remove_job(w, usize::from(c.work_level));
        }
    }

    /// Moves a citizen to `loc` (the routine and the agents drive this state machine; tests
    /// and scenarios set it directly).
    pub fn set_location(&mut self, id: CitizenId, loc: Location) {
        if let Some(c) = self.citizens.get_mut(id) {
            c.location = loc;
        }
    }

    pub(crate) fn citizen_mut(&mut self, id: CitizenId) -> Option<&mut Citizen> {
        self.citizens.get_mut(id)
    }

    pub(crate) fn household_mut(&mut self, id: HouseholdId) -> Option<&mut Household> {
        self.households.get_mut(id)
    }
}

#[cfg(test)]
mod pupil_tests {
    use super::*;

    /// T-291: the pupil counts are what counting the citizens' schools gives, through
    /// enrolments, moves between schools, deaths, households leaving, demolished schools and a
    /// rebuild after a load.
    #[test]
    fn the_pupil_counts_follow_the_citizens() {
        let home = BuildingId::new(0, 0);
        let schools = [BuildingId::new(1, 0), BuildingId::new(2, 0)];
        let mut g = CitizenGrid::new();
        g.ensure_derived();
        let hs: Vec<HouseholdId> = (0..6)
            .map(|_| g.add_household(home, 0, Tick(0), &[(3000, 0, false), (2000, 0, false)]))
            .collect();
        let count = |g: &CitizenGrid, s: BuildingId| {
            g.citizens().filter(|(_, c)| c.school == Some(s)).count() as u32
        };
        let check = |g: &CitizenGrid| {
            for s in schools {
                assert_eq!(g.pupils_in(s), count(g, s), "{s:?}");
            }
        };
        let ids: Vec<CitizenId> = g.citizens().map(|(id, _)| id).collect();
        for (k, id) in ids.iter().enumerate() {
            g.set_school(*id, Some(schools[k % 2]));
        }
        check(&g);
        g.set_school(ids[0], Some(schools[1]));
        g.set_school(ids[1], None);
        check(&g);
        g.remove_citizen(ids[2]);
        g.remove_household(hs[3]);
        check(&g);
        g.clear_schools(|s| s == schools[0]);
        check(&g);
        assert_eq!(g.pupils_in(schools[0]), 0);
        // A load: the counts are derived again from the arenas.
        let (c, h) = g.parts();
        let mut loaded = CitizenGrid::from_parts(c.clone(), h.clone(), g.dead.clone());
        loaded.ensure_derived();
        check(&loaded);
        assert!(loaded.pupils_in(schools[1]) > 0);
    }
}
