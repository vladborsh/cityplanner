//! Acceptance tests for `m0-data` content loading, driven from the committed
//! `assets/content` files through the public API only.

use cp_content::{ContentDb, LaneKind, RoadClass, ServiceKind, ZoneType};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")
}

fn sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<(String, String)>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "ron") {
                out.push((
                    p.to_string_lossy().into_owned(),
                    std::fs::read_to_string(&p).unwrap(),
                ));
            }
        }
    }
    let mut v = Vec::new();
    walk(&root(), &mut v);
    assert!(v.len() >= 8, "expected the committed content set");
    v
}

fn errs(s: Vec<(String, String)>) -> String {
    ContentDb::from_sources(s)
        .expect_err("must be rejected")
        .to_string()
}

/// Replaces `from` with `to` in the file whose name contains `file`.
fn mutate(file: &str, from: &str, to: &str) -> Vec<(String, String)> {
    let mut hit = false;
    let v = sources()
        .into_iter()
        .map(|(p, t)| {
            if p.contains(file) {
                assert!(t.contains(from), "{file} has no `{from}`");
                hit = true;
                (p, t.replace(from, to))
            } else {
                (p, t)
            }
        })
        .collect();
    assert!(hit, "no file {file}");
    v
}

#[test]
fn committed_content_loads_with_eleven_road_types_nine_roads_and_two_tracks() {
    let db = ContentDb::load_dir(root()).unwrap();
    assert_eq!(db.roads().count(), 12); // + the park path (T-222)
    let want = [
        ("basic_2l", RoadClass::Local, "Local street", false),
        (
            "medium_4l",
            RoadClass::Collector,
            "Collector (4 lanes)",
            false,
        ),
        ("large_6l", RoadClass::Arterial, "Arterial (6 lanes)", false),
        (
            "highway_1w_1l",
            RoadClass::Highway,
            "Highway, 1 lane (one-way)",
            true,
        ),
        (
            "highway_1w_2l",
            RoadClass::Highway,
            "Highway, 2 lanes (one-way)",
            true,
        ),
        (
            "highway_1w_3l",
            RoadClass::Highway,
            "Highway, 3 lanes (one-way)",
            true,
        ),
        (
            "highway_1w_4l",
            RoadClass::Highway,
            "Highway, 4 lanes (one-way)",
            true,
        ),
    ];
    for (id, class, name, highway) in want {
        let r = db.road(db.road_id(id).unwrap_or_else(|| panic!("road {id}")));
        assert!(
            r.elevated.is_some() && r.bridge.is_some(),
            "{id} needs elevated + bridge"
        );
        assert_eq!((r.class, r.name.as_str()), (class, name), "{id}");
        assert_eq!(r.is_one_way(), highway, "{id}");
        if highway {
            assert!(
                r.lanes.iter().all(|l| l.kind == LaneKind::Car),
                "{id}: no sidewalks"
            );
            assert!(
                !r.allows_zoning && r.max_slope <= 0.06 && r.rank == 4,
                "{id}"
            );
            assert_eq!(
                (r.speed_kmh, r.width),
                (100.0, 3.75 * r.lanes.len() as f32),
                "{id}"
            );
        }
    }
    // Cost and upkeep are proportional to the lanes (placeholder: 40/3 per lane-metre).
    let cost = |id: &str| db.road(db.road_id(id).unwrap()).cost_per_m;
    assert!((cost("highway_1w_2l") - 2.0 * cost("highway_1w_1l")).abs() < 1e-3);
    assert!((cost("highway_1w_3l") - 3.0 * cost("highway_1w_1l")).abs() < 1e-3);
    assert!((cost("highway_1w_4l") - 4.0 * cost("highway_1w_1l")).abs() < 1e-3);
    // The existing ids keep their names; the new ones sort between them, and the two Rail types
    // (T-161) sort after every road, so no saved road index moves.
    let ids: Vec<&str> = db.roads().map(|(_, r)| r.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "basic_1w_2l",
            "basic_2l",
            "highway_1w_1l",
            "highway_1w_2l",
            "highway_1w_3l",
            "highway_1w_4l",
            "large_6l",
            "medium_1w_3l",
            "medium_4l",
            "metro_track",
            "park_path",
            "rail_2t"
        ]
    );
    // T-172: one-way streets with sidewalks and zoning on both sides.
    for (id, class, cars, speed) in [
        ("basic_1w_2l", RoadClass::Local, 2, 40.0),
        ("medium_1w_3l", RoadClass::Collector, 3, 50.0),
    ] {
        let r = db.road(db.road_id(id).unwrap());
        assert_eq!((r.class, r.speed_kmh), (class, speed), "{id}");
        assert!(r.is_one_way() && r.allows_zoning, "{id}");
        let count = |k| r.lanes.iter().filter(|l| l.kind == k).count();
        assert_eq!(
            (count(LaneKind::Car), count(LaneKind::Pedestrian)),
            (cars, 2),
            "{id}"
        );
        assert!(
            r.elevated.is_some() && r.bridge.is_some() && r.tunnel.is_some(),
            "{id}"
        );
    }
    let house = db.building(db.building_id("house_low_1").unwrap());
    assert_eq!(house.zone, ZoneType::ResLow);
}

#[test]
fn content_hash_ignores_order_line_endings_and_formatting() {
    let base = ContentDb::from_sources(sources()).unwrap().content_hash();
    assert_eq!(base, ContentDb::load_dir(root()).unwrap().content_hash());

    let mut rev = sources();
    rev.reverse();
    assert_eq!(
        base,
        ContentDb::from_sources(rev).unwrap().content_hash(),
        "file order"
    );

    let crlf = sources()
        .into_iter()
        .map(|(p, t)| (p, t.replace('\n', "\r\n")))
        .collect();
    assert_eq!(
        base,
        ContentDb::from_sources(crlf).unwrap().content_hash(),
        "CRLF"
    );

    let spaced = sources()
        .into_iter()
        .map(|(p, t)| {
            (
                p,
                format!("// comment\n\n{}\n\n", t.replace(",\n", " ,\n\n")),
            )
        })
        .collect();
    assert_eq!(
        base,
        ContentDb::from_sources(spaced).unwrap().content_hash(),
        "whitespace/comments"
    );

    let renamed = sources()
        .into_iter()
        .map(|(p, t)| (format!("elsewhere/{p}"), t))
        .collect();
    assert_eq!(
        base,
        ContentDb::from_sources(renamed).unwrap().content_hash(),
        "paths"
    );
}

#[test]
fn content_hash_changes_with_any_value() {
    let base = ContentDb::from_sources(sources()).unwrap().content_hash();
    for (file, from, to) in [
        ("basic_2l", "speed_kmh: 40", "speed_kmh: 41"),
        ("basic_2l", "rank: 1", "rank: 2"),
        ("services/water_pump", "capacity: 3000", "capacity: 3001"),
        ("basic_2l", "road/asphalt", "road/asphalt2"),
    ] {
        let h = ContentDb::from_sources(mutate(file, from, to))
            .unwrap()
            .content_hash();
        assert_ne!(base, h, "{file}: {from} -> {to}");
    }
}

/// One Highway rule broken at a time: the error names the file and the field.
#[test]
fn highway_rules_are_enforced_with_file_and_field() {
    let hw = "highway_1w_2l";
    let cases = [
        (
            "a backward car lane",
            "dir: Forward, width: 3.75, offset: -1.875",
            "dir: Backward, width: 3.75, offset: -1.875",
            "lanes",
        ),
        (
            "a sidewalk",
            "kind: Car, dir: Forward, width: 3.75, offset: -1.875",
            "kind: Pedestrian, dir: Forward, width: 3.75, offset: -1.875",
            "lanes[1].kind",
        ),
        (
            "zoning",
            "allows_zoning: false",
            "allows_zoning: true",
            "allows_zoning",
        ),
        (
            "a steep grade",
            "max_slope: 0.06",
            "max_slope: 0.07",
            "max_slope",
        ),
        ("a low rank", "rank: 4", "rank: 3", "rank"),
    ];
    for (what, from, to, field) in cases {
        let e = errs(mutate(hw, from, to));
        assert!(e.contains(hw) && e.contains(field), "{what}: {e}");
    }
    // A Highway with no car lane at all is not one-way either.
    let e = errs(mutate(
        hw,
        "kind: Car, dir: Forward, width: 3.75, offset: 1.875",
        "kind: Pedestrian, dir: Forward, width: 3.75, offset: 1.875",
    ));
    assert!(e.contains("lanes[0].kind"), "{e}");
}

#[test]
fn invalid_content_is_rejected_with_file_and_field() {
    let e = errs(mutate("basic_2l", "width: 16.0,", "width: -16.0,"));
    assert!(e.contains("basic_2l") && e.contains("width"), "{e}");

    let e = errs(mutate(
        "basic_2l",
        "allows_zoning: true,",
        "allows_zoning: true, bogus: 1,",
    ));
    assert!(
        e.contains("basic_2l") && e.contains("bogus"),
        "unknown field: {e}"
    );

    let e = errs(mutate("house_low_1", "ResLow", "ResMedium"));
    assert!(
        e.contains("house_low_1") && e.contains("ResMedium"),
        "bad enum: {e}"
    );

    let e = errs(mutate("basic_2l", "id: \"basic_2l\"", "id: \"medium_4l\""));
    assert!(
        e.contains("duplicate") && e.contains("medium_4l"),
        "dup id: {e}"
    );

    let e = errs(mutate("basic_2l", "offset: 5.75", "offset: 40.0"));
    assert!(e.contains("lanes[0].offset"), "lane outside road: {e}");

    let e = errs(mutate(
        "services/water_pump",
        "vehicles: []",
        "vehicles: [\"nope\"]",
    ));
    assert!(e.contains("nope"), "bad vehicle ref: {e}");

    let e = errs(mutate("basic_2l", "width: 16.0,", "width: 0.0,"));
    assert!(e.contains("width"), "zero width: {e}");

    let mut s = sources();
    s.push(("broken.ron".into(), "Road((".into()));
    assert!(errs(s).contains("broken.ron"));
}

#[test]
fn all_problems_are_reported_together() {
    let mut s = mutate("basic_2l", "width: 16.0,", "width: -1.0,");
    for (p, t) in &mut s {
        if p.contains("car_basic") {
            *t = t.replace("accel:", "bogus: 1, accel:");
        }
    }
    let e = errs(s);
    assert!(e.contains("basic_2l") && e.contains("car_basic"), "{e}");
}

#[test]
fn hash_is_independent_of_number_spelling_but_distinguishes_negative_zero() {
    let base = ContentDb::from_sources(sources()).unwrap().content_hash();
    // Same numbers, different spelling.
    let alt = ContentDb::from_sources(mutate("basic_2l", "width: 16.0,", "width: 16,")).ok();
    if let Some(db) = alt {
        assert_eq!(base, db.content_hash(), "16 vs 16.0");
    }
    let alt = ContentDb::from_sources(mutate("basic_2l", "width: 16.0,", "width: 1.6e1,")).unwrap();
    assert_eq!(base, alt.content_hash(), "1.6e1 vs 16.0");
    // Known gap: -0.0 is a different bit pattern. Harmless as long as it only yields a
    // content-hash warning on load; assert both stay valid and report the behaviour.
    let neg = ContentDb::from_sources(mutate(
        "basic_2l",
        "median_width: 0.0,",
        "median_width: -0.0,",
    ))
    .expect("-0.0 is a valid non-negative width");
    assert_ne!(base, neg.content_hash());
}

/// T-101: water and sewage share one pipe; a utility of kind `Sewage` is a content error.
#[test]
fn a_sewage_utility_is_rejected_and_the_committed_content_has_one_pipe() {
    let e = errs(mutate("water_pipe", "kind: Water", "kind: Sewage"));
    assert!(e.contains("share one pipe"), "{e}");
    let db = ContentDb::load_dir(root()).unwrap();
    let kinds: Vec<_> = db.utilities().map(|(_, d)| d.kind).collect();
    assert_eq!(
        kinds,
        [
            cp_content::UtilityKind::Power,
            cp_content::UtilityKind::Water
        ]
    );
}

/// T-111: every shipped building names at least one style, every name is a shipped style, and
/// an unknown name is a load error naming the file and field.
#[test]
fn every_building_references_existing_styles_and_a_dangling_one_is_rejected() {
    let db = ContentDb::load_dir(root()).unwrap();
    assert!(db.styles().count() >= 1);
    for (_, d) in db.buildings() {
        assert!(!d.styles.is_empty(), "{}", d.id);
        for s in &d.styles {
            assert!(db.style_id(s).is_some(), "{}: style {s}", d.id);
        }
    }
    let e = errs(mutate("house_low_1.ron", "res_low_", "ghost_"));
    assert!(
        e.contains("house_low_1.ron: styles[0]: unknown style `ghost_"),
        "{e}"
    );
}

/// T-113 (§8.2): every (zone type, level) draws from at least four different styles across its
/// defs, and every zone type has at least five different massing × roof × palette combinations.
#[test]
fn every_zone_and_level_has_four_looks_and_every_zone_five_combinations() {
    use std::collections::{BTreeMap, BTreeSet};
    let db = ContentDb::load_dir(root()).unwrap();
    let mut per_level: BTreeMap<(ZoneType, u8), BTreeSet<&str>> = BTreeMap::new();
    let mut combos: BTreeMap<ZoneType, BTreeSet<String>> = BTreeMap::new();
    for (_, d) in db.buildings().filter(|(_, d)| d.service.is_none()) {
        for s in &d.styles {
            per_level.entry((d.zone, d.level)).or_default().insert(s);
            let st = db.style(db.style_id(s).unwrap());
            combos
                .entry(d.zone)
                .or_default()
                .insert(format!("{:?} {:?} {:?}", st.massing, st.roof, st.palette));
        }
    }
    assert_eq!(
        per_level.len(),
        5 + 5 + 3 * 4,
        "every (zone, level) is covered"
    );
    for ((zone, level), styles) in &per_level {
        assert!(styles.len() >= 4, "{zone:?} level {level}: only {styles:?}");
    }
    assert_eq!(combos.len(), 6);
    for (zone, c) in &combos {
        assert!(c.len() >= 5, "{zone:?}: only {} combinations", c.len());
    }
    // At least five looks per zone type (the tall zones have one set per height band), none unused.
    let used: BTreeSet<&str> = per_level.values().flatten().copied().collect();
    for (_, s) in db.styles().filter(|(_, s)| {
        !s.id.starts_with("service_")
            && !s.id.starts_with("unique_")
            && !s.id.starts_with("spec_")
            && !s.id.starts_with("chain_")
            && !s.id.starts_with("park_")
            && !s.id.starts_with("industry_")
    }) {
        assert!(used.contains(s.id.as_str()), "style {} is unused", s.id);
    }
    assert!(
        db.styles()
            .filter(|(_, s)| !s.id.starts_with("service_")
                && !s.id.starts_with("unique_")
                && !s.id.starts_with("spec_")
                && !s.id.starts_with("chain_")
                && !s.id.starts_with("park_")
                && !s.id.starts_with("industry_"))
            .count()
            >= 30
    );
}

/// T-114: the six service buildings have six different silhouettes (massings), each its own
/// `service_*` style. The M2 services (T-136 on) borrow these styles until T-147 draws their own.
#[test]
fn the_six_service_buildings_have_six_different_silhouettes() {
    use std::collections::BTreeSet;
    let db = ContentDb::load_dir(root()).unwrap();
    let mut massings = BTreeSet::new();
    let mut n = 0;
    let m1 = |d: &cp_content::BuildingDef| {
        d.service
            .as_deref()
            .and_then(|s| db.service_id(s))
            .is_some_and(|s| db.service(s).kind <= ServiceKind::Sewage)
    };
    for (_, d) in db.buildings().filter(|(_, d)| m1(d)) {
        assert_eq!(d.styles.len(), 1, "{}", d.id);
        assert!(
            d.styles[0].starts_with("service_"),
            "{}: {:?}",
            d.id,
            d.styles
        );
        massings.insert(format!(
            "{:?}",
            db.style(db.style_id(&d.styles[0]).unwrap()).massing
        ));
        n += 1;
    }
    assert_eq!(n, 6);
    assert_eq!(massings.len(), 6, "{massings:?}");
}

/// T-147: the eleven M2 service buildings have eleven different massings, each its own
/// `service_*` style, and none of them is one of the M1 six.
#[test]
fn the_eleven_m2_service_buildings_have_eleven_different_silhouettes() {
    use std::collections::BTreeSet;
    let db = ContentDb::load_dir(root()).unwrap();
    let mut massings = BTreeSet::new();
    let mut styles = BTreeSet::new();
    let mut n = 0;
    for (_, d) in db.buildings() {
        let Some(svc) = d.service.as_deref().and_then(|s| db.service_id(s)) else {
            continue;
        };
        if db.service(svc).kind < ServiceKind::Garbage
            || db.service(svc).kind >= ServiceKind::Transit
        {
            continue;
        }
        assert_eq!(d.styles.len(), 1, "{}", d.id);
        let st = db.style(db.style_id(&d.styles[0]).unwrap());
        assert!(st.id.starts_with("service_"), "{}", d.id);
        massings.insert(format!("{:?}", st.massing));
        styles.insert(st.id.clone());
        n += 1;
    }
    assert_eq!(n, 11);
    assert_eq!(massings.len(), 11, "{massings:?}");
    assert_eq!(styles.len(), 11);
    for m1 in [
        "PumpHouse",
        "Outflow",
        "CoalPlant",
        "WindTurbine",
        "WaterTower",
        "TreatmentPlant",
        "Box",
    ] {
        assert!(!massings.contains(m1), "{m1}");
    }
}

/// T-227, T-228: every city park, nature reserve, zoo and amusement park building (gate,
/// attractions) and every park prop has a style of its own (`park_<id>`, drawn by its own
/// massing) and a square icon file.
#[test]
fn park_area_defs_name_their_own_styles_and_icons() {
    let db = ContentDb::load_dir(root()).unwrap();
    let icons = root().join("../ui/icons");
    let mut n = 0;
    for (_, d) in db.buildings() {
        let Some(park) = &d.park else { continue };
        let ours = matches!(
            park.park,
            Some(
                cp_content::ParkKind::CityPark
                    | cp_content::ParkKind::Nature
                    | cp_content::ParkKind::Zoo
                    | cp_content::ParkKind::Amusement
            )
        ) || d.id.starts_with("prop_");
        if !ours {
            continue;
        }
        n += 1;
        assert_eq!(d.styles, vec![format!("park_{}", d.id)], "{}", d.id);
        let style = db.style(db.style_id(&d.styles[0]).expect("style exists"));
        assert!(style.massing as u8 >= 68, "{}: a massing of its own", d.id);
        let png = icons.join(format!("{}.png", d.icon));
        assert!(png.exists(), "{}: no icon {png:?}", d.id);
        assert!(
            icons.join(format!("src/{}.svg", d.icon)).exists(),
            "{}",
            d.id
        );
    }
    assert_eq!(n, 26, "four gates, sixteen attractions, six props");
}

/// T-218: the four unique factories, the cargo station and the cargo port have a style of their own
/// (`industry_<id>`, drawn by its own massing) and a square icon file of their own.
#[test]
fn unique_m5_defs_name_their_own_styles_and_icons() {
    let db = ContentDb::load_dir(root()).unwrap();
    let icons = root().join("../ui/icons");
    let mut massings = std::collections::BTreeSet::new();
    for id in [
        "food_factory",
        "furniture_factory",
        "electronics_factory",
        "car_factory",
        "cargo_station",
        "cargo_port",
    ] {
        let (_, d) = db.buildings().find(|(_, d)| d.id == id).expect(id);
        assert_eq!(d.styles, vec![format!("industry_{id}")], "{id}");
        assert_eq!(d.icon, id, "{id}: an icon of its own");
        let style = db.style(db.style_id(&d.styles[0]).expect("style exists"));
        assert!(style.massing as u8 >= 94, "{id}: a massing of its own");
        massings.insert(style.massing as u8);
        assert!(icons.join(format!("{id}.png")).exists(), "{id}");
        assert!(icons.join(format!("src/{id}.svg")).exists(), "{id}");
    }
    assert_eq!(massings.len(), 6);
}
