//! T-270 probe: how the movement clock (`cp_core::time::MOVE_SECONDS_PER_TICK`) shows in a
//! grown city: per game day the population, the agents on the map, the trips under way, the
//! average trip time (game minutes, on foot and by car), stuck despawns and the road load.
//! Prints a table (`cargo nextest run -p cp_sim --test agent_speed_probe --run-ignored only
//! --no-capture`); asserts nothing.

use std::path::Path;
use std::sync::Arc;

use cp_content::ContentDb;
use cp_core::time::{MOVE_SECONDS_PER_TICK, TICKS_PER_DAY};
use cp_sim::scenario::{GridCityConfig, grid_city, population};
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
#[ignore = "prints a table; run by hand"]
fn agents_and_trips_by_day() {
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let target: usize = std::env::var("PROBE_POP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3000);
    let days: u64 = std::env::var("PROBE_DAYS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(12);
    let (mut sim, _) = grid_city(target, 1, terrain, content, &GridCityConfig::default());
    println!(
        "MOVE_SECONDS_PER_TICK {MOVE_SECONDS_PER_TICK}; a 50 km/h car moves {:.1} m a tick",
        50.0 / 3.6 * MOVE_SECONDS_PER_TICK
    );
    println!(
        "day   pop  cars(max) agents(max)  in-transit(avg)  walk-min  car-min  stuck  no-path  load(avg/max)  employed unemployed abandoned"
    );
    let samples = 16;
    for day in 0..days {
        let (mut max_agents, mut max_cars, mut transit) = (0u32, 0u32, 0u64);
        let mut kinds = [0u32; 6];
        let st0 = sim.world().resource::<cp_traffic::Agents>().stats.clone();
        for _ in 0..samples {
            for _ in 0..TICKS_PER_DAY / samples {
                sim.tick_once();
            }
            let city = sim.view().city.clone();
            let t = &city.traffic;
            max_agents = max_agents.max(t.vehicles + t.pedestrians);
            if t.vehicles > max_cars {
                max_cars = t.vehicles;
                // What the vehicles on the map are at the peak: citizens' cars, keep runs
                // moving and parked, garbage trucks, goods trucks, other service vehicles.
                let mut k = [0u32; 6];
                for (_, v) in sim.world().resource::<cp_traffic::Agents>().vehicles() {
                    let i = if v.is_parked() {
                        2
                    } else if v.is_keep() {
                        1
                    } else if v.citizen.index != u32::MAX {
                        0
                    } else if cp_traffic::runs::is_goods_kind(v.kind_id()) {
                        4
                    } else if v.kind_id() == 1 {
                        3
                    } else {
                        5
                    };
                    k[i] += 1;
                }
                kinds = k;
            }
            transit += u64::from(t.trips_in_transit);
        }
        let city = sim.view().city.clone();
        let t = &city.traffic;
        println!(
            "{:3} {:5} {:9} {:11} {:16.0} {:9.1} {:8.1} {:6} {:8} {:6.2}/{:.2} {:9} {:10} {:9}",
            day + 1,
            population(&sim),
            max_cars,
            max_agents,
            transit as f64 / samples as f64,
            t.trip_minutes[0],
            t.trip_minutes[1],
            t.despawned_stuck,
            t.no_path,
            t.avg_load,
            t.max_load,
            city.employed,
            city.unemployed,
            city.buildings.abandoned,
        );
        let st = sim.world().resource::<cp_traffic::Agents>().stats.clone();
        println!(
            "      cars arrived {}  walks {}  abstract {}",
            st.driven - st0.driven,
            st.walked - st0.walked,
            st.abstract_trips - st0.abstract_trips
        );
        println!(
            "      at the peak: citizens' cars {} keep {} parked {} garbage {} goods {} other service {}",
            kinds[0], kinds[1], kinds[2], kinds[3], kinds[4], kinds[5]
        );
    }
}
