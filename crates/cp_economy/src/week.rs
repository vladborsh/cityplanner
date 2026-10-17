//! The weekly settlement (`architecture/system-design.md` §11): tax income minus upkeep,
//! booked to the treasury once per game week on the sim tick.
//!
//! [`week_report`] is a pure function of the sim state, so balance tools and tests call the
//! same code the sim does. Every line is an integer number of cents, rounded once per line
//! ([`Cost::scaled`]); lines are summed in id order.
//!
//! * **Tax**, per `Active` building, `rate = pct / 100`: residential
//!   `rate × households_living × tax_household[level]`; workplaces
//!   `rate × Σ filled_jobs[edu] × tax_job[edu] × level_mul[level]`; plus the optional
//!   `tax_base × rate × occupancy` surcharge (0 unless a definition sets it). The level is the
//!   definition's (`apt_high_3` is level 3). Tables are in `balance.ron` (T-128).
//! * **Roads**, per segment: `length × upkeep_per_m_week × budget(Roads) / 100`; the upkeep is
//!   that of the elevated or bridge variant for such segments.
//! * **Service buildings**, per `Active` building with a service: `upkeep_week × budget(kind)
//!   / 100`, the kind being its service's.
//! * **Loans** (T-129), per active loan: an equal share of the principal and interest left,
//!   in whole cents; the last week takes the remainder. Paid even into debt.
//! * **Utility links**, per link: `length × upkeep_per_m_week × budget / 100`; a pipe uses the
//!   water definition and budget, a power line the power ones.

use std::collections::BTreeMap;

use bevy_ecs::system::{Res, ResMut};
use cp_building::{BuildingGrid, BuildingState};
use cp_citizen::CitizenGrid;
use cp_content::{ContentDb, RoadDef, ServiceKind, SimContent, UtilityKind};
use cp_core::time::TICKS_PER_DAY;
use cp_core::{Cost, Tick, Treasury};
use cp_district::{DistrictId, DistrictMap, Districts, district_at};
use cp_net::{Elevation, NetGraph, Segment};
use cp_transit::TransitLedger;
use cp_utility::{UtilityGraph, UtilityNet};

use crate::flow::{FLOW_HOURS, Flow, MoneyFlow, SERVICE_KINDS};
use crate::state::{DistrictIncome, EconomyState, Loan, WeekReport};

fn percent(pct: u8) -> f64 {
    f64::from(pct) / 100.0
}

/// Weekly upkeep of one road segment at `budget_pct`.
pub fn segment_upkeep(def: &RoadDef, seg: &Segment, budget_pct: u8) -> Cost {
    let per_m = match seg.elevation {
        Elevation::Elevated => def.elevated.as_ref().map(|v| v.upkeep_per_m_week),
        Elevation::Bridge => def.bridge.as_ref().map(|v| v.upkeep_per_m_week),
        Elevation::Tunnel => def.tunnel.as_ref().map(|v| v.upkeep_per_m_week),
        Elevation::Ground => None,
    }
    .unwrap_or(def.upkeep_per_m_week);
    Cost::scaled(per_m, f64::from(seg.length) * percent(budget_pct))
}

/// Everything a settlement reads.
pub struct WeekInputs<'a> {
    pub content: &'a ContentDb,
    pub net: &'a NetGraph,
    pub buildings: &'a BuildingGrid,
    /// Its occupancy tables must be ready (`CitizenGrid::ensure_derived`).
    pub citizens: &'a CitizenGrid,
    pub utilities: &'a UtilityGraph,
    pub state: &'a EconomyState,
    /// Where districts are and what tax rates they override (T-132).
    pub district_map: &'a DistrictMap,
    pub districts: &'a Districts,
}

/// What week `week` books. See the module docs for each line.
pub fn week_report(i: &WeekInputs, week: u64) -> WeekReport {
    week_detail(i, week).0
}

/// [`week_report`] and the tax income by district: `None` (buildings outside every district)
/// first, then districts in id order; a row exists for each district that has a taxable
/// building. The rows sum to the report's income.
pub fn week_detail(i: &WeekInputs, week: u64) -> (WeekReport, Vec<(Option<DistrictId>, Cost)>) {
    let (r, rows, _) = week_lines(i, week);
    (r, rows)
}

/// The tax income by district.
pub type DistrictRows = Vec<(Option<DistrictId>, Cost)>;

/// [`week_detail`] and the service upkeep by kind ([`SERVICE_ORDER`]; sums to
/// `building_upkeep`).
pub fn week_lines(i: &WeekInputs, week: u64) -> (WeekReport, DistrictRows, [Cost; SERVICE_KINDS]) {
    let mut services = [Cost::ZERO; SERVICE_KINDS];
    let mut by_district: BTreeMap<Option<DistrictId>, Cost> = BTreeMap::new();
    let mut r = WeekReport {
        week,
        ..WeekReport::default()
    };
    let st = i.state;

    for (id, b) in i.buildings.iter() {
        if b.state != BuildingState::Active {
            continue;
        }
        let def = i.content.building(b.def);
        if let Some(zone_slot) = (def.zone as usize).checked_sub(1)
            && let Some(city_rate) = st.taxes.get(def.zone)
        {
            let district = district_at(i.district_map, b.pos);
            let rate = district
                .and_then(|d| i.districts.tax_override(d, def.zone))
                .unwrap_or(city_rate);
            let bal = i.content.balance();
            let rate_f = f64::from(rate) / 100.0;
            let lvl = usize::from(def.level.max(1)) - 1;
            let homes = f64::from(i.citizens.households_in(id).min(def.households));
            let mut tax = Cost::scaled(bal.tax_household[lvl.min(4)], rate_f * homes);
            let mul = f64::from(bal.level_mul[lvl.min(2)]);
            for (edu, &per_job) in bal.tax_job.iter().enumerate() {
                let filled = f64::from(i.citizens.jobs_filled(id, edu).min(def.jobs[edu]));
                tax += Cost::scaled(per_job, rate_f * filled * mul);
            }
            let cap =
                u32::from(def.households) + def.jobs.iter().map(|&j| u32::from(j)).sum::<u32>();
            if cap > 0 && def.tax_base > 0.0 {
                let filled = u32::from(i.citizens.households_in(id))
                    + (0..4)
                        .map(|l| u32::from(i.citizens.jobs_filled(id, l)))
                        .sum::<u32>();
                let filled = filled.min(cap);
                tax += Cost::scaled(def.tax_base, rate_f * f64::from(filled) / f64::from(cap));
            }
            r.income_by_zone[zone_slot] += tax;
            *by_district.entry(district).or_insert(Cost::ZERO) += tax;
        }
        if let Some(service) = def.service.as_deref().and_then(|s| i.content.service_id(s)) {
            let kind = i.content.service(service).kind;
            let upkeep = Cost::scaled(def.upkeep_week, percent(st.budgets.get(kind)));
            r.building_upkeep += upkeep;
            if let Some(slot) = services.get_mut(kind as usize) {
                *slot += upkeep;
            }
        }
    }

    let roads = st.budgets.get(ServiceKind::Roads);
    for (_, seg) in i.net.segments() {
        r.road_upkeep += segment_upkeep(i.content.road(seg.road), seg, roads);
    }

    for (_, link) in i.utilities.links() {
        let (kind, service) = match link.net {
            UtilityNet::Pipe => (UtilityKind::Water, ServiceKind::Water),
            UtilityNet::Power => (UtilityKind::Power, ServiceKind::Power),
        };
        if let Some((_, def)) = i.content.utility_for_kind(kind) {
            r.utility_upkeep += Cost::scaled(
                def.upkeep_per_m_week,
                f64::from(link.length) * percent(st.budgets.get(service)),
            );
        }
    }
    r.loan_payments = st.loans.iter().map(Loan::weekly).sum();
    (r, by_district.into_iter().collect(), services)
}

/// Game hours in a week.
pub const HOURS_PER_WEEK: u64 = 168;

/// The `k`-th of `n` shares (`k` in 1..=n) of an amount: the differences of `floor(x·k/n)`, so
/// the `n` shares add up to `x` exactly and no cent is lost.
pub fn share(x: Cost, k: u64, n: u64) -> Cost {
    let at = |k: u64| (i128::from(x.0) * i128::from(k)).div_euclid(i128::from(n)) as i64;
    Cost(at(k) - at(k - 1))
}

/// The `k`-th of seven daily shares (T-179).
pub fn day_share(x: Cost, k: u64) -> Cost {
    share(x, k, 7)
}

/// The `k`-th of the 168 hourly shares of a week (BUG-040).
pub fn hour_share(x: Cost, k: u64) -> Cost {
    share(x, k, HOURS_PER_WEEK)
}

/// The game hour `tick` is in (`tick × 24 / TICKS_PER_DAY`).
pub fn game_hour(tick: u64) -> u64 {
    tick * 24 / TICKS_PER_DAY
}

/// Economy phase (BUG-040): in the first tick of every game hour after tick 0 books that hour's
/// share (1/168, see [`hour_share`]) of the weekly tax income and of the road, building and
/// utility upkeep, by the same formula as [`week_report`], into the treasury (which may go
/// below the debt floor; only purchases are refused there). The hour that ends the week (the
/// 168th) also books the week's transit money and the loan payments, keeps the week's
/// [`WeekReport`] in [`EconomyState::last_week`] and the district rows. What every hour booked
/// goes to [`MoneyFlow`].
#[allow(clippy::too_many_arguments)]
pub fn settle_week(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    net: Res<NetGraph>,
    buildings: Res<BuildingGrid>,
    mut citizens: ResMut<CitizenGrid>,
    utilities: Res<UtilityGraph>,
    mut state: ResMut<EconomyState>,
    mut treasury: ResMut<Treasury>,
    mut district_income: ResMut<DistrictIncome>,
    mut flow: ResMut<MoneyFlow>,
    district_map: Res<DistrictMap>,
    districts: Res<Districts>,
    ledger: Option<ResMut<TransitLedger>>,
) {
    let hour = game_hour(tick.0);
    if tick.0 == 0 || hour == game_hour(tick.0 - 1) {
        return;
    }
    let k = match hour % HOURS_PER_WEEK {
        0 => HOURS_PER_WEEK,
        h => h,
    };
    let Some(content) = content else { return };
    citizens.ensure_derived();
    let (mut report, rows, services) = week_lines(
        &WeekInputs {
            content: &content.0,
            net: &net,
            buildings: &buildings,
            citizens: &citizens,
            utilities: &utilities,
            state: &state,
            district_map: &district_map,
            districts: &districts,
        },
        hour.div_ceil(HOURS_PER_WEEK),
    );
    let mut booked = Flow {
        income_by_zone: report.income_by_zone.map(|c| hour_share(c, k)),
        road_upkeep: hour_share(report.road_upkeep, k),
        utility_upkeep: hour_share(report.utility_upkeep, k),
        services: services.map(|c| hour_share(c, k)),
        ..Flow::default()
    };
    treasury.balance += booked.net();
    if k == 1 {
        flow.week_to_date = Flow::default();
    }
    if k == HOURS_PER_WEEK {
        *district_income = DistrictIncome {
            week: hour / HOURS_PER_WEEK,
            rows,
        };
        // The week's transit money, summed by `cp_transit`, is booked and starts over (T-160).
        if let Some(mut ledger) = ledger {
            report.transit_fares = ledger.fares;
            report.transit_upkeep = ledger.upkeep;
            report.tourism = ledger.tourism;
            report.exports = ledger.exports;
            *ledger = TransitLedger::default();
        }
        treasury.balance += report.transit_fares + report.tourism + report.exports
            - report.transit_upkeep
            - report.loan_payments;
        booked.fares = report.transit_fares;
        booked.tourism = report.tourism;
        booked.exports = report.exports;
        booked.transit_upkeep = report.transit_upkeep;
        booked.loans = report.loan_payments;
        for loan in &mut state.loans {
            loan.pay_week();
        }
        state.loans.retain(|l| l.weeks_left > 0);
        state.last_week = report;
    }
    flow.week_to_date.add(&booked);
    if flow.hours.len() == FLOW_HOURS {
        flow.hours.pop_front();
    }
    flow.hours.push_back((hour, booked));
}
