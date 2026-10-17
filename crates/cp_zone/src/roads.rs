//! The ground roads as short straight pieces in buckets: what cells must stay off and what
//! the layout measures block depths against. Derived from the net, never saved.

use std::collections::BTreeMap;

use bevy_math::Vec2;
use cp_content::RoadDefId;
use cp_core::curve::{ArcTable, to_xz};
use cp_net::{Elevation, NetGraph, Segment, SegmentId};

use crate::grid::RoadZoning;

pub(crate) const ROAD_BUCKET: f32 = 16.0;
/// Spacing of road centre-line samples, metres.
const ROAD_SAMPLE: f32 = 2.0;

pub(crate) type Bounds = (Vec2, Vec2);

pub(crate) fn union(a: Bounds, b: Bounds) -> Bounds {
    (a.0.min(b.0), a.1.max(b.1))
}

pub(crate) fn inflate(b: Bounds, by: f32) -> Bounds {
    (b.0 - Vec2::splat(by), b.1 + Vec2::splat(by))
}

pub(crate) fn bucket_range(b: Bounds, size: f32) -> impl Iterator<Item = (i32, i32)> {
    let (x0, z0) = ((b.0.x / size).floor() as i32, (b.0.y / size).floor() as i32);
    let (x1, z1) = ((b.1.x / size).floor() as i32, (b.1.y / size).floor() as i32);
    (x0..=x1).flat_map(move |x| (z0..=z1).map(move |z| (x, z)))
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RoadPiece {
    pub a: Vec2,
    pub b: Vec2,
    pub half_width: f32,
    pub footpath: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RoadIndex {
    pub buckets: BTreeMap<(i32, i32), Vec<(SegmentId, RoadPiece)>>,
    pub bounds: BTreeMap<SegmentId, Bounds>,
}

impl RoadIndex {
    pub fn from_net(net: &NetGraph, roads: &dyn Fn(RoadDefId) -> RoadZoning) -> Self {
        let mut out = Self::default();
        for (id, seg) in net.segments() {
            out.add(id, seg, roads(seg.road));
        }
        out
    }

    pub fn add(&mut self, id: SegmentId, seg: &Segment, road: RoadZoning) {
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
                footpath: road.footpath,
            };
            let b = inflate((w[0].min(w[1]), w[0].max(w[1])), road.half_width);
            for key in bucket_range(b, ROAD_BUCKET) {
                self.buckets.entry(key).or_default().push((id, piece));
            }
            bounds = Some(bounds.map_or(b, |x| union(x, b)));
        }
        if let Some(b) = bounds {
            self.bounds.insert(id, b);
        }
    }

    /// Drops a segment's pieces; returns the bounds they had.
    pub fn remove(&mut self, id: SegmentId) -> Option<Bounds> {
        let bounds = self.bounds.remove(&id)?;
        for key in bucket_range(bounds, ROAD_BUCKET) {
            if let Some(v) = self.buckets.get_mut(&key) {
                v.retain(|(s, _)| *s != id);
                if v.is_empty() {
                    self.buckets.remove(&key);
                }
            }
        }
        Some(bounds)
    }

    pub fn pieces_in(&self, b: Bounds) -> impl Iterator<Item = &(SegmentId, RoadPiece)> {
        bucket_range(b, ROAD_BUCKET).flat_map(|k| self.buckets.get(&k).into_iter().flatten())
    }

    /// Segments whose road strip bounds touch `region`, in id order.
    pub fn segments_near(&self, region: Bounds) -> Vec<SegmentId> {
        self.bounds
            .iter()
            .filter(|(_, b)| {
                b.1.x >= region.0.x
                    && b.0.x <= region.1.x
                    && b.1.y >= region.0.y
                    && b.0.y <= region.1.y
            })
            .map(|(id, _)| *id)
            .collect()
    }
}
