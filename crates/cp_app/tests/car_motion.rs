//! BUG-104: cars moved in jerks. A vehicle's drawn centre was half its length behind the front
//! bumper on the piece the front was on (lane or junction connection), clamped to that piece's
//! start: at every junction it stood still for half a length, then jumped ahead (a truck by
//! 6 m). The centre now follows the path back through the connection and the lane before it.
//! And a frame listed its agents in arena order while the agent key put the generation in its
//! high bits: once a slot was reused the frame was not ascending by key, the renderer's merge of
//! two frames missed those cars, and they were drawn at their tick poses, 20 steps a second.
//!
//! On the synthetic traffic grid: per tick, each car's drawn centre moves about as far as it
//! drove; per frame, the app's stepping and the renderer's interpolation draw it at an even
//! pace at ×1, ×2 and ×3, and it stands still while paused (no step back on pausing).

use std::path::Path;
use std::time::Duration;

use cp_core::time::MOVE_SECONDS_PER_TICK;
use cp_core::{SimLimits, SimSpeed};
use cp_sim::{AgentFrame, SimRunner};
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn traffic() -> SimRunner {
    let t = Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap();
    let mut sim = cp_app::build_sim(
        t,
        SimLimits::desktop(),
        10.0,
        false,
        Some("traffic:3:150:0"),
    );
    sim.world_mut().insert_resource(SimSpeed::X1);
    for _ in 0..300 {
        cp_app::scene::top_up_traffic(&mut sim);
        sim.tick_once();
    }
    sim
}

fn frame(sim: &SimRunner) -> std::sync::Arc<AgentFrame> {
    sim.view().agents.cur.clone()
}

#[test]
fn a_cars_centre_moves_as_far_as_it_drove_each_tick() {
    let mut sim = traffic();
    let mut prev = frame(&sim);
    let (mut steps, mut freeze, mut jump) = (0, 0, 0);
    for _ in 0..400 {
        cp_app::scene::top_up_traffic(&mut sim);
        sim.tick_once();
        let cur = frame(&sim);
        // Clients match two frames by merging them on the key.
        assert!(cur.vehicles.windows(2).all(|w| w[0].key < w[1].key));
        assert!(cur.pedestrians.windows(2).all(|w| w[0].key < w[1].key));
        for c in &cur.vehicles {
            let Ok(i) = prev.vehicles.binary_search_by_key(&c.key, |p| p.key) else {
                continue;
            };
            let p = &prev.vehicles[i];
            let drove = (p.speed + c.speed) * 0.5 * MOVE_SECONDS_PER_TICK;
            let moved = p.pos.distance(c.pos);
            steps += 1;
            freeze += usize::from(drove > 0.3 && moved < 0.3 * drove);
            jump += usize::from(moved > drove + 1.0);
        }
        prev = cur;
    }
    assert!(steps > 20_000, "too little traffic: {steps}");
    // Before the fix: 413 standstills and 255 jumps in 167 000 steps (traffic:4:300).
    assert!(
        (freeze + jump) * 2_000 < steps,
        "{freeze} standstills and {jump} jumps in {steps} car-ticks"
    );
}

type Drawn = std::collections::BTreeMap<u64, bevy_math::Vec3>;

/// The frame loop of the app at 60 frames/s: step the sim by the frame time at its speed
/// (`step_sim`), publish a view when ticks ran, draw at `blend(held_fraction)`.
struct Frames {
    view: cp_sim::SimView,
    held: Option<f32>,
    last: f32,
}

impl Frames {
    fn new(sim: &SimRunner) -> Self {
        Self {
            view: sim.view(),
            held: None,
            last: 0.0,
        }
    }

    fn draw(&mut self, sim: &mut SimRunner, frames: usize) -> Vec<Drawn> {
        (0..frames)
            .map(|_| {
                let scale = cp_app::game_speed_scale(sim.speed());
                let ticks = sim.step(Duration::from_secs_f64(scale / 60.0));
                if ticks > 0 {
                    cp_app::scene::top_up_traffic(sim);
                    self.view = sim.view();
                }
                self.last = cp_app::held_fraction(
                    &mut self.held,
                    self.last,
                    sim.tick_fraction(),
                    ticks,
                    sim.speed(),
                );
                let snap = &self.view.agents;
                cp_render::agent::vehicle_positions(
                    &snap.prev.vehicles,
                    &snap.cur.vehicles,
                    snap.blend(self.last),
                )
                .into_iter()
                .collect()
            })
            .collect()
    }
}

#[test]
fn cars_are_drawn_at_an_even_pace_at_every_speed_and_stand_still_when_paused() {
    let mut sim = traffic();
    let mut draw = Frames::new(&sim);
    for speed in [SimSpeed::X1, SimSpeed::X2, SimSpeed::X3] {
        sim.world_mut().insert_resource(speed);
        let frames = draw.draw(&mut sim, 180);
        let (mut pairs, mut jerks) = (0, 0);
        for w in frames.windows(3) {
            for (k, b) in &w[1] {
                let (Some(a), Some(c)) = (w[0].get(k), w[2].get(k)) else {
                    continue;
                };
                let (d1, d2) = (a.distance(*b), b.distance(*c));
                pairs += 1;
                // Speed changes by braking/accelerating are small per frame; a stop/go or a
                // jump of half a car is not.
                jerks += usize::from((d1 - d2).abs() > 0.25 + 0.5 * d1.max(d2));
            }
        }
        assert!(pairs > 10_000, "{speed:?}: too little traffic: {pairs}");
        // At ×3 a frame is 2.7 ticks of game time: a car pulling away or braking changes its
        // step from frame to frame by more. Before the fix ×1 had 4 211 in 23 545.
        let most = if speed == SimSpeed::X3 { 50 } else { 500 };
        assert!(
            jerks * most < pairs,
            "{speed:?}: {jerks} jerks in {pairs} frame pairs"
        );
    }
    // Pause mid-tick: every car stays exactly where the last running frame drew it.
    sim.world_mut().insert_resource(SimSpeed::X1);
    let mut running = draw.draw(&mut sim, 31);
    let before = running.pop().unwrap();
    sim.world_mut().insert_resource(SimSpeed::Pause);
    for drawn in draw.draw(&mut sim, 30) {
        for (k, p) in &drawn {
            if let Some(q) = before.get(k) {
                assert!(
                    p.distance(*q) < 1e-4,
                    "car {k} moved while paused: {q} -> {p}"
                );
            }
        }
    }
}
