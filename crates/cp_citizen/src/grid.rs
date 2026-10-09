//! The citizen and household arenas, plus derived occupancy tables.

use bevy_ecs::resource::Resource;
use cp_building::BuildingId;
use cp_core::{Arena, Tick};
use serde::{Deserialize, Serialize};

use crate::model::{Citizen, CitizenId, HOUSEHOLD_MAX, Household, HouseholdId, Location};

/// Per-building counts derived from the arenas (indexed by building slot index).
#[derive(Clone, Debug, Default)]
struct Derived {
    ready: bool,
    /// Households living in the building.
    homes: Vec<u16>,
    /// Filled job slots per education level.
    jobs: Vec<[u16; 4]>,
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

fn bump<T: Default + Clone>(v: &mut Vec<T>, i: usize) -> &mut T {
    if v.len() <= i {
        v.resize(i + 1, T::default());
    }
    &mut v[i]
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
            *bump(&mut d.homes, h.home.index as usize) += 1;
        }
        for (_, c) in self.citizens.iter() {
            if let Some(w) = c.work {
                bump(&mut d.jobs, w.index as usize)[usize::from(c.work_level)] += 1;
            }
        }
        d.ready = true;
        self.derived = d;
    }

    /// Households living in `building` (needs [`Self::ensure_derived`]).
    pub fn households_in(&self, building: BuildingId) -> u16 {
        self.derived
            .homes
            .get(building.index as usize)
            .copied()
            .unwrap_or(0)
    }

    /// Filled job slots of `level` in `building`.
    pub fn jobs_filled(&self, building: BuildingId, level: usize) -> u16 {
        self.derived
            .jobs
            .get(building.index as usize)
            .map_or(0, |j| j[level])
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
        *bump(&mut self.derived.homes, home.index as usize) += 1;
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
                let j = bump(&mut self.derived.jobs, w.index as usize);
                let slot = &mut j[usize::from(c.work_level)];
                *slot = slot.saturating_sub(1);
            }
        }
        let slot = bump(&mut self.derived.homes, h.home.index as usize);
        *slot = slot.saturating_sub(1);
        true
    }

    /// Gives a citizen a job slot of `level` in `building`.
    pub fn set_job(&mut self, id: CitizenId, building: BuildingId, level: u8) {
        self.clear_job(id);
        if let Some(c) = self.citizens.get_mut(id) {
            c.work = Some(building);
            c.work_level = level;
            bump(&mut self.derived.jobs, building.index as usize)[usize::from(level)] += 1;
        }
    }

    pub fn clear_job(&mut self, id: CitizenId) {
        let Some(c) = self.citizens.get_mut(id) else {
            return;
        };
        if let Some(w) = c.work.take() {
            let level = usize::from(c.work_level);
            let slot = &mut bump(&mut self.derived.jobs, w.index as usize)[level];
            *slot = slot.saturating_sub(1);
        }
    }

    pub(crate) fn citizen_mut(&mut self, id: CitizenId) -> Option<&mut Citizen> {
        self.citizens.get_mut(id)
    }

    pub(crate) fn household_mut(&mut self, id: HouseholdId) -> Option<&mut Household> {
        self.households.get_mut(id)
    }
}
