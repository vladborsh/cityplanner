//! The zone grid as save format 34 wrote it (before the block fit, T-185): blocks had square
//! cells only (no `cell_len`, no `row_depth`). `cp_save` reads old bodies through this and
//! converts: every old block has standard cells.

use std::collections::BTreeMap;

use cp_content::ZoneType;
use cp_core::Arena;
use cp_core::curve::CubicBezier3;
use cp_net::SegmentId;
use serde::{Deserialize, Serialize};

use crate::grid::{CELL_SIZE, Fingerprint, Side, ZoneBlock, ZoneBlockId, ZoneGrid};

/// A block of format 34.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ZoneBlockV34 {
    pub segment: SegmentId,
    pub side: Side,
    pub curve: CubicBezier3,
    pub length: f32,
    pub s0: f32,
    pub cols: u16,
    pub inner: f32,
    pub zones: Vec<ZoneType>,
    pub valid: Vec<bool>,
}

/// The grid of format 34.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ZoneGridV34 {
    blocks: Arena<ZoneBlockV34>,
    by_segment: BTreeMap<SegmentId, [Option<ZoneBlockId>; 2]>,
    fingerprints: BTreeMap<SegmentId, Fingerprint>,
}

impl ZoneGridV34 {
    pub(crate) fn from_parts(
        blocks: Arena<ZoneBlockV34>,
        by_segment: BTreeMap<SegmentId, [Option<ZoneBlockId>; 2]>,
        fingerprints: BTreeMap<SegmentId, Fingerprint>,
    ) -> Self {
        Self {
            blocks,
            by_segment,
            fingerprints,
        }
    }

    /// The same grid in the current layout: standard cells everywhere.
    pub fn into_current(self) -> ZoneGrid {
        ZoneGrid::from_saved_parts(
            self.blocks.map(|b| ZoneBlock {
                segment: b.segment,
                side: b.side,
                curve: b.curve,
                length: b.length,
                s0: b.s0,
                cols: b.cols,
                inner: b.inner,
                cell_len: CELL_SIZE,
                row_depth: CELL_SIZE,
                zones: b.zones.clone(),
                valid: b.valid.clone(),
            }),
            self.by_segment,
            self.fingerprints,
        )
    }
}
