//! T-115: a new game builds the map's entry highway (`system-design.md` §5.1).

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec2;
use cp_content::{ContentDb, LaneDir, LaneKind, RoadClass};

use cp_net::{JunctionGeometry, LaneId, NetGraph, NodeId, NodeKind};
use cp_sim::SimRunner;
use cp_terrain::{GRID, MAP_SIZE, MapMeta, TILE_SIZE, Terrain};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn content() -> Arc<ContentDb> {
    Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap())
}

fn map(name: &str) -> Arc<Terrain> {
    Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps").join(name)).unwrap())
}

fn sim_on(terrain: Arc<Terrain>) -> SimRunner {
    let mut sim = SimRunner::new(1);
    sim.set_terrain(terrain);
    sim.set_content(content());
    sim
}

fn in_start_tile(t: &Terrain, p: Vec2) -> bool {
    let (tx, tz) = t.meta().start_tile;
    let (x0, z0) = (tx as f32 * TILE_SIZE, tz as f32 * TILE_SIZE);
    p.x > x0 && p.x < x0 + TILE_SIZE && p.y > z0 && p.y < z0 + TILE_SIZE
}

/// Lanes (cars) reachable from `from` through the junction connections.
fn reach(graph: &NetGraph, geo: &JunctionGeometry, from: Vec<LaneId>) -> BTreeSet<LaneId> {
    let mut seen: BTreeSet<LaneId> = from.iter().copied().collect();
    let mut todo = from;
    while let Some(l) = todo.pop() {
        for (_, j) in geo.iter() {
            for c in j.links.connections.iter().filter(|c| c.from == l) {
                if graph.lane(c.to).is_some() && seen.insert(c.to) {
                    todo.push(c.to);
                }
            }
        }
    }
    seen
}

/// The car lanes that leave (`departing`) or arrive at `node`.
fn lanes_at(graph: &NetGraph, node: NodeId, departing: bool) -> Vec<LaneId> {
    let mut out = Vec::new();
    for (id, lane) in graph.lanes() {
        let seg = graph.segment(lane.segment).unwrap();
        let (from, to) = match lane.dir {
            LaneDir::Forward => (seg.start, seg.end),
            LaneDir::Backward => (seg.end, seg.start),
        };
        if lane.kind == LaneKind::Car && (if departing { from } else { to }) == node {
            out.push(id);
        }
    }
    out
}

#[test]
fn a_new_game_builds_the_entry_highway_on_every_map() {
    let db = content();
    for name in ["flatlands", "river_valley"] {
        let terrain = map(name);
        let mut sim = sim_on(Arc::clone(&terrain));
        sim.new_game().unwrap_or_else(|e| panic!("{name}: {e}"));
        sim.tick_once();
        sim.tick_once();
        let view = sim.view();
        let graph = &view.net;

        let outside: Vec<_> = graph
            .nodes()
            .filter(|(_, n)| n.kind == NodeKind::Outside && !cp_net::node_is_rail(graph, n))
            .collect();
        assert_eq!(outside.len(), 2, "{name}: two Outside nodes");
        for (_, n) in &outside {
            let on_edge = |v: f32| v.abs() < 0.01 || (v - MAP_SIZE).abs() < 0.01;
            assert!(
                on_edge(n.pos.x) || on_edge(n.pos.z),
                "{name}: Outside at {:?}",
                n.pos
            );
        }

        // Highway segments (one-way, Highway class) and the large stub.
        let class_of = |s: &cp_net::Segment| db.road(s.road).class;
        let highways = graph
            .segments()
            .filter(|(_, s)| class_of(s) == RoadClass::Highway)
            .count();
        let stubs: Vec<_> = graph
            .segments()
            .filter(|(_, s)| db.road(s.road).id == "large_6l")
            .collect();
        assert!(highways >= 2, "{name}");
        assert_eq!(stubs.len(), 1, "{name}");

        // The junction J joins the two highways and the stub; the stub ends in a dead end.
        let (_, stub) = stubs[0];
        let j = graph.node(stub.start).unwrap();
        assert_eq!(j.kind, NodeKind::Junction, "{name}");
        assert_eq!(j.segments.len(), 3, "{name}");
        let dead = graph.node(stub.end).unwrap();
        assert_eq!(dead.kind, NodeKind::End, "{name}");
        for n in [j, dead] {
            assert!(
                in_start_tile(&terrain, Vec2::new(n.pos.x, n.pos.z)),
                "{name}: {:?}",
                n.pos
            );
        }
        assert!(
            (Vec2::new(dead.pos.x, dead.pos.z) - Vec2::new(j.pos.x, j.pos.z)).length()
                > terrain.meta().entry_road.as_ref().unwrap().stub_m - 1.0
        );

        // Both ways by lanes: Outside -> dead end, and dead end -> the other Outside.
        let (inn, out) = (outside[0].0, outside[1].0);
        let (first, second) = if !lanes_at(graph, inn, true).is_empty() {
            (inn, out)
        } else {
            (out, inn)
        };
        let from_edge = reach(graph, &view.junctions, lanes_at(graph, first, true));
        let to_dead = lanes_at(graph, stub.end, false);
        assert!(
            !to_dead.is_empty() && to_dead.iter().any(|l| from_edge.contains(l)),
            "{name}: no way in"
        );
        let from_dead = reach(graph, &view.junctions, lanes_at(graph, stub.end, true));
        let to_edge = lanes_at(graph, second, false);
        assert!(
            !to_edge.is_empty() && to_edge.iter().any(|l| from_dead.contains(l)),
            "{name}: no way out"
        );
    }
}

#[test]
fn the_new_game_checksum_is_deterministic_and_other_starts_have_no_entry_road() {
    let run = |new_game: bool| {
        let mut sim = sim_on(map("flatlands"));
        if new_game {
            sim.new_game().unwrap();
        }
        for _ in 0..50 {
            sim.tick_once();
        }
        (sim.checksum(), sim.view().net.node_count())
    };
    let (a, na) = run(true);
    let (b, nb) = run(true);
    assert_eq!((a, na), (b, nb), "two new games differ");
    let (empty, n) = run(false);
    assert_eq!(n, 0, "set_terrain alone builds nothing");
    assert_ne!(a, empty);
}

fn flat_with(highway: &str, stub: &str) -> SimRunner {
    use cp_terrain::EntryRoad;
    let meta = MapMeta {
        name: "t".into(),
        sea_level: 0.0,
        start_tile: (2, 2),
        external_connections: vec![(5120.0, 0.0)],
        entry_road: Some(EntryRoad {
            connection: 0,
            path: vec![(5120.0, 0.0), (5120.0, 4300.0)],
            highway: highway.into(),
            stub: stub.into(),
            stub_m: 48.0,
        }),
        entry_rail: None,
    };
    let h = (50.0 / cp_terrain::MAX_HEIGHT * 65535.0) as u16;
    let t = Terrain::from_samples(meta, &vec![h; GRID * GRID], &vec![0; GRID * GRID]).unwrap();
    sim_on(Arc::new(t))
}

#[test]
fn new_game_says_why_when_the_road_types_are_wrong() {
    assert!(flat_with("highway_1w_2l", "large_6l").new_game().is_ok());
    let e = flat_with("no_such_road", "large_6l")
        .new_game()
        .unwrap_err();
    assert!(
        e.contains("no_such_road") && e.contains("does not exist"),
        "{e}"
    );
    let e = flat_with("basic_2l", "large_6l").new_game().unwrap_err();
    assert!(e.contains("Highway"), "{e}");
    let e = flat_with("highway_1w_2l", "nope").new_game().unwrap_err();
    assert!(e.contains("stub"), "{e}");
}

/// T-117: residents arrive only into houses connected to the entry highway (§5.1, §9.1).
#[test]
fn residents_arrive_only_when_the_street_is_connected_to_the_entry_highway() {
    use bevy_math::Vec3;
    use cp_content::ZoneType;
    use cp_core::curve::CubicBezier3;
    use cp_core::time::TICKS_PER_DAY;
    use cp_net::Elevation;
    use cp_zone::{ZoneCellRef, ZoneGrid};

    let db = content();
    let terrain = map("flatlands");
    let e = terrain.meta().entry_road.clone().unwrap();
    let mut sim = sim_on(Arc::clone(&terrain));
    sim.new_game().unwrap();
    *sim.world_mut().resource_mut::<cp_building::Demand>() = cp_building::Demand::fixed(1.0);
    *sim.world_mut().resource_mut::<cp_economy::Decay>() = cp_economy::Decay::disabled();

    let (end, dir) = (e.stub_end(), e.direction());
    let side = Vec2::new(-dir.y, dir.x);
    let y = |p: Vec2| Vec3::new(p.x, terrain.height(p), p.y);
    let a = end + side * 120.0;
    let b = a + dir * 160.0;
    let road = db.road_id("basic_2l").unwrap();
    let def = db.road(road).clone();
    let street = sim
        .world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            &def,
            road,
            cp_core::curve::CubicBezier3::line(y(a), y(b)),
            Elevation::Ground,
        )
        .unwrap();
    sim.tick_once();
    {
        let mut grid = sim.world_mut().resource_mut::<ZoneGrid>();
        let blocks: Vec<_> = grid
            .blocks_of(street)
            .into_iter()
            .flatten()
            .map(|id| (id, grid.block(id).unwrap().cell_count()))
            .collect();
        assert!(!blocks.is_empty(), "the street has zone blocks");
        for (id, cells) in blocks {
            for cell in 0..cells {
                grid.set_zone(
                    ZoneCellRef {
                        block: id,
                        cell: cell as u16,
                    },
                    ZoneType::ResLow,
                );
            }
        }
    }
    let households = |sim: &SimRunner| {
        sim.world()
            .resource::<cp_citizen::CitizenGrid>()
            .household_count()
    };
    for _ in 0..2 * TICKS_PER_DAY {
        sim.tick_once();
    }
    let buildings = sim
        .world()
        .resource::<cp_building::BuildingGrid>()
        .iter()
        .count();
    assert!(buildings > 0, "houses were built");
    assert_eq!(households(&sim), 0, "nobody arrives without a connection");
    assert!(!sim.view().city.connected_to_highway);

    // Connect the street to the end of the stub.
    sim.world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            &def,
            road,
            CubicBezier3::line(y(end), y(a)),
            Elevation::Ground,
        )
        .unwrap();
    for _ in 0..TICKS_PER_DAY {
        sim.tick_once();
    }
    assert!(households(&sim) > 0, "households arrive once connected");
    assert!(sim.view().city.connected_to_highway);
}

/// A connected new game on flatlands with a zoned street off the entry stub, run for three
/// days on `db`: `(checksum, most move-in cars on the road at once, citizens at home)`.
fn move_in_town(db: Arc<ContentDb>) -> (u64, usize, usize) {
    use bevy_math::Vec3;
    use cp_citizen::{Location, TripBook, TripPurpose};
    use cp_content::ZoneType;
    use cp_core::curve::CubicBezier3;
    use cp_core::time::TICKS_PER_DAY;
    use cp_net::Elevation;
    use cp_zone::{ZoneCellRef, ZoneGrid};

    {
        let terrain = map("flatlands");
        let e = terrain.meta().entry_road.clone().unwrap();
        let mut sim = SimRunner::new(1);
        sim.set_terrain(Arc::clone(&terrain));
        sim.set_content(db.clone());
        sim.new_game().unwrap();
        *sim.world_mut().resource_mut::<cp_building::Demand>() = cp_building::Demand::fixed(1.0);
        *sim.world_mut().resource_mut::<cp_economy::Decay>() = cp_economy::Decay::disabled();
        let (end, dir) = (e.stub_end(), e.direction());
        let side = Vec2::new(-dir.y, dir.x);
        let y = |p: Vec2| Vec3::new(p.x, terrain.height(p), p.y);
        let (a, b) = (end + side * 120.0, end + side * 120.0 + dir * 160.0);
        let road = db.road_id("basic_2l").unwrap();
        let def = db.road(road).clone();
        let street = {
            let mut g = sim.world_mut().resource_mut::<NetGraph>();
            let street = g
                .add_segment(
                    &def,
                    road,
                    CubicBezier3::line(y(a), y(b)),
                    Elevation::Ground,
                )
                .unwrap();
            g.add_segment(
                &def,
                road,
                CubicBezier3::line(y(end), y(a)),
                Elevation::Ground,
            )
            .unwrap();
            street
        };
        sim.tick_once();
        {
            let mut grid = sim.world_mut().resource_mut::<ZoneGrid>();
            let blocks: Vec<_> = grid
                .blocks_of(street)
                .into_iter()
                .flatten()
                .map(|id| (id, grid.block(id).unwrap().cell_count()))
                .collect();
            for (id, cells) in blocks {
                for cell in 0..cells {
                    grid.set_zone(
                        ZoneCellRef {
                            block: id,
                            cell: cell as u16,
                        },
                        ZoneType::ResLow,
                    );
                }
            }
        }
        let mut max_cars = 0usize;
        for _ in 0..3 * TICKS_PER_DAY {
            sim.tick_once();
            let book = sim.world().resource::<TripBook>();
            let agents = sim.world().resource::<cp_traffic::Agents>();
            let cars: Vec<_> = agents
                .vehicles()
                .filter(|(_, v)| {
                    book.get(v.trip)
                        .is_some_and(|t| t.purpose == TripPurpose::MoveIn)
                })
                .collect();
            max_cars = max_cars.max(cars.len());
        }
        let grid = sim.world().resource::<cp_citizen::CitizenGrid>();
        assert!(grid.household_count() > 0);
        let home = grid
            .citizens()
            .filter(|(_, c)| c.location == Location::Home)
            .count();
        (sim.checksum(), max_cars, home)
    }
}

/// T-118: a connected new game's households drive in from the Outside node along the highway.
#[test]
fn move_in_cars_drive_from_the_outside_node_and_the_household_arrives_with_them() {
    let run = || {
        let (checksum, max_cars, home) = move_in_town(content());
        assert!(max_cars > 0, "move-in cars were on the road");
        assert!(home > 0, "households arrived");
        (checksum, max_cars)
    };
    assert_eq!(run(), run(), "two runs differ");
}

/// T-306: `move_in_car_share` says which households drive in; the others arrive abstractly, and
/// `car_visible_share` has no say. At 0 no move-in car is on the road and the households still
/// arrive; at 1 every household's car is.
#[test]
fn the_move_in_car_share_decides_who_drives_in_and_everyone_arrives() {
    let sharing = |move_in: f32, car: f32| {
        let root = Path::new(ROOT).join("../../assets/content");
        let db = ContentDb::load_dir_with_balance(root, |text| {
            text.lines()
                .map(|l| {
                    let t = l.trim_start();
                    if t.starts_with("move_in_car_share:") {
                        format!("    move_in_car_share: {move_in:?},")
                    } else if t.starts_with("car_visible_share:") {
                        format!("    car_visible_share: {car:?},")
                    } else {
                        l.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap();
        assert_eq!(db.balance().move_in_car_share, move_in);
        Arc::new(db)
    };
    let (_, none, home) = move_in_town(sharing(0.0, 1.0));
    assert_eq!(none, 0, "no move-in car with the share at 0");
    assert!(home > 0, "the households arrive all the same");
    let (_, all, home_all) = move_in_town(sharing(1.0, 0.0));
    assert!(all > 0, "move-in cars with the share at 1");
    assert!(home_all > 0);
}

/// T-180: on river_valley the start tile is open ground: from the end of the entry stub a
/// `basic_2l` street of 200 m builds in four directions.
#[test]
fn river_valley_streets_build_in_four_directions_from_the_stub_end() {
    use cp_net::{ElevationMode, NetCommand, RoadPath};
    use cp_sim::SimCommand;

    let db = content();
    let terrain = map("river_valley");
    let e = terrain.meta().entry_road.clone().unwrap();
    let mut sim = sim_on(Arc::clone(&terrain));
    sim.new_game().unwrap();
    sim.tick_once();
    let (end, dir) = (e.stub_end(), e.direction());
    let left = Vec2::new(-dir.y, dir.x);
    let road = db.road_id("basic_2l").unwrap();
    for (name, d) in [
        ("ahead", dir),
        ("left", left),
        ("right", -left),
        ("ahead-left", (dir + left).normalize()),
    ] {
        sim.push_command(
            SimCommand::Net(NetCommand::BuildRoad {
                road,
                path: RoadPath::Straight {
                    from: end,
                    to: end + d * 200.0,
                },
                elevation: ElevationMode::Ground,
            }),
            sim.tick(),
        );
        sim.apply_due_commands();
        let out: Vec<_> = sim
            .world_mut()
            .resource_mut::<bevy_ecs::message::Messages<cp_core::CommandOutcome>>()
            .drain()
            .collect();
        assert!(
            out.last().unwrap().result.is_ok(),
            "{name}: {:?}",
            out.last().unwrap().result
        );
    }
}

/// T-205: a new game on a map with a resource map starts with its ore and oil full; a game
/// that is not started (no new game) has none.
#[test]
fn new_game_fills_the_deposits_from_the_resource_map() {
    use cp_content::ResourceKind;
    use cp_terrain::{RES_GRID, ResourceDeposits};
    let terrain = map("flatlands");
    let mut sim = sim_on(terrain.clone());
    assert!(sim.world().resource::<ResourceDeposits>().is_unfilled());
    sim.new_game().unwrap();
    let per_cell = content().balance().deposit_per_cell;
    let d = sim.world().resource::<ResourceDeposits>().clone();
    assert!(!d.is_unfilled());
    assert_eq!(sim.view().deposits.as_ref(), &d);
    let mut any = false;
    for cz in 0..RES_GRID {
        for cx in 0..RES_GRID {
            for (kind, got) in [
                (ResourceKind::Ore, d.remaining(ResourceKind::Ore, cx, cz)),
                (ResourceKind::Oil, d.remaining(ResourceKind::Oil, cx, cz)),
            ] {
                let want = (terrain.resources().richness(kind, cx, cz) * per_cell).round() as u16;
                assert_eq!(got, want);
                any |= want > 0;
            }
        }
    }
    assert!(any);
}
