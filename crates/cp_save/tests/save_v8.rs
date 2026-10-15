//! Format 8: pathfinding requests in flight survive a save, so load + N ticks equals the
//! continuous run (same outcomes at `t + 8`, same checksum), including requests that go
//! stale because the network changes.

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec3;
use cp_content::ContentDb;
use cp_core::Tick;
use cp_core::curve::CubicBezier3;
use cp_net::{Elevation, NetGraph, SegmentId};
use cp_save::migrate::{SaveDataV7, encode_v7};
use cp_save::*;
use cp_sim::SimRunner;
use cp_traffic::{
    Access, Mode, PATH_DELAY_TICKS, PathNetwork, PathOutcome, PathRequest, PathResult, PathResults,
    PathService, Side,
};

const HASH: u64 = 0x77;

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/saves")
            .join(name),
    )
    .unwrap()
}

fn runner(seed: u64) -> SimRunner {
    let mut sim = SimRunner::new(seed);
    sim.set_content(content());
    sim
}

fn add(sim: &mut SimRunner, a: (f32, f32), b: (f32, f32)) -> SegmentId {
    let db = content();
    let road = db.road_id("basic_2l").unwrap();
    let def = db.road(road).clone();
    sim.world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            &def,
            road,
            CubicBezier3::line(Vec3::new(a.0, 0.0, a.1), Vec3::new(b.0, 0.0, b.1)),
            Elevation::Ground,
        )
        .unwrap()
}

/// A small grid; returns its segments.
fn grid(sim: &mut SimRunner) -> Vec<SegmentId> {
    let mut segs = Vec::new();
    for i in 0..4 {
        for j in 0..4 {
            let p = (6000.0 + i as f32 * 100.0, 6000.0 + j as f32 * 100.0);
            if i < 3 {
                segs.push(add(sim, p, (p.0 + 100.0, p.1)));
            }
            if j < 3 {
                segs.push(add(sim, p, (p.0, p.1 + 100.0)));
            }
        }
    }
    segs
}

fn submit(sim: &mut SimRunner, segs: &[SegmentId], count: usize) {
    let now = sim.tick();
    for i in 0..count {
        let a = segs[(i * 7) % segs.len()];
        let b = segs[(i * 13 + 5) % segs.len()];
        let req = PathRequest {
            mode: if i % 4 == 0 {
                Mode::Pedestrian
            } else {
                Mode::Car
            },
            from: Access {
                segment: a,
                s: 10.0 + i as f32,
                side: Side::Right,
            },
            to: Access {
                segment: b,
                s: 60.0,
                side: if i % 2 == 0 { Side::Left } else { Side::Right },
            },
        };
        sim.world_mut().resource_scope(
            |w, mut svc: bevy_ecs::change_detection::Mut<PathService>| {
                svc.submit(w.resource::<PathNetwork>(), now, req);
            },
        );
    }
}

/// Ticks `n` times, collecting `(tick applied, outcomes)` for every tick that delivered.
fn run(sim: &mut SimRunner, n: u64, out: &mut Vec<(u64, Vec<PathOutcome>)>) {
    for _ in 0..n {
        let t = sim.tick().0;
        sim.tick_once();
        let got = sim.world().resource::<PathResults>().0.clone();
        if !got.is_empty() {
            out.push((t, got));
        }
    }
}

/// Continuous run vs a run saved at `t + 3` and loaded into a fresh runner; both then run to
/// `t + 10`. `change` is applied right after submission (before the save) or after the load.
fn compare(change_before_save: bool, change_after_save: bool) -> Vec<(u64, Vec<PathOutcome>)> {
    let mut a = runner(5);
    let segs = grid(&mut a);
    a.tick_once(); // build junctions and the snapshot
    let t = a.tick().0;
    submit(&mut a, &segs, 24);
    let mut out_a = Vec::new();

    if change_before_save {
        a.tick_once();
        add(&mut a, (9000.0, 9000.0), (9100.0, 9000.0));
        run(&mut a, 2, &mut out_a); // up to t + 3
    } else {
        run(&mut a, 3, &mut out_a);
    }
    assert_eq!(a.tick(), Tick(t + 3));

    let bytes = save_runner(&a, HASH, "m").unwrap();
    let mut b = runner(99);
    load_into_runner(&mut b, &bytes, HASH).unwrap();
    assert_eq!(b.checksum(), a.checksum(), "state right after load");
    let mut out_b = out_a.clone();

    if change_after_save {
        for sim in [&mut a, &mut b] {
            add(sim, (9000.0, 9200.0), (9100.0, 9200.0));
        }
    }
    run(&mut a, 7, &mut out_a);
    run(&mut b, 7, &mut out_b);
    assert_eq!(a.tick(), Tick(t + 10));
    assert_eq!(out_a, out_b, "PathResults of the loaded run");
    assert_eq!(b.checksum(), a.checksum());
    // Everything was delivered exactly at t + 8.
    let delivered: Vec<u64> = out_a.iter().map(|(tick, _)| *tick).collect();
    assert_eq!(delivered, vec![t + PATH_DELAY_TICKS]);
    out_a
}

#[test]
fn pending_requests_continue_identically_after_a_load() {
    let out = compare(false, false);
    let outcomes = &out[0].1;
    assert_eq!(outcomes.len(), 24);
    assert!(
        outcomes
            .iter()
            .any(|o| matches!(o.result, PathResult::Car(Ok(_))))
    );
    assert!(
        outcomes
            .iter()
            .any(|o| matches!(o.result, PathResult::Pedestrian(Ok(_))))
    );
    assert!(
        outcomes
            .iter()
            .all(|o| !matches!(o.result, PathResult::Stale))
    );
}

#[test]
fn a_network_change_after_the_load_makes_the_restored_requests_stale_like_live() {
    // The edit lands between the load and the first tick, so the rebuild the load forces
    // already sees a changed net (it is compared with the restored one, not assumed equal).
    let out = compare(false, true);
    assert!(
        out[0]
            .1
            .iter()
            .all(|o| matches!(o.result, PathResult::Stale))
    );
}

#[test]
fn a_network_change_before_the_save_is_remembered_across_the_load() {
    let out = compare(true, false);
    assert!(
        out[0]
            .1
            .iter()
            .all(|o| matches!(o.result, PathResult::Stale))
    );
}

#[test]
fn pending_state_is_in_the_checksum_and_round_trips_byte_identically() {
    let mut a = runner(5);
    let segs = grid(&mut a);
    a.tick_once();
    let before = a.checksum();
    submit(&mut a, &segs, 5);
    assert_ne!(
        a.checksum(),
        before,
        "a pending request changes the checksum"
    );
    let bytes = save_runner(&a, HASH, "m").unwrap();
    let loaded = load(&bytes, HASH).unwrap();
    assert_eq!(loaded.data.paths.pending.len(), 5);
    assert_eq!(loaded.data.paths.next_id, 5);
    assert_eq!(save(&loaded.data, HASH, "m").unwrap(), bytes);
}

#[test]
fn v7_fixture_migrates_with_no_requests_and_the_frozen_writer_reproduces_it() {
    let bytes = fixture("v7-basic.sav");
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!((h.format_version, h.tick), (7, 3));
    let l = load(&bytes, h.content_hash).unwrap();
    assert!(l.warnings.is_empty());
    assert_eq!(l.data.net.segment_count(), 3);
    assert!(l.data.paths.pending.is_empty());
    assert_eq!(l.data.paths.next_id, 0);

    let d = &l.data;
    let v7 = SaveDataV7 {
        tick: d.tick,
        rng: d.rng.clone(),
        speed: d.speed,
        queue: d.queue.clone(),
        log: d.log.clone(),
        tiles: d.tiles,
        net: d.net.clone(),
        zones: d.zones.clone(),
        buildings: cp_save::migrate::BuildingGridV12::from(&d.buildings),
        citizens: (&d.citizens).into(),
    };
    assert_eq!(
        encode_v7(&v7, h.content_hash, &h.map_id, h.map_hash).unwrap(),
        bytes
    );

    // Re-saving writes v8 and round-trips; the old save runs and serves requests.
    let v8 = save(d, h.content_hash, &h.map_id).unwrap();
    assert_eq!(read_header(&v8).unwrap().0.format_version, FORMAT_VERSION);
    assert_eq!(load(&v8, h.content_hash).unwrap().data, *d);
    let mut sim = runner(1);
    l.data.clone().restore(sim.world_mut());
    sim.tick_once();
    assert!(
        sim.world()
            .resource::<PathNetwork>()
            .graph()
            .car_lane_count()
            > 0
    );
}
