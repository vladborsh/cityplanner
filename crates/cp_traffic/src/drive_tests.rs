//! Vehicle movement: no overlaps, signals, jams, determinism across thread counts.

use std::sync::Arc;

use bevy_tasks::{TaskPool, TaskPoolBuilder};
use cp_citizen::{CitizenId, TripId};
use cp_core::Arena;
use cp_net::{SegmentId, Signal};

use crate::astar::find_car_path;
use crate::drive::*;
use crate::graph::{Access, CongestionMap, PathGraph, Side};
use crate::service::PathNetwork;
use crate::tests::{Sim, acc, sim};

/// Ticks of `seconds` of movement (T-270: test time budgets are movement time, not ticks).
fn ticks(seconds: f32) -> u64 {
    (seconds / cp_core::time::MOVE_SECONDS_PER_TICK).ceil() as u64
}
use crate::{Vehicle, VehicleId};

struct Rig {
    sim: Sim,
    graph: Arc<PathGraph>,
    veh: Arena<Vehicle>,
    st: DriveState,
    arrived: usize,
    invalid: usize,
    stuck: usize,
    stuck_list: Vec<Vehicle>,
    /// Vehicle dimensions: the defaults unless a test installs its own.
    specs: VehicleSpecs,
}

impl Rig {
    fn new(mut s: Sim) -> Self {
        s.tick();
        let graph = Arc::clone(s.world.resource::<PathNetwork>().graph());
        let mut st = DriveState::default();
        let veh = Arena::new();
        assert!(st.rebuild(&veh, &graph, 0).is_empty());
        Self {
            sim: s,
            graph,
            veh,
            st,
            arrived: 0,
            invalid: 0,
            stuck: 0,
            stuck_list: Vec::new(),
            specs: VehicleSpecs::default(),
        }
    }

    fn add(&mut self, from: Access, to: Access) -> VehicleId {
        self.add_kind(0, from, to)
    }

    fn add_kind(&mut self, kind: u8, from: Access, to: Access) -> VehicleId {
        // A bus, tram or trolleybus keeps to its lanes (T-302).
        let class = cp_content::VehicleKind::ALL
            .get(usize::from(kind))
            .map_or(cp_content::VehicleClass::Car, |k| k.class());
        let cong = CongestionMap::default();
        let path = match class {
            cp_content::VehicleClass::Car => find_car_path(&self.graph, &cong, &from, &to),
            class => crate::astar::find_class_path(&self.graph, &cong, class, &from, &to),
        }
        .expect("a route exists");
        let s = path.start_s;
        let id = self.veh.insert(Vehicle {
            trip: TripId::new(0, 0),
            citizen: CitizenId::new(0, 0),
            path,
            kind,
            hop: 0,
            in_conn: false,
            spawned: false,
            s,
            speed: 0.0,
            stuck_timer: 0,
            reroute: None,
        });
        self.st.waiting.push(id);
        id
    }

    fn tick(&mut self, tick: u64, pool: Option<&TaskPool>, stuck_ticks: u32) {
        let f = drive_tick(
            &mut self.veh,
            &mut self.st,
            &self.graph,
            tick,
            pool,
            stuck_ticks,
            &self.specs,
        );
        self.arrived += f.arrived.len();
        self.invalid += f.invalid.len();
        self.stuck += f.stuck.len();
        self.stuck_list.extend(f.stuck);
    }

    /// Runs until no vehicle is left or `max` ticks passed; returns the ticks used.
    fn run_dry(&mut self, max: u64, pool: Option<&TaskPool>) -> u64 {
        for t in 0..max {
            self.tick(t, pool, STUCK_TICKS);
            self.check_invariants();
            if self.veh.is_empty() {
                return t + 1;
            }
        }
        max
    }

    /// No two cars overlap in a place, and no two vehicles occupy conflicting connections.
    fn check_invariants(&self) {
        let lists = self.st.lanes.iter().chain(self.st.conns.values());
        for l in lists {
            for w in l.windows(2) {
                let (a, b) = (self.veh.get(w[0]).unwrap(), self.veh.get(w[1]).unwrap());
                // The leader's length, not the follower's, separates the two front bumpers.
                let gap = self.specs.of(a).length + HARD_GAP - 1e-2;
                assert!(
                    a.s - b.s >= gap,
                    "cars overlap: {} (kind {}, in_conn {}) behind {} (kind {}, in_conn {}), hop {} / {}",
                    b.s,
                    b.kind,
                    b.in_conn,
                    a.s,
                    a.kind,
                    a.in_conn,
                    b.hop,
                    a.hop
                );
            }
        }
        for (&(fs, ts), l) in &self.st.conns {
            if l.is_empty() {
                continue;
            }
            let ex = self.graph.exits[fs as usize]
                .iter()
                .find(|e| e.to_slot == ts)
                .expect("exit");
            for c in &ex.conflicts {
                assert!(
                    self.st
                        .conns
                        .get(&(c.from_slot, c.to.index))
                        .is_none_or(Vec::is_empty),
                    "two cars in conflicting connections"
                );
            }
        }
    }

    fn snapshot(&self) -> Vec<(VehicleId, u32, u32, u16, bool)> {
        self.veh
            .iter()
            .map(|(id, v)| (id, v.s.to_bits(), v.speed.to_bits(), v.hop, v.in_conn))
            .collect()
    }
}

/// Four arms of `road`, each directed towards the centre; returns W, E, N, S.
fn cross(s: &mut Sim, road: &str) -> [SegmentId; 4] {
    [
        s.seg_with(road, (880.0, 1000.0), (1000.0, 1000.0)),
        s.seg_with(road, (1120.0, 1000.0), (1000.0, 1000.0)),
        s.seg_with(road, (1000.0, 880.0), (1000.0, 1000.0)),
        s.seg_with(road, (1000.0, 1120.0), (1000.0, 1000.0)),
    ]
}

fn from_arm(seg: SegmentId, s: f32) -> Access {
    acc(seg, s, Side::Right)
}

fn to_arm(seg: SegmentId, s: f32) -> Access {
    acc(seg, s, Side::Left)
}

fn crossing_traffic(rig: &mut Rig, arms: &[SegmentId; 4], per_arm: usize) {
    for k in 0..per_arm {
        for (i, &a) in arms.iter().enumerate() {
            // Straight, right and left in turn.
            let to = arms[(i + 1 + k % 3) % 4];
            rig.add(from_arm(a, 5.0 + (k as f32)), to_arm(to, 60.0));
        }
    }
}

#[test]
fn cars_never_overlap_nor_share_conflicting_connections_and_all_arrive() {
    let mut s = sim();
    let arms = cross(&mut s, "basic_2l");
    let mut rig = Rig::new(s);
    crossing_traffic(&mut rig, &arms, 10);
    let n = rig.veh.len();
    assert_eq!(n, 40);
    let used = rig.run_dry(800, None);
    assert!(used < 800, "everybody gets through");
    assert_eq!((rig.arrived, rig.invalid, rig.stuck), (n, 0, 0));
}

#[test]
fn nobody_enters_the_junction_on_red() {
    let mut s = sim();
    let arms = cross(&mut s, "medium_4l");
    let mut rig = Rig::new(s);
    assert!(!rig.graph.lights.is_empty(), "the crossing has a light");
    crossing_traffic(&mut rig, &arms, 8);
    let n = rig.veh.len();
    let (mut entries, mut waited) = (0u32, 0u32);
    let mut tick = 0u64;
    let mut finished = Finished::default();
    while !rig.veh.is_empty() && tick < 3000 {
        spawn_waiting(&mut rig.veh, &mut rig.st, &rig.graph, None, &mut finished);
        for sub in 0..cp_core::time::MOVE_SUBSTEPS {
            let before: Vec<(VehicleId, bool)> =
                rig.veh.iter().map(|(id, v)| (id, v.in_conn)).collect();
            let signal = signal_tick(tick, sub);
            substep(
                &mut rig.veh,
                &mut rig.st,
                &rig.graph,
                signal,
                sub,
                None,
                &mut finished,
            );
            for (id, was) in before {
                let Some(v) = rig.veh.get(id) else { continue };
                if v.speed < 0.1 && !v.in_conn {
                    waited += 1;
                }
                if !was && v.in_conn {
                    entries += 1;
                    let (from, to) = v.path.steps[v.hop as usize];
                    let ex = find_exit(&rig.graph, from, to).unwrap();
                    if ex.light != u32::MAX {
                        let sig =
                            rig.graph.lights[ex.light as usize].connection_signal(ex.index, signal);
                        assert_ne!(sig, Signal::Red, "entered on red");
                    }
                }
            }
            rig.check_invariants();
        }
        tick += 1;
    }
    assert!(rig.veh.is_empty(), "traffic cleared");
    assert_eq!(
        finished.arrived.len() + finished.stuck.len() + finished.invalid.len(),
        n
    );
    assert!(finished.stuck.is_empty() && finished.invalid.is_empty());
    assert!(entries as usize >= n);
    assert!(waited > 0, "some cars really waited for the light");
}

/// T-303 (§6.5a): a T of a 4-lane road (W, E) and a street (N), one car on the road going
/// straight and one on the street turning across it, both standing well inside the priority
/// zone. Returns the substep at which each enters the junction: `(road car, street car)`.
/// `signs`: `Main` on the street, `Yield` on both road arms; `light`: the light stays on.
/// The road (`road`, a 4-lane type) and a `basic_2l` street meet at a T; a vehicle of `kind` on
/// the road and a car turning off the street cross each other. Gives when each entered.
fn sign_race(signs: bool, light: bool) -> (u64, u64) {
    sign_race_on("medium_4l", 0, signs, light)
}

fn sign_race_on(road: &str, kind: u8, signs: bool, light: bool) -> (u64, u64) {
    use cp_net::{NetGraph, Sign};
    let mut s = sim();
    let w = s.seg_with(road, (880.0, 1000.0), (1000.0, 1000.0));
    let e = s.seg_with(road, (1120.0, 1000.0), (1000.0, 1000.0));
    let n = s.seg_with("basic_2l", (1000.0, 880.0), (1000.0, 1000.0));
    s.tick();
    {
        let mut g = s.world.resource_mut::<NetGraph>();
        let node = g.segment(n).unwrap().end;
        assert!(g.node(node).unwrap().control.has_light(), "two big arms");
        if !light {
            g.toggle_traffic_light(node).unwrap();
        }
        if signs {
            g.set_sign(node, n, Some(Sign::Main)).unwrap();
            g.set_sign(node, w, Some(Sign::Yield)).unwrap();
            g.set_sign(node, e, Some(Sign::Yield)).unwrap();
        }
    }
    let mut rig = Rig::new(s);
    assert_eq!(rig.graph.lights.is_empty(), !light);
    let class = cp_content::VehicleKind::ALL[usize::from(kind)].class();
    let path = |from: Access, to: Access| {
        crate::astar::find_class_path(&rig.graph, &CongestionMap::default(), class, &from, &to)
            .unwrap()
    };
    let car_path = |from: Access, to: Access| {
        find_car_path(&rig.graph, &CongestionMap::default(), &from, &to).unwrap()
    };
    let crosses = |a: &crate::CarPath, b: &crate::CarPath| {
        a.steps.iter().any(|&(fa, ta)| {
            find_exit(&rig.graph, fa, ta).is_some_and(|ex| {
                ex.conflicts.iter().any(|c| {
                    b.steps
                        .iter()
                        .any(|&(fb, tb)| rig.graph.car_slot(fb) == Some(c.from_slot) && c.to == tb)
                })
            })
        })
    };
    // A direction of the road and a turn off the street that cross each other.
    let (from, to, turn) = [(w, e), (e, w)]
        .into_iter()
        .flat_map(|(f, t)| [(f, t, e), (f, t, w)])
        .find(|&(f, t, turn)| {
            crosses(
                &path(from_arm(f, 90.0), to_arm(t, 60.0)),
                &car_path(from_arm(n, 90.0), to_arm(turn, 60.0)),
            )
        })
        .expect("a street turn crosses the road");
    let a = rig.add_kind(kind, from_arm(from, 90.0), to_arm(to, 60.0));
    let b = rig.add(from_arm(n, 90.0), to_arm(turn, 60.0));
    let mut entered: [Option<u64>; 2] = [None, None];
    let mut finished = Finished::default();
    let mut k = 0u64;
    for tick in 0..3000u64 {
        spawn_waiting(&mut rig.veh, &mut rig.st, &rig.graph, None, &mut finished);
        for sub in 0..cp_core::time::MOVE_SUBSTEPS {
            let signal = signal_tick(tick, sub);
            substep(
                &mut rig.veh,
                &mut rig.st,
                &rig.graph,
                signal,
                sub,
                None,
                &mut finished,
            );
            for (i, id) in [a, b].into_iter().enumerate() {
                let Some(v) = rig.veh.get(id) else { continue };
                if v.in_conn && entered[i].is_none() {
                    entered[i] = Some(k);
                    let (from, to) = v.path.steps[v.hop as usize];
                    let ex = find_exit(&rig.graph, from, to).unwrap();
                    if ex.light != u32::MAX {
                        let sig =
                            rig.graph.lights[ex.light as usize].connection_signal(ex.index, signal);
                        assert_ne!(sig, Signal::Red, "entered on red");
                    }
                }
            }
            rig.check_invariants();
            k += 1;
        }
        if entered.iter().all(Option::is_some) {
            break;
        }
    }
    (
        entered[0].expect("the road car enters"),
        entered[1].expect("the street car enters"),
    )
}

#[test]
fn signs_make_the_road_give_way_to_the_street_and_a_light_overrides_them() {
    // No signs: the 4-lane road is the main road by rank, the street car waits.
    let (road, street) = sign_race(false, false);
    assert!(road < street, "rank rule: road {road}, street {street}");
    // Main on the street, yield on the road: the mirror image.
    let (road, street) = sign_race(true, false);
    assert!(street < road, "signs: road {road}, street {street}");
    // With the light on the signs do nothing: both go by the signal, as without signs.
    assert_eq!(sign_race(true, true), sign_race(false, true));
}

/// T-302b, T-302c: a tram on `tram_4l` and a trolleybus on `trolley_4l` obey the signs and the
/// light like a car (§6.5a).
#[test]
fn a_tram_and_a_trolleybus_give_way_by_the_signs_and_never_enter_on_red() {
    for (road_id, kind) in [
        ("tram_4l", cp_content::VehicleKind::Tram),
        ("trolley_4l", cp_content::VehicleKind::Trolley),
    ] {
        let kind = kind as u8;
        let (road, street) = sign_race_on(road_id, kind, false, false);
        assert!(
            road < street,
            "{road_id} rank rule: {road}, street {street}"
        );
        let (road, street) = sign_race_on(road_id, kind, true, false);
        assert!(street < road, "{road_id} signs: {road}, street {street}");
        // On the light `sign_race_on` asserts that neither enters on red.
        assert_eq!(
            sign_race_on(road_id, kind, true, true),
            sign_race_on(road_id, kind, false, true)
        );
    }
}

#[test]
fn a_queue_builds_at_the_light_and_clears_when_the_load_stops() {
    let mut s = sim();
    let arms = cross(&mut s, "medium_4l");
    let w = arms[0];
    let e = arms[1];
    let mut rig = Rig::new(s);
    for k in 0..8 {
        rig.add(from_arm(w, 5.0 + (k % 3) as f32), to_arm(e, 100.0));
    }
    let n = rig.veh.len();
    let (mut peak_standing, mut last_tick) = (0usize, 0);
    for t in 0..2000u64 {
        rig.tick(t, None, STUCK_TICKS);
        rig.check_invariants();
        let standing = rig.veh.iter().filter(|(_, v)| v.speed < 0.1).count();
        peak_standing = peak_standing.max(standing);
        last_tick = t;
        if rig.veh.is_empty() {
            break;
        }
    }
    assert!(
        peak_standing >= 4,
        "a queue formed ({peak_standing} standing)"
    );
    assert!(
        rig.veh.is_empty(),
        "the queue dissolved by tick {last_tick}"
    );
    assert_eq!((rig.arrived, rig.stuck, rig.invalid), (n, 0, 0));
}

#[test]
fn the_hard_cap_removes_cars_waiting_at_a_red_at_a_tiny_threshold() {
    let mut s = sim();
    let arms = cross(&mut s, "medium_4l");
    let mut rig = Rig::new(s);
    crossing_traffic(&mut rig, &arms, 10);
    for t in 0..400 {
        rig.tick(t, None, 2);
        if rig.veh.is_empty() {
            break;
        }
    }
    assert!(
        rig.stuck > 0,
        "a car waiting for a red is removed at a tiny threshold"
    );
    assert!(rig.veh.is_empty() || rig.arrived + rig.stuck < 40);
}

/// Packs `stuck_timer` (standing ticks << 16 | suspicious ticks), see `drive_tick`.
fn timer(standing: u32, suspicious: u32) -> u32 {
    standing << 16 | suspicious
}

#[test]
fn a_car_blocked_for_the_hard_cap_disappears_whatever_the_reason() {
    let mut s = sim();
    let arms = cross(&mut s, "medium_4l");
    let mut rig = Rig::new(s);
    crossing_traffic(&mut rig, &arms, 10);
    let cap = 10 * STUCK_TICKS;
    for t in 0..400 {
        if rig.veh.is_empty() {
            break;
        }
        for (_, v) in rig.veh.iter_mut() {
            if v.speed < 0.1 {
                // 600 ticks standing for a legitimate reason: the 601st removes the car.
                v.stuck_timer = timer(cap, 0);
            }
        }
        rig.tick(t, None, STUCK_TICKS);
    }
    assert!(
        rig.stuck > 0,
        "the cap clears a car that stood {cap} ticks and one more"
    );
}

#[test]
fn a_long_queue_at_a_red_light_loses_nobody() {
    let mut s = sim();
    let w = s.seg_with("medium_4l", (-600.0, 1000.0), (1000.0, 1000.0));
    let e = s.seg_with("medium_4l", (1120.0, 1000.0), (1000.0, 1000.0));
    s.seg_with("medium_4l", (1000.0, 880.0), (1000.0, 1000.0));
    s.seg_with("medium_4l", (1000.0, 1120.0), (1000.0, 1000.0));
    let mut rig = Rig::new(s);
    assert!(!rig.graph.lights.is_empty());
    for k in 0..200 {
        rig.add(from_arm(w, 5.0 + 7.0 * k as f32), to_arm(e, 100.0));
    }
    for t in 0..ticks(4000.0) {
        rig.tick(t, None, STUCK_TICKS);
        if rig.veh.is_empty() {
            break;
        }
    }
    assert_eq!((rig.arrived, rig.stuck, rig.invalid), (200, 0, 0));
}

#[test]
fn a_ring_of_full_lanes_is_a_gridlock_and_clears() {
    let mut s = sim();
    let pts = [
        (1000.0, 1000.0),
        (1090.0, 1000.0),
        (1090.0, 1090.0),
        (1000.0, 1090.0),
    ];
    let segs: Vec<SegmentId> = (0..4)
        .map(|i| s.seg_with("basic_2l", pts[i], pts[(i + 1) % 4]))
        .collect();
    let mut rig = Rig::new(s);
    for k in 0..4 {
        for j in 0..12 {
            // Two sides further round the ring: the car must go through the next lane.
            rig.add(
                from_arm(segs[k], 5.0 + 7.0 * j as f32),
                from_arm(segs[(k + 2) % 4], 60.0),
            );
        }
    }
    let n = rig.veh.len();
    for t in 0..ticks(800.0) {
        rig.tick(t, None, STUCK_TICKS);
        if rig.veh.is_empty() {
            break;
        }
    }
    println!("ring: arrived {} stuck {}", rig.arrived, rig.stuck);
    assert!(rig.stuck > 0 && rig.arrived + rig.stuck + rig.invalid == n);
}

fn lattice(lines: usize) -> (Sim, Vec<SegmentId>) {
    lattice_of(None, lines)
}

fn lattice_of(road: Option<&str>, lines: usize) -> (Sim, Vec<SegmentId>) {
    lattice_pitch(road, lines, 90.0)
}

/// [`lattice_of`] with its lines `pitch` metres apart.
fn lattice_pitch(road: Option<&str>, lines: usize, pitch: f32) -> (Sim, Vec<SegmentId>) {
    let mut s = sim();
    let mut segs = Vec::new();
    for line in 0..lines {
        let o = 1000.0 + line as f32 * pitch;
        for piece in 0..lines - 1 {
            let a = 1000.0 + piece as f32 * pitch;
            for (p, q) in [((a, o), (a + pitch, o)), ((o, a), (o, a + pitch))] {
                segs.push(match road {
                    Some(r) => s.seg_with(r, p, q),
                    None => s.seg(p, q),
                });
            }
        }
    }
    (s, segs)
}

fn crowd(rig: &mut Rig, segs: &[SegmentId], n: usize) {
    for i in 0..n {
        let a = segs[(i * 7) % segs.len()];
        let b = segs[(i * 11 + 7) % segs.len()];
        let from = from_arm(a, 6.0 + ((i / segs.len()) % 5) as f32 * 14.0);
        let to = if i % 2 == 0 {
            from_arm(b, 70.0)
        } else {
            to_arm(b, 20.0)
        };
        if let Ok(path) = find_car_path(&rig.graph, &CongestionMap::default(), &from, &to) {
            let _ = path;
            rig.add(from, to);
        }
    }
}

#[test]
fn one_and_eight_threads_and_sequential_runs_are_identical() {
    let run = |pool: Option<TaskPool>| {
        let (s, segs) = lattice(6);
        let mut rig = Rig::new(s);
        crowd(&mut rig, &segs, 1500);
        assert!(rig.veh.len() > 1000);
        for t in 0..150 {
            rig.tick(t, pool.as_ref(), STUCK_TICKS);
            rig.check_invariants();
        }
        (rig.snapshot(), rig.arrived, rig.invalid, rig.stuck)
    };
    let seq = run(None);
    let one = run(Some(TaskPoolBuilder::new().num_threads(1).build()));
    let eight = run(Some(TaskPoolBuilder::new().num_threads(8).build()));
    assert!(seq.1 > 0, "some cars arrived");
    assert_eq!(seq, one);
    assert_eq!(seq, eight);
}

#[test]
fn a_car_accelerates_to_the_limit_and_follows_a_slow_leader_without_touching() {
    let mut s = sim();
    let a = s.seg((1000.0, 1000.0), (1090.0, 1000.0));
    let b = s.seg((1090.0, 1000.0), (1180.0, 1000.0));
    let mut rig = Rig::new(s);
    let lead = rig.add(from_arm(a, 40.0), from_arm(b, 85.0));
    let chaser = rig.add(from_arm(a, 5.0), from_arm(b, 85.0));
    let limit = rig
        .graph
        .car
        .iter()
        .flatten()
        .map(|l| l.speed)
        .fold(0.0, f32::max);
    let mut top = 0.0f32;
    for t in 0..60 {
        rig.tick(t, None, STUCK_TICKS);
        rig.check_invariants();
        if let Some(v) = rig.veh.get(chaser) {
            top = top.max(v.speed);
        }
        let _ = lead;
    }
    assert!(
        top > 0.9 * limit && top <= limit + 1e-3,
        "top speed {top} vs {limit}"
    );
    assert_eq!(rig.arrived, 2);
}

/// The 16k-car workload of the driving benchmark: 16k cars on a 41×41 lattice for 80 ticks on
/// the compute pool. No clock in a sim crate: time it from outside, e.g.
/// `time cargo test -p cp_traffic --release drive_16k_cars_workload -- --ignored --nocapture`
/// (the setup, mostly path finding, is part of the wall time; compare two builds the same way).
#[test]
#[ignore = "workload for external timing: run in release with --nocapture"]
fn drive_16k_cars_workload() {
    let (s, segs) = lattice(41);
    let mut rig = Rig::new(s);
    crowd(&mut rig, &segs, 16_000);
    let pool = TaskPoolBuilder::new().build();
    let (mut ticks, mut on_road) = (0u32, 0usize);
    for t in 0..80u64 {
        rig.tick(t, Some(&pool), STUCK_TICKS);
        if t >= 20 {
            ticks += 1;
            on_road += rig.veh.iter().filter(|(_, v)| v.spawned).count();
        }
    }
    eprintln!(
        "workload: {} cars on the road on average over {ticks} ticks ({} threads), arrived {}, stuck {}",
        on_road / ticks as usize,
        pool.thread_num(),
        rig.arrived,
        rig.stuck
    );
}

#[test]
fn cars_whose_road_disappears_are_dropped_without_stale_ids() {
    let mut s = sim();
    let a = s.seg((1000.0, 1000.0), (1090.0, 1000.0));
    s.seg((1090.0, 1000.0), (1180.0, 1000.0));
    let c = s.seg((1180.0, 1000.0), (1270.0, 1000.0));
    let mut rig = Rig::new(s);
    for k in 0..4 {
        rig.add(from_arm(a, 8.0 + 12.0 * k as f32), from_arm(c, 80.0));
    }
    for t in 0..12 {
        rig.tick(t, None, STUCK_TICKS);
    }
    assert!(!rig.veh.is_empty(), "cars are under way");
    // The last street is demolished while they drive; the snapshot is rebuilt.
    rig.sim
        .world
        .resource_mut::<cp_net::NetGraph>()
        .remove_segment(c)
        .unwrap();
    rig.sim.tick();
    rig.graph = Arc::clone(rig.sim.world.resource::<PathNetwork>().graph());
    let gone = rig.st.rebuild(&rig.veh, &rig.graph, 1);
    for id in gone {
        rig.veh.remove(id);
        rig.invalid += 1;
    }
    for t in 12..300 {
        rig.tick(t, None, STUCK_TICKS);
        rig.check_invariants();
    }
    assert!(rig.veh.is_empty(), "nobody is left driving on a dead route");
    assert_eq!(rig.arrived + rig.invalid + rig.stuck, 4);
    assert!(rig.invalid > 0);
}

/// BUG-018: on roads with 2+ lanes per direction every ordered pair of segments of a connected
/// lattice has a car path (lane-change edges), and cars drive them, changing lanes.
#[test]
fn multilane_lattices_have_a_path_between_every_pair_and_cars_arrive() {
    for road in ["medium_4l", "large_6l", "basic_2l"] {
        let (s, segs) = lattice_of(Some(road), 4);
        let mut rig = Rig::new(s);
        let mut no_path = 0;
        for &a in &segs {
            for &b in &segs {
                if a == b {
                    continue;
                }
                let (from, to) = (acc(a, 45.0, Side::Right), acc(b, 45.0, Side::Right));
                if find_car_path(&rig.graph, &CongestionMap::default(), &from, &to).is_err() {
                    no_path += 1;
                }
            }
        }
        assert_eq!(no_path, 0, "{road}: pairs without a path");
        let n = 60;
        for i in 0..n {
            let a = segs[(i * 5) % segs.len()];
            let b = segs[(i * 7 + 3) % segs.len()];
            if a != b {
                rig.add(acc(a, 20.0, Side::Right), acc(b, 45.0, Side::Right));
            }
        }
        let total = rig.veh.len();
        rig.run_dry(ticks(1200.0), None);
        assert_eq!(rig.arrived + rig.stuck + rig.invalid, total, "{road}");
        assert!(
            rig.arrived * 10 >= total * 9,
            "{road}: {} of {total}",
            rig.arrived
        );
    }
}

/// Smallest clearance (m) between two cars in *different* places that share a source lane:
/// their centre distance minus half the sum of their lengths, so a positive value means the
/// bodies are apart. The pairs are one car on the lane and one in a connection leaving it, or
/// two in different connections leaving it. Cars in one place are ordered by `s`
/// (`check_invariants`); their centre distance on a tight curve is a chord, shorter than the
/// arc. `None` if no such pair exists right now.
fn min_shared_lane_distance(rig: &Rig) -> Option<f32> {
    let net = rig.sim.world.resource::<cp_net::NetGraph>();
    let geo = rig.sim.world.resource::<cp_net::JunctionGeometry>();
    let pos = |id: VehicleId| {
        let v = rig.veh.get(id)?;
        crate::pose::vehicle_pose(v, rig.st.specs.of(v).length, &rig.graph, net, geo)
            .map(|(p, _)| p)
    };
    let mut best: Option<f32> = None;
    for slot in 0..rig.st.lanes.len() as u32 {
        // The lane itself, then each connection leaving it.
        let mut places: Vec<&Vec<VehicleId>> = vec![&rig.st.lanes[slot as usize]];
        places.extend(
            rig.st
                .conns
                .iter()
                .filter(|((f, _), _)| *f == slot)
                .map(|(_, l)| l),
        );
        for (i, pa) in places.iter().enumerate() {
            for pb in &places[i + 1..] {
                for &a in pa.iter() {
                    for &b in pb.iter() {
                        if let (Some(p), Some(q)) = (pos(a), pos(b)) {
                            let d = p.distance(q)
                                - 0.5
                                    * (rig.st.specs.of(rig.veh.get(a).unwrap()).length
                                        + rig.st.specs.of(rig.veh.get(b).unwrap()).length);
                            best = Some(best.map_or(d, |m| m.min(d)));
                        }
                    }
                }
            }
        }
    }
    best
}

/// Smallest gap (m) in the lane's own coordinates between a car on a lane and the rear bumper of
/// a car that has left it into a connection (positive: the lane car is behind the other's rear).
/// Unlike centre distances this is exact for long vehicles, whose bodies do not follow a turn.
fn min_lane_to_connection_gap(rig: &Rig) -> Option<f32> {
    let mut best: Option<f32> = None;
    for (&(slot, _), conn) in &rig.st.conns {
        let Some(node) = rig.graph.car[slot as usize].as_ref() else {
            continue;
        };
        for &c in conn {
            let c = rig.veh.get(c).unwrap();
            let rear = node.s_out + c.s - rig.st.specs.of(c).length;
            for &l in &rig.st.lanes[slot as usize] {
                let l = rig.veh.get(l).unwrap();
                let gap = rear - l.s;
                best = Some(best.map_or(gap, |b| b.min(gap)));
            }
        }
    }
    best
}

/// `car_basic` plus a 12 m truck (second `VehicleDef`, kind `Truck`), as the specs of a run.
fn mixed_specs() -> VehicleSpecs {
    let car = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/content/vehicles/car_basic.ron"
    ))
    .unwrap();
    let truck = r#"Vehicle((id: "truck_12", name: "Truck", kind: Truck, length: 12.0,
        width: 2.5, max_speed_kmh: 80.0, accel: 1.0, decel: 3.0, min_gap: 3.0, mesh: "vehicle/truck"))"#;
    let db = cp_content::ContentDb::from_sources(vec![
        ("content/vehicles/car_basic.ron".into(), car),
        ("content/vehicles/truck_12.ron".into(), truck.into()),
    ])
    .unwrap();
    VehicleSpecs::from_content(&db)
}

/// 300 random trips on a 5x5 medium_4l lattice for a game day; every third vehicle is a truck
/// when `mixed`. Returns the smallest clearance between cars sharing a source lane. The lines are
/// 110 m apart: medium_4l is 32 m wide (4 cells, T-275), so each piece keeps 78 m of street
/// between its junctions, and the trips start and end on it, past the junction's 16 m.
fn shared_lane_run(seed: u64, mixed: bool) -> f32 {
    let (s, segs) = lattice_pitch(Some("medium_4l"), 5, 110.0);
    let mut rig = Rig::new(s);
    if mixed {
        rig.specs = mixed_specs();
    }
    let mut rng = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut next = |n: usize| {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 11) as usize % n
    };
    let mut trips = 0;
    while trips < 300 {
        let (a, b) = (segs[next(segs.len())], segs[next(segs.len())]);
        if a == b {
            continue;
        }
        let side = |k: usize| {
            if k.is_multiple_of(2) {
                Side::Right
            } else {
                Side::Left
            }
        };
        // Anywhere from past the junction (16 m) up to its stop line (94 m): a car that starts
        // close to the next junction in the wrong lane turns from its own lane or goes another
        // way (BUG-090), it is never stuck waiting for a lane change.
        let from = acc(a, 18.0 + next(70) as f32, side(next(2)));
        let to = acc(b, 18.0 + next(70) as f32, side(next(2)));
        if find_car_path(&rig.graph, &CongestionMap::default(), &from, &to).is_ok() {
            rig.add_kind(u8::from(mixed && trips % 3 == 0), from, to);
            trips += 1;
        }
    }
    let mut closest = f32::MAX;
    for t in 0..cp_core::time::TICKS_PER_DAY {
        rig.tick(t, None, STUCK_TICKS);
        rig.check_invariants();
        let d = if mixed {
            min_lane_to_connection_gap(&rig)
        } else {
            min_shared_lane_distance(&rig)
        };
        if let Some(d) = d {
            closest = closest.min(d);
        }
        if rig.veh.is_empty() {
            break;
        }
    }
    println!(
        "seed {seed} mixed {mixed}: clearance {closest:.2} m, arrived {} stuck {} invalid {} why {:?}",
        rig.arrived, rig.stuck, rig.invalid, rig.st.stuck_why
    );
    // BUG-090: nobody is removed for a lane change it could not make.
    assert_eq!(
        rig.st.stuck_why[crate::drive::StuckWhy::LaneChange as usize],
        0,
        "seed {seed} mixed {mixed}: stuck waiting for a lane change"
    );
    // Long trucks need 15 m of free lane to enter and can jam a dense lattice for good; only
    // the uniform run must lose nobody.
    if mixed {
        assert!(
            rig.arrived * 10 >= 300 * 9,
            "seed {seed}: {} arrived",
            rig.arrived
        );
    } else {
        assert_eq!(rig.stuck + rig.invalid, 0, "seed {seed}: cars were removed");
    }
    closest
}

/// BUG-020: after a lane hop cars keep their bumpers apart, also towards cars that have just
/// left the target lane into a junction connection.
#[test]
fn cars_sharing_a_lane_or_its_connections_never_touch() {
    for seed in [5u64, 7] {
        let clearance = shared_lane_run(seed, false);
        assert!(
            clearance > 0.0,
            "seed {seed}: bodies overlap by {clearance}"
        );
    }
}

/// T-065: the same with a mix of 4.2 m cars and 12 m trucks, each with its own gap.
#[test]
fn cars_and_trucks_sharing_a_lane_or_its_connections_never_touch() {
    for seed in [5u64, 7] {
        let clearance = shared_lane_run(seed, true);
        assert!(
            clearance > 0.0,
            "seed {seed}: bodies overlap by {clearance}"
        );
    }
}

/// T-065: a second `VehicleDef` of 12 m queues with 12 m of leader length (+ its own gap), not
/// with the constants.
#[test]
fn a_queue_of_twelve_metre_trucks_is_spaced_by_their_length() {
    let mut s = sim();
    let arms = cross(&mut s, "medium_4l");
    let (w, e) = (arms[0], arms[1]);
    let mut rig = Rig::new(s);
    rig.specs = mixed_specs();
    for k in 0..4 {
        rig.add_kind(1, from_arm(w, 5.0 + 14.0 * k as f32), to_arm(e, 100.0));
    }
    let mut gaps: Vec<f32> = Vec::new();
    for t in 0..2000u64 {
        rig.tick(t, None, STUCK_TICKS);
        rig.check_invariants();
        // Neighbouring trucks of one lane that both stand: a queue.
        for l in &rig.st.lanes {
            let v: Vec<&Vehicle> = l.iter().filter_map(|&id| rig.veh.get(id)).collect();
            gaps.extend(
                v.windows(2)
                    .filter(|w| w[0].speed < 0.1 && w[1].speed < 0.1)
                    .map(|w| w[0].s - w[1].s),
            );
        }
        if rig.veh.is_empty() {
            break;
        }
    }
    assert!(!gaps.is_empty(), "trucks queued at the light");
    for g in gaps {
        // Front bumper to front bumper: the 12 m body plus a standstill gap of 3 m (and some
        // braking slack), never the 4.2 m of a car.
        assert!(
            (12.0 + HARD_GAP..=12.0 + 3.0 + 4.0).contains(&g),
            "spacing {g}"
        );
    }
    assert_eq!((rig.stuck, rig.invalid), (0, 0));
}

/// T-268 acceptance (f): cars drive round curves of the smallest radius a 4-cell road takes
/// (5 cells, 40 m): a loop of four streets with a quarter curve at every corner, trips from
/// every street to every other, in either direction and lane. Everybody arrives, nobody is
/// removed as stuck, no two bodies in one lane touch.
#[test]
fn cars_drive_round_the_tightest_curves_of_a_4_cell_road() {
    use bevy_math::Vec3;
    use cp_core::curve::CubicBezier3;
    let mut s = sim();
    let road = s.road("medium_4l");
    let def = s.db.road(road).clone();
    let r = 40.0;
    // A circle's quarter as a cubic (handles 0.5523 r long).
    let k = 0.552_284_8 * r;
    let (lo, hi) = (1000.0, 1300.0);
    let v = |x: f32, z: f32| Vec3::new(x, 0.0, z);
    let mut sides = Vec::new();
    let add = |s: &mut Sim, c: CubicBezier3| {
        s.world
            .resource_mut::<cp_net::NetGraph>()
            .add_segment(&def, road, c, cp_net::Elevation::Ground)
            .unwrap()
    };
    // Clockwise (x east, z south): the top street, the top-right curve, the right street, ...
    let corners = [
        (
            v(hi - r, lo),
            v(hi - r + k, lo),
            v(hi, lo + r - k),
            v(hi, lo + r),
        ),
        (
            v(hi, hi - r),
            v(hi, hi - r + k),
            v(hi - r + k, hi),
            v(hi - r, hi),
        ),
        (
            v(lo + r, hi),
            v(lo + r - k, hi),
            v(lo, hi - r + k),
            v(lo, hi - r),
        ),
        (
            v(lo, lo + r),
            v(lo, lo + r - k),
            v(lo + r - k, lo),
            v(lo + r, lo),
        ),
    ];
    for (i, &(a, b, c, d)) in corners.iter().enumerate() {
        let prev = corners[(i + 3) % 4].3;
        sides.push(add(&mut s, CubicBezier3::line(prev, a)));
        add(&mut s, CubicBezier3::new(a, b, c, d));
    }
    let mut rig = Rig::new(s);
    let mut trips = 0;
    for i in 0..4 {
        for j in 0..4 {
            if i == j {
                continue;
            }
            for side in [Side::Right, Side::Left] {
                for at in [30.0, 90.0, 150.0] {
                    let from = acc(sides[i], at, side);
                    let to = acc(sides[j], 180.0 - at, side);
                    if find_car_path(&rig.graph, &CongestionMap::default(), &from, &to).is_ok() {
                        rig.add(from, to);
                        trips += 1;
                    }
                }
            }
        }
    }
    assert!(trips >= 60, "{trips} trips");
    let mut closest = f32::MAX;
    for t in 0..cp_core::time::TICKS_PER_DAY {
        rig.tick(t, None, STUCK_TICKS);
        rig.check_invariants();
        if let Some(d) = min_shared_lane_distance(&rig) {
            closest = closest.min(d);
        }
        if rig.veh.is_empty() {
            break;
        }
    }
    println!(
        "{trips} trips round the loop: arrived {} stuck {} clearance {closest:.2} m",
        rig.arrived, rig.stuck
    );
    assert!(
        rig.veh.is_empty(),
        "{} cars still on the loop",
        rig.veh.len()
    );
    assert_eq!(
        rig.arrived, trips,
        "stuck {} invalid {}",
        rig.stuck, rig.invalid
    );
    assert!(closest > 0.0, "bodies overlap by {closest}");
}

/// BUG-105: a 12 m truck turning right and left at an X sweeps round the corner. Drawn per tick
/// (`vehicle_pose`), its rear axle stays on the path its front axle drove (a two-axle body, the
/// rear following), and the heading turns only as the body moves: never on the spot.
#[test]
fn a_turning_truck_sweeps_round_the_corner_with_its_rear_following() {
    use crate::pose::{FRONT_AXLE, REAR_AXLE};
    use bevy_math::{Vec2, Vec3};
    for (turn, to) in [("to the south arm", 3), ("to the north arm", 2)] {
        let mut s = sim();
        let arms = cross(&mut s, "basic_2l");
        let mut rig = Rig::new(s);
        rig.specs = mixed_specs();
        let id = rig.add_kind(1, from_arm(arms[0], 20.0), to_arm(arms[to], 100.0));
        let len = 12.0;
        let mut poses: Vec<(Vec3, f32)> = Vec::new();
        for t in 0..2000u64 {
            rig.tick(t, None, STUCK_TICKS);
            let Some(v) = rig.veh.get(id) else { break };
            let net = rig.sim.world.resource::<cp_net::NetGraph>();
            let geo = rig.sim.world.resource::<cp_net::JunctionGeometry>();
            if let Some(p) = crate::pose::vehicle_pose(v, len, &rig.graph, net, geo) {
                poses.push(p);
            }
        }
        assert_eq!(rig.arrived, 1, "{turn}: the truck arrives");
        let axle = |(c, yaw): (Vec3, f32), share: f32| {
            let d = Vec2::new(yaw.sin(), yaw.cos());
            Vec2::new(c.x, c.z) + d * len * (0.5 - share)
        };
        let fronts: Vec<Vec2> = poses.iter().map(|p| axle(*p, FRONT_AXLE)).collect();
        let near_path = |q: Vec2| {
            fronts
                .windows(2)
                .map(|w| {
                    let (a, b) = (w[0], w[1]);
                    let ab = b - a;
                    let t = if ab.length_squared() > 1e-9 {
                        ((q - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    q.distance(a + ab * t)
                })
                .fold(f32::MAX, f32::min)
        };
        let mut turned = 0.0f32;
        for w in poses.windows(2) {
            let ((c0, y0), (c1, y1)) = (w[0], w[1]);
            let mut dy = (y1 - y0).abs() % std::f32::consts::TAU;
            if dy > std::f32::consts::PI {
                dy = std::f32::consts::TAU - dy;
            }
            turned += dy;
            let moved = Vec2::new(c1.x - c0.x, c1.z - c0.z).length();
            // A wheelbase of 7.2 m turns at most about 1 / 7 rad per metre on a tight curve.
            assert!(
                dy <= moved * 0.25 + 1e-3,
                "{turn}: turned {dy} rad while moving {moved} m"
            );
        }
        assert!(
            (1.4..1.8).contains(&turned),
            "{turn}: a quarter turn in all, got {turned}"
        );
        // Once the front has driven a wheelbase (before that the rear stands where it started).
        let start = fronts[0];
        for (p, f) in poses.iter().zip(&fronts) {
            if f.distance(start) < len {
                continue;
            }
            let rear = axle(*p, REAR_AXLE);
            let off = near_path(rear);
            assert!(off < 0.35, "{turn}: rear axle {off} m off the path");
        }
    }
}

#[test]
fn a_free_car_moves_exactly_as_its_four_substeps_and_most_of_a_lone_trip_is_free() {
    let mut s = sim();
    let a = s.seg((1000.0, 1000.0), (1450.0, 1000.0));
    let b = s.seg((1450.0, 1000.0), (1900.0, 1000.0));
    let mut rig = Rig::new(s);
    let car = rig.add(from_arm(a, 5.0), from_arm(b, 440.0));
    let limit = rig
        .graph
        .car
        .iter()
        .flatten()
        .map(|l| l.speed)
        .fold(0.0, f32::max);
    let spec = *rig.specs.of_kind(0);
    let (mut driven, mut free) = (0, 0);
    for t in 0..ticks(120.0) {
        let before = rig
            .veh
            .get(car)
            .map(|v| (v.s, v.speed, v.hop, v.in_conn, v.spawned));
        rig.tick(t, None, STUCK_TICKS);
        rig.check_invariants();
        let Some(v) = rig.veh.get(car) else { break };
        let Some((s0, v0, hop, in_conn, spawned)) = before else {
            continue;
        };
        if !spawned {
            continue;
        }
        driven += 1;
        if !rig.st.free.get(car.index as usize) {
            continue;
        }
        free += 1;
        assert!(
            !in_conn && !v.in_conn && v.hop == hop,
            "a free car stays on its lane"
        );
        let (mut s1, mut v1) = (s0, v0);
        for _ in 0..cp_core::time::MOVE_SUBSTEPS {
            (s1, v1) = follow_at(s1, v1, &spec, limit, None);
        }
        assert_eq!(
            (v.s.to_bits(), v.speed.to_bits()),
            (s1.to_bits(), v1.to_bits()),
            "tick {t}"
        );
    }
    assert_eq!(rig.arrived, 1);
    assert!(free * 2 > driven, "{free} of {driven} ticks free");
}
