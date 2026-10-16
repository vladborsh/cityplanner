//! The buses out on lines and what they cost (§16 M3 A "Депо и автобусы"). Serialized sim
//! state; the vehicles themselves are keep runs of `cp_traffic` (T-154), a bus here is the
//! record of which run belongs to which line and where in the ring it is.

use std::collections::BTreeMap;

use bevy_ecs::resource::Resource;
use cp_building::BuildingId;
use cp_core::{Arena, Cost, Id, Tick};
use cp_traffic::RunId;
use serde::{Deserialize, Serialize};

use crate::model::LineId;

pub type BusId = Id<Bus>;

/// What a bus is doing. Append-only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BusPhase {
    /// Driving from the depot to the first stop of the line.
    Leaving,
    /// Driving to stop `idx` of the ring.
    Driving { idx: u16 },
    /// Standing at stop `idx` until `until`.
    Dwell { idx: u16, until: Tick },
    /// Driving back to the depot; removed on arrival.
    ToDepot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bus {
    pub line: LineId,
    pub depot: BuildingId,
    /// The keep run that drives it.
    pub run: RunId,
    pub phase: BusPhase,
}

/// All buses, and when each line last sent one out.
#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TransitFleet {
    pub buses: Arena<Bus>,
    pub last_out: BTreeMap<LineId, Tick>,
}

impl TransitFleet {
    pub fn is_empty(&self) -> bool {
        self.buses.is_empty() && self.last_out.is_empty()
    }

    /// The buses of `line` in id order.
    pub fn of_line(&self, line: LineId) -> impl Iterator<Item = (BusId, &Bus)> {
        self.buses.iter().filter(move |(_, b)| b.line == line)
    }
}

/// Money flows of transit, summed over the current week. The economy takes them at the week's
/// end (T-159); until then they only add up.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitLedger {
    /// Upkeep of the buses out on lines this week.
    pub upkeep: Cost,
    /// Fares taken at boarding this week (T-159). Added in v31.
    pub fares: Cost,
}

impl TransitLedger {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}
