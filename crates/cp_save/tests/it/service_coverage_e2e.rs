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
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/m1-small.sav")).unwrap();
    let mut a = runner(&content);
    load_into_runner(&mut a, &bytes, hash).unwrap();
    // A coal plant, which has an access point, on the first street (the city of the M1
    // fixture; old saves are refused since T-275).
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
    let start = a.world().resource::<ServiceCoverage>().generation;
    while a.tick().0 % 64 != 20 || a.world().resource::<ServiceCoverage>().generation < start + 2 {
        a.tick_once();
    }
    // A road disappears after the boundary: `a` keeps reading the coverage of the boundary
    // until the next one, and so must the loaded runner.
    {
        let net = a.world().resource::<cp_net::NetGraph>();
        let last = net.segments().last().unwrap().0;
        let fresh_before = a.world().resource::<ServiceCoverage>().clone();
        // As the player removes it: a command (the cars on it go with it, in both runs alike).
        let tick = a.tick();
        a.push_command(
            cp_sim::SimCommand::Net(cp_net::NetCommand::RemoveSegment { seg: last }),
            tick,
        );
        a.tick_once();
        assert!(
            a.world()
                .resource::<cp_net::NetGraph>()
                .segment(last)
                .is_none()
        );
        assert_eq!(*a.world().resource::<ServiceCoverage>(), fresh_before);
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
