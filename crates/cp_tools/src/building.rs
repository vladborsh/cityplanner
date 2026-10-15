//! The service-building placement tool and the building half of the bulldozer.
//!
//! Pure: placement validity comes from `cp_building::validate_placement`, the same function the
//! `Place` command commits with; the plugin turns clicks into `BuildingCommand`s.

use std::f32::consts::{FRAC_PI_2, TAU};

use bevy_math::Vec2;
use cp_building::{
    BuildingError, BuildingFlags, BuildingId, BuildingState, footprint_quad, is_free_standing,
    validate_free_placement, validate_placement,
};
use cp_content::{BuildingDefId, ContentDb, ServiceKind, ServiceSite};
use cp_core::Cost;
use cp_core::curve::to_xz;
use cp_sim::SimView;
use cp_utility::{LINE_REACH_M, POWER_HOP_M, UtilityNet};
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
}

impl BuildingTool {
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
            "needs water under the lot: put at least one cell over water".to_string()
        }
        (BuildingError::NeedsWater, _) => format!(
            "needs a shore: the lot on land with water within {:.0} m",
            ServiceSite::SHORE_REACH_M
        ),
        (BuildingError::Water, _) => "water here: this building goes on land only".to_string(),
        (BuildingError::Road, _) => "too close to a road: keep 1 m clear of its edge".to_string(),
        (BuildingError::TooSteep, _) => "ground too steep: 25 % at most".to_string(),
        _ => e.to_string(),
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
}

/// Where a free-standing building would connect (§8.1, §10).
#[derive(Clone, Debug, PartialEq)]
pub struct ConnectionHint {
    pub net: UtilityNet,
    /// The point of the pipe, line or powered building it would connect to, when in reach.
    pub target: Option<Vec2>,
    pub connects: bool,
    pub text: String,
}

/// Which network a type of this service kind hooks into, and the words for it.
fn network_of(kind: ServiceKind) -> Option<UtilityNet> {
    match kind {
        ServiceKind::Water | ServiceKind::Sewage => Some(UtilityNet::Pipe),
        ServiceKind::Power => Some(UtilityNet::Power),
        ServiceKind::Roads => None,
    }
}

/// Whether the building of `def` placed at `pos` would connect to the network of its service:
/// a pipe or power line within [`LINE_REACH_M`], or (power) a supplied building within
/// [`POWER_HOP_M`]. `None` for a type with no network. The view's content must know `def`.
pub fn connection_hint(view: &SimView, def: BuildingDefId, pos: Vec2) -> Option<ConnectionHint> {
    let db = view.content.as_deref()?;
    let service = db
        .building(def)
        .service
        .as_deref()
        .and_then(|s| db.service_id(s))
        .map(|s| db.service(s))?;
    let net = network_of(service.kind)?;
    let line = view.utilities.nearest_link(net, pos, LINE_REACH_M);
    let mut target =
        line.and_then(|(id, t, _)| view.utilities.link(id).map(|l| to_xz(l.curve.point(t))));
    let mut what = match net {
        UtilityNet::Pipe => "the pipe",
        UtilityNet::Power => "the power line",
    };
    if target.is_none() && net == UtilityNet::Power {
        // A supplied building passes the current on (30 m).
        target = view
            .buildings
            .iter()
            .filter(|(_, b)| {
                b.state == BuildingState::Active && !b.flags.has(BuildingFlags::NO_POWER)
            })
            .map(|(_, b)| b.pos)
            .filter(|p| p.distance(pos) <= POWER_HOP_M)
            .min_by(|a, b| a.distance(pos).total_cmp(&b.distance(pos)));
        what = "a powered building";
    }
    let text = match (target.is_some(), net) {
        (true, _) => format!("will connect to {what}"),
        (false, UtilityNet::Pipe) => {
            format!("no pipe within {LINE_REACH_M:.0} m — lay a pipe")
        }
        (false, UtilityNet::Power) => format!(
            "no power line within {LINE_REACH_M:.0} m, no powered building within {POWER_HOP_M:.0} m — build a line"
        ),
    };
    Some(ConnectionHint {
        net,
        target,
        connects: target.is_some(),
        text,
    })
}

/// The ghost for `tool` with the cursor at `hover`; `None` without a selected type or content.
pub fn preview(view: &SimView, tool: &BuildingTool, hover: Vec2) -> Option<PlacementGhost> {
    let def = tool.def?;
    let db = view.content.as_deref()?;
    if is_free_standing(db, def) {
        return Some(free_preview(view, db, tool, def, hover));
    }
    Some(
        match validate_placement(
            db,
            &view.zones,
            &view.net,
            view.terrain.as_deref(),
            Some(&view.tiles),
            def,
            hover,
            tool.rot(),
        ) {
            Ok(plan) => PlacementGhost {
                cells: plan.cells(),
                valid: true,
                cost: Some(plan.cost),
                ..Default::default()
            },
            Err(e) => PlacementGhost {
                cells: view.zones.cell_at(hover).into_iter().collect(),
                valid: false,
                cost: None,
                free: None,
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
    };
    match validate_free_placement(
        db,
        &view.zones,
        &view.net,
        &view.buildings,
        view.terrain.as_deref(),
        Some(&view.tiles),
        def,
        pos,
        rot,
    ) {
        Ok(plan) => PlacementGhost {
            valid: true,
            cost: Some(plan.cost),
            free: Some(free),
            ..Default::default()
        },
        Err(e) => PlacementGhost {
            error: Some(error_text(&e, ServiceSite::Land)),
            free: Some(free),
            ..Default::default()
        },
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
            def: c.building_id("wind_turbine"),
            quarters: 0,
            free_rot: 0.0,
        };
        let view = sim.view();
        let near = Vec2::new(3100.0, 3012.0);
        let g = preview(&view, &tool, near).unwrap();
        assert!(
            g.valid && g.cells.len() == 4 && g.cost == Some(Cost::from_units(6000.0)),
            "{g:?}"
        );
        let far = preview(&view, &tool, Vec2::new(3100.0, 3300.0)).unwrap();
        assert!(!far.valid);
        assert_eq!(far.error.as_deref(), Some("must be next to a road"));
        assert!(preview(&view, &BuildingTool::default(), near).is_none());
        let _ = &mut sim;
    }

    #[test]
    fn rotation_cycles_and_picking_finds_the_building() {
        let (mut sim, c) = sim();
        let mut tool = BuildingTool {
            def: c.building_id("wind_turbine"),
            quarters: 0,
            free_rot: 0.0,
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
        // The same spot is now taken.
        assert!(!preview(&view, &tool, at).unwrap().valid);
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
                    style: (floors: 1, roof: Flat, palette: "utility_grey"),
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
        let mut w = tool(&c, "wind_turbine");
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
        assert!(err(&mut sim, Vec2::new(5100.0, 5003.0)).contains("too close to a road"));
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
        let far = connection_hint(&view, def, Vec2::new(5100.0, 5160.0)).unwrap();
        assert!(!far.connects && far.target.is_none());
        assert!(far.text.contains("no pipe within 40 m"), "{far:?}");
        // The same spot shows up in the ghost.
        let ghost = preview(&view, &tool(&c, "test_tower"), Vec2::new(5100.0, 5130.0)).unwrap();
        assert!(ghost.free.unwrap().hint.unwrap().connects);
        // A power type looks for a power line, which is not there: pipes do not count.
        let turbine = c.building_id("test_turbine").unwrap();
        let h = connection_hint(&view, turbine, Vec2::new(5100.0, 5130.0)).unwrap();
        assert!(!h.connects && h.net == UtilityNet::Power && h.text.contains("power line"));
        // A road type has no hint of its own.
        assert!(
            connection_hint(&view, c.building_id("wind_turbine").unwrap(), Vec2::ZERO).is_some()
        );
    }
}
