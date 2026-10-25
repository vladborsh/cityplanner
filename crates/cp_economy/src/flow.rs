//! Where the money went, hour by hour (BUG-040): the lines the hourly settlement booked, kept
//! for the last two game days and the week so far. Derived from the settlement, not saved and
//! not in the checksum; empty after a load until the next hours are booked.

use std::collections::VecDeque;

use bevy_ecs::resource::Resource;
use cp_content::ServiceKind;
use cp_core::Cost;

use crate::state::{EconomyState, WeekReport};

/// Hours of history kept (two game days).
pub const FLOW_HOURS: usize = 48;
/// Service kinds with an upkeep line (the `ServiceKind` discriminants `0..SERVICE_KINDS`).
pub const SERVICE_KINDS: usize = 11;
/// The service kinds in line order.
pub const SERVICE_ORDER: [ServiceKind; SERVICE_KINDS] = [
    ServiceKind::Water,
    ServiceKind::Power,
    ServiceKind::Roads,
    ServiceKind::Sewage,
    ServiceKind::Garbage,
    ServiceKind::Health,
    ServiceKind::Deathcare,
    ServiceKind::Fire,
    ServiceKind::Police,
    ServiceKind::Education,
    ServiceKind::Transit,
];

/// What was booked over a span: income by source and expenses by item (positive numbers).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flow {
    /// Tax per zone type, in [`crate::TAXED_ZONES`] order.
    pub income_by_zone: [Cost; 6],
    pub fares: Cost,
    /// Tourists' visits (T-168), booked once a week like the fares.
    pub tourism: Cost,
    /// Hotel nights (T-321), booked once a week like the fares.
    pub hotels: Cost,
    /// Goods sent out of the map (T-170), booked once a week like the fares.
    pub exports: Cost,
    /// Cargo sold to and bought from the outside (T-210), booked once a week.
    pub industry_export: Cost,
    pub industry_import: Cost,
    /// Entrance fees of the park areas (T-225), booked once a week.
    pub park_fees: Cost,
    /// What bulldozing gave back (BUG-097): paid into the treasury by the command, booked here
    /// by the hour it fell in.
    pub refunds: Cost,
    pub road_upkeep: Cost,
    /// Pipes and power lines.
    pub utility_upkeep: Cost,
    /// Upkeep of the service buildings by kind, in [`SERVICE_ORDER`] order.
    pub services: [Cost; SERVICE_KINDS],
    /// Upkeep of the buses.
    pub transit_upkeep: Cost,
    pub loans: Cost,
    /// The policies' cost (T-190), booked by the hour like the upkeep.
    pub policies: Cost,
}

impl Flow {
    pub fn tax(&self) -> Cost {
        self.income_by_zone.iter().copied().sum()
    }

    pub fn income(&self) -> Cost {
        self.tax()
            + self.fares
            + self.tourism
            + self.hotels
            + self.exports
            + self.industry_export
            + self.park_fees
            + self.refunds
    }

    pub fn building_upkeep(&self) -> Cost {
        self.services.iter().copied().sum()
    }

    pub fn expenses(&self) -> Cost {
        self.road_upkeep
            + self.utility_upkeep
            + self.building_upkeep()
            + self.transit_upkeep
            + self.loans
            + self.policies
            + self.industry_import
    }

    pub fn net(&self) -> Cost {
        self.income() - self.expenses()
    }

    pub fn add(&mut self, o: &Flow) {
        for (a, b) in self.income_by_zone.iter_mut().zip(o.income_by_zone) {
            *a += b;
        }
        for (a, b) in self.services.iter_mut().zip(o.services) {
            *a += b;
        }
        self.fares += o.fares;
        self.tourism += o.tourism;
        self.hotels += o.hotels;
        self.exports += o.exports;
        self.industry_export += o.industry_export;
        self.industry_import += o.industry_import;
        self.park_fees += o.park_fees;
        self.refunds += o.refunds;
        self.road_upkeep += o.road_upkeep;
        self.utility_upkeep += o.utility_upkeep;
        self.transit_upkeep += o.transit_upkeep;
        self.loans += o.loans;
        self.policies += o.policies;
    }

    /// `self × n / d` per line, rounded down (for scaling a part of a day up to a day).
    fn scaled(&self, n: i64, d: i64) -> Flow {
        let f = |c: Cost| Cost((i128::from(c.0) * i128::from(n)).div_euclid(i128::from(d)) as i64);
        Flow {
            income_by_zone: self.income_by_zone.map(f),
            fares: f(self.fares),
            tourism: f(self.tourism),
            hotels: f(self.hotels),
            exports: f(self.exports),
            industry_export: f(self.industry_export),
            industry_import: f(self.industry_import),
            park_fees: f(self.park_fees),
            refunds: f(self.refunds),
            road_upkeep: f(self.road_upkeep),
            utility_upkeep: f(self.utility_upkeep),
            services: self.services.map(f),
            transit_upkeep: f(self.transit_upkeep),
            loans: f(self.loans),
            policies: f(self.policies),
        }
    }
}

/// The demolition refunds paid since the last hour was booked (BUG-097). The commands pay them
/// into the treasury at once; the hourly settlement books them as that hour's
/// [`Flow::refunds`] and empties this. Like [`MoneyFlow`] not saved and not in the checksum: it
/// only feeds the budget's lines.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DemolitionRefunds(pub Cost);

/// The hourly bookings of the last [`FLOW_HOURS`] hours and the week so far.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct MoneyFlow {
    /// `(game hour, what was booked)`, oldest first.
    pub hours: VecDeque<(u64, Flow)>,
    /// Everything booked since the week began.
    pub week_to_date: Flow,
}

impl MoneyFlow {
    /// The money flows of one day: the tax and upkeep lines of the last 24 hours booked (a part
    /// of a day is scaled up to the day), and the lines that are booked once a week (fares, bus
    /// upkeep, loan payments) as a seventh of the last week's and the loans' figures. `None`
    /// before the first hour was booked.
    pub fn last_day(&self, state: &EconomyState) -> Option<Flow> {
        if self.hours.is_empty() {
            return None;
        }
        let n = self.hours.len().min(24);
        let mut sum = Flow::default();
        for (_, f) in self.hours.iter().rev().take(n) {
            sum.add(f);
        }
        let mut day = sum.scaled(24, n as i64);
        // A refund is paid once, not by the hour: as it came (BUG-097).
        day.refunds = sum.refunds;
        let w: &WeekReport = &state.last_week;
        day.fares = Cost(w.transit_fares.0 / 7);
        day.tourism = Cost(w.tourism.0 / 7);
        day.hotels = Cost(w.hotels.0 / 7);
        day.exports = Cost(w.exports.0 / 7);
        day.transit_upkeep = Cost(w.transit_upkeep.0 / 7);
        day.loans = Cost(state.loans.iter().map(|l| l.weekly().0).sum::<i64>() / 7);
        Some(day)
    }

    /// The balance of each of the last hours, oldest first (what the hour added to the treasury).
    pub fn hourly_net(&self) -> Vec<i64> {
        self.hours.iter().map(|(_, f)| f.net().0).collect()
    }
}
