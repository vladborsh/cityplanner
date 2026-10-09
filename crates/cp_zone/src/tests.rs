use bevy_math::Vec3;
use cp_content::{ContentDb, RoadDefId};
use cp_core::curve::CubicBezier3;
use cp_net::{Elevation, NetGraph, SegmentId};

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
fn older_block_wins_an_overlap_deterministically() {
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
    // Direction +X: Right side is +Z. A's right block faces B's left block.
    let older = facing(a, Side::Right);
    let newer = facing(b, Side::Left);
    assert!(older < newer);
    let (o, n) = (w.grid.block(older).unwrap(), w.grid.block(newer).unwrap());
    assert_eq!(
        o.cell_count() - o.valid_count(),
        0,
        "older block keeps its cells"
    );
    assert!(
        n.valid_count() < n.cell_count(),
        "newer block loses the contested cells"
    );
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

    // The incremental result equals a from-scratch sync of the same net.
    let mut fresh = ZoneGrid::new();
    fresh.sync(&w.net, &zoning(&w.db));
    assert_eq!(summary(&w.grid), summary(&fresh));
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
