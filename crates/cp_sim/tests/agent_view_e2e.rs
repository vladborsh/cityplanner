//! T-036: `SimView::agents` — poses of moving cars and pedestrians for the renderer.

use std::sync::Arc;

use bevy_math::Vec3;
use cp_content::ContentDb;
use cp_core::SimRng;
use cp_core::curve::CubicBezier3;
use cp_net::{Elevation, NetGraph};
use cp_sim::SimRunner;
use cp_traffic::{Agents, PathNetwork, demo};

const ORIGIN: f32 = 2000.0;
const SIDE: f32 = 270.0;

/// A 4×4-line lattice of 90 m streets with demo traffic injected.
fn busy_town() -> SimRunner {
    let content = Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    );
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    let mut sim = SimRunner::new(3);
    sim.set_content(Arc::clone(&content));
    {
        let mut g = sim.world_mut().resource_mut::<NetGraph>();
        let p = |x: f32, z: f32| Vec3::new(ORIGIN + x, 0.0, ORIGIN + z);
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
            let added = demo::top_up(&mut agents, &graph, net, 60, 80, rng, 0);
            assert!(added >= 100, "routes found: {added}");
        });
    sim
}

#[test]
fn view_carries_interpolatable_poses_of_moving_agents() {
    let mut sim = busy_town();
    // Routes on this small lattice are short: look while most are still under way.
    for _ in 0..6 {
        sim.tick_once();
    }
    let a = sim.view().agents;
    assert!(Arc::ptr_eq(&a, &sim.view().agents), "cached within a tick");
    sim.tick_once();
    let b = sim.view().agents;
    assert!(
        Arc::ptr_eq(&b.prev, &a.cur),
        "prev is the last published frame"
    );
    assert!(b.tick.0 > a.tick.0);
    assert!(
        b.cur.vehicles.len() > 10,
        "{} cars on the road",
        b.cur.vehicles.len()
    );
    assert!(
        b.cur.pedestrians.len() > 10,
        "{} walkers",
        b.cur.pedestrians.len()
    );

    let sorted = |keys: Vec<u64>| keys.windows(2).all(|w| w[0] < w[1]);
    assert!(sorted(b.cur.vehicles.iter().map(|v| v.key).collect()));
    assert!(sorted(b.cur.pedestrians.iter().map(|p| p.key).collect()));

    // Everybody is on the lattice, and one tick moves an agent by a small, bounded step.
    let on_map = |x: f32, z: f32| {
        (ORIGIN - 10.0..=ORIGIN + SIDE + 10.0).contains(&x)
            && (ORIGIN - 10.0..=ORIGIN + SIDE + 10.0).contains(&z)
    };
    let mut moved = 0;
    for v in &b.cur.vehicles {
        assert!(on_map(v.pos.x, v.pos.z), "car at {:?}", v.pos);
        assert!(v.yaw.is_finite() && v.pos.y.abs() < 5.0);
        if let Some(p) = b.prev.vehicles.iter().find(|p| p.key == v.key) {
            let d = p.pos.distance(v.pos);
            assert!(d < 25.0, "car jumped {d} m in one tick");
            moved += usize::from(d > 0.01);
        }
    }
    assert!(moved > 3, "cars are driving: {moved}");
    let mut walked = 0;
    for p in &b.cur.pedestrians {
        assert!(on_map(p.pos.x, p.pos.y), "walker at {:?}", p.pos);
        if let Some(q) = b.prev.pedestrians.iter().find(|q| q.key == p.key) {
            let d = q.pos.distance(p.pos);
            assert!(d < 15.0, "walker jumped {d} m in one tick");
            walked += usize::from(d > 0.01);
        }
    }
    assert!(walked > 3, "pedestrians are walking: {walked}");
}

#[test]
fn tick_fraction_runs_from_zero_towards_one_between_ticks() {
    let mut sim = busy_town();
    assert_eq!(sim.tick_fraction(), 0.0);
    // Half a tick of real time at x1 (50 ms per tick) leaves half the tick owed.
    assert_eq!(sim.step(std::time::Duration::from_millis(25)), 0);
    let f = sim.tick_fraction();
    assert!((f - 0.5).abs() < 0.01, "{f}");
    assert_eq!(sim.step(std::time::Duration::from_millis(30)), 1);
    assert!(sim.tick_fraction() < 0.2);
}

/// The §14 budget scene (16k cars, 20k pedestrians) builds its pose snapshot in a few ms.
/// Prints the cost; asserts only that all of them are there (timing varies by machine).
#[test]
fn budget_scene_snapshot_holds_every_agent() {
    let mut sim = busy_town();
    sim.world_mut()
        .resource_scope(|w, mut agents: bevy_ecs::change_detection::Mut<Agents>| {
            let net = w.resource::<NetGraph>();
            let graph = Arc::clone(w.resource::<PathNetwork>().graph());
            let rng = w.resource::<SimRng>();
            for batch in 0..50 {
                demo::top_up(&mut agents, &graph, net, 16_000, 20_000, rng, batch);
            }
            assert_eq!(agents.pedestrian_count(), 20_000);
        });
    for _ in 0..3 {
        sim.tick_once();
    }
    let snap = sim.view().agents;
    assert!(snap.cur.pedestrians.len() > 19_000);
    assert!(!snap.cur.vehicles.is_empty());
}

#[test]
fn demo_traffic_is_deterministic_per_sim_seed() {
    let poses = || {
        let mut sim = busy_town();
        for _ in 0..6 {
            sim.tick_once();
        }
        let f = sim.view().agents;
        (f.cur.vehicles.clone(), f.cur.pedestrians.clone())
    };
    assert_eq!(poses(), poses());
}

/// BUG-064: drawn the way the app draws them — a view after every frame that ran ticks,
/// `lerp(prev, cur, blend(tick_fraction))` — pedestrians move smoothly at x3 with uneven frame
/// times: no frame moves one faster than its walking speed (a jump after frames that stood
/// still), and none drops out of the frame while it waits or changes legs.
#[test]
fn pedestrians_are_drawn_smoothly_over_uneven_frames() {
    use bevy_math::Vec2;
    use std::collections::BTreeMap;
    use std::time::Duration;

    let mut sim = busy_town();
    let now = sim.tick();
    sim.push_command(
        cp_sim::SimCommand::SetSpeed {
            speed: cp_core::SimSpeed::X3,
        },
        now,
    );
    let frames_ms = [16u64, 33, 8, 50, 21, 70, 16, 16, 40];
    let mut view = sim.view();
    // key -> (frame seen last, drawn time, drawn position)
    let mut last: BTreeMap<u64, (usize, f32, Vec2)> = BTreeMap::new();
    let (mut checked, mut vanished, mut moved) = (0, 0, 0);
    for frame in 0..240 {
        let dt = Duration::from_millis(frames_ms[frame % frames_ms.len()]);
        if sim.step(dt) > 0 {
            view = sim.view();
        }
        let a = &view.agents;
        let f = sim.tick_fraction();
        let blend = a.blend(f);
        // The time the frame shows, in ticks: one tick behind the sim.
        let shown = a.tick.0 as f32 - 1.0 + f;
        let prev: BTreeMap<u64, Vec2> = a.prev.pedestrians.iter().map(|p| (p.key, p.pos)).collect();
        for p in &a.cur.pedestrians {
            let pos = prev.get(&p.key).map_or(p.pos, |q| q.lerp(p.pos, blend));
            if let Some((seen, t, q)) = last.get(&p.key).copied() {
                if seen + 1 != frame {
                    vanished += 1;
                }
                let dtime = shown - t;
                assert!(dtime >= -1e-4, "drawn time runs backwards: {dtime}");
                let d = pos.distance(q);
                if d > 1e-3 {
                    moved += 1;
                }
                assert!(
                    d <= cp_traffic::PED_STEP * dtime.max(0.0) * 1.02 + 1e-3,
                    "frame {frame}: pedestrian {} jumped {d} m in {dtime} ticks",
                    p.key
                );
                checked += 1;
            }
            last.insert(p.key, (frame, shown, pos));
        }
    }
    assert!(
        checked > 1000 && moved > 500,
        "{checked} checked, {moved} moved"
    );
    assert_eq!(vanished, 0, "pedestrians left the frame and came back");
}
