//! T-022 acceptance on the committed `river_valley` map: a bridge over the river is
//! valid and a ground road over it is not; an elevated road over a road creates no node;
//! the result survives save and load.

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_content::ContentDb;
use cp_core::{CommandOutcome, Tick};
use cp_net::{
    Elevation, ElevationMode, NetCommand, NetGraph, NodeId, RoadBuildError, RoadPath,
    RoadPathRequest, SnapOptions, pier_positions, validate_road_path,
};
use cp_save::{load_into_runner, save_runner};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::{Terrain, TileOwnership};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

struct World {
    content: Arc<ContentDb>,
    terrain: Arc<Terrain>,
}

fn world() -> World {
    world_of("river_valley")
}

fn world_of(map: &str) -> World {
    let root = Path::new(ROOT).join("../..");
    World {
        content: Arc::new(ContentDb::load_dir(root.join("assets/content")).unwrap()),
        terrain: Arc::new(Terrain::load(&root.join("assets/maps").join(map)).unwrap()),
    }
}

fn sim(w: &World) -> SimRunner {
    let mut s = SimRunner::new(3);
    s.set_terrain(Arc::clone(&w.terrain));
    s.set_content(Arc::clone(&w.content));
    s
}

/// River centre line x at `z` (the generator's formula, `tools/mapgen`).
fn river_x(z: f32) -> f32 {
    5120.0 + 1400.0 * (z / 1900.0).sin() + 500.0 * (z / 700.0 + 1.3).sin()
}

fn run(sim: &mut SimRunner, cmd: NetCommand) -> CommandOutcome {
    let tick = sim.tick();
    sim.push_command(SimCommand::Net(cmd), Tick(tick.0));
    sim.tick_once();
    sim.world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .next()
        .unwrap()
}

fn build(
    road: cp_content::RoadDefId,
    from: Vec2,
    to: Vec2,
    elevation: ElevationMode,
) -> NetCommand {
    NetCommand::BuildRoad {
        road,
        path: RoadPath::Straight { from, to },
        elevation,
    }
}

const H: f32 = 10.0;

/// A street on each bank joined by an elevated bridge over the river. Searches the
/// valley for a crossing where the terrain allows it; returns the sim and the two
/// street-end nodes that the bridge snapped to.
fn bridged_valley(w: &World) -> (SimRunner, Vec2, Vec2) {
    let basic = w.content.road_id("basic_2l").unwrap();
    for z in (3400..6800).step_by(40).map(|z| z as f32) {
        for off in [170.0f32, 200.0, 230.0] {
            let cx = river_x(z);
            let (xw, xe) = (cx - off, cx + off);
            let mut s = sim(w);
            let streets = [xw, xe].map(|x| {
                run(
                    &mut s,
                    build(
                        basic,
                        Vec2::new(x, z - 50.0),
                        Vec2::new(x, z + 50.0),
                        ElevationMode::Ground,
                    ),
                )
                .result
                .is_ok()
            });
            if !streets.iter().all(|&ok| ok) {
                continue;
            }
            let bridge = run(
                &mut s,
                build(
                    basic,
                    Vec2::new(xw + 3.0, z + 3.0),
                    Vec2::new(xe - 3.0, z - 3.0),
                    ElevationMode::Elevated(H),
                ),
            );
            if bridge.result.is_ok() {
                return (s, Vec2::new(xw, z), Vec2::new(xe, z));
            }
        }
    }
    panic!("no bridgeable crossing found on river_valley");
}

fn node_at(g: &NetGraph, p: Vec2) -> NodeId {
    g.nodes()
        .find(|(_, n)| {
            Vec2::new(n.pos.x, n.pos.z).distance(p) < 1.0 && n.elevation == Elevation::Ground
        })
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no ground node at {p}"))
}

/// Nodes reachable from `start` over segments.
fn component(g: &NetGraph, start: NodeId) -> Vec<NodeId> {
    let mut seen = vec![start];
    let mut i = 0;
    while i < seen.len() {
        for &sid in &g.node(seen[i]).unwrap().segments {
            let seg = g.segment(sid).unwrap();
            for n in [seg.start, seg.end] {
                if !seen.contains(&n) {
                    seen.push(n);
                }
            }
        }
        i += 1;
    }
    seen
}

#[test]
fn bridge_over_the_river_is_valid_and_a_ground_road_is_not() {
    let w = world();
    let (s, west, _) = bridged_valley(&w);
    // The bridge pieces exist and are priced as bridges; the ground path is not valid.
    let g = s.world().resource::<NetGraph>();
    assert!(g.segments().any(|(_, g)| g.elevation == Elevation::Bridge));
    let tiles = TileOwnership::m1_around((2, 2));
    let cx = west.x;
    let errors = validate_road_path(
        &NetGraph::new(),
        &w.terrain,
        &tiles,
        &w.content,
        &RoadPathRequest {
            road: w.content.road_id("basic_2l").unwrap(),
            path: RoadPath::Straight {
                from: Vec2::new(cx, west.y),
                to: Vec2::new(cx + 400.0, west.y),
            },
            elevation: ElevationMode::Ground,
            snap: SnapOptions::default(),
        },
    )
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, RoadBuildError::WaterWithoutBridge { .. })),
        "{errors:?}"
    );
}

#[test]
fn the_bridge_joins_both_banks_and_ramps_within_max_slope() {
    let w = world();
    let (s, west, east) = bridged_valley(&w);
    let g = s.world().resource::<NetGraph>();
    let (a, b) = (node_at(g, west), node_at(g, east));
    assert!(
        component(g, a).contains(&b),
        "banks are connected over the bridge"
    );

    let max = w
        .content
        .road(w.content.road_id("basic_2l").unwrap())
        .max_slope;
    let mut bridges = 0;
    let mut top: f32 = 0.0;
    for (_, seg) in g.segments() {
        let (p0, p3) = (seg.curve.p0, seg.curve.p3);
        if seg.elevation == Elevation::Ground {
            continue;
        }
        let run = Vec2::new(p3.x - p0.x, p3.z - p0.z).length();
        assert!((p3.y - p0.y).abs() / run <= max + 1e-4, "{seg:?}");
        bridges += (seg.elevation == Elevation::Bridge) as usize;
        for p in [p0, p3] {
            top = top.max(
                p.y - w.terrain.height(Vec2::new(p.x, p.z)).max(
                    w.terrain.height(Vec2::new(p.x, p.z))
                        + w.terrain.water_depth(Vec2::new(p.x, p.z)),
                ),
            );
        }
    }
    assert!(bridges >= 1);
    assert!(
        (top - H).abs() < 0.1,
        "deck reaches {H} m above ground/water, got {top}"
    );
    // The ground road beneath the approach is untouched: ends are ground nodes.
    assert_eq!(g.node(a).unwrap().elevation, Elevation::Ground);
}

#[test]
fn elevated_over_a_road_makes_no_node_and_ground_over_a_road_does() {
    // Flat plains: the hills of river_valley are too steep for these short test roads.
    let w = world_of("flatlands");
    let mut s = sim(&w);
    let basic = w.content.road_id("basic_2l").unwrap();
    // A north-south ground road on dry land near the start tile, then roads across it.
    let (x, z0) = (5120.0f32, 5000.0f32);
    assert!(!w.terrain.is_water(Vec2::new(x, z0 + 50.0)));
    let out = run(
        &mut s,
        build(
            basic,
            Vec2::new(x, z0),
            Vec2::new(x, z0 + 100.0),
            ElevationMode::Ground,
        ),
    );
    assert!(out.result.is_ok(), "{out:?}");
    let nodes = s.world().resource::<NetGraph>().node_count();
    let ground_segments = s.world().resource::<NetGraph>().segment_count();

    // A 300 m deck (4 pieces, nodes 75 m apart: none above the road's middle node).
    let (a, b) = (
        Vec2::new(x - 140.0, z0 + 50.0),
        Vec2::new(x + 160.0, z0 + 50.0),
    );
    let out = run(&mut s, build(basic, a, b, ElevationMode::Elevated(4.0)));
    assert!(out.result.is_ok(), "{out:?}");
    let g = s.world().resource::<NetGraph>();
    assert_eq!(
        g.node_count(),
        nodes + 5,
        "only the deck's own nodes were added"
    );
    assert_eq!(g.segment_count(), ground_segments + 4);
    let under: Vec<_> = g
        .nodes()
        .filter(|(_, n)| Vec2::new(n.pos.x, n.pos.z).distance(Vec2::new(x, z0 + 50.0)) < 1.0)
        .collect();
    assert!(
        under
            .iter()
            .all(|(_, n)| n.elevation == Elevation::Ground && n.segments.len() == 2),
        "no junction under the deck"
    );
    assert!(g.nodes().all(|(_, n)| n.segments.len() < 3));

    // The same crossing at ground level splits the road and makes a 4-way junction.
    let out = run(
        &mut s,
        build(
            basic,
            a + Vec2::new(0.0, 20.0),
            a + Vec2::new(90.0, 20.0),
            ElevationMode::Ground,
        ),
    );
    assert!(out.result.is_ok(), "{out:?}");
    let out = run(
        &mut s,
        build(
            basic,
            Vec2::new(x - 45.0, z0 + 20.0),
            Vec2::new(x + 45.0, z0 + 20.0),
            ElevationMode::Ground,
        ),
    );
    assert!(out.result.is_ok(), "{out:?}");
    let g = s.world().resource::<NetGraph>();
    assert!(g.nodes().any(|(_, n)| n.segments.len() == 4));
}

#[test]
fn a_built_bridge_survives_save_and_load() {
    let w = world();
    let (mut s, west, east) = bridged_valley(&w);
    let hash = w.content.content_hash();
    let bytes = save_runner(&s, hash, "river_valley").unwrap();
    let mut loaded = sim(&w);
    let (_, warnings) = load_into_runner(&mut loaded, &bytes, hash).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(loaded.checksum(), s.checksum());
    let b = loaded.world().resource::<NetGraph>();
    assert_eq!(s.world().resource::<NetGraph>(), b);
    assert!(b.segments().any(|(_, g)| g.elevation == Elevation::Bridge));
    // Still one connected network after loading.
    assert!(component(b, node_at(b, west)).contains(&node_at(b, east)));
    // Piers are derived from the loaded segments (nothing extra is stored).
    assert!(
        b.segments()
            .any(|(_, g)| g.length > 50.0 && !pier_positions(g).is_empty())
    );
}
