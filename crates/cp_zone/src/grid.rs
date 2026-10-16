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
//! * is covered by a building (see [`ZoneGrid::occupy`]), or
//! * stands on water (the terrain says so at the cell centre) or on ground steeper than
//!   [`MAX_CELL_SLOPE`] there (T-062; needs [`ZoneGrid::set_terrain`]).
//!
//! Overlap means interpenetration deeper than [`PENETRATION_EPS`]; touching and grazing
//! cells are fine. Squares on the inside of a curve crowd together (neighbouring columns
//! overlap by about `8 · depth / radius`), so deep rows on the inside of a bend are
//! invalid: that is where the road geometry leaves no room for them.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use bevy_ecs::resource::Resource;
use bevy_math::Vec2;
use cp_content::{RoadDefId, ZoneType};
use cp_core::curve::{ArcTable, CubicBezier3, to_xz};
use cp_core::{Arena, Id};
use cp_net::{Elevation, NetGraph, NodeId, Segment, SegmentId};
use cp_terrain::Terrain;
use serde::{Deserialize, Serialize};

use crate::geom::{
    Obb, Quad, point_segment_distance, quad_aabb, quad_contains, quad_intersection_area,
};

/// Steepest ground a cell may stand on: rise over run at the cell centre (25 %).
pub const MAX_CELL_SLOPE: f32 = 0.25;
/// Cell edge length, metres.
pub const CELL_SIZE: f32 = 8.0;
/// Rows of cells away from the road.
pub const BLOCK_DEPTH: u16 = 4;
/// Cell/cell interpenetration (m) tolerated before a cell is invalid.
pub const PENETRATION_EPS: f32 = 1.0;
/// A cell may overlap a road strip by at most this (m).
pub const ROAD_EPS: f32 = 0.05;

const ROAD_BUCKET: f32 = 16.0;
const BLOCK_BUCKET: f32 = 32.0;
/// Larger than any cell's reach (half diagonal 5.66 m) plus a margin.
/// Overlap with a cell of an older block that already removes the newer cell (BUG-043): a
/// cell is never kept over another one, only grazing contact is let through.
const CROSS_BLOCK_EPS: f32 = 0.05;
const REACH: f32 = 12.0;
/// Cosine of the largest bend at a degree-2 node that still counts as straight on (about 12°).
const STRAIGHT_COS: f32 = 0.978;
/// Spacing of road centre-line samples, metres.
const ROAD_SAMPLE: f32 = 2.0;
/// How far (m) a rebuilt cell may be from the old cell whose painted type it inherits.
const CARRY_RADIUS: f32 = 5.0;
const CARRY_BUCKET: f32 = 8.0;

pub type ZoneBlockId = Id<ZoneBlock>;

/// A painted cell of a block that is about to be rebuilt, identified by its foot point on
/// the road axis, its row and its side direction (not by its own position, which moves when
/// the road gets wider or narrower).
struct Carried {
    axis: Vec2,
    v: Vec2,
    row: u16,
    zone: ZoneType,
}

impl Carried {
    fn axis_of(o: &Obb, inner: f32, row: u16) -> Vec2 {
        o.c - o.v * (inner + (f32::from(row) + 0.5) * 2.0 * o.half_v)
    }

    fn new(o: &Obb, inner: f32, row: u16, zone: ZoneType) -> Self {
        Self {
            axis: Self::axis_of(o, inner, row),
            v: o.v,
            row,
            zone,
        }
    }
}

/// Process-wide change counter for [`ZoneGrid::revision`]. Not sim state (never saved or
/// hashed): it only lets snapshot caches tell grids apart, including a grid replaced by a load.
fn next_revision() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

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
    /// The stretch of the segment curve this block was built from.
    pub curve: CubicBezier3,
    /// Arc length of the curve (metres).
    pub length: f32,
    /// Arc offset of the first column from the curve start.
    pub s0: f32,
    pub cols: u16,
    /// Distance from the axis to the near edge of row 0.
    pub inner: f32,
    /// Length of a cell along the road and depth of a row: [`CELL_SIZE`] unless the block was
    /// fitted to its surroundings (T-185, `ZoneGrid::set_block_fit`).
    pub cell_len: f32,
    pub row_depth: f32,
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
            let s = self.s0 + (f32::from(col) + 0.5) * self.cell_len;
            // A cell that straddles the end of the stretch (it continues a straight neighbour,
            // BUG-043) stands on the straight line beyond it.
            let dir = |t: f32| {
                to_xz(self.curve.tangent(t))
                    .try_normalize()
                    .unwrap_or(Vec2::X)
            };
            let (p, tg) = if s < 0.0 {
                (to_xz(self.curve.point(0.0)) + dir(0.0) * s, dir(0.0))
            } else if s > self.length {
                (
                    to_xz(self.curve.point(1.0)) + dir(1.0) * (s - self.length),
                    dir(1.0),
                )
            } else {
                let t = table.s_to_t(s);
                (to_xz(self.curve.point(t)), dir(t))
            };
            let u = tg;
            // Left of the travel direction in XZ is (t.z, -t.x), as in `offset_xz`.
            let v = Vec2::new(tg.y, -tg.x) * self.side.sign();
            for row in 0..BLOCK_DEPTH {
                let d = self.inner + (f32::from(row) + 0.5) * self.row_depth;
                out.push(Obb {
                    c: p + v * d,
                    u,
                    v,
                    half: self.cell_len / 2.0,
                    half_v: self.row_depth / 2.0,
                });
            }
        }
        out
    }
}

/// The layout of the cells along a stretch of `length` metres: the arc offset of the first
/// column and the number of columns. Without `align` the row is centred (`floor(len / 8)`
/// cells, the rest split over both ends); with `align` (see [`ZoneGrid::joint_alignment`], the
/// start offset that continues a straight neighbour's row, BUG-043) the cells start there and
/// the rest is left at the far end. The road tool's ghost lays its preview with this too.
pub fn strip_layout(length: f32, align: Option<f32>) -> (f32, u16) {
    match align {
        Some(s0) => (s0, ((length - s0) / CELL_SIZE).floor().max(0.0) as u16),
        None => {
            let cols = (length / CELL_SIZE).floor() as u16;
            ((length - f32::from(cols) * CELL_SIZE) / 2.0, cols)
        }
    }
}

/// The cells one side of a road would get (the road tool's ghost, T-185).
#[derive(Clone, Debug, PartialEq)]
pub struct StripPreview {
    pub side: Side,
    pub cols: u16,
    /// Column-major, `BLOCK_DEPTH` rows per column.
    pub obbs: Vec<Obb>,
    pub valid: Vec<bool>,
}

/// What a segment looked like when its blocks were built; a different value means rebuild.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Fingerprint {
    curve: [u32; 12],
    length: u32,
    road: RoadDefId,
    ground: bool,
    zoning: bool,
    half_width: u32,
}

fn fingerprint(seg: &Segment, road: RoadZoning, zone_curve: &CubicBezier3) -> Fingerprint {
    let c = zone_curve;
    let mut bits = [0u32; 12];
    for (i, p) in [c.p0, c.p1, c.p2, c.p3].iter().enumerate() {
        bits[i * 3] = p.x.to_bits();
        bits[i * 3 + 1] = p.y.to_bits();
        bits[i * 3 + 2] = p.z.to_bits();
    }
    Fingerprint {
        curve: bits,
        length: ArcTable::build(zone_curve).length().to_bits(),
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

/// The rectangles in exactly one of `old` and `new` (as multisets, compared bit for bit).
fn quads_changed(old: &[Quad], new: &[Quad]) -> Vec<Quad> {
    let key = |q: &Quad| q.map(|p| [p.x.to_bits(), p.y.to_bits()]);
    let sorted = |list: &[Quad]| {
        let mut v: Vec<_> = list.iter().map(|q| (key(q), *q)).collect();
        v.sort_by_key(|e| e.0);
        v
    };
    let (a, b) = (sorted(old), sorted(new));
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < a.len() || j < b.len() {
        match (a.get(i), b.get(j)) {
            (Some(x), Some(y)) if x.0 == y.0 => {
                i += 1;
                j += 1;
            }
            (Some(x), Some(y)) if x.0 < y.0 => {
                out.push(x.1);
                i += 1;
            }
            (Some(_), Some(y)) => {
                out.push(y.1);
                j += 1;
            }
            (Some(x), None) => {
                out.push(x.1);
                i += 1;
            }
            (None, Some(y)) => {
                out.push(y.1);
                j += 1;
            }
            (None, None) => unreachable!(),
        }
    }
    out
}

fn inflate(b: Bounds, by: f32) -> Bounds {
    (b.0 - Vec2::splat(by), b.1 + Vec2::splat(by))
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
    /// Cells covered by a building. Derived from the buildings (which own the truth) and
    /// rebuilt after a load with [`ZoneGrid::set_occupied`]; the saved `valid` masks
    /// already account for it.
    #[serde(skip)]
    occupied: BTreeSet<ZoneCellRef>,
    /// Rectangles of free-standing buildings (§8.1): cells under them are invalid. Derived
    /// from the buildings, set with [`ZoneGrid::set_blockers`]; not saved (the saved `valid`
    /// masks already account for them).
    #[serde(skip)]
    blockers: Vec<Quad>,
    /// `blockers` by [`BLOCK_BUCKET`] bucket (indices into the list), so a cell looks only at
    /// the rectangles near it. Derived from `blockers`.
    #[serde(skip)]
    blocker_buckets: BTreeMap<(i32, i32), Vec<u32>>,
    /// The map, for the water and slope rule. Derived input, not saved; set with
    /// [`ZoneGrid::set_terrain`].
    #[serde(skip)]
    terrain: Option<Arc<Terrain>>,
    /// The terrain changed since the last [`ZoneGrid::sync`]: every block is revalidated.
    #[serde(skip)]
    terrain_dirty: bool,
    /// Metres by which a nearly whole block is fitted to whole cells (T-185); `0` fits nothing.
    /// Derived input from the balance, not saved; set with [`ZoneGrid::set_block_fit`].
    #[serde(skip)]
    fit_m: f32,
    /// Bumped on every change (paint, rebuild, validity, occupancy) so snapshots can be
    /// cached. Not saved, not part of equality.
    #[serde(skip, default = "next_revision")]
    revision: u64,
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

    /// Gives the grid the map for the water and slope rule. A different map (compared by
    /// identity) makes the next [`ZoneGrid::sync`] revalidate every block; the same one is a
    /// no-op.
    pub fn set_terrain(&mut self, terrain: Arc<Terrain>) {
        if !self.has_terrain(&terrain) {
            self.terrain = Some(terrain);
            self.terrain_dirty = true;
        }
    }

    /// Whether `terrain` is the map the grid already uses.
    /// Sets the block fit in metres (`balance.ron` `zoning_block_fit_m`); it applies to the
    /// segments built from now on, the cells already laid stay as they are.
    /// A grid from the saved parts of an older format (`legacy`); everything derived is empty
    /// and rebuilt on the first sync.
    pub(crate) fn from_saved_parts(
        blocks: Arena<ZoneBlock>,
        by_segment: BTreeMap<SegmentId, [Option<ZoneBlockId>; 2]>,
        fingerprints: BTreeMap<SegmentId, Fingerprint>,
    ) -> Self {
        Self {
            blocks,
            by_segment,
            fingerprints,
            ..Self::default()
        }
    }

    /// This grid as format 34 wrote it (the legacy writers of `cp_save` and their tests): the
    /// cell sizes are dropped, so only a grid of standard cells survives the trip unchanged.
    pub fn to_v34(&self) -> crate::legacy::ZoneGridV34 {
        crate::legacy::ZoneGridV34::from_parts(
            self.blocks.map(|b| crate::legacy::ZoneBlockV34 {
                segment: b.segment,
                side: b.side,
                curve: b.curve,
                length: b.length,
                s0: b.s0,
                cols: b.cols,
                inner: b.inner,
                zones: b.zones.clone(),
                valid: b.valid.clone(),
            }),
            self.by_segment.clone(),
            self.fingerprints.clone(),
        )
    }

    pub fn set_block_fit(&mut self, metres: f32) {
        self.fit_m = metres.max(0.0);
    }

    pub fn block_fit(&self) -> f32 {
        self.fit_m
    }

    pub fn has_terrain(&self, terrain: &Arc<Terrain>) -> bool {
        self.terrain
            .as_ref()
            .is_some_and(|t| Arc::ptr_eq(t, terrain))
    }

    /// The terrain changed and the next `sync` has to revalidate (so callers do not skip it).
    pub fn terrain_dirty(&self) -> bool {
        self.terrain_dirty
    }

    /// Whether the ground at cell centre `p` allows a cell: dry and not steeper than
    /// [`MAX_CELL_SLOPE`]. Everything is allowed without a map.
    fn ground_ok(&self, p: Vec2) -> bool {
        let Some(t) = &self.terrain else {
            return true;
        };
        if t.is_water(p) {
            return false;
        }
        let ny = t.normal(p).y;
        // slope = tan(angle from horizontal) = sqrt(1 - ny^2) / ny
        (1.0 - ny * ny).max(0.0).sqrt() <= MAX_CELL_SLOPE * ny
    }

    /// Replaces the road type ids remembered in the segment fingerprints by `f(old)` (a save
    /// loaded against content whose definitions sit at other indices), so the blocks are not
    /// rebuilt just because the ids moved.
    pub fn remap_roads(&mut self, f: impl Fn(RoadDefId) -> RoadDefId) {
        for fp in self.fingerprints.values_mut() {
            fp.road = f(fp.road);
        }
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
        self.by_segment
            .get(&segment)
            .copied()
            .unwrap_or([None, None])
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

    /// Whether the lookup structures are built (false after a load until the next `sync`).
    /// Changes whenever anything in the grid changes.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn index_ready(&self) -> bool {
        self.index.built
    }

    // ----- sync -----

    /// Brings the blocks in line with `net`: segments whose geometry or road changed (and
    /// new ones) get new blocks, vanished ones lose theirs, and every cell the change can
    /// affect is revalidated in block-id order. Deterministic: depends only on `net`, the
    /// road lookup and the current blocks.
    ///
    /// `extent` gives the stretch of a segment that is zoned: normally its curve cut at the
    /// junction setbacks (`JunctionGeometry::trimmed_curve`), so blocks start beside the
    /// arms instead of inside the junction. Road strips (for cell validity) always use the
    /// full segment.
    pub fn sync(
        &mut self,
        net: &NetGraph,
        roads: &dyn Fn(RoadDefId) -> RoadZoning,
        extent: &dyn Fn(SegmentId, &Segment) -> CubicBezier3,
    ) {
        if !self.index.built {
            self.rebuild_index(net, roads);
        }
        let zone_curve: BTreeMap<SegmentId, CubicBezier3> =
            net.segments().map(|(id, s)| (id, extent(id, s))).collect();
        let current: Vec<(SegmentId, Fingerprint)> = net
            .segments()
            .map(|(id, s)| (id, fingerprint(s, roads(s.road), &zone_curve[&id])))
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
        // Painted types of the blocks about to be rebuilt (split, upgrade, retrimmed junction)
        // are carried over to the new blocks by position.
        let mut carried: Vec<Carried> = Vec::new();
        for id in &stale {
            self.collect_painted(*id, &mut carried);
        }
        for id in stale {
            self.remove_segment(id, &mut dirty);
        }

        let mut created: BTreeSet<ZoneBlockId> = BTreeSet::new();
        for (id, fp) in current {
            if self.fingerprints.contains_key(&id) {
                continue;
            }
            let seg = net.segment(id).expect("listed segment");
            self.add_segment(
                id,
                seg,
                roads(seg.road),
                &zone_curve[&id],
                self.joint_alignment(
                    net,
                    roads,
                    id,
                    seg,
                    ArcTable::build(&zone_curve[&id]).length(),
                ),
                // Neither end a plain bend that a later segment may continue: a side of a block.
                [seg.start, seg.end]
                    .iter()
                    .all(|n| net.node(*n).is_none_or(|n| n.segments.len() != 2)),
                &mut dirty,
                &mut created,
            );
            self.fingerprints.insert(id, fp);
        }

        if !carried.is_empty() {
            self.apply_carried(&carried, &created);
        }

        // Everything near a change is re-examined, oldest first.
        let mut work: BTreeSet<ZoneBlockId> = created.clone();
        if std::mem::take(&mut self.terrain_dirty) {
            work.extend(self.blocks.iter().map(|(id, _)| id));
        }
        for region in &dirty {
            let region = inflate(*region, REACH);
            for key in bucket_range(region, BLOCK_BUCKET) {
                if let Some(set) = self.index.block_buckets.get(&key) {
                    work.extend(set.iter().copied());
                }
            }
        }
        self.revalidate(work, &created);
    }

    /// Recomputes validity of `work` oldest first; a block whose validity changes (or that
    /// is new, per `created`) can only affect newer ones, so ids only grow.
    fn revalidate(&mut self, mut work: BTreeSet<ZoneBlockId>, created: &BTreeSet<ZoneBlockId>) {
        if !work.is_empty() {
            self.revision = next_revision();
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

    /// Painted (non-`None`) cells of `segment`'s blocks, remembered by where they stand along
    /// the road, not where they were: see [`Carried`].
    fn collect_painted(&self, segment: SegmentId, out: &mut Vec<Carried>) {
        for id in self.blocks_of(segment).into_iter().flatten() {
            let (Some(b), Some(obbs)) = (self.blocks.get(id), self.index.obbs.get(&id)) else {
                continue;
            };
            for (i, (z, o)) in b.zones.iter().zip(obbs).enumerate() {
                if *z != ZoneType::None {
                    let row = (i as u16) % BLOCK_DEPTH;
                    out.push(Carried::new(o, b.inner, row, *z));
                }
            }
        }
    }

    /// Gives the cells of the `created` blocks the type of the nearest carried cell in the same
    /// row and on the same side of the road, within [`CARRY_RADIUS`] along it, each carried
    /// cell used once. Deterministic: new cells in id order, ties broken by the lower carried
    /// index. Matching is by the cell's foot point on the road axis, so a wider or narrower
    /// road (which moves the whole block outward or inward) keeps the painting.
    fn apply_carried(&mut self, carried: &[Carried], created: &BTreeSet<ZoneBlockId>) {
        let mut buckets: BTreeMap<(i32, i32), Vec<usize>> = BTreeMap::new();
        for (i, c) in carried.iter().enumerate() {
            buckets
                .entry((
                    (c.axis.x / CARRY_BUCKET).floor() as i32,
                    (c.axis.y / CARRY_BUCKET).floor() as i32,
                ))
                .or_default()
                .push(i);
        }
        let mut used = vec![false; carried.len()];
        for id in created {
            let Some(obbs) = self.index.obbs.get(id).cloned() else {
                continue;
            };
            let inner = self.blocks.get(*id).expect("created block").inner;
            let mut zones = self.blocks.get(*id).expect("created block").zones.clone();
            for (cell, o) in obbs.iter().enumerate() {
                let row = (cell as u16) % BLOCK_DEPTH;
                let axis = Carried::axis_of(o, inner, row);
                let (bx, bz) = (
                    (axis.x / CARRY_BUCKET).floor() as i32,
                    (axis.y / CARRY_BUCKET).floor() as i32,
                );
                let mut best: Option<(f32, usize)> = None;
                for dx in -1..=1 {
                    for dz in -1..=1 {
                        for &i in buckets.get(&(bx + dx, bz + dz)).into_iter().flatten() {
                            let c = &carried[i];
                            if used[i] || c.row != row || c.v.dot(o.v) < 0.7 {
                                continue;
                            }
                            let d = c.axis.distance(axis);
                            if d <= CARRY_RADIUS
                                && best.is_none_or(|(bd, bi)| d < bd || (d == bd && i < bi))
                            {
                                best = Some((d, i));
                            }
                        }
                    }
                }
                if let Some((_, i)) = best {
                    used[i] = true;
                    zones[cell] = carried[i].zone;
                }
            }
            self.blocks.get_mut(*id).expect("created block").zones = zones;
        }
        self.revision = next_revision();
    }

    // ----- queries (tools, overlays) -----

    /// Cells (valid or not) whose footprint contains `p`; lowest block id first.
    pub fn cell_at(&self, p: Vec2) -> Option<ZoneCellRef> {
        let key = (
            (p.x / BLOCK_BUCKET).floor() as i32,
            (p.y / BLOCK_BUCKET).floor() as i32,
        );
        let blocks = self.index.block_buckets.get(&key)?;
        for id in blocks {
            let Some(obbs) = self.index.obbs.get(id) else {
                continue;
            };
            if let Some(i) = obbs.iter().position(|o| o.distance_to_point(p) <= 1e-3) {
                return Some(ZoneCellRef {
                    block: *id,
                    cell: i as u16,
                });
            }
        }
        None
    }

    /// Every cell (valid or not) whose centre satisfies `keep`, among blocks near `bounds`.
    fn cells_where(&self, bounds: (Vec2, Vec2), keep: impl Fn(Vec2) -> bool) -> Vec<ZoneCellRef> {
        let mut ids: BTreeSet<ZoneBlockId> = BTreeSet::new();
        for key in bucket_range(inflate(bounds, REACH), BLOCK_BUCKET) {
            if let Some(set) = self.index.block_buckets.get(&key) {
                ids.extend(set.iter().copied());
            }
        }
        let mut out = Vec::new();
        for id in ids {
            if let Some(obbs) = self.index.obbs.get(&id) {
                for (i, o) in obbs.iter().enumerate() {
                    if keep(o.c) {
                        out.push(ZoneCellRef {
                            block: id,
                            cell: i as u16,
                        });
                    }
                }
            }
        }
        out
    }

    /// Cells whose centre is within `radius` of `p`, in id order.
    pub fn cells_within(&self, p: Vec2, radius: f32) -> Vec<ZoneCellRef> {
        self.cells_where((p - Vec2::splat(radius), p + Vec2::splat(radius)), |c| {
            c.distance(p) <= radius
        })
    }

    /// Cells whose centre is inside the axis-aligned rectangle, in id order.
    pub fn cells_in_rect(&self, a: Vec2, b: Vec2) -> Vec<ZoneCellRef> {
        let (lo, hi) = (a.min(b), a.max(b));
        self.cells_where((lo, hi), |c| {
            c.x >= lo.x && c.x <= hi.x && c.y >= lo.y && c.y <= hi.y
        })
    }

    /// All cells of a block.
    pub fn block_cells(&self, block: ZoneBlockId) -> Vec<ZoneCellRef> {
        self.blocks
            .get(block)
            .map(|b| {
                (0..b.cell_count() as u16)
                    .map(|cell| ZoneCellRef { block, cell })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The painted type of a cell.
    pub fn zone_of(&self, cell: ZoneCellRef) -> Option<ZoneType> {
        self.blocks
            .get(cell.block)?
            .zones
            .get(usize::from(cell.cell))
            .copied()
    }

    /// Sets the painted type of a cell. Returns `false` when the cell does not exist.
    /// (`PaintZones`, T-027, is the player-facing way; this is the primitive and what
    /// headless tests use.)
    pub fn set_zone(&mut self, cell: ZoneCellRef, zone: ZoneType) -> bool {
        match self
            .blocks
            .get_mut(cell.block)
            .and_then(|b| b.zones.get_mut(usize::from(cell.cell)))
        {
            Some(z) => {
                if *z != zone {
                    *z = zone;
                    self.revision = next_revision();
                }
                true
            }
            None => false,
        }
    }

    /// Whether a building covers the cell.
    pub fn is_occupied(&self, cell: ZoneCellRef) -> bool {
        self.occupied.contains(&cell)
    }

    /// Marks cells as covered by a building: they become invalid, and cells of newer
    /// blocks that overlap them are re-examined. Needs the index (a tick has run with
    /// content); without it the cells are only recorded.
    pub fn occupy(&mut self, cells: &[ZoneCellRef]) {
        self.occupied.extend(cells.iter().copied());
        self.revision = next_revision();
        self.revalidate_cells(cells);
    }

    /// The inverse of [`Self::occupy`] (the building is gone).
    pub fn release(&mut self, cells: &[ZoneCellRef]) {
        for c in cells {
            self.occupied.remove(c);
        }
        self.revision = next_revision();
        self.revalidate_cells(cells);
    }

    /// Replaces the occupancy set without touching validity (after a load: the saved
    /// `valid` masks already include it).
    pub fn set_occupied(&mut self, cells: impl IntoIterator<Item = ZoneCellRef>) {
        self.occupied = cells.into_iter().collect();
        self.revision = next_revision();
    }

    /// Replaces the free-standing rectangles that make cells invalid (§8.1): a cell is
    /// invalid when its centre is inside a rectangle or a rectangle covers at least half of
    /// its area. Derived state: the caller rebuilds it from the buildings after changes and
    /// after a load. Cells near an old or a new rectangle are revalidated; nothing happens
    /// when the list is unchanged.
    pub fn set_blockers(&mut self, blockers: Vec<Quad>) {
        if self.blockers == blockers {
            return;
        }
        // Only the rectangles that came or went change any cell (BUG-033).
        let changed = quads_changed(&self.blockers, &blockers);
        self.blockers = blockers;
        self.blocker_buckets.clear();
        for (i, q) in self.blockers.iter().enumerate() {
            for key in bucket_range(quad_aabb(q), BLOCK_BUCKET) {
                self.blocker_buckets.entry(key).or_default().push(i as u32);
            }
        }
        self.revision = next_revision();
        if !self.index.built {
            return;
        }
        let mut work: BTreeSet<ZoneBlockId> = BTreeSet::new();
        for q in &changed {
            let region = inflate(quad_aabb(q), REACH);
            for key in bucket_range(region, BLOCK_BUCKET) {
                if let Some(set) = self.index.block_buckets.get(&key) {
                    work.extend(set.iter().copied());
                }
            }
        }
        self.revalidate(work, &BTreeSet::new());
    }

    /// The free-standing rectangles currently blocking cells.
    pub fn blockers(&self) -> &[Quad] {
        &self.blockers
    }

    fn blocked_by_rect(&self, o: &Obb) -> bool {
        if self.blockers.is_empty() {
            return false;
        }
        let (lo, hi) = o.aabb();
        let cell = o.corners();
        bucket_range((lo, hi), BLOCK_BUCKET).any(|key| {
            self.blocker_buckets
                .get(&key)
                .into_iter()
                .flatten()
                .any(|&i| {
                    let q = &self.blockers[i as usize];
                    let (qlo, qhi) = quad_aabb(q);
                    if qhi.x < lo.x || qlo.x > hi.x || qhi.y < lo.y || qlo.y > hi.y {
                        return false;
                    }
                    quad_contains(&crate::geom::ccw(q), o.c)
                        || quad_intersection_area(&cell, q)
                            >= 0.5 * (2.0 * o.half) * (2.0 * o.half_v)
                })
        })
    }

    fn revalidate_cells(&mut self, cells: &[ZoneCellRef]) {
        if !self.index.built {
            return;
        }
        let work: BTreeSet<ZoneBlockId> = cells.iter().map(|c| c.block).collect();
        self.revalidate(work, &BTreeSet::new());
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
                self.index
                    .road_buckets
                    .entry(key)
                    .or_default()
                    .push((id, piece));
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

    #[allow(clippy::too_many_arguments)]
    fn add_segment(
        &mut self,
        id: SegmentId,
        seg: &Segment,
        road: RoadZoning,
        zone_curve: &CubicBezier3,
        align: Option<f32>,
        fit_along: bool,
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
        let Some(drafts) = self.lay_strips(zone_curve, road, align, fit_along) else {
            return;
        };
        let mut ids = [None, None];
        for (slot, mut block) in drafts.into_iter().enumerate() {
            block.segment = id;
            let bid = self.blocks.insert(block);
            self.index_block(bid);
            created.insert(bid);
            ids[slot] = Some(bid);
        }
        self.by_segment.insert(id, ids);
    }

    /// The cells a stretch `zone_curve` of a road with `road` gets, as two blocks (left, right;
    /// their `segment` is a placeholder). Used by the sync for a new segment and by the road
    /// tool's ghost for the preview, so both lay exactly the same cells (T-185).
    ///
    /// `align` is the start offset that continues a neighbour's row ([`Self::joint_alignment`]);
    /// without it, and with `fit_along` (neither end is a plain bend that a later segment
    /// may continue) and a block fit set ([`Self::set_block_fit`]), the cells are stretched or
    /// shrunk to fill the stretch exactly. Each side's rows are fitted to the road or older
    /// strip that faces it when that leaves 4 rows within the fit of the free depth.
    pub fn lay_strips(
        &self,
        zone_curve: &CubicBezier3,
        road: RoadZoning,
        align: Option<f32>,
        fit_along: bool,
    ) -> Option<[ZoneBlock; 2]> {
        let length = ArcTable::build(zone_curve).length();
        let (mut s0, mut cols) = strip_layout(length, align);
        let mut cell_len = CELL_SIZE;
        if align.is_none() && fit_along && self.fit_m > 0.0 {
            // Never more cells than the standard layout has: the cells grow (by at most a
            // quarter) to fill the stretch, so the growth of a town does not change.
            let n = (length / CELL_SIZE).floor();
            if n >= 4.0 && length / n <= CELL_SIZE * 1.25 {
                (s0, cols, cell_len) = (0.0, n as u16, length / n);
            }
        }
        if cols == 0 {
            return None;
        }
        let n = usize::from(cols) * usize::from(BLOCK_DEPTH);
        let mut out = [Side::Left, Side::Right].map(|side| ZoneBlock {
            segment: cp_core::Id::new(u32::MAX, 0),
            side,
            curve: *zone_curve,
            length,
            s0,
            cols,
            inner: road.half_width,
            cell_len,
            row_depth: CELL_SIZE,
            zones: vec![ZoneType::None; n],
            valid: vec![true; n],
        });
        if self.fit_m > 0.0 {
            for b in &mut out {
                if let Some(rd) = self.fit_rows(b) {
                    b.row_depth = rd;
                }
            }
        }
        Some(out)
    }

    /// The row depth that makes the 4 rows of `block` fill the free depth up to what faces it
    /// (a road, or the cells of an older strip), when that depth is within the block fit of 4
    /// standard rows. The nearest column decides (a cross street at a corner is not a facing
    /// road, see below). `None`: keep the standard rows.
    fn fit_rows(&self, block: &ZoneBlock) -> Option<f32> {
        let table = ArcTable::build(&block.curve);
        let reach = f32::from(BLOCK_DEPTH) * CELL_SIZE + self.fit_m;
        let mut gaps: Vec<f32> = Vec::new();
        for col in 0..block.cols {
            let s = block.s0 + (f32::from(col) + 0.5) * block.cell_len;
            let t = table.s_to_t(s.clamp(0.0, block.length));
            let p = to_xz(block.curve.point(t));
            let tg = to_xz(block.curve.tangent(t))
                .try_normalize()
                .unwrap_or(Vec2::X);
            let v = Vec2::new(tg.y, -tg.x) * block.side.sign();
            let mut d = 0.5;
            while d <= reach {
                let q = p + v * (block.inner + d);
                if self.obstacle_at(q) {
                    // What stands right at the road is a cross street (or its strip), not the
                    // far side of the block.
                    if d >= f32::from(BLOCK_DEPTH) * CELL_SIZE / 2.0 {
                        gaps.push(d);
                    }
                    break;
                }
                d += 0.5;
            }
        }
        if gaps.is_empty() {
            return None;
        }
        gaps.sort_by(f32::total_cmp);
        // The nearest column decides, so a facing road a few degrees off parallel never has a
        // row cut off by the older strip (the far end is left with a wedge, narrower than a row).
        let free = gaps[0];
        let standard = f32::from(BLOCK_DEPTH) * CELL_SIZE;
        let rd = free / f32::from(BLOCK_DEPTH);
        ((free - standard).abs() <= self.fit_m
            && (free - standard).abs() > 1e-3
            && (0.75..=1.25).contains(&(rd / CELL_SIZE)))
        .then_some(rd)
    }

    /// Whether `q` is on a road or on a valid cell of an existing block.
    fn obstacle_at(&self, q: Vec2) -> bool {
        let probe = (q - Vec2::splat(0.5), q + Vec2::splat(0.5));
        for key in bucket_range(probe, ROAD_BUCKET) {
            for (_, piece) in self.index.road_buckets.get(&key).into_iter().flatten() {
                if point_segment_distance(q, piece.a, piece.b) < piece.half_width - ROAD_EPS {
                    return true;
                }
            }
        }
        for key in bucket_range(probe, BLOCK_BUCKET) {
            for bid in self.index.block_buckets.get(&key).into_iter().flatten() {
                let (Some(b), Some(obbs)) = (self.blocks.get(*bid), self.index.obbs.get(bid))
                else {
                    continue;
                };
                for (j, o) in obbs.iter().enumerate() {
                    if b.valid[j] && o.distance_to_point(q) <= 1e-3 {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Where the first cell of a new segment `id` starts (`s0`) so that its row of cells continues
    /// the row of a segment it joins at a degree-2 node and runs on straight into (BUG-043):
    /// the cell edges meet at the joint, no gap. `None` for a segment with no such neighbour
    /// that already has blocks (the cells are then centred as before).
    pub fn joint_alignment(
        &self,
        net: &NetGraph,
        roads: &dyn Fn(RoadDefId) -> RoadZoning,
        id: SegmentId,
        seg: &Segment,
        length: f32,
    ) -> Option<f32> {
        let leaving = |from_start: bool| {
            to_xz(if from_start {
                seg.curve.tangent(0.0)
            } else {
                -seg.curve.tangent(1.0)
            })
            .try_normalize()
            .unwrap_or(Vec2::X)
        };
        self.alignment_at(
            net,
            roads,
            roads(seg.road),
            length,
            [
                (Some(seg.start), leaving(true)),
                (Some(seg.end), leaving(false)),
            ],
            Some(id),
        )
    }

    /// [`Self::joint_alignment`] for a segment that need not be in the net yet (the road tool's
    /// ghost): `ends` are its nodes with the direction leaving each of them along the segment;
    /// `skip` is the segment itself when it is already in the net. A node counts when, without
    /// `skip`, exactly one segment is at it (the segment becomes the second).
    pub fn alignment_at(
        &self,
        net: &NetGraph,
        roads: &dyn Fn(RoadDefId) -> RoadZoning,
        ours: RoadZoning,
        length: f32,
        ends: [(Option<NodeId>, Vec2); 2],
        skip: Option<SegmentId>,
    ) -> Option<f32> {
        for (k, (node, dir)) in ends.into_iter().enumerate() {
            let at_start = k == 0;
            let Some(node) = node else { continue };
            let Some(n) = net.node(node) else { continue };
            let others: Vec<SegmentId> = n
                .segments
                .iter()
                .copied()
                .filter(|s| Some(*s) != skip)
                .collect();
            let [other] = others[..] else { continue };
            let Some(o) = net.segment(other) else {
                continue;
            };
            let theirs = roads(o.road);
            if o.start == o.end
                || o.elevation != Elevation::Ground
                || !theirs.allows_zoning
                || (theirs.half_width - ours.half_width).abs() > 1e-3
            {
                continue;
            }
            // Straight on: the directions leaving the joint are (nearly) opposite.
            let theirs_dir = to_xz(if o.start == node {
                o.curve.tangent(0.0)
            } else {
                -o.curve.tangent(1.0)
            })
            .try_normalize()
            .unwrap_or(Vec2::X);
            if dir.dot(theirs_dir) > -STRAIGHT_COS {
                continue;
            }
            let Some(b) = self
                .by_segment
                .get(&other)
                .and_then(|ids| ids[0])
                .and_then(|b| self.blocks.get(b))
            else {
                continue;
            };
            // How far the neighbour's nearest cell edge is from the joint (negative: its cell
            // reaches over the joint).
            let free = if o.start == node {
                b.s0
            } else {
                b.length - (b.s0 + f32::from(b.cols) * b.cell_len)
            };
            return Some(if at_start {
                (-free).clamp(-CELL_SIZE, CELL_SIZE)
            } else {
                (length + free).rem_euclid(CELL_SIZE)
            });
        }
        None
    }

    /// The cells a road of `road` type would get along `curve`, with their validity against
    /// what stands now: what the road tool's ghost draws (T-185). `nodes` are the existing
    /// nodes the stretch starts and ends at (`None` for a new one). Laid by the same code as a
    /// real segment's cells ([`Self::lay_strips`]); junction setbacks are not known before the
    /// road exists, so a stretch that ends at a junction is laid from its full length.
    pub fn preview_strips(
        &self,
        net: &NetGraph,
        roads: &dyn Fn(RoadDefId) -> RoadZoning,
        road: RoadDefId,
        curve: &CubicBezier3,
        nodes: [Option<NodeId>; 2],
    ) -> Vec<StripPreview> {
        let ours = roads(road);
        if !ours.allows_zoning {
            return Vec::new();
        }
        let length = ArcTable::build(curve).length();
        let dir = |from_start: bool| {
            to_xz(if from_start {
                curve.tangent(0.0)
            } else {
                -curve.tangent(1.0)
            })
            .try_normalize()
            .unwrap_or(Vec2::X)
        };
        let align = self.alignment_at(
            net,
            roads,
            ours,
            length,
            [(nodes[0], dir(true)), (nodes[1], dir(false))],
            None,
        );
        // Plain bends (an existing end with one segment, which the new one continues) stay
        // unfitted along the road, as in `sync`.
        let bend = nodes
            .iter()
            .flatten()
            .any(|n| net.node(*n).is_some_and(|n| n.segments.len() == 1));
        let Some(blocks) = self.lay_strips(curve, ours, align, !bend) else {
            return Vec::new();
        };
        blocks
            .into_iter()
            .map(|b| {
                let obbs = b.footprints();
                let valid = self.valid_for(&obbs, None);
                StripPreview {
                    side: b.side,
                    cols: b.cols,
                    obbs,
                    valid,
                }
            })
            .collect()
    }

    /// Validity mask of block `id` against roads and older blocks.
    fn compute_valid(&self, id: ZoneBlockId) -> Option<Vec<bool>> {
        self.blocks.get(id)?;
        let obbs = self.index.obbs.get(&id)?;
        Some(self.valid_for(obbs, Some(id)))
    }

    /// Validity of `obbs`, the cells of block `id` (or of a block that does not exist yet:
    /// every existing block is then older).
    fn valid_for(&self, obbs: &[Obb], id: Option<ZoneBlockId>) -> Vec<bool> {
        let depth = usize::from(BLOCK_DEPTH);
        let mut valid = vec![true; obbs.len()];
        for (i, o) in obbs.iter().enumerate() {
            let reach = inflate(o.aabb(), REACH);

            // Water and steep ground.
            if !self.ground_ok(o.c) {
                valid[i] = false;
                continue;
            }

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
            // A building stands here.
            if id.is_some_and(|id| {
                self.occupied.contains(&ZoneCellRef {
                    block: id,
                    cell: i as u16,
                })
            }) {
                valid[i] = false;
                continue;
            }

            // A free-standing building stands here (§8.1).
            if self.blocked_by_rect(o) {
                valid[i] = false;
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
                    older.extend(set.iter().copied().filter(|b| id.is_none_or(|id| *b < id)));
                }
            }
            'older: for oid in older {
                let (Some(ob), Some(oo)) = (self.blocks.get(oid), self.index.obbs.get(&oid)) else {
                    continue;
                };
                for (j, other) in oo.iter().enumerate() {
                    let blocking = ob.valid[j]
                        || self.occupied.contains(&ZoneCellRef {
                            block: oid,
                            cell: j as u16,
                        });
                    if blocking && other.penetration(o) > CROSS_BLOCK_EPS {
                        valid[i] = false;
                        break 'older;
                    }
                }
            }
        }
        valid
    }
}
