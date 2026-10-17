//! Format 41 (T-205): resource deposits. The v40 fixture (made by the real v40 writer, a grid
//! city with a district that has a specialisation) loads with the ore and oil of the map
//! still full, and spent deposits survive a save and a load.

use std::path::Path;
use std::sync::Arc;

use cp_content::{ContentDb, ResourceKind};
use cp_core::SimLimits;
use cp_save::*;
use cp_sim::SimRunner;
use cp_terrain::{RES_GRID, ResourceDeposits, Terrain};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn env() -> (Arc<Terrain>, Arc<ContentDb>) {
    (
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap()),
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap()),
    )
}

#[test]
fn the_v40_fixture_loads_with_full_deposits_and_spent_ones_round_trip() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v40-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 40, "made by the v40 writer");
    let (terrain, content) = env();
    let mut sim = SimRunner::with_limits(1, SimLimits::desktop());
    sim.set_terrain(terrain.clone());
    sim.set_content(content.clone());
    load_into_runner(&mut sim, &bytes, content.content_hash()).unwrap();

    let full = ResourceDeposits::from_map(terrain.resources(), content.balance().deposit_per_cell);
    assert!(!full.is_unfilled());
    assert_eq!(sim.world().resource::<ResourceDeposits>(), &full);
    let (cx, cz) = (0..RES_GRID * RES_GRID)
        .map(|i| (i % RES_GRID, i / RES_GRID))
        .find(|&(x, z)| full.remaining(ResourceKind::Ore, x, z) > 100)
        .expect("ore on flatlands");

    // Spend some, save, load into a fresh runner: the spent figure is what comes back.
    let before = full.remaining(ResourceKind::Ore, cx, cz);
    sim.world_mut()
        .resource_mut::<ResourceDeposits>()
        .take(ResourceKind::Ore, cx, cz, 100);
    let saved = save_runner(&sim, content.content_hash(), "flatlands").unwrap();
    assert_eq!(
        read_header(&saved).unwrap().0.format_version,
        FORMAT_VERSION
    );
    let mut again = SimRunner::with_limits(1, SimLimits::desktop());
    again.set_terrain(terrain);
    again.set_content(content.clone());
    load_into_runner(&mut again, &saved, content.content_hash()).unwrap();
    let d = again.world().resource::<ResourceDeposits>();
    assert_eq!(d.remaining(ResourceKind::Ore, cx, cz), before - 100);
}
