//! Parking places (`architecture/system-design.md` §18.3, T-331a): the supply, counted, not
//! simulated. Two sources here: the **kerb** (the bays of `cp_net::kerb`, one place per
//! [`SLOT_LEN`] of a bay, minus those a service building's access or a yard gate overlaps) and
//! the **buildings** (a yard, a lot, a garage: `BuildingDef.parking`, or the rule of the balance
//! from households and jobs). [`ParkingIndex`] holds `cap` and `used` of each; the choice and the
//! reservation of a place are T-331b. Derived state: rebuilt from the net and the buildings, not
//! saved.
//!
//! The kerb places are the same ones the render draws parked cars in (`cp_render::parked` uses
//! [`kerb_fronts`] and [`slot_blocked`] too), so a place drawn is a place counted.

use std::collections::BTreeMap;

use bevy_ecs::prelude::*;
use bevy_math::Vec2;
use cp_building::{Building, BuildingGrid, BuildingId, BuildingState};
use cp_content::{ContentDb, SimContent, ZoneType};
use cp_core::Tick;
use cp_core::curve::to_xz;
use cp_net::kerb::{Bays, GATE_CLEAR, SLOT_LEN, bays};
use cp_net::{JunctionGeometry, NetGraph, SegmentId};

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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct ParkingIndex {
    kerb: Vec<[Place; 2]>,
    buildings: Vec<Place>,
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

    /// Takes a free place of a kerb side; `false` when none is free (T-331b reserves).
    pub fn reserve_kerb(&mut self, seg: SegmentId, side: usize) -> bool {
        match self.kerb.get_mut(seg.index as usize) {
            Some(s) if s[side.min(1)].free() > 0 => {
                s[side.min(1)].used += 1;
                true
            }
            _ => false,
        }
    }

    /// Takes a free place of a building.
    pub fn reserve_building(&mut self, id: BuildingId) -> bool {
        match self.buildings.get_mut(id.index as usize) {
            Some(p) if p.free() > 0 => {
                p.used += 1;
                true
            }
            _ => false,
        }
    }

    /// Gives a kerb place back.
    pub fn release_kerb(&mut self, seg: SegmentId, side: usize) {
        if let Some(s) = self.kerb.get_mut(seg.index as usize) {
            s[side.min(1)].used = s[side.min(1)].used.saturating_sub(1);
        }
    }

    /// Gives a building place back.
    pub fn release_building(&mut self, id: BuildingId) {
        if let Some(p) = self.buildings.get_mut(id.index as usize) {
            p.used = p.used.saturating_sub(1);
        }
    }

    /// Counts the places of the net and the buildings as they stand; `used` stays where the
    /// place is still there (clamped to the new capacity).
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
        for (id, _) in net.segments() {
            let Some(Bays { sides, a, b }) = bays(net, geo, db, id) else {
                continue;
            };
            let bay = Bays {
                sides: sides.clone(),
                a,
                b,
            };
            let slots = bay.slots();
            for side in &sides {
                let near: &[Front] = fronts
                    .get(&(id, side.sign as i8))
                    .map_or(&[], |v| v.as_slice());
                let cap = slots.iter().filter(|&&s| !slot_blocked(near, s)).count();
                let k = side_index(side.sign);
                let used = self.kerb(id, k).used;
                kerb[id.index as usize][k] = Place {
                    cap: cap as u16,
                    used: used.min(cap as u16),
                };
            }
        }
        let m = buildings
            .iter()
            .map(|(id, _)| id.index as usize + 1)
            .max()
            .unwrap_or(0);
        let mut places = vec![Place::default(); m];
        for (id, b) in buildings.iter() {
            let cap = building_places(db, b);
            places[id.index as usize] = Place {
                cap,
                used: self.building(id).used.min(cap),
            };
        }
        self.kerb = kerb;
        self.buildings = places;
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

/// Counts the places on the first run and every [`PARKING_PERIOD`] ticks (the Agents phase).
pub fn update_parking_index(
    tick: Option<Res<Tick>>,
    content: Option<Res<SimContent>>,
    net: Res<NetGraph>,
    geo: Res<JunctionGeometry>,
    buildings: Res<BuildingGrid>,
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
    fn a_reservation_takes_a_place_and_a_rebuild_keeps_it() {
        let db = db();
        let (w, seg) = street(&db, "basic_2l", 90.0);
        let mut i = index(&w, &db, &BuildingGrid::default());
        let cap = i.kerb(seg, 1).cap;
        assert!(i.reserve_kerb(seg, 1));
        assert_eq!(i.kerb(seg, 1).free(), cap - 1);
        i.rebuild(
            w.resource::<NetGraph>(),
            w.resource::<JunctionGeometry>(),
            &db,
            &BuildingGrid::default(),
        );
        assert_eq!(i.kerb(seg, 1).used, 1);
        i.release_kerb(seg, 1);
        assert_eq!(i.kerb(seg, 1).used, 0);
        // A full side refuses.
        for _ in 0..cap {
            assert!(i.reserve_kerb(seg, 0));
        }
        assert!(!i.reserve_kerb(seg, 0));
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
        assert!(i.reserve_building(lot));
        assert_eq!(i.building(lot).free(), 59);
    }
}
