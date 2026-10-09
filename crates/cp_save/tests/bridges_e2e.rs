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
    Elevation, ElevationMode, NetCommand, NetGraph, RoadBuildError, RoadPath, RoadPathRequest,
    SnapOptions, pier_positions, validate_road_path,
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

/// An east-west path across the river at `z`.
fn crossing(w: &World, z: f32, elevation: ElevationMode) -> RoadPathRequest {
    let cx = river_x(z);
    RoadPathRequest {
        road: w.content.road_id("basic_2l").unwrap(),
        path: RoadPath::Straight {
            from: Vec2::new(cx - 160.0, z),
            to: Vec2::new(cx + 160.0, z),
        },
        elevation,
        snap: SnapOptions::default(),
    }
}

/// A `z` in the owned area where the crossing is valid as a bridge.
fn bridge_z(w: &World) -> f32 {
    let graph = NetGraph::new();
    let tiles = TileOwnership::m1_around((2, 2));
    (3000..7000)
        .step_by(40)
        .map(|z| z as f32)
        .find(|&z| {
            validate_road_path(
                &graph,
                &w.terrain,
                &tiles,
                &w.content,
                &crossing(w, z, ElevationMode::Elevated(10.0)),
            )
            .is_ok()
        })
        .expect("some crossing of the river is bridgeable")
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

#[test]
fn bridge_over_the_river_is_valid_and_a_ground_road_is_not() {
    let w = world();
    let z = bridge_z(&w);
    let graph = NetGraph::new();
    let tiles = TileOwnership::m1_around((2, 2));

    let plan = validate_road_path(
        &graph,
        &w.terrain,
        &tiles,
        &w.content,
        &crossing(&w, z, ElevationMode::Elevated(10.0)),
    )
    .unwrap();
    assert!(
        plan.segments
            .iter()
            .any(|s| s.elevation == Elevation::Bridge)
    );
    // Bridge pieces are exactly the ones over water, and cost the bridge price.
    let def = w.content.road(w.content.road_id("basic_2l").unwrap());
    for s in &plan.segments {
        let per_m = if s.elevation == Elevation::Bridge {
            def.bridge.as_ref().unwrap().cost_per_m
        } else {
            def.elevated.as_ref().unwrap().cost_per_m
        };
        assert_eq!(s.cost, cp_net::segment_cost(s.length, per_m));
    }

    let errors = validate_road_path(
        &graph,
        &w.terrain,
        &tiles,
        &w.content,
        &crossing(&w, z, ElevationMode::Ground),
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
fn elevated_over_a_road_makes_no_node_and_ground_over_a_road_does() {
    // Flat plains: the hills of river_valley are too steep for these short test roads.
    let w = world_of("flatlands");
    let mut s = sim(&w);
    let basic = w.content.road_id("basic_2l").unwrap();
    // A north-south ground road on dry land near the start tile, then roads across it.
    let (x, z0) = (5120.0f32, 5000.0f32);
    assert!(!w.terrain.is_water(Vec2::new(x, z0 + 50.0)));
    let build = |from: Vec2, to: Vec2, elevation| NetCommand::BuildRoad {
        road: basic,
        path: RoadPath::Straight { from, to },
        elevation,
    };
    let out = run(
        &mut s,
        build(
            Vec2::new(x, z0),
            Vec2::new(x, z0 + 100.0),
            ElevationMode::Ground,
        ),
    );
    assert!(out.result.is_ok(), "{out:?}");
    let ground_segments = s.world().resource::<NetGraph>().segment_count();
    let nodes = s.world().resource::<NetGraph>().node_count();

    let (a, b) = (
        Vec2::new(x - 45.0, z0 + 50.0),
        Vec2::new(x + 45.0, z0 + 50.0),
    );
    let out = run(&mut s, build(a, b, ElevationMode::Elevated(8.0)));
    assert!(out.result.is_ok(), "{out:?}");
    let g = s.world().resource::<NetGraph>();
    // Only the new deck's own end nodes were added; the road beneath is untouched.
    assert_eq!(g.node_count(), nodes + 2);
    // The road beneath keeps its own 2-way middle node; nothing joins it to the deck.
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
    assert!(g.segment_count() > ground_segments);

    // The same crossing at ground level splits the road and makes a 4-way junction.
    let out = run(
        &mut s,
        build(
            a + Vec2::new(0.0, 20.0),
            b + Vec2::new(0.0, 20.0),
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
    let z = bridge_z(&w);
    let mut s = sim(&w);
    let req = crossing(&w, z, ElevationMode::Elevated(10.0));
    let out = run(
        &mut s,
        NetCommand::BuildRoad {
            road: req.road,
            path: req.path,
            elevation: req.elevation,
        },
    );
    assert!(out.result.is_ok(), "{out:?}");

    let hash = w.content.content_hash();
    let bytes = save_runner(&s, hash, "river_valley").unwrap();
    let mut loaded = sim(&w);
    let (_, warnings) = load_into_runner(&mut loaded, &bytes, hash).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(loaded.checksum(), s.checksum());
    let b = loaded.world().resource::<NetGraph>();
    assert_eq!(s.world().resource::<NetGraph>(), b);
    let bridges: Vec<_> = b
        .segments()
        .filter(|(_, g)| g.elevation == Elevation::Bridge)
        .collect();
    assert!(!bridges.is_empty());
    // Piers are derived from the loaded segments (nothing extra is stored).
    let long = b
        .segments()
        .any(|(_, g)| g.length > 50.0 && !pier_positions(g).is_empty());
    assert!(long || b.segments().all(|(_, g)| g.length <= 50.0));
}
