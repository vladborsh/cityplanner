//! The citizen and household arenas, plus derived occupancy tables.

use std::collections::BTreeMap;

use bevy_ecs::resource::Resource;
use cp_building::BuildingId;
use cp_core::{Arena, Tick};
use serde::{Deserialize, Serialize};

use crate::model::{Citizen, CitizenId, HOUSEHOLD_MAX, Household, HouseholdId, Location};

/// Per-building counts derived from the arenas, keyed by the full `BuildingId` (index
/// **and generation**), so a building that reuses a removed building's slot starts empty.
#[derive(Clone, Debug, Default)]
struct Derived {
    ready: bool,
    /// Households living in the building.
    homes: BTreeMap<BuildingId, u16>,
    /// Filled job slots per education level.
    jobs: BTreeMap<BuildingId, [u16; 4]>,
}

impl Derived {
    fn add_home(&mut self, b: BuildingId) {
        *self.homes.entry(b).or_default() += 1;
    }

    fn remove_home(&mut self, b: BuildingId) {
        if let Some(n) = self.homes.get_mut(&b) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.homes.remove(&b);
            }
        }
    }

    fn add_job(&mut self, b: BuildingId, level: usize) {
        self.jobs.entry(b).or_default()[level] += 1;
    }

    fn remove_job(&mut self, b: BuildingId, level: usize) {
        if let Some(j) = self.jobs.get_mut(&b) {
            j[level] = j[level].saturating_sub(1);
            if *j == [0; 4] {
                self.jobs.remove(&b);
            }
        }
    }
}

/// All citizens and households.
#[derive(Resource, Clone, Debug, Default, Serialize, Deserialize)]
pub struct CitizenGrid {
    citizens: Arena<Citizen>,
    households: Arena<Household>,
    #[serde(skip)]
    derived: Derived,
}

impl PartialEq for CitizenGrid {
    fn eq(&self, other: &Self) -> bool {
        self.citizens == other.citizens && self.households == other.households
    }
}

impl CitizenGrid {
    pub fn new() -> Self {
        Self::default()
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
        for (_, c) in self.citizens.iter() {
            if let Some(w) = c.work {
                d.add_job(w, usize::from(c.work_level));
            }
        }
        d.ready = true;
        self.derived = d;
    }

    /// Households living in `building` (needs [`Self::ensure_derived`]).
    pub fn households_in(&self, building: BuildingId) -> u16 {
        self.derived.homes.get(&building).copied().unwrap_or(0)
    }

    /// Filled job slots of `level` in `building`.
    pub fn jobs_filled(&self, building: BuildingId, level: usize) -> u16 {
        self.derived.jobs.get(&building).map_or(0, |j| j[level])
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
            }));
        }
        self.households.get_mut(hid).expect("just inserted").members = slots;
        self.derived.add_home(home);
        hid
    }

    /// Removes a household and all its members, freeing the apartment and their jobs.
    pub fn remove_household(&mut self, id: HouseholdId) -> bool {
        let Some(h) = self.households.remove(id) else {
            return false;
        };
        for cid in h.members.into_iter().flatten() {
            if let Some(c) = self.citizens.remove(cid)
                && let Some(w) = c.work
            {
                self.derived.remove_job(w, usize::from(c.work_level));
            }
        }
        self.derived.remove_home(h.home);
        true
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

    pub(crate) fn citizen_mut(&mut self, id: CitizenId) -> Option<&mut Citizen> {
        self.citizens.get_mut(id)
    }

    pub(crate) fn household_mut(&mut self, id: HouseholdId) -> Option<&mut Household> {
        self.households.get_mut(id)
    }
}
