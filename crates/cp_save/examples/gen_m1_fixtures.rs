//! Generates `fixtures/saves/m1-small.sav` and `fixtures/saves/m1-traffic.sav` (T-046) with
//! the current format, and prints the reference checksums the tests pin
//! (`crates/cp_save/tests/it/m1_fixtures.rs`): the state after loading the file and running
//! 2048 ticks (T-301: a game day). Run from the repo root: `cargo run -p cp_save --example gen_m1_fixtures`.
//! Old saves are not migrated during development (T-275): when the format changes, run it again
//! and re-pin the checksums.
//!
//! * `m1-small`: `grid_city(1800, seed 1)` on a 5x5-street lattice (`people_per_crossing` 100,
//!   or the default lattice would hold more than twice that) — about 1900 residents, built
//!   through commands, saved in the morning.
//! * `m1-traffic`: `grid_city(1800, seed 2)` on the same lattice, then 700 drivers and 150 walkers sent to two
//!   buildings near the centre (a jam), run until cars and pedestrians are on the road, and a
//!   building demolished on pause right before the save, so `BuildingGrid.removed` is not
//!   empty. The trips are sent by `cp_sim::scenario::send_citizens` (a jam that does not
//!   depend on the time of day; with BUG-022 in, the routine adds its own traffic).

use std::path::Path;
use std::sync::Arc;

use cp_building::{BuildingCommand, BuildingGrid, BuildingState};
use cp_citizen::TripMode;
use cp_content::ContentDb;
use cp_core::SimLimits;
use cp_save::{load_into_runner, save_runner};
use cp_sim::scenario::{GridCityConfig, grid_city, send_citizens};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::{MAP_SIZE, Terrain};
use cp_traffic::Agents;

fn main() {
    let terrain = Arc::new(Terrain::load(Path::new("assets/maps/flatlands")).expect("map"));
    let content = Arc::new(ContentDb::load_dir("assets/content").expect("content"));
    let hash = content.content_hash();

    let cfg = GridCityConfig {
        people_per_crossing: 100,
        ..GridCityConfig::default()
    };
    let (small, report) = grid_city(1800, 1, Arc::clone(&terrain), Arc::clone(&content), &cfg);
    assert!(report.reached && report.rejected.is_empty(), "{report:?}");
    write("m1-small", &small, hash, &terrain, &content);

    let (mut jam, report) = grid_city(1800, 2, Arc::clone(&terrain), Arc::clone(&content), &cfg);
    assert!(report.reached && report.rejected.is_empty(), "{report:?}");
    let centre = bevy_math::Vec2::splat(MAP_SIZE / 2.0);
    let mut active: Vec<_> = jam
        .world()
        .resource::<BuildingGrid>()
        .iter()
        .filter(|(_, b)| b.state == BuildingState::Active)
        .map(|(id, b)| (b.pos.distance(centre), id))
        .collect();
    active.sort_by(|a, b| a.0.total_cmp(&b.0));
    let targets = [active[0].1, active[1].1];
    let cars = send_citizens(&mut jam, 700, &targets, |_| TripMode::Car);
    let walkers = send_citizens(&mut jam, 150, &targets, |_| TripMode::Walk);
    println!("sent {cars} drivers and {walkers} walkers");
    for _ in 0..3000 {
        jam.tick_once();
        let a = jam.world().resource::<Agents>();
        if a.vehicle_count() >= 150 && a.pedestrian_count() >= 20 {
            break;
        }
    }
    let a = jam.world().resource::<Agents>();
    println!(
        "on the road: {} cars, {} pedestrians",
        a.vehicle_count(),
        a.pedestrian_count()
    );
    assert!(a.vehicle_count() > 0 && a.pedestrian_count() > 0);
    // A building goes on pause, right before the save.
    let victim = active.last().unwrap().1;
    let at = jam.tick();
    jam.push_command(
        SimCommand::Building(BuildingCommand::Demolish { building: victim }),
        at,
    );
    jam.apply_due_commands();
    assert!(!jam.world().resource::<BuildingGrid>().removed().is_empty());
    write("m1-traffic", &jam, hash, &terrain, &content);
}

fn write(name: &str, sim: &SimRunner, hash: u64, terrain: &Arc<Terrain>, content: &Arc<ContentDb>) {
    let bytes = save_runner(sim, hash, "flatlands").expect("save");
    let path = format!("fixtures/saves/{name}.sav");
    std::fs::write(&path, &bytes).unwrap();
    let mut fresh = SimRunner::with_limits(1, SimLimits::desktop());
    fresh.set_terrain(Arc::clone(terrain));
    fresh.set_content(Arc::clone(content));
    load_into_runner(&mut fresh, &bytes, hash).expect("load");
    let at_load = fresh.checksum();
    for _ in 0..cp_core::time::TICKS_PER_DAY {
        fresh.tick_once();
    }
    println!(
        "{name}: {} bytes, tick {}, citizens {}, checksum at load {at_load:016x}, after a day {:016x}",
        bytes.len(),
        sim.tick().0,
        sim.world()
            .resource::<cp_citizen::CitizenGrid>()
            .citizen_count(),
        fresh.checksum()
    );
}
