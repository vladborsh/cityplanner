//! Citizens and households (`architecture/system-design.md` §9.1).
//!
//! Save stability: serialised with postcard; changing fields is a save format change and
//! enums are append-only. Both types are plain fixed-size data (no heap) so the arenas
//! scale to the §3 capacities.

use cp_building::BuildingId;
use cp_core::{Id, Tick};
use serde::{Deserialize, Serialize};

pub type CitizenId = Id<Citizen>;
pub type HouseholdId = Id<Household>;

/// Game days per citizen year. PLACEHOLDER until aging lands (M2): age is only used to
/// tell adults from children in M1.
pub const DAYS_PER_YEAR: u32 = 28;
/// Age (days) from which a citizen is an adult and can work.
pub const ADULT_AGE_DAYS: u32 = 18 * DAYS_PER_YEAR;
/// People per household, at most (§9.1).
pub const HOUSEHOLD_MAX: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Location {
    Home,
    Work,
    Shop,
    /// Travelling: a household that has "arrived" in the city but not yet at its home.
    Moving,
    Outside,
    /// At a park, a city park or a unique building (T-224). Appended in save format 48.
    Leisure,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Citizen {
    /// Age in game days.
    pub age_days: u32,
    /// 0..=3.
    pub education: u8,
    pub household: HouseholdId,
    pub home: Option<BuildingId>,
    pub work: Option<BuildingId>,
    /// Education level of the job slot held (`<= education`); meaningful when `work` is set.
    pub work_level: u8,
    pub location: Location,
    pub wellbeing: u8,
    pub has_car: bool,
    /// Since when the citizen is sick (T-135); `None` = healthy. The health row fills it.
    pub sick_since: Option<Tick>,
    /// The school the citizen goes to (T-135); `None` = none. The education row fills it.
    pub school: Option<BuildingId>,
}

impl Citizen {
    pub fn is_adult(&self) -> bool {
        self.age_days >= ADULT_AGE_DAYS
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Household {
    pub members: [Option<CitizenId>; HOUSEHOLD_MAX],
    pub home: BuildingId,
    /// Cents.
    pub wealth: i64,
    /// Tick at which the household reaches its home (until then its members are `Moving`).
    pub arrive_at: Tick,
    /// Since when no adult of the household has had a job.
    pub jobless_since: Option<Tick>,
}

impl Household {
    pub fn member_ids(&self) -> impl Iterator<Item = CitizenId> + '_ {
        self.members.iter().flatten().copied()
    }
}
