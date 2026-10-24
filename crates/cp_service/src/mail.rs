//! Mail (T-247, M7 Post): buildings make mail, post offices send mail vans to collect it, and
//! what is collected must be sorted — a little by the post offices themselves, the rest by the
//! sorting facilities (a city-wide pool). A building that holds too much mail has the MAIL
//! problem.
//!
//! Same shape as garbage: every tick the buildings of bucket `index % 64 == tick % 64` get the
//! share of a game day that passed. With `cp_traffic`'s [`ServiceRuns`] present the collection is
//! a trip: a covered building holding `mail_van_min` gets a mail van from the nearest post office
//! with room and a van free, and the mail is taken when the van arrives (or at once when the run
//! failed). Without `ServiceRuns` every pass collects at once.
//!
//! A post office (`ServiceDef.tier == 1`) collects within its daily `capacity` and has
//! `storage` vans; a sorting facility (`tier == 2`) has no coverage and sorts `capacity` mail a
//! day. Collected mail waits in [`MailState::unsorted`]; once that is over
//! `mail_backlog_days` days of the city's sorting capacity the post offices stop collecting.

use std::collections::BTreeMap;

use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Res, ResMut};
use cp_building::{BuildingFlags, BuildingGrid, BuildingId, BuildingState};
use cp_citizen::CitizenGrid;
use cp_content::{Budgets, ServiceKind, ServiceUnlocks, SimContent, VehicleKind, ZoneType};
use cp_core::Tick;
use cp_core::time::TICKS_PER_DAY;
use cp_traffic::{RunPoint, RunState, ServiceRuns};
use serde::{Deserialize, Serialize};

use crate::coverage::{COVERAGE_PERIOD, ServiceCoverage};
use crate::facility::FacilityFinder;
use crate::state::{ServiceProblems, ServiceState};

/// Passes a building gets per game day (one every [`COVERAGE_PERIOD`] ticks).
const PASSES_PER_DAY: u64 = TICKS_PER_DAY / COVERAGE_PERIOD;

/// `ServiceDef.tier` of a post office.
pub const POST_OFFICE_TIER: u8 = 1;
/// `ServiceDef.tier` of a sorting facility.
pub const SORTING_TIER: u8 = 2;

/// The mail of one game day, city-wide.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailDay {
    /// Made by the buildings.
    pub made: u32,
    /// Taken from the buildings by the mail vans (and so delivered to them: the vans bring the
    /// incoming mail on the same trip).
    pub collected: u32,
    pub delivered: u32,
    /// Sorted by the post offices and sorting facilities.
    pub sorted: u32,
}

/// All mail state. Serialized sim state, part of the checksum. Added in save v53.
#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MailState {
    /// Mail waiting in a building to be collected; buildings without mail have no entry.
    piles: BTreeMap<BuildingId, u16>,
    /// Collected and not yet sorted: the city-wide backlog.
    pub unsorted: u32,
    /// Mail the working post offices and sorting facilities sort per game day, as of the last
    /// pass.
    pub sort_capacity: u32,
    /// The game day in progress and the one before it.
    pub today: MailDay,
    pub yesterday: MailDay,
}

impl MailState {
    /// Nothing waits anywhere, nothing was counted and nothing can be sorted.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The mail waiting in `id`.
    pub fn pile(&self, id: BuildingId) -> u16 {
        self.piles.get(&id).copied().unwrap_or(0)
    }

    /// Sets the mail waiting in `id` (0 clears it).
    pub fn set_pile(&mut self, id: BuildingId, mail: u16) {
        if mail == 0 {
            self.piles.remove(&id);
        } else {
            self.piles.insert(id, mail);
        }
    }

    /// Buildings with mail waiting, in id order.
    pub fn piles(&self) -> impl Iterator<Item = (BuildingId, u16)> + '_ {
        self.piles.iter().map(|(id, m)| (*id, *m))
    }

    /// The unsorted mail is over `days` days of the sorting capacity: the post offices stop
    /// collecting.
    pub fn backlog_full(&self, days: f32) -> bool {
        self.unsorted as f32 > days * self.sort_capacity as f32
    }

    /// Forgets a demolished building.
    pub fn remove(&mut self, id: BuildingId) {
        self.piles.remove(&id);
    }
}

/// The service definition of the building `id` at the Post budget (§20: capacity and vans),
/// when it is a service of `ServiceKind::Post`.
fn post_def(
    db: &cp_content::ContentDb,
    budgets: &Budgets,
    buildings: &BuildingGrid,
    id: BuildingId,
) -> Option<cp_content::ServiceDef> {
    buildings
        .get(id)
        .and_then(|f| db.building_service(f.def))
        .map(|s| db.service(s))
        .filter(|s| s.kind == ServiceKind::Post)
        .map(|s| budgets.def(s, db.balance()))
}

/// Services phase, before [`update_mail`]: once per pass (every [`COVERAGE_PERIOD`] ticks) starts
/// the game day over at its first tick, refreshes the city's sorting capacity and the vans each
/// post office has out, and sorts the pass's share of the day.
pub fn update_mail_sorting(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    buildings: Res<BuildingGrid>,
    runs: Option<Res<ServiceRuns>>,
    mut state: ResMut<ServiceState>,
    mut mail: ResMut<MailState>,
    budgets: Option<Res<Budgets>>,
) {
    if !tick.0.is_multiple_of(COVERAGE_PERIOD) {
        return;
    }
    let Some(content) = content else { return };
    let db = &content.0;
    let budgets = budgets.as_deref().copied().unwrap_or_default();
    if tick.0 != 0 && tick.0.is_multiple_of(TICKS_PER_DAY) {
        mail.yesterday = mail.today;
        mail.today = MailDay::default();
    }
    // Vans out per post office, counted from the runs so a van lost with its target is back.
    let mut out: BTreeMap<BuildingId, u32> = BTreeMap::new();
    if let Some(runs) = runs.as_deref() {
        for (_, r) in runs.iter() {
            if r.kind == VehicleKind::Mail as u8
                && let RunPoint::Building(from) = r.from
            {
                *out.entry(from).or_default() += 1;
            }
        }
    }
    let mut capacity = 0u32;
    for (id, b) in buildings.iter() {
        if b.state != BuildingState::Active {
            continue;
        }
        let Some(svc) = post_def(db, &budgets, &buildings, id) else {
            continue;
        };
        if svc.tier == POST_OFFICE_TIER {
            let vans = out.get(&id).copied().unwrap_or(0);
            if state.facility(id).is_some_and(|f| f.stored != vans)
                || (vans > 0 && state.facility(id).is_none())
            {
                state.facility_mut(id).stored = vans;
            }
        }
        if b.flags.has(BuildingFlags::NO_POWER) {
            continue;
        }
        capacity += match svc.tier {
            POST_OFFICE_TIER => cp_content::budget::budgeted(
                db.balance().mail_post_sort.round() as u32,
                budgets.get(ServiceKind::Post),
                db.balance(),
            ),
            SORTING_TIER => svc.capacity,
            _ => 0,
        };
    }
    mail.sort_capacity = capacity;
    let pass = (tick.0 / COVERAGE_PERIOD) % PASSES_PER_DAY;
    let cap = u64::from(capacity);
    let share = cap * (pass + 1) / PASSES_PER_DAY - cap * pass / PASSES_PER_DAY;
    let sorted = (share as u32).min(mail.unsorted);
    if sorted > 0 {
        mail.unsorted -= sorted;
        mail.today.sorted += sorted;
    }
}

/// A post office that works: Active, powered, of the post office tier and with collecting room
/// left today (`None`: no such building).
fn working_office(
    db: &cp_content::ContentDb,
    budgets: &Budgets,
    buildings: &BuildingGrid,
    state: &ServiceState,
    id: BuildingId,
) -> Option<cp_content::ServiceDef> {
    let b = buildings.get(id)?;
    if b.state != BuildingState::Active || b.flags.has(BuildingFlags::NO_POWER) {
        return None;
    }
    let svc = post_def(db, budgets, buildings, id).filter(|s| s.tier == POST_OFFICE_TIER)?;
    let used = state.facility(id).copied().unwrap_or_default();
    (used.used_today < svc.capacity).then_some(svc)
}

/// Office `office` takes up to `pile` mail within its room today; returns what is left.
fn collect(
    state: &mut ServiceState,
    mail: &mut MailState,
    office: BuildingId,
    svc: &cp_content::ServiceDef,
    pile: u64,
) -> u64 {
    let used = state.facility(office).copied().unwrap_or_default();
    let take = pile.min(u64::from(svc.capacity.saturating_sub(used.used_today)));
    if take > 0 {
        state.facility_mut(office).used_today += take as u32;
        mail.unsorted = mail.unsorted.saturating_add(take as u32);
        mail.today.collected += take as u32;
        mail.today.delivered += take as u32;
    }
    pile - take
}

/// Stores the pile of `id` and sets its MAIL problem by the threshold.
fn store(
    state: &mut ServiceState,
    mail: &mut MailState,
    id: BuildingId,
    pile: u64,
    problem_at: f32,
) {
    let pile = pile.min(u64::from(u16::MAX)) as u16;
    if pile != mail.pile(id) {
        mail.set_pile(id, pile);
    }
    let problem = f32::from(pile) >= problem_at;
    if problem != state.problems_of(id).has(ServiceProblems::MAIL) {
        state
            .get_mut(id)
            .problems
            .set(ServiceProblems::MAIL, problem);
    }
}

/// Services phase, after [`update_mail_sorting`], the coverage rebuild and the daily reset: every
/// tick the buildings of bucket `index % 64 == tick % 64` pile up the mail of the last 64 ticks
/// and, when a post office covers them, have it collected.
#[allow(clippy::too_many_arguments)]
pub fn update_mail(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    net: crate::ServiceNet,
    buildings: Res<BuildingGrid>,
    citizens: Res<CitizenGrid>,
    coverage: Res<ServiceCoverage>,
    mut state: ResMut<ServiceState>,
    mut mail: ResMut<MailState>,
    mut runs: Option<ResMut<ServiceRuns>>,
    unlocks: Option<Res<ServiceUnlocks>>,
) {
    let Some(content) = content else { return };
    // Worlds built without the resource have every service open.
    let unlocks = unlocks.map(|u| *u).unwrap_or_default();
    let db = &content.0;
    let balance = db.balance();
    let budgets = net.budgets();
    let bucket = tick.0 % COVERAGE_PERIOD;
    let pass = (tick.0 / COVERAGE_PERIOD) % PASSES_PER_DAY;
    let roads = net.graph();
    let mut finder = None;
    // Vans that arrived (or failed) are met on the tick they do, so a van is free again at
    // once: the mail is taken from the building they went to, if its office still works.
    if let Some(r) = runs.as_deref_mut() {
        let done: Vec<_> = r
            .iter()
            .filter(|(_, run)| {
                run.kind == VehicleKind::Mail as u8
                    && matches!(run.state, RunState::Arrived { .. } | RunState::Failed)
            })
            .map(|(id, run)| (id, run.from, run.to))
            .collect();
        for (run, from, to) in done {
            r.remove(run);
            let (RunPoint::Building(office), RunPoint::Building(target)) = (from, to) else {
                continue;
            };
            let open = !mail.backlog_full(balance.mail_backlog_days);
            if open
                && unlocks.open(ServiceKind::Post)
                && let Some(svc) = working_office(db, &budgets, &buildings, &state, office)
            {
                let held = u64::from(mail.pile(target));
                let pile = collect(&mut state, &mut mail, office, &svc, held);
                store(&mut state, &mut mail, target, pile, balance.mail_problem);
            }
        }
    }
    // Only this pass's bucket, in id order (T-256: the whole grid was walked to find it).
    for (id, b) in buildings.iter_bucket(COVERAGE_PERIOD as u32, bucket as u32) {
        // No post office is open yet: no mail piles up and nobody minds.
        if !unlocks.open(ServiceKind::Post) {
            if mail.pile(id) > 0 || state.problems_of(id).has(ServiceProblems::MAIL) {
                mail.set_pile(id, 0);
                state.get_mut(id).problems.set(ServiceProblems::MAIL, false);
            }
            continue;
        }
        if b.state != BuildingState::Active {
            continue;
        }
        let def = db.building(b.def);
        // The post's own buildings send no mail.
        if db
            .building_service(b.def)
            .is_some_and(|s| db.service(s).kind == ServiceKind::Post)
        {
            continue;
        }
        let workers: u16 = (0..4).map(|l| citizens.jobs_filled(id, l)).sum();
        let job_mul = match def.zone {
            ZoneType::ComLow | ZoneType::ComHigh => balance.mail_commercial_mul,
            ZoneType::Office => balance.mail_office_mul,
            _ => 1.0,
        };
        let per_day = (f32::from(citizens.households_in(id)) * balance.mail_household
            + f32::from(workers) * balance.mail_job * job_mul)
            .round() as u64;
        let held = mail.pile(id);
        if per_day == 0 && held == 0 {
            continue;
        }
        // The day's total spread over its 64 passes without losing the remainder.
        let made = per_day * (pass + 1) / PASSES_PER_DAY - per_day * pass / PASSES_PER_DAY;
        mail.today.made += made as u32;
        let mut pile = u64::from(held) + made;
        let covered = coverage.coverage_of(ServiceKind::Post, b) > 0.0;
        let open = !mail.backlog_full(balance.mail_backlog_days);
        let has_room = |used: &crate::state::Facility, svc: &cp_content::ServiceDef| {
            svc.tier == POST_OFFICE_TIER
                && used.used_today < svc.capacity
                && (svc.storage == 0 || used.stored < svc.storage)
        };
        let van = runs
            .as_deref()
            .is_some_and(|r| r.find(VehicleKind::Mail, id).is_some());
        match runs.as_deref_mut() {
            // No runs: the mail is taken at once, from the nearest office with room.
            None => {
                if pile > 0
                    && covered
                    && open
                    && let Some((office, _)) = finder
                        .get_or_insert_with(|| {
                            FacilityFinder::new(&roads, &buildings, db, &budgets, ServiceKind::Post)
                        })
                        .nearest_with(&state, id, |used, svc| {
                            svc.tier == POST_OFFICE_TIER && used.used_today < svc.capacity
                        })
                    && let Some(svc) = post_def(db, &budgets, &buildings, office)
                {
                    pile = collect(&mut state, &mut mail, office, &svc, pile);
                }
            }
            // A van is on its way (`arrived` takes the mail), or one is sent now.
            Some(r) => {
                if !van
                    && pile as f32 >= balance.mail_van_min
                    && covered
                    && open
                    && let Some((office, _)) = finder
                        .get_or_insert_with(|| {
                            FacilityFinder::new(&roads, &buildings, db, &budgets, ServiceKind::Post)
                        })
                        .nearest_with(&state, id, has_room)
                {
                    state.facility_mut(office).stored += 1;
                    r.start(VehicleKind::Mail, office, id);
                }
            }
        }
        store(&mut state, &mut mail, id, pile, balance.mail_problem);
    }
}
