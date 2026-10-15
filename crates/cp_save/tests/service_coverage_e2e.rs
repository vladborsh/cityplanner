//! Service coverage (T-134) is derived, not saved: a runner loaded in the middle of the 64-tick
//! cycle rebuilds the same coverage and keeps the same checksum as the continuous run.

use std::path::Path;
use std::sync::Arc;

use cp_building::{AccessPoint, BuildingGrid, BuildingState};
use cp_content::{ContentDb, ServiceKind};
use cp_core::SimLimits;
use cp_save::*;
use cp_service::ServiceCoverage;
use cp_sim::SimRunner;
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn runner(content: &Arc<ContentDb>) -> SimRunner {
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::with_limits(1, SimLimits::desktop());
    sim.set_terrain(terrain);
    sim.set_content(Arc::clone(content));
    sim
}

#[test]
fn a_load_in_the_middle_of_the_cycle_has_the_same_coverage_and_checksum() {
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let hash = content.content_hash();
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v19-basic.sav")).unwrap();
    let mut a = runner(&content);
    load_into_runner(&mut a, &bytes, hash).unwrap();
    // The fixture's services are free-standing (no road access, so no coverage): put a coal
    // plant, which has an access point, on the first street.
    {
        let seg = a
            .world()
            .resource::<cp_net::NetGraph>()
            .segments()
            .next()
            .unwrap()
            .0;
        let mut template = a
            .world()
            .resource::<BuildingGrid>()
            .iter()
            .next()
            .unwrap()
            .1
            .clone();
        template.def = content.building_id("coal_power_plant").unwrap();
        template.state = BuildingState::Active;
        template.set_access(Some(AccessPoint {
            segment: seg,
            t: 0.5,
        }));
        a.world_mut()
            .resource_mut::<BuildingGrid>()
            .insert(template);
    }
    // Two cycles in, then 20 ticks past the boundary.
    while a.tick().0 % 64 != 20 || a.world().resource::<ServiceCoverage>().generation < 2 {
        a.tick_once();
    }
    let bytes = save_runner(&a, hash, "flatlands").unwrap();
    let mut b = runner(&content);
    load_into_runner(&mut b, &bytes, hash).unwrap();
    let cov = |s: &SimRunner| s.world().resource::<ServiceCoverage>().clone();
    let (ca, cb) = (cov(&a), cov(&b));
    assert!(
        ca.kinds().any(|k| k == ServiceKind::Power),
        "the coal plant covers the streets"
    );
    for k in ca.kinds() {
        let net = a.world().resource::<cp_net::NetGraph>();
        for (id, _) in net.segments() {
            assert_eq!(ca.on_segment(k, id), cb.on_segment(k, id), "{k:?}");
        }
    }
    assert_eq!(a.checksum(), b.checksum());
    for _ in 0..140 {
        a.tick_once();
        b.tick_once();
    }
    assert_eq!(a.checksum(), b.checksum());
}
