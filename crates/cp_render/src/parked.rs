//! Parked cars in the parking bays (T-276, render only: no agent, no sim effect, nothing saved).
//!
//! The bays are [`crate::road::parking::bays`]. Each [`SLOT_LEN`] place of a bay holds a car or
//! not by a hash of the segment, the side and the place (stable: no flicker as the sim ticks)
//! against how busy the buildings on that side next to it are ([`occupancy`]): an empty lot or a
//! building site has none, a full house or shop most places. A place in front of a service
//! building's access stays free. Pedestrians are drawn clear of the bays ([`BayIndex::clip`]) so
//! they never walk through the cars.

use bevy_math::{Vec2, Vec3};
use cp_building::{BuildingFlags, BuildingGrid, BuildingState};
use cp_content::ContentDb;
use cp_core::curve::to_xz;
use cp_net::{JunctionGeometry, NetGraph, SegmentId};

pub use crate::road::parking::DRIVEWAY_W;
use crate::road::parking::{BaySide, Bays, bays};

/// Pedestrians are drawn this far beyond a bay's outer edge, metres.
const NUDGE_CLEAR: f32 = 0.35;
/// Cell of the [`BayIndex`] grid, metres.
const INDEX_CELL: f32 = 32.0;

/// A parked car: centre on the road surface, heading (`atan2(dir.x, dir.z)`) and a stable key
/// for its colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParkedCar {
    pub pos: Vec3,
    pub yaw: f32,
    pub key: u64,
}

/// How busy a building is for the street in front of it, `0..=1`: an active building by its
/// level, less when it lacks workers or customers; anything else (a site, abandoned) 0.
pub fn occupancy(db: &ContentDb, b: &cp_building::Building) -> f32 {
    if b.state != BuildingState::Active {
        return 0.0;
    }
    let def = db.building(b.def);
    if def.service.is_some() {
        return 0.5;
    }
    let mut o = (0.45 + 0.15 * f32::from(b.level)).min(1.0);
    if b.flags.has(BuildingFlags::NO_WORKERS) || b.flags.has(BuildingFlags::NO_CUSTOMERS) {
        o *= 0.5;
    }
    o
}

/// One bay side of a segment, sampled for [`BayIndex`]: points of the street's axis along the
/// bay every few metres with the unit left direction there (the bay's range is lateral to it).
#[derive(Clone, Debug)]
struct Strip {
    points: Vec<(Vec2, Vec2)>,
    side: BaySide,
}

/// The bays by grid cell, for drawing pedestrians out of them.
#[derive(Clone, Debug, Default)]
pub struct BayIndex {
    strips: Vec<Strip>,
    cells: bevy::platform::collections::HashMap<(i32, i32), Vec<u32>>,
}

impl BayIndex {
    /// The part of `lo..=hi` that keeps `p + side × x` out of the bays near `p`, beyond each
    /// bay's outer edge by [`NUDGE_CLEAR`] (§25.3: the spread across a sidewalk stops at the
    /// bay instead of being pushed out of it). When nothing of the range is clear, both ends
    /// are the clear point nearest the range.
    pub fn clip(&self, p: Vec2, side: Vec2, lo: f32, hi: f32) -> (f32, f32) {
        let (mut lo, mut hi) = (lo, hi);
        let cell = (
            (p.x / INDEX_CELL).floor() as i32,
            (p.y / INDEX_CELL).floor() as i32,
        );
        let Some(list) = self.cells.get(&cell) else {
            return (lo, hi);
        };
        for &k in list {
            let strip = &self.strips[k as usize];
            for w in strip.points.windows(2) {
                let (a, b) = (w[0].0, w[1].0);
                let ab = b - a;
                let t = (p - a).dot(ab) / ab.length_squared().max(1e-6);
                if !(0.0..=1.0).contains(&t) {
                    continue;
                }
                let left = w[0].1.lerp(w[1].1, t).normalize_or_zero();
                let lateral = (p - a.lerp(b, t)).dot(left);
                // How far across the street one metre of `x` moves; walking across the
                // street (a crossing) the bay is not this walker's to keep clear of.
                let k = side.dot(left);
                if k.abs() < 0.3 {
                    continue;
                }
                let (blo, bhi) = strip.side.range();
                // Only the bay beside the walker's own sidewalk: the clear side is outward.
                let (edge, outward) = if strip.side.sign > 0.0 {
                    (bhi + NUDGE_CLEAR, 1.0)
                } else {
                    (blo - NUDGE_CLEAR, -1.0)
                };
                if (lateral - edge) * outward < -(bhi - blo) - NUDGE_CLEAR
                    || (lateral - edge) * outward > 8.0
                {
                    continue;
                }
                // `lateral + k x` must be on the outward side of `edge`.
                let bound = (edge - lateral) / k;
                if k * outward > 0.0 {
                    lo = lo.max(bound);
                    if lo > hi {
                        hi = lo;
                    }
                } else {
                    hi = hi.min(bound);
                    if hi < lo {
                        lo = hi;
                    }
                }
            }
        }
        (lo, hi)
    }

    fn add(&mut self, strip: Strip) {
        let k = self.strips.len() as u32;
        let mut seen = std::collections::BTreeSet::new();
        for &(q, _) in &strip.points {
            let c = (
                (q.x / INDEX_CELL).floor() as i32,
                (q.y / INDEX_CELL).floor() as i32,
            );
            for dx in -1..=1 {
                for dz in -1..=1 {
                    if seen.insert((c.0 + dx, c.1 + dz)) {
                        self.cells.entry((c.0 + dx, c.1 + dz)).or_default().push(k);
                    }
                }
            }
        }
        self.strips.push(strip);
    }
}

/// Side of the [`ParkedBucket`]s, metres.
const BUCKET_M: f32 = 128.0;

/// The parked cars of one square of the map, with a sphere round them: a frame culls the
/// square before its cars.
#[derive(Clone, Debug, Default)]
pub struct ParkedBucket {
    pub center: Vec3,
    pub radius: f32,
    pub cars: Vec<ParkedCar>,
}

/// `cars` in [`BUCKET_M`] squares.
pub fn bucketed(cars: &[ParkedCar]) -> Vec<ParkedBucket> {
    let mut by_cell: std::collections::BTreeMap<(i32, i32), Vec<ParkedCar>> = Default::default();
    for c in cars {
        let cell = (
            (c.pos.x / BUCKET_M).floor() as i32,
            (c.pos.z / BUCKET_M).floor() as i32,
        );
        by_cell.entry(cell).or_default().push(*c);
    }
    by_cell
        .into_values()
        .map(|cars| {
            let center = cars.iter().map(|c| c.pos).sum::<Vec3>() / cars.len() as f32;
            let radius = cars
                .iter()
                .map(|c| c.pos.distance(center))
                .fold(0.0f32, f32::max);
            ParkedBucket {
                center,
                radius,
                cars,
            }
        })
        .collect()
}

/// The parked cars of the sim (T-331b2: `Agents::parked`, one per citizen, each in the place the
/// sim reserved) and the index of the bays. A car in a kerb place stands in that bay slot; a car
/// in a yard or a lot stands in rows behind the building's gate or across its footprint.
pub fn parked_cars(
    graph: &NetGraph,
    geo: &JunctionGeometry,
    db: &ContentDb,
    buildings: &BuildingGrid,
    parked: &cp_traffic::ParkedView,
) -> (Vec<ParkedCar>, BayIndex) {
    use cp_traffic::{ParkPlace, side_index};
    let mut kerb: std::collections::BTreeMap<(SegmentId, usize), Vec<(u16, u64)>> =
        Default::default();
    let mut yards: std::collections::BTreeMap<cp_building::BuildingId, Vec<u64>> =
        Default::default();
    for (citizen, car) in &parked.cars {
        let key = splitmix(u64::from(citizen.index) << 32 | u64::from(citizen.generation));
        match car.place {
            ParkPlace::Kerb {
                segment,
                side,
                slot,
            } => kerb
                .entry((segment, usize::from(side)))
                .or_default()
                .push((slot, key)),
            ParkPlace::Building(id) => yards.entry(id).or_default().push(key),
        }
    }
    let mut cars = Vec::new();
    let mut index = BayIndex::default();
    for (id, _) in graph.segments() {
        let Some(Bays { sides, a, b }) = bays(graph, geo, db, id) else {
            continue;
        };
        let Some(curve) = geo.trimmed_curve(graph, id) else {
            continue;
        };
        let table = curve.arc_table();
        for side in &sides {
            let at = |s: f32| {
                let t = table.s_to_t(s);
                let tangent = to_xz(curve.tangent(t)).normalize_or_zero();
                (curve.point(t), tangent, Vec2::new(tangent.y, -tangent.x))
            };
            let samples = ((b - a) / 4.0).ceil().max(1.0) as usize;
            index.add(Strip {
                points: (0..=samples)
                    .map(|i| {
                        let (c, _, left) = at(a + (b - a) * i as f32 / samples as f32);
                        (to_xz(c), left)
                    })
                    .collect(),
                side: *side,
            });
            let k = side_index(side.sign);
            for &(slot, key) in kerb.get(&(id, k)).map_or(&[][..], |v| v.as_slice()) {
                let Some(s) = parked.index.kerb_bay_s(id, k, slot) else {
                    continue;
                };
                let (c, tangent, left) = at(s);
                let p = to_xz(c) + left * side.car_offset();
                // Facing the traffic of its side: forward on the right, backward on the left.
                let dir = tangent * -side.sign;
                cars.push(ParkedCar {
                    pos: Vec3::new(p.x, c.y + crate::road::LIFT, p.y),
                    yaw: dir.x.atan2(dir.y),
                    key,
                });
            }
        }
    }
    for (id, keys) in yards {
        let Some(b) = buildings.get(id) else { continue };
        let Some(access) = b.access() else { continue };
        let Some(seg) = graph.segment(access.segment) else {
            continue;
        };
        let y = seg.curve.point(access.t.clamp(0.0, 1.0)).y + crate::road::LIFT;
        let service = db.building_service(b.def).is_some();
        let (w, d) = b.size();
        let (sn, cs) = b.rot.sin_cos();
        let (ax, ay) = (Vec2::new(cs, sn), Vec2::new(-sn, cs));
        // A lot or a garage fills its footprint in rows; a yard stands behind the gate.
        let (origin, row_dir, col_dir, cols, nose) = if service {
            let (hw, hd) = (
                f32::from(w) * cp_building::FOOTPRINT_CELL_M * 0.5 - 2.0,
                f32::from(d) * cp_building::FOOTPRINT_CELL_M * 0.5 - 3.0,
            );
            let cols = ((2.0 * hw / CAR_PITCH).floor() as usize).max(1);
            (
                b.pos - ax * hw - ay * hd,
                ay * CAR_ROW,
                ax * CAR_PITCH,
                cols,
                ay,
            )
        } else if let Some(p) = b.portals(graph) {
            let t = Vec2::new(p.out.y, -p.out.x);
            (
                p.drive - p.out * 3.0,
                -p.out * CAR_ROW,
                t * CAR_PITCH,
                3,
                p.out,
            )
        } else {
            continue;
        };
        for (i, key) in keys.into_iter().enumerate() {
            let p = origin + col_dir * (i % cols) as f32 + row_dir * (i / cols) as f32;
            cars.push(ParkedCar {
                pos: Vec3::new(p.x, y, p.y),
                yaw: nose.x.atan2(nose.y),
                key,
            });
        }
    }
    (cars, index)
}

/// Spacing of cars across a row and between rows in a yard or a lot, metres.
const CAR_PITCH: f32 = 2.6;
const CAR_ROW: f32 = 5.4;

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}
