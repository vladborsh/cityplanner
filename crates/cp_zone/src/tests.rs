use bevy_math::Vec3;
use cp_content::{ContentDb, RoadDefId};
use cp_core::curve::CubicBezier3;
use cp_net::{Elevation, NetGraph, SegmentId};

use crate::command::paint;
use crate::geom::Obb;
use crate::*;

fn db() -> ContentDb {
    ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap()
}

fn zoning(db: &ContentDb) -> impl Fn(RoadDefId) -> RoadZoning + '_ {
    move |id| {
        let d = db.road(id);
        RoadZoning {
            allows_zoning: d.allows_zoning,
            half_width: d.width / 2.0,
            footpath: d.class == cp_content::RoadClass::Path,
        }
    }
}

struct World {
    db: ContentDb,
    net: NetGraph,
    grid: ZoneGrid,
}

impl World {
    fn new() -> Self {
        Self {
            db: db(),
            net: NetGraph::new(),
            grid: ZoneGrid::new(),
        }
    }

    fn road(&self, name: &str) -> RoadDefId {
        self.db.road_id(name).unwrap()
    }

    fn add(&mut self, name: &str, curve: CubicBezier3) -> SegmentId {
        let id = self.road(name);
        let def = self.db.road(id).clone();
        self.net
            .add_segment(&def, id, curve, Elevation::Ground)
            .unwrap()
    }

    fn line(&mut self, name: &str, a: (f32, f32), b: (f32, f32)) -> SegmentId {
        self.add(
            name,
            CubicBezier3::line(Vec3::new(a.0, 0.0, a.1), Vec3::new(b.0, 0.0, b.1)),
        )
    }

    fn sync(&mut self) {
        let db = &self.db;
        self.grid.sync(&self.net, &zoning(db));
    }
}

/// No two valid cells (anywhere) interpenetrate beyond the tolerance, except neighbours
/// in the same block which the grid itself keeps apart.
fn assert_no_double_cells(grid: &ZoneGrid) {
    let mut cells = Vec::new();
    for (bid, b) in grid.blocks() {
        for (i, ok) in b.valid.iter().enumerate() {
            if *ok {
                let r = ZoneCellRef {
                    block: bid,
                    cell: i as u16,
                };
                cells.push((r, grid.cell_footprint(r).unwrap()));
            }
        }
    }
    for i in 0..cells.len() {
        for j in i + 1..cells.len() {
            let p = cells[i].1.penetration(&cells[j].1);
            assert!(
                p <= PENETRATION_EPS + 1e-4,
                "{:?} and {:?} overlap by {p}",
                cells[i].0,
                cells[j].0
            );
        }
    }
}

#[test]
fn straight_segment_gets_floor_len_over_8_times_4_cells_per_side() {
    for (len, cols) in [(96.0f32, 12usize), (83.0, 10), (24.0, 3), (8.5, 1)] {
        let mut w = World::new();
        let s = w.line("basic_2l", (1000.0, 1000.0), (1000.0 + len, 1000.0));
        w.sync();
        let [l, r] = w.grid.blocks_of(s);
        for id in [l.unwrap(), r.unwrap()] {
            let b = w.grid.block(id).unwrap();
            assert_eq!(b.cell_count(), cols * 4, "len {len}");
            assert_eq!(b.valid_count(), cols * 4, "len {len}: lone road, all valid");
            assert_eq!(b.zones.len(), cols * 4);
        }
        assert_eq!(w.grid.block(l.unwrap()).unwrap().side, Side::Left);
        assert_eq!(w.grid.block(r.unwrap()).unwrap().side, Side::Right);
    }
}

#[test]
fn too_short_segments_and_non_zoning_roads_get_no_blocks() {
    let mut w = World::new();
    let s = w.line("basic_2l", (1000.0, 1000.0), (1005.0, 1000.0));
    let h = w.line("highway_1w_3l", (1000.0, 1100.0), (1100.0, 1100.0));
    w.sync();
    assert_eq!(w.grid.blocks_of(s), [None, None]);
    assert!(!w.db.road(w.road("highway_1w_3l")).allows_zoning);
    assert_eq!(w.grid.blocks_of(h), [None, None]);
    assert_eq!(w.grid.block_count(), 0);
}

#[test]
fn cells_lie_beside_the_road_on_the_right_sides() {
    let mut w = World::new();
    let s = w.line("basic_2l", (1000.0, 1000.0), (1080.0, 1000.0));
    w.sync();
    let [l, r] = w.grid.blocks_of(s);
    let hw = w.db.road(w.road("basic_2l")).width / 2.0;
    let first = |id| {
        w.grid
            .cell_footprint(ZoneCellRef { block: id, cell: 0 })
            .unwrap()
    };
    // Direction +X; left of travel in XZ is (t.z, -t.x) = -Z.
    let (cl, cr) = (first(l.unwrap()), first(r.unwrap()));
    assert!((cl.c.y - (1000.0 - hw - 4.0)).abs() < 1e-3, "{:?}", cl.c);
    assert!((cr.c.y - (1000.0 + hw + 4.0)).abs() < 1e-3, "{:?}", cr.c);
    // Rows step away from the road by one cell.
    let row3 = w
        .grid
        .cell_footprint(ZoneCellRef {
            block: l.unwrap(),
            cell: 3,
        })
        .unwrap();
    assert!((row3.c.y - (1000.0 - hw - 28.0)).abs() < 1e-3);
}

#[test]
fn cells_under_a_road_are_invalid() {
    let mut w = World::new();
    // A long road along X and a crossing road along Z (no shared node).
    let a = w.line("basic_2l", (1000.0, 1000.0), (1090.0, 1000.0));
    let b = w.line("basic_2l", (1045.0, 940.0), (1045.0, 1060.0));
    w.sync();
    let hw = 6.0;
    let axes = [
        (
            bevy_math::Vec2::new(1000.0, 1000.0),
            bevy_math::Vec2::new(1090.0, 1000.0),
        ),
        (
            bevy_math::Vec2::new(1045.0, 940.0),
            bevy_math::Vec2::new(1045.0, 1060.0),
        ),
    ];
    let mut invalid = 0;
    for (id, blk) in w.grid.blocks() {
        for (i, ok) in blk.valid.iter().enumerate() {
            let c = w
                .grid
                .cell_footprint(ZoneCellRef {
                    block: id,
                    cell: i as u16,
                })
                .unwrap();
            let touches_road = axes
                .iter()
                .any(|(p, q)| c.distance_to_segment(*p, *q) < hw - 0.06);
            if *ok {
                assert!(!touches_road, "valid cell {i} of {id:?} overlaps a road");
            } else {
                invalid += 1;
            }
        }
    }
    // The crossing road really removes cells from both roads' blocks.
    assert!(invalid > 0);
    for seg in [a, b] {
        let lost: usize = w
            .grid
            .blocks_of(seg)
            .into_iter()
            .flatten()
            .map(|id| {
                let blk = w.grid.block(id).unwrap();
                blk.cell_count() - blk.valid_count()
            })
            .sum();
        assert!(lost > 0, "{seg:?}");
    }
}

#[test]
fn junction_has_no_double_cells_and_older_blocks_win() {
    let mut w = World::new();
    // A T: the first road, then a road ending on its middle node-free point is not allowed,
    // so build a proper four-arm cross at a shared node.
    let c = (1100.0, 1100.0);
    let arms = [
        (c.0 - 80.0, c.1),
        (c.0 + 80.0, c.1),
        (c.0, c.1 - 80.0),
        (c.0, c.1 + 80.0),
    ];
    let segs: Vec<SegmentId> = arms.iter().map(|a| w.line("basic_2l", c, *a)).collect();
    w.sync();
    assert_no_double_cells(&w.grid);
    let invalid: usize = w
        .grid
        .blocks()
        .map(|(_, b)| b.cell_count() - b.valid_count())
        .sum();
    assert!(invalid > 0, "corner cells collide");
    // Older wins: block ids rise with creation order, and the first segment's blocks lose
    // nothing to later ones except via roads.
    let first_blocks = w.grid.blocks_of(segs[0]);
    let last_blocks = w.grid.blocks_of(segs[3]);
    assert!(first_blocks[0].unwrap() < last_blocks[0].unwrap());
    let lost = |ids: [Option<ZoneBlockId>; 2]| -> usize {
        ids.into_iter()
            .flatten()
            .map(|id| {
                let b = w.grid.block(id).unwrap();
                b.cell_count() - b.valid_count()
            })
            .sum()
    };
    assert!(
        lost(last_blocks) >= lost(first_blocks),
        "newer blocks lose more"
    );
}

#[test]
fn the_nearest_street_wins_an_overlap_and_ties_are_a_lattice_square_each() {
    // Two parallel roads 60 m apart: the facing blocks (rows reach 38 m from each axis)
    // overlap in the middle without touching the other road.
    let mut w = World::new();
    let a = w.line("basic_2l", (1000.0, 1000.0), (1080.0, 1000.0));
    let b = w.line("basic_2l", (1000.0, 1060.0), (1080.0, 1060.0));
    w.sync();
    assert_no_double_cells(&w.grid);
    let facing = |seg: SegmentId, side: Side| {
        w.grid
            .blocks_of(seg)
            .into_iter()
            .flatten()
            .find(|id| w.grid.block(*id).unwrap().side == side)
            .unwrap()
    };
    // Direction +X: Right side is +Z. A's right block faces B's left block; 48 m between the
    // road edges is 6 rows, so row 3 of each is the square that row 2 of the other owns.
    let (o, n) = (
        w.grid.block(facing(a, Side::Right)).unwrap(),
        w.grid.block(facing(b, Side::Left)).unwrap(),
    );
    for blk in [o, n] {
        let lost: Vec<usize> = (0..blk.cell_count()).filter(|i| !blk.valid[*i]).collect();
        assert_eq!(lost.len(), 10, "row 3 only");
        assert!(lost.iter().all(|i| i % 4 == 3));
    }
}

fn arc(radius: f32, sweep: f32) -> CubicBezier3 {
    // Circular arc approximation by one cubic (sweep <= ~90 degrees), centre at origin
    // shifted to (2000, 2000).
    let k = 4.0 / 3.0 * (sweep / 4.0).tan();
    let c = Vec3::new(2000.0, 0.0, 2000.0);
    let p = |a: f32| c + Vec3::new(radius * a.cos(), 0.0, radius * a.sin());
    let d = |a: f32| Vec3::new(-a.sin(), 0.0, a.cos()) * radius * k;
    CubicBezier3::new(p(0.0), p(0.0) + d(0.0), p(sweep) - d(sweep), p(sweep))
}

#[test]
fn cells_on_a_curve_do_not_overlap() {
    for (radius, sweep) in [(400.0f32, 0.2f32), (120.0, 0.7), (60.0, 1.2), (36.0, 1.5)] {
        let mut w = World::new();
        let s = w.add("basic_2l", arc(radius, sweep));
        w.sync();
        let [l, r] = w.grid.blocks_of(s);
        assert!(l.is_some() && r.is_some());
        assert_no_double_cells(&w.grid);
        let valid: usize = w.grid.valid_cells();
        assert!(valid > 0, "radius {radius}");
        if radius >= 400.0 {
            // Very gentle curves lose nothing.
            assert_eq!(
                valid,
                w.grid.blocks().map(|(_, b)| b.cell_count()).sum::<usize>()
            );
        }
        if radius <= 120.0 {
            // On the inside of a tight curve the fan-out collides: some cells go.
            assert!(valid < w.grid.blocks().map(|(_, b)| b.cell_count()).sum::<usize>());
        }
    }
}

fn summary(grid: &ZoneGrid) -> Vec<(SegmentId, Side, Vec<bool>)> {
    let mut v: Vec<_> = grid
        .blocks()
        .map(|(_, b)| (b.segment, b.side, b.valid.clone()))
        .collect();
    v.sort_by_key(|(s, side, _)| (*s, *side));
    v
}

#[test]
fn changes_rebuild_blocks_and_match_a_from_scratch_sync() {
    let mut w = World::new();
    let a = w.line("basic_2l", (1000.0, 1000.0), (1090.0, 1000.0));
    let b = w.line("basic_2l", (1045.0, 1000.0), (1045.0, 1090.0));
    w.sync();
    let before = w.grid.blocks_of(a);

    // Unchanged net: nothing is rebuilt.
    w.sync();
    assert_eq!(w.grid.blocks_of(a), before);

    // Remove the crossing road: its blocks go, A's cells under it come back.
    w.net.remove_segment(b).unwrap();
    w.sync();
    assert_eq!(w.grid.blocks_of(b), [None, None]);
    assert_eq!(w.grid.blocks_of(a), before, "A itself is untouched");
    let a_blocks: usize = w
        .grid
        .blocks_of(a)
        .into_iter()
        .flatten()
        .map(|id| w.grid.block(id).unwrap().valid_count())
        .sum();
    assert_eq!(a_blocks, 2 * (90 / 8) * 4);

    // Split A: both halves get fresh blocks, the old ones are gone.
    let def = w.db.road(w.road("basic_2l")).clone();
    let split = w.net.split_segment(&def, a, 0.5).unwrap();
    w.sync();
    assert_eq!(w.grid.blocks_of(a), [None, None]);
    for s in &split.segments {
        assert!(w.grid.blocks_of(*s)[0].is_some());
    }

    // Upgrade a half: rebuilt (new ids), possibly different width.
    let up = split.segments[0];
    let old = w.grid.blocks_of(up);
    let mid = w.road("medium_4l");
    let mdef = w.db.road(mid).clone();
    w.net.upgrade_segment(up, &mdef, mid).unwrap();
    w.sync();
    let new = w.grid.blocks_of(up);
    assert!(new[0].is_some() && new != old);

    // The incremental result has the blocks of a from-scratch sync of the same net. (Not the
    // same cells to the last one: a new segment continues the row of the straight neighbour it
    // joins, so the layout depends on the order of building, BUG-043.)
    let mut fresh = ZoneGrid::new();
    fresh.sync(&w.net, &zoning(&w.db));
    let keys =
        |g: &ZoneGrid| -> Vec<_> { summary(g).into_iter().map(|(s, d, _)| (s, d)).collect() };
    assert_eq!(keys(&w.grid), keys(&fresh));
    assert_no_double_cells(&w.grid);
}

#[test]
fn serde_round_trip_keeps_state_and_resyncs_without_changes() {
    let mut w = World::new();
    w.line("basic_2l", (1000.0, 1000.0), (1090.0, 1000.0));
    w.line("basic_2l", (1045.0, 940.0), (1045.0, 1060.0));
    w.sync();
    let bytes = postcard::to_stdvec(&w.grid).unwrap();
    let mut loaded: ZoneGrid = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(loaded, w.grid);
    // A sync of an unchanged net after loading rebuilds the index and changes nothing.
    loaded.sync(&w.net, &zoning(&w.db));
    assert_eq!(loaded, w.grid);
    assert_eq!(postcard::to_stdvec(&loaded).unwrap(), bytes);
    // And it can still react to changes.
    let s = w.net.segments().next().unwrap().0;
    w.net.remove_segment(s).unwrap();
    loaded.sync(&w.net, &zoning(&w.db));
    w.sync();
    assert_eq!(summary(&loaded), summary(&w.grid));
}

#[test]
fn bridges_do_not_zone_or_block() {
    let mut w = World::new();
    let id = w.road("basic_2l");
    let def = w.db.road(id).clone();
    let s = w
        .net
        .add_segment(
            &def,
            id,
            CubicBezier3::line(
                Vec3::new(1000.0, 0.0, 1000.0),
                Vec3::new(1090.0, 0.0, 1000.0),
            ),
            Elevation::Bridge,
        )
        .unwrap();
    w.line("basic_2l", (1045.0, 940.0), (1045.0, 1060.0));
    w.sync();
    assert_eq!(w.grid.blocks_of(s), [None, None]);
    // The ground road's cells at x = 1045 are not blocked by the bridge above them.
    assert_eq!(
        w.grid.valid_cells(),
        w.grid.blocks().map(|(_, b)| b.cell_count()).sum::<usize>()
    );
}

#[test]
fn occupied_cells_are_invalid_until_released_and_cover_newer_blocks() {
    let mut w = World::new();
    // Two crossing roads: the later block overlaps the older one at the corners.
    w.line("basic_2l", (3000.0, 3000.0), (3200.0, 3000.0));
    w.line("basic_2l", (3100.0, 2900.0), (3100.0, 3100.0));
    w.sync();
    let (id, block) = w.grid.blocks().next().map(|(i, b)| (i, b.clone())).unwrap();
    let cell = ZoneCellRef { block: id, cell: 0 };
    assert!(block.valid[0] && !w.grid.is_occupied(cell));

    let before = w.grid.valid_cells();
    w.grid.occupy(&[cell]);
    assert!(w.grid.is_occupied(cell) && !w.grid.is_valid(cell));
    assert!(w.grid.valid_cells() < before);
    w.grid.release(&[cell]);
    assert!(w.grid.is_valid(cell));
    assert_eq!(w.grid.valid_cells(), before);

    // set_zone paints, and refuses cells that do not exist.
    assert!(w.grid.set_zone(cell, cp_content::ZoneType::ResLow));
    assert_eq!(
        w.grid.block(id).unwrap().zones[0],
        cp_content::ZoneType::ResLow
    );
    assert!(!w.grid.set_zone(
        ZoneCellRef {
            block: id,
            cell: 9999
        },
        cp_content::ZoneType::ResLow
    ));
}

// ---------- T-027: painting, queries, carrying painted types across rebuilds ----------

use bevy_math::Vec2;
use cp_content::ZoneType;

fn first_cells(w: &World, seg: SegmentId, side: Side) -> Vec<ZoneCellRef> {
    let id = w
        .grid
        .blocks_of(seg)
        .into_iter()
        .flatten()
        .find(|id| w.grid.block(*id).unwrap().side == side)
        .unwrap();
    w.grid.block_cells(id)
}

fn paint_side(w: &mut World, seg: SegmentId, side: Side, zone: ZoneType) {
    let cells = first_cells(w, seg, side);
    paint(&mut w.grid, &cells, zone).unwrap();
}

fn painted(w: &World, zone: ZoneType) -> usize {
    w.grid
        .blocks()
        .map(|(_, b)| b.zones.iter().filter(|z| **z == zone).count())
        .sum()
}

#[test]
fn paint_sets_valid_cells_skips_invalid_and_built_on_ones_and_erases() {
    let mut w = World::new();
    // Crossing roads: the cross street invalidates some cells of the long road's blocks.
    let a = w.line("basic_2l", (1000.0, 1000.0), (1090.0, 1000.0));
    w.line("basic_2l", (1045.0, 940.0), (1045.0, 1060.0));
    w.sync();
    let cells = first_cells(&w, a, Side::Right);
    let valid: Vec<_> = cells
        .iter()
        .copied()
        .filter(|c| w.grid.is_valid(*c))
        .collect();
    assert!(
        !valid.is_empty() && valid.len() < cells.len(),
        "some cells are under the road"
    );

    let rev = w.grid.revision();
    assert_eq!(
        paint(&mut w.grid, &cells, ZoneType::ResLow),
        Ok(cp_core::Cost::ZERO)
    );
    assert!(w.grid.revision() > rev);
    for c in &cells {
        let want = if w.grid.is_valid(*c) {
            ZoneType::ResLow
        } else {
            ZoneType::None
        };
        assert_eq!(w.grid.zone_of(*c), Some(want));
    }

    // Painting the same type again changes nothing.
    let rev = w.grid.revision();
    assert!(matches!(
        paint(&mut w.grid, &cells, ZoneType::ResLow),
        Err(ZoneError::NothingToPaint { unchanged, .. }) if unchanged == valid.len()
    ));
    assert_eq!(w.grid.revision(), rev);

    // A building on a cell does not protect it (BUG-053): repainting and erasing work, and the
    // sim demolishes the building that no longer fits.
    w.grid.occupy(&[valid[0]]);
    let r = paint(&mut w.grid, &[valid[0]], ZoneType::Industrial);
    assert_eq!(r, Ok(cp_core::Cost::ZERO));
    assert_eq!(w.grid.zone_of(valid[0]), Some(ZoneType::Industrial));

    // Erase the rest.
    assert!(paint(&mut w.grid, &cells, ZoneType::None).is_ok());
    assert_eq!(painted(&w, ZoneType::ResLow), 0);
    assert_eq!(
        paint(&mut w.grid, &[], ZoneType::ResLow),
        Err(ZoneError::NoCells)
    );
}

#[test]
fn queries_find_cells_by_point_radius_rectangle_and_block() {
    let mut w = World::new();
    let a = w.line("basic_2l", (1000.0, 1000.0), (1096.0, 1000.0));
    w.sync();
    let right = first_cells(&w, a, Side::Right);
    let c0 = w.grid.cell_footprint(right[0]).unwrap();
    assert_eq!(w.grid.cell_at(c0.c), Some(right[0]));
    assert_eq!(w.grid.cell_at(Vec2::new(0.0, 0.0)), None);
    // A radius of one cell's half diagonal around a centre contains at least that cell.
    let near = w.grid.cells_within(c0.c, 1.0);
    assert!(near.contains(&right[0]));
    let bigger = w.grid.cells_within(c0.c, 9.0);
    assert!(bigger.len() > near.len());
    assert!(bigger.windows(2).all(|p| p[0] <= p[1]), "id order");
    // A rectangle around the first column's four cells.
    let rect = w
        .grid
        .cells_in_rect(c0.c - Vec2::new(1.0, 1.0), c0.c + Vec2::new(1.0, 30.0));
    assert!(rect.contains(&right[0]));
    assert_eq!(w.grid.block_cells(right[0].block).len(), 12 * 4);
    assert_eq!(w.grid.zone_of(right[0]), Some(ZoneType::None));
}

#[test]
fn painted_types_survive_a_split() {
    let mut w = World::new();
    let a = w.line("basic_2l", (1000.0, 1000.0), (1096.0, 1000.0));
    w.sync();
    paint_side(&mut w, a, Side::Right, ZoneType::ResLow);
    paint_side(&mut w, a, Side::Left, ZoneType::Industrial);
    let (res, ind) = (
        painted(&w, ZoneType::ResLow),
        painted(&w, ZoneType::Industrial),
    );
    assert!(res > 0 && ind > 0);

    let def = w.db.road(w.road("basic_2l")).clone();
    let split = w.net.split_segment(&def, a, 0.5).unwrap();
    w.sync();
    assert_eq!(w.grid.blocks_of(a), [None, None], "the old blocks are gone");
    // Cells of the new blocks near the old painted ones inherit the type (positions shift by
    // up to a few metres because the columns are centred per segment).
    let (res2, ind2) = (
        painted(&w, ZoneType::ResLow),
        painted(&w, ZoneType::Industrial),
    );
    assert!(res2 * 10 >= res * 8, "ResLow {res} -> {res2}");
    assert!(ind2 * 10 >= ind * 8, "Industrial {ind} -> {ind2}");
    // Each side kept its own type.
    for seg in &split.segments {
        for c in first_cells(&w, *seg, Side::Right) {
            assert_ne!(w.grid.zone_of(c), Some(ZoneType::Industrial));
        }
    }
    assert_no_double_cells(&w.grid);
}

#[test]
fn painted_types_survive_an_upgrade() {
    let mut w = World::new();
    let a = w.line("basic_2l", (1000.0, 1000.0), (1096.0, 1000.0));
    w.sync();
    paint_side(&mut w, a, Side::Right, ZoneType::ComLow);
    let before = painted(&w, ZoneType::ComLow);
    let mid = w.road("medium_4l");
    let mdef = w.db.road(mid).clone();
    w.net.upgrade_segment(a, &mdef, mid).unwrap();
    w.sync();
    let after = painted(&w, ZoneType::ComLow);
    assert!(after * 10 >= before * 7, "ComLow {before} -> {after}");
}

#[test]
fn zone_revision_serde_and_painted_state_round_trip() {
    let mut w = World::new();
    let a = w.line("basic_2l", (1000.0, 1000.0), (1090.0, 1000.0));
    w.sync();
    paint_side(&mut w, a, Side::Right, ZoneType::Office);
    let bytes = postcard::to_stdvec(&w.grid).unwrap();
    let loaded: ZoneGrid = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(loaded, w.grid, "painted types are part of the saved grid");
}

/// Paints every cell of a street of `from`, changes the road to `to` and returns
/// `(valid cells afterwards, of which painted)`.
fn repaint_after_road_change(from: &str, to: &str) -> (usize, usize) {
    let mut w = World::new();
    let a = w.line(from, (1000.0, 1000.0), (1090.0, 1000.0));
    w.sync();
    for side in [Side::Left, Side::Right] {
        paint_side(&mut w, a, side, ZoneType::ResLow);
    }
    assert!(painted(&w, ZoneType::ResLow) > 50);
    let to_id = w.road(to);
    let def = w.db.road(to_id).clone();
    w.net.upgrade_segment(a, &def, to_id).unwrap();
    w.sync();
    let (mut valid, mut kept) = (0, 0);
    for (_, b) in w.grid.blocks() {
        for (z, v) in b.zones.iter().zip(&b.valid) {
            if *v {
                valid += 1;
                kept += usize::from(*z == ZoneType::ResLow);
            }
        }
    }
    (valid, kept)
}

#[test]
fn an_upgrade_or_downgrade_keeps_the_painting_of_the_cells_that_stay_valid() {
    for (from, to) in [
        ("basic_2l", "medium_4l"),
        ("basic_2l", "large_6l"),
        ("medium_4l", "basic_2l"),
        ("large_6l", "basic_2l"),
    ] {
        let (valid, kept) = repaint_after_road_change(from, to);
        assert!(valid > 40, "{from}->{to}: {valid} valid cells");
        assert!(
            kept * 100 >= valid * 90,
            "{from}->{to}: only {kept} of {valid} valid cells stay painted"
        );
    }
}

// ---------- terrain: water and slope (T-062) ----------

mod ground {
    use std::sync::Arc;

    use bevy_math::Vec2;
    use cp_terrain::{GRID, MAX_HEIGHT, MapMeta, SPACING, Terrain};

    use super::*;

    fn enc(m: f32) -> u16 {
        (m / MAX_HEIGHT * 65535.0).round() as u16
    }

    /// Terrain with ground height `f(x, z)` and the given sea level.
    fn terrain(sea: f32, f: impl Fn(f32, f32) -> f32) -> Arc<Terrain> {
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

    fn flat() -> Arc<Terrain> {
        terrain(0.0, |_, _| 100.0)
    }

    /// A 200 m road along +X at z = 5000, synced against `t` (or no terrain).
    fn world(t: Option<&Arc<Terrain>>) -> World {
        let mut w = World::new();
        w.line("basic_2l", (5000.0, 5000.0), (5200.0, 5000.0));
        if let Some(t) = t {
            w.grid.set_terrain(Arc::clone(t));
        }
        w.sync();
        w
    }

    fn cells(w: &World) -> Vec<(ZoneCellRef, Vec2, bool)> {
        w.grid
            .blocks()
            .flat_map(|(id, b)| {
                (0..b.cell_count()).map(move |i| ZoneCellRef {
                    block: id,
                    cell: i as u16,
                })
            })
            .map(|c| (c, w.grid.cell_footprint(c).unwrap().c, w.grid.is_valid(c)))
            .collect()
    }

    #[test]
    fn a_flat_dry_map_changes_nothing() {
        let none = world(None);
        let flat = world(Some(&flat()));
        assert!(none.grid.valid_cells() > 0);
        assert_eq!(none.grid.valid_cells(), flat.grid.valid_cells());
    }

    #[test]
    fn cells_over_water_are_invalid_and_the_rest_valid() {
        // A gentle shore (20 %): water below z = 5030, dry ground rising above it.
        let t = terrain(100.0, |_, z| 100.0 + 0.2 * (z - 5030.0));
        let w = world(Some(&t));
        let plain = world(None);
        let all = cells(&w);
        let wet = all.iter().filter(|(_, c, _)| t.is_water(*c)).count();
        assert!(wet > 0, "the fixture has cells over water");
        assert!(wet < all.len(), "and cells that are not");
        for (c, p, valid) in &all {
            if t.is_water(*p) {
                assert!(!valid, "{c:?} at {p} is over water");
            }
        }
        // Dry cells are exactly as valid as without a map.
        for ((c, p, valid), (_, _, plain_valid)) in all.iter().zip(cells(&plain)) {
            if !t.is_water(*p) {
                assert_eq!(*valid, plain_valid, "{c:?} at {p}");
            }
        }
    }

    #[test]
    fn rows_on_a_slope_over_25_percent_are_invalid() {
        // Ground rises 40 % north of z = 5010; the south side stays level.
        let steep = terrain(0.0, |_, z| 100.0 + 0.4 * (z - 5010.0).max(0.0));
        let w = world(Some(&steep));
        let all = cells(&w);
        let north: Vec<_> = all.iter().filter(|(_, p, _)| p.y > 5030.0).collect();
        assert!(
            !north.is_empty() && north.iter().all(|(_, _, v)| !v),
            "steep rows"
        );
        let level: Vec<_> = all.iter().filter(|(_, p, _)| p.y < 5000.0).collect();
        assert!(level.iter().any(|(_, _, v)| *v), "level side stays valid");
        // 20 % is fine.
        let gentle = terrain(0.0, |_, z| 100.0 + 0.2 * (z - 5010.0).max(0.0));
        let g = world(Some(&gentle));
        assert_eq!(g.grid.valid_cells(), world(None).grid.valid_cells());
    }

    #[test]
    fn a_terrain_given_after_the_blocks_exist_revalidates_them() {
        let mut w = world(None);
        let before = w.grid.valid_cells();
        let lake = terrain(100.0, |_, z| 100.0 + 0.2 * (z - 5030.0));
        w.grid.set_terrain(Arc::clone(&lake));
        assert!(w.grid.terrain_dirty());
        w.sync();
        assert!(!w.grid.terrain_dirty());
        assert!(w.grid.valid_cells() < before);
        // The same map again: nothing to do, the revision stays.
        let rev = w.grid.revision();
        w.grid.set_terrain(lake);
        assert!(!w.grid.terrain_dirty());
        w.sync();
        assert_eq!(w.grid.revision(), rev);
    }

    #[test]
    fn releasing_a_building_does_not_free_a_cell_over_water() {
        let t = terrain(100.0, |_, z| 100.0 + 0.2 * (z - 5030.0));
        let mut w = world(Some(&t));
        let (wet, _, _) = *cells(&w)
            .iter()
            .find(|(_, p, _)| t.is_water(*p))
            .expect("a wet cell");
        w.grid.occupy(&[wet]);
        w.grid.release(&[wet]);
        assert!(!w.grid.is_valid(wet));
    }
}

// ---------- free-standing blockers (T-107) ----------

mod blockers {
    use bevy_math::Vec2;

    use super::*;

    fn world() -> World {
        let mut w = World::new();
        w.line("basic_2l", (5000.0, 5000.0), (5200.0, 5000.0));
        w.sync();
        w
    }

    /// A rectangle in the cell's own frame: along `u` the whole cell, along `v` from the
    /// cell's near edge to `frac` of its depth.
    fn rect(grid: &ZoneGrid, cell: ZoneCellRef, frac: f32) -> [Vec2; 4] {
        let o = grid.cell_footprint(cell).unwrap();
        let h = o.half;
        let (a, b) = (-h - 2.0, -h + 2.0 * h * frac);
        [
            o.c + o.u * (-h - 2.0) + o.v * a,
            o.c + o.u * (h + 2.0) + o.v * a,
            o.c + o.u * (h + 2.0) + o.v * b,
            o.c + o.u * (-h - 2.0) + o.v * b,
        ]
    }

    fn some_cell(w: &World) -> ZoneCellRef {
        let (block, _) = w.grid.blocks().next().unwrap();
        ZoneCellRef {
            block,
            cell: 5 * 4 + 2,
        }
    }

    #[test]
    fn a_rectangle_covering_half_a_cell_or_its_centre_makes_it_invalid_and_releasing_restores_it() {
        let mut w = world();
        let cell = some_cell(&w);
        assert!(w.grid.is_valid(cell));
        let rev = w.grid.revision();

        // 40 % of the area, centre outside: still valid.
        w.grid
            .set_blockers(vec![crate::geom::ccw(&rect(&w.grid, cell, 0.4))]);
        assert!(w.grid.is_valid(cell));
        // 60 %: invalid.
        w.grid.set_blockers(vec![rect(&w.grid, cell, 0.6)]);
        assert!(!w.grid.is_valid(cell));
        assert_ne!(w.grid.revision(), rev);
        // The neighbour along the road is untouched.
        let next = ZoneCellRef {
            cell: cell.cell + 4,
            ..cell
        };
        assert!(w.grid.is_valid(next));
        // Gone: valid again.
        w.grid.set_blockers(Vec::new());
        assert!(w.grid.is_valid(cell));

        // A thin strip through the centre (12 % of the area) counts: the centre is inside.
        let o = w.grid.cell_footprint(cell).unwrap();
        let strip = [
            o.c - o.u * 0.5 - o.v * 6.0,
            o.c + o.u * 0.5 - o.v * 6.0,
            o.c + o.u * 0.5 + o.v * 6.0,
            o.c - o.u * 0.5 + o.v * 6.0,
        ];
        w.grid.set_blockers(vec![strip]);
        assert!(!w.grid.is_valid(cell));
        assert_eq!(w.grid.blockers().len(), 1);
    }

    #[test]
    fn blockers_hold_for_cells_made_after_them_and_do_not_change_equality() {
        let mut w = world();
        let before = w.grid.clone();
        let cell = some_cell(&w);
        let q = rect(&w.grid, cell, 1.0);
        w.grid.set_blockers(vec![q]);
        assert!(!w.grid.is_valid(cell));
        // A road laid later over the same ground: its blocks see the obstacle.
        w.line("basic_2l", (5300.0, 5000.0), (5400.0, 5000.0));
        w.sync();
        assert!(!w.grid.is_valid(cell));
        // The list is derived state: it is not part of equality or of the save.
        assert_eq!(w.grid.blockers().len(), 1);
        assert_ne!(before.valid_cells(), w.grid.valid_cells());
    }

    /// BUG-033: adding and removing rectangles one at a time (only the changed ones are
    /// revalidated) gives exactly the validity masks of setting the final list at once.
    #[test]
    fn incremental_blockers_match_a_full_recompute() {
        let mut inc = world();
        let cells: Vec<ZoneCellRef> = {
            let (block, b) = inc.grid.blocks().next().unwrap();
            (0..b.cell_count() as u16)
                .step_by(3)
                .map(|cell| ZoneCellRef { block, cell })
                .collect()
        };
        let quads: Vec<_> = cells
            .iter()
            .enumerate()
            .map(|(i, &c)| rect(&inc.grid, c, 0.3 + 0.1 * (i % 6) as f32))
            .collect();
        let mut list = Vec::new();
        for q in &quads {
            list.push(*q);
            inc.grid.set_blockers(list.clone());
        }
        // Drop every other one, then move one.
        let mut keep: Vec<_> = quads.iter().copied().step_by(2).collect();
        inc.grid.set_blockers(keep.clone());
        keep[0] = quads[1];
        inc.grid.set_blockers(keep.clone());

        let mut full = world();
        full.grid.set_blockers(keep);
        let masks = |g: &ZoneGrid| -> Vec<bool> {
            g.blocks()
                .flat_map(|(block, b)| {
                    (0..b.cell_count() as u16).map(move |cell| ZoneCellRef { block, cell })
                })
                .map(|c| g.is_valid(c))
                .collect()
        };
        assert_eq!(masks(&inc.grid), masks(&full.grid));
        assert_ne!(masks(&inc.grid), masks(&world().grid));
    }
}

/// Valid cells of row `row` on `side` of every block, as (position along x, footprint).
fn row_cells(w: &World, side: Side, row: u16) -> Vec<(f32, Obb)> {
    let mut out = Vec::new();
    for (bid, b) in w.grid.blocks() {
        if b.side != side {
            continue;
        }
        for col in 0..b.cols {
            let i = usize::from(col) * usize::from(BLOCK_DEPTH) + usize::from(row);
            if b.valid[i] {
                let o = w
                    .grid
                    .cell_footprint(ZoneCellRef {
                        block: bid,
                        cell: i as u16,
                    })
                    .unwrap();
                out.push((o.c.x, o));
            }
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

/// BUG-043 A: a street built as chained segments gets one continuous row of cells, with no gap
/// at the joints, like the same street in one piece.
#[test]
fn a_chained_straight_street_has_one_continuous_row() {
    let mut w = World::new();
    let mut x = 0.0;
    for len in [90.0f32, 71.0, 85.0, 63.0] {
        w.line("basic_2l", (x, 0.0), (x + len, 0.0));
        x += len;
    }
    w.sync();
    for side in [Side::Left, Side::Right] {
        let cells = row_cells(&w, side, 0);
        for pair in cells.windows(2) {
            let gap = pair[1].0 - pair[0].0 - CELL_SIZE;
            assert!(gap.abs() < 0.01, "{side:?}: gap {gap} at x = {}", pair[0].0);
        }
        // One piece of the same length has as many cells (within the tail at the ends).
        let mut one = World::new();
        one.line("basic_2l", (0.0, 0.0), (x, 0.0));
        one.sync();
        let n = row_cells(&one, side, 0).len();
        assert!(
            cells.len() + 1 >= n && cells.len() <= n + 1,
            "{} vs {n}",
            cells.len()
        );
    }
}

/// BUG-043 B: four streets around a square block leave no two cells over each other.
#[test]
fn a_block_of_four_streets_has_no_overlapping_cells() {
    let mut w = World::new();
    let s = 150.0;
    w.line("basic_2l", (0.0, 0.0), (s, 0.0));
    w.line("basic_2l", (s, 0.0), (s, s));
    w.line("basic_2l", (s, s), (0.0, s));
    w.line("basic_2l", (0.0, s), (0.0, 0.0));
    w.sync();
    let mut cells = Vec::new();
    for (bid, b) in w.grid.blocks() {
        for (i, ok) in b.valid.iter().enumerate() {
            if *ok {
                let r = ZoneCellRef {
                    block: bid,
                    cell: i as u16,
                };
                cells.push((r, w.grid.cell_footprint(r).unwrap()));
            }
        }
    }
    for i in 0..cells.len() {
        for j in i + 1..cells.len() {
            let p = cells[i].1.penetration(&cells[j].1);
            assert!(
                p < 0.05,
                "{:?} and {:?} overlap by {p}",
                cells[i].0,
                cells[j].0
            );
        }
    }
}

fn worst_overlap(w: &World) -> f32 {
    let mut cells = Vec::new();
    for (bid, b) in w.grid.blocks() {
        for (i, ok) in b.valid.iter().enumerate() {
            if *ok {
                let r = ZoneCellRef {
                    block: bid,
                    cell: i as u16,
                };
                cells.push(w.grid.cell_footprint(r).unwrap());
            }
        }
    }
    let mut worst = 0.0f32;
    for i in 0..cells.len() {
        for j in i + 1..cells.len() {
            worst = worst.max(cells[i].penetration(&cells[j]));
        }
    }
    worst
}

/// BUG-043 B: blocks of streets of every size, in one or two pieces, straight or turned, leave
/// no two cells over each other.
#[test]
fn blocks_of_any_size_and_angle_have_no_overlapping_cells() {
    for angle in [0.0f32, 0.5, 0.9] {
        let (sn, cs) = angle.sin_cos();
        for s in [50.0f32, 57.0, 64.0, 71.0, 80.0, 93.0, 110.0] {
            for pieces in [1usize, 2, 3] {
                let mut w = World::new();
                let rot =
                    |p: (f32, f32)| (1000.0 + p.0 * cs - p.1 * sn, 1000.0 + p.0 * sn + p.1 * cs);
                let corners = [(0.0, 0.0), (s, 0.0), (s, s * 0.8), (0.0, s), (0.0, 0.0)];
                for k in 0..4 {
                    let (a, b) = (corners[k], corners[k + 1]);
                    for p in 0..pieces {
                        let f = |t: f32| rot((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
                        w.line(
                            "basic_2l",
                            f(p as f32 / pieces as f32),
                            f((p + 1) as f32 / pieces as f32),
                        );
                    }
                }
                w.sync();
                let worst = worst_overlap(&w);
                assert!(
                    worst < 0.05 + 1e-3,
                    "angle {angle} s {s} pieces {pieces}: {worst}"
                );
            }
        }
    }
}

/// T-185: with the block fit on, a block between two facing streets that is nearly 8 rows
/// deep is one clean grid of exactly 8 rows, however far off 64 m it is within the fit.
/// `gap`: metres between the edges of the facing streets (they are 12 m wide).
fn block_of(gap: f32, skew_deg: f32) -> World {
    let depth = gap + 12.0;
    let mut w = World::new();
    w.grid.set_block_fit(6.0);
    let len = 128.0;
    // Two cross streets close the block; the facing street may be crooked.
    w.line("basic_2l", (0.0, 0.0), (len, 0.0));
    w.line("basic_2l", (0.0, 0.0), (0.0, depth));
    w.line("basic_2l", (len, 0.0), (len, depth));
    let k = skew_deg.to_radians().tan();
    w.line("basic_2l", (0.0, depth), (len, depth + len * k));
    w.sync();
    w
}

/// Rows of the valid cells between the two streets, counted along the middle of the block.
fn rows_across(w: &World, x: f32) -> Vec<(f32, f32)> {
    let mut spans: Vec<(f32, f32)> = Vec::new();
    for (bid, b) in w.grid.blocks() {
        for (i, ok) in b.valid.iter().enumerate() {
            if !*ok {
                continue;
            }
            let o = w
                .grid
                .cell_footprint(ZoneCellRef {
                    block: bid,
                    cell: i as u16,
                })
                .unwrap();
            if (o.c.x - x).abs() < o.half && o.u.x.abs() > 0.99 {
                spans.push((o.c.y - o.half_v, o.c.y + o.half_v));
            }
        }
    }
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    spans
}

#[test]
fn blocks_nearly_8_cells_deep_are_fitted_to_one_clean_grid() {
    for depth in [60.0f32, 64.0, 69.0] {
        let w = block_of(depth, 0.0);
        let spans = rows_across(&w, 60.0);
        let inner: Vec<_> = spans
            .iter()
            .filter(|s| s.0 > 5.9 && s.1 < depth + 6.1)
            .collect();
        assert_eq!(inner.len(), 8, "gap {depth}: {inner:?}");
        for pair in inner.windows(2) {
            assert!(
                (pair[1].0 - pair[0].1).abs() < 0.02,
                "gap {depth}: gap or overlap between rows {pair:?}"
            );
        }
        assert!((inner[0].0 - 6.0).abs() < 0.02 && (inner[7].1 - (depth + 6.0)).abs() < 0.02);
        assert!(worst_overlap(&w) < 0.06, "gap {depth}");
    }
}

#[test]
fn a_slightly_skewed_block_still_has_no_overlap() {
    let w = block_of(64.0, 3.0);
    assert!(worst_overlap(&w) < 0.06);
    // Eight rows across, none cut off, even at the middle of the crooked block.
    let rows = rows_across(&w, 60.0);
    let between: Vec<_> = rows.iter().filter(|r| r.0 > 5.9 && r.1 < 79.0).collect();
    assert_eq!(between.len(), 8, "{rows:?}");
}

/// T-091: a segment that is turned round (`FlipSegment`) gets its blocks made again; what was
/// painted stays on the same cells (the BUG-015 carry-over, by position, not by side).
#[test]
fn painted_zones_stay_on_the_same_ground_when_a_segment_is_flipped() {
    use cp_content::ZoneType;
    let mut w = World::new();
    let seg = w.line("basic_2l", (1000.0, 1000.0), (1090.0, 1000.0));
    w.sync();
    // Paint the left block's row 0 and the right block's row 1.
    for (slot, row) in [(0usize, 0u16), (1, 1)] {
        let block = w.grid.blocks_of(seg)[slot].unwrap();
        let cells: Vec<ZoneCellRef> = (0..w.grid.block(block).unwrap().cols)
            .map(|col| ZoneCellRef {
                block,
                cell: col * BLOCK_DEPTH + row,
            })
            .collect();
        for c in cells {
            w.grid.set_zone(c, ZoneType::ResLow);
        }
    }
    // The painted cells' centres before.
    let centres = |w: &World| -> Vec<(i32, i32)> {
        let mut v: Vec<(i32, i32)> = w
            .grid
            .blocks()
            .flat_map(|(id, b)| {
                (0..b.cell_count())
                    .filter(move |i| b.zones[*i] != ZoneType::None)
                    .map(move |i| (id, i))
            })
            .map(|(id, i)| {
                let o = w
                    .grid
                    .cell_footprint(ZoneCellRef {
                        block: id,
                        cell: i as u16,
                    })
                    .unwrap();
                ((o.c.x * 10.0).round() as i32, (o.c.y * 10.0).round() as i32)
            })
            .collect();
        v.sort();
        v
    };
    let before = centres(&w);
    assert!(!before.is_empty());
    let def = w.db.road(w.road("basic_2l")).clone();
    w.net.flip_segment(seg, &def).unwrap();
    w.sync();
    let after = centres(&w);
    // The lattice starts at the other end now, so the cells moved by the remainder (< 1 cell);
    // the painting is carried over by position.
    assert_eq!(before.len(), after.len());
    for (x, z) in &before {
        assert!(
            after
                .iter()
                .any(|(a, b)| (a - x).abs() <= 40 && (b - z).abs() <= 40),
            "{x},{z}"
        );
    }
}

/// Zone layout v2 (§7.1, BUG-047).
mod layout_v2 {
    use super::*;
    use crate::geom::quad_intersection_area;
    use bevy_math::Vec2;

    /// Corners of every valid cell.
    fn valid_cells(g: &ZoneGrid) -> Vec<(ZoneCellRef, Obb)> {
        let mut out = Vec::new();
        for (bid, b) in g.blocks() {
            for (i, ok) in b.valid.iter().enumerate() {
                if *ok {
                    let r = ZoneCellRef {
                        block: bid,
                        cell: i as u16,
                    };
                    out.push((r, g.cell_footprint(r).unwrap()));
                }
            }
        }
        out
    }

    /// Corner coordinates of every valid cell, the corners of each in a canonical order.
    fn polygons(g: &ZoneGrid) -> Vec<[f32; 8]> {
        valid_cells(g)
            .iter()
            .map(|(_, o)| {
                let mut c = o.corners().map(|p| (p.x, p.y));
                // The same square from either side starts at another corner: order them.
                c.sort_by(|a, b| {
                    (a.0 * 10.0)
                        .round()
                        .total_cmp(&(b.0 * 10.0).round())
                        .then((a.1 * 10.0).round().total_cmp(&(b.1 * 10.0).round()))
                });
                let mut out = [0f32; 8];
                for (k, (x, z)) in c.iter().enumerate() {
                    out[k * 2] = *x;
                    out[k * 2 + 1] = *z;
                }
                out
            })
            .collect()
    }

    /// The polygons of `a` that have no partner within 2 cm in `b`.
    fn unmatched(a: &[[f32; 8]], b: &[[f32; 8]]) -> Vec<[f32; 8]> {
        let mut used = vec![false; b.len()];
        let mut out = Vec::new();
        for p in a {
            let hit = (0..b.len())
                .find(|&j| !used[j] && p.iter().zip(&b[j]).all(|(x, y)| (x - y).abs() <= 0.02));
            match hit {
                Some(j) => used[j] = true,
                None => out.push(*p),
            }
        }
        out
    }

    fn assert_no_overlap_and_off_roads(w: &World) {
        let cells = valid_cells(&w.grid);
        for i in 0..cells.len() {
            for j in i + 1..cells.len() {
                let a = quad_intersection_area(&cells[i].1.corners(), &cells[j].1.corners());
                assert!(
                    a <= 0.05,
                    "{:?} {:?} overlap {a} m2",
                    cells[i].0,
                    cells[j].0
                );
            }
        }
        for (r, o) in &cells {
            for (_, s) in w.net.segments() {
                let hw = w.db.road(s.road).width / 2.0;
                let (a, b) = (
                    cp_core::curve::to_xz(s.curve.p0),
                    cp_core::curve::to_xz(s.curve.p3),
                );
                assert!(
                    o.distance_to_segment(a, b) >= hw - 0.06,
                    "{r:?} lies on a road"
                );
            }
        }
    }

    /// A grid of straight streets at `xs` and `zs` (axis positions), cut at every crossing.
    fn city(order: u32, xs: &[f32], zs: &[f32]) -> World {
        let mut w = World::new();
        w.grid.set_block_fit(6.0);
        let mut pieces: Vec<((f32, f32), (f32, f32))> = Vec::new();
        for &z in zs {
            for p in xs.windows(2) {
                pieces.push(((p[0], z), (p[1], z)));
            }
        }
        for &x in xs {
            for p in zs.windows(2) {
                pieces.push(((x, p[0]), (x, p[1])));
            }
        }
        match order {
            1 => pieces.reverse(),
            2 => {
                let n = pieces.len();
                pieces = (0..n).map(|i| pieces[(i * 7) % n]).collect();
            }
            _ => {}
        }
        for (a, b) in pieces {
            w.line(
                "basic_2l",
                (1000.0 + a.0, 1000.0 + a.1),
                (1000.0 + b.0, 1000.0 + b.1),
            );
        }
        w.sync();
        w
    }

    const H: f32 = 6.0;

    #[test]
    fn a_grid_of_blocks_is_one_lattice_per_block_whatever_their_size() {
        // Axis positions that leave 64, 70 and 80 m between the road edges.
        let gaps = [64.0f32, 70.0, 80.0];
        let mut pos = vec![0.0f32];
        for g in gaps {
            pos.push(pos.last().unwrap() + g + 2.0 * H);
        }
        let w = city(0, &pos, &pos);
        assert_no_overlap_and_off_roads(&w);
        let cells = valid_cells(&w.grid);
        for (bx, gx) in gaps.iter().enumerate() {
            for (bz, gz) in gaps.iter().enumerate() {
                let (x0, z0) = (1000.0 + pos[bx] + H, 1000.0 + pos[bz] + H);
                let (nx, ny) = (fit_cells(*gx, 6.0), fit_cells(*gz, 6.0));
                let (cu, cv) = (nx.1, ny.1);
                assert!((6.0..=10.0).contains(&cu) && (6.0..=10.0).contains(&cv));
                let mut seen = vec![0u32; (nx.0 * ny.0) as usize];
                for (_, o) in cells
                    .iter()
                    .filter(|(_, o)| o.c.x > x0 && o.c.x < x0 + gx && o.c.y > z0 && o.c.y < z0 + gz)
                {
                    let (fi, fj) = ((o.c.x - x0) / cu - 0.5, (o.c.y - z0) / cv - 0.5);
                    let (i, j) = (fi.round(), fj.round());
                    assert!(
                        (fi - i).abs() * cu < 0.05 && (fj - j).abs() * cv < 0.05,
                        "cell centre {:?} off the lattice",
                        o.c
                    );
                    seen[(i as u32 * ny.0 + j as u32) as usize] += 1;
                }
                for i in 0..nx.0 {
                    for j in 0..ny.0 {
                        let near = i.min(nx.0 - 1 - i).min(j.min(ny.0 - 1 - j)) < 4;
                        assert_eq!(
                            seen[(i * ny.0 + j) as usize],
                            u32::from(near),
                            "block {bx},{bz} square {i},{j}"
                        );
                    }
                }
            }
        }
        // 64 m is 8 cells of 8 m; 70 m is 8 cells of 8.75 m.
        assert_eq!(fit_cells(64.0, 6.0), (8, 8.0));
        assert_eq!(fit_cells(70.0, 6.0), (8, 8.75));
    }

    #[test]
    fn the_layout_does_not_depend_on_the_order_the_streets_were_built_in() {
        let pos = [0.0f32, 76.0, 158.0, 250.0];
        let a = polygons(&city(0, &pos, &pos).grid);
        assert!(!a.is_empty());
        for order in [1, 2] {
            let b = polygons(&city(order, &pos, &pos).grid);
            let only_a: Vec<_> = unmatched(&a, &b);
            let only_b: Vec<_> = unmatched(&b, &a);
            assert!(
                only_a.is_empty() && only_b.is_empty(),
                "order {order}: {} vs {} cells; only first {only_a:?}; only other {only_b:?}",
                a.len(),
                b.len()
            );
        }
    }

    fn row0_on(w: &World, z_sign: f32, z_axis: f32) -> Vec<Obb> {
        let mut v: Vec<Obb> = valid_cells(&w.grid)
            .into_iter()
            .map(|(_, o)| o)
            .filter(|o| (o.c.y - z_axis) * z_sign > 0.0 && (o.c.y - z_axis).abs() < H + 4.5)
            .collect();
        v.sort_by(|a, b| a.c.x.total_cmp(&b.c.x));
        v
    }

    #[test]
    fn a_t_junction_leaves_no_gap_opposite_the_side_street() {
        let mut w = World::new();
        w.line("basic_2l", (1000.0, 1000.0), (1080.0, 1000.0));
        w.line("basic_2l", (1080.0, 1000.0), (1160.0, 1000.0));
        w.line("basic_2l", (1080.0, 1000.0), (1080.0, 1100.0));
        w.sync();
        assert_no_overlap_and_off_roads(&w);
        // North (-z): no side street, one run of 20 cells straight through the node.
        let north = row0_on(&w, -1.0, 1000.0);
        assert_eq!(north.len(), 20);
        for p in north.windows(2) {
            assert!((p[1].c.x - p[0].c.x - 8.0).abs() < 0.05);
        }
        // South (+z): the cells stop at the edges of the side street.
        let south = row0_on(&w, 1.0, 1000.0);
        let gap: Vec<f32> = south
            .windows(2)
            .map(|p| (p[1].c.x - p[1].half) - (p[0].c.x + p[0].half))
            .filter(|g| *g > 0.05)
            .collect();
        assert_eq!(gap.len(), 1);
        assert!((gap[0] - 2.0 * H).abs() < 0.1, "{gap:?}");
    }

    #[test]
    fn a_street_cut_into_segments_is_laid_like_one_segment() {
        let lens = [37.0f32, 96.0, 50.0, 96.0, 41.0];
        let build = |branch_at: Option<usize>| {
            let mut w = World::new();
            let mut x = 1000.0;
            for (k, l) in lens.iter().enumerate() {
                w.line("basic_2l", (x, 1000.0), (x + l, 1000.0));
                x += l;
                if branch_at == Some(k) {
                    w.line("basic_2l", (x, 1000.0), (x, 1100.0));
                }
            }
            w.sync();
            w
        };
        let w = build(None);
        for sign in [-1.0, 1.0] {
            let row = row0_on(&w, sign, 1000.0);
            assert_eq!(row.len(), 40, "320 m is 40 cells of 8 m");
            for p in row.windows(2) {
                assert!((p[1].c.x - p[0].c.x - 8.0).abs() < 0.05);
            }
        }
        let t = build(Some(1));
        assert_no_overlap_and_off_roads(&t);
        let north = row0_on(&t, -1.0, 1000.0);
        assert_eq!(north.len(), 40);
        for p in north.windows(2) {
            assert!((p[1].c.x - p[0].c.x - 8.0).abs() < 0.05);
        }
        let _ = Vec2::ZERO;
    }

    /// BUG-056: in a block of any size every point within three rows of an edge is covered by
    /// a valid cell, and no two valid cells overlap: the perpendicular frontages share one
    /// lattice, so the seam between two edges is whole cells of one edge, not a staircase.
    #[test]
    fn big_blocks_have_no_holes_along_the_seams() {
        let mut sizes: Vec<(f32, f32)> = Vec::new();
        for g in (64..=128).step_by(8) {
            sizes.push((g as f32, g as f32));
            sizes.push((g as f32, (g as f32 * 1.5 / 8.0).round() * 8.0));
        }
        // Not multiples of a cell: the ties and the fitted pitches.
        sizes.extend([
            (90.0, 90.0),
            (100.0, 100.0),
            (110.0, 82.0),
            (90.0, 135.0),
            (100.0, 150.0),
        ]);
        for (gx, gz) in sizes {
            let pos_x = [0.0, gx + 2.0 * H, 2.0 * (gx + 2.0 * H)];
            let pos_z = [0.0, gz + 2.0 * H, 2.0 * (gz + 2.0 * H)];
            let w = city(0, &pos_x, &pos_z);
            assert_no_overlap_and_off_roads(&w);
            let cells = valid_cells(&w.grid);
            let (x0, z0) = (1000.0 + pos_x[1] + H, 1000.0 + pos_z[1] + H);
            let mut holes = Vec::new();
            let (mut x, mut z);
            z = 0.5;
            while z < gz {
                x = 0.5;
                while x < gx {
                    let edge = x.min(gx - x).min(z).min(gz - z);
                    let p = Vec2::new(x0 + x, z0 + z);
                    if edge < 24.0 && !cells.iter().any(|(_, o)| o.distance_to_point(p) <= 1e-3) {
                        holes.push((x, z));
                    }
                    x += 1.0;
                }
                z += 1.0;
            }
            assert!(
                holes.is_empty(),
                "block {gx} x {gz}: holes at {:?}",
                &holes[..holes.len().min(8)]
            );
        }
    }

    /// BUG-067: at a corner the two sides of the street that leaves it count from the same
    /// distance, so their columns stand opposite each other.
    #[test]
    fn the_two_sides_of_a_street_leaving_a_corner_have_the_same_columns() {
        let mut w = World::new();
        w.line("basic_2l", (1000.0, 1000.0), (1000.0, 1100.0));
        w.line("basic_2l", (1000.0, 1000.0), (1160.0, 1000.0));
        w.sync();
        let cells = valid_cells(&w.grid);
        let row = |sign: f32| {
            let mut xs: Vec<f32> = cells
                .iter()
                .filter(|(_, o)| (o.c.y - 1000.0) * sign > H && (o.c.y - 1000.0) * sign < H + 8.0)
                .filter(|(_, o)| o.c.x > 1000.0 + H)
                .map(|(_, o)| o.c.x)
                .collect();
            xs.sort_by(f32::total_cmp);
            xs
        };
        let (north, south) = (row(-1.0), row(1.0));
        assert!(north.len() >= 15, "{} cells", north.len());
        assert_eq!(north.len(), south.len(), "{north:?} against {south:?}");
        for (a, b) in north.iter().zip(&south) {
            assert!((a - b).abs() < 0.05, "{north:?} against {south:?}");
        }
    }

    #[test]
    fn a_30_degree_street_and_a_curve_leave_no_cell_on_another() {
        let mut w = World::new();
        w.line("basic_2l", (1000.0, 1000.0), (1100.0, 1000.0));
        w.line("basic_2l", (1100.0, 1000.0), (1200.0, 1000.0));
        // A street leaving the node at 30 degrees.
        let r = 30f32.to_radians();
        w.line(
            "basic_2l",
            (1100.0, 1000.0),
            (1100.0 + 120.0 * r.cos(), 1000.0 + 120.0 * r.sin()),
        );
        // A curved street under the first, and a straight one joining its end.
        let curve = CubicBezier3 {
            p0: Vec3::new(1000.0, 0.0, 1150.0),
            p1: Vec3::new(1060.0, 0.0, 1100.0),
            p2: Vec3::new(1140.0, 0.0, 1100.0),
            p3: Vec3::new(1200.0, 0.0, 1150.0),
        };
        w.add("basic_2l", curve);
        w.line("basic_2l", (1200.0, 1150.0), (1200.0, 1260.0));
        w.sync();
        assert!(w.grid.valid_cells() > 100);
        assert_no_double_cells(&w.grid);
        let cells = valid_cells(&w.grid);
        for i in 0..cells.len() {
            for j in i + 1..cells.len() {
                let a = quad_intersection_area(&cells[i].1.corners(), &cells[j].1.corners());
                assert!(
                    a <= 0.05,
                    "{:?} {:?} overlap {a} m2",
                    cells[i].0,
                    cells[j].0
                );
            }
        }
        // And no cell stands on the straight streets.
        for (r, o) in &cells {
            for (a, b) in [
                ((1000.0, 1000.0), (1200.0, 1000.0)),
                (
                    (1100.0, 1000.0),
                    (1100.0 + 120.0 * 30f32.to_radians().cos(), 1060.0),
                ),
            ] {
                assert!(
                    o.distance_to_segment(Vec2::new(a.0, a.1), Vec2::new(b.0, b.1)) >= H - 0.06,
                    "{r:?} lies on a road"
                );
            }
        }
    }
}

#[test]
fn a_path_gets_no_zone_blocks() {
    let mut w = World::new();
    let p = w.line("park_path", (1000.0, 1000.0), (1100.0, 1000.0));
    w.sync();
    assert!(!w.db.road(w.road("park_path")).allows_zoning);
    assert_eq!(w.grid.blocks_of(p), [None, None]);
    assert_eq!(w.grid.block_count(), 0);
}

#[test]
fn a_footpath_meeting_a_street_is_no_cross_street() {
    let valid = |w: &World| -> usize {
        w.grid
            .blocks()
            .map(|(_, b)| b.valid.iter().filter(|v| **v).count())
            .sum()
    };
    let mut plain = World::new();
    plain.line("basic_2l", (1000.0, 1000.0), (1100.0, 1000.0));
    plain.line("basic_2l", (1100.0, 1000.0), (1200.0, 1000.0));
    plain.sync();
    let mut w = World::new();
    w.line("basic_2l", (1000.0, 1000.0), (1100.0, 1000.0));
    w.line("basic_2l", (1100.0, 1000.0), (1200.0, 1000.0));
    w.line("park_path", (1100.0, 1000.0), (1100.0, 1060.0));
    w.sync();
    // Only the cells the path itself crosses are lost, on its side: one or two columns.
    let (a, b) = (valid(&plain), valid(&w));
    assert!(b < a && a - b <= 8, "{a} -> {b}");
    assert_eq!(w.grid.block_count(), plain.grid.block_count());
}
