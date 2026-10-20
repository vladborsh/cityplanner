//! T-261: the service placement sweep (`architecture/system-design.md` §6.8.5): a service
//! building with a lot fits wherever its lot stands against a road edge: mid-street, in the
//! corners of a crossing and of a T, inside a rounded corner, on both sides of a curve and
//! across a street's automatic cut ([`cp_sim::road_cases::place_sites`]).
//!
//! Every lot size of the road-access service buildings up to 4 x 4 cells, with `basic_2l`,
//! `medium_4l` and `large_6l`, square to the map and turned by 30 degrees. The cursor is the
//! lot's centre when the lot is pushed into the spot (0.5 m off the edges). Check: `refused`
//! the placement is refused (the error is in the message).
//!
//! Run: `cargo nextest run -p cp_sim --test place_sweep`; `PLACE_SWEEP_REPORT=1` prints every
//! failing case (`--no-capture`). Cases that fail today are in [`KNOWN_RED`] (T-267 makes them
//! pass); `known_red_cases_still_fail` says when one passes.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use bevy_math::Vec2;
use cp_building::validate_placement_over;
use cp_content::{BuildingDefId, ContentDb, ServiceSite};
use cp_net::RoadPath;
use cp_sim::SimRunner;
use cp_sim::road_cases::{build_roads, place_sites};
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const ROADS: [&str; 3] = ["basic_2l", "medium_4l", "large_6l"];
const TURNS: [f32; 2] = [0.0, 30.0];
/// The lot stands this far off the road edge and off a corner's cross street, metres.
const GAP: f32 = 0.5;

/// Cases that fail today (T-275, roads 2 and 4 cells wide: 184 of 546, all `Cut`: lots of 3 cells
/// and more in the corners of a crossing and of a T, the larger lots inside a rounded corner and
/// every lot inside a curve; mid-street and across a cut all fit), as prefixes of
/// `site/road/spot/WxD/turn` (a prefix ending in `/` covers every case under it). T-267 empties it.
const KNOWN_RED: &[&str] = &[
    "x/basic_2l/ne/3x3/",
    "x/basic_2l/ne/3x4/",
    "x/basic_2l/ne/4x4/",
    "x/basic_2l/nw/3x3/",
    "x/basic_2l/nw/3x4/",
    "x/basic_2l/nw/4x4/",
    "x/basic_2l/sw/3x3/",
    "x/basic_2l/sw/3x4/",
    "x/basic_2l/sw/4x4/",
    "x/basic_2l/se/3x3/",
    "x/basic_2l/se/3x4/",
    "x/basic_2l/se/4x4/",
    "t/basic_2l/corner_e/3x3/",
    "t/basic_2l/corner_e/3x4/",
    "t/basic_2l/corner_e/4x4/",
    "t/basic_2l/corner_w/3x3/",
    "t/basic_2l/corner_w/3x4/",
    "t/basic_2l/corner_w/4x4/",
    "rounded/basic_2l/inside/2x2/",
    "rounded/basic_2l/inside/3x2/",
    "rounded/basic_2l/inside/3x3/",
    "rounded/basic_2l/inside/3x4/",
    "rounded/basic_2l/inside/4x4/",
    "curve/basic_2l/inside/",
    "x/medium_4l/ne/3x3/",
    "x/medium_4l/ne/3x4/",
    "x/medium_4l/ne/4x4/",
    "x/medium_4l/nw/3x3/",
    "x/medium_4l/nw/3x4/",
    "x/medium_4l/nw/4x3/",
    "x/medium_4l/nw/4x4/",
    "x/medium_4l/sw/3x3/",
    "x/medium_4l/sw/3x4/",
    "x/medium_4l/sw/4x3/",
    "x/medium_4l/sw/4x4/",
    "x/medium_4l/se/3x3/",
    "x/medium_4l/se/3x4/",
    "x/medium_4l/se/4x4/",
    "t/medium_4l/corner_e/3x3/",
    "t/medium_4l/corner_e/3x4/",
    "t/medium_4l/corner_e/4x4/",
    "t/medium_4l/corner_w/3x3/",
    "t/medium_4l/corner_w/3x4/",
    "t/medium_4l/corner_w/4x3/",
    "t/medium_4l/corner_w/4x4/",
    "rounded/medium_4l/inside/3x3/",
    "rounded/medium_4l/inside/3x4/",
    "rounded/medium_4l/inside/4x4/",
    "curve/medium_4l/inside/",
    "x/large_6l/ne/3x3/",
    "x/large_6l/ne/3x4/",
    "x/large_6l/ne/4x4/",
    "x/large_6l/nw/3x3/",
    "x/large_6l/nw/3x4/",
    "x/large_6l/nw/4x3/",
    "x/large_6l/nw/4x4/",
    "x/large_6l/sw/3x3/",
    "x/large_6l/sw/3x4/",
    "x/large_6l/sw/4x3/",
    "x/large_6l/sw/4x4/",
    "x/large_6l/se/3x3/",
    "x/large_6l/se/3x4/",
    "x/large_6l/se/4x4/",
    "t/large_6l/corner_e/3x3/",
    "t/large_6l/corner_e/3x4/",
    "t/large_6l/corner_e/4x4/",
    "t/large_6l/corner_w/3x3/",
    "t/large_6l/corner_w/3x4/",
    "t/large_6l/corner_w/4x3/",
    "t/large_6l/corner_w/4x4/",
    "rounded/large_6l/inside/3x3/",
    "rounded/large_6l/inside/3x4/",
    "rounded/large_6l/inside/4x4/",
    "curve/large_6l/inside/",
];

fn known_red(case: &str) -> bool {
    KNOWN_RED.iter().any(|k| case.starts_with(k))
}

fn assets() -> (Arc<ContentDb>, Arc<Terrain>) {
    static A: OnceLock<(Arc<ContentDb>, Arc<Terrain>)> = OnceLock::new();
    A.get_or_init(|| {
        let db = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
        let t = Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap();
        (Arc::new(db), Arc::new(t))
    })
    .clone()
}

/// One road-access service building on land per lot size up to 4 x 4, by lot.
fn services(db: &ContentDb) -> BTreeMap<(u8, u8), (BuildingDefId, String)> {
    let mut out = BTreeMap::new();
    for (id, d) in db.buildings() {
        let Some(svc) = d.service.as_deref().and_then(|s| db.service_id(s)) else {
            continue;
        };
        let svc = db.service(svc);
        if !svc.road_access || svc.site != ServiceSite::Land || d.lot.0 > 4 || d.lot.1 > 4 {
            continue;
        }
        out.entry(d.lot).or_insert((id, d.id.clone()));
    }
    out
}

type Outcome = (String, Vec<String>);

fn all_cases() -> Vec<Outcome> {
    let (db, terrain) = assets();
    let services = services(&db);
    let mut out = Vec::new();
    let origin = Vec2::new(4600.0, 4600.0);
    for road in ROADS {
        let hw = db.road(db.road_id(road).unwrap()).width / 2.0;
        for site in place_sites(road, hw) {
            for turn in TURNS {
                let rot = Vec2::from_angle(turn.to_radians());
                let roads: Vec<_> = site
                    .roads
                    .iter()
                    .map(|(r, p)| {
                        let p = match p {
                            RoadPath::Straight { from, to } => RoadPath::Straight {
                                from: rot.rotate(*from),
                                to: rot.rotate(*to),
                            },
                            RoadPath::Curve { from, control, to } => RoadPath::Curve {
                                from: rot.rotate(*from),
                                control: rot.rotate(*control),
                                to: rot.rotate(*to),
                            },
                            RoadPath::Free(v) => {
                                RoadPath::Free(v.iter().map(|q| rot.rotate(*q)).collect())
                            }
                        };
                        (*r, p)
                    })
                    .collect();
                let mut sim = SimRunner::new(1);
                sim.set_terrain(Arc::clone(&terrain));
                sim.set_content(Arc::clone(&db));
                sim.tick_once();
                let refused = build_roads(&mut sim, origin, &roads);
                let w = sim.world();
                let zones = w.resource::<cp_zone::ZoneGrid>();
                let net = w.resource::<cp_net::NetGraph>();
                let buildings = w.resource::<cp_building::BuildingGrid>();
                for spot in &site.spots {
                    for (&(lw, ld), (def, name)) in &services {
                        let case = format!("{}/{}/{lw}x{ld}/{turn}", site.name, spot.name);
                        let mut fails: Vec<String> = refused
                            .iter()
                            .map(|(i, e)| format!("road {i} refused: {e}"))
                            .collect();
                        let at = origin
                            + rot.rotate(spot.centre(
                                f32::from(lw) * 8.0,
                                f32::from(ld) * 8.0,
                                GAP,
                            ));
                        let r = validate_placement_over(
                            &db, zones, net, buildings, None, None, *def, at, 0.0,
                        );
                        if let Err(e) = r {
                            fails.push(format!("{name} ({lw}x{ld}) refused: {e:?}"));
                        }
                        out.push((case, fails));
                    }
                }
            }
        }
    }
    out
}

fn report(cases: &[Outcome]) -> String {
    let mut s = String::new();
    for (case, fails) in cases.iter().filter(|(_, f)| !f.is_empty()) {
        s.push_str(&format!("{case}\n"));
        for m in fails {
            s.push_str(&format!("    [refused] {m}\n"));
        }
    }
    let red = cases.iter().filter(|(_, f)| !f.is_empty()).count();
    format!("{red} of {} cases red\n{s}", cases.len())
}

fn results() -> &'static [Outcome] {
    static R: OnceLock<Vec<Outcome>> = OnceLock::new();
    R.get_or_init(|| {
        let r = all_cases();
        if std::env::var_os("PLACE_SWEEP_REPORT").is_some() {
            println!("{}", report(&r));
            for (c, f) in &r {
                println!("CASE {} {c}", if f.is_empty() { "green" } else { "red" });
            }
        }
        r
    })
}

/// Every case not in [`KNOWN_RED`] places its service building.
#[test]
fn the_green_cases_place_their_service() {
    let red: Vec<_> = results()
        .iter()
        .filter(|(c, f)| !f.is_empty() && !known_red(c))
        .cloned()
        .collect();
    assert!(red.is_empty(), "{}", report(&red));
}

/// The cases under [`KNOWN_RED`] still fail; one that passes is fixed: narrow its prefix.
#[test]
fn known_red_cases_still_fail() {
    let fixed: Vec<&str> = results()
        .iter()
        .filter(|(c, f)| f.is_empty() && known_red(c))
        .map(|(c, _)| c.as_str())
        .collect();
    assert!(
        fixed.is_empty(),
        "{} cases pass now, take them out of KNOWN_RED: {fixed:?}",
        fixed.len()
    );
}

#[test]
fn the_sweep_has_lots_of_several_sizes() {
    let (db, _) = assets();
    assert!(services(&db).len() >= 3, "{:?}", services(&db).keys());
}
