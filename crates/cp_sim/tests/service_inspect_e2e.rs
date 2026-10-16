//! The inspector of a service building shows its day's throughput and stock (T-137).

use std::path::Path;
use std::sync::Arc;

use cp_building::{AccessPoint, BuildingGrid, BuildingState};
use cp_content::ContentDb;
use cp_service::ServiceState;
use cp_sim::scenario::{GridCityConfig, grid_city};
use cp_sim::{Inspection, Selection};
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn a_landfill_reports_served_today_and_stored_and_a_house_its_garbage() {
    let root = Path::new(ROOT).join("../..");
    let terrain = Arc::new(Terrain::load(&root.join("assets/maps/flatlands")).unwrap());
    let content = Arc::new(ContentDb::load_dir(root.join("assets/content")).unwrap());
    let (mut sim, _) = grid_city(
        200,
        3,
        terrain,
        Arc::clone(&content),
        &GridCityConfig::default(),
    );
    let seg = sim
        .world()
        .resource::<cp_net::NetGraph>()
        .segments()
        .next()
        .unwrap()
        .0;
    let (house, mut template) = {
        let (id, b) = sim
            .world()
            .resource::<BuildingGrid>()
            .iter()
            .next()
            .unwrap();
        (id, b.clone())
    };
    template.def = content.building_id("landfill").unwrap();
    template.state = BuildingState::Active;
    template.set_access(Some(AccessPoint {
        segment: seg,
        t: 0.5,
    }));
    let dump = sim
        .world_mut()
        .resource_mut::<BuildingGrid>()
        .insert(template);
    {
        let mut st = sim.world_mut().resource_mut::<ServiceState>();
        let f = st.facility_mut(dump);
        f.used_today = 120;
        f.stored = 900;
        st.get_mut(house).garbage = 42;
    }
    let rows = |sel| match sim.inspect(sel) {
        Some(Inspection::Building(b)) => Inspection::Building(b).rows(),
        other => panic!("{other:?}"),
    };
    let get = |rows: &[(String, String)], l: &str| {
        rows.iter().find(|(k, _)| k == l).map(|(_, v)| v.clone())
    };
    let r = rows(Selection::Building(dump));
    assert_eq!(get(&r, "Service").as_deref(), Some("Garbage"));
    assert_eq!(get(&r, "Served today").as_deref(), Some("120 of 5000"));
    assert_eq!(get(&r, "Stored").as_deref(), Some("900 of 1000000"));
    let r = rows(Selection::Building(house));
    assert_eq!(get(&r, "Garbage").as_deref(), Some("42"));
    assert_eq!(get(&r, "Service"), None);
}
