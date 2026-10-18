//! Zone blocks and cells (`architecture/system-design.md` §7, §7.1).
//!
//! The cells are laid out per *frontage* (one side of a street between its cross streets, see
//! [`crate::layout`]): one lattice of columns of about 8 m along the street by [`BLOCK_DEPTH`]
//! rows away from it, starting at the road edge. A frontage is cut into one [`ZoneBlock`] per
//! segment side, the columns belonging to the segment their centre lies on. A cell is *valid*
//! unless it
//!
//! * overlaps a road strip (any ground segment's centre line buffered by half its width),
//! * on a curve, overlaps the previous column of its own block (the later column loses),
//! * overlaps a cell that is covered by a building, or a valid cell with a smaller rank
//!   `(row, ZoneBlockId, cell)` — the cell of the nearest street wins (§7.1),
//! * is covered by a building (see [`ZoneGrid::occupy`]), or
//! * stands on water (the terrain says so at the cell centre) or on ground steeper than
//!   [`MAX_CELL_SLOPE`] there (T-062; needs [`ZoneGrid::set_terrain`]).
//!
//! Overlap means interpenetration deeper than [`CROSS_BLOCK_EPS`] between blocks and
//! [`PENETRATION_EPS`] inside one; touching and grazing cells are fine.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use bevy_ecs::resource::Resource;
use bevy_math::Vec2;
use cp_content::{RoadDefId, ZoneType};
use cp_core::curve::{ArcTable, CubicBezier3, to_xz};
use cp_core::{Arena, Id};
use cp_net::{Elevation, NetGraph, Segment, SegmentId};
use cp_terrain::Terrain;
use serde::{Deserialize, Serialize};

use crate::geom::{Obb, Quad, quad_aabb, quad_contains, quad_intersection_area};
use crate::layout::FACING_CELLS;
use crate::layout::{BlockDraft, frontage_of, layout_frontage};
use crate::roads::{Bounds, RoadIndex, bucket_range, inflate, union};

/// Steepest ground a cell may stand on: rise over run at the cell centre (25 %).
pub const MAX_CELL_SLOPE: f32 = 0.25;
/// Cell edge length, metres.
pub const CELL_SIZE: f32 = 8.0;
/// Rows of cells away from the road.
pub const BLOCK_DEPTH: u16 = 4;
/// Cell/cell interpenetration (m) tolerated between cells of one block before a cell is invalid.
pub const PENETRATION_EPS: f32 = 1.0;
/// A cell may overlap a road strip by at most this (m).
pub const ROAD_EPS: f32 = 0.05;

const BLOCK_BUCKET: f32 = 32.0;
/// Overlap with a cell of another block that wins (smaller rank, or a building) removes the
/// cell; only grazing contact is let through.
pub const CROSS_BLOCK_EPS: f32 = 0.05;
/// Larger than any cell's reach (half diagonal 5.66 m) plus a margin.
const REACH: f32 = 12.0;
/// How far (m) a rebuilt cell may be from the old cell whose painted type it inherits: three
/// quarters of a cell.
const CARRY_RADIUS: f32 = 6.0;
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
    pub(crate) fn sign(self) -> f32 {
        match self {
            Side::Left => 1.0,
            Side::Right => -1.0,
        }
    }

    pub(crate) fn flip(self) -> Self {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }

    /// Index in `[Left, Right]`.
    pub(crate) fn slot(self) -> usize {
        match self {
            Side::Left => 0,
            Side::Right => 1,
        }
    }
}

/// What the zoning needs to know about a road type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoadZoning {
    pub allows_zoning: bool,
    /// Half the profile width, metres.
    pub half_width: f32,
    /// A pedestrian path (T-222): it is no cross street. A street's frontage runs on past
    /// it; only the cells the path itself crosses are dropped.
    pub footpath: bool,
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

/// The cells one side of a new frontage would get (the road tool's ghost, T-185, §7.1).
#[derive(Clone, Debug, PartialEq)]
pub struct StripPreview {
    pub side: Side,
    pub cols: u16,
    /// Column-major, `BLOCK_DEPTH` rows per column.
    pub obbs: Vec<Obb>,
    pub valid: Vec<bool>,
    /// Every 8th column boundary of the frontage from its start (not its two ends), across the
    /// whole depth.
    pub marks: Vec<[Vec2; 2]>,
}

/// What a segment looked like when its blocks were built; a different value means its
/// frontages are laid again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Fingerprint {
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
        length: ArcTable::build(c).length().to_bits(),
        road: seg.road,
        ground: seg.elevation == Elevation::Ground,
        zoning: road.allows_zoning,
        half_width: road.half_width.to_bits(),
    }
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

/// Derived lookup structures; rebuilt from the blocks and the net, never saved.
#[derive(Clone, Debug, Default)]
pub(crate) struct Index {
    pub(crate) built: bool,
    pub(crate) obbs: BTreeMap<ZoneBlockId, Vec<Obb>>,
    pub(crate) block_bounds: BTreeMap<ZoneBlockId, Bounds>,
    pub(crate) block_buckets: BTreeMap<(i32, i32), BTreeSet<ZoneBlockId>>,
    pub(crate) roads: RoadIndex,
}

/// All zone state.
#[derive(Resource, Clone, Debug, Default, Serialize, Deserialize)]
pub struct ZoneGrid {
    blocks: Arena<ZoneBlock>,
    by_segment: BTreeMap<SegmentId, [Option<ZoneBlockId>; 2]>,
    fingerprints: BTreeMap<SegmentId, Fingerprint>,
    #[serde(skip)]
    index: Index,
    /// The saved layout is of an older format: the next sync lays every frontage again.
    #[serde(skip)]
    relay_all: bool,
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
}

impl ZoneGrid {
    /// How far a road reaches into the row depth of the frontages near it (§7.1).
    fn depth_reach(&self) -> f32 {
        FACING_CELLS * CELL_SIZE + self.fit_m
    }

    /// Marks every block to be laid again by the next [`ZoneGrid::sync`] (a save of an older
    /// layout was loaded): painted types are carried over by position, validity is recomputed.
    pub fn relay_all(&mut self) {
        self.relay_all = true;
    }

    /// Brings the blocks in line with `net` (§7.1): the layout is a pure function of the net,
    /// so the frontages near a changed segment are laid again and only the blocks whose
    /// parameters changed are rebuilt (their painted types carried over by position); then every
    /// cell the change can affect is revalidated in rank order. Deterministic: depends only on
    /// `net`, the road lookup and the current blocks.
    pub fn sync(&mut self, net: &NetGraph, roads: &dyn Fn(RoadDefId) -> RoadZoning) {
        if !self.index.built {
            self.rebuild_index(net, roads);
        }
        let relay = std::mem::take(&mut self.relay_all);
        let terrain_dirty = std::mem::take(&mut self.terrain_dirty);
        let current: BTreeMap<SegmentId, Fingerprint> = net
            .segments()
            .map(|(id, s)| (id, fingerprint(s, roads(s.road))))
            .collect();
        let changed: Vec<SegmentId> = current
            .iter()
            .filter(|(id, fp)| self.fingerprints.get(id) != Some(*fp))
            .map(|(id, _)| *id)
            .collect();
        let removed: Vec<SegmentId> = self
            .fingerprints
            .keys()
            .filter(|id| !current.contains_key(id))
            .copied()
            .collect();
        if changed.is_empty() && removed.is_empty() && !relay && !terrain_dirty {
            return;
        }

        // The road strips first: the layouts below measure against them.
        let mut dirty: Vec<Bounds> = Vec::new();
        for id in changed.iter().chain(&removed) {
            if let Some(b) = self.index.roads.remove(*id) {
                dirty.push(b);
            }
        }
        for id in &changed {
            let seg = net.segment(*id).expect("listed segment");
            self.index.roads.add(*id, seg, roads(seg.road));
            if let Some(b) = self.index.roads.bounds.get(id) {
                dirty.push(*b);
            }
        }

        // Painted types of the blocks about to go are carried over to the new blocks by position.
        let mut carried: Vec<Carried> = Vec::new();
        for id in &removed {
            for slot in 0..2 {
                if let Some(b) = self.by_segment.get(id).and_then(|a| a[slot]) {
                    self.collect_painted_block(b, &mut carried);
                    self.drop_block(*id, slot, b, &mut dirty);
                }
            }
            self.fingerprints.remove(id);
        }
        for id in &changed {
            self.fingerprints.insert(*id, current[id]);
        }

        // Frontages to lay again: those with a changed segment, those that end at a node of one,
        // and those within reach of the change (a parallel street closes a block).
        let mut seeds: BTreeSet<SegmentId> = BTreeSet::new();
        if relay {
            seeds.extend(current.keys().copied());
        } else {
            for id in &changed {
                seeds.insert(*id);
                let seg = net.segment(*id).expect("listed segment");
                for n in [seg.start, seg.end] {
                    if let Some(n) = net.node(n) {
                        seeds.extend(n.segments.iter().copied());
                    }
                }
            }
            let reach = self.depth_reach() + 16.0;
            for b in &dirty {
                for id in self.index.roads.segments_near(inflate(*b, reach)) {
                    if current.contains_key(&id) {
                        seeds.insert(id);
                    }
                }
            }
        }
        let (drafts, evaluated) = self.lay_frontages(net, roads, &self.index.roads, &seeds);

        let mut created: BTreeSet<ZoneBlockId> = BTreeSet::new();
        for key in &evaluated {
            let (id, side) = *key;
            let slot = side.slot();
            let existing = self.by_segment.get(&id).and_then(|a| a[slot]);
            let draft = drafts.get(key);
            if let (Some(e), Some(d)) = (existing, draft)
                && self.blocks.get(e).is_some_and(|b| d.same_as(b))
            {
                continue;
            }
            if let Some(e) = existing {
                self.collect_painted_block(e, &mut carried);
                self.drop_block(id, slot, e, &mut dirty);
            }
            if let Some(d) = draft {
                let bid = self.insert_draft(d);
                created.insert(bid);
            }
        }
        if !carried.is_empty() {
            self.apply_carried(&carried, &created);
        }

        let mut work: BTreeSet<(u16, ZoneBlockId)> = BTreeSet::new();
        for id in &created {
            for r in 0..BLOCK_DEPTH {
                work.insert((r, *id));
            }
        }
        if terrain_dirty || relay {
            for (id, _) in self.blocks.iter() {
                for r in 0..BLOCK_DEPTH {
                    work.insert((r, id));
                }
            }
        }
        for region in &dirty {
            self.add_work(inflate(*region, REACH), &mut work);
        }
        self.revision = next_revision();
        self.revalidate(work, &created);
    }

    /// The drafts of the frontages of `seeds` (every frontage once) and the segment sides they
    /// cover, with or without cells.
    #[allow(clippy::type_complexity)]
    fn lay_frontages(
        &self,
        net: &NetGraph,
        roads: &dyn Fn(RoadDefId) -> RoadZoning,
        idx: &RoadIndex,
        seeds: &BTreeSet<SegmentId>,
    ) -> (
        BTreeMap<(SegmentId, Side), BlockDraft>,
        BTreeSet<(SegmentId, Side)>,
    ) {
        let mut drafts = BTreeMap::new();
        let mut evaluated = BTreeSet::new();
        for &id in seeds {
            for side in [Side::Left, Side::Right] {
                if evaluated.contains(&(id, side)) {
                    continue;
                }
                match frontage_of(net, roads, id, side) {
                    None => {
                        evaluated.insert((id, side));
                    }
                    Some(f) => {
                        for d in layout_frontage(net, roads, idx, self.fit_m, &f) {
                            drafts.insert((d.segment, d.side), d);
                        }
                        for i in 0..f.segs.len() {
                            evaluated.insert((f.segs[i].0, f.block_side(i)));
                        }
                    }
                }
            }
        }
        (drafts, evaluated)
    }

    fn insert_draft(&mut self, d: &BlockDraft) -> ZoneBlockId {
        let n = usize::from(d.cols) * usize::from(BLOCK_DEPTH);
        let bid = self.blocks.insert(ZoneBlock {
            segment: d.segment,
            side: d.side,
            curve: d.curve,
            length: d.length,
            s0: d.s0,
            cols: d.cols,
            inner: d.inner,
            cell_len: d.cell_len,
            row_depth: d.row_depth,
            zones: vec![ZoneType::None; n],
            valid: vec![true; n],
        });
        self.index_block(bid);
        self.by_segment.entry(d.segment).or_insert([None, None])[d.side.slot()] = Some(bid);
        bid
    }

    fn drop_block(&mut self, seg: SegmentId, slot: usize, b: ZoneBlockId, dirty: &mut Vec<Bounds>) {
        if let Some(bounds) = self.unindex_block(b) {
            dirty.push(bounds);
        }
        self.blocks.remove(b);
        if let Some(a) = self.by_segment.get_mut(&seg) {
            a[slot] = None;
            if a.iter().all(Option::is_none) {
                self.by_segment.remove(&seg);
            }
        }
    }

    fn add_work(&self, region: Bounds, work: &mut BTreeSet<(u16, ZoneBlockId)>) {
        for key in bucket_range(region, BLOCK_BUCKET) {
            if let Some(set) = self.index.block_buckets.get(&key) {
                for &id in set {
                    for r in 0..BLOCK_DEPTH {
                        work.insert((r, id));
                    }
                }
            }
        }
    }

    /// Recomputes validity of the rows in `work` in rank order `(row, block)`. A cell depends
    /// only on cells of smaller rank (and on buildings), so a changed row (or a row of a block in
    /// `force`) can only affect rows of greater rank: they are queued, and ranks only grow.
    fn revalidate(
        &mut self,
        mut work: BTreeSet<(u16, ZoneBlockId)>,
        force: &BTreeSet<ZoneBlockId>,
    ) {
        if !work.is_empty() {
            self.revision = next_revision();
        }
        let depth = usize::from(BLOCK_DEPTH);
        while let Some((row, id)) = work.pop_first() {
            let Some(obbs) = self.index.obbs.get(&id) else {
                continue;
            };
            let new = self.row_valid(obbs, id, row);
            let Some(block) = self.blocks.get_mut(id) else {
                continue;
            };
            let mut changed = false;
            for (col, v) in new.into_iter().enumerate() {
                let i = col * depth + usize::from(row);
                if block.valid[i] != v {
                    block.valid[i] = v;
                    changed = true;
                }
            }
            if changed || force.contains(&id) {
                let bounds = inflate(self.index.block_bounds[&id], REACH);
                for key in bucket_range(bounds, BLOCK_BUCKET) {
                    if let Some(set) = self.index.block_buckets.get(&key) {
                        for &n in set {
                            for r in row..BLOCK_DEPTH {
                                if (r, self.priority(n)) > (row, self.priority(id)) {
                                    work.insert((r, n));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Painted (non-`None`) cells of one block, remembered by where they stand along the road.
    fn collect_painted_block(&self, id: ZoneBlockId, out: &mut Vec<Carried>) {
        let (Some(b), Some(obbs)) = (self.blocks.get(id), self.index.obbs.get(&id)) else {
            return;
        };
        for (i, (z, o)) in b.zones.iter().zip(obbs).enumerate() {
            if *z != ZoneType::None {
                let row = (i as u16) % BLOCK_DEPTH;
                out.push(Carried::new(o, b.inner, row, *z));
            }
        }
    }

    fn rebuild_index(&mut self, net: &NetGraph, roads: &dyn Fn(RoadDefId) -> RoadZoning) {
        self.index = Index::default();
        self.index.roads = RoadIndex::from_net(net, roads);
        let ids: Vec<ZoneBlockId> = self.blocks.iter().map(|(id, _)| id).collect();
        for id in ids {
            self.index_block(id);
        }
        self.index.built = true;
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

    /// Validity of row `row` of block `id` (one entry per column): not on water or steep ground,
    /// not on a road, not under a building, not on a rectangle of a free-standing building, not
    /// over the previous column of its own block, and not over a cell that wins: one covered by a
    /// building, or a valid one of smaller rank `(row, block)`.
    /// Which of two overlapping cells of the same row wins: the one of the lower segment, then
    /// side, then block (BUG-073: not the arena slot alone, which a rebuilt block may reuse, so
    /// the cells the ghost shows are the cells the road gets).
    fn priority(&self, id: ZoneBlockId) -> (Option<SegmentId>, u8, ZoneBlockId) {
        self.blocks
            .get(id)
            .map_or((None, 0, id), |b| (Some(b.segment), b.side.slot() as u8, id))
    }

    fn row_valid(&self, obbs: &[Obb], id: ZoneBlockId, row: u16) -> Vec<bool> {
        let depth = usize::from(BLOCK_DEPTH);
        let row_i = usize::from(row);
        let cols = obbs.len() / depth;
        let mut out = vec![true; cols];
        for (col, ok) in out.iter_mut().enumerate() {
            let i = col * depth + row_i;
            let o = &obbs[i];
            if self.occupied.contains(&ZoneCellRef {
                block: id,
                cell: i as u16,
            }) || !self.ground_ok(o.c)
                || self.blocked_by_rect(o)
            {
                *ok = false;
                continue;
            }
            let reach = inflate(o.aabb(), REACH);
            if self.index.roads.pieces_in(reach).any(|(_, piece)| {
                o.distance_to_segment(piece.a, piece.b) < piece.half_width - ROAD_EPS
            }) {
                *ok = false;
                continue;
            }
            // The previous column of the same block (curves): the later column loses.
            if col > 0 {
                let prev = (col - 1) * depth;
                if (row_i.saturating_sub(1)..=(row_i + 1).min(depth - 1))
                    .any(|r| obbs[prev + r].penetration(o) > PENETRATION_EPS)
                {
                    *ok = false;
                    continue;
                }
            }
            // The cells that win over this one.
            let mut near: BTreeSet<ZoneBlockId> = BTreeSet::new();
            for key in bucket_range(reach, BLOCK_BUCKET) {
                if let Some(set) = self.index.block_buckets.get(&key) {
                    near.extend(set.iter().copied().filter(|b| *b != id));
                }
            }
            'near: for nid in near {
                let (Some(nb), Some(no)) = (self.blocks.get(nid), self.index.obbs.get(&nid)) else {
                    continue;
                };
                for (j, other) in no.iter().enumerate() {
                    if (other.c - o.c).length_squared() > 14.0 * 14.0 {
                        continue;
                    }
                    let blocking = self.occupied.contains(&ZoneCellRef {
                        block: nid,
                        cell: j as u16,
                    }) || (nb.valid[j] && ((j % depth) as u16, self.priority(nid)) < (row, self.priority(id)));
                    if blocking && other.penetration(o) > CROSS_BLOCK_EPS {
                        *ok = false;
                        break 'near;
                    }
                }
            }
        }
        out
    }

    /// The column boundaries of both sides of `segment` as laid now (`[Left, Right]`), for the
    /// road tool's marks on an existing street (§7.1).
    pub fn frontage_marks(
        &self,
        net: &NetGraph,
        roads: &dyn Fn(RoadDefId) -> RoadZoning,
        segment: SegmentId,
    ) -> [Vec<crate::layout::Mark>; 2] {
        let mut out = [Vec::new(), Vec::new()];
        for side in [Side::Left, Side::Right] {
            if let Some(f) = frontage_of(net, roads, segment, side) {
                for d in layout_frontage(net, roads, &self.index.roads, self.fit_m, &f) {
                    if d.segment == segment && d.side == side {
                        out[side.slot()] = d.marks;
                    }
                }
            }
        }
        out
    }

    /// The cells the frontages near `region` get in `net_after` (the net with a planned road
    /// applied) and whose blocks differ from the ones standing now, with their validity against
    /// buildings, terrain and the remaining blocks: what the road tool's ghost draws. Laid and
    /// validated by the same code as a real sync, so the preview is the built result (§7.1).
    pub fn preview_layout(
        &self,
        net_after: &NetGraph,
        roads: &dyn Fn(RoadDefId) -> RoadZoning,
        region: Bounds,
    ) -> Vec<StripPreview> {
        let idx = RoadIndex::from_net(net_after, roads);
        let seeds: BTreeSet<SegmentId> = idx
            .segments_near(inflate(region, self.depth_reach() + 16.0))
            .into_iter()
            .collect();
        let (drafts, evaluated) = self.lay_frontages(net_after, roads, &idx, &seeds);
        let existing =
            |k: &(SegmentId, Side)| self.by_segment.get(&k.0).and_then(|a| a[k.1.slot()]);
        let mut replaced: BTreeSet<ZoneBlockId> = BTreeSet::new();
        let mut fresh: Vec<&BlockDraft> = Vec::new();
        for k in &evaluated {
            let same = match (existing(k), drafts.get(k)) {
                (Some(e), Some(d)) => self.blocks.get(e).is_some_and(|b| d.same_as(b)),
                (None, None) => true,
                _ => false,
            };
            if same {
                continue;
            }
            replaced.extend(existing(k));
            fresh.extend(drafts.get(k));
        }
        // Blocks of segments that the plan removed (split) near the region go as well.
        let gone = self
            .index
            .roads
            .segments_near(inflate(region, self.depth_reach()));
        for id in gone {
            if net_after.segment(id).is_none() {
                replaced.extend(self.by_segment.get(&id).into_iter().flatten().flatten());
            }
        }
        if fresh.is_empty() {
            return Vec::new();
        }
        let mut scratch = ZoneGrid::new();
        scratch.terrain = self.terrain.clone();
        scratch.fit_m = self.fit_m;
        scratch.index.roads = idx;
        scratch.index.built = true;
        let mut bounds: Option<Bounds> = None;
        for d in &fresh {
            let b = ZoneBlock {
                segment: d.segment,
                side: d.side,
                curve: d.curve,
                length: d.length,
                s0: d.s0,
                cols: d.cols,
                inner: d.inner,
                cell_len: d.cell_len,
                row_depth: d.row_depth,
                zones: Vec::new(),
                valid: Vec::new(),
            };
            for o in b.footprints() {
                bounds = Some(bounds.map_or(o.aabb(), |x| union(x, o.aabb())));
            }
        }
        let bounds = inflate(bounds.expect("fresh drafts have cells"), REACH);
        let mut near: BTreeSet<ZoneBlockId> = BTreeSet::new();
        for key in bucket_range(bounds, BLOCK_BUCKET) {
            if let Some(set) = self.index.block_buckets.get(&key) {
                near.extend(set.iter().copied());
            }
        }
        let mut map: BTreeMap<ZoneBlockId, ZoneBlockId> = BTreeMap::new();
        for id in near.iter().filter(|id| !replaced.contains(id)) {
            let b = self.blocks.get(*id).expect("indexed block").clone();
            let sid = scratch.blocks.insert(b);
            scratch.index_block(sid);
            map.insert(*id, sid);
        }
        scratch.occupied = self
            .occupied
            .iter()
            .filter_map(|c| {
                map.get(&c.block).map(|b| ZoneCellRef {
                    block: *b,
                    cell: c.cell,
                })
            })
            .collect();
        // Buildings on cells that go stay where they are: their cells block the new ones.
        let mut blockers = self.blockers.clone();
        for c in self.occupied.iter().filter(|c| replaced.contains(&c.block)) {
            if let Some(o) = self.cell_footprint(*c) {
                blockers.push(o.corners());
            }
        }
        scratch.assign_blockers(blockers);
        let mut created: BTreeSet<ZoneBlockId> = BTreeSet::new();
        let mut order: Vec<(ZoneBlockId, &BlockDraft)> = Vec::new();
        for d in &fresh {
            let id = scratch.insert_draft(d);
            created.insert(id);
            order.push((id, d));
        }
        let mut work = BTreeSet::new();
        for id in &created {
            for r in 0..BLOCK_DEPTH {
                work.insert((r, *id));
            }
        }
        scratch.revalidate(work, &created);
        order
            .into_iter()
            .map(|(id, d)| {
                let b = scratch.blocks.get(id).expect("preview block");
                StripPreview {
                    side: d.side,
                    cols: d.cols,
                    obbs: scratch.index.obbs[&id].clone(),
                    valid: b.valid.clone(),
                    marks: d
                        .marks
                        .iter()
                        .filter(|m| m.major && !m.end)
                        .map(|m| {
                            [
                                m.axis + m.v * d.inner,
                                m.axis + m.v * (d.inner + f32::from(BLOCK_DEPTH) * d.row_depth),
                            ]
                        })
                        .collect(),
                }
            })
            .collect()
    }

    fn assign_blockers(&mut self, blockers: Vec<Quad>) {
        self.blockers = blockers;
        self.blocker_buckets.clear();
        for (i, q) in self.blockers.iter().enumerate() {
            for key in bucket_range(quad_aabb(q), BLOCK_BUCKET) {
                self.blocker_buckets.entry(key).or_default().push(i as u32);
            }
        }
    }
}

impl ZoneGrid {
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

    /// The cell whose footprint contains `p`. Where blocks overlap (corners, BUG-047) the cell
    /// that stands there wins: a valid or an occupied one, lowest block id first; a point only
    /// covered by cells that lost to others gets the first of those.
    pub fn cell_at(&self, p: Vec2) -> Option<ZoneCellRef> {
        let key = (
            (p.x / BLOCK_BUCKET).floor() as i32,
            (p.y / BLOCK_BUCKET).floor() as i32,
        );
        let blocks = self.index.block_buckets.get(&key)?;
        let mut loser = None;
        for id in blocks {
            let Some(obbs) = self.index.obbs.get(id) else {
                continue;
            };
            for (i, o) in obbs.iter().enumerate() {
                if o.distance_to_point(p) > 1e-3 {
                    continue;
                }
                let cell = ZoneCellRef {
                    block: *id,
                    cell: i as u16,
                };
                if self.blocks.get(*id).is_some_and(|b| b.valid[i]) || self.occupied.contains(&cell)
                {
                    return Some(cell);
                }
                loser.get_or_insert(cell);
            }
        }
        loser
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
        // Both corners are inclusive (BUG-050): a click in the half of its cell away from the
        // rest of the rectangle must still take that cell, so the rectangle reaches the centre
        // of each corner cell, and with it the cell's whole row and column.
        let reach = |p: Vec2| {
            self.cell_at(p)
                .and_then(|c| self.cell_footprint(c))
                .map_or(p, |o| o.c)
        };
        let (ca, cb) = (reach(a), reach(b));
        let (lo, hi) = (a.min(b).min(ca).min(cb), a.max(b).max(ca).max(cb));
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
        let mut work: BTreeSet<(u16, ZoneBlockId)> = BTreeSet::new();
        for q in &changed {
            self.add_work(inflate(quad_aabb(q), REACH), &mut work);
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
        let mut work: BTreeSet<(u16, ZoneBlockId)> = BTreeSet::new();
        for c in cells {
            if let Some(b) = self.index.block_bounds.get(&c.block) {
                self.add_work(inflate(*b, REACH), &mut work);
            }
        }
        self.revalidate(work, &BTreeSet::new());
    }
}
