//! BUG-113 probe: a town with a landfill; per day the garbage trucks started and on the
//! road, who holds garbage (by zone), who has the GARBAGE problem, the landfill's use.
//! `cargo nextest run -p cp_app --test garbage_probe --run-ignored all --no-capture`.

use std::path::Path;

use cp_building::{BuildingGrid, BuildingState};
use cp_content::{ServiceKind, SimContent, VehicleKind, ZoneType};
use cp_core::SimLimits;
use cp_core::time::TICKS_PER_DAY;
use cp_service::{ServiceCoverage, ServiceProblems, ServiceState};
use cp_sim::SimRunner;
use cp_terrain::Terrain;
use cp_traffic::ServiceRuns;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn zone_name(z: ZoneType) -> &'static str {
    match z {
        ZoneType::ResLow | ZoneType::ResHigh => "home",
        ZoneType::ComLow | ZoneType::ComHigh => "shop",
        ZoneType::Industrial => "industry",
        ZoneType::Office => "office",
        ZoneType::None => "other",
    }
}

#[test]
#[ignore = "probe"]
fn garbage_trucks_per_day() {
    let t = Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap();
    let mut sim: SimRunner = cp_app::build_sim(t, SimLimits::desktop(), 10.0, false, None);
    let n: u32 = std::env::var("PROBE_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3000);
    sim.world_mut().resource_mut::<cp_core::Treasury>().balance = cp_core::Cost(1_000_000_000);
    let rej = cp_app::town::lay_out_town(&mut sim);
    println!("rejected {}", rej.len());
    *sim.world_mut().resource_mut::<cp_economy::Decay>() = cp_economy::Decay::disabled();
    *sim.world_mut().resource_mut::<cp_building::Demand>() = cp_building::Demand::fixed(1.0);
    {
        let db = sim.world().resource::<SimContent>().0.clone();
        let tick = sim.tick();
        sim.push_command(
            cp_sim::SimCommand::Net(cp_net::NetCommand::BuildRoad {
                road: db.road_id("basic_2l").unwrap(),
                path: cp_net::RoadPath::Straight {
                    from: bevy_math::Vec2::new(5120.0, 4348.0),
                    to: bevy_math::Vec2::new(5120.0, 5000.0),
                },
                elevation: cp_net::ElevationMode::Ground,
            }),
            tick,
        );
        sim.tick_once();
    }
    let grow: u64 = std::env::var("PROBE_GROW")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(14);
    for _ in 0..grow * TICKS_PER_DAY {
        sim.tick_once();
    }
    let residents = sim
        .world()
        .resource::<cp_citizen::CitizenGrid>()
        .citizens()
        .count();
    println!("residents {residents} (wanted {n})");
    sim.world_mut()
        .resource_mut::<cp_content::ServiceUnlocks>()
        .set(ServiceKind::Garbage, true);
    let db = sim.world().resource::<SimContent>().0.clone();
    let o = cp_app::town::origin();
    let def = db
        .building_id(&std::env::var("PROBE_DUMP").unwrap_or("landfill".into()))
        .unwrap();
    let at = bevy_math::Vec2::new(o.x + 320.0, o.y - 14.0);
    sim.world_mut().resource_mut::<cp_core::Treasury>().balance = cp_core::Cost(1_000_000_000);
    let tick = sim.tick();
    sim.push_command(
        cp_sim::SimCommand::Building(cp_building::BuildingCommand::Place {
            def,
            pos: at,
            rot: 0.0,
        }),
        tick,
    );
    sim.tick_once();
    for o in sim
        .world_mut()
        .resource_mut::<bevy_ecs::message::Messages<cp_core::CommandOutcome>>()
        .drain()
    {
        println!("outcome: {:?}", o.result.err().map(|e| e.to_string()));
    }
    let pop = sim
        .world()
        .get_resource::<cp_sim::stats::CityStats>()
        .map(|c| c.population);
    println!("population {pop:?}");
    let truck = VehicleKind::Truck as u8;
    let mut last_next = sim.world().resource::<ServiceRuns>().parts().0;
    for day in 0..10 {
        let mut max_on_road = 0;
        let mut seen: std::collections::BTreeSet<cp_traffic::RunId> = Default::default();
        for _ in 0..TICKS_PER_DAY {
            sim.tick_once();
            let mine: Vec<_> = sim
                .world()
                .resource::<ServiceRuns>()
                .iter()
                .filter(|(_, r)| r.kind == truck)
                .map(|(id, _)| id)
                .collect();
            let on = mine.len();
            seen.extend(mine);
            max_on_road = max_on_road.max(on);
        }
        let w = sim.world();
        let next = w.resource::<ServiceRuns>().parts().0;
        let started = next - last_next;
        last_next = next;
        let grid = w.resource::<BuildingGrid>();
        let st = w.resource::<ServiceState>();
        let cov = w.resource::<ServiceCoverage>();
        let mut by: std::collections::BTreeMap<&str, (u32, u64, u32, u32)> = Default::default();
        let mut fac = Vec::new();
        for (id, b) in grid.iter() {
            if b.state != BuildingState::Active {
                continue;
            }
            let d = db.building(b.def);
            if d.service
                .as_deref()
                .and_then(|s| db.service_id(s))
                .is_some_and(|s| db.service(s).kind == ServiceKind::Garbage)
            {
                let f = st.facility(id).copied().unwrap_or_default();
                fac.push((d.id.clone(), f.used_today, f.stored));
                continue;
            }
            let e = by.entry(zone_name(d.zone)).or_default();
            e.0 += 1;
            e.1 += u64::from(st.get(id).map_or(0, |s| s.garbage));
            if st.problems_of(id).has(ServiceProblems::GARBAGE) {
                e.2 += 1;
            }
            if cov.coverage_of(ServiceKind::Garbage, b) <= 0.0 {
                e.3 += 1;
            }
        }
        println!(
            "day {day}: garbage trucks {}, all runs {started}, max on road {max_on_road}, facilities {fac:?}",
            seen.len()
        );
        for (z, (n, held, bad, unc)) in by {
            println!("    {z:9} n={n:4} held={held:7} GARBAGE={bad:4} uncovered={unc}");
        }
    }
}
