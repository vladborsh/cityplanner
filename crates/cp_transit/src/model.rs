//! The serialized transit state (§16 M3 A "Данные").

use bevy_ecs::resource::Resource;
use bevy_math::Vec2;
use cp_building::BuildingId;
use cp_core::{Arena, Id};
use cp_net::SegmentId;
use cp_traffic::Side;
use serde::{Deserialize, Serialize};

pub type StopId = Id<Stop>;
pub type LineId = Id<Line>;

/// Stops one line may have.
pub const MAX_LINE_STOPS: usize = 32;
/// Colours of the line palette (`Line::color` is an index below this).
pub const PALETTE_LEN: u8 = 12;
/// Vehicles a new line starts with. // tune later
pub const DEFAULT_LINE_VEHICLES: u8 = 2;
/// Most vehicles a line may be given until the vehicle definitions carry their own limit
/// (T-156). // tune later
pub const MAX_LINE_VEHICLES: u8 = 20;

/// Where a stop stands.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum StopKind {
    /// On the pavement of a road segment, `s` metres from the segment start.
    Road { seg: SegmentId, s: f32 },
    /// A station with its own tracks (metro, train). Only the type exists in T-153; T-162
    /// fills it.
    Station {
        building: BuildingId,
        seg: SegmentId,
        s: f32,
    },
}

impl StopKind {
    pub fn segment(&self) -> SegmentId {
        match *self {
            StopKind::Road { seg, .. } | StopKind::Station { seg, .. } => seg,
        }
    }

    pub fn s(&self) -> f32 {
        match *self {
            StopKind::Road { s, .. } | StopKind::Station { s, .. } => s,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stop {
    /// Where the pavement point is, metres on the ground plane.
    pub pos: Vec2,
    pub side: Side,
    pub kind: StopKind,
    /// Lines that call here, ascending.
    pub lines: Vec<LineId>,
}

/// Append-only (saved as the discriminant).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum TransitMode {
    Bus = 0,
    Metro = 1,
    Train = 2,
}

/// Append-only. `Broken` and `Ok` follow the route; `NoDepot` is set by T-156.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineState {
    Ok,
    Broken,
    NoDepot,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub mode: TransitMode,
    pub name: String,
    /// Index into the palette, `0..PALETTE_LEN`.
    pub color: u8,
    /// The ring: after the last stop comes the first.
    pub stops: Vec<StopId>,
    pub vehicles: u8,
    pub state: LineState,
}

/// All stops and lines. Serialized sim state, part of the checksum.
#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TransitNet {
    pub stops: Arena<Stop>,
    pub lines: Arena<Line>,
}

impl TransitNet {
    pub fn is_empty(&self) -> bool {
        self.stops.is_empty() && self.lines.is_empty()
    }

    /// The access point the pathfinder routes a bus to.
    pub fn access(&self, stop: StopId) -> Option<cp_traffic::Access> {
        let st = self.stops.get(stop)?;
        Some(cp_traffic::Access {
            segment: st.kind.segment(),
            s: st.kind.s(),
            side: st.side,
        })
    }

    /// Takes `stop` out of the stops and out of every line.
    pub fn remove_stop(&mut self, stop: StopId) {
        if self.stops.remove(stop).is_none() {
            return;
        }
        for (_, line) in self.lines.iter_mut() {
            line.stops.retain(|&s| s != stop);
        }
    }

    /// Re-derives `Stop::lines` from the lines (after any edit of them).
    pub fn relink_stops(&mut self) {
        for (_, st) in self.stops.iter_mut() {
            st.lines.clear();
        }
        let links: Vec<(LineId, Vec<StopId>)> = self
            .lines
            .iter()
            .map(|(id, l)| (id, l.stops.clone()))
            .collect();
        for (line, stops) in links {
            for s in stops {
                if let Some(st) = self.stops.get_mut(s)
                    && !st.lines.contains(&line)
                {
                    st.lines.push(line);
                }
            }
        }
    }
}
