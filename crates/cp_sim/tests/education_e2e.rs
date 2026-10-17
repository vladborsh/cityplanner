//! A university student takes no job (T-141).

use std::path::Path;
use std::sync::Arc;

use cp_building::{AccessPoint, BuildingGrid, BuildingState};
use cp_citizen::{CitizenGrid, DAYS_PER_YEAR, Location};
use cp_content::ContentDb;
use cp_core::Tick;
use cp_sim::scenario::{GridCityConfig, grid_city};
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn a_nineteen_year_old_enrols_in_the_university_and_takes_no_job() {
    let root = Path::new(ROOT).join("../..");
    let terrain = Arc::new(Terrain::load(&root.join("assets/maps/flatlands")).unwrap());
    let content = Arc::new(ContentDb::load_dir(root.join("assets/content")).unwrap());
    let (mut sim, _) = grid_city(
        300,
        3,
        terrain,
        Arc::clone(&content),
        &GridCityConfig::default(),
    );
    let (home, mut template) = {
        let grid = sim.world().resource::<BuildingGrid>();
        let (id, b) = grid
            .iter()
            .find(|(id, b)| {
                b.state == BuildingState::Active
                    && sim.world().resource::<CitizenGrid>().households_in(*id) > 0
            })
            .unwrap();
        (id, b.clone())
    };
    template.def = content.building_id("university").unwrap();
    template.state = BuildingState::Active;
    // On the street of the home it copies, so the way there is short.
    let seg = template.access().expect("a home has a street").segment;
    template.set_access(Some(AccessPoint {
        segment: seg,
        t: 0.5,
    }));
    let uni = sim
        .world_mut()
        .resource_mut::<BuildingGrid>()
        .insert(template);
    let student = {
        let now = sim.world().resource::<Tick>().0;
        let mut g = sim.world_mut().resource_mut::<CitizenGrid>();
        let h = g.add_household(home, 0, Tick(now), &[(19 * DAYS_PER_YEAR, 1, false)]);
        let id = g.household(h).unwrap().member_ids().next().unwrap();
        // On the move, so that the job search (which skips the moving) does not take him in the
        // very tick the school would: the school is visited first, then he is home.
        g.set_location(id, Location::Moving);
        id
    };
    for _ in 0..300 {
        sim.tick_once();
    }
    sim.world_mut()
        .resource_mut::<CitizenGrid>()
        .set_location(student, Location::Home);
    for _ in 0..900 {
        sim.tick_once();
    }
    let c = sim
        .world()
        .resource::<CitizenGrid>()
        .citizen(student)
        .unwrap();
    assert_eq!(c.school, Some(uni), "enrolled in the university");
    assert!(c.work.is_none(), "a student takes no job");
}
