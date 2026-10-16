//! Format 18: the save names its definitions, and loading re-assigns every road and building id
//! by name in the content it is loaded against (T-077). A content file that sorts before the
//! old ones no longer retypes the roads of an old city; a name the content lacks becomes a
//! placeholder and a warning.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cp_content::{ContentDb, DefKind, PLACEHOLDER_INDEX};
use cp_net::NetGraph;
use cp_save::migrate::{SaveDataV17, encode_v17};
use cp_save::*;
use cp_sim::SimRunner;
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(Path::new(ROOT).join("../../fixtures/saves").join(name)).unwrap()
}

fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, String)>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let path: PathBuf = e.path();
        let rel = format!("{rel}{name}");
        if path.is_dir() {
            walk(&path, &format!("{rel}/"), out);
        } else if name.ends_with(".ron") {
            out.push((
                format!("content/{rel}"),
                std::fs::read_to_string(&path).unwrap(),
            ));
        }
    }
}

/// The shipped content, minus the files named in `skip`, plus `extra` `(path, text)` files.
fn content(extra: &[(&str, &str)], skip: &[&str]) -> Arc<ContentDb> {
    let mut sources = Vec::new();
    walk(
        &Path::new(ROOT).join("../../assets/content"),
        "",
        &mut sources,
    );
    sources.retain(|(p, _)| !skip.iter().any(|s| p.ends_with(s)));
    for (p, t) in extra {
        sources.push(((*p).to_string(), (*t).to_string()));
    }
    Arc::new(ContentDb::from_sources(sources).unwrap_or_else(|e| panic!("{e}")))
}

/// A road type that sorts before every shipped one.
const AAA: (&str, &str) = (
    "content/roads/aaa_test_2l.ron",
    r#"Road((id: "aaa_test_2l", name: "Test street", width: 12.0,
        lanes: [
            (kind: Pedestrian, dir: Forward, width: 2.5, offset: 4.75),
            (kind: Car, dir: Backward, width: 3.5, offset: 1.75),
            (kind: Car, dir: Forward, width: 3.5, offset: -1.75),
            (kind: Pedestrian, dir: Forward, width: 2.5, offset: -4.75),
        ],
        median_width: 0.0, speed_kmh: 40, max_slope: 0.12, cost_per_m: 10.0,
        upkeep_per_m_week: 0.1, allows_zoning: true, rank: 1, elevated: None, bridge: None,
        mesh: "road/basic_2l", material: "road/asphalt"))"#,
);

fn runner(content: &Arc<ContentDb>) -> SimRunner {
    let mut sim = SimRunner::new(1);
    sim.set_terrain(Arc::new(
        Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap(),
    ));
    sim.set_content(Arc::clone(content));
    sim
}

/// The road and building type names of every segment and building, by id.
fn types(sim: &SimRunner) -> (Vec<String>, Vec<String>) {
    let db = Arc::clone(&sim.world().resource::<cp_content::SimContent>().0);
    let roads = sim
        .world()
        .resource::<NetGraph>()
        .segments()
        .map(|(_, s)| db.road(s.road).id.clone())
        .collect();
    let buildings = sim
        .world()
        .resource::<cp_building::BuildingGrid>()
        .iter()
        .map(|(_, b)| db.building(b.def).id.clone())
        .collect();
    (roads, buildings)
}

#[test]
fn v17_fixture_migrates_without_names_and_round_trips() {
    let bytes = fixture("v17-basic.sav");
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 17);
    let l = load(&bytes, h.content_hash).unwrap();
    assert!(l.warnings.is_empty());
    assert_eq!(
        l.data.def_names,
        cp_save::def_tables::def_names_before_v18(17),
        "an old save gets the names of the content of its time"
    );
    assert!(l.data.agents.vehicle_count() > 0 && !l.data.buildings.is_empty());

    // The frozen v17 writer reproduces the committed file byte for byte.
    let v17 = SaveDataV17 {
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
        land: l.data.land.clone(),
        decay: l.data.decay.clone(),
    };
    let again = encode_v17(&v17, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(again, bytes, "frozen v17 writer matches the fixture");

    let v18 = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(read_header(&v18).unwrap().0.format_version, FORMAT_VERSION);
    let l2 = load(&v18, h.content_hash).unwrap();
    assert_eq!(l2.data, l.data);
    assert_eq!(
        save_with_map(&l2.data, h.content_hash, &h.map_id, h.map_hash).unwrap(),
        v18
    );
}

/// A building type that sorts before every shipped one.
const AAA_HOUSE: (&str, &str) = (
    "content/buildings/aaa_test_house.ron",
    r#"Building((id: "aaa_test_house", name: "Test house", zone: ResLow, level: 1, lot: (2, 2),
        households: 1, consumption: {Water: 1.0, Sewage: 1.0, Power: 1.5}, tax_base: 10.0,
        styles: ["res_low_cottage_gabled_a"]))"#,
);

/// Every committed fixture, from v0 to the m1 and bench saves.
fn all_fixtures() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(Path::new(ROOT).join("../../fixtures/saves"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".sav"))
        .collect();
    v.sort();
    v
}

#[test]
fn every_fixture_keeps_its_road_and_building_types_when_new_types_sort_first() {
    let base = content(&[], &[]);
    let extra = content(&[AAA, AAA_HOUSE], &[]);
    assert_eq!(extra.road_id("aaa_test_2l"), Some(cp_content::RoadDefId(0)));
    let fixtures = all_fixtures();
    assert!(fixtures.len() >= 21, "{fixtures:?}");
    let (mut with_roads, mut with_buildings) = (0, 0);
    for name in fixtures {
        let bytes = fixture(&name);
        let mut a = runner(&base);
        let mut b = runner(&extra);
        let (_, wa) = load_into_runner(&mut a, &bytes, base.content_hash())
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let (_, wb) = load_into_runner(&mut b, &bytes, extra.content_hash()).unwrap();
        for w in wa.iter().chain(&wb) {
            assert!(
                !matches!(w, LoadWarning::UnknownDef { .. }),
                "{name}: {w:?}"
            );
        }
        // Against the names the file itself implies: the raw index through the frozen table of
        // its time, with the real highway defs now in the shipped content.
        let raw = load(&bytes, base.content_hash()).unwrap().data;
        let want_roads: Vec<String> = raw
            .net
            .segments()
            .map(|(_, s)| raw.def_names.roads[s.road.0 as usize].clone())
            .collect();
        let want_buildings: Vec<String> = raw
            .buildings
            .iter()
            .map(|(_, b)| raw.def_names.buildings[b.def.index()].clone())
            .collect();
        let (roads_a, buildings_a) = types(&a);
        assert_eq!(roads_a, want_roads, "{name}: road names of the file");
        assert_eq!(
            buildings_a, want_buildings,
            "{name}: building names of the file"
        );
        let (roads_b, buildings_b) = types(&b);
        assert_eq!(roads_a, roads_b, "{name}: road types");
        assert_eq!(buildings_a, buildings_b, "{name}: building types");
        assert!(!roads_b.iter().any(|r| r == "aaa_test_2l"), "{name}");
        assert!(!buildings_b.iter().any(|r| r == "aaa_test_house"), "{name}");
        with_roads += usize::from(!roads_a.is_empty());
        with_buildings += usize::from(!buildings_a.is_empty());
    }
    assert!(
        with_roads >= 15 && with_buildings >= 10,
        "{with_roads} {with_buildings}"
    );
}

#[test]
fn a_save_from_a_runner_carries_the_names_of_its_content() {
    let base = content(&[], &[]);
    let mut a = runner(&base);
    let hash = base.content_hash();
    let (_, w) = load_into_runner(&mut a, &fixture("v17-basic.sav"), hash).unwrap();
    assert!(
        w.iter()
            .all(|x| !matches!(x, LoadWarning::UnknownDef { .. })),
        "{w:?}"
    );
    let bytes = save_runner(&a, hash, "flatlands").unwrap();
    let l = load(&bytes, hash).unwrap();
    assert_eq!(l.data.def_names, DefNames::from_content(&base));
    assert!(l.data.def_names.roads.contains(&"basic_2l".to_string()));
    // Round trip with names: load -> save is byte for byte.
    assert_eq!(
        save_with_map(&l.data, hash, &l.header.map_id, l.header.map_hash).unwrap(),
        bytes
    );
}

#[test]
fn a_road_type_that_sorts_first_does_not_retype_the_old_city() {
    let base = content(&[], &[]);
    let hash = base.content_hash();
    let mut a = runner(&base);
    load_into_runner(&mut a, &fixture("v17-basic.sav"), hash).unwrap();
    let saved = save_runner(&a, hash, "flatlands").unwrap();
    let (roads_before, buildings_before) = types(&a);
    assert!(!roads_before.is_empty() && !buildings_before.is_empty());

    let with_extra = content(&[AAA], &[]);
    assert_eq!(
        with_extra.road_id("aaa_test_2l"),
        Some(cp_content::RoadDefId(0)),
        "the new type sorts first and shifts every other index"
    );
    let mut b = runner(&with_extra);
    let (_, warnings) = load_into_runner(&mut b, &saved, with_extra.content_hash()).unwrap();
    // Only the content hash differs; no definition is missing.
    assert!(
        warnings
            .iter()
            .all(|w| matches!(w, LoadWarning::ContentHashMismatch { .. })),
        "{warnings:?}"
    );
    let (roads_after, buildings_after) = types(&b);
    assert_eq!(
        roads_after, roads_before,
        "every segment keeps its type by name"
    );
    assert_eq!(buildings_after, buildings_before);
    // The raw indices did move (that is what the remap compensated).
    let raw = |s: &SimRunner| {
        s.world()
            .resource::<NetGraph>()
            .segments()
            .map(|(_, s)| s.road.0)
            .collect::<Vec<_>>()
    };
    assert!(raw(&a).iter().zip(raw(&b)).all(|(x, y)| y == x + 1));
    // And the city runs on.
    for _ in 0..200 {
        b.tick_once();
    }
}

#[test]
fn an_unknown_name_gets_the_placeholder_and_a_warning_and_the_load_goes_on() {
    let base = content(&[], &[]);
    let hash = base.content_hash();
    let mut a = runner(&base);
    load_into_runner(&mut a, &fixture("v17-basic.sav"), hash).unwrap();
    let saved = save_runner(&a, hash, "flatlands").unwrap();

    // Content without the street the city is made of.
    let without = content(&[], &["roads/basic_2l.ron"]);
    assert!(without.road_id("basic_2l").is_none());
    let mut b = runner(&without);
    let (_, warnings) = load_into_runner(&mut b, &saved, without.content_hash()).unwrap();
    assert!(
        warnings.contains(&LoadWarning::UnknownDef {
            kind: DefKind::Road,
            name: "basic_2l".to_string()
        }),
        "{warnings:?}"
    );
    let net = b.world().resource::<NetGraph>();
    assert!(net.segments().count() > 0);
    for (_, s) in net.segments() {
        assert_eq!(s.road.0, PLACEHOLDER_INDEX);
        assert_eq!(without.road(s.road).id, "placeholder_road");
    }
    for _ in 0..200 {
        b.tick_once();
    }

    // An unknown building type, too: the first building's own type goes missing.
    let missing = types(&a).1[0].clone();
    let file = format!("buildings/{missing}.ron");
    let no_building = content(&[], &[file.as_str()]);
    assert!(no_building.building_id(&missing).is_none());
    let mut c = runner(&no_building);
    let (_, w) = load_into_runner(&mut c, &saved, no_building.content_hash()).unwrap();
    assert!(
        w.contains(&LoadWarning::UnknownDef {
            kind: DefKind::Building,
            name: missing.clone()
        }),
        "{w:?}"
    );
    let db = Arc::clone(&c.world().resource::<cp_content::SimContent>().0);
    let n_placeholder = c
        .world()
        .resource::<cp_building::BuildingGrid>()
        .iter()
        .filter(|(_, b)| {
            b.def.0 == PLACEHOLDER_INDEX && db.building(b.def).id == "placeholder_building"
        })
        .count();
    assert!(n_placeholder > 0);
    for _ in 0..100 {
        c.tick_once();
    }
}
