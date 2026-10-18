//! The serialized per-building state of the M2 services (§16 M2 D "Состояние M2"). T-135
//! introduces the fields empty; the rows of garbage, health, deathcare, fire, police and
//! education fill them.

use std::collections::BTreeMap;

use bevy_ecs::resource::Resource;
use cp_building::BuildingId;
use cp_core::Tick;
use serde::{Deserialize, Serialize};

/// Problem bits of a building (append-only). A set bit means the building suffers the problem.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceProblems(pub u16);

impl ServiceProblems {
    pub const GARBAGE: u16 = 1;
    pub const SICK: u16 = 2;
    pub const DEAD: u16 = 4;
    pub const FIRE: u16 = 8;
    pub const CRIME: u16 = 16;
    /// Mail waiting for a van (T-247).
    pub const MAIL: u16 = 32;

    pub fn has(self, bit: u16) -> bool {
        self.0 & bit != 0
    }

    pub fn set(&mut self, bit: u16, on: bool) {
        if on {
            self.0 |= bit;
        } else {
            self.0 &= !bit;
        }
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// A fire in a building.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fire {
    pub since: Tick,
    /// When a fire engine has put it out; `None` while it burns unattended.
    pub out_at: Option<Tick>,
    /// The fire station whose engine is on its way or at work (it counts that engine in
    /// `Facility::stored`). Added with T-142: no save before it holds a fire, so the layout
    /// of the saved `None` is unchanged.
    pub station: Option<BuildingId>,
}

/// What the services know about one building. The slot is reset when the building's id
/// generation changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildingServices {
    /// Generation of the `BuildingId` this slot describes; [`BuildingServices::VACANT`] when
    /// it describes none.
    pub generation: u32,
    /// Garbage piled up, in the unit of the garbage row.
    pub garbage: u16,
    /// Crime accumulated.
    pub crime: u16,
    pub fire: Option<Fire>,
    pub problems: ServiceProblems,
}

impl BuildingServices {
    /// The generation of an unused slot (no id has it).
    pub const VACANT: u32 = u32::MAX;
}

impl Default for BuildingServices {
    fn default() -> Self {
        Self {
            generation: Self::VACANT,
            garbage: 0,
            crime: 0,
            fire: None,
            problems: ServiceProblems::default(),
        }
    }
}

/// Daily throughput and stock of a service building.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Facility {
    /// Served today; back to 0 at the start of every game day.
    pub used_today: u32,
    /// Stored (garbage, bodies); capped by the definition's `storage` when that is set.
    pub stored: u32,
}

/// All service state. Serialized sim state, part of the checksum.
#[derive(Resource, Clone, Debug, Default, Serialize, Deserialize)]
pub struct ServiceState {
    /// Dense by `BuildingId.index`.
    buildings: Vec<BuildingServices>,
    facilities: BTreeMap<BuildingId, Facility>,
    /// Changes with every mutable access (cache key of the published snapshot).
    #[serde(skip)]
    revision: u64,
}

impl PartialEq for ServiceState {
    fn eq(&self, other: &Self) -> bool {
        self.buildings == other.buildings && self.facilities == other.facilities
    }
}

impl ServiceState {
    /// A state from its saved parts (the migrations of `cp_save`).
    pub fn from_parts(
        buildings: Vec<BuildingServices>,
        facilities: BTreeMap<BuildingId, Facility>,
    ) -> Self {
        Self {
            buildings,
            facilities,
            revision: 0,
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The state of `id`; `None` when it has none (never touched, or another generation).
    pub fn get(&self, id: BuildingId) -> Option<&BuildingServices> {
        self.buildings
            .get(id.index as usize)
            .filter(|b| b.generation == id.generation)
    }

    /// The problem bits of `id`; empty without a record.
    pub fn problems_of(&self, id: BuildingId) -> ServiceProblems {
        self.get(id)
            .map_or_else(ServiceProblems::default, |b| b.problems)
    }

    /// The state of `id` for writing; a slot of an older generation starts over.
    pub fn get_mut(&mut self, id: BuildingId) -> &mut BuildingServices {
        self.revision += 1;
        let i = id.index as usize;
        if self.buildings.len() <= i {
            self.buildings.resize(i + 1, BuildingServices::default());
        }
        let slot = &mut self.buildings[i];
        if slot.generation != id.generation {
            *slot = BuildingServices {
                generation: id.generation,
                ..BuildingServices::default()
            };
        }
        slot
    }

    pub fn facility(&self, id: BuildingId) -> Option<&Facility> {
        self.facilities.get(&id)
    }

    /// The facility record of `id`, created empty on first use.
    pub fn facility_mut(&mut self, id: BuildingId) -> &mut Facility {
        self.revision += 1;
        self.facilities.entry(id).or_default()
    }

    /// Facilities in id order.
    pub fn facilities(&self) -> impl Iterator<Item = (BuildingId, &Facility)> {
        self.facilities.iter().map(|(id, f)| (*id, f))
    }

    /// All records with their building ids (generation included), in index order.
    pub fn records(&self) -> impl Iterator<Item = (BuildingId, &BuildingServices)> {
        self.buildings
            .iter()
            .enumerate()
            .map(|(i, b)| (BuildingId::new(i as u32, b.generation), b))
    }

    /// Forgets everything about a demolished building.
    pub fn remove(&mut self, id: BuildingId) {
        self.revision += 1;
        if let Some(slot) = self.buildings.get_mut(id.index as usize)
            && slot.generation == id.generation
        {
            *slot = BuildingServices::default();
        }
        self.facilities.remove(&id);
    }

    /// Zeroes every facility's daily throughput (the start of a game day).
    pub fn reset_day(&mut self) {
        self.revision += 1;
        for f in self.facilities.values_mut() {
            f.used_today = 0;
        }
    }

    /// Counts of buildings with each problem bit: garbage, sick, dead, fire, crime, mail.
    pub fn summary(&self) -> ServiceSummary {
        let mut s = ServiceSummary::default();
        for b in &self.buildings {
            for (n, bit) in s.counts.iter_mut().zip([
                ServiceProblems::GARBAGE,
                ServiceProblems::SICK,
                ServiceProblems::DEAD,
                ServiceProblems::FIRE,
                ServiceProblems::CRIME,
                ServiceProblems::MAIL,
            ]) {
                *n += u32::from(b.problems.has(bit));
            }
        }
        s
    }
}

/// How many buildings suffer each problem, in the bit order of [`ServiceProblems`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ServiceSummary {
    pub counts: [u32; 6],
}
