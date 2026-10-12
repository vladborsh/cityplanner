//! The weekly settlement (`architecture/system-design.md` §11): tax income minus upkeep,
//! booked to the treasury once per game week on the sim tick.
//!
//! [`week_report`] is a pure function of the sim state, so balance tools and tests call the
//! same code the sim does. Every line is an integer number of cents, rounded once per line
//! ([`Cost::scaled`]); lines are summed in id order.
//!
//! * **Tax**, per `Active` building: `tax_base × rate × occupancy`, `rate = pct / 100`,
//!   `occupancy = (households + filled jobs) / (household capacity + job slots)`, at most 1.
//!   The *level factor* of the formula is 1: a building's level is its definition (`apt_high_3`
//!   is level 3), and `tax_base` is already per definition.
//! * **Roads**, per segment: `length × upkeep_per_m_week × budget(Roads) / 100`; the upkeep is
//!   that of the elevated or bridge variant for such segments.
//! * **Service buildings**, per `Active` building with a service: `upkeep_week × budget(kind)
//!   / 100`, the kind being its service's.
//! * **Utility links**, per link: `length × upkeep_per_m_week × budget / 100`; a pipe uses the
//!   water definition and budget, a power line the power ones.

use bevy_ecs::system::{Res, ResMut};
use cp_building::{BuildingGrid, BuildingState};
use cp_citizen::CitizenGrid;
use cp_content::{ContentDb, RoadDef, ServiceKind, SimContent, UtilityKind};
use cp_core::time::TICKS_PER_WEEK;
use cp_core::{Cost, Tick, Treasury};
use cp_net::{Elevation, NetGraph, Segment};
use cp_utility::{UtilityGraph, UtilityNet};

use crate::state::{EconomyState, WeekReport};

fn percent(pct: u8) -> f64 {
    f64::from(pct) / 100.0
}

/// Weekly upkeep of one road segment at `budget_pct`.
pub fn segment_upkeep(def: &RoadDef, seg: &Segment, budget_pct: u8) -> Cost {
    let per_m = match seg.elevation {
        Elevation::Elevated => def.elevated.as_ref().map(|v| v.upkeep_per_m_week),
        Elevation::Bridge => def.bridge.as_ref().map(|v| v.upkeep_per_m_week),
        Elevation::Ground | Elevation::Tunnel => None,
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
}

/// What week `week` books. See the module docs for each line.
pub fn week_report(i: &WeekInputs, week: u64) -> WeekReport {
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
            && let Some(rate) = st.taxes.get(def.zone)
        {
            let cap =
                u32::from(def.households) + def.jobs.iter().map(|&j| u32::from(j)).sum::<u32>();
            if cap > 0 {
                let filled = u32::from(i.citizens.households_in(id))
                    + (0..4)
                        .map(|l| u32::from(i.citizens.jobs_filled(id, l)))
                        .sum::<u32>();
                let filled = filled.min(cap);
                let factor = f64::from(rate) * f64::from(filled) / (100.0 * f64::from(cap));
                r.income_by_zone[zone_slot] += Cost::scaled(def.tax_base, factor);
            }
        }
        if let Some(service) = def.service.as_deref().and_then(|s| i.content.service_id(s)) {
            let kind = i.content.service(service).kind;
            r.building_upkeep += Cost::scaled(def.upkeep_week, percent(st.budgets.get(kind)));
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
    r
}

/// Economy phase: on every week boundary after tick 0, books [`week_report`] into the
/// treasury (which may go below the debt floor; only purchases are refused there) and keeps
/// the report in [`EconomyState::last_week`].
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
) {
    if tick.0 == 0 || !tick.0.is_multiple_of(TICKS_PER_WEEK) {
        return;
    }
    let Some(content) = content else { return };
    citizens.ensure_derived();
    let report = week_report(
        &WeekInputs {
            content: &content.0,
            net: &net,
            buildings: &buildings,
            citizens: &citizens,
            utilities: &utilities,
            state: &state,
        },
        tick.0 / TICKS_PER_WEEK,
    );
    treasury.balance += report.balance();
    state.last_week = report;
}
