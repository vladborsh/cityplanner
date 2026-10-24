//! Corruption, institutions and trust (`architecture/corruption.md`, T-324): the city's state
//! and its daily update. Phase 1 grows it task by task: T-324 the state, trust and its pull on
//! the tax take; T-325 the deals that fill the inbox and the evidence; T-326 scandals and
//! captured police stations; T-327 the institutions.
//!
//! * **Daily** (the first tick of every game day after tick 0): corruption falls by
//!   `corruption_decay_day`, trust climbs by `trust_recover_day` towards its ceiling
//!   (`trust_ceiling`, `trust_ceiling_clan` on the clan edge of the axis `C - I`). A new city
//!   gets `trust_start` on its first day; until then [`CorruptionState::trust_now`] reads it.
//! * **Tax** ([`CorruptionState::tax_collection`]): below `tax_trust_full` trust a share of
//!   the tax is lost, linearly up to `tax_loss_max` at no trust. `settle_week` applies it.
//! * **Move-in** is not here: trust is factor 8 of the home contentment (`system-design.md`
//!   §19.2), which reads [`CorruptionState::trust_now`].
//! * **Offers** (T-325, §4): once the city has `deals_population` residents, a deal of the
//!   content comes to the inbox every few days (sooner in a bigger city on dearer land), with a
//!   target the deal wants (a district under its policy, a factory, a shop) picked by the sim
//!   RNG. An offer not answered by `expires_day` is declined.
//! * **Answers** ([`CorruptionCommand`]): accepting pays now (money, trust, a lifted district
//!   policy), adds `C` and the evidence, and leaves an [`ActiveDeal`] whose effects run for the
//!   deal's `days`: a weekly payment, a build discount (the command dispatcher), more pollution
//!   from the target ([`crate::EmissionInputs::boost`]), more crime around it
//!   (`update_police`), more tourists ([`cp_transit::Tourists::boost_pct`]). Declining costs
//!   trust on the clan edge.
//!
//! Saved whole; points are `f32` (same binary and platform determinism, AGENTS.md).

use std::fmt;

use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Res, ResMut};
use bevy_ecs::world::World;
use bevy_math::Vec2;
use cp_building::{BuildingGrid, BuildingId, BuildingState};
use cp_citizen::CitizenGrid;
use cp_content::{ContentDb, CorruptionKnobs, DealDef, DealWants, SimContent, ZoneType};
use cp_core::time::TICKS_PER_DAY;
use cp_core::{Cost, SimRng, Tick, Treasury};
use cp_district::{DistrictId, Districts};
use serde::{Deserialize, Serialize};

use crate::land::LandGrids;

/// Sub-stream of the sim RNG the offers are drawn from (forked by the tick).
pub const CORRUPTION_STREAM: u64 = 0xC022_0F7E;

/// An accepted deal nobody has found out about yet (§2): it stays until a scandal reveals it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    /// The deal's content id.
    pub deal: String,
    pub accepted_day: u32,
    pub weight: f32,
}

/// What an offer is about, lit on the map (§4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DealTarget {
    District(DistrictId),
    Building(BuildingId),
}

/// A deal waiting in the inbox for an answer until `expires_day` (§4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Offer {
    /// The deal's content id.
    pub deal: String,
    pub target: Option<DealTarget>,
    pub expires_day: u32,
}

/// An accepted deal whose effects run until `until_day` (0: for good).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActiveDeal {
    /// The deal's content id.
    pub deal: String,
    pub target: Option<DealTarget>,
    pub accepted_day: u32,
    pub until_day: u32,
}

/// The answer to an offer in the inbox, by the deal's id (one offer of a deal at a time).
/// `SimCommand::Corruption`. Append-only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CorruptionCommand {
    Accept { deal: String },
    Decline { deal: String },
}

/// Why a [`CorruptionCommand`] was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CorruptionError {
    /// The world has no corruption state or content.
    Missing,
    /// No offer of this deal is waiting.
    NoOffer(String),
}

impl fmt::Display for CorruptionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(f, "no corruption state in the sim world"),
            Self::NoOffer(d) => write!(f, "no offer of deal {d:?} is waiting"),
        }
    }
}

impl std::error::Error for CorruptionError {}

/// The city's corruption state (§2). See the module docs.
#[derive(Resource, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CorruptionState {
    /// Corruption `C`, `0..=100`.
    pub corruption: f32,
    /// Institutions `I`, `0..=100`.
    pub institutions: f32,
    /// Trust in the mayor, `0..=100`; meaningful once `started`.
    pub trust: f32,
    /// Whether the first day has seeded `trust` from the content.
    pub started: bool,
    pub evidence: Vec<Evidence>,
    pub inbox: Vec<Offer>,
    /// Police stations on the take, sorted.
    pub captured: Vec<BuildingId>,
    pub next_offer_day: u32,
    pub scandal_cooldown_until: u32,
    /// Accepted deals whose effects still run, in the order they were accepted (T-325).
    pub active: Vec<ActiveDeal>,
}

impl Default for CorruptionState {
    fn default() -> Self {
        Self {
            corruption: 0.0,
            institutions: 0.0,
            trust: 0.0,
            started: false,
            evidence: Vec::new(),
            inbox: Vec::new(),
            captured: Vec::new(),
            next_offer_day: 0,
            scandal_cooldown_until: 0,
            active: Vec::new(),
        }
    }
}

/// Where the city stands on the axis of power.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    /// Clan power: `C - I > edge`.
    Clan,
    /// The grey middle.
    Middle,
    /// Rule of law: `I - C > edge`.
    Law,
}

impl CorruptionState {
    /// The axis `C - I` (§2).
    pub fn axis(&self) -> f32 {
        self.corruption - self.institutions
    }

    pub fn edge(&self, k: &CorruptionKnobs) -> Edge {
        let a = self.axis();
        if a > k.edge {
            Edge::Clan
        } else if a < -k.edge {
            Edge::Law
        } else {
            Edge::Middle
        }
    }

    /// Trust as it stands: `trust_start` before the first day.
    pub fn trust_now(&self, k: &CorruptionKnobs) -> f32 {
        if self.started {
            self.trust
        } else {
            k.trust_start
        }
    }

    /// The highest trust can climb to (§3).
    pub fn trust_ceiling(&self, k: &CorruptionKnobs) -> f32 {
        match self.edge(k) {
            Edge::Clan => k.trust_ceiling_clan,
            _ => k.trust_ceiling,
        }
    }

    /// The share of the tax the city collects (§3), `1 - tax_loss_max ..= 1`.
    pub fn tax_collection(&self, k: &CorruptionKnobs) -> f64 {
        let full = k.tax_trust_full;
        if full <= 0.0 {
            return 1.0;
        }
        let short = ((full - self.trust_now(k)) / full).clamp(0.0, 1.0);
        1.0 - f64::from(short * k.tax_loss_max)
    }

    /// The share off the cost of building the running deals give, `0..=1`.
    pub fn build_discount(&self, db: &ContentDb) -> f32 {
        self.active
            .iter()
            .filter_map(|a| db.deal(&a.deal))
            .map(|d| d.build_discount)
            .sum::<f32>()
            .clamp(0.0, 1.0)
    }

    /// The extra pollution share of each building a running deal targets, sorted by id.
    pub fn pollution_boost(&self, db: &ContentDb) -> Vec<(BuildingId, f32)> {
        let mut v: Vec<(BuildingId, f32)> = self
            .active
            .iter()
            .filter_map(|a| match (a.target, db.deal(&a.deal)) {
                (Some(DealTarget::Building(b)), Some(d)) if d.pollution > 0.0 => {
                    Some((b, d.pollution))
                }
                _ => None,
            })
            .collect();
        v.sort_by_key(|&(b, _)| b);
        v
    }

    /// `(centre, radius, extra share)` of the crime the running deals raise around their target.
    pub fn crime_areas(&self, db: &ContentDb, buildings: &BuildingGrid) -> Vec<(Vec2, f32, f32)> {
        self.active
            .iter()
            .filter_map(|a| match (a.target, db.deal(&a.deal)) {
                (Some(DealTarget::Building(b)), Some(d)) if d.crime > 0.0 => {
                    Some((buildings.get(b)?.pos, d.radius_m, d.crime))
                }
                _ => None,
            })
            .collect()
    }

    /// The extra tourist share of the running deals.
    pub fn tourist_boost(&self, db: &ContentDb) -> f32 {
        self.active
            .iter()
            .filter_map(|a| db.deal(&a.deal))
            .map(|d| d.tourists)
            .sum()
    }

    /// What a declined (or lapsed) offer does: on the clan edge it costs trust.
    fn declined(&mut self, k: &CorruptionKnobs) {
        if self.edge(k) == Edge::Clan {
            self.trust = (self.trust - k.decline_trust).clamp(0.0, 100.0);
        }
    }

    /// One game day of drift (see the module docs).
    pub fn day(&mut self, k: &CorruptionKnobs) {
        if !self.started {
            self.trust = k.trust_start;
            self.started = true;
        }
        self.corruption = (self.corruption - k.corruption_decay_day).clamp(0.0, 100.0);
        let ceiling = self.trust_ceiling(k);
        // Above the ceiling (the city slid to the clan edge) trust sinks to it at the same pace.
        self.trust = if self.trust < ceiling {
            (self.trust + k.trust_recover_day).min(ceiling)
        } else {
            (self.trust - k.trust_recover_day).max(ceiling)
        }
        .clamp(0.0, 100.0);
    }
}

/// The game day `tick` falls in.
pub fn day_of(tick: Tick) -> u32 {
    (tick.0 / TICKS_PER_DAY) as u32
}

/// Days until the next offer in a city of `population` on land worth `land` on average
/// (`0..=100`), see `CorruptionKnobs::offer_days`.
pub fn offer_interval(k: &CorruptionKnobs, population: u32, land: f32) -> u32 {
    let extra = (population as f32 - k.deals_population).max(0.0);
    let rate =
        1.0 + extra / k.offer_pop_step.max(1.0) + k.offer_land * land.clamp(0.0, 100.0) / 100.0;
    (k.offer_days / rate).max(k.offer_days_min).ceil() as u32
}

/// The targets `wants` can have in the city, in id order.
fn targets(
    wants: DealWants,
    db: &ContentDb,
    buildings: &BuildingGrid,
    districts: Option<&Districts>,
    taken: &[DealTarget],
) -> Vec<Option<DealTarget>> {
    let free = |t: &DealTarget| !taken.contains(t);
    match wants {
        DealWants::City => vec![None],
        DealWants::DistrictWith(policy) => districts
            .map(|ds| {
                ds.list
                    .iter()
                    .enumerate()
                    .filter(|(_, d)| d.as_ref().is_some_and(|d| d.policies & policy.bit() != 0))
                    .map(|(i, _)| DealTarget::District(DistrictId(i as u8 + 1)))
                    .filter(free)
                    .map(Some)
                    .collect()
            })
            .unwrap_or_default(),
        DealWants::Industry | DealWants::Commercial => buildings
            .iter()
            .filter(|(_, b)| b.state == BuildingState::Active)
            .filter(|(_, b)| {
                let def = db.building(b.def);
                if wants == DealWants::Industry {
                    def.service.is_none()
                        && (def.industry.is_some() || def.zone == ZoneType::Industrial)
                } else {
                    def.service.is_none()
                        && matches!(def.zone, ZoneType::ComLow | ZoneType::ComHigh)
                }
            })
            .map(|(id, _)| DealTarget::Building(id))
            .filter(free)
            .map(Some)
            .collect(),
    }
}

/// Whether the target of a running deal is still there.
fn target_stands(
    t: Option<DealTarget>,
    buildings: &BuildingGrid,
    districts: Option<&Districts>,
) -> bool {
    match t {
        None => true,
        Some(DealTarget::Building(b)) => buildings.get(b).is_some(),
        Some(DealTarget::District(d)) => districts.is_some_and(|ds| ds.get(d).is_some()),
    }
}

/// The city around the daily update.
pub struct DayInputs<'a> {
    pub db: &'a ContentDb,
    pub tick: Tick,
    pub rng: &'a SimRng,
    pub population: u32,
    /// Mean land value of the zoned buildings, `0..=100`.
    pub land: f32,
    pub buildings: &'a BuildingGrid,
    pub districts: Option<&'a Districts>,
}

impl CorruptionState {
    /// The daily update after [`Self::day`]: lapsed offers and finished deals go, the running
    /// deals pay their week, a new offer may come. Returns the money the deals paid.
    pub fn deals_day(&mut self, i: &DayInputs) -> Cost {
        let k = &i.db.balance().corruption;
        let today = day_of(i.tick);
        let lapsed = self.inbox.iter().filter(|o| o.expires_day <= today).count();
        self.inbox.retain(|o| o.expires_day > today);
        for _ in 0..lapsed {
            self.declined(k);
        }
        self.active.retain(|a| {
            (a.until_day == 0 || a.until_day > today)
                && i.db.deal(&a.deal).is_some()
                && target_stands(a.target, i.buildings, i.districts)
        });
        let mut paid = Cost::ZERO;
        for a in &self.active {
            if let Some(d) = i.db.deal(&a.deal)
                && d.weekly != 0
                && today > a.accepted_day
                && (today - a.accepted_day).is_multiple_of(7)
            {
                paid += Cost::from_units(d.weekly as f32);
            }
        }
        if i.population as f32 >= k.deals_population
            && today >= self.next_offer_day
            && (self.inbox.len() as f32) < k.inbox_max
        {
            self.next_offer_day = today + offer_interval(k, i.population, i.land);
            self.offer(i, today);
        }
        paid
    }

    /// Draws one offer: a deal neither waiting nor running, with a free target.
    fn offer(&mut self, i: &DayInputs, today: u32) {
        let taken: Vec<DealTarget> = self
            .active
            .iter()
            .filter_map(|a| a.target)
            .chain(self.inbox.iter().filter_map(|o| o.target))
            .collect();
        let busy = |id: &str| {
            self.inbox.iter().any(|o| o.deal == id) || self.active.iter().any(|a| a.deal == id)
        };
        let choices: Vec<(&DealDef, Vec<Option<DealTarget>>)> =
            i.db.deals()
                .iter()
                .filter(|d| !busy(&d.id))
                .map(|d| (d, targets(d.target, i.db, i.buildings, i.districts, &taken)))
                .filter(|(_, t)| !t.is_empty())
                .collect();
        if choices.is_empty() {
            return;
        }
        let mut r = i.rng.fork(CORRUPTION_STREAM, i.tick);
        let (d, ts) = &choices[r.below(choices.len() as u32) as usize];
        let target = ts[r.below(ts.len() as u32) as usize];
        self.inbox.push(Offer {
            deal: d.id.clone(),
            target,
            expires_day: today + u32::from(d.answer_days),
        });
    }
}

/// Applies an answer to an offer (see the module docs). Charges nothing; an accepted deal's
/// money goes straight to the treasury.
pub fn apply(world: &mut World, cmd: &CorruptionCommand) -> Result<Cost, CorruptionError> {
    let content = world
        .get_resource::<SimContent>()
        .ok_or(CorruptionError::Missing)?
        .0
        .clone();
    let k = &content.balance().corruption;
    let today = day_of(world.get_resource::<Tick>().copied().unwrap_or_default());
    let mut state = world
        .get_resource_mut::<CorruptionState>()
        .ok_or(CorruptionError::Missing)?;
    let (CorruptionCommand::Accept { deal } | CorruptionCommand::Decline { deal }) = cmd;
    let at = state
        .inbox
        .iter()
        .position(|o| &o.deal == deal)
        .ok_or_else(|| CorruptionError::NoOffer(deal.clone()))?;
    let offer = state.inbox.remove(at);
    let Some(def) = content.deal(deal) else {
        // Content without the deal any more: the offer just goes.
        return Ok(Cost::ZERO);
    };
    if matches!(cmd, CorruptionCommand::Decline { .. }) {
        state.declined(k);
        return Ok(Cost::ZERO);
    }
    state.corruption = (state.corruption + def.corruption).clamp(0.0, 100.0);
    if def.evidence > 0.0 {
        state.evidence.push(Evidence {
            deal: def.id.clone(),
            accepted_day: today,
            weight: def.evidence,
        });
    }
    let trust = state.trust_now(k);
    state.trust = (trust + def.trust).clamp(0.0, 100.0);
    state.started = true;
    if def.lasts() {
        state.active.push(ActiveDeal {
            deal: def.id.clone(),
            target: offer.target,
            accepted_day: today,
            until_day: if def.days == 0 {
                0
            } else {
                today + u32::from(def.days)
            },
        });
    }
    if def.lift_policy
        && let (DealWants::DistrictWith(policy), Some(DealTarget::District(d))) =
            (def.target, offer.target)
        && let Some(mut ds) = world.get_resource_mut::<Districts>()
        && let Some(d) = ds.get_mut(d)
    {
        d.policies &= !policy.bit();
    }
    if def.money != 0
        && let Some(mut t) = world.get_resource_mut::<Treasury>()
    {
        t.balance += Cost::from_units(def.money as f32);
    }
    Ok(Cost::ZERO)
}

/// `Economy`-phase system: [`CorruptionState::day`] and [`CorruptionState::deals_day`] in the
/// first tick of every game day after tick 0.
#[allow(clippy::too_many_arguments)]
pub fn update_corruption(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    rng: Res<SimRng>,
    citizens: Res<CitizenGrid>,
    buildings: Res<BuildingGrid>,
    land: Res<LandGrids>,
    districts: Option<Res<Districts>>,
    (mut state, mut treasury, tourists): (
        ResMut<CorruptionState>,
        ResMut<Treasury>,
        Option<ResMut<cp_transit::Tourists>>,
    ),
) {
    if tick.0 == 0 || !tick.0.is_multiple_of(TICKS_PER_DAY) {
        return;
    }
    let Some(content) = content else { return };
    let db = &*content.0;
    state.day(&db.balance().corruption);
    let (mut sum, mut n) = (0.0f64, 0u32);
    for (_, b) in buildings.iter() {
        if b.state == BuildingState::Active && cp_building::is_growable(db.building(b.def)) {
            sum += f64::from(land.value_at(b.pos));
            n += 1;
        }
    }
    let paid = state.deals_day(&DayInputs {
        db,
        tick: *tick,
        rng: &rng,
        population: citizens.citizen_count() as u32,
        land: if n == 0 {
            0.0
        } else {
            (sum / f64::from(n)) as f32
        },
        buildings: &buildings,
        districts: districts.as_deref(),
    });
    if paid.0 != 0 {
        treasury.balance += paid;
    }
    let boost = (state.tourist_boost(db) * 100.0).round().clamp(0.0, 1000.0) as u16;
    if let Some(mut t) = tourists
        && t.boost_pct != boost
    {
        t.boost_pct = boost;
    }
}
