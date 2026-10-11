//! Format 9: utility networks. The committed v8 fixture migrates to "no utilities"; v7
//! saves round-trip pipes and power lines; `BuildLink` / `RemoveLink` run through the sim.

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::{Vec2, Vec3};
use cp_building::{BuildingGrid, BuildingState};
use cp_content::{ContentDb, UtilityKind, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::{CommandError, CommandOutcome, Tick};
use cp_net::{Elevation, NetGraph};
use cp_save::migrate::{SaveDataV8, encode_v8};
use cp_save::*;
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::TileOwnership;
use cp_utility::{UtilityCommand, UtilityGraph, UtilityNet};
use cp_zone::{ZoneCellRef, ZoneGrid};

const HASH: u64 = 7;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/saves")
            .join(name),
    )
    .unwrap()
}

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

fn runner(seed: u64) -> SimRunner {
    let mut sim = SimRunner::new(seed);
    sim.set_content(content());
    // Everything near (3000, 3000) and (5000, 5000) is owned: tiles (1,1) and (2,2).
    let mut t = TileOwnership::none();
    t.set(1, 1, true);
    t.set(2, 2, true);
    sim.world_mut().insert_resource(t);
    sim
}

fn run(sim: &mut SimRunner, cmds: Vec<UtilityCommand>) -> Vec<CommandOutcome> {
    let start = sim.tick().0;
    let n = cmds.len() as u64;
    for (i, c) in cmds.into_iter().enumerate() {
        sim.push_command(SimCommand::Utility(c), Tick(start + i as u64));
    }
    let mut out = Vec::new();
    for _ in 0..n + 1 {
        sim.tick_once();
        out.extend(
            sim.world_mut()
                .resource_mut::<Messages<CommandOutcome>>()
                .drain(),
        );
    }
    out
}

fn build(kind: UtilityKind, pts: &[(f32, f32)]) -> UtilityCommand {
    UtilityCommand::BuildLink {
        kind,
        path: pts.iter().map(|&(x, z)| Vec2::new(x, z)).collect(),
    }
}

fn graph(sim: &SimRunner) -> UtilityGraph {
    sim.world().resource::<UtilityGraph>().clone()
}

#[test]
fn v8_fixture_migrates_to_no_utilities_and_the_frozen_writer_reproduces_it() {
    let bytes = fixture("v8-basic.sav");
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 8);
    let l = load(&bytes, h.content_hash).unwrap();
    assert!(l.warnings.is_empty());
    let d = &l.data;
    assert_eq!(d.utilities, UtilityGraph::default());
    assert!(d.net.segment_count() >= 1 && d.zones.block_count() >= 1);

    let v8 = SaveDataV8 {
        tick: d.tick,
        rng: d.rng.clone(),
        speed: d.speed,
        queue: d.queue.clone(),
        log: d.log.clone(),
        tiles: d.tiles,
        net: d.net.clone(),
        zones: d.zones.clone(),
        buildings: cp_save::migrate::BuildingGridV12::from(&d.buildings),
        citizens: d.citizens.clone(),
        paths: d.paths.clone(),
    };
    assert_eq!(
        encode_v8(&v8, h.content_hash, &h.map_id, h.map_hash).unwrap(),
        bytes
    );

    // Re-saving writes the current version and is a fixed point.
    let v9 = save_with_map(d, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(read_header(&v9).unwrap().0.format_version, FORMAT_VERSION);
    assert_eq!(load(&v9, h.content_hash).unwrap().data, *d);
}

#[test]
fn older_fixtures_reach_the_current_version_with_no_utilities() {
    for name in ["v6-basic.sav", "v7-basic.sav"] {
        let bytes = fixture(name);
        let h = read_header(&bytes).unwrap().0;
        let l = load(&bytes, h.content_hash).unwrap();
        assert_eq!(l.data.utilities, UtilityGraph::default(), "{name}");
    }
}

#[test]
fn commands_build_remove_and_report_costs() {
    let mut sim = runner(1);
    let o = run(
        &mut sim,
        vec![
            build(UtilityKind::Water, &[(5000.0, 5000.0), (5120.0, 5000.0)]),
            build(UtilityKind::Power, &[(5000.0, 5050.0), (5150.0, 5050.0)]),
        ],
    );
    assert!(o.iter().all(|o| o.result.is_ok()), "{o:?}");
    let costs: Vec<i64> = o.iter().map(|o| o.result.clone().unwrap().0).collect();
    // 120 m of pipe (2 links) and 150 m of power line (1 link), per-metre cents from content.
    let db = content();
    let per_m = |k| db.utility_for_kind(k).unwrap().1.cost_per_m;
    assert_eq!(
        costs[0],
        cp_net::segment_cost(60.0, per_m(UtilityKind::Water)).0 * 2
    );
    assert_eq!(
        costs[1],
        cp_net::segment_cost(150.0, per_m(UtilityKind::Power)).0
    );
    let g = graph(&sim);
    assert_eq!(g.components(UtilityNet::Pipe).count, 1);
    assert_eq!(g.components(UtilityNet::Power).count, 1);

    // RemoveLink: take out the first pipe link; the pipe network splits in two.
    let first_pipe = g
        .links()
        .find(|(_, l)| l.net == UtilityNet::Pipe)
        .map(|(id, _)| id)
        .unwrap();
    let o = run(
        &mut sim,
        vec![UtilityCommand::RemoveLink { link: first_pipe }],
    );
    assert!(o[0].result.is_ok());
    let g2 = graph(&sim);
    assert_eq!(g2.link_count(), g.link_count() - 1);
    // Removing it again is rejected and changes nothing.
    let o = run(
        &mut sim,
        vec![UtilityCommand::RemoveLink { link: first_pipe }],
    );
    assert!(matches!(
        o[0].result,
        Err(CommandError::Rejected {
            domain: "utility",
            ..
        })
    ));
    assert_eq!(graph(&sim), g2);
}

#[test]
fn rejected_commands_leave_the_state_untouched() {
    let mut sim = runner(2);
    run(
        &mut sim,
        vec![build(
            UtilityKind::Water,
            &[(5000.0, 5000.0), (5100.0, 5000.0)],
        )],
    );
    let before = graph(&sim);
    let bad = [
        build(UtilityKind::Water, &[(5000.0, 5100.0)]), // one point
        build(UtilityKind::Water, &[(5000.0, 5100.0), (5001.0, 5100.0)]), // too short
        build(UtilityKind::Water, &[(100.0, 100.0), (180.0, 100.0)]), // unowned tile
        build(UtilityKind::Power, &[(5000.0, 5100.0), (5004.0, 5100.0)]), // power min 8 m
    ];
    let o = run(&mut sim, bad.to_vec());
    assert_eq!(o.len(), 4);
    for o in &o {
        assert!(
            matches!(
                o.result,
                Err(CommandError::Rejected {
                    domain: "utility",
                    ..
                })
            ),
            "{o:?}"
        );
    }
    assert_eq!(graph(&sim), before);
}

fn busy_city() -> SimRunner {
    let db = content();
    let road = db.road_id("basic_2l").unwrap();
    let def = db.road(road).clone();
    let mut sim = runner(3);
    {
        let mut g = sim.world_mut().resource_mut::<NetGraph>();
        for (a, b) in [
            ((3000.0, 3000.0), (3090.0, 3000.0)),
            ((3045.0, 2940.0), (3045.0, 3060.0)),
        ] {
            g.add_segment(
                &def,
                road,
                CubicBezier3::line(Vec3::new(a.0, 0.0, a.1), Vec3::new(b.0, 0.0, b.1)),
                Elevation::Ground,
            )
            .unwrap();
        }
    }
    sim.tick_once();
    {
        let mut grid = sim.world_mut().resource_mut::<ZoneGrid>();
        let ids: Vec<_> = grid.blocks().map(|(i, b)| (i, b.cell_count())).collect();
        for (id, n) in ids {
            for cell in 0..n {
                grid.set_zone(
                    ZoneCellRef {
                        block: id,
                        cell: cell as u16,
                    },
                    ZoneType::ComLow,
                );
            }
        }
    }
    while sim
        .world()
        .resource::<BuildingGrid>()
        .iter()
        .filter(|(_, b)| b.state == BuildingState::Active)
        .count()
        < 2
    {
        sim.tick_once();
    }
    sim
}

#[test]
fn power_lines_avoid_buildings_but_pipes_may_pass_under_them() {
    let mut sim = busy_city();
    let pos = sim
        .world()
        .resource::<BuildingGrid>()
        .iter()
        .next()
        .map(|(_, b)| b.pos)
        .unwrap();
    let through = [(pos.x - 25.0, pos.y), (pos.x + 25.0, pos.y)];
    let o = run(&mut sim, vec![build(UtilityKind::Power, &through)]);
    assert!(
        matches!(
            o[0].result,
            Err(CommandError::Rejected {
                domain: "utility",
                ..
            })
        ),
        "{o:?}"
    );
    assert_eq!(graph(&sim).link_count(), 0);
    let o = run(&mut sim, vec![build(UtilityKind::Water, &through)]);
    assert!(o[0].result.is_ok(), "{o:?}");
    assert_eq!(graph(&sim).link_count(), 1);
}

#[test]
fn utilities_survive_save_load_and_are_checksummed() {
    let mut a = runner(4);
    let o = run(
        &mut a,
        vec![
            build(
                UtilityKind::Water,
                &[(5000.0, 5000.0), (5100.0, 5000.0), (5100.0, 5090.0)],
            ),
            build(UtilityKind::Power, &[(5000.0, 5200.0), (5180.0, 5200.0)]),
        ],
    );
    assert!(o.iter().all(|o| o.result.is_ok()), "{o:?}");
    let sum_with = a.checksum();
    let bytes = save_runner(&a, HASH, "m").unwrap();
    let loaded = load(&bytes, HASH).unwrap();
    assert_eq!(
        save(&loaded.data, HASH, "m").unwrap(),
        bytes,
        "byte-stable re-save"
    );
    assert_eq!(loaded.data.utilities, graph(&a));

    let mut b = runner(99);
    load_into_runner(&mut b, &bytes, HASH).unwrap();
    assert_eq!(graph(&b), graph(&a));
    assert_eq!(b.checksum(), a.checksum());
    // Both keep behaving identically afterwards.
    let more = vec![build(
        UtilityKind::Water,
        &[(5100.0, 5090.0), (5200.0, 5090.0)],
    )];
    run(&mut a, more.clone());
    run(&mut b, more);
    assert_eq!(graph(&a), graph(&b));
    assert_eq!(a.checksum(), b.checksum());
    assert_ne!(a.checksum(), sum_with);

    // The utility state is part of the checksum on its own.
    let mut c = runner(4);
    let mut d = runner(4);
    assert_eq!(c.checksum(), d.checksum());
    run(
        &mut c,
        vec![build(
            UtilityKind::Water,
            &[(5000.0, 5000.0), (5060.0, 5000.0)],
        )],
    );
    d.tick_once();
    d.tick_once();
    assert_ne!(c.checksum(), d.checksum());
}
