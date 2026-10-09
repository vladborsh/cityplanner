//! Zone blocks and cells (`architecture/system-design.md` §7).
//!
//! Every zoneable ground segment gets one [`ZoneBlock`] per side: columns of 8 m along the
//! segment (`floor(length / 8)`, centred on it) by [`BLOCK_DEPTH`] rows away from the road,
//! starting at the road edge. Each column is oriented by the tangent at its own centre, so
//! on a curve the grid fans out piecewise. A cell is *valid* unless it
//!
//! * overlaps a road strip (any ground segment's centre line buffered by half its width),
//! * overlaps a valid cell of an **older** block (lower [`ZoneBlockId`], i.e. earlier
//!   creation; arena slots are reused, but ids order by `(index, generation)` — see
//!   [`ZoneBlockId`]), or
//! * on a curve, overlaps the previous column of its own block (the later column loses),
//! * (stub) is covered by a building.
//!
//! Overlap means interpenetration deeper than [`PENETRATION_EPS`]; touching and grazing
//! cells are fine, which keeps cells on gentle curves valid.

use std::collections::{BTreeMap, BTreeSet};

use bevy_ecs::resource::Resource;
use bevy_math::Vec2;
use cp_content::{RoadDefId, ZoneType};
use cp_core::curve::{ArcTable, CubicBezier3, to_xz};
use cp_core::{Arena, Id};
use cp_net::{Elevation, NetGraph, Segment, SegmentId};
use serde::{Deserialize, Serialize};

use crate::geom::Obb;

/// Cell edge length, metres.
pub const CELL_SIZE: f32 = 8.0;
/// Rows of cells away from the road.
pub const BLOCK_DEPTH: u16 = 4;
/// Cell/cell interpenetration (m) tolerated before a cell is invalid.
pub const PENETRATION_EPS: f32 = 0.3;
/// A cell may overlap a road strip by at most this (m).
pub const ROAD_EPS: f32 = 0.05;

const ROAD_BUCKET: f32 = 16.0;
const BLOCK_BUCKET: f32 = 32.0;
/// Larger than any cell's reach (half diagonal 5.66 m) plus a margin.
const REACH: f32 = 12.0;
/// Spacing of road centre-line samples, metres.
const ROAD_SAMPLE: f32 = 2.0;

pub type ZoneBlockId = Id<ZoneBlock>;

/// A cell of a block: `cell = column * BLOCK_DEPTH + row` (row 0 touches the road).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ZoneCellRef {
    pub block: ZoneBlockId,
    pub cell: u16,
}

/// Which side of the segment's own direction (start → end) a block lies on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    fn sign(self) -> f32 {
        match self {
            Side::Left => 1.0,
            Side::Right => -1.0,
        }
    }
}

/// What the zoning needs to know about a road type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoadZoning {
    pub allows_zoning: bool,
    /// Half the profile width, metres.
    pub half_width: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ZoneBlock {
    pub segment: SegmentId,
    pub side: Side,
    /// The segment curve this block was built from.
    pub curve: CubicBezier3,
    /// Arc length of the curve (metres).
    pub length: f32,
    /// Arc offset of the first column from the curve start.
    pub s0: f32,
    pub cols: u16,
    /// Distance from the axis to the near edge of row 0.
    pub inner: f32,
    /// Painted type per cell (`ZoneType::None` until painted).
    pub zones: Vec<ZoneType>,
    /// Per cell: not blocked by a road, an older block, or a building.
    pub valid: Vec<bool>,
}

impl ZoneBlock {
    pub fn cell_count(&self) -> usize {
        usize::from(self.cols) * usize::from(BLOCK_DEPTH)
    }

    pub fn valid_count(&self) -> usize {
        self.valid.iter().filter(|v| **v).count()
    }

    /// Footprints of all cells, in cell order.
    pub fn footprints(&self) -> Vec<Obb> {
        let table = ArcTable::build(&self.curve);
        let mut out = Vec::with_capacity(self.cell_count());
        for col in 0..self.cols {
            let s = self.s0 + (f32::from(col) + 0.5) * CELL_SIZE;
            let t = table.s_to_t(s);
            let p = to_xz(self.curve.point(t));
            let tg = to_xz(self.curve.tangent(t)).normalize_or(Vec2::X);
            let u = tg;
            // Left of the travel direction in XZ is (t.z, -t.x), as in `offset_xz`.
            let v = Vec2::new(tg.y, -tg.x) * self.side.sign();
            for row in 0..BLOCK_DEPTH {
                let d = self.inner + (f32::from(row) + 0.5) * CELL_SIZE;
                out.push(Obb {
                    c: p + v * d,
                    u,
                    v,
                    half: CELL_SIZE / 2.0,
                });
            }
        }
        out
    }
}

trait NormalizeOr {
    fn normalize_or(self, fallback: Vec2) -> Vec2;
}
impl NormalizeOr for Vec2 {
    fn normalize_or(self, fallback: Vec2) -> Vec2 {
        self.try_normalize().unwrap_or(fallback)
    }
}

/// What a segment looked like when its blocks were built; a different value means rebuild.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Fingerprint {
    curve: [u32; 12],
    length: u32,
    road: RoadDefId,
    ground: bool,
    zoning: bool,
    half_width: u32,
}

fn fingerprint(seg: &Segment, road: RoadZoning) -> Fingerprint {
    let c = &seg.curve;
    let mut bits = [0u32; 12];
    for (i, p) in [c.p0, c.p1, c.p2, c.p3].iter().enumerate() {
        bits[i * 3] = p.x.to_bits();
        bits[i * 3 + 1] = p.y.to_bits();
        bits[i * 3 + 2] = p.z.to_bits();
    }
    Fingerprint {
        curve: bits,
        length: seg.length.to_bits(),
        road: seg.road,
        ground: seg.elevation == Elevation::Ground,
        zoning: road.allows_zoning,
        half_width: road.half_width.to_bits(),
    }
}

#[derive(Clone, Copy, Debug)]
struct RoadPiece {
    a: Vec2,
    b: Vec2,
    half_width: f32,
}

type Bounds = (Vec2, Vec2);

fn union(a: Bounds, b: Bounds) -> Bounds {
    (a.0.min(b.0), a.1.max(b.1))
}

fn bucket_range(b: Bounds, size: f32) -> impl Iterator<Item = (i32, i32)> {
    let (x0, z0) = ((b.0.x / size).floor() as i32, (b.0.y / size).floor() as i32);
    let (x1, z1) = ((b.1.x / size).floor() as i32, (b.1.y / size).floor() as i32);
    (x0..=x1).flat_map(move |x| (z0..=z1).map(move |z| (x, z)))
}

fn inflate(b: Bounds, by: f32) -> Bounds {
    (b.0 - Vec2::splat(by), b.1 + Vec2::splat(by))
}

fn overlaps(a: Bounds, b: Bounds) -> bool {
    a.0.x <= b.1.x && b.0.x <= a.1.x && a.0.y <= b.1.y && b.0.y <= a.1.y
}

/// Derived lookup structures; rebuilt from the blocks and the net, never saved.
#[derive(Clone, Debug, Default)]
struct Index {
    built: bool,
    obbs: BTreeMap<ZoneBlockId, Vec<Obb>>,
    block_bounds: BTreeMap<ZoneBlockId, Bounds>,
    block_buckets: BTreeMap<(i32, i32), BTreeSet<ZoneBlockId>>,
    road_buckets: BTreeMap<(i32, i32), Vec<(SegmentId, RoadPiece)>>,
    road_bounds: BTreeMap<SegmentId, Bounds>,
}

/// All zone state.
#[derive(Resource, Clone, Debug, Default, Serialize, Deserialize)]
pub struct ZoneGrid {
    blocks: Arena<ZoneBlock>,
    by_segment: BTreeMap<SegmentId, [Option<ZoneBlockId>; 2]>,
    fingerprints: BTreeMap<SegmentId, Fingerprint>,
    #[serde(skip)]
    index: Index,
}

impl PartialEq for ZoneGrid {
    fn eq(&self, other: &Self) -> bool {
        self.blocks == other.blocks
            && self.by_segment == other.by_segment
            && self.fingerprints == other.fingerprints
    }
}

impl ZoneGrid {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn block(&self, id: ZoneBlockId) -> Option<&ZoneBlock> {
        self.blocks.get(id)
    }

    /// Blocks in arena (index) order.
    pub fn blocks(&self) -> impl Iterator<Item = (ZoneBlockId, &ZoneBlock)> {
        self.blocks.iter()
    }

    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// The `[Left, Right]` blocks of a segment, when it has them.
    pub fn blocks_of(&self, segment: SegmentId) -> [Option<ZoneBlockId>; 2] {
        self.by_segment.get(&segment).copied().unwrap_or([None, None])
    }

    /// Footprint of a cell, when the block exists and the cell is in range.
    pub fn cell_footprint(&self, cell: ZoneCellRef) -> Option<Obb> {
        let block = self.blocks.get(cell.block)?;
        if usize::from(cell.cell) >= block.cell_count() {
            return None;
        }
        match self.index.obbs.get(&cell.block) {
            Some(o) => o.get(usize::from(cell.cell)).copied(),
            None => block.footprints().get(usize::from(cell.cell)).copied(),
        }
    }

    pub fn is_valid(&self, cell: ZoneCellRef) -> bool {
        self.blocks
            .get(cell.block)
            .and_then(|b| b.valid.get(usize::from(cell.cell)))
            .copied()
            .unwrap_or(false)
    }

    /// Total number of valid cells (all blocks).
    pub fn valid_cells(&self) -> usize {
        self.blocks.iter().map(|(_, b)| b.valid_count()).sum()
    }

    // ----- sync -----

    /// Brings the blocks in line with `net`: segments whose geometry or road changed (and
    /// new ones) get new blocks, vanished ones lose theirs, and every cell the change can
    /// affect is revalidated in block-id order. Deterministic: depends only on `net`, the
    /// road lookup and the current blocks.
    pub fn sync(&mut self, net: &NetGraph, roads: &dyn Fn(RoadDefId) -> RoadZoning) {
        if !self.index.built {
            self.rebuild_index(net, roads);
        }
        let current: Vec<(SegmentId, Fingerprint)> = net
            .segments()
            .map(|(id, s)| (id, fingerprint(s, roads(s.road))))
            .collect();
        let present: BTreeSet<SegmentId> = current.iter().map(|(id, _)| *id).collect();
        let mut dirty: Vec<Bounds> = Vec::new();

        let stale: Vec<SegmentId> = self
            .fingerprints
            .iter()
            .filter(|(id, fp)| {
                !present.contains(id)
                    || current.iter().find(|(c, _)| c == *id).map(|(_, f)| f) != Some(*fp)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            self.remove_segment(id, &mut dirty);
        }

        let mut created: BTreeSet<ZoneBlockId> = BTreeSet::new();
        for (id, fp) in current {
            if self.fingerprints.contains_key(&id) {
                continue;
            }
            let seg = net.segment(id).expect("listed segment");
            self.add_segment(id, seg, roads(seg.road), &mut dirty, &mut created);
            self.fingerprints.insert(id, fp);
        }

        // Everything near a change is re-examined, oldest first. A block whose validity
        // changes (or that is new) can only affect newer ones, so ids only grow.
        let mut work: BTreeSet<ZoneBlockId> = created.clone();
        for region in &dirty {
            let region = inflate(*region, REACH);
            for key in bucket_range(region, BLOCK_BUCKET) {
                if let Some(set) = self.index.block_buckets.get(&key) {
                    work.extend(set.iter().copied());
                }
            }
        }
        while let Some(id) = work.pop_first() {
            let Some(new_valid) = self.compute_valid(id) else {
                continue;
            };
            let block = self.blocks.get_mut(id).expect("worked block");
            let changed = block.valid != new_valid;
            block.valid = new_valid;
            if changed || created.contains(&id) {
                let bounds = inflate(self.index.block_bounds[&id], REACH);
                for key in bucket_range(bounds, BLOCK_BUCKET) {
                    if let Some(set) = self.index.block_buckets.get(&key) {
                        work.extend(set.iter().copied().filter(|n| *n > id));
                    }
                }
            }
        }
    }

    fn rebuild_index(&mut self, net: &NetGraph, roads: &dyn Fn(RoadDefId) -> RoadZoning) {
        self.index = Index::default();
        for (id, seg) in net.segments() {
            self.add_road_pieces(id, seg, roads(seg.road));
        }
        let ids: Vec<ZoneBlockId> = self.blocks.iter().map(|(id, _)| id).collect();
        for id in ids {
            self.index_block(id);
        }
        self.index.built = true;
    }

    fn add_road_pieces(&mut self, id: SegmentId, seg: &Segment, road: RoadZoning) {
        if seg.elevation != Elevation::Ground {
            return;
        }
        let n = ((seg.length / ROAD_SAMPLE).ceil() as usize).max(1);
        let table = ArcTable::build(&seg.curve);
        let pts: Vec<Vec2> = (0..=n)
            .map(|i| {
                let t = table.s_to_t(table.length() * i as f32 / n as f32);
                to_xz(seg.curve.point(t))
            })
            .collect();
        let mut bounds: Option<Bounds> = None;
        for w in pts.windows(2) {
            let piece = RoadPiece {
                a: w[0],
                b: w[1],
                half_width: road.half_width,
            };
            let b = inflate((w[0].min(w[1]), w[0].max(w[1])), road.half_width);
            for key in bucket_range(b, ROAD_BUCKET) {
                self.index.road_buckets.entry(key).or_default().push((id, piece));
            }
            bounds = Some(bounds.map_or(b, |x| union(x, b)));
        }
        if let Some(b) = bounds {
            self.index.road_bounds.insert(id, b);
        }
    }

    fn index_block(&mut self, id: ZoneBlockId) {
        let block = self.blocks.get(id).expect("block");
        let obbs = block.footprints();
        let mut bounds: Option<Bounds> = None;
        for o in &obbs {
            bounds = Some(bounds.map_or(o.aabb(), |b| union(b, o.aabb())));
        }
        let bounds = bounds.expect("a block has cells");
        for key in bucket_range(bounds, BLOCK_BUCKET) {
            self.index.block_buckets.entry(key).or_default().insert(id);
        }
        self.index.block_bounds.insert(id, bounds);
        self.index.obbs.insert(id, obbs);
    }

    fn unindex_block(&mut self, id: ZoneBlockId) -> Option<Bounds> {
        self.index.obbs.remove(&id);
        let bounds = self.index.block_bounds.remove(&id)?;
        for key in bucket_range(bounds, BLOCK_BUCKET) {
            if let Some(set) = self.index.block_buckets.get_mut(&key) {
                set.remove(&id);
                if set.is_empty() {
                    self.index.block_buckets.remove(&key);
                }
            }
        }
        Some(bounds)
    }

    fn remove_segment(&mut self, id: SegmentId, dirty: &mut Vec<Bounds>) {
        self.fingerprints.remove(&id);
        if let Some(blocks) = self.by_segment.remove(&id) {
            for b in blocks.into_iter().flatten() {
                if let Some(bounds) = self.unindex_block(b) {
                    dirty.push(bounds);
                }
                self.blocks.remove(b);
            }
        }
        if let Some(bounds) = self.index.road_bounds.remove(&id) {
            dirty.push(bounds);
            for key in bucket_range(bounds, ROAD_BUCKET) {
                if let Some(v) = self.index.road_buckets.get_mut(&key) {
                    v.retain(|(s, _)| *s != id);
                    if v.is_empty() {
                        self.index.road_buckets.remove(&key);
                    }
                }
            }
        }
    }

    fn add_segment(
        &mut self,
        id: SegmentId,
        seg: &Segment,
        road: RoadZoning,
        dirty: &mut Vec<Bounds>,
        created: &mut BTreeSet<ZoneBlockId>,
    ) {
        self.add_road_pieces(id, seg, road);
        if let Some(b) = self.index.road_bounds.get(&id) {
            dirty.push(*b);
        }
        if !road.allows_zoning || seg.elevation != Elevation::Ground {
            return;
        }
        let cols = (seg.length / CELL_SIZE).floor() as u16;
        if cols == 0 {
            return;
        }
        let s0 = (seg.length - f32::from(cols) * CELL_SIZE) / 2.0;
        let mut ids = [None, None];
        for (slot, side) in [Side::Left, Side::Right].into_iter().enumerate() {
            let n = usize::from(cols) * usize::from(BLOCK_DEPTH);
            let bid = self.blocks.insert(ZoneBlock {
                segment: id,
                side,
                curve: seg.curve,
                length: seg.length,
                s0,
                cols,
                inner: road.half_width,
                zones: vec![ZoneType::None; n],
                valid: vec![true; n],
            });
            self.index_block(bid);
            created.insert(bid);
            ids[slot] = Some(bid);
        }
        self.by_segment.insert(id, ids);
    }

    /// Validity mask of block `id` against roads and older blocks.
    fn compute_valid(&self, id: ZoneBlockId) -> Option<Vec<bool>> {
        let block = self.blocks.get(id)?;
        let obbs = self.index.obbs.get(&id)?;
        let depth = usize::from(BLOCK_DEPTH);
        let mut valid = vec![true; obbs.len()];
        for (i, o) in obbs.iter().enumerate() {
            let reach = inflate(o.aabb(), REACH);

            // Roads.
            'road: for key in bucket_range(reach, ROAD_BUCKET) {
                for (_, piece) in self.index.road_buckets.get(&key).into_iter().flatten() {
                    if o.distance_to_segment(piece.a, piece.b) < piece.half_width - ROAD_EPS {
                        valid[i] = false;
                        break 'road;
                    }
                }
            }
            if !valid[i] {
                continue;
            }

            // The previous column of the same block (curves): the later column loses.
            let (col, row) = (i / depth, i % depth);
            if col > 0 {
                let prev = (col - 1) * depth;
                for r in row.saturating_sub(1)..=(row + 1).min(depth - 1) {
                    if obbs[prev + r].penetration(o) > PENETRATION_EPS {
                        valid[i] = false;
                    }
                }
            }
            if !valid[i] {
                continue;
            }

            // Older blocks win.
            let mut older: BTreeSet<ZoneBlockId> = BTreeSet::new();
            for key in bucket_range(reach, BLOCK_BUCKET) {
                if let Some(set) = self.index.block_buckets.get(&key) {
                    older.extend(set.iter().copied().filter(|b| *b < id));
                }
            }
            'older: for oid in older {
                let (Some(ob), Some(oo)) = (self.blocks.get(oid), self.index.obbs.get(&oid)) else {
                    continue;
                };
                for (j, other) in oo.iter().enumerate() {
                    if ob.valid[j] && other.penetration(o) > PENETRATION_EPS {
                        valid[i] = false;
                        break 'older;
                    }
                }
            }
            // TODO(cp_building, T-0xx): a cell covered by a building becomes invalid here.
            let _ = block;
        }
        Some(valid)
    }
}
