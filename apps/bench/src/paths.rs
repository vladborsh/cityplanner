//! `cp_bench paths`: pathfinding throughput on an N×N street grid (T-033).
//!
//! Builds the grid, takes the path snapshot through a real `SimRunner` tick, then submits
//! `--requests` car/pedestrian requests (random access points, seeded) in one tick and times
//! the whole submit-and-deliver cycle at each thread count. It also checks that every thread
//! count produced identical outcomes. An app, so it may read the wall clock.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use bevy_math::Vec3;
use bevy_tasks::TaskPoolBuilder;
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_core::{SimRng, Tick};
use cp_net::{Elevation, NetGraph, SegmentId};
use cp_sim::SimRunner;
use cp_traffic::{
    Access, Mode, PATH_DELAY_TICKS, PathNetwork, PathRequest, PathResults, PathService, Side,
};

struct Opts {
    grid: usize,
    requests: usize,
    threads: Vec<usize>,
    seed: u64,
    content: String,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        grid: 30,
        requests: 20_000,
        threads: vec![1, 8],
        seed: 1,
        content: "assets/content".into(),
    };
    let mut it = args.iter();
    while let Some(f) = it.next() {
        let v = it.next().ok_or(format!("{f} needs a value"))?;
        match f.as_str() {
            "--grid" => o.grid = v.parse().map_err(|e| format!("--grid: {e}"))?,
            "--requests" => o.requests = v.parse().map_err(|e| format!("--requests: {e}"))?,
            "--seed" => o.seed = v.parse().map_err(|e| format!("--seed: {e}"))?,
            "--content" => o.content = v.clone(),
            "--threads" => {
                o.threads = v
                    .split(',')
                    .map(|t| t.parse().map_err(|e| format!("--threads: {e}")))
                    .collect::<Result<_, _>>()?
            }
            _ => return Err(format!("unknown argument '{f}'")),
        }
    }
    if o.grid < 2 || o.requests == 0 || o.threads.is_empty() || o.threads.contains(&0) {
        return Err("--grid >= 2, --requests > 0, --threads >= 1".into());
    }
    Ok(o)
}

fn build(o: &Opts) -> Result<SimRunner, String> {
    let db = Arc::new(ContentDb::load_dir(&o.content).map_err(|e| format!("--content: {e:?}"))?);
    let road = db.road_id("basic_2l").ok_or("content has no basic_2l")?;
    let def = db.road(road).clone();
    let mut sim = SimRunner::new(o.seed);
    sim.set_content(db);
    {
        let mut net = sim.world_mut().resource_mut::<NetGraph>();
        let at = |i: usize, j: usize| {
            Vec3::new(1000.0 + i as f32 * 100.0, 0.0, 1000.0 + j as f32 * 100.0)
        };
        for i in 0..o.grid {
            for j in 0..o.grid {
                for (di, dj) in [(1, 0), (0, 1)] {
                    if i + di < o.grid && j + dj < o.grid {
                        net.add_segment(
                            &def,
                            road,
                            CubicBezier3::line(at(i, j), at(i + di, j + dj)),
                            Elevation::Ground,
                        )
                        .map_err(|e| format!("grid: {e}"))?;
                    }
                }
            }
        }
    }
    sim.tick_once(); // junction links + path snapshot
    Ok(sim)
}

fn requests(sim: &SimRunner, o: &Opts) -> Vec<PathRequest> {
    let segs: Vec<(SegmentId, f32)> = sim
        .world()
        .resource::<NetGraph>()
        .segments()
        .map(|(id, s)| (id, s.length))
        .collect();
    let mut rng = SimRng::seed_from_u64(o.seed);
    let mut pick = || {
        let (id, len) = segs[rng.below(segs.len() as u32) as usize];
        let side = if rng.below(2) == 0 {
            Side::Left
        } else {
            Side::Right
        };
        Access {
            segment: id,
            s: rng.next_f32() * len,
            side,
        }
    };
    (0..o.requests)
        .map(|i| PathRequest {
            mode: if i % 10 == 0 {
                Mode::Pedestrian
            } else {
                Mode::Car
            },
            from: pick(),
            to: pick(),
        })
        .collect()
}

pub fn run(args: &[String]) -> Result<ExitCode, String> {
    let o = parse(args)?;
    let mut sim = build(&o)?;
    let reqs = requests(&sim, &o);
    let net = sim.world().resource::<PathNetwork>();
    println!(
        "grid {0}x{0}: {1} car lanes, {2} car connections; {3} requests (every 10th pedestrian)",
        o.grid,
        net.graph().car_lane_count(),
        net.graph().car_edge_count(),
        o.requests
    );
    let mut reference: Option<Vec<String>> = None;
    for &threads in &o.threads {
        let pool = TaskPoolBuilder::new().num_threads(threads).build();
        sim.world_mut()
            .insert_resource(PathService::with_pool(pool));
        let now = sim.tick();
        let t0 = Instant::now();
        sim.world_mut().resource_scope(
            |w, mut svc: bevy_ecs::change_detection::Mut<PathService>| {
                let net = w.resource::<PathNetwork>();
                for r in &reqs {
                    svc.submit(net, now, *r);
                }
            },
        );
        let mut delivered = Vec::new();
        for _ in 0..=PATH_DELAY_TICKS {
            sim.tick_once();
            delivered.extend(
                sim.world()
                    .resource::<PathResults>()
                    .0
                    .iter()
                    .map(|o| format!("{:?}", o.result)),
            );
        }
        let secs = t0.elapsed().as_secs_f64();
        let ok = delivered
            .iter()
            .filter(|r| !r.contains("Err") && !r.contains("Stale"))
            .count();
        println!(
            "threads {threads:>2}: {:>9.0} requests/s  ({:.3} s for {} outcomes, {ok} found)",
            delivered.len() as f64 / secs,
            secs,
            delivered.len()
        );
        match &reference {
            None => reference = Some(delivered),
            Some(r) => {
                if *r != delivered {
                    return Err(format!(
                        "outcomes with {threads} threads differ from the first run"
                    ));
                }
            }
        }
    }
    println!("outcomes identical across thread counts");
    let _ = Tick(0);
    Ok(ExitCode::SUCCESS)
}
