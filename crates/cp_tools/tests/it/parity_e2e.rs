//! T-024: the ghost and the command agree. For every scenario the preview (the pure
//! `validate_road_path` against the `SimView` snapshot) and the `BuildRoad` command run through
//! `SimRunner` give the same verdict, and the same cost when it is valid. The scenarios double
//! as the Tester's manual walk-through (straight, curve, snap to node, snap to segment, slope
//! error, bridge).

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_content::{ContentDb, RoadDefId};
use cp_core::{CommandError, CommandOutcome, Cost, Tick};
use cp_net::{
    ElevationMode, NetCommand, NetGraph, RoadBuildError, RoadPath, RoadPathRequest, SnapOptions,
};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_tools::{CommandOutbox, RoadMode, RoadTool, format_money, pick_segment, plan_for, preview};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn content() -> Arc<ContentDb> {
    Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap())
}

fn terrain(map: &str) -> Arc<Terrain> {
    Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps").join(map)).unwrap())
}

fn sim(map: &str) -> (SimRunner, RoadDefId, Arc<Terrain>) {
    let db = content();
    let t = terrain(map);
    let mut sim = SimRunner::new(1);
    sim.set_terrain(Arc::clone(&t));
    sim.set_content(Arc::clone(&db));
    (sim, db.road_id("basic_2l").unwrap(), t)
}

fn req(road: RoadDefId, path: RoadPath, elevation: ElevationMode) -> RoadPathRequest {
    RoadPathRequest {
        road,
        path,
        elevation,
        snap: SnapOptions::default(),
    }
}

/// Sends `r` as the tool does (the `BuildPlan` command, T-264) and returns its outcome.
fn command(sim: &mut SimRunner, r: &RoadPathRequest) -> Result<Cost, CommandError> {
    let plan = plan_for(&sim.view(), r).expect("content present").0;
    let mut outbox = CommandOutbox::default();
    outbox.push(SimCommand::Net(NetCommand::BuildPlan { plan }));
    outbox.flush_into(sim);
    sim.tick_once();
    let outcomes: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    assert_eq!(outcomes.len(), 1);
    outcomes[0].result.clone()
}

/// Preview, then command; asserts they agree and returns the preview's verdict.
fn agree(sim: &mut SimRunner, r: &RoadPathRequest) -> Result<Cost, Vec<RoadBuildError>> {
    let view = sim.view();
    let pv = preview(&view, r).expect("terrain and content present");
    let outcome = command(sim, r);
    match (&pv, &outcome) {
        (Ok(plan), Ok(cost)) => assert_eq!(plan.cost, *cost, "preview cost equals command cost"),
        (Err(errs), Err(CommandError::Rejected { reason, .. })) => {
            for e in errs {
                assert!(reason.contains(&format!("{e:?}")), "{reason} lacks {e:?}");
            }
        }
        other => panic!("preview and command disagree: {other:?}"),
    }
    pv.map(|p| p.cost)
}

fn straight(a: (f32, f32), b: (f32, f32)) -> RoadPath {
    RoadPath::Straight {
        from: Vec2::new(a.0, a.1),
        to: Vec2::new(b.0, b.1),
    }
}

#[test]
fn straight_and_curve_previews_cost_what_the_command_costs() {
    let (mut s, road, _) = sim("flatlands");
    let c1 = agree(
        &mut s,
        &req(
            road,
            straight((4600.0, 4600.0), (4800.0, 4600.0)),
            ElevationMode::Ground,
        ),
    )
    .unwrap();
    assert!(c1.0 > 0);
    let curve = RoadPath::Curve {
        from: Vec2::new(4600.0, 4700.0),
        control: Vec2::new(4800.0, 4700.0),
        to: Vec2::new(4800.0, 4900.0),
    };
    let c2 = agree(&mut s, &req(road, curve, ElevationMode::Ground)).unwrap();
    assert!(c2.0 > 0);
    let free = RoadPath::Free(vec![
        Vec2::new(5000.0, 4600.0),
        Vec2::new(5100.0, 4650.0),
        Vec2::new(5200.0, 4600.0),
    ]);
    agree(&mut s, &req(road, free, ElevationMode::Ground)).unwrap();
    // The money string shown next to the ghost.
    assert!(format_money(c1).starts_with('$'));
}

#[test]
fn snapping_to_a_node_and_to_a_segment_is_part_of_both_results() {
    let (mut s, road, _) = sim("flatlands");
    agree(
        &mut s,
        &req(
            road,
            straight((4600.0, 4600.0), (4700.0, 4600.0)),
            ElevationMode::Ground,
        ),
    )
    .unwrap();
    let nodes_before = s.world().resource::<NetGraph>().node_count();
    // Ends 8 m from the existing end node (within the 12 m node snap): reuses that node.
    agree(
        &mut s,
        &req(
            road,
            straight((4708.0, 4690.0), (4700.0, 4600.0)),
            ElevationMode::Ground,
        ),
    )
    .unwrap();
    // The end point 8 m off the node is not a new node.
    assert!(s.world().resource::<NetGraph>().node_count() <= nodes_before + 1);
    // A road ending 3 m from the axis of an existing segment splits it (segment snap).
    let segs_before = s.world().resource::<NetGraph>().segment_count();
    agree(
        &mut s,
        &req(
            road,
            straight((4650.0, 4500.0), (4650.0, 4597.0)),
            ElevationMode::Ground,
        ),
    )
    .unwrap();
    assert!(
        s.world().resource::<NetGraph>().segment_count() > segs_before + 1,
        "the split adds a piece"
    );
}

fn is_land(t: &Terrain, p: Vec2) -> bool {
    !t.is_water(p)
}

#[test]
fn a_steep_road_is_flagged_by_both() {
    let (mut s, road, t) = sim("river_valley");
    let max = {
        let db = content();
        db.road(road).max_slope
    };
    // Scan the owned area for a 60 m run on land steeper than the road's limit.
    let mut found = None;
    // (T-180 softened the hills of the valley: the steep runs are the river banks and the
    // meadow borders, so look in both directions and just over the limit.)
    'scan: for zi in (2300..7800).step_by(20) {
        for xi in (2300..7800).step_by(20) {
            for (dx, dz) in [(60.0, 0.0), (0.0, 60.0)] {
                let (a, b) = (
                    Vec2::new(xi as f32, zi as f32),
                    Vec2::new(xi as f32 + dx, zi as f32 + dz),
                );
                if is_land(&t, a)
                    && is_land(&t, b)
                    && (t.height(a) - t.height(b)).abs() / 60.0 > max * 1.05
                {
                    found = Some((a, b));
                    break 'scan;
                }
            }
        }
    }
    let (a, b) = found.expect("the river valley has a steep slope");
    let r = req(
        road,
        RoadPath::Straight { from: a, to: b },
        ElevationMode::Ground,
    );
    let errs = agree(&mut s, &r).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| matches!(e, RoadBuildError::TooSteep { .. })),
        "{errs:?}"
    );
    assert!(errs.iter().any(|e| e.to_string().contains("steep")));
}

/// An east-west run with land at both ends and water in between: from a water point walk
/// left and right to the nearest land (within 150 m each way).
fn river_crossing(t: &Terrain) -> (Vec2, Vec2) {
    let mut found = None;
    'scan: for zi in (2300..7800).step_by(20) {
        let z = zi as f32;
        let mut x = 2300.0;
        while x < 7700.0 {
            let w = Vec2::new(x, z);
            if t.is_water(w) {
                let left = (1..=30)
                    .map(|k| x - k as f32 * 5.0)
                    .find(|lx| is_land(t, Vec2::new(*lx, z)));
                let right = (1..=30)
                    .map(|k| x + k as f32 * 5.0)
                    .find(|rx| is_land(t, Vec2::new(*rx, z)));
                if let (Some(l), Some(r)) = (left, right)
                    && r - l >= 40.0
                    && l > 2200.0
                    && r < 7900.0
                {
                    found = Some((Vec2::new(l - 8.0, z), Vec2::new(r + 8.0, z)));
                    break 'scan;
                }
            }
            x += 20.0;
        }
    }
    found.expect("the river valley has a river to cross")
}

#[test]
fn a_road_type_with_a_bridge_variant_crosses_the_river_with_a_bridge_piece() {
    let db = content();
    let t = terrain("river_valley");
    let (a, b) = river_crossing(&t);
    let mut built = 0;
    for (id, def) in db.roads() {
        if def.bridge.is_none() || def.elevated.is_none() {
            continue;
        }
        for level in 1..=cp_tools::MAX_ELEVATION_LEVEL {
            let (mut s, _, _) = sim("river_valley");
            let mut tool = RoadTool {
                road: Some(id),
                ..Default::default()
            };
            (0..level).for_each(|_| tool.raise());
            let r = req(id, RoadPath::Straight { from: a, to: b }, tool.elevation());
            if agree(&mut s, &r).is_ok() {
                let bridges = s
                    .world()
                    .resource::<NetGraph>()
                    .segments()
                    .filter(|(_, seg)| seg.elevation == cp_net::Elevation::Bridge)
                    .count();
                assert!(
                    bridges > 0,
                    "{} level {level}: no bridge piece over the water",
                    def.id
                );
                built += 1;
                break;
            }
        }
    }
    assert!(
        built > 0,
        "some road type bridges the river within the elevation levels"
    );
}

#[test]
fn water_needs_a_bridge_and_a_raised_road_gets_one() {
    let (mut s, road, t) = sim("river_valley");
    let (a, b) = river_crossing(&t);
    let ground = req(
        road,
        RoadPath::Straight { from: a, to: b },
        ElevationMode::Ground,
    );
    let errs = agree(&mut s, &ground).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| matches!(e, RoadBuildError::WaterWithoutBridge { .. })),
        "{errs:?}"
    );
    // The same path raised: PgUp once in the tool. Whether it is buildable depends on the road's
    // elevated variant; both sides must agree either way.
    let mut tool = RoadTool {
        mode: RoadMode::Straight,
        road: Some(road),
        ..Default::default()
    };
    tool.raise();
    let raised = req(
        road,
        RoadPath::Straight { from: a, to: b },
        tool.elevation(),
    );
    let verdict = agree(&mut s, &raised);
    if let Err(errs) = &verdict {
        // basic_2l may have no bridge variant: then the error is the explicit one.
        assert!(
            errs.iter().all(|e| matches!(
                e,
                RoadBuildError::NoBridgeVariant(_)
                    | RoadBuildError::NoElevatedVariant(_)
                    | RoadBuildError::RampTooShort { .. }
                    | RoadBuildError::RampStartsOverWater { .. }
            )),
            "{errs:?}"
        );
    }
}

#[test]
fn an_elevated_road_over_land_previews_like_it_builds() {
    let db = content();
    let mut tried = 0;
    // Every road type that ships an elevated variant, each in a fresh world.
    for (id, def) in db.roads() {
        if def.elevated.is_some() {
            let (mut s, _, _) = sim("flatlands");
            let r = req(
                id,
                straight((4600.0, 5000.0), (4900.0, 5000.0)),
                ElevationMode::Elevated(5.0),
            );
            agree(&mut s, &r).unwrap();
            tried += 1;
        }
    }
    assert!(tried > 0, "some road type has an elevated variant");
}

#[test]
fn a_tile_that_is_not_owned_is_reported_with_its_text() {
    let (mut s, road, _) = sim("flatlands");
    let r = req(
        road,
        straight((100.0, 100.0), (300.0, 100.0)),
        ElevationMode::Ground,
    );
    let errs = agree(&mut s, &r).unwrap_err();
    assert!(
        errs.iter().any(|e| e.to_string() == "tile not owned"),
        "{errs:?}"
    );
}

#[test]
fn the_bulldozer_removes_the_segment_it_highlights() {
    let (mut s, road, _) = sim("flatlands");
    agree(
        &mut s,
        &req(
            road,
            straight((4600.0, 4600.0), (4690.0, 4600.0)),
            ElevationMode::Ground,
        ),
    )
    .unwrap();
    let view = s.view();
    let seg = pick_segment(&view.net, Vec2::new(4650.0, 4603.0), 8.0).expect("under the cursor");
    let mut outbox = CommandOutbox::default();
    outbox.push(SimCommand::Net(NetCommand::RemoveSegment { seg }));
    outbox.flush_into(&mut s);
    s.tick_once();
    assert_eq!(s.world().resource::<NetGraph>().segment_count(), 0);
    let _ = Tick(0);
}
