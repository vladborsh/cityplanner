//! `cp_industry` — production of the industrial areas (T-209, `architecture/system-design.md`
//! §16 M5 (C)).
//!
//! Once a day, in 64 buckets by `BuildingId.index % 64` (the same clock as the goods of M3), each
//! `Active` chain building does its work with `f` = filled job slots / job slots:
//!
//! * **Extractor** adds `output × f × richness` of its raw cargo (richness is the average under
//!   the lot, [`cp_building::lot_richness`]); ore and oil are taken from the
//!   [`ResourceDeposits`] cells under the lot, and an empty lot is [`IndustryProblem::Depleted`].
//! * **Processor** takes its inputs in proportion to what it has (nothing: `NoInput`) and
//!   adds its outputs.
//! * Production stops at the building's storage (`Full`); a building whose area has no Main
//!   building is `NoMain` and does nothing.
//!
//! The area earns `xp` for what its buildings made, and rises one level at each of
//! `Balance::industry_levels`; each level above the first adds `industry_storage_per_level` to
//! every building's storage. Cargo leaves in trucks (T-210, [`freight`]).

pub mod freight;
pub mod warehouse;

use std::collections::BTreeMap;

use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Res, ResMut};
use cp_building::{BuildingGrid, BuildingId, BuildingState, footprint_points, lot_richness};
use cp_citizen::CitizenGrid;
use cp_content::{CargoKind, IndustryRole, ResourceKind, SimContent};
use cp_core::Tick;
use cp_core::time::TICKS_PER_DAY;
use cp_district::{AreaMap, Areas, area_at};
use cp_terrain::{ResourceDeposits, ResourceMap, SimTerrain};
use cp_zone::ZoneGrid;
use serde::{Deserialize, Serialize};

/// Ticks between two buckets: a day in 64 parts.
pub const INDUSTRY_BUCKET_TICKS: u64 = TICKS_PER_DAY / 64;
/// Highest area level.
pub const MAX_AREA_LEVEL: u8 = 5;

/// Why a building is not producing (shown over it by the client). Append-only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndustryProblem {
    #[default]
    None,
    /// The area has no main building.
    NoMain,
    /// A processor lacks input.
    NoInput,
    /// The storage is full.
    Full,
    /// The ground under an extractor has no ore or oil left.
    Depleted,
}

/// What the chain buildings hold and how they are doing. Saved and in the checksum.
#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct IndustryState {
    /// Stock per building, per cargo (cargo kinds in order; empty entries are removed).
    pub stock: BTreeMap<BuildingId, Vec<(CargoKind, f32)>>,
    /// The problem of each building that has one.
    pub problems: BTreeMap<BuildingId, IndustryProblem>,
}

impl IndustryState {
    pub fn is_empty(&self) -> bool {
        self.stock.is_empty() && self.problems.is_empty()
    }

    /// Units of `cargo` in `building`.
    pub fn stock_of(&self, building: BuildingId, cargo: CargoKind) -> f32 {
        self.stock
            .get(&building)
            .and_then(|s| s.iter().find(|(k, _)| *k == cargo))
            .map_or(0.0, |&(_, v)| v)
    }

    /// Everything the building holds, in units.
    pub fn total(&self, building: BuildingId) -> f32 {
        self.stock
            .get(&building)
            .map_or(0.0, |s| s.iter().map(|&(_, v)| v).sum())
    }

    pub fn problem_of(&self, building: BuildingId) -> IndustryProblem {
        self.problems
            .get(&building)
            .copied()
            .unwrap_or(IndustryProblem::None)
    }

    /// Adds `delta` (may be negative) of `cargo`, keeping the list sorted and without zeros.
    pub(crate) fn add(&mut self, building: BuildingId, cargo: CargoKind, delta: f32) {
        let list = self.stock.entry(building).or_default();
        match list.iter().position(|(k, _)| *k >= cargo) {
            Some(i) if list[i].0 == cargo => list[i].1 = (list[i].1 + delta).max(0.0),
            Some(i) => list.insert(i, (cargo, delta.max(0.0))),
            None => list.push((cargo, delta.max(0.0))),
        }
        list.retain(|&(_, v)| v > 0.0);
        if list.is_empty() {
            self.stock.remove(&building);
        }
    }

    fn set_problem(&mut self, building: BuildingId, p: IndustryProblem) {
        if p == IndustryProblem::None {
            self.problems.remove(&building);
        } else {
            self.problems.insert(building, p);
        }
    }
}

/// The storage of a building in an area of `level`: `storage × (1 + per_level × (level - 1))`.
pub fn storage_cap(storage: f32, per_level: f32, level: u8) -> f32 {
    storage * (1.0 + per_level * f32::from(level.saturating_sub(1)))
}

/// The level an area with `xp` has reached, given the thresholds for levels 2 to 5.
pub fn level_for_xp(xp: u32, thresholds: &[u32; 4]) -> u8 {
    1 + thresholds.iter().filter(|&&t| xp >= t).count() as u8
}

/// `Agents` phase, after `run_goods`: the bucket of this tick.
#[allow(clippy::too_many_arguments)]
pub fn run_industry(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    terrain: Option<Res<SimTerrain>>,
    zones: Res<ZoneGrid>,
    citizens: Res<CitizenGrid>,
    buildings: Res<BuildingGrid>,
    area_map: Res<AreaMap>,
    mut areas: ResMut<Areas>,
    mut deposits: ResMut<ResourceDeposits>,
    mut state: ResMut<IndustryState>,
) {
    let (Some(content), Some(terrain)) = (content, terrain) else {
        return;
    };
    let now = *tick;
    if !now.0.is_multiple_of(INDUSTRY_BUCKET_TICKS) {
        return;
    }
    let db = &*content.0;
    let bal = db.balance();
    let bucket = (now.0 / INDUSTRY_BUCKET_TICKS) % 64;
    state.stock.retain(|id, _| buildings.get(*id).is_some());
    state.problems.retain(|id, _| buildings.get(*id).is_some());
    let resources = terrain.0.resources();

    // The areas with a Main building, as of now (a Main that is still being built counts).
    let mut has_main: BTreeMap<u8, bool> = BTreeMap::new();
    for (_, b) in buildings.iter() {
        if db
            .building(b.def)
            .industry
            .as_ref()
            .is_some_and(|s| s.role == IndustryRole::Main)
            && let Some(a) = area_at(&area_map, b.pos)
        {
            has_main.insert(a.0, true);
        }
    }

    for (id, b) in buildings.iter() {
        if u64::from(id.index) % 64 != bucket {
            continue;
        }
        let def = db.building(b.def);
        let Some(spec) = &def.industry else { continue };
        if b.state != BuildingState::Active {
            state.set_problem(id, IndustryProblem::None);
            continue;
        }
        let area_id = area_at(&area_map, b.pos);
        let Some(area_id) = area_id.filter(|a| has_main.contains_key(&a.0)) else {
            state.set_problem(id, IndustryProblem::NoMain);
            continue;
        };
        let level = areas.get(area_id).map_or(1, |a| a.level);
        if spec.role == IndustryRole::Main {
            state.set_problem(id, IndustryProblem::None);
            continue;
        }
        let slots: u32 = def.jobs.iter().map(|&j| u32::from(j)).sum();
        let filled: u32 = (0..4).map(|l| u32::from(citizens.jobs_filled(id, l))).sum();
        let f = if slots == 0 {
            1.0
        } else {
            (filled as f32 / slots as f32).min(1.0)
        };
        let cap = storage_cap(spec.storage, bal.industry_storage_per_level, level);
        let room = (cap - state.total(id)).max(0.0);
        let mut produced = 0.0f32;
        let problem = match spec.role {
            IndustryRole::Extractor => {
                let Some(kind) = spec.resource else { continue };
                let points = footprint_points(&zones, &b.footprint, b.pos, b.rot);
                let richness = lot_richness(resources, kind, &points);
                let want: f32 = spec.outputs.iter().map(|&(_, r)| r).sum::<f32>() * f * richness;
                if room <= 0.0 {
                    IndustryProblem::Full
                } else if want <= 0.0 {
                    IndustryProblem::None
                } else {
                    let mut amount = want.min(room);
                    let mut problem = IndustryProblem::None;
                    if kind.is_finite() {
                        let taken = take_from_lot(&mut deposits, resources, kind, &points, amount);
                        if taken <= 0.0 {
                            problem = IndustryProblem::Depleted;
                        }
                        amount = taken;
                    }
                    if amount > 0.0 {
                        let total_rate: f32 = spec.outputs.iter().map(|&(_, r)| r).sum();
                        for &(cargo, rate) in &spec.outputs {
                            state.add(id, cargo, amount * rate / total_rate);
                        }
                        produced = amount;
                    }
                    problem
                }
            }
            IndustryRole::Processor => {
                let in_scale = spec
                    .inputs
                    .iter()
                    .map(|&(c, r)| (state.stock_of(id, c) / (r * f).max(f32::EPSILON)).min(1.0))
                    .fold(1.0f32, f32::min);
                let out_total: f32 = spec.outputs.iter().map(|&(_, r)| r).sum::<f32>() * f;
                let in_total: f32 = spec.inputs.iter().map(|&(_, r)| r).sum::<f32>() * f;
                // Inputs leave the building, so the room is what the outputs add net of them.
                let net = (out_total - in_total).max(0.0);
                let room_scale = if net > 0.0 {
                    (room / net).min(1.0)
                } else {
                    1.0
                };
                let p = in_scale.min(room_scale);
                if f <= 0.0 {
                    IndustryProblem::None
                } else if in_scale <= 0.0 {
                    IndustryProblem::NoInput
                } else if p <= 0.0 {
                    IndustryProblem::Full
                } else {
                    for &(c, r) in &spec.inputs {
                        state.add(id, c, -r * f * p);
                    }
                    for &(c, r) in &spec.outputs {
                        state.add(id, c, r * f * p);
                        produced += r * f * p;
                    }
                    IndustryProblem::None
                }
            }
            _ => IndustryProblem::None,
        };
        state.set_problem(id, problem);
        if produced > 0.0
            && let Some(a) = areas.get_mut(area_id)
        {
            a.xp = a.xp.saturating_add(produced as u32);
            a.level = a
                .level
                .max(level_for_xp(a.xp, &bal.industry_levels))
                .min(MAX_AREA_LEVEL);
        }
    }
}

/// Takes up to `amount` of an exhaustible resource from the cells under a lot, splitting it
/// evenly over the cells that have some left (a cell that runs dry hands the rest to the next
/// pass); returns what was taken. One unit is the deposit's `u16` step.
fn take_from_lot(
    deposits: &mut ResourceDeposits,
    _resources: &ResourceMap,
    kind: ResourceKind,
    points: &[bevy_math::Vec2],
    amount: f32,
) -> f32 {
    let mut cells: Vec<(usize, usize)> = points.iter().map(|&p| ResourceMap::cell_of(p)).collect();
    cells.sort_unstable();
    cells.dedup();
    let mut left = amount.round().max(0.0) as u32;
    let mut taken = 0u32;
    for _ in 0..4 {
        if left == 0 {
            break;
        }
        let live: Vec<(usize, usize)> = cells
            .iter()
            .copied()
            .filter(|&(x, z)| deposits.remaining(kind, x, z) > 0)
            .collect();
        if live.is_empty() {
            break;
        }
        let share = (left / live.len() as u32).max(1);
        for (x, z) in live {
            let want = share.min(left).min(u32::from(u16::MAX)) as u16;
            let got = u32::from(deposits.take(kind, x, z, want));
            taken += got;
            left -= got;
        }
    }
    taken as f32
}

pub use freight::{IndustryLedger, run_freight};
pub use warehouse::{
    IndustryCommand, IndustryError, WarehouseMode, WarehouseSetting, WarehouseSettings, apply,
    chain_cargo,
};

#[cfg(test)]
mod tests;
