//! Format 28 (T-153): bus stops and lines. The v27 fixture (made by the real v27 writer) loads
//! with none and round-trips on the new version; a runner saved with a line in the middle of a
//! run loads to the same checksum and the same routes.

use std::path::Path;
use std::sync::Arc;

use cp_content::{ContentDb, RoadClass};
use cp_core::{SimLimits, Tick};
use cp_net::NetGraph;
use cp_save::*;
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_traffic::Side;
use cp_transit::{LinePlans, StopRef, TransitCommand, TransitMode, TransitNet};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v27_fixture_loads_without_transit_and_round_trips_on_the_new_version() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v27-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 27, "made by the v27 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    assert!(l.data.transit.is_empty());
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
}

fn runner(content: &Arc<ContentDb>) -> SimRunner {
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::with_limits(1, SimLimits::desktop());
    sim.set_terrain(terrain);
    sim.set_content(Arc::clone(content));
    sim
}

#[test]
fn a_load_in_the_middle_of_a_run_keeps_the_lines_checksum_and_routes() {
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let hash = content.content_hash();
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v27-basic.sav")).unwrap();
    let mut a = runner(&content);
    load_into_runner(&mut a, &bytes, hash).unwrap();
    // Three streets of the fixture, a stop in the middle of each.
    let stops: Vec<StopRef> = {
        let net = a.world().resource::<NetGraph>();
        net.segments()
            .filter(|(_, s)| s.length >= 60.0 && content.road(s.road).class != RoadClass::Highway)
            .take(3)
            .map(|(seg, s)| StopRef::NewRoad {
                seg,
                s: s.length / 2.0,
                side: Side::Right,
            })
            .collect()
    };
    assert_eq!(stops.len(), 3, "the fixture has streets to stop on");
    a.push_command(
        SimCommand::Transit(TransitCommand::CreateLine {
            mode: TransitMode::Bus,
            stops,
        }),
        Tick(a.tick().0 + 1),
    );
    for _ in 0..40 {
        a.tick_once();
    }
    assert_eq!(a.world().resource::<TransitNet>().lines.len(), 1);
    assert_eq!(a.world().resource::<TransitNet>().stops.len(), 3);
    let bytes = save_runner(&a, hash, "flatlands").unwrap();
    let mut b = runner(&content);
    load_into_runner(&mut b, &bytes, hash).unwrap();
    assert_eq!(
        a.world().resource::<TransitNet>(),
        b.world().resource::<TransitNet>()
    );
    assert_eq!(a.checksum(), b.checksum());
    for _ in 0..140 {
        a.tick_once();
        b.tick_once();
    }
    assert_eq!(a.checksum(), b.checksum());
    // The routes were derived again after the load.
    let plans = |s: &SimRunner| {
        let net = s.world().resource::<TransitNet>();
        let plans = s.world().resource::<LinePlans>();
        net.lines
            .iter()
            .map(|(id, _)| plans.get(id).map(|p| (p.legs.len(), p.length_m, p.is_ok())))
            .collect::<Vec<_>>()
    };
    assert_eq!(plans(&a), plans(&b));
    assert!(plans(&b)[0].is_some());
}
