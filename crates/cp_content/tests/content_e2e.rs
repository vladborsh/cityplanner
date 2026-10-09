//! Acceptance tests for `m0-data` content loading, driven from the committed
//! `assets/content` files through the public API only.

use cp_content::{ContentDb, ZoneType};
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
fn committed_content_loads_with_four_m1_roads() {
    let db = ContentDb::load_dir(root()).unwrap();
    assert_eq!(db.roads().count(), 4);
    for id in ["basic_2l", "medium_4l", "large_6l", "highway_1w_3l"] {
        let r = db.road(db.road_id(id).unwrap_or_else(|| panic!("road {id}")));
        assert!(
            r.elevated.is_some() && r.bridge.is_some(),
            "{id} needs elevated + bridge"
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
        ("services/water_pump", "capacity: 1000", "capacity: 1001"),
        ("basic_2l", "road/asphalt", "road/asphalt2"),
    ] {
        let h = ContentDb::from_sources(mutate(file, from, to))
            .unwrap()
            .content_hash();
        assert_ne!(base, h, "{file}: {from} -> {to}");
    }
}

#[test]
fn invalid_content_is_rejected_with_file_and_field() {
    let e = errs(mutate("basic_2l", "width: 12.0,", "width: -12.0,"));
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

    let e = errs(mutate("basic_2l", "offset: 4.75", "offset: 40.0"));
    assert!(e.contains("lanes[0].offset"), "lane outside road: {e}");

    let e = errs(mutate(
        "services/water_pump",
        "vehicles: []",
        "vehicles: [\"nope\"]",
    ));
    assert!(e.contains("nope"), "bad vehicle ref: {e}");

    let e = errs(mutate("basic_2l", "width: 12.0,", "width: 0.0,"));
    assert!(e.contains("width"), "zero width: {e}");

    let mut s = sources();
    s.push(("broken.ron".into(), "Road((".into()));
    assert!(errs(s).contains("broken.ron"));
}

#[test]
fn all_problems_are_reported_together() {
    let mut s = mutate("basic_2l", "width: 12.0,", "width: -1.0,");
    for (p, t) in &mut s {
        if p.contains("car_basic") {
            *t = t.replace("accel:", "bogus: 1, accel:");
        }
    }
    let e = errs(s);
    assert!(e.contains("basic_2l") && e.contains("car_basic"), "{e}");
}
