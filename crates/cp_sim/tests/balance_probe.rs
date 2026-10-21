//! T-244 probe: a scripted growing city for eight game weeks, one table row per week. Prints
//! (`cargo test -p cp_sim --test balance_probe -- --nocapture --ignored`); asserts nothing. The
//! numbers it prints are the targets the balance pass tunes against (population, treasury and
//! its change, income / upkeep, unemployment, demand, mode split, stuck cars, building problems,
//! park levels and visits, tourists, policies' cost).

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_building::{
    Building, BuildingCommand, BuildingFlags, BuildingGrid, BuildingState, Footprint,
};
use cp_content::{ContentDb, ParkKind};
use cp_core::time::{TICKS_PER_DAY, TICKS_PER_WEEK};
use cp_core::{CommandError, CommandOutcome, Cost, Tick, Treasury};
use cp_district::{AreaId, AreaKind, DistrictCommand};
use cp_economy::EconomyState;
use cp_sim::scenario::{GridCityConfig, grid_city, population};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_zone::{ZoneCellRef, ZoneGrid};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn run(sim: &mut SimRunner, cmd: SimCommand) -> Result<(), String> {
    sim.push_command(cmd, sim.tick());
    sim.apply_due_commands();
    let out: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    match out.last().map(|o| o.result.clone()) {
        Some(Ok(_)) | None => Ok(()),
        Some(Err(CommandError::Rejected { reason, .. })) => Err(reason),
        Some(Err(e)) => Err(format!("{e:?}")),
    }
}

fn place_anywhere(sim: &mut SimRunner, c: &ContentDb, def: &str) -> bool {
    let def = c.building_id(def).unwrap();
    let spots: Vec<Vec2> = {
        let zones = sim.world().resource::<ZoneGrid>();
        zones
            .blocks()
            .flat_map(|(block, _)| (0..200u16).map(move |cell| ZoneCellRef { block, cell }))
            .filter_map(|r| zones.cell_footprint(r).map(|o| o.c))
            .collect()
    };
    spots.into_iter().any(|pos| {
        run(
            sim,
            SimCommand::Building(BuildingCommand::Place { def, pos, rot: 0.0 }),
        )
        .is_ok()
    })
}

fn put(sim: &mut SimRunner, c: &ContentDb, id: &str, pos: Vec2) {
    let def = c.building_id(id).unwrap();
    let d = c.building(def);
    sim.world_mut()
        .resource_mut::<BuildingGrid>()
        .insert(Building {
            def,
            footprint: Footprint::Free {
                w: d.lot.0,
                d: d.lot.1,
            },
            level: 1,
            state: BuildingState::Active,
            ready_at: Tick(0),
            pos,
            rot: 0.0,
            access: None,
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        });
}

/// A city-park area over the middle of the town with a gate and two attractions.
fn add_park(sim: &mut SimRunner, c: &ContentDb, fee: f32) -> Option<AreaId> {
    run(
        sim,
        SimCommand::District(DistrictCommand::CreateArea {
            kind: AreaKind::Park(ParkKind::CityPark),
            name: "Park".into(),
        }),
    )
    .ok()?;
    let id = AreaId(sim.view().areas.iter().last()?.0.0);
    let cells = (125..185)
        .flat_map(|x| (125..185).map(move |z| (x, z)))
        .collect();
    run(
        sim,
        SimCommand::District(DistrictCommand::PaintArea { cells, area: id }),
    )
    .ok()?;
    put(sim, c, "city_park_attr_1", Vec2::new(4300.0, 4300.0));
    put(sim, c, "city_park_attr_2", Vec2::new(4400.0, 4300.0));
    if !place_anywhere(sim, c, "city_park_gate") {
        return None;
    }
    let _ = run(
        sim,
        SimCommand::Park(cp_park::ParkCommand::SetFee { area: id, fee }),
    );
    Some(id)
}

/// Two bus lines across the middle of the lattice (one along each axis), a depot and `buses`
/// buses on each. Returns the number of lines made.
fn add_bus_lines(sim: &mut SimRunner, c: &ContentDb, buses: u8) -> usize {
    use cp_traffic::Side;
    use cp_transit::{StopRef, TransitCommand, TransitMode, apply};
    let segs: Vec<(cp_net::SegmentId, Vec2, Vec2, f32)> = sim
        .world()
        .resource::<cp_net::NetGraph>()
        .segments()
        .map(|(id, s)| {
            let (a, b) = (s.curve.point(0.0), s.curve.point(1.0));
            (id, Vec2::new(a.x, a.z), Vec2::new(b.x, b.z), s.length)
        })
        .filter(|(_, _, _, len)| *len > 90.0)
        .collect();
    let mid = |a: Vec2, b: Vec2| (a + b) * 0.5;
    let centre = segs
        .iter()
        .fold(Vec2::ZERO, |acc, (_, a, b, _)| acc + mid(*a, *b))
        / segs.len().max(1) as f32;
    // T-302: station first — the depot before any line.
    if !place_anywhere(sim, c, "bus_depot") {
        println!("no lot for the bus depot");
    }
    let mut made = 0;
    for horizontal in [true, false] {
        let along_across = |a: Vec2, b: Vec2| {
            let m = mid(a, b);
            if horizontal {
                ((b.x - a.x).abs(), (m.y - centre.y).abs())
            } else {
                ((b.y - a.y).abs(), (m.x - centre.x).abs())
            }
        };
        // The street nearest the middle of the lattice.
        let nearest = segs
            .iter()
            .map(|(_, a, b, _)| along_across(*a, *b))
            .filter(|(along, _)| *along > 90.0)
            .map(|(_, across)| across)
            .fold(f32::MAX, f32::min);
        let mut row: Vec<_> = segs
            .iter()
            .filter(|(_, a, b, _)| {
                let (along, across) = along_across(*a, *b);
                along > 90.0 && across <= nearest + 1.0
            })
            .collect();
        row.sort_by(|x, y| {
            let (mx, my) = (mid(x.1, x.2), mid(y.1, y.2));
            if horizontal {
                mx.x.total_cmp(&my.x)
            } else {
                mx.y.total_cmp(&my.y)
            }
        });
        let stops: Vec<StopRef> = row
            .iter()
            .take(10)
            .map(|(id, _, _, len)| StopRef::NewRoad {
                seg: *id,
                s: len / 2.0,
                side: Side::Right,
            })
            .collect();
        if stops.len() < 2 {
            continue;
        }
        let r = apply(
            sim.world_mut(),
            &TransitCommand::CreateLine {
                mode: TransitMode::Bus,
                stops,
            },
        );
        if r.is_err() {
            println!("line refused: {r:?}");
            continue;
        }
        made += 1;
    }
    if made == 0 {
        println!("no bus line was made");
    }
    let lines: Vec<_> = sim
        .world()
        .resource::<cp_transit::TransitNet>()
        .lines
        .iter()
        .map(|(id, _)| id)
        .collect();
    for line in lines {
        let _ = apply(
            sim.world_mut(),
            &TransitCommand::SetLineVehicles { line, n: buses },
        );
    }
    made
}

fn money(sim: &SimRunner) -> f64 {
    sim.world().resource::<Treasury>().balance.0 as f64 / 100.0
}

fn cents(c: Cost) -> f64 {
    c.0 as f64 / 100.0
}

#[test]
#[ignore = "prints a balance table; run by hand"]
fn eight_weeks_of_a_growing_city() {
    // PROBE_CONTENT points at another copy of the content, to compare balance passes.
    let dir = std::env::var("PROBE_CONTENT")
        .map_or_else(|_| Path::new(ROOT).join("../../assets/content"), Into::into);
    let content = Arc::new(ContentDb::load_dir(dir).unwrap());
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let target: usize = std::env::var("PROBE_POP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1500);
    let (mut sim, report) = grid_city(
        target,
        1,
        terrain,
        Arc::clone(&content),
        &GridCityConfig {
            industrial_every: 4,
            ..GridCityConfig::default()
        },
    );
    println!(
        "base city: pop {} rejected {:?}",
        report.population, report.rejected
    );
    // The weeks start from a modest treasury: the table shows whether the city feeds itself.
    sim.world_mut().resource_mut::<Treasury>().balance = Cost(5_000_000);

    let park = add_park(&mut sim, &content, 3.0);
    println!("park area: {park:?}");
    let lines = add_bus_lines(&mut sim, &content, 6);
    println!("bus lines: {lines}");
    let mut policies_on = Vec::new();
    println!(
        "wk   pop   money   delta  income upkeep ratio  unemp%  R/C/I/O demand        walk/car/bus now  bus%trips stuck% trips/wk aband nowater nopower  parkL visits tourists policies"
    );
    let mut last_money = money(&sim);
    let (mut last_done, mut last_stuck) = (0u64, 0u64);
    for week in 1..=8u64 {
        if week == 3 {
            // Every policy the city can switch on.
            for p in content.policies() {
                if run(
                    &mut sim,
                    SimCommand::District(DistrictCommand::SetPolicy {
                        district: None,
                        policy: p.kind,
                        on: true,
                    }),
                )
                .is_ok()
                {
                    policies_on.push(p.kind);
                }
            }
        }
        // The riders of each day are read just before the day's counter starts over.
        let mut riders = 0u32;
        for _ in 0..TICKS_PER_WEEK / TICKS_PER_DAY {
            for _ in 0..TICKS_PER_DAY - 1 {
                sim.tick_once();
            }
            riders += sim
                .world()
                .resource::<cp_transit::TransitNet>()
                .stats
                .values()
                .map(|s| s.riders_today)
                .sum::<u32>();
            sim.tick_once();
        }
        let view = sim.view();
        let st = &*view.city;
        let r = sim.world().resource::<EconomyState>().last_week;
        let (inc, up) = (cents(r.income()), cents(r.upkeep()));
        let now = money(&sim);
        let unemp = if st.adults == 0 {
            0.0
        } else {
            st.unemployed as f32 * 100.0 / st.adults as f32
        };
        let d = |i: usize| st.zones[i].demand;
        let t = &st.traffic;
        let moving: u32 = t.trips_by_mode.iter().sum::<u32>().max(1);
        let (park_level, visits) = park
            .and_then(|id| Some((view.areas.get(id)?.level, view.parks.get(id)?.visitors_week)))
            .unwrap_or((0, 0));
        let done = sim.world().resource::<cp_citizen::TripBook>().completed;
        let stuck = t.despawned_stuck;
        let car_trips = ((done - last_done) as f64 * f64::from(t.trips_by_mode[1])
            / f64::from(moving))
        .max(1.0);
        let stuck_pct = (stuck - last_stuck) as f64 * 100.0 / car_trips;
        let (trips_wk, aband) = (done - last_done, st.buildings.abandoned);
        (last_done, last_stuck) = (done, stuck);
        let bus_share = f64::from(riders) * 100.0 / (trips_wk as f64).max(1.0);
        println!(
            "{week:2} {:5} {:8.0} {:7.0} {:7.0} {:6.0} {:5.2} {:6.1}  {:4.0}/{:4.0}/{:4.0}/{:4.0}     {:3}%/{:3}%/{:3}%  {:7.1}  {:5.1} {:8} {:5} {:7} {:7}  {:5} {:6} {:8} {:8.0}",
            population(&sim),
            now,
            now - last_money,
            inc,
            up,
            inc / up.max(1.0),
            unemp,
            d(0).max(d(1)),
            d(2).max(d(3)),
            d(4),
            d(5),
            t.trips_by_mode[0] * 100 / moving,
            t.trips_by_mode[1] * 100 / moving,
            t.trips_by_mode[2] * 100 / moving,
            bus_share,
            stuck_pct,
            trips_wk,
            aband,
            st.buildings.no_water,
            st.buildings.no_power,
            park_level,
            visits,
            view.freight.tourists_week,
            cents(r.policies),
        );
        last_money = now;
        let _ = TICKS_PER_DAY;
    }
    println!("policies on: {policies_on:?}");
    let st = sim.view().city.clone();
    println!(
        "adults {} employed {} unemployed {}",
        st.adults, st.employed, st.unemployed
    );
    for (z, zs) in cp_sim::stats::ZONES.iter().zip(st.zones.iter()) {
        println!(
            "  {z:?}: painted {} occupied {} homes {} free {} jobs {} free {} levels {:?}",
            zs.painted, zs.occupied, zs.homes, zs.homes_free, zs.jobs, zs.jobs_free, zs.by_level
        );
    }
}
