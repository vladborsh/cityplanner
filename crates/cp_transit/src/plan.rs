//! The route of a line: derived from the stops and the road network, never saved (§16 M3 A
//! "Маршрут линии — производное").

use std::collections::BTreeMap;

use bevy_ecs::resource::Resource;
use cp_traffic::{Access, CarPath, CongestionMap, PathGraph, find_car_path};

use crate::model::{LineId, StopId};

/// One leg: the way from a stop to the next one of the ring.
#[derive(Clone, Debug, PartialEq)]
pub struct Leg {
    pub from: StopId,
    pub to: StopId,
    pub path: CarPath,
    pub length_m: f32,
    /// Estimated travel time on an empty road, seconds.
    pub time_s: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanError {
    /// Fewer than two stops.
    TooFewStops,
    /// No way from stop `from` to stop `to` (leg `leg` of the ring).
    NoPath {
        leg: usize,
        from: StopId,
        to: StopId,
    },
}

/// The route of a line. `legs` holds one leg per stop (the last one closes the ring) when
/// every leg has a path; `errors` names the legs that have none.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinePlan {
    pub legs: Vec<Leg>,
    pub length_m: f32,
    pub cycle_s: f32,
    pub errors: Vec<PlanError>,
}

impl LinePlan {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Routes the ring `stops[0] → stops[1] → … → stops[0]` on `graph`, one A* per leg, on an
/// empty-road cost (no congestion). Synchronous: a line has a handful of legs and the plan
/// is redone only when the network changes. The same function gives the tool's ghost.
pub fn plan_line(graph: &PathGraph, stops: &[(StopId, Access)]) -> LinePlan {
    let mut plan = LinePlan::default();
    if stops.len() < 2 {
        plan.errors.push(PlanError::TooFewStops);
        return plan;
    }
    let cong = CongestionMap::default();
    for (leg, (&(from, ref a), &(to, ref b))) in stops
        .iter()
        .zip(stops.iter().cycle().skip(1))
        .take(stops.len())
        .enumerate()
    {
        match find_car_path(graph, &cong, a, b) {
            Ok(path) => {
                let length_m = graph.car_path_length(&path);
                plan.length_m += length_m;
                plan.cycle_s += path.cost;
                plan.legs.push(Leg {
                    from,
                    to,
                    time_s: path.cost,
                    length_m,
                    path,
                });
            }
            Err(_) => plan.errors.push(PlanError::NoPath { leg, from, to }),
        }
    }
    plan
}

/// The plans of all lines, by line. Derived state: rebuilt by [`crate::update_transit`] after
/// every rebuild of the `PathGraph` and after a load; not saved, not in the checksum.
#[derive(Resource, Clone, Debug, Default)]
pub struct LinePlans {
    pub(crate) by_line: BTreeMap<LineId, LinePlan>,
    /// `PathNetwork::generation` the plans were made on; `0` = none yet.
    pub(crate) generation: u64,
    /// The stops have been attached to the network since the load (false until the first
    /// run of [`crate::update_transit`]).
    pub(crate) bound: bool,
    /// Stops or lines changed since the plans were made.
    pub(crate) dirty: bool,
}

impl LinePlans {
    pub fn get(&self, line: LineId) -> Option<&LinePlan> {
        self.by_line.get(&line)
    }

    pub fn iter(&self) -> impl Iterator<Item = (LineId, &LinePlan)> {
        self.by_line.iter().map(|(id, p)| (*id, p))
    }

    /// Marks the plans out of date (the lines changed).
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}
