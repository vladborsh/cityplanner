//! What changed in the city between two snapshots (T-287, T-288): buildings placed, finished,
//! removed or moved, and road segments laid or removed. Pure: a diff of the old and the new
//! `SimView` grids, no sim event stream; the emoji wave ([`crate::mood`]) and the dust puffs
//! read it.

use cp_building::{Building, BuildingGrid, BuildingId, BuildingState};
use cp_core::curve::CubicBezier3;
use cp_net::{NetGraph, SegmentId};

/// How far (metres) a building's centre must move to count as moved.
const MOVED_M: f32 = 0.5;

/// What happened to one building.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildingEvent {
    /// A new building (any state: a service starts under construction, a house as a site).
    Placed,
    /// A building became Active (finished, or placed Active).
    Activated,
    /// A building that stood is gone (bulldozed, demolished under a road, burnt down).
    Removed,
    /// An Active building that stays Active stands elsewhere (`Move`, T-177).
    Moved,
    /// An Active building is no longer Active (abandoned, burnt out).
    Deactivated,
}

/// One building's change: the building as it is now (or, removed, as it was), and for a move
/// where it stood.
#[derive(Clone, Debug, PartialEq)]
pub struct BuildingChange {
    pub id: BuildingId,
    pub event: BuildingEvent,
    pub building: Building,
    /// [`BuildingEvent::Moved`]: the building as it stood before.
    pub before: Option<Building>,
}

/// The building changes from `old` to `new`, in id order (a new Active building gives
/// `Placed` and then `Activated`).
pub fn building_events(old: &BuildingGrid, new: &BuildingGrid) -> Vec<BuildingChange> {
    let mut out = Vec::new();
    for (id, b) in new.iter() {
        let change = |event, before: Option<&Building>| BuildingChange {
            id,
            event,
            building: b.clone(),
            before: before.cloned(),
        };
        match old.get(id) {
            None => {
                out.push(change(BuildingEvent::Placed, None));
                if b.state == BuildingState::Active {
                    out.push(change(BuildingEvent::Activated, None));
                }
            }
            Some(o) => {
                let (was, is) = (
                    o.state == BuildingState::Active,
                    b.state == BuildingState::Active,
                );
                if !was && is {
                    out.push(change(BuildingEvent::Activated, None));
                } else if was && !is {
                    out.push(change(BuildingEvent::Deactivated, Some(o)));
                } else if was && is && (o.pos.distance(b.pos) > MOVED_M || o.access != b.access) {
                    out.push(change(BuildingEvent::Moved, Some(o)));
                }
            }
        }
    }
    for (id, o) in old.iter() {
        if new.get(id).is_none() {
            out.push(BuildingChange {
                id,
                event: BuildingEvent::Removed,
                building: o.clone(),
                before: None,
            });
        }
    }
    out.sort_by_key(|c| c.id);
    out
}

/// A road piece laid or removed: its centre line and half width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SegmentChange {
    pub id: SegmentId,
    pub curve: CubicBezier3,
    pub half_width: f32,
}

/// The ground segments laid and removed from `old` to `new`, by id. `half_width` reads the road
/// type's width through `width_of`.
pub fn segment_events(
    old: &NetGraph,
    new: &NetGraph,
    width_of: impl Fn(cp_content::RoadDefId) -> f32,
) -> (Vec<SegmentChange>, Vec<SegmentChange>) {
    let pick = |a: &NetGraph, b: &NetGraph| -> Vec<SegmentChange> {
        a.segments()
            .filter(|(id, s)| {
                s.elevation == cp_net::Elevation::Ground
                    && b.segment(*id).is_none_or(|t| t.curve != s.curve)
            })
            .map(|(id, s)| SegmentChange {
                id,
                curve: s.curve,
                half_width: width_of(s.road) * 0.5,
            })
            .collect()
    };
    (pick(new, old), pick(old, new))
}
