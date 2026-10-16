//! Format 30 (T-156): buses and the transit ledger. The v29 fixture (made by the real v29
//! writer) loads without them and round-trips on the new version; a runner saved with buses on
//! the road loads to the same checksum and goes on identically.

use std::path::Path;
use std::sync::Arc;

use cp_content::{ContentDb, RoadClass};
use cp_core::{SimLimits, Tick};
use cp_net::NetGraph;
use cp_save::*;
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_traffic::Side;
use cp_transit::{StopRef, TransitCommand, TransitFleet, TransitLedger, TransitMode};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v29_fixture_loads_without_buses_and_round_trips_on_the_new_version() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v29-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 29, "made by the v29 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    assert!(l.data.fleet.is_empty() && l.data.ledger.is_empty());
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
fn a_load_with_buses_on_the_road_keeps_the_checksum_and_the_fleet() {
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let hash = content.content_hash();
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v29-basic.sav")).unwrap();
    let mut a = runner(&content);
    load_into_runner(&mut a, &bytes, hash).unwrap();
    // A depot on the first street, a line over three others.
    let (stops, depot_seg) = {
        let net = a.world().resource::<NetGraph>();
        let ok = |s: &cp_net::Segment| {
            s.length >= 60.0 && content.road(s.road).class != RoadClass::Highway
        };
        let mut it = net.segments().filter(|(_, s)| ok(s));
        let depot_seg = it.next().unwrap().0;
        let stops: Vec<StopRef> = net
            .segments()
            .filter(|(_, s)| ok(s))
            .take(3)
            .map(|(seg, s)| StopRef::NewRoad {
                seg,
                s: s.length / 2.0,
                side: Side::Right,
            })
            .collect();
        (stops, depot_seg)
    };
    {
        use cp_building::*;
        let mut template = a
            .world()
            .resource::<BuildingGrid>()
            .iter()
            .next()
            .unwrap()
            .1
            .clone();
        template.def = content.building_id("bus_depot").unwrap();
        template.state = BuildingState::Active;
        template.set_access(Some(AccessPoint {
            segment: depot_seg,
            t: 0.5,
        }));
        a.world_mut()
            .resource_mut::<BuildingGrid>()
            .insert(template);
    }
    assert_eq!(stops.len(), 3);
    a.push_command(
        SimCommand::Transit(TransitCommand::CreateLine {
            mode: TransitMode::Bus,
            stops,
        }),
        Tick(a.tick().0 + 1),
    );
    for _ in 0..150 {
        a.tick_once();
    }
    assert!(
        !a.world().resource::<TransitFleet>().buses.is_empty(),
        "a bus is out"
    );
    let bytes = save_runner(&a, hash, "flatlands").unwrap();
    let mut b = runner(&content);
    load_into_runner(&mut b, &bytes, hash).unwrap();
    assert_eq!(
        a.world().resource::<TransitFleet>(),
        b.world().resource::<TransitFleet>()
    );
    assert_eq!(a.checksum(), b.checksum());
    for _ in 0..200 {
        a.tick_once();
        b.tick_once();
    }
    assert_eq!(a.checksum(), b.checksum());
    assert_eq!(
        a.world().resource::<TransitLedger>(),
        b.world().resource::<TransitLedger>()
    );
}
