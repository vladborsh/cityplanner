//! T-252: the placement ghost of a service over zoned buildings is valid (green) and names the
//! buildings it would demolish; the app counts the residents and jobs in them from the sim. Over
//! another service building it stays red. The renderer's red shell covers the whole building.

use std::path::Path;
use std::sync::Arc;

use cp_building::{BuildingGrid, BuildingState, is_growable};
use cp_content::ContentDb;
use cp_sim::SimRunner;
use cp_sim::scenario::{GridCityConfig, grid_city};
use cp_terrain::Terrain;
use cp_tools::BuildingTool;
use cp_tools::building::preview as preview_placement;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn city() -> (SimRunner, Arc<ContentDb>) {
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let (sim, report) = grid_city(
        400,
        1,
        terrain,
        Arc::clone(&content),
        &GridCityConfig::default(),
    );
    assert!(report.reached, "{report:?}");
    (sim, content)
}

#[test]
fn the_ghost_over_a_house_is_green_and_names_what_it_demolishes() {
    let (sim, db) = city();
    let view = sim.view();
    let clinic = db.building_id("clinic").unwrap();
    let tool = BuildingTool {
        def: Some(clinic),
        ..Default::default()
    };
    // An inhabited house with a 2 x 2 lot or more: the clinic on its centre covers it.
    let buildings = sim.world().resource::<BuildingGrid>();
    let citizens = sim.world().resource::<cp_citizen::CitizenGrid>();
    let (house, b) = buildings
        .iter()
        .filter(|(_, b)| b.state == BuildingState::Active && is_growable(db.building(b.def)))
        .find(|(id, b)| citizens.households_in(*id) > 0 && db.building(b.def).lot.0 >= 2)
        .expect("an inhabited house");
    let g = preview_placement(&view, &tool, b.pos).expect("a ghost");
    assert!(g.valid, "{:?}", g.error);
    assert!(g.demolish.contains(&house), "{:?}", g.demolish);
    let (residents, _) = sim.eviction(&g.demolish);
    assert!(residents > 0);
    // The app fills the counts in; the panel's line.
    let mut g = g;
    g.evicted = Some(sim.eviction(&g.demolish));
    let text = g.demolish_text().unwrap();
    assert!(
        text.starts_with(&format!("Demolishes {} building", g.demolish.len()))
            && text.contains(&format!("{residents} residents evicted")),
        "{text}"
    );

    // The renderer tints each doomed building whole (T-252b): its own near mesh, pushed just
    // outside its surfaces, all in the demolish tint.
    let shell = cp_render::demolish_ghost::demolish_mesh(&view, &g.demolish);
    let own: usize = g
        .demolish
        .iter()
        .map(|id| {
            let b = buildings.get(*id).unwrap();
            let terrain = view.terrain.as_deref().unwrap();
            cp_render::building::build_building(
                *id,
                b,
                db.building(b.def),
                &view.net,
                terrain,
                &db,
                cp_core::Tick(view.tick.0),
            )
            .near
            .positions
            .len()
        })
        .sum();
    assert!(own > 0 && shell.positions.len() == own);
    assert!(
        shell
            .colors
            .iter()
            .all(|c| *c == cp_render::demolish_ghost::DEMOLISH_TINT)
    );
    let roof = shell
        .positions
        .iter()
        .map(|p| p[1])
        .fold(f32::MIN, f32::max);
    let ground = view.terrain.as_deref().unwrap().height(b.pos);
    assert!(
        roof > ground + 3.0,
        "the shell reaches the roof: {roof} vs {ground}"
    );

    // Over a service building: red, nothing to demolish.
    let service = buildings
        .iter()
        .find(|(_, b)| db.building(b.def).service.is_some() && b.lot().is_some())
        .map(|(_, b)| b.pos);
    if let Some(pos) = service {
        let g = preview_placement(&view, &tool, pos).expect("a ghost");
        assert!(!g.valid && g.demolish.is_empty(), "{g:?}");
    }
}
