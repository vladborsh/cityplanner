//! A station (T-162) is saved and loaded with its platform and stop; no save version change.

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec3;
use cp_building::{BuildingCommand, BuildingGrid};
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_core::treasury::Treasury;
use cp_core::{Cost, SimLimits};
use cp_net::{Elevation, NetGraph};
use cp_save::*;
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::{Terrain, TileOwnership};
use cp_transit::{StopKind, TransitNet};
use cp_zone::{ZoneCellRef, ZoneGrid};

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
fn a_station_survives_a_save_with_its_platform_and_stop() {
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let hash = content.content_hash();
    let mut a = runner(&content);
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    a.world_mut().insert_resource(TileOwnership::only((1, 1)));
    a.world_mut().resource_mut::<Treasury>().balance = Cost(100_000_000);
    a.world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            &def,
            road,
            CubicBezier3::line(
                Vec3::new(3000.0, 0.0, 3000.0),
                Vec3::new(3200.0, 0.0, 3000.0),
            ),
            Elevation::Ground,
        )
        .unwrap();
    a.tick_once();
    let at = {
        let zones = a.world().resource::<ZoneGrid>();
        let (id, _) = zones.blocks().next().unwrap();
        zones
            .cell_footprint(ZoneCellRef {
                block: id,
                cell: 5 * 4 + 1,
            })
            .unwrap()
            .c
    };
    a.push_command(
        SimCommand::Building(BuildingCommand::Place {
            def: content.building_id("metro_station").unwrap(),
            pos: at,
            rot: 0.0,
        }),
        a.tick(),
    );
    for _ in 0..3 {
        a.tick_once();
    }
    assert_eq!(a.world().resource::<BuildingGrid>().len(), 1);
    assert!(matches!(
        a.world()
            .resource::<TransitNet>()
            .stops
            .iter()
            .next()
            .unwrap()
            .1
            .kind,
        StopKind::Station { .. }
    ));

    let bytes = save_runner(&a, hash, "flatlands").unwrap();
    let mut b = runner(&content);
    load_into_runner(&mut b, &bytes, hash).unwrap();
    assert_eq!(a.checksum(), b.checksum());
    assert_eq!(
        a.world().resource::<TransitNet>(),
        b.world().resource::<TransitNet>()
    );
    for _ in 0..20 {
        a.tick_once();
        b.tick_once();
    }
    assert_eq!(a.checksum(), b.checksum());
    // The stop is still on a platform that exists.
    let t = b.world().resource::<TransitNet>();
    let (_, st) = t.stops.iter().next().unwrap();
    assert!(
        b.world()
            .resource::<NetGraph>()
            .segment(st.kind.segment())
            .is_some()
    );
}
