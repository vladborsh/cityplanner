//! Format 17: a car carries the id of its re-route request. The committed v16 fixture migrates
//! with no car waiting for a route, and a save taken while re-route requests are in flight
//! resumes exactly like the uninterrupted run.

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec3;
use cp_content::ContentDb;
use cp_core::SimRng;
use cp_core::curve::CubicBezier3;
use cp_net::{Elevation, NetGraph};
use cp_save::migrate::{AgentsV16, SaveDataV16, encode_v16};
use cp_save::*;
use cp_sim::SimRunner;
use cp_traffic::{Agents, PathNetwork, demo};

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

#[test]
fn v16_fixture_migrates_with_no_reroutes_and_round_trips() {
    let bytes = fixture("v16-basic.sav");
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 16);
    let l = load(&bytes, h.content_hash).unwrap();
    assert!(l.warnings.is_empty());
    assert!(l.data.agents.vehicle_count() > 0, "the fixture has cars");
    assert!(
        l.data.agents.vehicles().all(|(_, v)| v.reroute.is_none()),
        "nobody is waiting for a route"
    );
    assert_eq!(l.data.agents.stats.rerouted, 0);
    assert_eq!(l.data.agents.stats.reroute_failed, 0);

    // The frozen v16 writer reproduces the committed file byte for byte.
    let v16 = SaveDataV16 {
        tick: l.data.tick,
        rng: l.data.rng.clone(),
        speed: l.data.speed,
        queue: l.data.queue.clone(),
        log: l.data.log.clone(),
        tiles: l.data.tiles,
        net: l.data.net.clone(),
        zones: l.data.zones.clone(),
        buildings: cp_save::migrate::BuildingGridV18::from(&l.data.buildings),
        citizens: (&l.data.citizens).into(),
        paths: l.data.paths.clone(),
        utilities: l.data.utilities.clone(),
        trips: l.data.trips.clone(),
        agents: AgentsV16::from(&l.data.agents),
        treasury: l.data.treasury,
        economy: (&l.data.economy).into(),
        demand: l.data.demand,
        demand_acc: l.data.demand_acc,
        land: l.data.land.clone(),
        decay: l.data.decay.clone(),
    };
    let again = encode_v16(&v16, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(again, bytes, "frozen v16 writer matches the fixture");

    // Re-saved at the current version it round-trips byte for byte.
    let v17 = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(read_header(&v17).unwrap().0.format_version, FORMAT_VERSION);
    let l2 = load(&v17, h.content_hash).unwrap();
    assert_eq!(l2.data, l.data);
    assert_eq!(
        save_with_map(&l2.data, h.content_hash, &h.map_id, h.map_hash).unwrap(),
        v17
    );
}

/// A 4×4-line lattice with demo cars, one street cut: cars with the cut ahead of them wait for
/// new routes.
fn cut_lattice(content: &Arc<ContentDb>) -> SimRunner {
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    let mut sim = SimRunner::new(3);
    sim.set_content(Arc::clone(content));
    {
        let mut g = sim.world_mut().resource_mut::<NetGraph>();
        let p = |x: f32, z: f32| Vec3::new(2000.0 + x, 0.0, 2000.0 + z);
        for line in 0..4 {
            let o = line as f32 * 90.0;
            for piece in 0..3 {
                let a = piece as f32 * 90.0;
                for (from, to) in [(p(a, o), p(a + 90.0, o)), (p(o, a), p(o, a + 90.0))] {
                    g.add_segment(&def, road, CubicBezier3::line(from, to), Elevation::Ground)
                        .unwrap();
                }
            }
        }
    }
    sim.tick_once();
    sim.tick_once();
    sim.world_mut()
        .resource_scope(|w, mut agents: bevy_ecs::change_detection::Mut<Agents>| {
            let net = w.resource::<NetGraph>();
            let graph = Arc::clone(w.resource::<PathNetwork>().graph());
            let rng = w.resource::<SimRng>();
            demo::top_up(&mut agents, &graph, net, 100, 0, rng, 0);
        });
    for _ in 0..10 {
        sim.tick_once();
    }
    sim
}

#[test]
fn a_save_while_reroutes_are_pending_resumes_like_the_uninterrupted_run() {
    let content = content();
    let hash = content.content_hash();
    let mut a = cut_lattice(&content);
    let cut = a
        .world()
        .resource::<NetGraph>()
        .segments()
        .nth(9)
        .map(|(id, _)| id)
        .unwrap();
    a.world_mut()
        .resource_mut::<NetGraph>()
        .remove_segment(cut)
        .unwrap();
    // Run until cars are waiting for their new routes (the request takes 8 ticks).
    let mut pending = 0;
    for _ in 0..4 {
        a.tick_once();
        pending = a
            .world()
            .resource::<Agents>()
            .vehicles()
            .filter(|(_, v)| v.reroute.is_some())
            .count();
        if pending > 0 {
            break;
        }
    }
    assert!(pending > 0, "cars wait for new routes");

    let bytes = save_runner(&a, hash, "none").unwrap();
    let mut b = SimRunner::new(3);
    b.set_content(Arc::clone(&content));
    load_into_runner(&mut b, &bytes, hash).unwrap();
    assert_eq!(a.checksum(), b.checksum(), "identical right after the load");
    assert_eq!(
        b.world()
            .resource::<Agents>()
            .vehicles()
            .filter(|(_, v)| v.reroute.is_some())
            .count(),
        pending,
        "the pending ids came along"
    );

    let before = a.world().resource::<Agents>().stats.rerouted;
    for tick in 0..800 {
        a.tick_once();
        b.tick_once();
        assert_eq!(
            a.checksum(),
            b.checksum(),
            "diverged {tick} ticks after the load"
        );
    }
    let st = &a.world().resource::<Agents>().stats;
    assert!(st.rerouted > before, "the pending reroutes were adopted");
    assert_eq!(
        a.world().resource::<Agents>().stats,
        b.world().resource::<Agents>().stats
    );
}
