//! Player commands of this domain (`architecture/system-design.md` §4, level 2).
//! M1 variants are added by the domain tasks; new variants are only appended
//! (save stability rule).
//!
//! `Place` puts a service building (a def with `service: Some(..)`) on the lot grid of the
//! zone blocks beside a road, so it is adjacent to the road by construction and shares the
//! occupancy and revision bookkeeping of spawned buildings. Zoned buildings are never
//! placed by the player: they spawn. `Demolish` removes any building.
//!
//! [`validate_placement`] is the pure check the placement tool previews with and `apply`
//! commits with. Money is not deducted (budget task); content has no building price yet, so
//! the cost is zero.

use std::f32::consts::FRAC_PI_2;
use std::fmt;

use bevy_ecs::world::World;
use bevy_math::Vec2;
use cp_content::{BuildingDef, BuildingDefId, ContentDb, ServiceSite, SimContent};
use cp_core::time::TICKS_PER_DAY;
use cp_core::{Cost, SimLimits, Tick};
use cp_net::NetGraph;
use cp_terrain::{SimTerrain, Terrain, TileOwnership};
use cp_zone::{BLOCK_DEPTH, ZoneCellRef, ZoneGrid};
use serde::{Deserialize, Serialize};

use crate::grid::BuildingGrid;
use crate::model::{AccessPoint, Building, BuildingFlags, BuildingId, BuildingState, Lot};
use crate::spawn::place;

/// Commands handled by this crate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum BuildingCommand {
    /// Places a service building. `pos` picks the lot: the zone-block column under it is
    /// the middle column of the lot, and the lot starts at the road edge. `rot` (radians) is
    /// snapped to quarter turns; a quarter turn swaps the lot's sides (along the road ×
    /// deep).
    Place {
        def: BuildingDefId,
        pos: Vec2,
        rot: f32,
    },
    /// Removes a building and frees its cells.
    Demolish { building: BuildingId },
}

/// Validation failure of a BuildingCommand; wrapped into `CommandError::Rejected` by the dispatcher.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildingError {
    /// A resource the command needs is missing from the world.
    Missing,
    UnknownDef,
    /// Zoned buildings spawn by themselves; only service buildings are placed.
    NotPlaceable,
    /// No road side next to the position.
    NoRoad,
    /// The lot does not fit the block: it runs past the end of the road or is too deep.
    DoesNotFit,
    /// A road, a building or another zone block covers part of the footprint.
    Blocked,
    /// Part of the footprint is on a tile the player does not own.
    NotOwned,
    Water,
    /// The type must stand on water (a pump) or on the shore (an outflow).
    NeedsWater,
    LimitReached,
    UnknownBuilding,
}

impl fmt::Display for BuildingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Missing => "no map or content in the sim world",
            Self::UnknownDef => "unknown building type",
            Self::NotPlaceable => "this building cannot be placed by hand",
            Self::NoRoad => "must be next to a road",
            Self::DoesNotFit => "does not fit along the road",
            Self::Blocked => "blocked by a road or another building",
            Self::NotOwned => "tile not owned",
            Self::Water => "cannot build on water",
            Self::NeedsWater => "must be built on or beside water",
            Self::LimitReached => "building limit reached",
            Self::UnknownBuilding => "no such building",
        })
    }
}

impl std::error::Error for BuildingError {}

/// A validated placement.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacementPlan {
    pub lot: Lot,
    /// Lot centre and road-direction yaw, as spawned buildings have them.
    pub pos: Vec2,
    pub rot: f32,
    pub access: AccessPoint,
    pub cost: Cost,
}

impl PlacementPlan {
    pub fn cells(&self) -> Vec<ZoneCellRef> {
        self.lot.cells().collect()
    }
}

/// The lot size `rot` gives a def: `(columns along the road, rows deep)`.
pub fn oriented_lot(def: &BuildingDef, rot: f32) -> (u8, u8) {
    let quarter = (rot / FRAC_PI_2).round() as i64;
    if quarter.rem_euclid(2) == 1 {
        (def.lot.1, def.lot.0)
    } else {
        def.lot
    }
}

/// The pure placement check. `terrain` and `tiles` are skipped when `None` (bare test
/// worlds); the sim always has both.
#[allow(clippy::too_many_arguments)]
pub fn validate_placement(
    db: &ContentDb,
    zones: &ZoneGrid,
    net: &NetGraph,
    terrain: Option<&Terrain>,
    tiles: Option<&TileOwnership>,
    def: BuildingDefId,
    pos: Vec2,
    rot: f32,
) -> Result<PlacementPlan, BuildingError> {
    let def = db
        .buildings()
        .find(|(id, _)| *id == def)
        .map(|(_, d)| d)
        .ok_or(BuildingError::UnknownDef)?;
    if def.service.is_none() {
        return Err(BuildingError::NotPlaceable);
    }
    let (w, d) = oriented_lot(def, rot);
    let under = zones.cell_at(pos).ok_or(BuildingError::NoRoad)?;
    let block = zones.block(under.block).ok_or(BuildingError::NoRoad)?;
    let mid = under.cell / BLOCK_DEPTH;
    let col = mid.saturating_sub(u16::from(w.saturating_sub(1)) / 2);
    if w == 0 || d == 0 || col + u16::from(w) > block.cols || u16::from(d) > BLOCK_DEPTH {
        return Err(BuildingError::DoesNotFit);
    }
    let lot = Lot {
        block: under.block,
        col,
        w,
        d,
    };
    let mut centres = Vec::new();
    for cell in lot.cells() {
        if zones.is_occupied(cell) || !zones.is_valid(cell) {
            return Err(BuildingError::Blocked);
        }
        centres.push(zones.cell_footprint(cell).ok_or(BuildingError::Blocked)?.c);
    }
    if let Some(tiles) = tiles
        && !centres.iter().all(|&c| tiles.contains_point(c))
    {
        return Err(BuildingError::NotOwned);
    }
    let site = def
        .service
        .as_deref()
        .and_then(|s| db.service_id(s))
        .map_or(ServiceSite::Land, |s| db.service(s).site);
    let wet = |c: Vec2| terrain.is_some_and(|t| t.is_water(c));
    match site {
        ServiceSite::Land => {
            if centres.iter().any(|&c| wet(c)) {
                return Err(BuildingError::Water);
            }
        }
        ServiceSite::OnWater => {
            if !centres.iter().any(|&c| wet(c)) {
                return Err(BuildingError::NeedsWater);
            }
        }
        ServiceSite::Shore => {
            if centres.iter().any(|&c| wet(c)) {
                return Err(BuildingError::Water);
            }
            let r = ServiceSite::SHORE_REACH_M;
            let near = centres.iter().any(|&c| {
                [(r, 0.0), (-r, 0.0), (0.0, r), (0.0, -r)]
                    .iter()
                    .any(|&(dx, dz)| wet(c + Vec2::new(dx, dz)))
            });
            if !near {
                return Err(BuildingError::NeedsWater);
            }
        }
    }
    let (pos, rot, access) = place(zones, net, under.block, &lot).ok_or(BuildingError::NoRoad)?;
    Ok(PlacementPlan {
        lot,
        pos,
        rot,
        access,
        cost: Cost::ZERO,
    })
}

/// Validates and applies `cmd`, returning its cost.
pub fn apply(world: &mut World, cmd: &BuildingCommand) -> Result<Cost, BuildingError> {
    match cmd {
        BuildingCommand::Place { def, pos, rot } => {
            let tick = world.get_resource::<Tick>().copied().unwrap_or_default();
            let limit = world
                .get_resource::<SimLimits>()
                .map_or(u32::MAX, |l| l.buildings);
            let (plan, level) = {
                let (Some(content), Some(zones), Some(net)) = (
                    world.get_resource::<SimContent>(),
                    world.get_resource::<ZoneGrid>(),
                    world.get_resource::<NetGraph>(),
                ) else {
                    return Err(BuildingError::Missing);
                };
                let plan = validate_placement(
                    &content.0,
                    zones,
                    net,
                    world.get_resource::<SimTerrain>().map(|t| &*t.0),
                    world.get_resource::<TileOwnership>(),
                    *def,
                    *pos,
                    *rot,
                )?;
                (plan, content.0.building(*def).level)
            };
            let mut buildings = world
                .get_resource_mut::<BuildingGrid>()
                .ok_or(BuildingError::Missing)?;
            if buildings.len() >= limit as usize {
                return Err(BuildingError::LimitReached);
            }
            buildings.insert(Building {
                def: *def,
                lot: plan.lot,
                level,
                state: BuildingState::UnderConstruction,
                ready_at: Tick(tick.0 + TICKS_PER_DAY),
                pos: plan.pos,
                rot: plan.rot,
                access: plan.access,
                flags: BuildingFlags::default(),
                level_progress: 0.0,
            });
            world.resource_mut::<ZoneGrid>().occupy(&plan.cells());
            Ok(plan.cost)
        }
        BuildingCommand::Demolish { building } => {
            let mut buildings = world
                .get_resource_mut::<BuildingGrid>()
                .ok_or(BuildingError::Missing)?;
            let Some(b) = buildings.remove(*building) else {
                return Err(BuildingError::UnknownBuilding);
            };
            let cells: Vec<ZoneCellRef> = b.lot.cells().collect();
            world.resource_mut::<ZoneGrid>().release(&cells);
            Ok(Cost::ZERO)
        }
    }
}
