//! Test scenes for measuring and screenshots: `--scene grid:N` (desktop) or `?scene=grid:N`
//! (web) builds an N×N street lattice through the normal `BuildRoad` commands.

use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_ecs::resource::Resource;
use bevy_math::Vec2;
use cp_content::ContentDb;
use cp_core::{CommandOutcome, Tick};
use cp_net::{ElevationMode, NetCommand, RoadPath};
use cp_sim::{SimCommand, SimRunner};

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/content_files.rs"));
}

/// All of `assets/content`, compiled in (the browser build has no file system).
pub fn embedded_content() -> Arc<ContentDb> {
    let sources = embedded::FILES
        .iter()
        .map(|(name, text)| (format!("content/{name}"), (*text).to_string()))
        .collect();
    Arc::new(ContentDb::from_sources(sources).expect("embedded content is valid"))
}

/// Kept for callers that only need roads.
pub fn road_content() -> Arc<ContentDb> {
    embedded_content()
}

/// Distance between streets, metres (below the 96 m segment limit).
pub const SPACING: f32 = 100.0;

/// Parses `city:N` (a grid of N streets with zoning and buildings).
pub fn parse_city(spec: &str) -> Option<usize> {
    spec.strip_prefix("city:")?
        .parse()
        .ok()
        .filter(|n| (2..=60).contains(n))
}

/// Showcase scene: every building def in rows by zone, then a row of states.
pub const SHOWCASE: &str = "showcase";

/// Unique buildings scene (T-202, T-203): the ten unique buildings and monuments in a row.
/// Look at it with `--cam uniques`.
pub const UNIQUES: &str = "uniques";

/// Specialisation buildings scene (T-198, T-199): five kinds in columns, small, medium and large
/// in rows. Look at it with `--cam specs`.
pub const SPECS: &str = "specs";

/// Builds the [`UNIQUES`] scene; returns how many buildings stand.
pub fn build_uniques(sim: &mut SimRunner) -> usize {
    build_showcase_defs(sim, |db| {
        let mut defs: Vec<_> = db
            .buildings()
            .filter(|(_, d)| d.unique.is_some())
            .map(|(i, d)| {
                (
                    d.unique.as_ref().is_some_and(|u| u.monument),
                    d.id.clone(),
                    i,
                )
            })
            .collect();
        defs.sort();
        defs.iter()
            .enumerate()
            .map(|(col, (_, _, id))| (*id, -225.0 + col as f32 * 50.0, 0.0))
            .collect()
    })
}

/// M5 chain buildings scene (T-212, T-213): a row per resource (farm, forest, ore, oil), the
/// main building, extractors, processors and warehouse along it. Look at it with `--cam chains`.
pub const CHAINS: &str = "chains";

/// Builds the [`CHAINS`] scene; returns how many buildings stand.
pub fn build_chains(sim: &mut SimRunner) -> usize {
    build_showcase_defs(sim, |db| {
        let mut out = Vec::new();
        for (row, res) in ["farm", "forest", "ore", "oil"].into_iter().enumerate() {
            for (col, role) in [
                "main",
                "extractor_s",
                "extractor_l",
                "processor_a",
                "processor_b",
                "warehouse",
            ]
            .into_iter()
            .enumerate()
            {
                if let Some(id) = db.building_id(&format!("{res}_{role}")) {
                    out.push((id, -125.0 + col as f32 * 50.0, row as f32 * 50.0));
                }
            }
        }
        out
    })
}

/// Chain trucks scene (T-213b): the four cargo trucks and the goods truck in a row on a straight
/// street, each parked at its run's end. Look at it with `--cam trucks`.
pub const TRUCKS: &str = "trucks";

/// Builds the [`TRUCKS`] scene; returns how many trucks stand on the road.
pub fn build_trucks(sim: &mut SimRunner) -> usize {
    use cp_building::{AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingState, Lot};
    use cp_content::{CargoKind, VehicleKind};
    use cp_traffic::{RunPoint, ServiceRuns};
    let db = embedded_content();
    let c = cp_terrain::MAP_SIZE / 2.0;
    let basic = db.road_id("basic_2l").expect("basic_2l");
    sim.push_command(
        SimCommand::Net(NetCommand::BuildRoad {
            road: basic,
            path: RoadPath::Straight {
                from: Vec2::new(c - 100.0, c),
                to: Vec2::new(c + 100.0, c),
            },
            elevation: ElevationMode::Ground,
        }),
        sim.tick(),
    );
    sim.tick_once();
    sim.tick_once();
    // The street may be cut into several segments: `along` metres from its west end is a
    // segment and a t on it.
    let mut segs: Vec<_> = sim
        .world()
        .resource::<cp_net::NetGraph>()
        .segments()
        .map(|(id, s)| (s.curve.point(0.0).x, id, s.length))
        .collect();
    segs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total: f32 = segs.iter().map(|s| s.2).sum();
    let locate = move |along: f32| {
        let mut left = along;
        for &(_, id, len) in &segs {
            if left <= len {
                return (id, (left / len).clamp(0.0, 1.0));
            }
            left -= len;
        }
        let &(_, id, _) = segs.last().expect("a segment");
        (id, 1.0)
    };
    let def = db.building_id("small_park").expect("small_park");
    let d = db.building(def);
    let stand = |sim: &mut SimRunner, f: f32| {
        let (segment, t) = locate(f * total);
        sim.world_mut()
            .resource_mut::<BuildingGrid>()
            .insert(Building {
                def,
                footprint: cp_building::Footprint::Lot(Lot {
                    block: cp_core::Id::new(0, 0),
                    col: 0,
                    w: d.lot.0,
                    d: d.lot.1,
                }),
                level: d.level,
                state: BuildingState::Active,
                ready_at: cp_core::Tick(0),
                pos: Vec2::new(c - 100.0 + f * total, c - 30.0),
                rot: 0.0,
                access: Some(AccessPoint { segment, t }),
                flags: BuildingFlags::default(),
                level_progress: 0.0,
            })
    };
    let kinds = [
        CargoKind::Goods,
        CargoKind::Crops,
        CargoKind::Logs,
        CargoKind::Ore,
        CargoKind::Oil,
    ];
    // Each truck drives 16 m from its own building to the next one; the pairs are 28 m apart
    // (the trucks are 12 m long), so they stay clear of each other.
    let pairs: Vec<_> = (0..kinds.len())
        .map(|i| {
            let f = 0.1 + i as f32 * 0.14;
            (stand(sim, f), stand(sim, f + 0.08))
        })
        .collect();
    let mut runs = sim.world_mut().resource_mut::<ServiceRuns>();
    for (cargo, (from, to)) in kinds.into_iter().zip(pairs) {
        runs.start_run(
            VehicleKind::for_cargo(cargo),
            RunPoint::Building(from),
            RunPoint::Building(to),
            true,
        );
    }
    kinds.len()
}

/// Post scene (T-248): a street with the post office and the sorting facility beside it and a
/// mail van on its way from one to the other. Look at it with `--cam post`, `post-office`,
/// `post-sorting` or `post-van`.
pub const POST: &str = "post";

/// Builds the [`POST`] scene; returns how many mail vans set out.
pub fn build_post(sim: &mut SimRunner) -> usize {
    use cp_building::{AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingState, Lot};
    use cp_content::VehicleKind;
    use cp_traffic::{RunPoint, ServiceRuns};
    let db = embedded_content();
    let c = cp_terrain::MAP_SIZE / 2.0;
    let basic = db.road_id("basic_2l").expect("basic_2l");
    sim.push_command(
        SimCommand::Net(NetCommand::BuildRoad {
            road: basic,
            path: RoadPath::Straight {
                from: Vec2::new(c - 100.0, c),
                to: Vec2::new(c + 100.0, c),
            },
            elevation: ElevationMode::Ground,
        }),
        sim.tick(),
    );
    sim.tick_once();
    sim.tick_once();
    // The segment and t at `x` along the street (it may be cut into several segments).
    let segs: Vec<_> = sim
        .world()
        .resource::<cp_net::NetGraph>()
        .segments()
        .map(|(id, s)| (s.curve.point(0.0).x, s.curve.point(1.0).x, id))
        .collect();
    let locate = |x: f32| {
        segs.iter()
            .find(|s| x >= s.0.min(s.1) && x <= s.0.max(s.1))
            .map(|&(a, b, id)| (id, ((x - a) / (b - a)).clamp(0.0, 1.0)))
            .expect("a segment under x")
    };
    // (def, x of the lot's middle, its depth from the street's middle to the lot's middle)
    let lots = [
        ("post_office", -45.0, 15.0),
        ("post_sorting_facility", 20.0, 23.0),
    ];
    let mut ids = Vec::new();
    for (name, dx, depth) in lots {
        let def = db.building_id(name).expect("a post building");
        let d = db.building(def);
        let (segment, t) = locate(c + dx);
        let id = sim
            .world_mut()
            .resource_mut::<BuildingGrid>()
            .insert(Building {
                def,
                footprint: cp_building::Footprint::Lot(Lot {
                    block: cp_core::Id::new(0, 0),
                    col: 0,
                    w: d.lot.0,
                    d: d.lot.1,
                }),
                level: d.level,
                state: BuildingState::Active,
                ready_at: cp_core::Tick(0),
                pos: Vec2::new(c + dx, c - depth),
                rot: 0.0,
                access: Some(AccessPoint { segment, t }),
                flags: BuildingFlags::default(),
                level_progress: 0.0,
            });
        ids.push(id);
    }
    sim.world_mut().resource_mut::<ServiceRuns>().start_run(
        VehicleKind::Mail,
        RunPoint::Building(ids[0]),
        RunPoint::Building(ids[1]),
        true,
    );
    1
}

/// Houses across the street of the [`POST`] scene with mail piled up (a few, a van's worth,
/// the MAIL problem, a lot), so the Mail overlay (`--overlay mail`) shows its orange and red.
/// Set after the scene has run: a city with the post still locked clears the piles.
pub fn add_mail_houses(sim: &mut SimRunner) {
    use cp_building::{Building, BuildingFlags, BuildingGrid, BuildingState};
    let db = embedded_content();
    let c = cp_terrain::MAP_SIZE / 2.0;
    let def = db.building_id("house_low_1").expect("house_low_1");
    for (dx, pile) in [
        (-60.0, 10u16),
        (-30.0, 45),
        (0.0, 95),
        (30.0, 150),
        (60.0, 220),
    ] {
        let id = sim
            .world_mut()
            .resource_mut::<BuildingGrid>()
            .insert(Building {
                def,
                footprint: cp_building::Footprint::Free { w: 3, d: 3 },
                level: db.building(def).level,
                state: BuildingState::Active,
                ready_at: cp_core::Tick(0),
                pos: Vec2::new(c + dx, c + 18.0),
                rot: 0.0,
                access: None,
                flags: BuildingFlags::default(),
                level_progress: 0.0,
            });
        sim.world_mut()
            .resource_mut::<cp_service::MailState>()
            .set_pile(id, pile);
    }
}

/// Construction scene (T-235): the same house at 0, 20, 40, 60, 80 and 99 % of its construction
/// and, last, finished; the sim stands still. Look at it with `--cam construction`.
pub const CONSTRUCTION: &str = "construction";

/// Builds the [`CONSTRUCTION`] scene: seven buildings of one def and one floor count (the look
/// of a building is a hash of its id, so candidates are made and the ones that match kept).
/// Returns how many buildings stand.
pub fn build_construction(sim: &mut SimRunner, def_name: &str) -> usize {
    use cp_building::{BuildingGrid, BuildingState};
    const SLOTS: usize = 7;
    let now = sim.tick().0;
    let db = embedded_content();
    build_showcase_defs(sim, |db| {
        let id = db.building_id(def_name).expect("the construction def");
        (0..48).map(|_| (id, 0.0, 0.0)).collect()
    });
    let mut grid = sim.world_mut().resource_mut::<BuildingGrid>();
    let floors_of = |grid: &BuildingGrid| -> Vec<(cp_building::BuildingId, u8)> {
        grid.iter()
            .map(|(id, b)| {
                let f = cp_render::building::variant_for(&db, id, b, db.building(b.def))
                    .1
                    .floors;
                (id, f)
            })
            .collect()
    };
    let all = floors_of(&grid);
    // The tallest floor count that at least SLOTS candidates share.
    let floors = (1..=u8::MAX)
        .rev()
        .find(|&f| all.iter().filter(|c| c.1 == f).count() >= SLOTS)
        .expect("seven candidates of one height");
    let keep: Vec<_> = all
        .iter()
        .filter(|c| c.1 == floors)
        .take(SLOTS)
        .map(|c| c.0)
        .collect();
    for (id, _) in &all {
        if !keep.contains(id) {
            grid.remove(*id);
        }
    }
    for (i, id) in keep.iter().enumerate() {
        let b = grid.get_mut(*id).expect("a kept building");
        let total = cp_building::build_ticks(db.building(b.def));
        b.pos = Vec2::new(
            cp_terrain::MAP_SIZE / 2.0 - 150.0 + i as f32 * 50.0,
            cp_terrain::MAP_SIZE / 2.0 - 130.0,
        );
        if i == SLOTS - 1 {
            b.state = BuildingState::Active;
        } else {
            // `ready_at - tick` is what is left (the sim stands still from here).
            let done = [0.0f32, 0.2, 0.4, 0.6, 0.8, 0.99][i];
            b.state = BuildingState::UnderConstruction;
            b.ready_at = cp_core::Tick(now + ((1.0 - done) * total as f32).ceil() as u64);
        }
    }
    sim.world_mut()
        .insert_resource(cp_core::time::SimSpeed::Pause);
    SLOTS
}

/// Garden scene (T-234): one zoned street of low-density homes that the spawner built, each
/// with its trees and bushes. Look at it with `--cam gardens` (or `gardens-close`).
pub const GARDENS: &str = "gardens";

/// Builds the [`GARDENS`] scene: a 240 m street with ResLow zoned on both sides, grown until
/// `homes` buildings stand finished; the sim then stands still. Returns the building count.
pub fn build_gardens(sim: &mut SimRunner, homes: usize) -> usize {
    use cp_building::{BuildingGrid, BuildingState};
    use cp_zone::{ZoneCellRef, ZoneGrid};
    let db = embedded_content();
    let c = cp_terrain::MAP_SIZE / 2.0;
    *sim.world_mut().resource_mut::<cp_economy::Decay>() = cp_economy::Decay::disabled();
    *sim.world_mut().resource_mut::<cp_building::Demand>() = cp_building::Demand::fixed(1.0);
    sim.world_mut()
        .resource_mut::<cp_core::treasury::Treasury>()
        .balance = cp_core::Cost(1_000_000_000);
    sim.push_command(
        SimCommand::Net(NetCommand::BuildRoad {
            road: db.road_id("basic_2l").expect("basic_2l"),
            path: RoadPath::Straight {
                from: Vec2::new(c - 120.0, c),
                to: Vec2::new(c + 120.0, c),
            },
            elevation: ElevationMode::Ground,
        }),
        sim.tick(),
    );
    for _ in 0..4 {
        sim.tick_once();
    }
    let cells: Vec<ZoneCellRef> = {
        let zones = sim.world().resource::<ZoneGrid>();
        zones
            .blocks()
            .flat_map(|(id, b)| {
                (0..b.cell_count() as u16).map(move |cell| ZoneCellRef { block: id, cell })
            })
            .collect()
    };
    {
        let mut zones = sim.world_mut().resource_mut::<ZoneGrid>();
        for cell in cells {
            zones.set_zone(cell, cp_content::ZoneType::ResLow);
        }
    }
    let finished = |sim: &SimRunner| {
        let g = sim.world().resource::<BuildingGrid>();
        g.iter()
            .filter(|(_, b)| b.state == BuildingState::Active)
            .count()
    };
    for _ in 0..40_000 {
        sim.tick_once();
        if finished(sim) >= homes {
            break;
        }
    }
    // No new sites: let the ones under way finish.
    *sim.world_mut().resource_mut::<cp_building::Demand>() = cp_building::Demand::fixed(-1.0);
    for _ in 0..20_000 {
        let building = sim
            .world()
            .resource::<BuildingGrid>()
            .iter()
            .any(|(_, b)| b.state == BuildingState::UnderConstruction);
        if !building {
            break;
        }
        sim.tick_once();
    }
    sim.world_mut()
        .insert_resource(cp_core::time::SimSpeed::Pause);
    sim.world().resource::<BuildingGrid>().len()
}

/// Street scene (T-233): a straight avenue and a local street with homes between them, to see
/// the lamps and utility poles. Look at it with `--cam lamps` (add `--hour 22 --day-night` for
/// the night).
pub const LAMPS: &str = "lamps";

/// Builds the [`LAMPS`] scene: a medium avenue and, 60 m south of it, a local street.
pub fn build_lamps(sim: &mut SimRunner) {
    let db = embedded_content();
    let c = cp_terrain::MAP_SIZE / 2.0;
    sim.world_mut()
        .resource_mut::<cp_core::treasury::Treasury>()
        .balance = cp_core::Cost(1_000_000_000);
    for (road, z, x0, x1) in [
        ("medium_4l", c - 40.0, c - 160.0, c + 160.0),
        ("basic_2l", c + 40.0, c - 160.0, c + 160.0),
    ] {
        sim.push_command(
            SimCommand::Net(NetCommand::BuildRoad {
                road: db.road_id(road).expect("road"),
                path: RoadPath::Straight {
                    from: Vec2::new(x0, z),
                    to: Vec2::new(x1, z),
                },
                elevation: ElevationMode::Ground,
            }),
            sim.tick(),
        );
        sim.tick_once();
    }
    sim.world_mut()
        .insert_resource(cp_core::time::SimSpeed::Pause);
}

/// Shadows scene (BUG-063): a 3×3 street grid with lamps and poles, a power line in the
/// block south of the middle street, and synthetic traffic (cars and pedestrians). Look at
/// it with `--cam shadows`.
pub const SHADOWS: &str = "shadows";

/// Builds the [`SHADOWS`] scene; the app keeps its traffic topped up ([`DemoTraffic`]).
pub fn build_shadows(sim: &mut SimRunner) -> Vec<String> {
    let mut rejected = build_grid(sim, 3);
    let c = cp_terrain::MAP_SIZE / 2.0;
    sim.world_mut().resource_mut::<cp_core::Treasury>().balance = cp_core::Cost(1_000_000_000);
    sim.push_command(
        SimCommand::Utility(cp_utility::UtilityCommand::BuildLink {
            kind: cp_content::UtilityKind::Power,
            // A pylon every 40 m.
            path: (0..5)
                .map(|i| Vec2::new(c - 80.0 + 40.0 * i as f32, c + 22.0))
                .collect(),
        }),
        sim.tick(),
    );
    sim.tick_once();
    rejected.extend(
        sim.world_mut()
            .resource_mut::<Messages<CommandOutcome>>()
            .drain()
            .filter_map(|o| o.result.err().map(|e| e.to_string())),
    );
    sim.world_mut().insert_resource(DemoTraffic {
        cars: 240,
        peds: 900,
    });
    top_up_traffic(sim);
    rejected
}

/// Path scene (T-222): a street with gravel footpaths leaving it across the lawn. Look at it
/// with `--cam paths`.
pub const PATHS: &str = "paths";

/// Builds the [`PATHS`] scene: a local street, a path from its middle that forks into two.
pub fn build_paths(sim: &mut SimRunner) {
    let db = embedded_content();
    let c = cp_terrain::MAP_SIZE / 2.0;
    sim.world_mut()
        .resource_mut::<cp_core::treasury::Treasury>()
        .balance = cp_core::Cost(1_000_000_000);
    let p = |x: f32, z: f32| Vec2::new(c + x, c + z);
    let builds = [
        ("basic_2l", p(-120.0, 0.0), p(120.0, 0.0)),
        ("park_path", p(0.0, 0.0), p(0.0, 30.0)),
        ("park_path", p(0.0, 30.0), p(-40.0, 62.0)),
        ("park_path", p(0.0, 30.0), p(45.0, 55.0)),
        ("park_path", p(45.0, 55.0), p(70.0, 20.0)),
    ];
    for (road, from, to) in builds {
        sim.push_command(
            SimCommand::Net(NetCommand::BuildRoad {
                road: db.road_id(road).expect("road"),
                path: RoadPath::Straight { from, to },
                elevation: ElevationMode::Ground,
            }),
            sim.tick(),
        );
        sim.tick_once();
    }
    sim.world_mut()
        .insert_resource(cp_core::time::SimSpeed::Pause);
}

/// City parks scene (T-221): the four parks in a row. Look at it with `--cam parks`.
pub const PARKS: &str = "parks";

/// Builds the [`PARKS`] scene; returns how many buildings stand.
pub fn build_parks(sim: &mut SimRunner) -> usize {
    build_showcase_defs(sim, |db| {
        ["small_park", "playground", "plaza_fountain", "sports_field"]
            .into_iter()
            .enumerate()
            .filter_map(|(col, name)| {
                db.building_id(name)
                    .map(|id| (id, -75.0 + col as f32 * 50.0, 0.0))
            })
            .collect()
    })
}

pub const PARK_AREAS: &str = "park-areas";

/// Builds the [`PARK_AREAS`] scene (T-227, T-228): the city park gate and its four
/// attractions, the nature reserve's, the zoo's, the amusement park's, and the six props, a row
/// each; returns how many buildings stand.
pub fn build_park_areas(sim: &mut SimRunner) -> usize {
    build_showcase_defs(sim, |db| {
        let rows: [&[&str]; 5] = [
            &[
                "prop_bench",
                "prop_tree_cluster",
                "prop_lamp",
                "prop_flowerbed",
                "prop_kiosk",
                "prop_statue",
            ],
            &[
                "city_park_gate",
                "city_park_attr_1",
                "city_park_attr_2",
                "city_park_attr_3",
                "city_park_attr_4",
            ],
            &[
                "nature_gate",
                "nature_attr_1",
                "nature_attr_2",
                "nature_attr_3",
                "nature_attr_4",
            ],
            &[
                "zoo_gate",
                "zoo_attr_1",
                "zoo_attr_2",
                "zoo_attr_3",
                "zoo_attr_4",
            ],
            &[
                "amusement_gate",
                "amusement_attr_1",
                "amusement_attr_2",
                "amusement_attr_3",
                "amusement_attr_4",
            ],
        ];
        let mut out = Vec::new();
        for (row, names) in rows.iter().enumerate() {
            let step = if row == 0 { 16.0 } else { 40.0 };
            let x0 = -step * (names.len() - 1) as f32 * 0.5;
            for (col, name) in names.iter().enumerate() {
                if let Some(id) = db.building_id(name) {
                    out.push((id, x0 + col as f32 * step, -45.0 * row as f32));
                }
            }
        }
        out
    })
}

pub const FACTORIES: &str = "factories";

/// Builds the [`FACTORIES`] scene (T-218): the four unique factories, the cargo station and the
/// cargo port in a row; returns how many buildings stand.
pub fn build_factories(sim: &mut SimRunner) -> usize {
    build_showcase_defs(sim, |db| {
        [
            "food_factory",
            "furniture_factory",
            "electronics_factory",
            "car_factory",
            "cargo_station",
            "cargo_port",
        ]
        .into_iter()
        .enumerate()
        .filter_map(|(col, name)| {
            db.building_id(name)
                .map(|id| (id, -96.0 + col as f32 * 48.0, 0.0))
        })
        .collect()
    })
}

/// Builds the [`SPECS`] scene; returns how many buildings stand.
pub fn build_specs(sim: &mut SimRunner) -> usize {
    build_showcase_defs(sim, |db| {
        let mut out = Vec::new();
        for (col, kind) in [
            "leisure",
            "tourism",
            "organic",
            "it_cluster",
            "self_sufficient",
        ]
        .into_iter()
        .enumerate()
        {
            for (row, size) in ["s", "m", "l"].into_iter().enumerate() {
                if let Some(id) = db.building_id(&format!("spec_{kind}_{size}")) {
                    out.push((id, -100.0 + col as f32 * 50.0, row as f32 * 45.0));
                }
            }
        }
        out
    })
}

/// Free-standing buildings at `(dx, dz)` from the middle of the map, state Active, on a road
/// segment borrowed for their access point.
fn build_showcase_defs(
    sim: &mut SimRunner,
    layout: impl Fn(&ContentDb) -> Vec<(cp_content::BuildingDefId, f32, f32)>,
) -> usize {
    build_showcase_with(sim, layout, |_, _| {
        (cp_building::BuildingState::Active, cp_core::Tick(0))
    })
}

/// [`build_showcase_defs`] with the state and `ready_at` of the `i`-th building chosen by
/// `state_of(i, def)`.
fn build_showcase_with(
    sim: &mut SimRunner,
    layout: impl Fn(&ContentDb) -> Vec<(cp_content::BuildingDefId, f32, f32)>,
    state_of: impl Fn(usize, &cp_content::BuildingDef) -> (cp_building::BuildingState, cp_core::Tick),
) -> usize {
    use cp_building::{AccessPoint, Building, BuildingFlags, BuildingGrid, Lot};
    let db = embedded_content();
    let c = cp_terrain::MAP_SIZE / 2.0;
    let basic = db.road_id("basic_2l").expect("basic_2l");
    sim.push_command(
        SimCommand::Net(NetCommand::BuildRoad {
            road: basic,
            path: RoadPath::Straight {
                from: Vec2::new(c - 400.0, c - 600.0),
                to: Vec2::new(c - 330.0, c - 600.0),
            },
            elevation: ElevationMode::Ground,
        }),
        sim.tick(),
    );
    sim.tick_once();
    sim.tick_once();
    let segment = sim
        .world()
        .resource::<cp_net::NetGraph>()
        .segments()
        .next()
        .map(|(id, _)| id)
        .expect("a segment");
    let defs = layout(&db);
    let mut grid = sim.world_mut().resource_mut::<BuildingGrid>();
    for (i, (id, dx, dz)) in defs.iter().enumerate() {
        let d = db.building(*id);
        let (state, ready_at) = state_of(i, d);
        grid.insert(Building {
            def: *id,
            footprint: cp_building::Footprint::Lot(Lot {
                block: cp_core::Id::new(0, 0),
                col: 0,
                w: d.lot.0,
                d: d.lot.1,
            }),
            level: d.level,
            state,
            ready_at,
            pos: Vec2::new(c + dx, c - 130.0 + dz),
            rot: 0.0,
            access: Some(AccessPoint { segment, t: 0.5 }),
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        });
    }
    defs.len()
}

/// Industry scene (T-207): a new game with an industry area of each resource painted over the
/// richest spot of that resource. Look at it with `--cam industry --overlay resources` (or
/// `--overlay "industry areas"`).
pub const INDUSTRY: &str = "industry";

/// Builds the [`INDUSTRY`] scene; returns the centres of the four areas.
pub fn build_industry(sim: &mut SimRunner) -> Vec<Vec2> {
    use cp_content::ResourceKind;
    use cp_district::{AreaKind, DistrictCommand};
    use cp_terrain::{RES_GRID, ResourceMap};
    let _ = sim.new_game();
    let terrain = sim.world().resource::<cp_terrain::SimTerrain>().0.clone();
    let c = cp_terrain::MAP_SIZE / 2.0;
    let mut centres = Vec::new();
    for (k, kind) in ResourceKind::ALL.into_iter().enumerate() {
        // The richest cell within 2.5 km of the middle of the map.
        let best = (0..RES_GRID * RES_GRID)
            .map(|i| (i % RES_GRID, i / RES_GRID))
            .filter(|&(x, z)| ResourceMap::cell_centre(x, z).distance(Vec2::splat(c)) < 2500.0)
            .max_by(|a, b| {
                let r = |p: &(usize, usize)| terrain.resources().richness(kind, p.0, p.1);
                r(a).total_cmp(&r(b))
            });
        let Some((cx, cz)) = best else { continue };
        let at = ResourceMap::cell_centre(cx, cz);
        sim.push_command(
            SimCommand::District(DistrictCommand::CreateArea {
                kind: AreaKind::Industry(kind),
                name: format!("{kind:?} area"),
            }),
            sim.tick(),
        );
        sim.tick_once();
        let (ax, az) = (at.x / cp_district::CELL_M, at.y / cp_district::CELL_M);
        let cells: Vec<_> = (-6i32..=6)
            .flat_map(|dz| (-8i32..=8).map(move |dx| (dx, dz)))
            .filter_map(|(dx, dz)| {
                let (x, z) = (ax as i32 + dx, az as i32 + dz);
                (x >= 0 && z >= 0).then_some((x as u16, z as u16))
            })
            .collect();
        sim.push_command(
            SimCommand::District(DistrictCommand::PaintArea {
                cells,
                area: cp_district::AreaId(k as u8 + 1),
            }),
            sim.tick(),
        );
        sim.tick_once();
        centres.push(at);
    }
    centres
}

/// Road tunnel scene (T-173): a street dives under the river of `river_valley`. Run it with
/// `--map river_valley` and look with `--cam tunnel` (add `--overlay underground` to see the
/// tunnel).
pub const TUNNEL: &str = "tunnel";

/// Builds the [`TUNNEL`] scene; returns the west and east portal.
pub fn build_tunnel(sim: &mut SimRunner) -> Option<(Vec2, Vec2)> {
    let db = embedded_content();
    let terrain = sim.world().resource::<cp_terrain::SimTerrain>().0.clone();
    sim.world_mut()
        .resource_mut::<cp_core::treasury::Treasury>()
        .balance = cp_core::Cost(1_000_000_000);
    sim.world_mut()
        .insert_resource(cp_terrain::TileOwnership::m1_around(
            terrain.meta().start_tile,
        ));
    let basic = db.road_id("basic_2l")?;
    let (lo, hi) = (4300.0f32, 6700.0f32);
    for k in 0..60 {
        let z = 4300.0 + k as f32 * 25.0;
        let wet: Vec<f32> = (0..)
            .map(|i| lo + i as f32 * 4.0)
            .take_while(|x| *x <= hi)
            .filter(|x| terrain.is_water(Vec2::new(*x, z)))
            .collect();
        let (Some(&first), Some(&last)) = (wet.first(), wet.last()) else {
            continue;
        };
        let (a, b) = (Vec2::new(first - 160.0, z), Vec2::new(last + 160.0, z));
        if a.x > lo && b.x < hi && (b.x - a.x) < 700.0 {
            sim.push_command(
                SimCommand::Net(NetCommand::BuildRoad {
                    road: basic,
                    path: RoadPath::Straight { from: a, to: b },
                    elevation: ElevationMode::Tunnel(12.0),
                }),
                sim.tick(),
            );
            sim.tick_once();
            // The streets that lead to the portals.
            for (from, to) in [
                (a - Vec2::new(120.0, 0.0), a),
                (b, b + Vec2::new(120.0, 0.0)),
            ] {
                sim.push_command(
                    SimCommand::Net(NetCommand::BuildRoad {
                        road: basic,
                        path: RoadPath::Straight { from, to },
                        elevation: ElevationMode::Ground,
                    }),
                    sim.tick(),
                );
                sim.tick_once();
            }
            return Some((a, b));
        }
    }
    None
}

/// Intercity train scene (T-167): a new game on flatlands with a train station at the dead end of
/// the entry railway and a street beside it, run until the first train stands at the station.
/// Look at it with `--cam train`.
pub const TRAIN: &str = "train";

/// Builds the [`TRAIN`] scene; returns whether a train stands at the station.
pub fn build_train(sim: &mut SimRunner) -> bool {
    use cp_building::BuildingCommand;
    let db = embedded_content();
    let terrain = sim.world().resource::<cp_terrain::SimTerrain>().0.clone();
    if sim.new_game().is_err() {
        return false;
    }
    sim.world_mut()
        .resource_mut::<cp_core::treasury::Treasury>()
        .balance = cp_core::Cost(1_000_000_000);
    let Some(rail) = terrain.meta().entry_rail.as_ref() else {
        return false;
    };
    let end = rail.dead_end();
    let basic = db.road_id("basic_2l").expect("basic_2l");
    let send = |sim: &mut SimRunner, cmd: SimCommand| {
        sim.push_command(cmd, sim.tick());
        sim.tick_once();
    };
    send(
        sim,
        SimCommand::Net(NetCommand::BuildRoad {
            road: basic,
            path: RoadPath::Straight {
                from: Vec2::new(end.x - 150.0, end.y + 28.5),
                to: Vec2::new(end.x - 30.0, end.y + 28.5),
            },
            elevation: ElevationMode::Ground,
        }),
    );
    send(
        sim,
        SimCommand::Building(BuildingCommand::Place {
            def: db.building_id("train_station").expect("train_station"),
            pos: Vec2::new(end.x - 64.0, end.y),
            rot: 0.0,
        }),
    );
    for _ in 0..4000 {
        sim.tick_once();
        let standing = sim
            .world()
            .resource::<cp_transit::Intercity>()
            .train
            .is_some_and(|t| matches!(t.phase, cp_transit::TrainPhase::Dwell { .. }));
        if standing {
            return true;
        }
    }
    false
}

/// Cargo train scene (T-218): a new game on flatlands with a cargo station at the dead end of the
/// entry railway, exports waiting on its platform, run until its cargo train stands there. Look
/// at it with `--cam cargo-train`.
pub const CARGO_TRAIN: &str = "cargo-train";

/// Builds the [`CARGO_TRAIN`] scene; returns whether the cargo train stands at the station.
pub fn build_cargo_train(sim: &mut SimRunner) -> bool {
    use cp_building::{BuildingCommand, BuildingGrid};
    let db = embedded_content();
    let terrain = sim.world().resource::<cp_terrain::SimTerrain>().0.clone();
    if sim.new_game().is_err() {
        return false;
    }
    sim.world_mut()
        .resource_mut::<cp_core::treasury::Treasury>()
        .balance = cp_core::Cost(1_000_000_000);
    let Some(rail) = terrain.meta().entry_rail.as_ref() else {
        return false;
    };
    let end = rail.dead_end();
    let basic = db.road_id("basic_2l").expect("basic_2l");
    let send = |sim: &mut SimRunner, cmd: SimCommand| {
        sim.push_command(cmd, sim.tick());
        sim.tick_once();
    };
    send(
        sim,
        SimCommand::Net(NetCommand::BuildRoad {
            road: basic,
            path: RoadPath::Straight {
                from: Vec2::new(end.x - 190.0, end.y + 28.5),
                to: Vec2::new(end.x - 30.0, end.y + 28.5),
            },
            elevation: ElevationMode::Ground,
        }),
    );
    let def = db.building_id("cargo_station").expect("cargo_station");
    send(
        sim,
        SimCommand::Building(BuildingCommand::Place {
            def,
            pos: Vec2::new(end.x - 80.0, end.y),
            rot: 0.0,
        }),
    );
    let Some(station) = sim
        .world()
        .resource::<BuildingGrid>()
        .iter()
        .find(|(_, b)| b.def == def)
        .map(|(id, _)| id)
    else {
        return false;
    };
    // Exports waiting on the platform: the next train comes for them.
    let now = sim.tick();
    sim.world_mut()
        .resource_mut::<cp_transit::CargoHubs>()
        .hubs
        .insert(
            station,
            cp_transit::Hub {
                out: vec![(cp_content::CargoKind::Logs, 500.0)],
                next_at: now,
                ..Default::default()
            },
        );
    for _ in 0..6000 {
        sim.tick_once();
        let standing = sim
            .world()
            .resource::<cp_transit::CargoHubs>()
            .hubs
            .get(&station)
            .and_then(|h| h.train)
            .is_some_and(|t| matches!(t.phase, cp_transit::TrainPhase::Dwell { .. }));
        if standing {
            return true;
        }
    }
    false
}

/// Cargo ship scene (T-218, `--map river_valley`): a cargo port on the north bank of the river
/// (south of x = 6400, z = 5200) with a ship at its berth, and a second port 200 m east with its
/// ship half way along its line up the river. The sim stands paused. Look at it with `--cam cargo-ship` or
/// `--cam cargo-ship-in`.
pub const CARGO_SHIP: &str = "cargo-ship";

/// Builds the [`CARGO_SHIP`] scene; returns where the ports stand.
pub fn build_cargo_ship(sim: &mut SimRunner) -> Vec<Vec2> {
    use cp_building::{Building, BuildingFlags, BuildingGrid, BuildingState, Lot};
    use cp_transit::{CargoHubs, Hub, ShipPhase, ShipVisit};
    let db = embedded_content();
    let terrain = sim.world().resource::<cp_terrain::SimTerrain>().0.clone();
    let def = db.building_id("cargo_port").expect("cargo_port");
    let sail = (db.balance().ship_sail_h * cp_core::time::TICKS_PER_HOUR).ceil() as u64;
    let mut out = Vec::new();
    for (k, x) in [6400.0f32, 6600.0].into_iter().enumerate() {
        // Walk south from z = 5200 to the water; the port stands 20 m short of it.
        let Some(z) = (0..400)
            .map(|i| 5200.0 - i as f32 * 2.0)
            .find(|&z| terrain.is_water(Vec2::new(x, z)))
        else {
            continue;
        };
        let pos = Vec2::new(x, z + 20.0);
        let d = db.building(def);
        let id = sim
            .world_mut()
            .resource_mut::<BuildingGrid>()
            .insert(Building {
                def,
                footprint: cp_building::Footprint::Lot(Lot {
                    block: cp_core::Id::new(0, 0),
                    col: 0,
                    w: d.lot.0,
                    d: d.lot.1,
                }),
                level: d.level,
                state: BuildingState::Active,
                ready_at: Tick(0),
                pos,
                rot: 0.0,
                access: None,
                flags: BuildingFlags::default(),
                level_progress: 0.0,
            });
        let now = sim.tick();
        let ship = if k == 0 {
            ShipVisit {
                phase: ShipPhase::Docked,
                until: Tick(now.0 + 1_000_000),
            }
        } else {
            // Half way along its line up the river.
            ShipVisit {
                phase: ShipPhase::Arriving,
                until: Tick(now.0 + sail / 2),
            }
        };
        sim.world_mut().resource_mut::<CargoHubs>().hubs.insert(
            id,
            Hub {
                ship: Some(ship),
                next_at: Tick(now.0 + 1_000_000),
                ..Default::default()
            },
        );
        out.push(pos);
    }
    sim.tick_once();
    sim.world_mut()
        .insert_resource(cp_core::time::SimSpeed::Pause);
    for s in sim.view().ships.iter() {
        bevy::log::info!(
            "cargo ship {:?} at {} (berth {}, dir {}, t {}, line {} m: {:?})",
            s.phase,
            s.pos(),
            s.berth,
            s.dir,
            s.t,
            s.length(),
            s.points.iter().step_by(6).collect::<Vec<_>>()
        );
    }
    out
}

/// Zone cell scene (BUG-043, BUG-047): a street built as four chained segments of different
/// lengths and, south of it, a grid of 3 x 3 blocks with 64, 72 and 80 m between the road edges
/// and a side street meeting the east edge in a T. Look at it with `--overlay zones --cam zones`.
pub const ZONES: &str = "zones";

/// Builds the [`ZONES`] scene through the normal commands.
pub fn build_zones(sim: &mut SimRunner) {
    let db = embedded_content();
    let c = cp_terrain::MAP_SIZE / 2.0;
    sim.world_mut()
        .resource_mut::<cp_core::treasury::Treasury>()
        .balance = cp_core::Cost(1_000_000_000);
    let basic = db.road_id("basic_2l").expect("basic_2l");
    let hw = db.road(basic).width / 2.0;
    let mut x = c - 200.0;
    let mut legs: Vec<(Vec2, Vec2)> = Vec::new();
    for len in [90.0f32, 71.0, 85.0, 63.0] {
        legs.push((Vec2::new(x, c), Vec2::new(x + len, c)));
        x += len;
    }
    // Streets at the axis positions that leave 64, 72 and 80 m between the road edges, cut at
    // every crossing.
    let mut pos = vec![0.0f32];
    for gap in [64.0f32, 72.0, 80.0] {
        pos.push(pos.last().copied().unwrap_or(0.0) + gap + 2.0 * hw);
    }
    let o = Vec2::new(c - 130.0, c + 40.0);
    for &z in &pos {
        for w in pos.windows(2) {
            legs.push((o + Vec2::new(w[0], z), o + Vec2::new(w[1], z)));
        }
    }
    for &x in &pos {
        for w in pos.windows(2) {
            legs.push((o + Vec2::new(x, w[0]), o + Vec2::new(x, w[1])));
        }
    }
    // A side street ending on the middle of the east edge: a T.
    let east = pos[3];
    let mid = (pos[1] + pos[2]) / 2.0;
    legs.push((o + Vec2::new(east + 90.0, mid), o + Vec2::new(east, mid)));
    for (from, to) in legs {
        sim.push_command(
            SimCommand::Net(NetCommand::BuildRoad {
                road: basic,
                path: RoadPath::Straight { from, to },
                elevation: ElevationMode::Ground,
            }),
            sim.tick(),
        );
        sim.tick_once();
    }
}

/// Big zone blocks (BUG-056): a grid of 3 x 3 blocks of 96 and 112 m (and 100 m, a tie of the
/// cell fit) between the road edges, every cell painted residential. Look at it with
/// `--cam zones-big --overlay zones`.
pub const ZONES_BIG: &str = "zones-big";

/// Builds the [`ZONES_BIG`] scene through the normal commands.
pub fn build_zones_big(sim: &mut SimRunner) {
    use cp_zone::{ZoneCommand, ZoneGrid};
    let db = embedded_content();
    let c = cp_terrain::MAP_SIZE / 2.0;
    sim.world_mut()
        .resource_mut::<cp_core::treasury::Treasury>()
        .balance = cp_core::Cost(1_000_000_000);
    let basic = db.road_id("basic_2l").expect("basic_2l");
    let hw = db.road(basic).width / 2.0;
    let mut pos = vec![0.0f32];
    for gap in [96.0f32, 112.0, 100.0] {
        pos.push(pos.last().copied().unwrap_or(0.0) + gap + 2.0 * hw);
    }
    let o = Vec2::new(c - 150.0, c - 150.0);
    let mut legs: Vec<(Vec2, Vec2)> = Vec::new();
    for &z in &pos {
        for w in pos.windows(2) {
            legs.push((o + Vec2::new(w[0], z), o + Vec2::new(w[1], z)));
        }
    }
    for &x in &pos {
        for w in pos.windows(2) {
            legs.push((o + Vec2::new(x, w[0]), o + Vec2::new(x, w[1])));
        }
    }
    for (from, to) in legs {
        sim.push_command(
            SimCommand::Net(NetCommand::BuildRoad {
                road: basic,
                path: RoadPath::Straight { from, to },
                elevation: ElevationMode::Ground,
            }),
            sim.tick(),
        );
        sim.tick_once();
    }
    sim.tick_once();
    let cells: Vec<_> = {
        let zones = sim.world().resource::<ZoneGrid>();
        zones
            .blocks()
            .flat_map(|(id, _)| zones.block_cells(id))
            .filter(|c| zones.is_valid(*c))
            .collect()
    };
    sim.push_command(
        SimCommand::Zone(ZoneCommand::PaintZones {
            cells,
            zone: cp_content::ZoneType::ResLow,
        }),
        sim.tick(),
    );
    sim.tick_once();
}

/// Grown blocks (T-236): a residential 8 x 8 and 8 x 16 block and an industrial 8 x 16 block,
/// painted and built on until they are full. Look at it with `--cam grown`.
pub const GROWN: &str = "grown";

/// Builds the [`GROWN`] scene through the normal commands and lets the blocks grow.
pub fn build_grown(sim: &mut SimRunner) {
    use cp_zone::{ZoneCommand, ZoneGrid};
    let db = embedded_content();
    let c = cp_terrain::MAP_SIZE / 2.0;
    sim.world_mut()
        .resource_mut::<cp_core::treasury::Treasury>()
        .balance = cp_core::Cost(1_000_000_000);
    let basic = db.road_id("basic_2l").expect("basic_2l");
    let hw = db.road(basic).width / 2.0;
    // Streets across x with 64 m and 128 m between the road edges (8 and 16 cells), and
    // crossing streets 64 m apart, one grid of 3 x 2 blocks.
    let xs = [0.0, 64.0 + 2.0 * hw, 64.0 + 2.0 * hw + 128.0 + 2.0 * hw];
    let zs = [0.0, 64.0 + 2.0 * hw, 2.0 * (64.0 + 2.0 * hw)];
    let o = Vec2::new(c - 120.0, c - 60.0);
    let mut legs: Vec<(Vec2, Vec2)> = Vec::new();
    for &z in &zs {
        for w in xs.windows(2) {
            legs.push((o + Vec2::new(w[0], z), o + Vec2::new(w[1], z)));
        }
    }
    for &x in &xs {
        for w in zs.windows(2) {
            legs.push((o + Vec2::new(x, w[0]), o + Vec2::new(x, w[1])));
        }
    }
    for (from, to) in legs {
        sim.push_command(
            SimCommand::Net(NetCommand::BuildRoad {
                road: basic,
                path: RoadPath::Straight { from, to },
                elevation: ElevationMode::Ground,
            }),
            sim.tick(),
        );
        sim.tick_once();
    }
    sim.tick_once();
    // Residential in the top row of blocks, industrial in the bottom row (by the block's
    // middle).
    let split = o.y + zs[1] + hw;
    let paint: Vec<SimCommand> = {
        let zones = sim.world().resource::<ZoneGrid>();
        let mut res = Vec::new();
        let mut ind = Vec::new();
        for (id, _) in zones.blocks() {
            for cell in zones.block_cells(id) {
                if !zones.is_valid(cell) {
                    continue;
                }
                match zones.cell_footprint(cell) {
                    Some(f) if f.c.y < split => res.push(cell),
                    Some(_) => ind.push(cell),
                    None => {}
                }
            }
        }
        vec![
            SimCommand::Zone(ZoneCommand::PaintZones {
                cells: res,
                zone: cp_content::ZoneType::ResLow,
            }),
            SimCommand::Zone(ZoneCommand::PaintZones {
                cells: ind,
                zone: cp_content::ZoneType::Industrial,
            }),
        ]
    };
    for cmd in paint {
        sim.push_command(cmd, sim.tick());
        sim.tick_once();
    }
    *sim.world_mut().resource_mut::<cp_building::Demand>() = cp_building::Demand::fixed(1.0);
    for _ in 0..4000 {
        sim.tick_once();
    }
}

/// Corner scene (T-186): an X crossing, T junctions and 90 degree bends of basic streets, for
/// the rounded curbs. Look at it with `--cam corners`.
pub const CORNERS: &str = "corners";

/// Builds the [`CORNERS`] scene through the normal commands.
pub fn build_corners(sim: &mut SimRunner) {
    let db = embedded_content();
    let c = cp_terrain::MAP_SIZE / 2.0;
    sim.world_mut()
        .resource_mut::<cp_core::treasury::Treasury>()
        .balance = cp_core::Cost(1_000_000_000);
    let basic = db.road_id("basic_2l").expect("basic_2l");
    let p = |dx: f32, dz: f32| Vec2::new(c + dx, c + dz);
    // Two streets cross (X); a street ends on the first (T) and a loop of three legs closes a
    // block with two 90 degree bends.
    for (from, to) in [
        (p(-120.0, 0.0), p(-40.0, 0.0)),
        (p(-40.0, 0.0), p(60.0, 0.0)),
        (p(60.0, 0.0), p(120.0, 0.0)),
        (p(-40.0, -80.0), p(-40.0, 0.0)),
        (p(-40.0, 0.0), p(-40.0, 80.0)),
        (p(-40.0, 80.0), p(60.0, 80.0)),
        (p(60.0, 0.0), p(60.0, 80.0)),
        // A 45 degree bend (BUG-054) at the end of the east street.
        (p(120.0, 0.0), p(170.0, 50.0)),
    ] {
        sim.push_command(
            SimCommand::Net(NetCommand::BuildRoad {
                road: basic,
                path: RoadPath::Straight { from, to },
                elevation: ElevationMode::Ground,
            }),
            sim.tick(),
        );
        sim.tick_once();
    }
}

/// Net overlay scene (T-025): a T and an X of basic streets and an X of 6-lane roads, 250 m
/// apart along the map's middle, 160 m legs. Look at each with `--cam net-t` (`net-x`,
/// `net-x6`) and `--net-debug`.
pub const NET_DEBUG: &str = "net-debug";

/// Builds the [`NET_DEBUG`] scene through the normal commands.
pub fn build_net_debug(sim: &mut SimRunner) {
    let db = embedded_content();
    sim.world_mut()
        .resource_mut::<cp_core::treasury::Treasury>()
        .balance = cp_core::Cost(1_000_000_000);
    let c = cp_terrain::MAP_SIZE / 2.0;
    let basic = db.road_id("basic_2l").expect("basic_2l");
    let large = db.road_id("large_6l").expect("large_6l");
    let mut legs = Vec::new();
    for (i, (road, four)) in [(basic, false), (basic, true), (large, true)]
        .into_iter()
        .enumerate()
    {
        let o = Vec2::new(c - 250.0 + 250.0 * i as f32, c);
        legs.push((road, o - Vec2::X * 80.0, o + Vec2::X * 80.0));
        if four {
            legs.push((road, o - Vec2::Y * 80.0, o + Vec2::Y * 80.0));
        } else {
            legs.push((road, o + Vec2::Y * 80.0, o));
        }
    }
    for (road, from, to) in legs {
        sim.push_command(
            SimCommand::Net(NetCommand::BuildRoad {
                road,
                path: RoadPath::Straight { from, to },
                elevation: ElevationMode::Ground,
            }),
            sim.tick(),
        );
        sim.tick_once();
    }
}

/// Wide-road corner scene (BUG-065): bends of 90, 60, 45, 120 and 135 degrees, a T and an X of
/// 4-lane collectors in a row along the map's middle, 200 m apart. Look at each with
/// `--cam wide-90` (`wide-60`, `wide-45`, `wide-120`, `wide-135`, `wide-t`, `wide-x`).
pub const CORNERS_WIDE: &str = "corners-wide";

/// The features of [`CORNERS_WIDE`], west to east: the camera suffix and, for a bend, the
/// direction of its second arm in degrees from +x.
pub const WIDE_FEATURES: [&str; 7] = ["90", "60", "45", "120", "135", "t", "x"];

/// Centre of feature `i` of [`CORNERS_WIDE`].
pub fn wide_feature_centre(i: usize) -> Vec2 {
    let c = cp_terrain::MAP_SIZE / 2.0;
    Vec2::new(c - 600.0 + 200.0 * i as f32, c)
}

/// Builds the [`CORNERS_WIDE`] scene through the normal commands.
pub fn build_corners_wide(sim: &mut SimRunner) {
    let db = embedded_content();
    sim.world_mut()
        .resource_mut::<cp_core::treasury::Treasury>()
        .balance = cp_core::Cost(1_000_000_000);
    let road = db.road_id("medium_4l").expect("medium_4l");
    let dir = |deg: f32| Vec2::from_angle(deg.to_radians());
    let mut legs = Vec::new();
    for (i, f) in WIDE_FEATURES.iter().enumerate() {
        let o = wide_feature_centre(i);
        match *f {
            "t" => {
                legs.push((o - Vec2::X * 80.0, o + Vec2::X * 80.0));
                legs.push((o + Vec2::Y * 70.0, o));
            }
            "x" => {
                legs.push((o - Vec2::X * 80.0, o + Vec2::X * 80.0));
                legs.push((o - Vec2::Y * 70.0, o + Vec2::Y * 70.0));
            }
            deg => {
                let deg: f32 = deg.parse().expect("a bend angle");
                legs.push((o - Vec2::X * 70.0, o));
                legs.push((o, o + dir(deg) * 70.0));
            }
        }
    }
    for (from, to) in legs {
        sim.push_command(
            SimCommand::Net(NetCommand::BuildRoad {
                road,
                path: RoadPath::Straight { from, to },
                elevation: ElevationMode::Ground,
            }),
            sim.tick(),
        );
        sim.tick_once();
    }
}

/// Metro scene (T-164): a street, three metro stations beside it, the tunnels between the
/// platforms and a metro line with two trains. Look at it with `--overlay underground`
/// and `--cam metro`.
pub const METRO: &str = "metro";

/// Builds the [`METRO`] scene through the normal commands; returns the stations placed.
pub fn build_metro(sim: &mut SimRunner) -> usize {
    use cp_building::{BuildingCommand, BuildingGrid};
    let db = embedded_content();
    let c = cp_terrain::MAP_SIZE / 2.0;
    sim.world_mut()
        .resource_mut::<cp_core::treasury::Treasury>()
        .balance = cp_core::Cost(1_000_000_000);
    let basic = db.road_id("basic_2l").expect("basic_2l");
    let metro = db.road_id("metro_track").expect("metro_track");
    let send = |sim: &mut SimRunner, cmd: SimCommand| {
        sim.push_command(cmd, sim.tick());
        sim.tick_once();
    };
    for k in 0..6 {
        let x = c - 600.0 + 200.0 * k as f32;
        send(
            sim,
            SimCommand::Net(NetCommand::BuildRoad {
                road: basic,
                path: RoadPath::Straight {
                    from: Vec2::new(x, c),
                    to: Vec2::new(x + 200.0, c),
                },
                elevation: ElevationMode::Ground,
            }),
        );
    }
    let xs = [c - 450.0, c - 100.0, c + 300.0];
    let mut stations = Vec::new();
    for x in xs {
        send(
            sim,
            SimCommand::Building(BuildingCommand::Place {
                def: db.building_id("metro_station").expect("metro_station"),
                pos: Vec2::new(x, c + 12.0),
                rot: 0.0,
            }),
        );
        if let Some((id, _)) = sim.world().resource::<BuildingGrid>().iter().last() {
            stations.push(id);
        }
    }
    // The tunnels, platform end to platform end.
    let ends: Vec<(Vec2, Vec2)> = {
        let transit = sim.world().resource::<cp_transit::TransitNet>();
        let net = sim.world().resource::<cp_net::NetGraph>();
        let mut v: Vec<(Vec2, Vec2)> = transit
            .stops
            .iter()
            .filter_map(|(_, s)| match s.kind {
                cp_transit::StopKind::Station { seg, .. } => net.segment(seg),
                _ => None,
            })
            .map(|s| {
                let (a, b) = (net.node(s.start).unwrap().pos, net.node(s.end).unwrap().pos);
                let (a, b) = (Vec2::new(a.x, a.z), Vec2::new(b.x, b.z));
                if a.x < b.x { (a, b) } else { (b, a) }
            })
            .collect();
        v.sort_by(|p, q| p.0.x.total_cmp(&q.0.x));
        v
    };
    for w in ends.windows(2) {
        send(
            sim,
            SimCommand::Net(NetCommand::BuildRoad {
                road: metro,
                path: RoadPath::Straight {
                    from: w[0].1,
                    to: w[1].0,
                },
                elevation: ElevationMode::Tunnel(12.0),
            }),
        );
    }
    send(
        sim,
        SimCommand::Transit(cp_transit::TransitCommand::CreateLine {
            mode: cp_transit::TransitMode::Metro,
            stops: stations
                .iter()
                .map(|&b| cp_transit::StopRef::Station(b))
                .collect(),
        }),
    );
    // Let the trains out of the first station.
    for _ in 0..40 {
        sim.tick_once();
    }
    stations.len()
}

/// Parses `grid:N`.
pub fn parse_grid(spec: &str) -> Option<usize> {
    spec.strip_prefix("grid:")?
        .parse()
        .ok()
        .filter(|n| (1..=60).contains(n))
}

/// Parses `traffic:N[:CARS:PEDS]`: an `N`×`N` grid with synthetic traffic (default 16 000 cars
/// and 20 000 pedestrians, the §14 budget scene).
pub fn parse_traffic(spec: &str) -> Option<(usize, usize, usize)> {
    let mut parts = spec.strip_prefix("traffic:")?.split(':');
    let n = parts
        .next()?
        .parse()
        .ok()
        .filter(|n| (2..=60).contains(n))?;
    let cars = parts.next().map_or(Some(16_000), |v| v.parse().ok())?;
    let peds = parts.next().map_or(Some(20_000), |v| v.parse().ok())?;
    Some((n, cars, peds))
}

/// Synthetic traffic kept topped up by the app (`traffic:` scene): the app sends
/// `DebugCommand::TopUpTraffic` through the sim's own command queue whenever the pools run below
/// these numbers.
#[derive(Resource, Clone, Copy, Debug)]
pub struct DemoTraffic {
    pub cars: usize,
    pub peds: usize,
}

/// Adds the missing demo agents. Cheap when the pools are full.
pub fn top_up_traffic(sim: &mut SimRunner) {
    let Some(demo) = sim.world().get_resource::<DemoTraffic>().copied() else {
        return;
    };
    let (cars, peds) = sim.agent_counts();
    if cars < demo.cars || peds < demo.peds {
        sim.top_up_traffic(demo.cars as u32, demo.peds as u32);
    }
}

/// Builds an `n`×`n` lattice (n streets each way, n² crossings) centred on the map, and
/// runs the ticks that apply it. Every fifth street is a 4-lane road. Returns the reasons
/// of rejected commands (empty when everything was built).
pub fn build_grid(sim: &mut SimRunner, n: usize) -> Vec<String> {
    build_grid_with(sim, n, 5)
}

/// Like [`build_grid`]; every `arterial_every`-th street is 4-lane (0 = none).
pub fn build_grid_with(sim: &mut SimRunner, n: usize, arterial_every: usize) -> Vec<String> {
    let content = road_content();
    let basic = content.road_id("basic_2l").expect("basic_2l");
    let medium = content.road_id("medium_4l").expect("medium_4l");
    let c = cp_terrain::MAP_SIZE / 2.0;
    let span = (n - 1) as f32 * SPACING;
    let (x0, z0) = (c - span / 2.0, c - span / 2.0);
    // A scripted scene is not paid for: fund it, and put the treasury back afterwards.
    let treasury = sim.world().resource::<cp_core::Treasury>().balance;
    sim.world_mut().resource_mut::<cp_core::Treasury>().balance = cp_core::Cost(i64::MAX / 2);
    let start = sim.tick().0;
    let mut k = 0;
    // Each street is built in runs of two blocks: one long command would be cut into equal
    // pieces that cross the 100 m crossing grid at awkward places and produce pieces
    // shorter than the sim's minimum.
    let blocks_per_run = 2;
    let mut runs: Vec<(bool, usize, usize)> = Vec::new();
    for horizontal in [true, false] {
        for i in 0..n {
            let mut b = 0;
            while b < n - 1 {
                runs.push((horizontal, i, b));
                b += blocks_per_run;
            }
        }
    }
    for (horizontal, i, b) in runs {
        let o = i as f32 * SPACING;
        let a = b as f32 * SPACING;
        let e = ((b + blocks_per_run).min(n - 1)) as f32 * SPACING;
        let (from, to) = if horizontal {
            (Vec2::new(x0 + a, z0 + o), Vec2::new(x0 + e, z0 + o))
        } else {
            (Vec2::new(x0 + o, z0 + a), Vec2::new(x0 + o, z0 + e))
        };
        let road = if arterial_every > 0 && i % arterial_every == 0 {
            medium
        } else {
            basic
        };
        sim.push_command(
            SimCommand::Net(NetCommand::BuildRoad {
                road,
                path: RoadPath::Straight { from, to },
                elevation: ElevationMode::Ground,
            }),
            Tick(start + k),
        );
        k += 1;
    }
    let mut rejected = Vec::new();
    for _ in 0..=k {
        sim.tick_once();
        let outcomes = sim
            .world_mut()
            .resource_mut::<Messages<CommandOutcome>>()
            .drain()
            .collect::<Vec<_>>();
        rejected.extend(
            outcomes
                .into_iter()
                .filter_map(|o| o.result.err().map(|e| e.to_string())),
        );
    }
    sim.world_mut().resource_mut::<cp_core::Treasury>().balance = treasury;
    rejected
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_terrain::Terrain;

    fn sim(n: usize, every: usize) -> SimRunner {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/maps/flatlands");
        let mut sim = SimRunner::new(1);
        sim.set_terrain(Arc::new(Terrain::load(&dir).unwrap()));
        sim.set_content(road_content());
        let rejected = build_grid_with(&mut sim, n, every);
        assert!(rejected.is_empty(), "{rejected:#?}");
        sim
    }

    #[test]
    fn grid_scene_builds_every_street() {
        for (n, every) in [(3, 0), (10, 0), (10, 5)] {
            let v = sim(n, every).view();
            // The interior crossings are four-way junctions.
            let crossings = v
                .junctions
                .iter()
                .filter(|(_, j)| j.arms.len() == 4)
                .count();
            assert!(
                n < 4 || crossings >= (n - 2) * (n - 2) * 3 / 4,
                "n={n} every={every}"
            );
            assert!(
                v.net.segment_count() >= 2 * n * (n - 1),
                "n={n}: {}",
                v.net.segment_count()
            );
        }
    }

    #[test]
    fn shadows_scene_has_streets_a_power_line_and_traffic() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/maps/flatlands");
        let mut sim = SimRunner::new(1);
        sim.set_terrain(Arc::new(Terrain::load(&dir).unwrap()));
        sim.set_content(road_content());
        let rejected = build_shadows(&mut sim);
        assert!(rejected.is_empty(), "{rejected:#?}");
        for _ in 0..20 {
            sim.tick_once();
        }
        let v = sim.view();
        assert!(v.utilities.links().count() > 0, "no power line");
        assert!(!v.agents.cur.pedestrians.is_empty(), "no pedestrians");
    }

    #[test]
    fn grid_spec_parsing() {
        assert_eq!(parse_grid("grid:10"), Some(10));
        assert_eq!(parse_grid("grid:0"), None);
        assert_eq!(parse_grid("grid:99"), None);
        assert_eq!(parse_grid("nope"), None);
    }
}

/// Zone painted on a block for a city scene: dense and tall in the middle, low and
/// industrial at the edge, mixed in between (deterministic from the block).
fn zone_for(block_index: u32, dist: f32) -> cp_content::ZoneType {
    use cp_content::ZoneType::*;
    let h = block_index.wrapping_mul(2_654_435_761) >> 16;
    match dist {
        d if d < 400.0 => [ComHigh, Office, ResHigh, Office][(h % 4) as usize],
        d if d < 900.0 => [ResHigh, ComLow, ResHigh, ComHigh, ResLow][(h % 5) as usize],
        d if d < 1500.0 => [ResLow, ResLow, ComLow, ResHigh, Industrial][(h % 5) as usize],
        _ => [ResLow, Industrial, Industrial, ResLow][(h % 4) as usize],
    }
}

/// Builds an `n`×`n` street grid, paints zones on every block and lets the spawner run
/// until `target` buildings exist (or `max_ticks` passed). Then dresses the result for a
/// showcase: higher-level variants where the lot allows, some buildings still under
/// construction (at all stages), a few abandoned and burned. Returns the building count.
pub fn build_city(sim: &mut SimRunner, n: usize, target: usize, max_ticks: u64) -> usize {
    use cp_building::{BuildingGrid, BuildingState};
    use cp_zone::{ZoneCellRef, ZoneGrid};
    // A scripted scene is not waited for: every zone is wanted while it grows.
    // The scene has no water or power lines and shows buildings in every state: no decay.
    *sim.world_mut().resource_mut::<cp_economy::Decay>() = cp_economy::Decay::disabled();
    let demand = *sim.world().resource::<cp_building::Demand>();
    *sim.world_mut().resource_mut::<cp_building::Demand>() = cp_building::Demand::fixed(1.0);
    let _ = build_grid_with(sim, n, 5);
    // Zone blocks exist after a few quiet ticks.
    for _ in 0..4 {
        sim.tick_once();
    }
    let centre = Vec2::splat(cp_terrain::MAP_SIZE / 2.0);
    let paint: Vec<(ZoneCellRef, cp_content::ZoneType)> = {
        let zones = sim.world().resource::<ZoneGrid>();
        zones
            .blocks()
            .flat_map(|(id, b)| {
                let p = b.curve.point(0.5);
                let d = Vec2::new(p.x, p.z).distance(centre);
                let zone = zone_for(id.index, d);
                (0..b.cell_count() as u16).map(move |cell| (ZoneCellRef { block: id, cell }, zone))
            })
            .collect()
    };
    {
        let mut zones = sim.world_mut().resource_mut::<ZoneGrid>();
        for (cell, zone) in paint {
            zones.set_zone(cell, zone);
        }
    }
    for _ in 0..max_ticks {
        sim.tick_once();
        if sim.world().resource::<BuildingGrid>().len() >= target {
            break;
        }
    }
    *sim.world_mut().resource_mut::<cp_building::Demand>() = demand;
    dress_city(sim);
    let _ = BuildingState::Active;
    sim.world().resource::<BuildingGrid>().len()
}

/// Night scene (T-245): `night:N` is [`build_city`] held (paused) at the asked hour of the next
/// day, with every building supplied with water and power except the ones in [`DARK_DISTRICT`], to see the lit
/// windows and an outage. Look at it with `--cam night-wide`, `night-street` or `night-dark`.
pub fn parse_night(spec: &str) -> Option<usize> {
    spec.strip_prefix("night:")?
        .parse()
        .ok()
        .filter(|n| (2..=60).contains(n))
}

/// The district of the night scene without power: centre (offset from the map centre) and
/// radius, metres.
pub const DARK_DISTRICT: (Vec2, f32) = (Vec2::new(250.0, -150.0), 180.0);

/// Builds the `night:N` scene (see [`parse_night`]); returns the buildings and how many of them
/// have no power.
pub fn build_night(sim: &mut SimRunner, n: usize, hour: f32) -> (usize, usize) {
    use cp_building::{BuildingFlags, BuildingGrid, BuildingState};
    use cp_core::time::TICKS_PER_DAY;
    // The same city at every hour: it grows from midnight, then the clock jumps.
    sim.world_mut().insert_resource(Tick(0));
    let built = build_city(sim, n, 5200, 6000);
    let now = sim.tick().0;
    let at = (now / TICKS_PER_DAY + 1) * TICKS_PER_DAY
        + (hour.rem_euclid(24.0) / 24.0 * TICKS_PER_DAY as f32) as u64;
    sim.world_mut().insert_resource(Tick(at));
    let dark_at = Vec2::splat(cp_terrain::MAP_SIZE / 2.0) + DARK_DISTRICT.0;
    let mut dark = 0;
    let mut grid = sim.world_mut().resource_mut::<BuildingGrid>();
    let ids: Vec<_> = grid.iter().map(|(id, _)| id).collect();
    for id in ids {
        let Some(b) = grid.get_mut(id) else { continue };
        if b.state == BuildingState::UnderConstruction {
            // Still part way up after the jump in time.
            b.ready_at = Tick(b.ready_at.0 + (at - now));
        }
        // Water and sewage too: their icons would hide the windows.
        b.flags.0 &= !BuildingFlags::UTILITY_MASK;
        if b.pos.distance(dark_at) < DARK_DISTRICT.1 {
            b.flags.0 |= BuildingFlags::NO_POWER;
            dark += 1;
        }
    }
    // Held: the utility pass would put the power back as the sim finds it.
    sim.world_mut()
        .insert_resource(cp_core::time::SimSpeed::Pause);
    (built, dark)
}

/// Showcase dressing (see [`build_city`]), deterministic from the building ids.
fn dress_city(sim: &mut SimRunner) {
    use cp_building::{BuildingGrid, BuildingState};
    let db = sim.world().resource::<cp_content::SimContent>().0.clone();
    let now = sim.tick().0;
    let ids: Vec<_> = sim
        .world()
        .resource::<BuildingGrid>()
        .iter()
        .map(|(id, _)| id)
        .collect();
    let mut grid = sim.world_mut().resource_mut::<BuildingGrid>();
    for id in ids {
        let h = id.index.wrapping_mul(2_246_822_519) >> 8;
        let Some(b) = grid.get(id).cloned() else {
            continue;
        };
        let def = db.building(b.def);
        // A taller variant of the same zone that fits the lot exactly.
        let want = 1 + (h % 3) as u8;
        let better = db
            .buildings()
            .filter(|(_, d)| {
                d.zone == def.zone && d.level == want && d.lot == def.lot && d.service.is_none()
            })
            .map(|(i, _)| i)
            .nth((h >> 4) as usize % 2);
        let state = match h % 100 {
            0..=11 => BuildingState::UnderConstruction,
            12..=14 => BuildingState::Abandoned,
            15 => BuildingState::Burned,
            _ => BuildingState::Active,
        };
        if let Some(m) = grid.get_mut(id) {
            if let Some(d) = better {
                m.def = d;
                m.level = want;
            }
            m.state = state;
            if state == BuildingState::UnderConstruction {
                // Spread over the construction stages.
                m.ready_at = cp_core::Tick(now + 200 + u64::from(h % 4) * 1000);
            }
        }
    }
}

#[cfg(test)]
mod city_tests {
    use super::*;
    use cp_terrain::Terrain;

    #[test]
    fn city_scene_spawns_and_dresses_buildings() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/maps/flatlands");
        let mut sim = SimRunner::new(1);
        sim.set_terrain(Arc::new(Terrain::load(&dir).unwrap()));
        sim.set_content(embedded_content());
        let t = std::time::Instant::now();
        let n = build_city(&mut sim, 6, 150, 800);
        eprintln!("CITY n={n} in {:?}", t.elapsed());
        assert!(n >= 100, "{n}");
        let v = sim.view();
        let states: std::collections::BTreeSet<_> =
            v.buildings.iter().map(|(_, b)| b.state as u8).collect();
        assert!(states.len() >= 3, "{states:?}");
        assert_eq!(parse_city("city:12"), Some(12));
        assert_eq!(parse_city("city:1"), None);
        assert_eq!(parse_night("night:12"), Some(12));
        assert_eq!(parse_night("city:12"), None);
    }
}

/// Places one of every building def (rows by zone family, levels left to right, variants
/// together) plus a row showing construction stages, abandoned and burned. Positions are
/// centred on the map; `rot` is 0 so every front faces -z (towards the default camera).
pub fn build_showcase(sim: &mut SimRunner) -> usize {
    use cp_building::{AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingState, Lot};
    use cp_content::ZoneType::*;
    let db = embedded_content();
    *sim.world_mut().resource_mut::<cp_economy::Decay>() = cp_economy::Decay::disabled();
    // One road to borrow a real segment id from (the access point is only a hint).
    let c = cp_terrain::MAP_SIZE / 2.0;
    let basic = db.road_id("basic_2l").expect("basic_2l");
    sim.push_command(
        SimCommand::Net(NetCommand::BuildRoad {
            road: basic,
            path: RoadPath::Straight {
                from: Vec2::new(c - 400.0, c - 600.0),
                to: Vec2::new(c - 330.0, c - 600.0),
            },
            elevation: ElevationMode::Ground,
        }),
        sim.tick(),
    );
    sim.tick_once();
    sim.tick_once();
    let segment = sim
        .world()
        .resource::<cp_net::NetGraph>()
        .segments()
        .next()
        .map(|(id, _)| id)
        .expect("a segment");
    let spacing = 44.0;
    let mut placed = 0;
    let mut grid = sim.world_mut().resource_mut::<BuildingGrid>();
    let add = |grid: &mut BuildingGrid, def_id, col: usize, row: usize, state, ready: u64| {
        let d = db.building(def_id);
        let b = Building {
            def: def_id,
            footprint: cp_building::Footprint::Lot(Lot {
                block: cp_core::Id::new(0, 0),
                col: 0,
                w: d.lot.0,
                d: d.lot.1,
            }),
            level: d.level,
            state,
            ready_at: cp_core::Tick(ready),
            pos: Vec2::new(
                c - 200.0 + col as f32 * spacing,
                c - 130.0 + row as f32 * spacing,
            ),
            rot: 0.0,
            access: Some(AccessPoint { segment, t: 0.5 }),
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        };
        grid.insert(b);
    };
    let zones = [ResLow, ResHigh, ComLow, ComHigh, Industrial, Office];
    for (row, zone) in zones.into_iter().enumerate() {
        let mut defs: Vec<_> = db
            .buildings()
            .filter(|(_, d)| d.zone == zone && d.service.is_none())
            .map(|(i, d)| (d.level, i))
            .collect();
        defs.sort();
        for (col, (_, id)) in defs.into_iter().enumerate() {
            add(&mut grid, id, col, row, BuildingState::Active, 0);
            placed += 1;
        }
    }
    // States: a 4-floor-ish office and a house, at every construction stage, abandoned, burned.
    let row = zones.len();
    for (k, zone) in [ResHigh, Office].into_iter().enumerate() {
        let id = db
            .buildings()
            .find(|(_, d)| d.zone == zone && d.level == 3 && d.service.is_none())
            .map(|(i, _)| i)
            .expect("def");
        let now = sim.tick().0;
        let day = cp_core::time::TICKS_PER_DAY;
        let mut grid = sim.world_mut().resource_mut::<BuildingGrid>();
        for stage in 0..4u64 {
            // `ready_at` such that progress lands in the middle of `stage`.
            let left = day - (stage * day / 4 + day / 8);
            add(
                &mut grid,
                id,
                stage as usize,
                row + k,
                BuildingState::UnderConstruction,
                now + left,
            );
        }
        add(&mut grid, id, 4, row + k, BuildingState::Abandoned, 0);
        add(&mut grid, id, 5, row + k, BuildingState::Burned, 0);
        add(&mut grid, id, 6, row + k, BuildingState::Active, 0);
        placed += 7;
    }
    placed
}
