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
//! commits with. The price is `BuildingDef::cost` ([`Cost::from_units`]); `apply` checks the
//! treasury before placing and charges it after ([`BuildingError::Funds`]).
//!
//! A service may be placed over zoned (growable) buildings (T-252): [`validate_placement_over`]
//! treats their cells and rectangles as free and names them, and `Place` demolishes them the way
//! the bulldozer does (no refund) before it places the service. The zone cells keep their type.
//! Another service, a road, water or a tile not owned still blocks.

use std::f32::consts::FRAC_PI_2;
use std::fmt;

use bevy_ecs::world::World;
use bevy_math::Vec2;
use cp_content::{BuildingDef, BuildingDefId, ContentDb, ServiceSite, SimContent};
use cp_core::treasury::{charge_funds, check_funds};
use cp_core::{Cost, InsufficientFunds, SimLimits, Tick};
use cp_net::{
    ElevationMode, NetGraph, RoadBuildPlan, RoadPath, RoadPathRequest, SegmentId, SnapOptions,
};
use cp_terrain::{SimTerrain, Terrain, TileOwnership};
use cp_zone::{BLOCK_DEPTH, ZoneCellRef, ZoneGrid};
use serde::{Deserialize, Serialize};

use crate::free::{is_free_standing, validate_free_placement_except};
use crate::grid::BuildingGrid;
use crate::model::{
    AccessPoint, Building, BuildingFlags, BuildingId, BuildingState, Footprint, Lot, build_ticks,
};
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
    /// Moves a service building to a new spot under the placement rules of its type (the
    /// building's own cells and rectangle are not obstacles). The same entity: its state,
    /// level and id stay, so everything keyed by the id (facility stock, workers) carries
    /// over. Costs [`MOVE_COST_FRACTION`] of the price. Zoned buildings do not move.
    Move {
        building: BuildingId,
        pos: Vec2,
        rot: f32,
    },
}

/// Share of a building's price a move costs. Free for now; tune later (T-177).
pub const MOVE_COST_FRACTION: f32 = 0.0;

/// Validation failure of a BuildingCommand; wrapped into `CommandError::Rejected` by the dispatcher.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildingError {
    /// The treasury cannot pay for the building.
    Funds(InsufficientFunds),
    /// A resource the command needs is missing from the world.
    Missing,
    UnknownDef,
    /// The position or rotation is NaN, infinite or absurdly large.
    BadCoordinates,
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
    /// The rectangle of a free-standing building is closer than 1 m to a road (§8.1).
    Road,
    /// The ground under a free-standing building is steeper than `MAX_CELL_SLOPE`.
    TooSteep,
    /// The platform under a station cannot be built (T-162); says why.
    Platform(String),
    /// A station is not moved: its platform stays where it is (T-162).
    Fixed,
    /// A unique building stands in the city already (T-200): one per city.
    UniqueBuilt,
    /// A chain building is not in an industrial area of its resource (T-208).
    NotInArea(Option<cp_content::ResourceKind>),
    /// The area has no main building yet.
    NoMain,
    /// The area has its main building.
    SecondMain,
    /// The area is below the level the building needs.
    AreaLevel {
        need: u8,
        have: u8,
    },
    /// The resource under an extractor is too poor (percent).
    Poor {
        pct: u8,
        need_pct: u8,
    },
    /// A park attraction or prop is not in a park area (of its kind) (T-223).
    NotInPark(Option<cp_content::ParkKind>),
    /// A park gate does not touch an area of its kind.
    GateAway(cp_content::ParkKind),
}

impl BuildingError {
    /// The shortfall, if that is why the command failed.
    pub fn insufficient_funds(&self) -> Option<InsufficientFunds> {
        match self {
            Self::Funds(f) => Some(*f),
            _ => None,
        }
    }
}

impl fmt::Display for BuildingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Self::Funds(e) = self {
            return write!(
                f,
                "insufficient funds: need {}, can spend {}",
                e.needed.0, e.available.0
            );
        }
        f.write_str(match self {
            Self::Funds(_) => unreachable!(),
            Self::Missing => "no map or content in the sim world",
            Self::UnknownDef => "unknown building type",
            Self::BadCoordinates => "invalid position or rotation",
            Self::NotPlaceable => "this building cannot be placed by hand",
            Self::NoRoad => "must be next to a road",
            Self::DoesNotFit => "does not fit along the road",
            Self::Blocked => "blocked by a road or another building",
            Self::NotOwned => "tile not owned",
            Self::Water => "cannot build on water",
            Self::NeedsWater => "must be built on or beside water",
            Self::LimitReached => "building limit reached",
            Self::UnknownBuilding => "no such building",
            Self::Road => "too close to a road",
            Self::TooSteep => "ground too steep",
            Self::Platform(why) => return write!(f, "no room for the platform: {why}"),
            Self::Fixed => "a station cannot be moved",
            Self::UniqueBuilt => "this unique building already stands in the city",
            Self::NotInArea(Some(r)) => {
                return write!(f, "must stand in a {r:?} industry area");
            }
            Self::NotInArea(None) => "must stand in an industry area",
            Self::NoMain => "the area has no main building yet",
            Self::SecondMain => "the area has its main building already",
            Self::AreaLevel { need, have } => {
                return write!(f, "needs area level {need} (it is {have})");
            }
            Self::NotInPark(Some(k)) => {
                return write!(f, "must stand inside a {k:?} park area");
            }
            Self::NotInPark(None) => "must stand inside a park area",
            Self::GateAway(k) => {
                return write!(f, "the gate must touch a {k:?} park area");
            }
            Self::Poor { pct, need_pct } => {
                return write!(f, "only {pct}% of the resource here, needs {need_pct}%");
            }
        })
    }
}

impl std::error::Error for BuildingError {}

/// A validated placement.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacementPlan {
    pub footprint: Footprint,
    /// Lot centre and road-direction yaw, as spawned buildings have them; the rectangle's
    /// centre and own yaw for a free-standing one.
    pub pos: Vec2,
    pub rot: f32,
    /// `None` for a free-standing building.
    pub access: Option<AccessPoint>,
    pub cost: Cost,
}

impl PlacementPlan {
    /// The zone cells the building covers (none when it stands free).
    pub fn cells(&self) -> Vec<ZoneCellRef> {
        match &self.footprint {
            Footprint::Lot(lot) => lot.cells().collect(),
            Footprint::Free { .. } => Vec::new(),
        }
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
    validate_placement_except(db, zones, net, terrain, tiles, def, pos, rot, &[])
}

/// [`validate_placement`] for moving a building: the cells in `ignore` (the ones it holds now)
/// count as free (T-177).
#[allow(clippy::too_many_arguments)]
pub fn validate_placement_except(
    db: &ContentDb,
    zones: &ZoneGrid,
    net: &NetGraph,
    terrain: Option<&Terrain>,
    tiles: Option<&TileOwnership>,
    def: BuildingDefId,
    pos: Vec2,
    rot: f32,
    ignore: &[ZoneCellRef],
) -> Result<PlacementPlan, BuildingError> {
    if !cp_core::sane_point(pos) || !rot.is_finite() {
        return Err(BuildingError::BadCoordinates);
    }
    let def = db
        .buildings()
        .find(|(id, _)| *id == def)
        .map(|(_, d)| d)
        .ok_or(BuildingError::UnknownDef)?;
    let Some(service) = def.service.as_deref() else {
        return Err(BuildingError::NotPlaceable);
    };
    if db
        .service_id(service)
        .is_some_and(|s| !db.service(s).road_access)
    {
        // Free-standing types are placed by `validate_free_placement`.
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
    let site = def
        .service
        .as_deref()
        .and_then(|s| db.service_id(s))
        .map_or(ServiceSite::Land, |s| db.service(s).site);
    let wet = |c: Vec2| terrain.is_some_and(|t| t.is_water(c));
    let mut centres = Vec::new();
    for cell in lot.cells() {
        let centre = zones.cell_footprint(cell).ok_or(BuildingError::Blocked)?.c;
        let own = ignore.contains(&cell);
        if zones.is_occupied(cell) && !own {
            return Err(BuildingError::Blocked);
        }
        // Cells over water are invalid for zoning (T-062). A building that stands on the
        // water (a pump) may cover them; anything else is told it is water, not "blocked".
        if !own && !zones.is_valid(cell) {
            match (wet(centre), site) {
                (true, ServiceSite::OnWater) => {}
                (true, _) => return Err(BuildingError::Water),
                (false, _) => return Err(BuildingError::Blocked),
            }
        }
        centres.push(centre);
    }
    if let Some(tiles) = tiles
        && !centres.iter().all(|&c| tiles.contains_point(c))
    {
        return Err(BuildingError::NotOwned);
    }
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
        footprint: Footprint::Lot(lot),
        pos,
        rot,
        access: Some(access),
        cost: Cost::from_units(def.cost),
    })
}

/// Whether buildings of `def` grow on their zone by themselves (a house, a shop, an office, a
/// factory): the player never places them, and a service may be placed over them (T-252).
pub fn is_growable(def: &BuildingDef) -> bool {
    def.service.is_none() && def.unique.is_none() && def.industry.is_none() && def.park.is_none()
}

/// How far from the cursor zoned buildings are looked at for [`validate_placement_over`],
/// metres: more than half the diagonal of any lot or rectangle plus a lot.
const OVER_REACH_M: f32 = 96.0;

/// A placement over zoned buildings (T-252): the plan and the zoned buildings it demolishes, in
/// id order.
#[derive(Clone, Debug, PartialEq)]
pub struct Clearance {
    pub plan: PlacementPlan,
    pub demolish: Vec<BuildingId>,
}

/// The placement check the tool and `Place` use (T-252): [`validate_placement`] (or
/// [`validate_free_placement`] for a free-standing def) where the zoned buildings near `pos` are
/// no obstacle, plus the ones the building would stand on. Everything else (another service,
/// a road, water, a tile not owned) blocks as before.
#[allow(clippy::too_many_arguments)]
pub fn validate_placement_over(
    db: &ContentDb,
    zones: &ZoneGrid,
    net: &NetGraph,
    buildings: &BuildingGrid,
    terrain: Option<&Terrain>,
    tiles: Option<&TileOwnership>,
    def: BuildingDefId,
    pos: Vec2,
    rot: f32,
) -> Result<Clearance, BuildingError> {
    let near: Vec<(BuildingId, Vec<ZoneCellRef>)> = buildings
        .iter()
        .filter(|(_, b)| {
            b.pos.distance(pos) <= OVER_REACH_M
                && db
                    .buildings()
                    .find(|(id, _)| *id == b.def)
                    .is_some_and(|(_, d)| is_growable(d))
        })
        .map(|(id, b)| (id, crate::grid::held_cells(zones, b)))
        .collect();
    let cells: Vec<ZoneCellRef> = near.iter().flat_map(|(_, c)| c.iter().copied()).collect();
    let ids: Vec<BuildingId> = near.iter().map(|(id, _)| *id).collect();
    let plan = if is_free_standing(db, def) {
        crate::free::validate_free_placement_ignoring(
            db, zones, net, buildings, terrain, tiles, def, pos, rot, &ids, &cells,
        )?
    } else {
        validate_placement_except(db, zones, net, terrain, tiles, def, pos, rot, &cells)?
    };
    let demolish = match plan.footprint {
        Footprint::Lot(_) => {
            let mine = plan.cells();
            near.iter()
                .filter(|(_, held)| held.iter().any(|c| mine.contains(c)))
                .map(|(id, _)| *id)
                .collect()
        }
        Footprint::Free { w, d } => {
            let quad = crate::model::footprint_quad(plan.pos, plan.rot, w, d);
            near.iter()
                .filter(|(id, held)| {
                    buildings.get(*id).is_some_and(|b| {
                        cp_zone::geom::quads_overlap(&quad, &b.footprint_polygon())
                    }) || held.iter().any(|c| {
                        zones
                            .cell_footprint(*c)
                            .is_some_and(|o| cp_zone::geom::quads_overlap(&quad, &o.corners()))
                    })
                })
                .map(|(id, _)| *id)
                .collect()
        }
    };
    Ok(Clearance { plan, demolish })
}

/// The chain-building rule (T-208) on a plan that passed the geometry checks; a no-op for every
/// other def. The sim world needs the areas and the buildings for it.
fn check_industry(
    world: &World,
    db: &ContentDb,
    zones: &ZoneGrid,
    def: BuildingDefId,
    plan: &PlacementPlan,
    ignore: Option<BuildingId>,
) -> Result<(), BuildingError> {
    if db.building(def).industry.is_none() {
        return Ok(());
    }
    let (Some(areas), Some(area_map), Some(buildings), Some(terrain)) = (
        world.get_resource::<cp_district::Areas>(),
        world.get_resource::<cp_district::AreaMap>(),
        world.get_resource::<BuildingGrid>(),
        world.get_resource::<SimTerrain>(),
    ) else {
        return Err(BuildingError::Missing);
    };
    crate::industry::validate_industry_site(
        &crate::industry::IndustryWorld {
            db,
            areas,
            area_map,
            resources: terrain.0.resources(),
            buildings,
            zones,
        },
        def,
        plan,
        ignore,
    )
}

/// The park-building rule (T-223) on a plan that passed the geometry checks; a no-op for every
/// other def.
fn check_park(
    world: &World,
    db: &ContentDb,
    zones: &ZoneGrid,
    def: BuildingDefId,
    plan: &PlacementPlan,
) -> Result<(), BuildingError> {
    if db.building(def).park.is_none() {
        return Ok(());
    }
    let (Some(areas), Some(area_map)) = (
        world.get_resource::<cp_district::Areas>(),
        world.get_resource::<cp_district::AreaMap>(),
    ) else {
        return Err(BuildingError::Missing);
    };
    crate::park_site::validate_park_site(
        &crate::park_site::ParkWorld {
            db,
            areas,
            area_map,
            zones,
        },
        def,
        plan,
    )
}

/// What a `Place` made: the building, for a station the segment of its platform, and the zoned
/// buildings it demolished with where they stood (T-252), in id order.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub building: BuildingId,
    pub platform: Option<SegmentId>,
    pub demolished: Vec<(BuildingId, Vec2)>,
}

/// The platform under a station of `def` standing at `pos` turned by `rot`: a request for one
/// segment along the building's axis, centred on it (T-162). `None` for a def without track.
pub fn platform_request(
    db: &ContentDb,
    def: &BuildingDef,
    pos: Vec2,
    rot: f32,
) -> Option<RoadPathRequest> {
    platform_request_behind(db, def, pos, rot, Vec2::ZERO)
}

/// [`platform_request`] with the platform moved by `shift` (a platform behind the building,
/// see [`platform_shift`]).
pub fn platform_request_behind(
    db: &ContentDb,
    def: &BuildingDef,
    pos: Vec2,
    rot: f32,
    shift: Vec2,
) -> Option<RoadPathRequest> {
    let track = def.track.as_ref()?;
    let road = db.road_id(&track.road)?;
    let half = Vec2::new(rot.cos(), rot.sin()) * (track.length_m * 0.5);
    let pos = pos + shift;
    Some(RoadPathRequest {
        road,
        path: RoadPath::Straight {
            from: pos - half,
            to: pos + half,
        },
        elevation: if track.depth > 0.0 {
            ElevationMode::Tunnel(track.depth)
        } else {
            ElevationMode::Ground
        },
        // Only the ends snap, and onto nodes: the platform stays where the building is.
        snap: SnapOptions {
            node: true,
            segment: false,
            angle: false,
            zone_grid: false,
        },
    })
}

/// How far a ground platform stands from the centre of its building (BUG-046): back from the
/// street, past the back edge of the lot and `behind_m` more, along the lot's depth axis (the
/// cells' `v`, which points away from the road). Zero for a platform under the building or a
/// building that stands free.
pub fn platform_shift(world: &World, def: &BuildingDef, plan: &PlacementPlan) -> Vec2 {
    let (Some(behind), Footprint::Lot(lot), Some(zones)) = (
        def.track.as_ref().and_then(|t| t.behind_m),
        &plan.footprint,
        world.get_resource::<ZoneGrid>(),
    ) else {
        return Vec2::ZERO;
    };
    let Some(cell) = zones.cell_footprint(cp_zone::ZoneCellRef {
        block: lot.block,
        cell: lot.col * cp_zone::BLOCK_DEPTH,
    }) else {
        return Vec2::ZERO;
    };
    let depth = f32::from(lot.d) * 2.0 * cell.half_v;
    cell.v * (depth * 0.5 + behind)
}

/// Validates the platform of a station: no junction in it (a long ground platform is cut in a
/// few segments, the first is the stop's).
fn plan_platform(world: &World, req: &RoadPathRequest) -> Result<RoadBuildPlan, BuildingError> {
    let plan =
        cp_net::plan_build(world, req).map_err(|e| BuildingError::Platform(e.to_string()))?;
    if plan.segments.is_empty() || !plan.splits.is_empty() {
        return Err(BuildingError::Platform(
            "it would cross another tunnel on the same depth".to_string(),
        ));
    }
    Ok(plan)
}

/// Validates and applies `cmd`, returning its cost.
pub fn apply(world: &mut World, cmd: &BuildingCommand) -> Result<Cost, BuildingError> {
    apply_placed(world, cmd).map(|(cost, _)| cost)
}

/// [`apply`], and what a `Place` made. The station's platform and the building are one step:
/// both are checked before the first change, and the platform is applied from its plan.
pub fn apply_placed(
    world: &mut World,
    cmd: &BuildingCommand,
) -> Result<(Cost, Option<Placed>), BuildingError> {
    match cmd {
        BuildingCommand::Place { def, pos, rot } => {
            let tick = world.get_resource::<Tick>().copied().unwrap_or_default();
            let limit = world
                .get_resource::<SimLimits>()
                .map_or(u32::MAX, |l| l.buildings);
            let (plan, demolish, level, build, is_service) = {
                let (Some(content), Some(zones), Some(net)) = (
                    world.get_resource::<SimContent>(),
                    world.get_resource::<ZoneGrid>(),
                    world.get_resource::<NetGraph>(),
                ) else {
                    return Err(BuildingError::Missing);
                };
                // One of each unique building per city, whatever state the first is in.
                if content.0.building(*def).unique.is_some()
                    && world
                        .get_resource::<BuildingGrid>()
                        .is_some_and(|g| g.iter().any(|(_, b)| b.def == *def))
                {
                    return Err(BuildingError::UniqueBuilt);
                }
                let terrain = world.get_resource::<SimTerrain>().map(|t| &*t.0);
                let tiles = world.get_resource::<TileOwnership>();
                let buildings = world
                    .get_resource::<BuildingGrid>()
                    .ok_or(BuildingError::Missing)?;
                let Clearance { plan, demolish } = validate_placement_over(
                    &content.0, zones, net, buildings, terrain, tiles, *def, *pos, *rot,
                )?;
                check_industry(world, &content.0, zones, *def, &plan, None)?;
                check_park(world, &content.0, zones, *def, &plan)?;
                (
                    plan,
                    demolish,
                    content.0.building(*def).level,
                    build_ticks(content.0.building(*def)),
                    content.0.building(*def).service.is_some(),
                )
            };
            let platform = {
                let content = world.resource::<SimContent>();
                let def = content.0.building(*def);
                let shift = platform_shift(world, def, &plan);
                match platform_request_behind(&content.0, def, plan.pos, plan.rot, shift) {
                    Some(req) => Some(plan_platform(world, &req)?),
                    None => None,
                }
            };
            let total = platform
                .as_ref()
                .map_or(plan.cost, |p| Cost(plan.cost.0 + p.cost.0));
            check_funds(world, total).map_err(BuildingError::Funds)?;
            if world
                .get_resource::<BuildingGrid>()
                .ok_or(BuildingError::Missing)?
                .len()
                - demolish.len()
                >= limit as usize
            {
                return Err(BuildingError::LimitReached);
            }
            // The zoned buildings under it go first, as the bulldozer takes them (T-252).
            let mut demolished = Vec::with_capacity(demolish.len());
            world.resource_scope(|world, mut buildings: bevy_ecs::world::Mut<BuildingGrid>| {
                let mut zones = world.resource_mut::<ZoneGrid>();
                for id in &demolish {
                    let at = buildings.get(*id).map(|b| b.pos);
                    if crate::grid::demolish_building(&mut buildings, &mut zones, *id)
                        && let Some(at) = at
                    {
                        demolished.push((*id, at));
                    }
                }
            });
            let mut buildings = world.resource_mut::<BuildingGrid>();
            let building = buildings.insert(Building {
                def: *def,
                footprint: plan.footprint,
                level,
                // City services stand ready at once (T-175); zone houses keep their build time.
                state: if is_service {
                    BuildingState::Active
                } else {
                    BuildingState::UnderConstruction
                },
                ready_at: Tick(if is_service { tick.0 } else { tick.0 + build }),
                pos: plan.pos,
                rot: plan.rot,
                access: plan.access,
                flags: BuildingFlags::default(),
                level_progress: 0.0,
            });
            world.resource_mut::<ZoneGrid>().occupy(&plan.cells());
            // The plan was checked against this very graph: it applies.
            let platform = match platform {
                Some(p) => cp_net::apply_build_plan(world, &p)
                    .map_err(|e| BuildingError::Platform(e.to_string()))?
                    .first()
                    .copied(),
                None => None,
            };
            charge_funds(world, total);
            Ok((
                total,
                Some(Placed {
                    building,
                    platform,
                    demolished,
                }),
            ))
        }
        BuildingCommand::Move { building, pos, rot } => {
            let (plan, cost) = {
                let (Some(content), Some(zones), Some(net), Some(buildings)) = (
                    world.get_resource::<SimContent>(),
                    world.get_resource::<ZoneGrid>(),
                    world.get_resource::<NetGraph>(),
                    world.get_resource::<BuildingGrid>(),
                ) else {
                    return Err(BuildingError::Missing);
                };
                let b = buildings
                    .get(*building)
                    .ok_or(BuildingError::UnknownBuilding)?;
                let def = b.def;
                if content.0.building(def).track.is_some() {
                    return Err(BuildingError::Fixed);
                }
                let terrain = world.get_resource::<SimTerrain>().map(|t| &*t.0);
                let tiles = world.get_resource::<TileOwnership>();
                let plan = if is_free_standing(&content.0, def) {
                    validate_free_placement_except(
                        &content.0,
                        zones,
                        net,
                        buildings,
                        terrain,
                        tiles,
                        def,
                        *pos,
                        *rot,
                        Some(*building),
                    )?
                } else {
                    let held = crate::grid::held_cells(zones, b);
                    validate_placement_except(
                        &content.0, zones, net, terrain, tiles, def, *pos, *rot, &held,
                    )?
                };
                check_industry(world, &content.0, zones, def, &plan, Some(*building))?;
                check_park(world, &content.0, zones, def, &plan)?;
                let cost = Cost::from_units(content.0.building(def).cost * MOVE_COST_FRACTION);
                (plan, cost)
            };
            check_funds(world, cost).map_err(BuildingError::Funds)?;
            let old = {
                let zones = world.resource::<ZoneGrid>();
                let buildings = world.resource::<BuildingGrid>();
                crate::grid::held_cells(zones, &buildings.get(*building).unwrap().clone())
            };
            {
                let mut buildings = world.resource_mut::<BuildingGrid>();
                let b = buildings.get_mut(*building).unwrap();
                b.footprint = plan.footprint;
                b.pos = plan.pos;
                b.rot = plan.rot;
                b.access = plan.access;
            }
            let mut zones = world.resource_mut::<ZoneGrid>();
            zones.release(&old);
            zones.occupy(&plan.cells());
            charge_funds(world, cost);
            Ok((cost, None))
        }
        BuildingCommand::Demolish { building } => {
            let mut buildings = world
                .get_resource_mut::<BuildingGrid>()
                .ok_or(BuildingError::Missing)?;
            let Some(b) = buildings.remove(*building) else {
                return Err(BuildingError::UnknownBuilding);
            };
            let mut zones = world.resource_mut::<ZoneGrid>();
            let cells = crate::grid::held_cells(&zones, &b);
            zones.release(&cells);
            Ok((Cost::ZERO, None))
        }
    }
}
