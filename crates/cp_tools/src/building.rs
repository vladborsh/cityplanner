//! The service-building placement tool and the building half of the bulldozer.
//!
//! Pure: placement validity comes from `cp_building::validate_placement_over`, the same function
//! the `Place` command commits with (a service may stand on zoned buildings, which it demolishes,
//! T-252); the plugin turns clicks into `BuildingCommand`s.

use std::f32::consts::{FRAC_PI_2, TAU};

use bevy_math::Vec2;
use cp_building::{
    BuildingError, BuildingFlags, BuildingId, BuildingState, MOVE_COST_FRACTION, PlacementPlan,
    footprint_quad, is_free_standing, validate_placement_except, validate_placement_over,
};
use cp_content::{BuildingDefId, ContentDb, ServiceKind, ServiceSite};
use cp_core::Cost;
use cp_core::curve::to_xz;
use cp_locale::{tr, trf, trn};
use cp_sim::SimView;
use cp_utility::{UtilityNet, reach_m};
use cp_zone::ZoneCellRef;

/// Pick distance from a building's lot centre before the exact cell test, metres.
const PICK_REACH: f32 = 40.0;
/// `T` turns a free-standing ghost by this much, radians (15°).
pub const FREE_ROT_STEP: f32 = TAU / 24.0;
/// `Shift+T` looks for a road to align to within this distance of the ghost, metres.
pub const ALIGN_REACH_M: f32 = 60.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BuildingTool {
    pub def: Option<BuildingDefId>,
    /// Quarter turns, 0..4 (types that stand on a road).
    pub quarters: u8,
    /// Free turn of a free-standing type, radians in `[0, TAU)`.
    pub free_rot: f32,
    /// The placed building this ghost carries to a new spot (T-177); `None` when placing a
    /// new one. Its own cells and rectangle are no obstacle to it.
    pub moving: Option<BuildingId>,
}

impl BuildingTool {
    /// Picks `id` up: the ghost is a building of its type with its turn, and a click sends
    /// `Move` instead of `Place`. `false` (nothing changes) for a building that is gone or not
    /// a service building.
    pub fn start_move(&mut self, view: &SimView, id: BuildingId) -> bool {
        let Some((b, db)) = view.buildings.get(id).zip(view.content.as_deref()) else {
            return false;
        };
        if db.building(b.def).service.is_none() {
            return false;
        }
        *self = Self {
            def: Some(b.def),
            quarters: 0,
            free_rot: b.rot.rem_euclid(TAU),
            moving: Some(id),
        };
        true
    }

    /// T key for a type on a road: next quarter turn.
    pub fn rotate(&mut self) {
        self.quarters = (self.quarters + 1) % 4;
    }

    /// T key for a free-standing type: 15° further.
    pub fn rotate_free(&mut self) {
        self.free_rot = (self.free_rot + FREE_ROT_STEP).rem_euclid(TAU);
    }

    /// Shift+T for a free-standing type: turn to the direction of the nearest road within
    /// [`ALIGN_REACH_M`] of `pos`. Returns whether there was one.
    pub fn align_to_road(&mut self, view: &SimView, pos: Vec2) -> bool {
        let Some(angle) = road_angle(view, pos) else {
            return false;
        };
        self.free_rot = angle.rem_euclid(TAU);
        true
    }

    /// The T key (Shift+T with `align`), whichever way the selected type turns.
    pub fn turn(&mut self, view: Option<&SimView>, hover: Option<Vec2>, align: bool) {
        let free = self
            .def
            .zip(view.and_then(|v| v.content.as_deref()))
            .is_some_and(|(d, db)| is_free_standing(db, d));
        match (free, view, hover) {
            (true, Some(v), Some(p)) if align => {
                self.align_to_road(v, p);
            }
            (true, ..) if align => {}
            (true, ..) => self.rotate_free(),
            (false, ..) => self.rotate(),
        }
    }

    /// The quarter turn of a type on a road.
    pub fn rot(&self) -> f32 {
        f32::from(self.quarters) * FRAC_PI_2
    }

    /// The turn the command carries: the free angle for a free-standing type.
    pub fn rot_for(&self, free: bool) -> f32 {
        if free { self.free_rot } else { self.rot() }
    }
}

/// Angle (radians, XZ) of the nearest ground road within [`ALIGN_REACH_M`] of `pos`.
fn road_angle(view: &SimView, pos: Vec2) -> Option<f32> {
    let mut best: Option<(f32, f32)> = None;
    for (_, seg) in view.net.segments() {
        if seg.elevation != cp_net::Elevation::Ground {
            continue;
        }
        let (t, d) = seg.curve.nearest_xz(pos);
        if d <= ALIGN_REACH_M && best.is_none_or(|(bd, _)| d < bd) {
            let tan = to_xz(seg.curve.tangent(t));
            best = Some((d, tan.y.atan2(tan.x)));
        }
    }
    best.map(|(_, a)| a)
}

/// The ghost's reason for a refused placement, in the player's words: the water errors say
/// what the type needs (`site`), the rest is the command's own text.
pub fn error_text(e: &BuildingError, site: ServiceSite) -> String {
    match (e, site) {
        (BuildingError::NeedsWater, ServiceSite::OnWater) => {
            tr("needs water under the lot: put at least one cell over water")
        }
        (BuildingError::NeedsWater, _) => trf!(
            "needs a shore: the lot on land with water within {reach} m",
            reach = format!("{:.0}", ServiceSite::SHORE_REACH_M)
        ),
        (BuildingError::Water, _) => tr("water here: this building goes on land only"),
        (BuildingError::Road, _) => tr("a road is in the way"),
        (BuildingError::TooSteep, _) => tr("ground too steep: 25 % at most"),
        _ => building_error_text(e),
    }
}

/// The command's own text of `e` in the player's language (the English is `e`'s `Display`).
/// The texts naming a resource or park kind stay English.
pub fn building_error_text(e: &BuildingError) -> String {
    match e {
        BuildingError::Funds(f) => trf!(
            "insufficient funds: need {need}, can spend {have}",
            need = f.needed.0,
            have = f.available.0
        ),
        BuildingError::Platform(why) => {
            trf!("no room for the platform: {why}", why = tr(why))
        }
        BuildingError::AreaLevel { need, have } => trf!(
            "needs area level {need} (it is {have})",
            need = need,
            have = have
        ),
        BuildingError::Poor { pct, need_pct } => trf!(
            "only {pct}% of the resource here, needs {need_pct}%",
            pct = pct,
            need_pct = need_pct
        ),
        BuildingError::NotInArea(Some(_))
        | BuildingError::NotInPark(Some(_))
        | BuildingError::GateAway(_) => e.to_string(),
        // The rest are fixed texts: the text is its own key.
        _ => tr(&e.to_string()),
    }
}

/// What the cursor would place.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PlacementGhost {
    /// The lot's cells when valid; the cell under the cursor when not (to draw something red).
    pub cells: Vec<ZoneCellRef>,
    pub valid: bool,
    pub cost: Option<Cost>,
    pub error: Option<String>,
    /// The rectangle of a free-standing type (§8.1); `None` for a type on a road.
    pub free: Option<FreeGhost>,
    /// For an extractor (T-211): the average richness of the lot under the cursor and the least
    /// the rules ask for, in percent. The rule itself is `cp_building::validate_industry_site`;
    /// both use `cp_building::lot_richness`.
    pub richness: Option<(u8, u8)>,
    /// Where a valid ghost on a road would join it: the same access point the placement gives
    /// the building (`PlacementPlan::access`); the coverage preview starts there (T-254).
    pub access: Option<cp_building::AccessPoint>,
    /// The zoned buildings the placement would demolish, in id order (T-252); drawn red.
    pub demolish: Vec<BuildingId>,
    /// Where the blocker is when the placement is refused (T-267): the tool draws a red cross
    /// there.
    pub error_at: Option<Vec2>,
    /// `(residents, jobs)` in [`Self::demolish`]: the app fills it from the sim (the view has no
    /// citizens), `None` until then.
    pub evicted: Option<(u32, u32)>,
}

impl PlacementGhost {
    /// "Demolishes N buildings (M residents evicted, K jobs)", or `None` when nothing is in the way.
    pub fn demolish_text(&self) -> Option<String> {
        let n = self.demolish.len();
        if n == 0 {
            return None;
        }
        Some(match self.evicted {
            Some((residents, jobs)) => trf!(
                plural "Demolishes {n} buildings ({residents} residents evicted, {jobs} jobs)",
                n = n,
                residents = residents,
                jobs = jobs
            ),
            None => trn("Demolishes {n} buildings", n as i64),
        })
    }
}

/// The ghost of a free-standing building: not tied to cells, so it carries its own pose.
#[derive(Clone, Debug, PartialEq)]
pub struct FreeGhost {
    /// The cursor rounded to 1 m: where `Place` puts the building.
    pub pos: Vec2,
    pub rot: f32,
    /// The rectangle, counter-clockwise.
    pub polygon: [Vec2; 4],
    /// Whether the network the type needs is in reach.
    pub hint: Option<ConnectionHint>,
    /// A service at the road edge (T-267): `Place` is sent the cursor and the quarter turns, not
    /// `pos` and `rot` (they are where the placement put the rectangle, which it works out again).
    pub edge: bool,
}

/// Where a free-standing building would connect (§8.1, §10).
#[derive(Clone, Debug, PartialEq)]
pub struct ConnectionHint {
    pub net: UtilityNet,
    /// The point of the pipe, line or powered building it would connect to, when in reach.
    pub target: Option<Vec2>,
    pub connects: bool,
    /// The reach of `net` (`balance.ron` `networks`), the radius of the circle the tool draws.
    pub reach: f32,
    pub text: String,
}

/// Which network a type of this service kind hooks into, and the words for it.
fn network_of(kind: ServiceKind) -> Option<UtilityNet> {
    match kind {
        ServiceKind::Water | ServiceKind::Sewage => Some(UtilityNet::Pipe),
        ServiceKind::Power => Some(UtilityNet::Power),
        _ => None,
    }
}

/// Whether the building of `def` placed at `pos` would connect to the network of its service:
/// a pipe or power line within its reach, or (power) a supplied building within
/// `power_gap_m` (edge to edge) -- both from `balance.ron` `networks`. `None` for a type with no network. The view's content must know `def`.
pub fn connection_hint(view: &SimView, def: BuildingDefId, pos: Vec2) -> Option<ConnectionHint> {
    let db = view.content.as_deref()?;
    let service = db
        .building(def)
        .service
        .as_deref()
        .and_then(|s| db.service_id(s))
        .map(|s| db.service(s))?;
    let net = network_of(service.kind)?;
    let half = |size: (u8, u8)| f32::from(size.0.max(size.1)) * cp_building::FOOTPRINT_CELL_M * 0.5;
    let my_half = half(db.building(def).lot);
    let nets = &db.balance().networks;
    let reach = reach_m(nets, net);
    let line = view.utilities.nearest_link(net, pos, reach + my_half);
    let mut target =
        line.and_then(|(id, t, _)| view.utilities.link(id).map(|l| to_xz(l.curve.point(t))));
    let mut what = match net {
        UtilityNet::Pipe => "will connect to the pipe",
        UtilityNet::Power => "will connect to the power line",
    };
    if target.is_none() && net == UtilityNet::Power {
        // A supplied building passes the current on (edge to edge).
        target = view
            .buildings
            .iter()
            .filter(|(_, b)| {
                b.state == BuildingState::Active && !b.flags.has(BuildingFlags::NO_POWER)
            })
            .filter(|(_, b)| b.pos.distance(pos) - my_half - half(b.size()) <= nets.power_gap_m)
            .map(|(_, b)| b.pos)
            .min_by(|a, b| a.distance(pos).total_cmp(&b.distance(pos)));
        what = "will connect to a powered building";
    }
    let text = match (target.is_some(), net) {
        (true, _) => tr(what),
        (false, UtilityNet::Pipe) => trf!(
            "no pipe within {reach} m — lay a pipe",
            reach = format!("{reach:.0}")
        ),
        (false, UtilityNet::Power) => trf!(
            "no power line within {reach} m, no powered building within {gap} m — build a line",
            reach = format!("{reach:.0}"),
            gap = format!("{:.0}", nets.power_gap_m)
        ),
    };
    Some(ConnectionHint {
        net,
        target,
        connects: target.is_some(),
        reach,
        text,
    })
}

/// What the ghost shows as the price: the placement price, or for a move its share
/// ([`MOVE_COST_FRACTION`]).
fn price(db: &ContentDb, def: BuildingDefId, tool: &BuildingTool, placing: Cost) -> Cost {
    if tool.moving.is_some() {
        Cost::from_units(db.building(def).cost * MOVE_COST_FRACTION)
    } else {
        placing
    }
}

/// The ghost for `tool` with the cursor at `hover`; `None` without a selected type or content.
pub fn preview(view: &SimView, tool: &BuildingTool, hover: Vec2) -> Option<PlacementGhost> {
    let def = tool.def?;
    let db = view.content.as_deref()?;
    if is_free_standing(db, def) {
        return Some(free_preview(view, db, tool, def, hover));
    }
    if cp_building::is_edge_placed(db, def) {
        return Some(edge_preview(view, db, tool, def, hover));
    }
    let held = tool
        .moving
        .and_then(|id| view.buildings.get(id))
        .map(|b| cp_building::held_cells(&view.zones, b))
        .unwrap_or_default();
    let richness = lot_richness_percent(view, db, def, hover, tool.rot(), &held);
    let mut demolish = Vec::new();
    Some(
        match place_over(view, db, tool, def, hover, tool.rot(), &held).and_then(|(plan, over)| {
            demolish = over;
            // A chain building also needs its industrial area (T-208).
            if let Some(terrain) = view.terrain.as_deref() {
                cp_building::validate_industry_site(
                    &cp_building::IndustryWorld {
                        db,
                        areas: &view.areas,
                        area_map: &view.area_map,
                        resources: terrain.resources(),
                        buildings: &view.buildings,
                        zones: &view.zones,
                    },
                    def,
                    &plan,
                    tool.moving,
                )?;
            }
            // A park building also needs its park area (T-223).
            cp_building::validate_park_site(
                &cp_building::ParkWorld {
                    db,
                    areas: &view.areas,
                    area_map: &view.area_map,
                    zones: &view.zones,
                },
                def,
                &plan,
            )?;
            Ok(plan)
        }) {
            Ok(plan) => PlacementGhost {
                cells: plan.cells(),
                valid: true,
                cost: Some(price(db, def, tool, plan.cost)),
                richness,
                access: plan.access,
                demolish,
                ..Default::default()
            },
            Err(e) => PlacementGhost {
                cells: view.zones.cell_at(hover).into_iter().collect(),
                valid: false,
                cost: None,
                free: None,
                richness,
                access: None,
                demolish: Vec::new(),
                evicted: None,
                error_at: None,
                error: Some(error_text(
                    &e,
                    db.building(def)
                        .service
                        .as_deref()
                        .and_then(|s| db.service_id(s))
                        .map_or(ServiceSite::Land, |s| db.service(s).site),
                )),
            },
        },
    )
}

/// The placement check for a new building ([`validate_placement_over`]: zoned buildings are no
/// obstacle, they are named) or for a building being moved (its own cells are free, zoned
/// buildings still block).
fn place_over(
    view: &SimView,
    db: &ContentDb,
    tool: &BuildingTool,
    def: BuildingDefId,
    pos: Vec2,
    rot: f32,
    held: &[ZoneCellRef],
) -> Result<(PlacementPlan, Vec<BuildingId>), BuildingError> {
    if tool.moving.is_some() {
        let ignore: Vec<BuildingId> = tool.moving.into_iter().collect();
        return cp_building::validate_placement_for(
            db,
            &view.zones,
            &view.net,
            &view.buildings,
            view.terrain.as_deref(),
            Some(&view.tiles),
            def,
            pos,
            rot,
            &ignore,
            held,
        )
        .map(|plan| (plan, Vec::new()));
    }
    validate_placement_over(
        db,
        &view.zones,
        &view.net,
        &view.buildings,
        view.terrain.as_deref(),
        Some(&view.tiles),
        def,
        pos,
        rot,
    )
    .map(|c| (c.plan, c.demolish))
}

/// The lot richness of an extractor under the cursor (T-211): `(percent, needed percent)`, from
/// the same function the placement rule uses. `None` for every other building.
fn lot_richness_percent(
    view: &SimView,
    db: &ContentDb,
    def: BuildingDefId,
    hover: Vec2,
    rot: f32,
    held: &[ZoneCellRef],
) -> Option<(u8, u8)> {
    let spec = db.building(def).industry.as_ref()?;
    if spec.role != cp_content::IndustryRole::Extractor {
        return None;
    }
    let res = spec.resource?;
    let terrain = view.terrain.as_deref()?;
    let plan = validate_placement_except(
        db,
        &view.zones,
        &view.net,
        Some(terrain),
        Some(&view.tiles),
        def,
        hover,
        rot,
        held,
    )
    .ok()?;
    let have = cp_building::lot_richness(
        terrain.resources(),
        res,
        &cp_building::plan_points(&view.zones, &plan),
    );
    Some((
        (have * 100.0).round() as u8,
        (db.balance().min_richness * 100.0).round() as u8,
    ))
}

/// The ghost of a free-standing type: the cursor rounded to 1 m, no cells.
fn free_preview(
    view: &SimView,
    db: &ContentDb,
    tool: &BuildingTool,
    def: BuildingDefId,
    hover: Vec2,
) -> PlacementGhost {
    let pos = hover.round();
    let rot = tool.rot_for(true);
    let (w, d) = db.building(def).lot;
    let free = FreeGhost {
        pos,
        rot,
        polygon: footprint_quad(pos, rot, w, d),
        hint: connection_hint(view, def, pos),
        edge: false,
    };
    let mut demolish = Vec::new();
    match place_over(view, db, tool, def, pos, rot, &[]).and_then(|(plan, over)| {
        demolish = over;
        // A park attraction or prop also needs its park area (T-223).
        cp_building::validate_park_site(
            &cp_building::ParkWorld {
                db,
                areas: &view.areas,
                area_map: &view.area_map,
                zones: &view.zones,
            },
            def,
            &plan,
        )?;
        Ok(plan)
    }) {
        Ok(plan) => PlacementGhost {
            valid: true,
            cost: Some(price(db, def, tool, plan.cost)),
            free: Some(free),
            demolish,
            ..Default::default()
        },
        Err(e) => PlacementGhost {
            error: Some(error_text(&e, ServiceSite::Land)),
            free: Some(free),
            ..Default::default()
        },
    }
}

/// The ghost of a service at the road edge (T-267): the rectangle where the placement puts it, its
/// access point on the road it faces, the zoned buildings it goes over. A refusal says what is in
/// the way.
fn edge_preview(
    view: &SimView,
    db: &ContentDb,
    tool: &BuildingTool,
    def: BuildingDefId,
    hover: Vec2,
) -> PlacementGhost {
    let held = tool
        .moving
        .and_then(|id| view.buildings.get(id))
        .map(|b| cp_building::held_cells(&view.zones, b))
        .unwrap_or_default();
    match place_over(view, db, tool, def, hover, tool.rot(), &held) {
        Ok((plan, demolish)) => {
            let (w, d) = match plan.footprint {
                cp_building::Footprint::Free { w, d } => (w, d),
                cp_building::Footprint::Lot(l) => (l.w, l.d),
            };
            PlacementGhost {
                valid: true,
                cost: Some(price(db, def, tool, plan.cost)),
                free: Some(FreeGhost {
                    pos: plan.pos,
                    rot: plan.rot,
                    polygon: footprint_quad(plan.pos, plan.rot, w, d),
                    hint: None,
                    edge: true,
                }),
                access: plan.access,
                demolish,
                ..Default::default()
            }
        }
        Err(e) => {
            // Never one cell: the whole rectangle where it would stand, in red, and a cross
            // where it is blocked.
            let blocked = cp_building::edge_blocker(
                db,
                &view.zones,
                &view.net,
                &view.buildings,
                view.terrain.as_deref(),
                Some(&view.tiles),
                def,
                hover,
                tool.rot(),
                tool.moving,
            );
            let free = blocked
                .as_ref()
                .and_then(|b| b.rect)
                .map(|polygon| FreeGhost {
                    pos: (polygon[0] + polygon[2]) * 0.5,
                    rot: 0.0,
                    polygon,
                    hint: None,
                    edge: true,
                });
            PlacementGhost {
                error: Some(match e {
                    BuildingError::Road => tr("a road is in the way"),
                    BuildingError::Blocked => tr("another building is in the way"),
                    e => error_text(&e, ServiceSite::Land),
                }),
                free,
                error_at: blocked.and_then(|b| b.at),
                ..Default::default()
            }
        }
    }
}

/// The building whose lot covers `p` (lowest id wins).
pub fn pick_building(view: &SimView, p: Vec2) -> Option<BuildingId> {
    view.buildings
        .iter()
        .filter(|(_, b)| b.pos.distance(p) <= PICK_REACH)
        .find(|(_, b)| match b.lot() {
            Some(lot) => lot.cells().any(|c| {
                view.zones
                    .cell_footprint(c)
                    .is_some_and(|o| o.distance_to_point(p) <= 1e-3)
            }),
            None => b.footprint_contains(p),
        })
        .map(|(id, _)| id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use bevy_math::Vec3;
    use cp_content::ContentDb;
    use cp_core::curve::CubicBezier3;
    use cp_net::{Elevation, NetGraph};
    use cp_sim::SimRunner;
    use cp_terrain::TileOwnership;

    fn sim() -> (SimRunner, Arc<ContentDb>) {
        let c = Arc::new(
            ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
                .unwrap(),
        );
        let road = c.road_id("basic_2l").unwrap();
        let def = c.road(road).clone();
        let mut sim = SimRunner::new(1);
        sim.set_content(Arc::clone(&c));
        sim.world_mut().insert_resource(TileOwnership::only((1, 1)));
        sim.world_mut()
            .resource_mut::<NetGraph>()
            .add_segment(
                &def,
                road,
                CubicBezier3::line(
                    Vec3::new(3000.0, 0.0, 3000.0),
                    Vec3::new(3200.0, 0.0, 3000.0),
                ),
                Elevation::Ground,
            )
            .unwrap();
        sim.tick_once();
        (sim, c)
    }

    #[test]
    fn ghost_is_valid_beside_the_road_and_red_elsewhere() {
        let (mut sim, c) = sim();
        let tool = BuildingTool {
            def: c.building_id("coal_power_plant"),
            quarters: 0,
            free_rot: 0.0,
            moving: None,
        };
        let view = sim.view();
        let near = Vec2::new(3100.0, 3012.0);
        let g = preview(&view, &tool, near).unwrap();
        assert!(
            g.valid && g.access.is_some() && g.cost == Some(Cost::from_units(16500.0)),
            "{g:?}"
        );
        let far = preview(&view, &tool, Vec2::new(3100.0, 3300.0)).unwrap();
        assert!(!far.valid);
        assert_eq!(far.error.as_deref(), Some("must be next to a road"));
        assert!(preview(&view, &BuildingTool::default(), near).is_none());
        let _ = &mut sim;
    }

    /// T-267: a refusal is never one cell: the whole 3 x 3 footprint in red where the plant would
    /// stand, and a cross where it is blocked.
    #[test]
    fn a_refused_service_ghost_shows_the_whole_footprint_and_the_blocker() {
        let (mut sim, c) = sim();
        sim.world_mut().insert_resource(TileOwnership::none());
        let tool = BuildingTool {
            def: c.building_id("coal_power_plant"),
            quarters: 0,
            free_rot: 0.0,
            moving: None,
        };
        let view = sim.view();
        let g = preview(&view, &tool, Vec2::new(3100.0, 3012.0)).unwrap();
        assert!(!g.valid);
        assert_eq!(g.error.as_deref(), Some("tile not owned"));
        let f = g.free.expect("the footprint");
        let (lo, hi) = cp_zone::geom::quad_aabb(&f.polygon);
        assert!((hi.x - lo.x - 24.0).abs() < 0.1 && (hi.y - lo.y - 24.0).abs() < 0.1);
        assert!(g.error_at.is_some() && g.cells.is_empty());
    }

    #[test]
    fn rotation_cycles_and_picking_finds_the_building() {
        let (mut sim, c) = sim();
        let mut tool = BuildingTool {
            def: c.building_id("coal_power_plant"),
            quarters: 0,
            free_rot: 0.0,
            moving: None,
        };
        for _ in 0..4 {
            tool.rotate();
        }
        assert_eq!(tool.quarters, 0);
        let at = Vec2::new(3100.0, 3012.0);
        let g = preview(&sim.view(), &tool, at).unwrap();
        assert!(g.valid);
        sim.push_command(
            cp_sim::SimCommand::Building(cp_building::BuildingCommand::Place {
                def: tool.def.unwrap(),
                pos: at,
                rot: tool.rot(),
            }),
            sim.tick(),
        );
        sim.apply_due_commands();
        let view = sim.view();
        let id = pick_building(&view, at).expect("the new building is under the cursor");
        assert_eq!(view.buildings.iter().next().unwrap().0, id);
        assert!(pick_building(&view, Vec2::new(3100.0, 3300.0)).is_none());
        // The same cursor again: a second plant stands beside the first (T-267).
        assert!(preview(&view, &tool, at).unwrap().valid);
    }

    #[test]
    fn a_fire_station_ghost_carries_the_access_point_its_building_gets() {
        let (mut sim, c) = sim();
        let tool = BuildingTool {
            def: c.building_id("fire_station"),
            quarters: 0,
            free_rot: 0.0,
            moving: None,
        };
        let at = Vec2::new(3100.0, 3012.0);
        let g = preview(&sim.view(), &tool, at).unwrap();
        assert!(g.valid, "{g:?}");
        let access = g.access.expect("a ghost on a road has an access point");
        sim.push_command(
            cp_sim::SimCommand::Building(cp_building::BuildingCommand::Place {
                def: tool.def.unwrap(),
                pos: at,
                rot: tool.rot(),
            }),
            sim.tick(),
        );
        sim.apply_due_commands();
        let view = sim.view();
        let (_, b) = view.buildings.iter().next().unwrap();
        assert_eq!(b.access(), Some(access));
        // Off the road: no access point, so no coverage preview.
        assert!(
            preview(&view, &tool, Vec2::new(3100.0, 3300.0))
                .unwrap()
                .access
                .is_none()
        );
    }

    // ---------- free-standing placement tool (T-108) ----------

    use cp_building::BuildingCommand;
    use cp_core::curve::CubicBezier3 as Bezier;
    use cp_terrain::{GRID, MAX_HEIGHT, MapMeta, SPACING, Terrain};
    use cp_utility::UtilityGraph;

    /// The real content plus a free-standing `test_tower` (kind Water) and `test_turbine`
    /// (kind Power), both 2×2.
    fn free_content() -> Arc<ContentDb> {
        fn walk(dir: &std::path::Path, rel: &str, out: &mut Vec<(String, String)>) {
            let mut entries: Vec<_> = std::fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap())
                .collect();
            entries.sort_by_key(|e| e.file_name());
            for e in entries {
                let name = format!("{rel}{}", e.file_name().to_string_lossy());
                if e.file_type().unwrap().is_dir() {
                    walk(&e.path(), &format!("{name}/"), out);
                } else if name.ends_with(".ron") {
                    out.push((
                        format!("content/{name}"),
                        std::fs::read_to_string(e.path()).unwrap(),
                    ));
                }
            }
        }
        let mut sources = Vec::new();
        walk(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content"),
            "",
            &mut sources,
        );
        for (id, kind) in [("test_tower", "Water"), ("test_turbine", "Power")] {
            sources.push((
                format!("content/buildings/{id}.ron"),
                format!(
                    r#"Building((id: "{id}", name: "T", zone: Industrial, level: 1,
                    lot: (2, 2), jobs: (0, 0, 0, 0), consumption: {{}}, tax_base: 0.0,
                    cost: 5000.0, upkeep_week: 1.0,
                    styles: ["service_water_tower"],
                    service: Some("{id}"), icon: "wind_turbine"))"#
                ),
            ));
            sources.push((
                format!("content/services/{id}.ron"),
                format!(
                    r#"Service((id: "{id}", name: "T", kind: {kind}, radius_m: 100.0,
                    capacity: 10, road_access: false, vehicles: []))"#
                ),
            ));
        }
        Arc::new(ContentDb::from_sources(sources).unwrap())
    }

    fn terrain(sea: f32, f: impl Fn(f32, f32) -> f32) -> Arc<Terrain> {
        let enc = |m: f32| (m / MAX_HEIGHT * 65535.0).round() as u16;
        let mut hs = vec![0u16; GRID * GRID];
        for iz in 0..GRID {
            for ix in 0..GRID {
                hs[iz * GRID + ix] = enc(f(ix as f32 * SPACING, iz as f32 * SPACING));
            }
        }
        let meta = MapMeta {
            name: "t".into(),
            sea_level: sea,
            start_tile: (2, 2),
            external_connections: vec![],
            entry_road: None,
            entry_rail: None,
        };
        Arc::new(Terrain::from_samples(meta, &hs, &vec![0; GRID * GRID]).unwrap())
    }

    /// A road along +X at z = 5000 (x 5000..5200) on a flat dry map, tiles around (2, 2).
    fn field() -> (SimRunner, Arc<ContentDb>) {
        let c = free_content();
        let road = c.road_id("basic_2l").unwrap();
        let mut sim = SimRunner::new(1);
        sim.set_content(Arc::clone(&c));
        sim.set_terrain(terrain(0.0, |_, _| 100.0));
        sim.world_mut()
            .insert_resource(TileOwnership::m1_around((2, 2)));
        sim.world_mut()
            .resource_mut::<NetGraph>()
            .add_segment(
                &c.road(road).clone(),
                road,
                Bezier::line(
                    Vec3::new(5000.0, 100.0, 5000.0),
                    Vec3::new(5200.0, 100.0, 5000.0),
                ),
                Elevation::Ground,
            )
            .unwrap();
        sim.tick_once();
        (sim, c)
    }

    fn tool(c: &ContentDb, id: &str) -> BuildingTool {
        BuildingTool {
            def: c.building_id(id),
            ..Default::default()
        }
    }

    #[test]
    fn the_free_ghost_follows_the_cursor_to_a_metre_with_no_cells() {
        let (mut sim, c) = field();
        let t = tool(&c, "test_tower");
        let g = preview(&sim.view(), &t, Vec2::new(5100.4, 5100.6)).unwrap();
        assert!(g.valid && g.cells.is_empty(), "{g:?}");
        let f = g.free.unwrap();
        assert_eq!(f.pos, Vec2::new(5100.0, 5101.0));
        let centre = (f.polygon[0] + f.polygon[2]) * 0.5;
        assert!(centre.distance(f.pos) < 1e-3);
        // Moving by 1 m moves the ghost by exactly 1 m: no snapping to cells.
        let g2 = preview(&sim.view(), &t, Vec2::new(5101.4, 5100.6)).unwrap();
        assert_eq!(g2.free.unwrap().pos, Vec2::new(5101.0, 5101.0));
        assert_eq!(g.cost, Some(Cost::from_units(5000.0)));
        let _ = &mut sim;
    }

    #[test]
    fn t_turns_by_15_degrees_and_wraps() {
        let (mut sim, c) = field();
        let mut t = tool(&c, "test_tower");
        let view = sim.view();
        t.turn(Some(&view), None, false);
        assert!((t.free_rot - 15f32.to_radians()).abs() < 1e-5);
        assert_eq!(t.quarters, 0, "the quarter turn is for road types");
        let rot = preview(&view, &t, Vec2::new(5100.0, 5100.0))
            .unwrap()
            .free
            .unwrap()
            .rot;
        assert!((rot - 15f32.to_radians()).abs() < 1e-5);
        for _ in 0..23 {
            t.turn(Some(&view), None, false);
        }
        assert!(t.free_rot < 1e-4 || (t.free_rot - std::f32::consts::TAU).abs() < 1e-4);
        // A road type still turns by quarters.
        let mut w = tool(&c, "coal_power_plant");
        w.turn(Some(&view), None, false);
        assert_eq!((w.quarters, w.free_rot), (1, 0.0));
        let _ = &mut sim;
    }

    #[test]
    fn shift_t_takes_the_angle_of_the_nearest_road_within_60_m() {
        let (mut sim, c) = field();
        let mut t = tool(&c, "test_tower");
        t.free_rot = 1.0;
        let view = sim.view();
        // 50 m from the road along +X: angle 0.
        t.turn(Some(&view), Some(Vec2::new(5100.0, 5050.0)), true);
        assert!(t.free_rot.abs() < 1e-4, "{}", t.free_rot);
        // 100 m away there is no road to follow: unchanged.
        t.free_rot = 1.0;
        t.turn(Some(&view), Some(Vec2::new(5100.0, 5100.0)), true);
        assert_eq!(t.free_rot, 1.0);
        drop(view);

        // A road at 45°.
        let road = c.road_id("basic_2l").unwrap();
        sim.world_mut()
            .resource_mut::<NetGraph>()
            .add_segment(
                &c.road(road).clone(),
                road,
                Bezier::line(
                    Vec3::new(4200.0, 100.0, 4200.0),
                    Vec3::new(4300.0, 100.0, 4300.0),
                ),
                Elevation::Ground,
            )
            .unwrap();
        sim.tick_once();
        t.turn(Some(&sim.view()), Some(Vec2::new(4250.0, 4270.0)), true);
        assert!((t.free_rot - std::f32::consts::FRAC_PI_4).abs() < 1e-3);
    }

    #[test]
    fn the_verdict_names_each_error() {
        let (mut sim, c) = field();
        let t = tool(&c, "test_tower");
        let err = |sim: &mut SimRunner, p: Vec2| {
            let g = preview(&sim.view(), &t, p).unwrap();
            assert!(!g.valid && g.cost.is_none(), "{g:?}");
            g.error.unwrap()
        };
        // On the road.
        assert!(err(&mut sim, Vec2::new(5100.0, 5003.0)).contains("road"));
        // On a tile that is not owned.
        sim.world_mut().insert_resource(TileOwnership::only((0, 0)));
        assert_eq!(err(&mut sim, Vec2::new(5100.0, 5100.0)), "tile not owned");
        sim.world_mut()
            .insert_resource(TileOwnership::m1_around((2, 2)));
        // On a lot or another building.
        let at = Vec2::new(5100.0, 5100.0);
        sim.push_command(
            cp_sim::SimCommand::Building(BuildingCommand::Place {
                def: t.def.unwrap(),
                pos: at,
                rot: 0.0,
            }),
            sim.tick(),
        );
        sim.apply_due_commands();
        assert!(err(&mut sim, Vec2::new(5104.0, 5100.0)).contains("blocked"));
        // On water.
        sim.set_terrain(terrain(100.0, |_, _| 90.0));
        assert!(err(&mut sim, Vec2::new(5100.0, 5300.0)).contains("water"));
        // On a 40 % slope.
        sim.set_terrain(terrain(0.0, |_, z| 100.0 + 0.4 * (z - 5300.0)));
        assert!(err(&mut sim, Vec2::new(5100.0, 5300.0)).contains("too steep"));
    }

    #[test]
    fn the_hint_says_connects_at_30_m_and_no_network_at_60_m() {
        let (mut sim, c) = field();
        let def = c.building_id("test_tower").unwrap();
        {
            let mut g = sim.world_mut().resource_mut::<UtilityGraph>();
            let a = g.add_node(UtilityNet::Pipe, Vec3::new(5000.0, 100.0, 5100.0));
            let b = g.add_node(UtilityNet::Pipe, Vec3::new(5200.0, 100.0, 5100.0));
            g.add_link(
                a,
                b,
                Bezier::line(
                    Vec3::new(5000.0, 100.0, 5100.0),
                    Vec3::new(5200.0, 100.0, 5100.0),
                ),
            )
            .unwrap();
        }
        sim.tick_once();
        let view = sim.view();
        let near = connection_hint(&view, def, Vec2::new(5100.0, 5130.0)).unwrap();
        assert!(
            near.connects && near.text.contains("will connect to the pipe"),
            "{near:?}"
        );
        let t = near.target.unwrap();
        assert!((t.y - 5100.0).abs() < 0.5, "{t:?}");
        let far = connection_hint(&view, def, Vec2::new(5100.0, 5200.0)).unwrap();
        assert!(!far.connects && far.target.is_none());
        assert!(far.text.contains("no pipe within 64 m"), "{far:?}");
        // The same spot shows up in the ghost.
        let ghost = preview(&view, &tool(&c, "test_tower"), Vec2::new(5100.0, 5130.0)).unwrap();
        assert!(ghost.free.unwrap().hint.unwrap().connects);
        // A power type looks for a power line, which is not there: pipes do not count.
        let turbine = c.building_id("test_turbine").unwrap();
        let h = connection_hint(&view, turbine, Vec2::new(5100.0, 5130.0)).unwrap();
        assert!(!h.connects && h.net == UtilityNet::Power && h.text.contains("power line"));
        // The circle of the tool is the line reach of the content (T-181).
        assert_eq!(h.reach, c.balance().networks.line_reach_m);
        assert_eq!(near.reach, c.balance().networks.pipe_reach_m);
        // A road type has no hint of its own.
        assert!(
            connection_hint(
                &view,
                c.building_id("coal_power_plant").unwrap(),
                Vec2::ZERO
            )
            .is_some()
        );
    }
}
