//! The contentment pass (T-318, `architecture/system-design.md` §19.2–§19.3): every home's index
//! from eight factors, its complaints, and the district and city roll-ups.
//!
//! [`update_contentment`] runs in the `Economy` phase on the ticks of the city pass
//! (`tick % 256 == 0`, not tick 0), after the services of that phase, and visits the homes
//! (`Active` buildings of a residential zone) in id order. Per home it gathers the facts
//! ([`HomeFacts`]), scores the factors ([`score`], all `0..=100`), takes their weighted mean as
//! the target ([`target`]) and moves the saved index a step towards it ([`step`]); a home seen
//! for the first time starts at its target. The worst factors below `complain_below` become its
//! complaints ([`complaints`]).
//!
//! Factors (weights in `balance.ron` `contentment`):
//! 1. **services** — Health, Police, Fire, Education coverage against `res_service_cover`, the
//!    mean over the kinds the reached milestone has opened (`ServiceUnlocks`); 100 while none is;
//! 2. **environment** — ground pollution and noise at the home (T-300);
//! 3. **home problems** — points per problem bit;
//! 4. **work** and 5. **commute** — 100 for now (T-318b);
//! 6. **leisure** — the parks' green (T-237) plus half for a leisure target within reach;
//! 7. **tax** — the residential rate at the home (district override, else the city);
//! 8. **trust** — the trust in the mayor of the corruption state (T-324), `trust_default`
//!    in a world without it.
//!
//! Integer index, homes in arena order, no RNG, no map iteration: deterministic.

use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Res, ResMut};
use bevy_math::Vec2;
use cp_building::{
    BuildingFlags, BuildingGrid, BuildingId, BuildingState, COMPLAINTS_MAX, Complaint,
    HomeContentment, green_at, park_sites,
};
use cp_citizen::{CitizenGrid, LeisureTargets};
use cp_content::{ContentmentKnobs, ServiceUnlocks, SimContent, ZoneType};
use cp_core::Tick;
use cp_district::{DistrictId, DistrictMap, Districts, district_at};
use cp_service::{ServiceCoverage, ServiceProblems, ServiceState};
use serde::{Deserialize, Serialize};

use crate::growth::CITY_PERIOD;
use crate::land::SERVICE_KINDS;
use crate::{CorruptionState, EconomyState, Emissions};

/// Number of factors.
pub const FACTORS: usize = 8;
/// Factor positions, in the order of `ContentmentKnobs::weights`.
pub const F_SERVICES: usize = 0;
pub const F_ENVIRONMENT: usize = 1;
pub const F_PROBLEMS: usize = 2;
pub const F_WORK: usize = 3;
pub const F_COMMUTE: usize = 4;
pub const F_LEISURE: usize = 5;
pub const F_TAX: usize = 6;
pub const F_TRUST: usize = 7;

/// The complaint of each service kind, in [`SERVICE_KINDS`] order.
const SERVICE_COMPLAINTS: [Complaint; 4] = [
    Complaint::NoHealth,
    Complaint::NoPolice,
    Complaint::NoFire,
    Complaint::NoSchool,
];

/// The complaint of each home problem, in `ContentmentKnobs::problem_penalty` order.
const PROBLEM_COMPLAINTS: [Complaint; 8] = [
    Complaint::NoRoad,
    Complaint::NoWater,
    Complaint::NoPower,
    Complaint::Garbage,
    Complaint::Crime,
    Complaint::BadWater,
    Complaint::Dead,
    Complaint::Sick,
];

/// What a home's factors are scored from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HomeFacts {
    /// Coverage `0..=1` of each kind in [`SERVICE_KINDS`] order; `None` = not expected yet.
    pub services: [Option<f32>; 4],
    /// Ground pollution and noise at the home, `0..=100`.
    pub ground: f32,
    pub noise: f32,
    /// Which problems the home has, in `problem_penalty` order.
    pub problems: [bool; 8],
    /// Share `0..=1` of the households without work long enough to mind; `None` = not known
    /// (scores 100, T-318b fills it).
    pub jobless: Option<f32>,
    /// Minutes to work; `None` = not known or nobody works (scores 100).
    pub commute_min: Option<f32>,
    /// The parks' green at the home, `0..=1`.
    pub green: f32,
    /// A leisure target within `leisure_near_m`.
    pub leisure_near: bool,
    /// The residential tax rate at the home, percent.
    pub tax_pct: u8,
    /// Trust in the mayor, `0..=100`.
    pub trust: u8,
}

/// The factors of one home: a score `0..=100` and the complaint it would give.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scored {
    pub scores: [u8; FACTORS],
    pub codes: [Complaint; FACTORS],
}

fn pct(x: f32) -> u8 {
    x.round().clamp(0.0, 100.0) as u8
}

/// The problems of a home in `problem_penalty` order, from its flags and service problems.
pub fn home_problems(flags: BuildingFlags, service: ServiceProblems) -> [bool; 8] {
    [
        flags.has(BuildingFlags::NO_ROAD) || flags.has(BuildingFlags::NO_PATH),
        flags.has(BuildingFlags::NO_WATER) || flags.has(BuildingFlags::NO_SEWAGE),
        flags.has(BuildingFlags::NO_POWER),
        service.has(ServiceProblems::GARBAGE),
        service.has(ServiceProblems::CRIME),
        service.has(ServiceProblems::BAD_WATER),
        service.has(ServiceProblems::DEAD),
        service.has(ServiceProblems::SICK),
    ]
}

/// Scores the eight factors of a home (§19.2). `cover` is `res_service_cover`: the coverage
/// that counts as full.
pub fn score(f: &HomeFacts, k: &ContentmentKnobs, cover: f32) -> Scored {
    let mut scores = [100u8; FACTORS];
    let mut codes = [Complaint::None; FACTORS];

    // 1. Services: the mean over the expected kinds; the worst one names the complaint.
    let cover = cover.max(1e-6);
    let (mut sum, mut n, mut worst) = (0.0f32, 0u32, None::<(f32, usize)>);
    for (i, c) in f.services.iter().enumerate() {
        if let Some(c) = *c {
            let s = (c / cover).min(1.0) * 100.0;
            sum += s;
            n += 1;
            if worst.is_none_or(|(w, _)| s < w) {
                worst = Some((s, i));
            }
        }
    }
    if n > 0 {
        scores[F_SERVICES] = pct(sum / n as f32);
    }
    codes[F_SERVICES] = worst.map_or(Complaint::None, |(_, i)| SERVICE_COMPLAINTS[i]);

    // 2. Environment.
    let (p, q) = (k.env_pollution_w * f.ground, k.env_noise_w * f.noise);
    scores[F_ENVIRONMENT] = pct(100.0 - (p + q).min(100.0));
    codes[F_ENVIRONMENT] = if p >= q {
        Complaint::Pollution
    } else {
        Complaint::Noise
    };

    // 3. Home problems: the heaviest names the complaint (the first on a tie).
    let mut lost = 0u32;
    let mut heaviest: Option<(u8, usize)> = None;
    for (i, &on) in f.problems.iter().enumerate() {
        if on {
            let w = k.problem_penalty[i];
            lost += u32::from(w);
            if heaviest.is_none_or(|(h, _)| w > h) {
                heaviest = Some((w, i));
            }
        }
    }
    scores[F_PROBLEMS] = 100u32.saturating_sub(lost) as u8;
    codes[F_PROBLEMS] = heaviest.map_or(Complaint::None, |(_, i)| PROBLEM_COMPLAINTS[i]);

    // 4. Work.
    if let Some(j) = f.jobless {
        scores[F_WORK] = pct(100.0 * (1.0 - j.clamp(0.0, 1.0)));
    }
    codes[F_WORK] = Complaint::Jobless;

    // 5. Commute: full at `COMMUTE_GOOD_MIN`, nothing at `COMMUTE_BAD_MIN` (T-318b moves the
    // limits to the balance with the rest of the factor).
    if let Some(m) = f.commute_min {
        let t = ((m - COMMUTE_GOOD_MIN) / (COMMUTE_BAD_MIN - COMMUTE_GOOD_MIN)).clamp(0.0, 1.0);
        scores[F_COMMUTE] = pct(100.0 * (1.0 - t));
    }
    codes[F_COMMUTE] = Complaint::LongCommute;

    // 6. Leisure.
    let near = if f.leisure_near { 50.0 } else { 0.0 };
    scores[F_LEISURE] = pct(100.0 * f.green.clamp(0.0, 1.0) + near);
    codes[F_LEISURE] = Complaint::NoPark;

    // 7. Tax.
    let over = u32::from(f.tax_pct.saturating_sub(k.tax_neutral));
    scores[F_TAX] = 100u32.saturating_sub(over * u32::from(k.tax_step)) as u8;
    codes[F_TAX] = Complaint::HighTax;

    // 8. Trust.
    scores[F_TRUST] = f.trust.min(100);
    codes[F_TRUST] = Complaint::Distrust;

    Scored { scores, codes }
}

/// Minutes to work that cost nothing, and from which the factor is 0 (§19.2 row 5).
pub const COMMUTE_GOOD_MIN: f32 = 15.0;
pub const COMMUTE_BAD_MIN: f32 = 60.0;

/// The weighted mean of the scores, rounded: the index a home moves towards.
pub fn target(scores: &[u8; FACTORS], weights: &[u8; FACTORS]) -> u8 {
    let total: u32 = weights.iter().map(|&w| u32::from(w)).sum();
    if total == 0 {
        return 100;
    }
    let sum: u32 = scores
        .iter()
        .zip(weights)
        .map(|(&s, &w)| u32::from(s) * u32::from(w))
        .sum();
    ((sum + total / 2) / total).min(100) as u8
}

/// One pass of smoothing: `h` moves `1/div` of the way to `t` (at least one point, never past).
pub fn step(h: u8, t: u8, div: u8) -> u8 {
    let gap = h.abs_diff(t);
    if gap == 0 {
        return h;
    }
    let by = (gap / div.max(1)).max(1);
    if t > h { h + by } else { h - by }
}

/// The complaints of a home: the factors below `complain_below` (with a weight), worst first by
/// `weight x (100 - score)`, the first factor on a tie; `Complaint::None` fills the rest.
pub fn complaints(s: &Scored, k: &ContentmentKnobs) -> [Complaint; COMPLAINTS_MAX] {
    let mut bad: Vec<(u32, usize)> = (0..FACTORS)
        .filter(|&i| {
            k.weights[i] > 0 && s.scores[i] < k.complain_below && s.codes[i] != Complaint::None
        })
        .map(|i| (u32::from(k.weights[i]) * u32::from(100 - s.scores[i]), i))
        .collect();
    bad.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut out = [Complaint::None; COMPLAINTS_MAX];
    for (slot, (_, i)) in out.iter_mut().zip(bad) {
        *slot = s.codes[i];
    }
    out
}

/// The contentment of one district (or of the homes outside every district).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DistrictContentment {
    /// `None` = the homes in no district.
    pub district: Option<DistrictId>,
    /// Mean index weighted by residents (by homes while nobody lives there).
    pub index: u8,
    pub homes: u32,
    pub residents: u32,
    /// How many homes name each complaint, most first (the code on a tie).
    pub complaints: Vec<(Complaint, u32)>,
}

/// The roll-ups of the last pass: the city, its factors and its districts. Saved and
/// checksummed with the homes (the client reads it; rebuilding it on a load would need a pass).
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CityContentment {
    /// Mean index of the city weighted by residents (by homes while nobody lives there).
    pub city: u8,
    pub homes: u32,
    pub residents: u32,
    /// Mean score of each factor over the homes, weighted the same way.
    pub factors: [u8; FACTORS],
    /// How many homes name each complaint, most first (the code on a tie).
    pub complaints: Vec<(Complaint, u32)>,
    /// One row per district with homes, in id order, then the homes in no district.
    pub districts: Vec<DistrictContentment>,
}

/// A running weighted mean: by residents, falling back to a plain mean while nobody lives in
/// the homes summed.
#[derive(Clone, Copy, Debug, Default)]
struct Mean {
    by_res: u64,
    res: u64,
    plain: u64,
    n: u64,
}

impl Mean {
    fn add(&mut self, v: u8, residents: u32) {
        self.by_res += u64::from(v) * u64::from(residents);
        self.res += u64::from(residents);
        self.plain += u64::from(v);
        self.n += 1;
    }

    fn get(&self) -> u8 {
        let (sum, n) = if self.res > 0 {
            (self.by_res, self.res)
        } else {
            (self.plain, self.n)
        };
        (sum + n / 2).checked_div(n).map_or(0, |m| m.min(100) as u8)
    }
}

/// One roll-up being summed.
#[derive(Clone, Debug, Default)]
struct Roll {
    index: Mean,
    factors: [Mean; FACTORS],
    homes: u32,
    residents: u32,
    complaints: [u32; Complaint::ALL.len() + 1],
}

impl Roll {
    fn add(&mut self, index: u8, s: &Scored, c: &[Complaint; COMPLAINTS_MAX], residents: u32) {
        self.index.add(index, residents);
        for (m, &v) in self.factors.iter_mut().zip(&s.scores) {
            m.add(v, residents);
        }
        self.homes += 1;
        self.residents += residents;
        for &k in c.iter().filter(|&&k| k != Complaint::None) {
            self.complaints[k as usize] += 1;
        }
    }

    fn histogram(&self) -> Vec<(Complaint, u32)> {
        let mut v: Vec<(Complaint, u32)> = Complaint::ALL
            .iter()
            .map(|&k| (k, self.complaints[k as usize]))
            .filter(|&(_, n)| n > 0)
            .collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v
    }
}

/// Residents per home, dense by `BuildingId.index` with the generation they were counted for.
fn residents_by_home(citizens: &CitizenGrid, slots: usize) -> Vec<(u32, u32)> {
    let mut out = vec![(u32::MAX, 0u32); slots];
    for (_, h) in citizens.households() {
        let Some(slot) = out.get_mut(h.home.index as usize) else {
            continue;
        };
        if slot.0 != h.home.generation {
            *slot = (h.home.generation, 0);
        }
        slot.1 += h.member_ids().count() as u32;
    }
    out
}

fn residents_of(table: &[(u32, u32)], id: BuildingId) -> u32 {
    table
        .get(id.index as usize)
        .filter(|(g, _)| *g == id.generation)
        .map_or(0, |(_, n)| *n)
}

fn is_home(zone: ZoneType) -> bool {
    matches!(zone, ZoneType::ResLow | ZoneType::ResHigh)
}

/// Whether a leisure target that draws anybody stands within `reach` of `pos`.
fn leisure_near(targets: Option<&LeisureTargets>, pos: Vec2, reach: f32) -> bool {
    let r2 = reach * reach;
    targets.is_some_and(|t| {
        t.list
            .iter()
            .any(|l| l.weight > 0.0 && l.pos.distance_squared(pos) <= r2)
    })
}

/// `Economy`-phase system, see the module docs.
#[allow(clippy::too_many_arguments)]
pub fn update_contentment(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    buildings: Res<BuildingGrid>,
    citizens: Res<CitizenGrid>,
    emissions: Res<Emissions>,
    economy: Res<EconomyState>,
    (district_map, districts): (Res<DistrictMap>, Res<Districts>),
    coverage: Option<Res<ServiceCoverage>>,
    services: Option<Res<ServiceState>>,
    unlocks: Option<Res<ServiceUnlocks>>,
    (leisure, corruption): (Option<Res<LeisureTargets>>, Option<Res<CorruptionState>>),
    mut homes: ResMut<HomeContentment>,
    mut city: ResMut<CityContentment>,
) {
    if tick.0 == 0 || !tick.0.is_multiple_of(CITY_PERIOD) {
        return;
    }
    let Some(content) = content else { return };
    let db = &*content.0;
    let bal = db.balance();
    let k = &bal.contentment;
    let unlocks = unlocks.as_deref().copied().unwrap_or_default();
    let expected = SERVICE_KINDS.map(|kind| unlocks.open(kind));
    let trust = corruption.as_deref().map_or(k.trust_default, |c| {
        c.trust_now(&bal.corruption).round().clamp(0.0, 100.0) as u8
    });
    let parks = park_sites(db, &buildings);
    let slots = buildings.iter().map(|(id, _)| id.index as usize + 1).max();
    let residents = residents_by_home(&citizens, slots.unwrap_or(0));

    // Records of buildings that are no active home any more.
    let gone: Vec<BuildingId> = homes
        .homes()
        .map(|(id, _)| id)
        .filter(|&id| {
            buildings.get(id).is_none_or(|b| {
                b.state != BuildingState::Active || !is_home(db.building(b.def).zone)
            })
        })
        .collect();
    for id in gone {
        homes.remove(id);
    }

    let mut all = Roll::default();
    // Keyed by district id, the homes in no district last.
    let mut by_district: std::collections::BTreeMap<u16, Roll> = Default::default();
    for (id, b) in buildings.iter() {
        if b.state != BuildingState::Active {
            continue;
        }
        let def = db.building(b.def);
        if !is_home(def.zone) {
            continue;
        }
        let service_problems = services
            .as_deref()
            .map_or_else(ServiceProblems::default, |s| s.problems_of(id));
        let district = district_at(&district_map, b.pos);
        let tax_pct = district
            .and_then(|d| districts.tax_override(d, def.zone))
            .or_else(|| economy.taxes.get(def.zone))
            .unwrap_or(0);
        let facts = HomeFacts {
            services: std::array::from_fn(|i| {
                expected[i].then(|| {
                    coverage
                        .as_deref()
                        .map_or(0.0, |c| c.coverage_of(SERVICE_KINDS[i], b))
                })
            }),
            ground: emissions.ground_at(b.pos),
            noise: emissions.noise_at(b.pos),
            problems: home_problems(b.flags, service_problems),
            jobless: None,
            commute_min: None,
            green: green_at(&parks, b.pos),
            leisure_near: leisure_near(leisure.as_deref(), b.pos, k.leisure_near_m),
            tax_pct,
            trust,
        };
        let scored = score(&facts, k, bal.res_service_cover);
        let t = target(&scored.scores, &k.weights);
        let index = homes.get(id).map_or(t, |s| step(s.index, t, k.step_div));
        let named = complaints(&scored, k);
        homes.set(id, index, named);
        let n = residents_of(&residents, id);
        all.add(index, &scored, &named, n);
        by_district
            .entry(district.map_or(u16::MAX, |d| u16::from(d.0)))
            .or_default()
            .add(index, &scored, &named, n);
    }

    let next = CityContentment {
        city: all.index.get(),
        homes: all.homes,
        residents: all.residents,
        factors: std::array::from_fn(|i| all.factors[i].get()),
        complaints: all.histogram(),
        districts: by_district
            .into_iter()
            .map(|(key, r)| DistrictContentment {
                district: (key != u16::MAX).then_some(DistrictId(key as u8)),
                index: r.index.get(),
                homes: r.homes,
                residents: r.residents,
                complaints: r.histogram(),
            })
            .collect(),
    };
    if *city != next {
        *city = next;
    }
}
