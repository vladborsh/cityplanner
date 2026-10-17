//! Format 19 (T-106): a building stands on a `Footprint` (a lot, or free) and its access point
//! is optional. The v18 fixture (made by the real v18 writer before the change) and the older
//! M1 fixtures load, migrate with every id and every lot kept, round-trip byte for byte on the
//! new version, and run as before: the building flags, the residents and the households after
//! 256 ticks are the numbers recorded on the code before the type change.

use std::path::Path;
use std::sync::Arc;

use cp_building::{BuildingGrid, Footprint};
use cp_citizen::CitizenGrid;
use cp_content::ContentDb;
use cp_core::SimLimits;
use cp_save::migrate::{BuildingV18, SaveDataV18, encode_v18};
use cp_save::*;
use cp_sim::SimRunner;
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(Path::new(ROOT).join("../../fixtures/saves").join(name)).unwrap()
}

fn env() -> (Arc<Terrain>, Arc<ContentDb>) {
    (
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap()),
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap()),
    )
}

fn loaded(bytes: &[u8]) -> SimRunner {
    let (terrain, content) = env();
    let hash = content.content_hash();
    let mut sim = SimRunner::with_limits(1, SimLimits::desktop());
    sim.set_terrain(terrain);
    sim.set_content(content);
    let (_, warnings) = load_into_runner(&mut sim, bytes, hash).unwrap();
    assert!(
        // The v18 fixture carries a made-up map hash, as the other generated ones do.
        warnings.iter().all(|w| matches!(
            w,
            LoadWarning::ContentHashMismatch { .. } | LoadWarning::MapMismatch { .. }
        )),
        "{warnings:?}"
    );
    sim
}

/// `(buildings, flag counts by bit, residents, households)` after 256 ticks.
fn summary(sim: &mut SimRunner) -> (usize, [u32; 8], usize, usize) {
    for _ in 0..256 {
        sim.tick_once();
    }
    let g = sim.world().resource::<BuildingGrid>();
    let mut flags = [0u32; 8];
    for (_, b) in g.iter() {
        for (i, f) in flags.iter_mut().enumerate() {
            *f += u32::from(b.flags.0 & (1 << i) != 0);
        }
    }
    let c = sim.world().resource::<CitizenGrid>();
    (g.len(), flags, c.citizen_count(), c.household_count())
}

#[test]
fn the_v18_fixture_and_the_m1_fixtures_run_as_before_the_footprint_change() {
    // BUG-042: one building of m1-small gained a source in reach (140 -> 139). Recorded with the code of the commit before the change (v18 reader). BUG-036: v18-basic
    // lost power in 35 buildings then; power now passes edge to edge, so none lacks it.
    for (name, expect) in [
        ("v18-basic.sav", (48, [46, 0, 0, 0, 0, 46, 0, 0], 233, 81)),
        (
            "m1-small.sav",
            (200, [192, 139, 0, 0, 0, 192, 0, 0], 1899, 630),
        ),
        (
            "m1-traffic.sav",
            (188, [185, 133, 0, 0, 0, 185, 0, 0], 1876, 634),
        ),
    ] {
        let mut sim = loaded(&fixture(name));
        assert_eq!(summary(&mut sim), expect, "{name}");
    }
}

#[test]
fn the_v18_fixture_migrates_every_building_onto_its_lot_with_its_access_point() {
    let bytes = fixture("v18-basic.sav");
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 18, "made by the v18 writer");
    let (_, content) = env();
    let old: SaveDataV18 = {
        // The frozen v18 body, decoded the way the loader does.
        let l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
        // Re-encode the migrated data as v18 to compare building by building.
        SaveDataV18 {
            tick: l.data.tick,
            rng: l.data.rng.clone(),
            speed: l.data.speed,
            queue: l.data.queue.clone(),
            log: l.data.log.clone(),
            tiles: l.data.tiles,
            net: l.data.net.clone(),
            zones: l.data.zones.clone().to_v34(),
            buildings: cp_save::migrate::BuildingGridV18::from(&l.data.buildings),
            citizens: (&l.data.citizens).into(),
            paths: l.data.paths.clone(),
            utilities: l.data.utilities.clone(),
            trips: l.data.trips.clone(),
            agents: l.data.agents.clone(),
            treasury: l.data.treasury,
            economy: (&l.data.economy).into(),
            demand: l.data.demand,
            demand_acc: l.data.demand_acc,
            land: l.data.land,
            decay: l.data.decay,
            def_names: l.data.def_names.clone(),
        }
    };
    // The migrated grid has lots and access points everywhere, none free.
    let l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    let mut n = 0;
    for (_, b) in l.data.buildings.iter() {
        assert!(matches!(b.footprint, Footprint::Lot(_)));
        assert!(b.access().is_some());
        n += 1;
    }
    assert_eq!(n, old.buildings.buildings.len());
    // And the v18 encoding of the same data reads back to the same buildings.
    let again = encode_v18(&old, h.content_hash, &h.map_id, h.map_hash).unwrap();
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    let a: Vec<BuildingV18> = l
        .data
        .buildings
        .iter()
        .map(|(_, b)| BuildingV18::from(b))
        .collect();
    let b: Vec<BuildingV18> = l2
        .data
        .buildings
        .iter()
        .map(|(_, b)| BuildingV18::from(b))
        .collect();
    assert_eq!(a, b);
}

#[test]
fn the_fixtures_round_trip_byte_for_byte_on_the_new_version() {
    let (_, content) = env();
    for name in ["v18-basic.sav", "m1-small.sav", "m1-traffic.sav"] {
        let bytes = fixture(name);
        let (h, _) = read_header(&bytes).unwrap();
        let l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
        let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
        assert_eq!(
            read_header(&again).unwrap().0.format_version,
            FORMAT_VERSION
        );
        let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
        let third = save_with_map(&l2.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
        assert_eq!(again, third, "{name}: save(load(save(x))) == save(x)");
    }
}
