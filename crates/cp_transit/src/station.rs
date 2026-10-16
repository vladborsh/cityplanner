//! The stop of a station building (T-162): made with the building, bound to its platform, gone
//! with it. The platform itself is laid by `cp_building` (`BuildingCommand::Place`).

use bevy_ecs::world::World;
use bevy_math::Vec2;
use cp_building::{BuildingGrid, BuildingId};
use cp_net::{NetGraph, SegmentId};
use cp_traffic::Side;

use crate::model::{Stop, StopId, StopKind, TransitNet};

/// The stop of station `building`, if it has one.
pub fn station_stop(transit: &TransitNet, building: BuildingId) -> Option<StopId> {
    transit.stops.iter().find_map(|(id, st)| match st.kind {
        StopKind::Station { building: b, .. } if b == building => Some(id),
        _ => None,
    })
}

/// The platform segment under station `building`.
pub fn platform_segment(transit: &TransitNet, building: BuildingId) -> Option<SegmentId> {
    station_stop(transit, building)
        .and_then(|id| transit.stops.get(id))
        .map(|st| st.kind.segment())
}

/// Makes the stop of the station `building` on its platform `seg`, halfway along it. The
/// pedestrian leg ends at the building's access point; `pos` is the building.
pub fn add_station(world: &mut World, building: BuildingId, seg: SegmentId) -> Option<StopId> {
    let pos: Vec2 = world.get_resource::<BuildingGrid>()?.get(building)?.pos;
    let length = world.get_resource::<NetGraph>()?.segment(seg)?.length;
    let mut transit = world.get_resource_mut::<TransitNet>()?;
    Some(transit.stops.insert(Stop {
        pos,
        side: Side::Right,
        kind: StopKind::Station {
            building,
            seg,
            s: length * 0.5,
        },
        lines: Vec::new(),
    }))
}

/// Takes the stop of station `building` out of the stops and the lines, and gives its platform
/// segment (the caller removes it from the network).
pub fn remove_station(world: &mut World, building: BuildingId) -> Option<SegmentId> {
    let mut transit = world.get_resource_mut::<TransitNet>()?;
    let stop = station_stop(&transit, building)?;
    let seg = transit.stops.get(stop)?.kind.segment();
    transit.remove_stop(stop);
    transit.relink_stops();
    if let Some(mut plans) = world.get_resource_mut::<crate::plan::LinePlans>() {
        plans.dirty = true;
    }
    Some(seg)
}
