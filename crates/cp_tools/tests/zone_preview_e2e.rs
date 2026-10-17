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
use cp_tools::{CommandOutbox, RoadMode, RoadTool, compute_ghost, compute_ghost_alt};

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
    let (a, b) = (Vec2::new(4600.0, 4600.0), Vec2::new(4600.0 + 120.0, 4600.0));
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
    // Ticks: one every 8 cells (64 m) on each side, none before the first 64 m.
    assert_eq!(ghost.zone.ticks.len(), 2);
}

#[test]
fn a_node_near_the_8_cell_guide_snaps_to_it_and_alt_does_not() {
    let (mut sim, db) = sim();
    // An east-west street; the guide is 64 m of block between its edge and the new road's:
    // 64 + 6 + 6 m between the axes (basic_2l is 12 m wide).
    build(
        &mut sim,
        &db,
        Vec2::new(4500.0, 4600.0),
        Vec2::new(4620.0, 4600.0),
    );
    let view = sim.view();
    let from = Vec2::new(4520.0, 4676.0);
    let near = Vec2::new(4600.0, 4679.0);
    let g = compute_ghost_alt(&view, &tool(&db, from), near, false).expect("a ghost");
    let guide = g.block.as_ref().expect("a guide");
    assert!(guide.snapped && guide.note == "block 8 cells", "{guide:?}");
    assert!((guide.cursor.y - 4676.0).abs() < 1e-3, "{:?}", guide.cursor);
    let end = g.polylines.iter().flatten().last().unwrap();
    assert!((end.z - 4676.0).abs() < 0.01, "ends at z {}", end.z);
    // Alt: no guide, the node stays where the cursor is.
    let off = compute_ghost_alt(&view, &tool(&db, from), near, true).unwrap();
    assert!(off.block.is_none());
    // A partial block says so and does not snap.
    let partial = compute_ghost_alt(
        &view,
        &tool(&db, Vec2::new(4520.0, 4660.0)),
        Vec2::new(4600.0, 4660.0),
        false,
    )
    .unwrap();
    let guide = partial.block.expect("a guide");
    assert!(
        !guide.snapped && guide.note.contains("partial"),
        "{guide:?}"
    );
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

/// BUG-047: every tick of the ghost stands on a column boundary of its cells, 8 cells (64 m) apart.
#[test]
fn the_ticks_are_8_real_cells_apart_and_on_cell_boundaries() {
    let (sim, db) = sim();
    let (a, b) = (Vec2::new(4600.0, 4600.0), Vec2::new(4800.0, 4600.0));
    let view = sim.view();
    let ghost = compute_ghost(&view, &tool(&db, a), b).expect("a ghost");
    assert!(ghost.valid, "{:?}", ghost.errors);
    // 200 m open street: 25 columns, boundaries 8, 16 and 24 on each side.
    assert_eq!(ghost.zone.ticks.len(), 6, "{:?}", ghost.zone.ticks);
    let corners: Vec<Vec2> = ghost.zone.cells.iter().flat_map(|c| c.corners).collect();
    let mut xs: Vec<f32> = Vec::new();
    for t in &ghost.zone.ticks {
        assert!(
            corners.iter().any(|c| c.distance(t[0]) <= 0.05),
            "tick {t:?} is not on a cell corner"
        );
        if t[0].y < 4600.0 {
            xs.push(t[0].x);
        }
    }
    xs.sort_by(f32::total_cmp);
    for w in xs.windows(2) {
        assert!((w[1] - w[0] - 64.0).abs() < 0.05, "{xs:?}");
    }
}

fn tool_of(db: &ContentDb, road: &str, from: Vec2) -> RoadTool {
    RoadTool {
        mode: RoadMode::Straight,
        road: Some(db.road_id(road).unwrap()),
        points: vec![from],
        ..Default::default()
    }
}

/// BUG-067: whatever the road type and length, the ticks of a ghost are 8 cells apart: eight
/// cells of the row beside the road lie between two neighbouring ticks.
#[test]
fn every_zonable_road_type_has_8_cells_between_ticks_at_every_length() {
    let (sim, db) = sim();
    let view = sim.view();
    let a = Vec2::new(4600.0, 4600.0);
    let mut bad: Vec<String> = Vec::new();
    for road in [
        "basic_2l",
        "basic_1w_2l",
        "medium_4l",
        "medium_1w_3l",
        "large_6l",
    ] {
        let hw = db.road(db.road_id(road).unwrap()).width / 2.0;
        let mut len = 120.0f32;
        while len <= 330.0 {
            let ghost = compute_ghost(&view, &tool_of(&db, road, a), a + Vec2::new(len, 0.0));
            let Some(ghost) = ghost else {
                len += 0.5;
                continue;
            };
            for side in [-1.0f32, 1.0] {
                let mut ticks: Vec<f32> = ghost
                    .zone
                    .ticks
                    .iter()
                    .filter(|t| (t[0].y - a.y) * side > 0.0)
                    .map(|t| t[0].x)
                    .collect();
                ticks.sort_by(f32::total_cmp);
                // The row beside the road: cells whose centre is within a cell and a half of the edge.
                let mut xs: Vec<f32> = ghost
                    .zone
                    .cells
                    .iter()
                    .map(|c| (c.corners[0] + c.corners[1] + c.corners[2] + c.corners[3]) / 4.0)
                    .filter(|m| (m.y - a.y) * side > hw && (m.y - a.y).abs() < hw + 6.0)
                    .map(|m| m.x)
                    .collect();
                xs.sort_by(f32::total_cmp);
                for w in ticks.windows(2) {
                    let n = xs.iter().filter(|x| **x > w[0] && **x < w[1]).count();
                    if n != 8 {
                        bad.push(format!(
                            "{road} len {len} side {side}: {n} cells between ticks {w:?}"
                        ));
                    }
                }
            }
            len += 0.5;
        }
    }
    assert!(
        bad.is_empty(),
        "{} cases, first: {:?}",
        bad.len(),
        &bad[..bad.len().min(6)]
    );
}

/// BUG-067 (part 2): a street drawn across a block from a column boundary of one facing street
/// to the other ends on the matching boundary, at a right angle, whatever the block's size:
/// the boundaries of both streets sit on the block's lattice, the join takes them.
#[test]
fn a_street_across_a_block_from_mark_to_mark_is_perpendicular() {
    use cp_net::{ElevationMode, RoadPathRequest, SnapOptions};
    for (gx, gz) in [
        (128.0f32, 128.0f32),
        (96.0, 128.0),
        (128.0, 96.0),
        (112.0, 112.0),
    ] {
        let (mut sim, db) = sim();
        let hw = db.road(db.road_id("basic_2l").unwrap()).width / 2.0;
        let o = Vec2::new(4600.0, 4600.0);
        let (x1, z1) = (gx + 2.0 * hw, gz + 2.0 * hw);
        for (a, b) in [
            (o, o + Vec2::new(x1, 0.0)),
            (o + Vec2::new(0.0, z1), o + Vec2::new(x1, z1)),
            (o, o + Vec2::new(0.0, z1)),
            (o + Vec2::new(x1, 0.0), o + Vec2::new(x1, z1)),
        ] {
            build(&mut sim, &db, a, b);
        }
        let view = sim.view();
        // From the middle of the left street (the cursor within 3 m of the boundary) to the
        // right street, the cursor 4 m off the matching boundary.
        let mid = o.y + hw + (gz / 2.0 / 8.0).round() * 8.0;
        let req = RoadPathRequest {
            road: db.road_id("basic_2l").unwrap(),
            path: RoadPath::Straight {
                from: Vec2::new(o.x + 1.0, mid + 3.0),
                to: Vec2::new(o.x + x1 - 1.0, mid - 4.0),
            },
            elevation: ElevationMode::Ground,
            snap: SnapOptions::default(),
        };
        let (q, _) = cp_tools::quantized(&view, req, false);
        let RoadPath::Straight { from, to } = q.path else {
            panic!("straight");
        };
        let dir = to - from;
        let off_axis = dir.y.atan2(dir.x).to_degrees().abs();
        assert!(
            off_axis < 0.5,
            "block {gx} x {gz}: the street is {off_axis:.2} degrees off the perpendicular: {from:?} -> {to:?}"
        );
        // On a column boundary of the cells beside the left street: some cell corner there.
        let corners: Vec<Vec2> = view
            .zones
            .cells_in_rect(
                Vec2::new(o.x + hw, o.y + hw),
                Vec2::new(o.x + hw + 8.0, o.y + hw + gz),
            )
            .into_iter()
            .filter_map(|c| view.zones.cell_footprint(c))
            .flat_map(|f| f.corners())
            .collect();
        assert!(
            corners
                .iter()
                .any(|c| (c.y - from.y).abs() < 0.1 || ((c.y - from.y).abs() - hw).abs() < 0.1),
            "{from:?} is not a cell boundary (or an edge on one) of the left street"
        );
        // And on one of the right street.
        let corners: Vec<Vec2> = view
            .zones
            .cells_in_rect(
                Vec2::new(o.x + x1 - hw - 8.0, o.y + hw),
                Vec2::new(o.x + x1 - hw, o.y + hw + gz),
            )
            .into_iter()
            .filter_map(|c| view.zones.cell_footprint(c))
            .flat_map(|f| f.corners())
            .collect();
        assert!(
            corners
                .iter()
                .any(|c| (c.y - to.y).abs() < 0.1 || ((c.y - to.y).abs() - hw).abs() < 0.1),
            "{to:?} is not a cell boundary (or an edge on one) of the right street"
        );
    }
}

/// BUG-067: a cross street joined to a boundary of a street's cells is placed with its near
/// EDGE on the boundary, so the block between it and the previous corner is exactly 8 cells, for
/// every road width of the street and of the cross street.
#[test]
fn a_cross_street_on_a_tick_leaves_exactly_8_cells_before_it() {
    use cp_net::{ElevationMode, RoadPathRequest, SnapOptions};
    for street in ["basic_2l", "medium_4l", "large_6l"] {
        for cross in ["basic_2l", "medium_4l"] {
            let (mut sim, db) = sim();
            let hw_s = db.road(db.road_id(street).unwrap()).width / 2.0;
            let hw_c = db.road(db.road_id(cross).unwrap()).width / 2.0;
            let hw_v = db.road(db.road_id("basic_2l").unwrap()).width / 2.0;
            // A north-south street at x = 4600 and the street east of it, from its edge on.
            build(
                &mut sim,
                &db,
                Vec2::new(4600.0, 4400.0),
                Vec2::new(4600.0, 4800.0),
            );
            build_road(
                &mut sim,
                &db,
                street,
                Vec2::new(4600.0, 4600.0),
                Vec2::new(4900.0, 4600.0),
            );
            let view = sim.view();
            let roads = cp_zone::road_zoning(&db);
            let mut north: Vec<cp_zone::Mark> = Vec::new();
            for (id, g) in view.net.segments() {
                let on_street = (g.curve.p0.z - 4592.0).abs() < 0.1
                    && (g.curve.p3.z - 4592.0).abs() < 0.1
                    && g.curve.p3.x.max(g.curve.p0.x) > 4600.5;
                if on_street {
                    for side in view.zones.frontage_marks(&view.net, &roads, id) {
                        north.extend(side.into_iter().filter(|m| m.v.y < 0.0));
                    }
                }
            }
            north.sort_by(|a, b| a.axis.x.total_cmp(&b.axis.x));
            north.dedup_by(|a, b| (a.axis.x - b.axis.x).abs() < 0.05);
            let tick = north[8];
            assert!(tick.major, "the 8th boundary is a tick");
            let req = RoadPathRequest {
                road: db.road_id(cross).unwrap(),
                path: RoadPath::Straight {
                    from: Vec2::new(tick.axis.x + 1.5, 4592.0 - hw_s - 2.0),
                    to: Vec2::new(tick.axis.x + 1.5, 4480.0),
                },
                elevation: ElevationMode::Ground,
                snap: SnapOptions::default(),
            };
            let (q, _) = cp_tools::quantized(&view, req, false);
            let RoadPath::Straight { from, to } = q.path else {
                panic!("straight");
            };
            assert!(
                (from.x - (tick.axis.x + hw_c)).abs() < 0.05 && (to.x - from.x).abs() < 0.05,
                "{street}/{cross}: the cross street is at {from:?} -> {to:?}, the tick at {:?} (half width {hw_c})",
                tick.axis
            );
            // Build it and count the cells of the north row between the street's edge and it.
            build_road(&mut sim, &db, cross, Vec2::new(from.x, 4592.0), to);
            let view = sim.view();
            let cells = view
                .zones
                .cells_in_rect(
                    Vec2::new(4600.0 + hw_v, 4592.0 - hw_s - 12.0),
                    Vec2::new(from.x - hw_c + 0.01, 4592.0 - hw_s),
                )
                .into_iter()
                .filter_map(|c| view.zones.cell_footprint(c).map(|f| (c, f)))
                .filter(|(c, f)| {
                    view.zones.is_valid(*c) && (f.c.y - (4592.0 - hw_s - 4.0)).abs() < 3.0
                })
                .count();
            assert_eq!(
                cells, 8,
                "{street}/{cross}: {cells} cells between the corner and the cross street"
            );
        }
    }
}
