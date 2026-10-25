//! T-185: the road tool shows the zone cells the road will get, ticks every 8 cells and the
//! 8-cell block guides.
//!
//! * the preview is the cells a built road gets (same layout code);
//! * a node near the 8-cell guide snaps to it exactly (64 m + the half widths), `Alt` does not;
//! * a block built on the guides is one clean 8 x 8 grid, no gaps, no overlap.
//!
//! Manual check: `--scene zones` is the BUG-043 scene; for this one pick the road tool and
//! hover beside a street: the translucent band is the future zone, yellow ticks mark every 64 m.

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_content::ContentDb;
use cp_core::CommandOutcome;
use cp_net::{NetCommand, RoadPath};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_tools::{CommandOutbox, RoadMode, RoadTool, compute_ghost};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn sim() -> (SimRunner, Arc<ContentDb>) {
    let db = Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let t = Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::new(1);
    sim.set_terrain(t);
    sim.set_content(Arc::clone(&db));
    sim.tick_once();
    (sim, db)
}

fn tool(db: &ContentDb, from: Vec2) -> RoadTool {
    RoadTool {
        mode: RoadMode::Straight,
        road: Some(db.road_id("basic_2l").unwrap()),
        points: vec![from],
        ..Default::default()
    }
}

fn build(sim: &mut SimRunner, db: &ContentDb, from: Vec2, to: Vec2) {
    build_road(sim, db, "basic_2l", from, to);
}

fn build_road(sim: &mut SimRunner, db: &ContentDb, road: &str, from: Vec2, to: Vec2) {
    let mut outbox = CommandOutbox::default();
    outbox.push(SimCommand::Net(NetCommand::BuildRoad {
        road: db.road_id(road).unwrap(),
        path: RoadPath::Straight { from, to },
        elevation: cp_net::ElevationMode::Ground,
    }));
    outbox.flush_into(sim);
    sim.tick_once();
    let out: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    assert!(out.last().unwrap().result.is_ok(), "{out:?}");
    sim.tick_once();
}

/// Valid cells of every block, as corner lists rounded to centimetres, sorted.
fn valid_cells(sim: &SimRunner) -> Vec<[(i32, i32); 4]> {
    let view = sim.view();
    let mut out = Vec::new();
    for (id, b) in view.zones.blocks() {
        for (i, ok) in b.valid.iter().enumerate() {
            if *ok {
                let o = view
                    .zones
                    .cell_footprint(cp_zone::ZoneCellRef {
                        block: id,
                        cell: i as u16,
                    })
                    .unwrap();
                out.push(round(o.corners()));
            }
        }
    }
    out.sort();
    out
}

fn round(c: [Vec2; 4]) -> [(i32, i32); 4] {
    c.map(|p| ((p.x * 50.0).round() as i32, (p.y * 50.0).round() as i32))
}

#[test]
fn the_preview_cells_are_the_cells_a_built_road_gets() {
    let (mut sim, db) = sim();
    // 160 m: two whole sections (T-275), so the snapped ghost and the road built to the same
    // point are one road.
    let (a, b) = (Vec2::new(4600.0, 4600.0), Vec2::new(4600.0 + 160.0, 4600.0));
    let view = sim.view();
    let ghost = compute_ghost(&view, &tool(&db, a), b).expect("a ghost");
    assert!(ghost.valid, "{:?}", ghost.errors);
    let mut preview: Vec<_> = ghost
        .zone
        .cells
        .iter()
        .filter(|c| c.valid)
        .map(|c| round(c.corners))
        .collect();
    preview.sort();
    assert!(preview.len() > 100, "{}", preview.len());
    drop(view);
    build(&mut sim, &db, a, b);
    assert_eq!(valid_cells(&sim), preview);
    // Ticks: the section mark in the middle (80 m from either end, T-275), on each side.
    assert_eq!(ghost.zone.ticks.len(), 2);
}

/// Four streets 64 m + a road width apart (64 m between the road edges) cross and enclose one 8 x 8
/// grid of cells: the four blocks' cells tile the square by rank, none doubled, none missing.
#[test]
fn a_block_built_on_the_guides_is_one_clean_8_by_8_grid() {
    let (mut sim, db) = sim();
    let o = Vec2::new(4600.0, 4600.0);
    let hw = db.road(db.road_id("basic_2l").unwrap()).width / 2.0;
    let s = 64.0 + 2.0 * hw;
    let sx = s;
    // Two streets each way, crossing: the 64 m between the road edges are four columns of 8.
    let e = 30.0;
    for k in [0.0, s] {
        build(
            &mut sim,
            &db,
            o + Vec2::new(-e, k),
            o + Vec2::new(sx + e, k),
        );
    }
    for k in [0.0, sx] {
        build(&mut sim, &db, o + Vec2::new(k, -e), o + Vec2::new(k, s + e));
    }
    let view = sim.view();
    let mut inside: Vec<Vec2> = Vec::new();
    for (id, b) in view.zones.blocks() {
        for (i, ok) in b.valid.iter().enumerate() {
            let c = view
                .zones
                .cell_footprint(cp_zone::ZoneCellRef {
                    block: id,
                    cell: i as u16,
                })
                .unwrap();
            let r = c.c - o;
            if *ok && r.x > hw && r.x < sx - hw && r.y > hw && r.y < s - hw {
                inside.push(r);
            }
        }
    }
    let mut dbg: Vec<(i32, i32)> = inside
        .iter()
        .map(|r| (r.x.round() as i32, r.y.round() as i32))
        .collect();
    dbg.sort();
    assert_eq!(inside.len(), 64, "an 8 x 8 grid: {dbg:?}");
    // Aligned: every centre sits on the 8 m lattice that starts at the road edge.
    for r in &inside {
        let (fx, fz) = ((r.x - hw - 4.0) / 8.0, (r.y - hw - 4.0) / 8.0);
        assert!(
            (fx - fx.round()).abs() < 0.01 && (fz - fz.round()).abs() < 0.01,
            "{r:?} is off the lattice"
        );
    }
    inside.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    inside.dedup_by(|a, b| a.distance(*b) < 0.01);
    assert_eq!(inside.len(), 64, "no two cells on top of each other");
}

/// BUG-073: in an L the ticks of the vertical street into the corner count from the bottom
/// street's EDGE, like the ticks of a street that leaves it: 64 m from the edge, on both.
#[test]
fn ticks_of_a_street_into_a_corner_count_from_the_cross_streets_edge() {
    for road in ["basic_2l", "medium_4l"] {
        for len in [300.0f32, 311.0, 337.5] {
            let (mut sim, db) = sim();
            let hw = db.road(db.road_id(road).unwrap()).width / 2.0;
            let o = Vec2::new(4600.0, 4600.0);
            build_road(&mut sim, &db, road, o + Vec2::new(0.0, -len), o);
            build_road(&mut sim, &db, road, o, o + Vec2::new(200.0, 0.0));
            build_road(
                &mut sim,
                &db,
                road,
                o + Vec2::new(200.0, 0.0),
                o + Vec2::new(200.0, -150.0),
            );
            let view = sim.view();
            let roads = cp_zone::road_zoning(&db);
            // Distances of the major marks from the bottom street's near edge, per vertical street.
            let mut ticks: Vec<Vec<f32>> = Vec::new();
            for x in [4600.0f32, 4800.0] {
                let mut d: Vec<f32> = Vec::new();
                for (id, g) in view.net.segments() {
                    if (g.curve.p0.x - x).abs() < 0.1 && (g.curve.p3.x - x).abs() < 0.1 {
                        for side in view.zones.frontage_marks(&view.net, &roads, id) {
                            d.extend(
                                side.iter()
                                    .filter(|m| m.major && !m.end)
                                    .map(|m| (4600.0 - hw) - m.axis.y),
                            );
                        }
                    }
                }
                d.sort_by(f32::total_cmp);
                d.dedup_by(|a, b| (*a - *b).abs() < 0.05);
                ticks.push(d);
            }
            assert!(
                ticks[0].len() >= 2
                    && (ticks[0][0] - 64.0).abs() < 0.05
                    && (ticks[1][0] - 64.0).abs() < 0.05,
                "{road} len {len}: ticks from the corner edge {ticks:?}"
            );
        }
    }
}
