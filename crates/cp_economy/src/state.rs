//! Tax rates, budget percentages and the last settled week: the serialized sim state of the
//! economy besides the [`Treasury`](cp_core::Treasury) (`architecture/system-design.md` §11).

use bevy_ecs::resource::Resource;
use cp_content::{ServiceKind, ZoneType};
use cp_core::Cost;
use serde::{Deserialize, Serialize};

/// Tax rate limits and default, percent.
pub const TAX_MIN: u8 = 1;
pub const TAX_MAX: u8 = 29;
pub const TAX_DEFAULT: u8 = 9;
/// Budget limits and default, percent of the nominal upkeep.
pub const BUDGET_MIN: u8 = 50;
pub const BUDGET_MAX: u8 = 150;
pub const BUDGET_DEFAULT: u8 = 100;

/// Zone types that pay tax (everything but `ZoneType::None`), in tax-table order.
pub const TAXED_ZONES: [ZoneType; 6] = [
    ZoneType::ResLow,
    ZoneType::ResHigh,
    ZoneType::ComLow,
    ZoneType::ComHigh,
    ZoneType::Industrial,
    ZoneType::Office,
];

/// Budgeted service kinds, in budget-table order (the `ServiceKind` discriminants).
pub const BUDGETED: [ServiceKind; 4] = [
    ServiceKind::Water,
    ServiceKind::Power,
    ServiceKind::Roads,
    ServiceKind::Sewage,
];

/// Tax rate per zone type, percent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaxRates {
    pct: [u8; 6],
}

impl Default for TaxRates {
    fn default() -> Self {
        Self {
            pct: [TAX_DEFAULT; 6],
        }
    }
}

impl TaxRates {
    fn slot(zone: ZoneType) -> Option<usize> {
        (zone != ZoneType::None).then(|| zone as usize - 1)
    }

    /// The rate of `zone`; `None` for `ZoneType::None`, which pays nothing.
    pub fn get(&self, zone: ZoneType) -> Option<u8> {
        Self::slot(zone).map(|i| self.pct[i])
    }

    /// Sets the rate; false (nothing changed) for `ZoneType::None`. The range is the
    /// caller's to check (the command does).
    pub fn set(&mut self, zone: ZoneType, pct: u8) -> bool {
        Self::slot(zone).map(|i| self.pct[i] = pct).is_some()
    }
}

/// Budget percentage per service kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budgets {
    pct: [u8; 4],
}

impl Default for Budgets {
    fn default() -> Self {
        Self {
            pct: [BUDGET_DEFAULT; 4],
        }
    }
}

impl Budgets {
    pub fn get(&self, kind: ServiceKind) -> u8 {
        self.pct[kind as usize]
    }

    pub fn set(&mut self, kind: ServiceKind, pct: u8) {
        self.pct[kind as usize] = pct;
    }
}

/// What the last weekly settlement booked, by line. All cents.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeekReport {
    /// Number of the settled week (1 = the first week boundary after tick 0); 0 = none yet.
    pub week: u64,
    /// Tax income per zone type, in [`TAXED_ZONES`] order.
    pub income_by_zone: [Cost; 6],
    pub road_upkeep: Cost,
    pub building_upkeep: Cost,
    pub utility_upkeep: Cost,
}

impl WeekReport {
    pub fn income(&self) -> Cost {
        self.income_by_zone.iter().copied().sum()
    }

    pub fn upkeep(&self) -> Cost {
        self.road_upkeep + self.building_upkeep + self.utility_upkeep
    }

    /// Income minus upkeep: the change of the treasury that week.
    pub fn balance(&self) -> Cost {
        self.income() - self.upkeep()
    }
}

/// The economy's settings and last report. Serialized sim state.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EconomyState {
    pub taxes: TaxRates,
    pub budgets: Budgets,
    pub last_week: WeekReport,
}
