//! T-164: the metro tools. The track tool's two clicks send a `Tunnel` build; the metro line
//! tool takes stations only (a click on a road adds nothing); a built line is routed on the
//! track and its trains have a pose for the Underground view.

use std::path::Path;
use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_building::{BuildingCommand, BuildingGrid};
use cp_content::ContentDb;
use cp_core::Cost;
use cp_core::curve::CubicBezier3;
use cp_core::treasury::Treasury;
use cp_net::{Elevation, ElevationMode, NetCommand, NetGraph};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::{Terrain, TileOwnership};
use cp_tools::transit::{Click, LineTool};
use cp_tools::{RoadMode, RoadTool};
use cp_transit::{StopKind, TransitMode, TransitNet};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn town() -> (SimRunner, Arc<ContentDb>) {
    let db = Arc::new(ContentDb::load_dir(format!("{ROOT}/../../assets/content")).unwrap());
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let road = db.road_id("basic_2l").unwrap();
    let def = db.road(road).clone();
    let mut sim = SimRunner::new(1);
    sim.set_terrain(terrain);
    sim.set_content(Arc::clone(&db));
    sim.world_mut().insert_resource(TileOwnership::only((1, 1)));
    sim.world_mut().resource_mut::<Treasury>().balance = Cost(1_000_000_000);
    for k in 0..4 {
        let x = 2600.0 + 200.0 * k as f32;
        sim.world_mut()
            .resource_mut::<NetGraph>()
            .add_segment(
                &def,
                road,
                CubicBezier3::line(Vec3::new(x, 0.0, 3000.0), Vec3::new(x + 200.0, 0.0, 3000.0)),
                Elevation::Ground,
            )
            .unwrap();
    }
    sim.tick_once();
    (sim, db)
}

fn place_station(sim: &mut SimRunner, db: &ContentDb, x: f32) {
    sim.push_command(
        SimCommand::Building(BuildingCommand::Place {
            def: db.building_id("metro_station").unwrap(),
            pos: Vec2::new(x, 3010.0),
            rot: 0.0,
        }),
        sim.tick(),
    );
    sim.tick_once();
}

#[test]
fn the_track_tool_builds_a_tunnel_and_the_line_tool_takes_stations_only() {
    let (mut sim, db) = town();
    for x in [2700.0, 2900.0] {
        place_station(&mut sim, &db, x);
    }
    assert_eq!(sim.world().resource::<BuildingGrid>().len(), 2);

    // The track tool: two clicks between the platforms are one tunnel build.
    let metro = db.road_id("metro_track").unwrap();
    let depth = db.road(metro).tunnel.as_ref().unwrap().depth;
    let ends = |i: usize| {
        let transit = sim.world().resource::<TransitNet>();
        let net = sim.world().resource::<NetGraph>();
        let seg = transit
            .stops
            .iter()
            .filter_map(|(_, s)| match s.kind {
                StopKind::Station { seg, .. } => Some(seg),
                _ => None,
            })
            .nth(i)
            .unwrap();
        let s = net.segment(seg).unwrap();
        let (a, b) = (net.node(s.start).unwrap().pos, net.node(s.end).unwrap().pos);
        let (a, b) = (Vec2::new(a.x, a.z), Vec2::new(b.x, b.z));
        if a.x < b.x { (a, b) } else { (b, a) }
    };
    let (from, to) = (ends(0).1, ends(1).0);
    let mut tool = RoadTool {
        mode: RoadMode::Straight,
        road: Some(metro),
        tunnel_depth: Some(depth),
        ..Default::default()
    };
    assert!(tool.click(from).is_none());
    let req = tool.click(to).expect("two clicks finish the piece");
    assert_eq!(req.elevation, ElevationMode::Tunnel(depth));
    let before = sim.world().resource::<NetGraph>().segments().count();
    sim.push_command(
        SimCommand::Net(NetCommand::BuildRoad {
            road: req.road,
            path: req.path,
            elevation: req.elevation,
        }),
        sim.tick(),
    );
    sim.tick_once();
    let net = sim.world().resource::<NetGraph>();
    assert!(net.segments().count() > before);
    assert!(
        net.segments()
            .any(|(_, s)| s.elevation == Elevation::Tunnel && s.road == metro)
    );

    // The line tool in metro mode: a click on a road is nothing, a click on a station is a stop.
    let view = sim.view();
    let mut line = LineTool {
        mode: TransitMode::Metro,
        ..Default::default()
    };
    assert_eq!(line.click(&view, Vec2::new(2650.0, 3003.0)), Click::Nothing);
    assert!(line.stops.is_empty());
    let stations: Vec<Vec2> = view
        .transit
        .net
        .stops
        .iter()
        .filter(|(_, s)| matches!(s.kind, StopKind::Station { .. }))
        .map(|(_, s)| s.pos)
        .collect();
    assert_eq!(stations.len(), 2);
    assert_eq!(line.click(&view, stations[0]), Click::Added);
    assert_eq!(line.click(&view, stations[1]), Click::Added);
    let Click::Create(SimCommand::Transit(cp_transit::TransitCommand::CreateLine { mode, .. })) =
        line.click(&view, stations[0])
    else {
        panic!("a click on the first station closes the line");
    };
    assert_eq!(mode, TransitMode::Metro);
    // A bus line tool does not take a station.
    let mut bus = LineTool::default();
    assert_ne!(bus.click(&view, stations[0]), Click::Added);
    assert!(bus.stops.is_empty());
}

#[test]
fn a_metro_line_is_routed_on_the_track_and_its_trains_have_a_pose_underground() {
    let (mut sim, db) = town();
    for x in [2700.0, 3100.0] {
        place_station(&mut sim, &db, x);
    }
    let metro = db.road_id("metro_track").unwrap();
    let (a, b) = {
        let net = sim.world().resource::<NetGraph>();
        let mut ends: Vec<(Vec2, Vec2)> = net
            .segments()
            .filter(|(_, s)| s.elevation == Elevation::Tunnel)
            .map(|(_, s)| {
                let (a, b) = (net.node(s.start).unwrap().pos, net.node(s.end).unwrap().pos);
                (Vec2::new(a.x, a.z), Vec2::new(b.x, b.z))
            })
            .collect();
        ends.sort_by(|p, q| p.0.x.total_cmp(&q.0.x));
        (ends[0].1.max(ends[0].0), ends[1].0.min(ends[1].1))
    };
    sim.push_command(
        SimCommand::Net(NetCommand::BuildRoad {
            road: metro,
            path: cp_net::RoadPath::Straight { from: a, to: b },
            elevation: ElevationMode::Tunnel(12.0),
        }),
        sim.tick(),
    );
    sim.tick_once();
    let stops: Vec<_> = sim
        .world()
        .resource::<TransitNet>()
        .stops
        .iter()
        .map(|(id, _)| cp_transit::StopRef::Existing(id))
        .collect();
    cp_transit::apply(
        sim.world_mut(),
        &cp_transit::TransitCommand::CreateLine {
            mode: TransitMode::Metro,
            stops,
        },
    )
    .unwrap();
    let mut seen = 0;
    for _ in 0..120 {
        sim.tick_once();
        let view = sim.view();
        for t in view.metro_trains.iter() {
            assert!(t.pos.is_finite() && t.next.is_finite());
            let ground = view
                .terrain
                .as_deref()
                .unwrap()
                .height(Vec2::new(t.pos.x, t.pos.z));
            assert!(t.pos.y < ground - 5.0, "{t:?} under {ground}");
            seen += 1;
        }
    }
    assert!(seen > 0, "a metro train was never posed");
    let view = sim.view();
    let route = view.transit.routes.values().next().unwrap();
    assert!(route.underground() && route.points.len() > 10);
    assert_eq!(route.points.len(), route.heights.len());
}
