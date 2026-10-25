//! The Parking overlay (T-331c, `architecture/system-design.md` §18.3): how full the places are.
//! The bays of every street are tinted by the share of their places taken (green free → red full),
//! and so are the lots of the buildings with a yard, a lot or a garage; a building a car found no
//! place for (`NO_PARKING`) is purple. A pure function of the sim snapshot, like the other layers.

use bevy_math::Vec2;
use cp_building::{BuildingFlags, BuildingGrid, BuildingState};
use cp_content::ContentDb;
use cp_core::curve::to_xz;
use cp_net::kerb::{Bays, bays};
use cp_net::{JunctionGeometry, NetGraph};
use cp_terrain::Terrain;
use cp_traffic::{ParkingIndex, side_index};
use cp_zone::ZoneGrid;

use super::utility::lot_quads;
use super::{flat, ramp};
use crate::chunk::MeshData;

/// The tint of places `0` (all free) to `1` (all taken).
pub fn parking_color(v: f32) -> [f32; 4] {
    ramp(
        v,
        &[
            [0.20, 0.75, 0.25, 0.80],
            [0.95, 0.80, 0.15, 0.80],
            [0.85, 0.20, 0.15, 0.80],
        ],
    )
}

/// A building with no place for its cars.
const NO_PLACE: [f32; 4] = [0.60, 0.15, 0.65, 0.85];
/// Length of one quad of a bay strip, metres.
const STEP_M: f32 = 3.0;

/// The bays and the lots, each tinted by how full its places are.
pub fn parking_overlay_mesh(
    index: &ParkingIndex,
    net: &NetGraph,
    geo: &JunctionGeometry,
    db: &ContentDb,
    buildings: &BuildingGrid,
    zones: &ZoneGrid,
    terrain: &Terrain,
) -> MeshData {
    let mut m = MeshData::default();
    for (id, _) in net.segments() {
        let Some(Bays { sides, a, b }) = bays(net, geo, db, id) else {
            continue;
        };
        let Some(curve) = geo.trimmed_curve(net, id) else {
            continue;
        };
        let table = curve.arc_table();
        for side in &sides {
            let place = index.kerb(id, side_index(side.sign));
            if place.cap == 0 {
                continue;
            }
            let color = parking_color(f32::from(place.used) / f32::from(place.cap));
            let (lo, hi) = side.range();
            let n = (((b - a) / STEP_M).ceil() as usize).max(1);
            let edge = |s: f32, lateral: f32| {
                let t = table.s_to_t(s);
                let tangent = to_xz(curve.tangent(t)).normalize_or_zero();
                let left = Vec2::new(tangent.y, -tangent.x);
                let p = to_xz(curve.point(t)) + left * lateral;
                [p.x, curve.point(t).y + crate::road::LIFT + 0.1, p.y]
            };
            for i in 0..n {
                let s0 = a + (b - a) * i as f32 / n as f32;
                let s1 = a + (b - a) * (i + 1) as f32 / n as f32;
                flat(
                    &mut m,
                    [edge(s0, lo), edge(s1, lo), edge(s1, hi), edge(s0, hi)],
                    color,
                );
            }
        }
    }
    for (id, b) in buildings.iter() {
        if b.state != BuildingState::Active {
            continue;
        }
        let place = index.building(id);
        let color = if b.flags.has(BuildingFlags::NO_PARKING) {
            NO_PLACE
        } else if place.cap > 0 {
            parking_color(f32::from(place.used) / f32::from(place.cap))
        } else {
            continue;
        };
        lot_quads(&mut m, b, zones, terrain, color);
    }
    m
}
