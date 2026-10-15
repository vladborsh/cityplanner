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
            .filter(|(_, n)| n.kind == NodeKind::Outside)
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
