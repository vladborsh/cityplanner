//! Parking places (`architecture/system-design.md` §18.3, T-331a): the supply, counted, not
//! simulated. Two sources here: the **kerb** (the bays of `cp_net::kerb`, one place per
//! [`SLOT_LEN`] of a bay, minus those a service building's access or a yard gate overlaps) and
//! the **buildings** (a yard, a lot, a garage: `BuildingDef.parking`, or the rule of the balance
//! from households and jobs). [`ParkingIndex`] holds `cap` and `used` of each; the choice and the
//! reservation of a place are T-331b. Saved (T-331b: which cars it counts depends on the history
//! of the trips, and the next rebuild must come on the schedule of the game that went on).
//!
//! The kerb places are the same ones the render draws parked cars in (`cp_render::parked` uses
//! [`kerb_fronts`] and [`slot_blocked`] too), so a place drawn is a place counted.

use std::collections::{BTreeMap, BTreeSet};

use crate::agents::Agents;
use crate::graph::{Access, Side};
use bevy_ecs::prelude::*;
use bevy_math::Vec2;
use cp_building::{Building, BuildingGrid, BuildingId, BuildingState};
use cp_citizen::{CitizenGrid, CitizenId, ParkPlace, TripBook};
use cp_content::{ContentDb, ServiceKind, SimContent, ZoneType};
use cp_core::Tick;
use cp_core::curve::to_xz;
use cp_net::kerb::{Bays, GATE_CLEAR, SLOT_LEN, bays};
use cp_net::{JunctionGeometry, NetGraph, SegmentId};
use serde::{Deserialize, Serialize};

/// The index is rebuilt every this many ticks (and on its first run).
pub const PARKING_PERIOD: u64 = 64;

/// One building seen from a kerb side: where along the street it stands and where its yard gate
/// is, on the trimmed curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Front {
    /// Arc length of the building's access point.
    pub s: f32,
    pub building: BuildingId,
    /// A service building: the places in front of its access stay free (it is where the
    /// service vehicles stop).
    pub service: bool,
    /// Arc length of its yard gate (§25.1): cars pull out there.
    pub gate: Option<f32>,
}

/// The buildings of each `(segment, side)` (`side` is `1` left of the street, `-1` right) by
/// their access point, ascending by arc length.
pub fn kerb_fronts(
    net: &NetGraph,
    geo: &JunctionGeometry,
    db: &ContentDb,
    buildings: &BuildingGrid,
) -> BTreeMap<(SegmentId, i8), Vec<Front>> {
    let mut out: BTreeMap<(SegmentId, i8), Vec<Front>> = BTreeMap::new();
    for (id, b) in buildings.iter() {
        let Some(access) = b.access() else { continue };
        let (Some(seg), Some(curve)) = (
            net.segment(access.segment),
            geo.trimmed_curve(net, access.segment),
        ) else {
            continue;
        };
        let at = to_xz(seg.curve.point(access.t.clamp(0.0, 1.0)));
        let (t, _) = curve.nearest_xz(at);
        let s = curve.arc_table().t_to_s(t);
        let tangent = to_xz(curve.tangent(t)).normalize_or_zero();
        let left = Vec2::new(tangent.y, -tangent.x);
        let side = if (b.pos - at).dot(left) >= 0.0 { 1 } else { -1 };
        let gate = b.portals(net).map(|p| {
            let (t, _) = curve.nearest_xz(p.drive);
            curve.arc_table().t_to_s(t)
        });
        out.entry((access.segment, side)).or_default().push(Front {
            s,
            building: id,
            service: db.building(b.def).service.is_some(),
            gate,
        });
    }
    for v in out.values_mut() {
        v.sort_by(|a, c| a.s.total_cmp(&c.s));
    }
    out
}

/// Whether the place at arc length `s` of a bay side is not a place: in front of a service
/// building's access, or across a yard gate (§25.1: a place that overlaps its driveway is left
/// free).
pub fn slot_blocked(near: &[Front], s: f32) -> bool {
    near.iter().any(|f| f.service && (f.s - s).abs() < SLOT_LEN)
        || near
            .iter()
            .any(|f| f.gate.is_some_and(|g| (g - s).abs() < GATE_CLEAR))
}

/// Capacity and use of one supply of places.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub cap: u16,
    pub used: u16,
}

impl Place {
    pub fn free(&self) -> u16 {
        self.cap.saturating_sub(self.used)
    }
}

/// The index of the side of a street: `0` right of its direction, `1` left.
pub fn side_index(sign: f32) -> usize {
    usize::from(sign > 0.0)
}

/// Kerb and building places, dense by segment and by building slot.
#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ParkingIndex {
    kerb: Vec<[Place; 2]>,
    buildings: Vec<Place>,
    /// Per segment and side, the `Access.s` of each place (the arc length on the segment's own
    /// curve), in the order of the bay.
    slots: Vec<[Vec<f32>; 2]>,
    /// The lots and garages (buildings with `ServiceKind::Parking`) by the segment of their
    /// access point.
    lots: BTreeMap<SegmentId, Vec<BuildingId>>,
    /// The cars standing in or on their way to each place; `used` is counted from it.
    occupied: BTreeMap<ParkPlace, u16>,
    /// The tick of the last rebuild (`None` before the first).
    built: Option<u64>,
}

impl ParkingIndex {
    /// The kerb places of a side of a segment (`side` as [`side_index`]).
    pub fn kerb(&self, seg: SegmentId, side: usize) -> Place {
        self.kerb
            .get(seg.index as usize)
            .map_or(Place::default(), |s| s[side.min(1)])
    }

    /// The places of a building.
    pub fn building(&self, id: BuildingId) -> Place {
        self.buildings
            .get(id.index as usize)
            .copied()
            .unwrap_or_default()
    }

    /// `(capacity, used)` of every kerb place.
    pub fn kerb_total(&self) -> (u32, u32) {
        self.kerb.iter().flatten().fold((0, 0), |a, p| {
            (a.0 + u32::from(p.cap), a.1 + u32::from(p.used))
        })
    }

    /// `(capacity, used)` of every building's places.
    pub fn building_total(&self) -> (u32, u32) {
        self.buildings.iter().fold((0, 0), |a, p| {
            (a.0 + u32::from(p.cap), a.1 + u32::from(p.used))
        })
    }

    /// The lots and garages whose access point is on `seg`, ascending by id.
    pub fn lots_on(&self, seg: SegmentId) -> &[BuildingId] {
        self.lots.get(&seg).map_or(&[], |v| v.as_slice())
    }

    /// `Access.s` (metres from the segment start) of a kerb place; `None` for a place that is no
    /// longer there.
    pub fn kerb_s(&self, seg: SegmentId, side: usize, slot: u16) -> Option<f32> {
        self.slots
            .get(seg.index as usize)?
            .get(side.min(1))?
            .get(usize::from(slot))
            .copied()
    }

    /// Whether the place still exists (a kerb slot inside the bay, a building with places).
    pub fn exists(&self, place: ParkPlace) -> bool {
        match place {
            ParkPlace::Kerb {
                segment,
                side,
                slot,
            } => slot < self.kerb(segment, usize::from(side)).cap,
            ParkPlace::Building(id) => self.building(id).cap > 0,
        }
    }

    /// Takes the first free place of a kerb side (so reservations are a function of the index
    /// alone); `None` when the side is full.
    pub fn take_kerb(&mut self, seg: SegmentId, side: usize) -> Option<ParkPlace> {
        let side = side.min(1);
        let cap = self.kerb(seg, side).cap;
        let slot = (0..cap).find(|&slot| {
            !self.occupied.contains_key(&ParkPlace::Kerb {
                segment: seg,
                side: side as u8,
                slot,
            })
        })?;
        let place = ParkPlace::Kerb {
            segment: seg,
            side: side as u8,
            slot,
        };
        self.occupy(place);
        Some(place)
    }

    /// Takes a free place of a building.
    pub fn take_building(&mut self, id: BuildingId) -> Option<ParkPlace> {
        if self.building(id).free() == 0 {
            return None;
        }
        let place = ParkPlace::Building(id);
        self.occupy(place);
        Some(place)
    }

    /// Marks one more car in `place`.
    fn occupy(&mut self, place: ParkPlace) {
        *self.occupied.entry(place).or_insert(0) += 1;
        match place {
            ParkPlace::Kerb { segment, side, .. } => {
                if let Some(s) = self.kerb.get_mut(segment.index as usize) {
                    s[usize::from(side).min(1)].used += 1;
                }
            }
            ParkPlace::Building(id) => {
                if let Some(p) = self.buildings.get_mut(id.index as usize) {
                    p.used += 1;
                }
            }
        }
    }

    /// Gives a place back.
    pub fn release(&mut self, place: ParkPlace) {
        match self.occupied.get_mut(&place) {
            Some(n) if *n > 1 => *n -= 1,
            Some(_) => {
                self.occupied.remove(&place);
            }
            None => return,
        }
        match place {
            ParkPlace::Kerb { segment, side, .. } => {
                if let Some(s) = self.kerb.get_mut(segment.index as usize) {
                    let p = &mut s[usize::from(side).min(1)];
                    p.used = p.used.saturating_sub(1);
                }
            }
            ParkPlace::Building(id) => {
                if let Some(p) = self.buildings.get_mut(id.index as usize) {
                    p.used = p.used.saturating_sub(1);
                }
            }
        }
    }

    /// Replaces the cars the index counts with `cars` (the parked cars and the reservations of
    /// the trips on the road: the authoritative state) and counts `used` from them. Places that
    /// are gone count for nothing.
    pub fn sync(&mut self, cars: impl IntoIterator<Item = ParkPlace>) {
        self.occupied.clear();
        for c in cars {
            *self.occupied.entry(c).or_insert(0) += 1;
        }
        self.count_used();
    }

    /// `used` of each supply, from the cars; a place past the capacity is dropped.
    fn count_used(&mut self) {
        for s in &mut self.kerb {
            s[0].used = 0;
            s[1].used = 0;
        }
        for p in &mut self.buildings {
            p.used = 0;
        }
        let occupied = std::mem::take(&mut self.occupied);
        self.occupied = occupied
            .into_iter()
            .filter(|(place, _)| self.exists(*place))
            .collect();
        for (&place, &n) in &self.occupied {
            match place {
                ParkPlace::Kerb { segment, side, .. } => {
                    self.kerb[segment.index as usize][usize::from(side).min(1)].used += n;
                }
                ParkPlace::Building(id) => self.buildings[id.index as usize].used += n,
            }
        }
        // A yard holds no more cars than it has places (cars sent there before it shrank).
        for p in &mut self.buildings {
            p.used = p.used.min(p.cap);
        }
    }

    /// Counts the places of the net and the buildings as they stand; the cars stay where
    /// their place is still there.
    pub fn rebuild(
        &mut self,
        net: &NetGraph,
        geo: &JunctionGeometry,
        db: &ContentDb,
        buildings: &BuildingGrid,
    ) {
        let fronts = kerb_fronts(net, geo, db, buildings);
        let n = net
            .segments()
            .map(|(id, _)| id.index as usize + 1)
            .max()
            .unwrap_or(0);
        let mut kerb = vec![[Place::default(); 2]; n];
        let mut slots: Vec<[Vec<f32>; 2]> = vec![Default::default(); n];
        for (id, seg) in net.segments() {
            let Some(Bays { sides, a, b }) = bays(net, geo, db, id) else {
                continue;
            };
            let bay = Bays {
                sides: sides.clone(),
                a,
                b,
            };
            let all = bay.slots();
            let trimmed = geo.trimmed_curve(net, id);
            for side in &sides {
                let near: &[Front] = fronts
                    .get(&(id, side.sign as i8))
                    .map_or(&[], |v| v.as_slice());
                let k = side_index(side.sign);
                let free: Vec<f32> = all
                    .iter()
                    .copied()
                    .filter(|&s| !slot_blocked(near, s))
                    .filter_map(|s| {
                        // The path service measures along the segment's own curve, the bays
                        // along the trimmed one: go through the point.
                        let trimmed = trimmed.as_ref()?;
                        let at = to_xz(trimmed.point(trimmed.arc_table().s_to_t(s)));
                        let (t, _) = seg.curve.nearest_xz(at);
                        Some(seg.curve.arc_table().t_to_s(t))
                    })
                    .collect();
                kerb[id.index as usize][k] = Place {
                    cap: free.len() as u16,
                    used: 0,
                };
                slots[id.index as usize][k] = free;
            }
        }
        let m = buildings
            .iter()
            .map(|(id, _)| id.index as usize + 1)
            .max()
            .unwrap_or(0);
        let mut places = vec![Place::default(); m];
        let mut lots: BTreeMap<SegmentId, Vec<BuildingId>> = BTreeMap::new();
        for (id, b) in buildings.iter() {
            let cap = building_places(db, b);
            places[id.index as usize] = Place { cap, used: 0 };
            if cap > 0
                && db
                    .building_service(b.def)
                    .is_some_and(|sid| db.service(sid).kind == ServiceKind::Parking)
                && let Some(access) = b.access()
            {
                lots.entry(access.segment).or_default().push(id);
            }
        }
        self.kerb = kerb;
        self.buildings = places;
        self.slots = slots;
        self.lots = lots;
        self.count_used();
    }
}

/// The parking places of an active building: `BuildingDef.parking` when it says so, else the
/// balance rule from households (low density `parking_per_home_low`, high density
/// `parking_per_home_high`) and jobs (`parking_per_job`), each rounded up; a site or an
/// abandoned building has none.
pub fn building_places(db: &ContentDb, b: &Building) -> u16 {
    if b.state != BuildingState::Active {
        return 0;
    }
    let def = db.building(b.def);
    if def.parking > 0 {
        return def.parking;
    }
    let bal = db.balance();
    let per_home = match def.zone {
        ZoneType::ResHigh => bal.parking_per_home_high,
        _ => bal.parking_per_home_low,
    };
    let jobs: u32 = def.jobs.iter().map(|&j| u32::from(j)).sum();
    let homes = (f32::from(def.households) * per_home).ceil();
    let work = (jobs as f32 * bal.parking_per_job).ceil();
    ((homes + work) as u32).min(u32::from(u16::MAX)) as u16
}

/// How far from the goal a car looks for a place, metres (§18.3).
pub const PARK_SEARCH_M: f32 = 300.0;
/// The search looks at no more than this many segments.
pub const PARK_SEGMENTS: usize = 64;
/// Width of one ring of the search, metres: within a ring the lower segment id goes first.
const PARK_RING_M: f32 = 100.0;
/// Places a trip may give up (no route to one) before it drives to the goal and parks
/// abstractly.
pub const PARK_RETRIES: u8 = 2;
/// Cars the arena holds (desktop).
pub const PARKED_CAP: usize = 65_536;

/// A car that stands in a place while its owner is on foot (§18.3).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParkedCar {
    pub place: ParkPlace,
    pub since: Tick,
}

/// The parked cars, one per citizen, ascending by citizen. Saved and in the checksum (it lives
/// in `Agents`); the occupancy of [`ParkingIndex`] is counted from it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ParkedCars {
    cars: BTreeMap<CitizenId, ParkedCar>,
}

impl ParkedCars {
    pub fn get(&self, citizen: CitizenId) -> Option<&ParkedCar> {
        self.cars.get(&citizen)
    }

    pub fn len(&self) -> usize {
        self.cars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cars.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (CitizenId, &ParkedCar)> {
        self.cars.iter().map(|(c, p)| (*c, p))
    }

    /// Parks the citizen's car, in place of the one it had (a citizen has one car); `false`
    /// when the arena is full.
    pub fn park(&mut self, citizen: CitizenId, car: ParkedCar) -> bool {
        if self.cars.len() >= PARKED_CAP && !self.cars.contains_key(&citizen) {
            return false;
        }
        self.cars.insert(citizen, car);
        true
    }

    /// Takes the citizen's car out of its place.
    pub fn take(&mut self, citizen: CitizenId) -> Option<ParkedCar> {
        self.cars.remove(&citizen)
    }

    /// Forgets the cars of citizens that are gone.
    pub fn retain_citizens(&mut self, alive: impl Fn(CitizenId) -> bool) {
        self.cars.retain(|c, _| alive(*c));
    }
}

/// Chooses and takes a place for a car going to `goal` (§18.3): a place of the goal's own (its
/// yard), else the nearest free kerb place or lot within [`PARK_SEARCH_M`] of the goal's
/// access point, looking at the segments in rings of 100 m, the lower id first, at most
/// [`PARK_SEGMENTS`] of them. `None` when there is none: the car parks abstractly.
pub fn choose_place(
    index: &mut ParkingIndex,
    net: &NetGraph,
    buildings: &BuildingGrid,
    goal: BuildingId,
) -> Option<ParkPlace> {
    if let Some(p) = index.take_building(goal) {
        return Some(p);
    }
    let access = buildings.get(goal)?.access()?;
    let mut dist: BTreeMap<SegmentId, f32> = BTreeMap::new();
    let mut open: BTreeSet<(u32, SegmentId)> = BTreeSet::new();
    let ring = |d: f32| (d / PARK_RING_M) as u32;
    dist.insert(access.segment, 0.0);
    open.insert((0, access.segment));
    let mut seen = 0;
    while let Some((_, seg)) = open.pop_first() {
        seen += 1;
        if seen > PARK_SEGMENTS {
            break;
        }
        for side in 0..2 {
            if let Some(p) = index.take_kerb(seg, side) {
                return Some(p);
            }
        }
        let lots: Vec<BuildingId> = index.lots_on(seg).to_vec();
        for lot in lots {
            if let Some(p) = index.take_building(lot) {
                return Some(p);
            }
        }
        let d = dist[&seg];
        let Some(segment) = net.segment(seg) else {
            continue;
        };
        for node in [segment.start, segment.end] {
            for &next in net.node(node).map_or(&[][..], |n| n.segments.as_slice()) {
                let Some(ns) = net.segment(next) else {
                    continue;
                };
                let nd = d + ns.length;
                if nd > PARK_SEARCH_M || dist.get(&next).is_some_and(|&old| old <= nd) {
                    continue;
                }
                dist.insert(next, nd);
                open.insert((ring(nd), next));
            }
        }
    }
    None
}

/// The access point of a place as the path service wants it: where a car going there stops
/// and a pedestrian from the car starts.
pub fn place_access(
    index: &ParkingIndex,
    net: &NetGraph,
    buildings: &BuildingGrid,
    place: ParkPlace,
) -> Option<Access> {
    match place {
        ParkPlace::Kerb {
            segment,
            side,
            slot,
        } => Some(Access {
            segment,
            s: index.kerb_s(segment, usize::from(side), slot)?,
            side: if side == 1 { Side::Left } else { Side::Right },
        }),
        ParkPlace::Building(id) => crate::agents::access_of(net, buildings.get(id)?),
    }
}

/// Counts the places on the first run and every [`PARKING_PERIOD`] ticks (the Agents phase),
/// and counts the cars in them from the parked cars and the trips that reserved a place: what
/// a trip that vanished (a citizen gone, a lost car) held comes back here.
#[allow(clippy::too_many_arguments)]
pub fn update_parking_index(
    tick: Option<Res<Tick>>,
    content: Option<Res<SimContent>>,
    net: Res<NetGraph>,
    geo: Res<JunctionGeometry>,
    buildings: Res<BuildingGrid>,
    book: Res<TripBook>,
    grid: Res<CitizenGrid>,
    mut agents: ResMut<Agents>,
    mut index: ResMut<ParkingIndex>,
) {
    let (Some(tick), Some(content)) = (tick, content) else {
        return;
    };
    let due = index.built.is_none_or(|t| tick.0 >= t + PARKING_PERIOD);
    if !due {
        return;
    }
    index.rebuild(&net, &geo, &content.0, &buildings);
    agents.parked.retain_citizens(|c| grid.citizen(c).is_some());
    let reserved = book
        .iter()
        .filter(|(_, t)| !t.park.parked)
        .filter_map(|(_, t)| t.park.place);
    index.sync(agents.parked.iter().map(|(_, c)| c.place).chain(reserved));
    index.built = Some(tick.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_math::Vec3;
    use cp_building::{BuildingFlags, Footprint, Lot};
    use cp_content::ContentDb;
    use cp_core::curve::CubicBezier3;
    use cp_net::{Elevation, update_junctions};
    use std::sync::Arc;

    fn db() -> Arc<ContentDb> {
        Arc::new(
            ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
                .unwrap(),
        )
    }

    /// A street of `road` along z = 1000 from x = 1000, `len` metres.
    fn street(db: &Arc<ContentDb>, road: &str, len: f32) -> (World, SegmentId) {
        let mut g = NetGraph::new();
        let id = db.road_id(road).unwrap();
        let seg = g
            .add_segment(
                db.road(id),
                id,
                CubicBezier3::line(
                    Vec3::new(1000.0, 0.0, 1000.0),
                    Vec3::new(1000.0 + len, 0.0, 1000.0),
                ),
                Elevation::Ground,
            )
            .unwrap();
        let mut w = World::new();
        w.insert_resource(g);
        w.init_resource::<JunctionGeometry>();
        w.insert_resource(SimContent(db.clone()));
        w.run_system_cached(update_junctions).unwrap();
        (w, seg)
    }

    fn place(
        grid: &mut BuildingGrid,
        db: &ContentDb,
        def: &str,
        state: BuildingState,
    ) -> BuildingId {
        let def = db.building_id(def).unwrap_or_else(|| panic!("no {def}"));
        let d = db.building(def);
        grid.insert(Building {
            def,
            footprint: Footprint::Lot(Lot {
                block: cp_core::Id::new(0, 0),
                col: 0,
                w: d.lot.0,
                d: d.lot.1,
            }),
            level: d.level,
            state,
            ready_at: Tick(0),
            pos: Vec2::new(1040.0, 1020.0),
            rot: 0.0,
            access: None,
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        })
    }

    fn index(w: &World, db: &ContentDb, grid: &BuildingGrid) -> ParkingIndex {
        let mut i = ParkingIndex::default();
        i.rebuild(
            w.resource::<NetGraph>(),
            w.resource::<JunctionGeometry>(),
            db,
            grid,
        );
        i
    }

    #[test]
    fn a_local_street_has_kerb_places_on_both_sides_and_an_avenue_none() {
        let db = db();
        let (w, seg) = street(&db, "basic_2l", 90.0);
        let i = index(&w, &db, &BuildingGrid::default());
        // The bay runs between the margins (2 m at a joint, 8 m at a dead end), 6 m a place.
        let bay = bays(
            w.resource::<NetGraph>(),
            w.resource::<JunctionGeometry>(),
            &db,
            seg,
        )
        .expect("bays");
        let n = bay.slots().len() as u16;
        assert!(n >= 8, "{n} places on a 90 m street");
        assert_eq!((i.kerb(seg, 0).cap, i.kerb(seg, 1).cap), (n, n));
        assert_eq!(i.kerb_total().0, u32::from(n) * 2);
        // medium_4l and the highway take no kerb parking.
        for road in ["medium_4l", "large_6l", "highway_1w_2l"] {
            let (w, seg) = street(&db, road, 90.0);
            let i = index(&w, &db, &BuildingGrid::default());
            assert_eq!(i.kerb(seg, 0).cap + i.kerb(seg, 1).cap, 0, "{road}");
        }
    }

    #[test]
    fn a_reservation_takes_the_first_free_slot_and_a_rebuild_keeps_it() {
        let db = db();
        let (w, seg) = street(&db, "basic_2l", 90.0);
        let mut i = index(&w, &db, &BuildingGrid::default());
        let cap = i.kerb(seg, 1).cap;
        let first = i.take_kerb(seg, 1).expect("a place");
        assert_eq!(
            first,
            ParkPlace::Kerb {
                segment: seg,
                side: 1,
                slot: 0
            }
        );
        assert_eq!(i.kerb(seg, 1).free(), cap - 1);
        i.rebuild(
            w.resource::<NetGraph>(),
            w.resource::<JunctionGeometry>(),
            &db,
            &BuildingGrid::default(),
        );
        assert_eq!(i.kerb(seg, 1).used, 1);
        let second = i.take_kerb(seg, 1).expect("a place");
        assert_ne!(first, second);
        i.release(first);
        assert_eq!(i.kerb(seg, 1).used, 1);
        // The freed slot is the next one taken.
        assert_eq!(i.take_kerb(seg, 1), Some(first));
        // A full side refuses.
        while i.take_kerb(seg, 0).is_some() {}
        assert_eq!(i.kerb(seg, 0).free(), 0);
        assert!(i.take_kerb(seg, 0).is_none());
        // Every place has a distinct position along the street.
        let s: Vec<f32> = (0..cap).map(|k| i.kerb_s(seg, 1, k).unwrap()).collect();
        assert!(s.windows(2).all(|w| w[1] > w[0] + 5.0), "{s:?}");
        // `sync` counts from the cars alone.
        i.sync([first]);
        assert_eq!(i.kerb(seg, 1).used, 1);
        assert_eq!(i.kerb(seg, 0).used, 0);
    }

    #[test]
    fn buildings_bring_their_own_places() {
        let db = db();
        let (w, _) = street(&db, "basic_2l", 90.0);
        let mut grid = BuildingGrid::default();
        let lot = place(&mut grid, &db, "parking_lot", BuildingState::Active);
        let garage = place(&mut grid, &db, "parking_garage", BuildingState::Active);
        let site = place(
            &mut grid,
            &db,
            "parking_lot",
            BuildingState::UnderConstruction,
        );
        let home = place(&mut grid, &db, "house_low_1", BuildingState::Active);
        let flats = place(&mut grid, &db, "apt_high_1", BuildingState::Active);
        let i = index(&w, &db, &grid);
        assert_eq!(i.building(lot).cap, 60);
        assert_eq!(i.building(garage).cap, 200);
        assert_eq!(i.building(site).cap, 0, "a site has none");
        let bal = db.balance();
        let households = |id| f32::from(db.building(grid.get(id).unwrap().def).households);
        assert!(
            i.building(home).cap >= (households(home) * bal.parking_per_home_low).ceil() as u16
        );
        assert!(i.building(home).cap >= 1);
        // High density brings fewer places per household than low density.
        assert!(
            f32::from(i.building(flats).cap) / households(flats).max(1.0)
                < f32::from(i.building(home).cap) / households(home).max(1.0)
        );
        let mut i = i;
        assert_eq!(i.take_building(lot), Some(ParkPlace::Building(lot)));
        assert_eq!(i.building(lot).free(), 59);
    }
}
