//! T-249: the inspector of a post office and of a building with mail (rows and `MailInfo`).

use std::path::Path;
use std::sync::Arc;

use cp_building::BuildingGrid;
use cp_content::{ContentDb, ServiceKind};
use cp_core::time::TICKS_PER_DAY;
use cp_service::MailState;
use cp_sim::scenario::{GridCityConfig, grid_city};
use cp_sim::{Inspection, Progress, Selection, SimRunner};
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn city() -> SimRunner {
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    grid_city(2500, 1, terrain, content, &GridCityConfig::default()).0
}

fn run(sim: &mut SimRunner, ticks: u64, m: u8) {
    for _ in 0..ticks {
        sim.tick_once();
        sim.world_mut().insert_resource(Progress::at(m));
        cp_sim::progress::refresh_unlocks(sim.world_mut());
    }
}

fn rows(sim: &SimRunner, id: cp_building::BuildingId) -> Vec<(String, String)> {
    match sim.inspect(Selection::Building(id)) {
        Some(b @ Inspection::Building(_)) => b.rows(),
        other => panic!("{other:?}"),
    }
}

fn get(rows: &[(String, String)], l: &str) -> Option<String> {
    rows.iter().find(|(k, _)| k == l).map(|(_, v)| v.clone())
}

#[test]
fn a_post_office_and_a_house_show_their_mail_once_the_post_is_open() {
    let mut sim = city();
    let (office, house) = {
        let db = &sim.world().resource::<cp_content::SimContent>().0;
        let grid = sim.world().resource::<BuildingGrid>();
        let office = grid
            .iter()
            .find(|(_, b)| {
                db.building(b.def)
                    .service
                    .as_deref()
                    .and_then(|s| db.service_id(s))
                    .is_some_and(|s| {
                        let s = db.service(s);
                        s.kind == ServiceKind::Post && s.tier == 1
                    })
            })
            .map(|(id, _)| id)
            .expect("the scripted city has a post office");
        let house = grid
            .iter()
            .find(|(_, b)| db.building(b.def).households > 0)
            .map(|(id, _)| id)
            .unwrap();
        (office, house)
    };
    run(&mut sim, TICKS_PER_DAY, 1);
    assert_eq!(
        get(&rows(&sim, office), "Vans"),
        None,
        "closed: no mail rows"
    );
    assert_eq!(get(&rows(&sim, house), "Mail held"), None);

    run(&mut sim, 6 * TICKS_PER_DAY, 2);
    let r = rows(&sim, office);
    // T-270: 40 vans (was 5), post_office.ron.
    assert!(get(&r, "Vans").unwrap().ends_with("out of 40"), "{r:?}");
    assert!(get(&r, "Collected today").is_some() && get(&r, "Backlog to sort").is_some());
    let r = rows(&sim, house);
    assert!(
        get(&r, "Mail held").is_some() && get(&r, "Mail van").is_some(),
        "{r:?}"
    );

    // A pile is shown as it is.
    sim.world_mut()
        .resource_mut::<MailState>()
        .set_pile(house, 55);
    assert_eq!(get(&rows(&sim, house), "Mail held").as_deref(), Some("55"));
}
