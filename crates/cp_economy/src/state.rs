//! Tax rates, budget percentages and the last settled week: the serialized sim state of the
//! economy besides the [`Treasury`](cp_core::Treasury) (`architecture/system-design.md` §11).

use bevy_ecs::resource::Resource;
use cp_content::LoanOffer;
use cp_content::{ServiceKind, ZoneType};
use cp_core::{Cost, Tick};
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
    /// The percentage of `kind`; kinds without a budget line (the M2 services before their
    /// budget rows) run at the default.
    pub fn get(&self, kind: ServiceKind) -> u8 {
        self.pct
            .get(kind as usize)
            .copied()
            .unwrap_or(BUDGET_DEFAULT)
    }

    /// Sets the percentage of `kind`; a kind without a budget line ignores it.
    pub fn set(&mut self, kind: ServiceKind, pct: u8) {
        if let Some(p) = self.pct.get_mut(kind as usize) {
            *p = pct;
        }
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
    /// Weekly loan payments, principal and interest (T-129).
    #[serde(default)]
    pub loan_payments: Cost,
    /// Fares taken and the upkeep of the buses, from the transit ledger (T-160). Added in v32.
    #[serde(default)]
    pub transit_fares: Cost,
    #[serde(default)]
    pub transit_upkeep: Cost,
    /// What the visits of tourists brought, from the transit ledger (T-168). Added in v33.
    #[serde(default)]
    pub tourism: Cost,
    /// What the goods trucks that left the map brought, from the transit ledger (T-170). Added
    /// in v37.
    #[serde(default)]
    pub exports: Cost,
    /// What the policies in force cost, per building under them (T-190). Added in v38.
    #[serde(default)]
    pub policies: Cost,
}

impl WeekReport {
    pub fn income(&self) -> Cost {
        self.income_by_zone.iter().copied().sum()
    }

    pub fn upkeep(&self) -> Cost {
        self.road_upkeep + self.building_upkeep + self.utility_upkeep
    }

    /// Income minus upkeep and loan payments: the change of the treasury that week.
    pub fn balance(&self) -> Cost {
        self.income() + self.transit_fares + self.tourism + self.exports
            - self.upkeep()
            - self.transit_upkeep
            - self.loan_payments
            - self.policies
    }
}

/// An active loan, one per offer at most. Serialized sim state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Loan {
    /// Index into `Balance::loan_offers`.
    pub offer: u8,
    pub principal_left: Cost,
    /// Interest still to be paid over the remaining weeks (simple, fixed at the start).
    pub interest_left: Cost,
    pub weeks_left: u16,
    pub taken_at: Tick,
}

impl Loan {
    /// A loan just taken from `o`: the interest for the whole term is
    /// `amount × interest_pct / 100`, rounded once.
    pub fn new(offer: u8, o: &LoanOffer, taken_at: Tick) -> Self {
        Self {
            offer,
            principal_left: Cost::from_units(o.amount),
            interest_left: Cost::scaled(o.amount, f64::from(o.interest_pct) / 100.0),
            weeks_left: o.weeks,
            taken_at,
        }
    }

    /// This week's `(principal, interest)` in whole cents: an equal share of what is left; the
    /// last week takes the remainder.
    pub fn payment(&self) -> (Cost, Cost) {
        let w = i64::from(self.weeks_left.max(1));
        if w == 1 {
            (self.principal_left, self.interest_left)
        } else {
            (
                Cost(self.principal_left.0 / w),
                Cost(self.interest_left.0 / w),
            )
        }
    }

    /// This week's total payment.
    pub fn weekly(&self) -> Cost {
        let (p, i) = self.payment();
        p + i
    }

    /// Books one weekly payment; true when that was the last.
    pub fn pay_week(&mut self) -> bool {
        let (p, i) = self.payment();
        self.principal_left = self.principal_left - p;
        self.interest_left = self.interest_left - i;
        self.weeks_left = self.weeks_left.saturating_sub(1);
        self.weeks_left == 0
    }
}

/// The economy's settings, loans and last report. Serialized sim state.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EconomyState {
    pub taxes: TaxRates,
    pub budgets: Budgets,
    pub last_week: WeekReport,
    /// Active loans, ordered by offer index (T-129).
    pub loans: Vec<Loan>,
}

impl EconomyState {
    pub fn loan(&self, offer: u8) -> Option<&Loan> {
        self.loans.iter().find(|l| l.offer == offer)
    }
}

/// Tax income of the last settled week by district (T-132): `None` is the buildings outside
/// every district, then districts in id order. Derived from the same pass as
/// [`WeekReport`], so the rows sum to [`WeekReport::income`]; not saved and not in the
/// checksum (the save format is unchanged), so it is empty after a load until the next week
/// boundary.
#[derive(bevy_ecs::prelude::Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct DistrictIncome {
    /// Number of the settled week, 0 = none yet.
    pub week: u64,
    pub rows: Vec<(Option<cp_district::DistrictId>, Cost)>,
}
