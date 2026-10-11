//! Vehicle movement on lanes and through junctions (`architecture/system-design.md` §9.4).
//!
//! **Time.** A tick is [`MOVE_SECONDS_PER_TICK`] seconds of movement, integrated in
//! [`MOVE_SUBSTEPS`] substeps of [`DT`] seconds; the leader, junction and stuck checks run on
//! every substep. The signal plan is evaluated on the movement clock ([`signal_tick`]).
//!
//! **State.** A vehicle is *on a lane* or *on a junction connection*, named by the stable
//! `(from, to)` lane pair of its path — never by a connection index (indices change when a
//! node is recomputed). Its `s` is the position of its front bumper in metres along the lane
//! (from the lane start) or along the connection curve. The occupancy lists
//! ([`DriveState`]) are derived: front-most vehicle first, rebuilt after a load and after
//! every road-network change. Lanes are indexed by arena slot.
//!
//! **Substep.** Every place (lane or connection) with vehicles is stepped in parallel in
//! fixed chunks, reading only the state of the previous substep and writing a list of
//! updates ([`Upd`]). The updates are then applied sequentially in place order, so a move
//! between places (lane → connection → lane) is deterministic and independent of the thread
//! count. The junction gate is re-checked against live state while applying, so two vehicles
//! cannot both enter conflicting connections in the same substep.
//!
//! **Following** is a Krauss-style safe speed behind the leader: the next vehicle on the
//! place, else the rear-most vehicle of the connection / next lane (when the junction gate
//! is open), else a virtual standing obstacle at the stop line (gate closed). Positions never
//! overlap: a hard clamp keeps `s` behind the leader's rear bumper.
//!
//! **Junction gate** (checked by the front vehicle of a lane): the light is green (or yellow
//! and the car cannot stop); no conflicting connection holds a vehicle; at a node without a
//! light, the front car of no lane with priority on a conflicting connection is within
//! [`PRIORITY_ZONE`] of its stop line and heading for it; and the target lane has room for the car (counting
//! cars already inside the junction heading there).

use std::collections::BTreeMap;

use bevy_tasks::TaskPool;
use cp_core::Arena;
use cp_core::time::{MOVE_SECONDS_PER_TICK, MOVE_SUBSTEPS};
use cp_net::{LaneId, Signal};

use crate::agents::{Vehicle, VehicleId};
use crate::graph::{ExitConn, LaneNode, PathGraph};

/// Seconds of movement per substep.
pub const DT: f32 = MOVE_SECONDS_PER_TICK / MOVE_SUBSTEPS as f32;
/// Vehicle length, metres (`car_basic`).
pub const CAR_LENGTH: f32 = 4.2;
/// Standstill bumper-to-bumper gap, metres.
pub const MIN_GAP: f32 = 2.0;
/// Closest two cars may ever get, metres (hard clamp).
pub const HARD_GAP: f32 = 0.5;
/// Acceleration, m/s² (§9.4).
pub const ACCEL: f32 = 2.0;
/// Braking, m/s² (§9.4).
pub const DECEL: f32 = 4.0;
/// Reaction time in the safe-speed formula, seconds.
pub const REACTION: f32 = 1.0;
/// Distance before a stop line inside which a car with priority blocks yielding traffic.
pub const PRIORITY_ZONE: f32 = 30.0;
/// Ticks standing for a suspicious reason (no room beyond the junction, or behind such a car)
/// before a car is removed (§9.4: 120 s of movement). A red light or yielding does not count.
pub const STUCK_TICKS: u32 = 60;
/// Hard cap on a standstill for any reason, in multiples of the stuck threshold (§9.4 v1.1).
pub const HARD_CAP_FACTOR: u32 = 10;
/// How many full lanes ahead the suspicion of a blocked car is followed.
const LEADER_CHAIN: u32 = 8;
/// Vehicles per parallel chunk of places.
const CHUNK: usize = 64;
/// Fewer vehicles than this are stepped on the calling thread.
const PARALLEL_MIN: usize = 512;

/// Plan time of the signal at `substep` of `tick`: the plan counts 50 ms ticks, one substep
/// is 0.5 s of movement = 10 plan ticks.
pub fn signal_tick(tick: u64, substep: u32) -> u64 {
    (tick * u64::from(MOVE_SUBSTEPS) + u64::from(substep)) * 10
}

/// Derived occupancy of lanes and connections.
#[derive(Clone, Debug, Default)]
pub(crate) struct DriveState {
    /// Vehicles per lane slot, front-most first.
    pub lanes: Vec<Vec<VehicleId>>,
    /// Vehicles per connection `(from slot, to slot)`, front-most first.
    pub conns: BTreeMap<(u32, u32), Vec<VehicleId>>,
    /// Vehicles inside a junction heading for each lane slot.
    pub entering: Vec<u16>,
    /// Vehicles not yet on the road (waiting for room at their start), in id order.
    pub waiting: Vec<VehicleId>,
    pub ready: bool,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ev {
    Stay,
    /// Leaves its lane/connection forwards.
    Cross,
    Arrive,
    Invalid,
}

struct Upd {
    id: VehicleId,
    s: f32,
    v: f32,
    ev: Ev,
}

#[derive(Clone, Copy)]
enum Place {
    Lane(u32),
    Conn(u32, u32),
}

/// What a tick of driving finished.
#[derive(Default)]
pub(crate) struct Finished {
    pub arrived: Vec<Vehicle>,
    pub invalid: Vec<Vehicle>,
    pub stuck: Vec<Vehicle>,
}

struct Ctx<'a> {
    g: &'a PathGraph,
    veh: &'a Arena<Vehicle>,
    st: &'a DriveState,
    signal: u64,
}

pub(crate) fn lane_node(g: &PathGraph, id: LaneId) -> Option<&LaneNode> {
    g.car
        .get(id.index as usize)?
        .as_ref()
        .filter(|l| l.id == id)
}

pub(crate) fn find_exit(g: &PathGraph, from: LaneId, to: LaneId) -> Option<&ExitConn> {
    let slot = g.car_slot(from)?;
    g.exits[slot as usize].iter().find(|e| e.to == to)
}

impl DriveState {
    /// Rebuilds the occupancy from the vehicles; returns those whose place no longer exists.
    pub fn rebuild(
        &mut self,
        vehicles: &Arena<Vehicle>,
        g: &PathGraph,
        generation: u64,
    ) -> Vec<VehicleId> {
        let n = g.car.len();
        self.lanes = vec![Vec::new(); n];
        self.conns.clear();
        self.entering = vec![0; n];
        self.waiting.clear();
        let mut invalid = Vec::new();
        for (id, v) in vehicles.iter() {
            if !v.spawned {
                self.waiting.push(id);
                continue;
            }
            if v.in_conn {
                let Some((from, to)) = v.path.steps.get(v.hop as usize).copied() else {
                    invalid.push(id);
                    continue;
                };
                match find_exit(g, from, to) {
                    Some(ex) => {
                        let fs = g.car_slot(from).expect("exit implies lane");
                        self.conns.entry((fs, ex.to_slot)).or_default().push(id);
                        self.entering[ex.to_slot as usize] += 1;
                    }
                    None => invalid.push(id),
                }
            } else {
                match g.car_slot(v.lane()) {
                    Some(slot) => self.lanes[slot as usize].push(id),
                    None => invalid.push(id),
                }
            }
        }
        let key = |id: &VehicleId| {
            let v = vehicles.get(*id).expect("listed vehicle");
            (std::cmp::Reverse(v.s.to_bits()), *id)
        };
        for l in self.lanes.iter_mut().filter(|l| l.len() > 1) {
            l.sort_by_key(key);
        }
        for l in self.conns.values_mut().filter(|l| l.len() > 1) {
            l.sort_by_key(key);
        }
        self.ready = true;
        self.generation = generation;
        invalid
    }

    /// Takes `id` out of whatever list holds it (by lane slot, so it works for vehicles whose
    /// lanes were replaced).
    pub fn remove(&mut self, v: &Vehicle, id: VehicleId) {
        if !v.spawned {
            self.waiting.retain(|w| *w != id);
        } else if v.in_conn {
            if let Some((from, to)) = v.path.steps.get(v.hop as usize).copied() {
                let key = (from.index, to.index);
                if let Some(l) = self.conns.get_mut(&key) {
                    l.retain(|w| *w != id);
                    if l.is_empty() {
                        self.conns.remove(&key);
                    }
                    if let Some(e) = self.entering.get_mut(to.index as usize) {
                        *e = e.saturating_sub(1);
                    }
                }
            }
        } else if let Some(l) = self.lanes.get_mut(v.lane().index as usize) {
            l.retain(|w| *w != id);
        }
    }
}

/// Front vehicle's view of what lies ahead: `(rear bumper position, speed, standstill gap)`
/// in the coordinates of the place it is on.
type Leader = (f32, f32, f32);

fn follow(v: &Vehicle, limit: f32, leader: Option<Leader>) -> (f32, f32) {
    let mut vt = (v.speed + ACCEL * DT).min(limit);
    if let Some((rear, vl, g0)) = leader {
        let gap = (rear - v.s - g0).max(0.0);
        let bt = DECEL * REACTION;
        let safe = -bt + (bt * bt + vl * vl + 2.0 * DECEL * gap).sqrt();
        vt = vt.min(safe);
    }
    vt = vt.max(v.speed - DECEL * DT).max(0.0);
    let mut ds = 0.5 * (v.speed + vt) * DT;
    if let Some((rear, _, _)) = leader {
        let room = (rear - HARD_GAP - v.s).max(0.0);
        if ds > room {
            ds = room;
            vt = ds / DT;
        }
    }
    (v.s + ds, vt)
}

/// Room for one more car at the start of lane `slot`, given cars already inside junctions
/// heading there (`extra` of them are about to arrive).
fn has_room(g: &PathGraph, veh: &Arena<Vehicle>, st: &DriveState, slot: u32, extra: u16) -> bool {
    let Some(t) = g.car[slot as usize].as_ref() else {
        return false;
    };
    let span = t.s_out - t.s_in;
    let free = match st.lanes[slot as usize].last().and_then(|&id| veh.get(id)) {
        Some(o) => o.s - CAR_LENGTH - t.s_in,
        None => span,
    };
    let need = (CAR_LENGTH + MIN_GAP).min(0.5 * span);
    free - f32::from(st.entering[slot as usize] + extra) * (CAR_LENGTH + MIN_GAP) >= need
}

/// May the front vehicle `v` of its lane (`s_out` = its stop line) enter connection `ex`?
fn gate_open(
    g: &PathGraph,
    veh: &Arena<Vehicle>,
    st: &DriveState,
    signal: u64,
    v: &Vehicle,
    ex: &ExitConn,
    s_out: f32,
) -> bool {
    if ex.light != u32::MAX {
        match g.lights[ex.light as usize].connection_signal(ex.index, signal) {
            Signal::Green => {}
            Signal::Yellow => {
                // Only a car that cannot stop any more goes on.
                if v.speed * v.speed / (2.0 * DECEL) <= s_out - v.s {
                    return false;
                }
            }
            Signal::Red => return false,
        }
    }
    for c in &ex.conflicts {
        if st
            .conns
            .get(&(c.from_slot, c.to.index))
            .is_some_and(|l| !l.is_empty())
        {
            return false;
        }
        if c.higher
            && let Some(node) = g.car[c.from_slot as usize].as_ref()
        {
            // Only the front car of a lane is about to enter; the ones queued behind it
            // cannot, and counting them would let two queues block each other for good.
            if let Some(o) = st.lanes[c.from_slot as usize]
                .first()
                .and_then(|&id| veh.get(id))
                && node.s_out - o.s <= PRIORITY_ZONE
                && o.path
                    .steps
                    .get(o.hop as usize)
                    .is_some_and(|st| st.1 == c.to)
            {
                return false;
            }
        }
    }
    has_room(g, veh, st, ex.to_slot, 0)
}

fn step_lane(ctx: &Ctx, slot: u32, out: &mut Vec<Upd>) {
    let list = &ctx.st.lanes[slot as usize];
    let Some(node) = ctx.g.car[slot as usize].as_ref() else {
        return;
    };
    let mut prev: Option<&Vehicle> = None;
    for &id in list {
        let Some(v) = ctx.veh.get(id) else { continue };
        let mut ev = Ev::Stay;
        let mut open = false;
        let leader: Option<Leader> = match prev {
            Some(l) => Some((l.s - CAR_LENGTH, l.speed, MIN_GAP)),
            None => match v.path.steps.get(v.hop as usize).copied() {
                None => None,
                Some((_, to)) => match find_exit(ctx.g, node.id, to) {
                    None => {
                        out.push(Upd {
                            id,
                            s: v.s,
                            v: 0.0,
                            ev: Ev::Invalid,
                        });
                        prev = Some(v);
                        continue;
                    }
                    Some(ex) => {
                        open = gate_open(ctx.g, ctx.veh, ctx.st, ctx.signal, v, ex, node.s_out);
                        if open {
                            ahead_of_stop_line(ctx, node, ex)
                        } else {
                            Some((node.s_out + HARD_GAP, 0.0, HARD_GAP))
                        }
                    }
                },
            },
        };
        let (mut s, nv) = follow(v, node.speed, leader);
        let next = v.path.steps.get(v.hop as usize);
        match next {
            Some(_) if open && s >= node.s_out => ev = Ev::Cross,
            Some(_) => s = s.min(node.s_out),
            None => {
                let goal = v.path.goal_s.clamp(node.s_in, node.s_out);
                if s >= goal {
                    ev = Ev::Arrive;
                }
            }
        }
        out.push(Upd { id, s, v: nv, ev });
        prev = Some(v);
    }
}

/// The nearest thing past the stop line when the gate is open, in lane coordinates.
fn ahead_of_stop_line(ctx: &Ctx, node: &LaneNode, ex: &ExitConn) -> Option<Leader> {
    let fs = ctx.g.car_slot(node.id)?;
    if let Some(l) = ctx.st.conns.get(&(fs, ex.to_slot))
        && let Some(o) = l.last().and_then(|&id| ctx.veh.get(id))
    {
        return Some((node.s_out + o.s - CAR_LENGTH, o.speed, MIN_GAP));
    }
    let t = ctx.g.car[ex.to_slot as usize].as_ref()?;
    let o = ctx.st.lanes[ex.to_slot as usize]
        .last()
        .and_then(|&id| ctx.veh.get(id))?;
    Some((
        node.s_out + ex.length + o.s - CAR_LENGTH - t.s_in,
        o.speed,
        MIN_GAP,
    ))
}

fn step_conn(ctx: &Ctx, key: (u32, u32), out: &mut Vec<Upd>) {
    let list = &ctx.st.conns[&key];
    let Some(first) = list.first().and_then(|&id| ctx.veh.get(id)) else {
        return;
    };
    let Some((from, to)) = first.path.steps.get(first.hop as usize).copied() else {
        return;
    };
    let Some(ex) = find_exit(ctx.g, from, to) else {
        for &id in list {
            out.push(Upd {
                id,
                s: 0.0,
                v: 0.0,
                ev: Ev::Invalid,
            });
        }
        return;
    };
    let Some(target) = ctx.g.car[ex.to_slot as usize].as_ref() else {
        return;
    };
    let limit = ex.speed;
    let mut prev: Option<&Vehicle> = None;
    for &id in list {
        let Some(v) = ctx.veh.get(id) else { continue };
        let leader: Option<Leader> = match prev {
            Some(l) => Some((l.s - CAR_LENGTH, l.speed, MIN_GAP)),
            None => ctx.st.lanes[ex.to_slot as usize]
                .last()
                .and_then(|&id| ctx.veh.get(id))
                .map(|o| (ex.length + o.s - CAR_LENGTH - target.s_in, o.speed, MIN_GAP)),
        };
        let (s, nv) = follow(v, limit, leader);
        let ev = if prev.is_none() && s >= ex.length {
            Ev::Cross
        } else {
            Ev::Stay
        };
        out.push(Upd {
            id,
            s: if prev.is_some() { s.min(ex.length) } else { s },
            v: nv,
            ev,
        });
        prev = Some(v);
    }
}

fn step_place(ctx: &Ctx, p: Place, out: &mut Vec<Upd>) {
    match p {
        Place::Lane(slot) => step_lane(ctx, slot, out),
        Place::Conn(a, b) => step_conn(ctx, (a, b), out),
    }
}

/// Puts waiting vehicles on the road where there is room. Returns those that cannot ever
/// start (their start lane is gone).
pub(crate) fn spawn_waiting(
    vehicles: &mut Arena<Vehicle>,
    st: &mut DriveState,
    g: &PathGraph,
    finished: &mut Finished,
) {
    // Id order, whatever the order they arrived in: a load rebuilds the list from the arena.
    let mut waiting = std::mem::take(&mut st.waiting);
    waiting.sort_unstable();
    for id in waiting {
        let Some(v) = vehicles.get(id) else { continue };
        let Some(slot) = g.car_slot(v.path.start) else {
            if let Some(v) = vehicles.remove(id) {
                finished.invalid.push(v);
            }
            continue;
        };
        let node = g.car[slot as usize].as_ref().expect("slot");
        let s = v.path.start_s.clamp(node.s_in, node.s_out);
        let list = &st.lanes[slot as usize];
        // Index of the first vehicle behind `s`.
        let at = list.partition_point(|&o| vehicles.get(o).is_some_and(|o| o.s > s));
        let ahead_ok = at == 0
            || vehicles
                .get(list[at - 1])
                .is_some_and(|o| o.s - CAR_LENGTH - MIN_GAP >= s);
        let behind_ok = at == list.len()
            || vehicles
                .get(list[at])
                .is_some_and(|o| o.s + CAR_LENGTH + MIN_GAP <= s);
        if ahead_ok && behind_ok {
            st.lanes[slot as usize].insert(at, id);
            let v = vehicles.get_mut(id).expect("vehicle");
            v.spawned = true;
            v.s = s;
            v.speed = 0.0;
            v.stuck_timer = 0;
        } else {
            st.waiting.push(id);
        }
    }
}

/// Is the front car `v` of its lane held by something that should not last: no room beyond the
/// junction, or a conflicting car that itself stands? A red light, or yielding to a car that
/// is moving, is legitimate waiting.
fn front_suspicious(
    g: &PathGraph,
    veh: &Arena<Vehicle>,
    st: &DriveState,
    signal: u64,
    v: &Vehicle,
    node: &LaneNode,
    depth: u32,
) -> bool {
    let Some((_, to)) = v.path.steps.get(v.hop as usize).copied() else {
        return false;
    };
    let Some(ex) = find_exit(g, node.id, to) else {
        return false;
    };
    if gate_open(g, veh, st, signal, v, ex, node.s_out) {
        return false;
    }
    if ex.light != u32::MAX
        && g.lights[ex.light as usize].connection_signal(ex.index, signal) != Signal::Green
    {
        return false;
    }
    let (mut legit, mut stuck_behind) = (false, false);
    for c in &ex.conflicts {
        let inside = st.conns.get(&(c.from_slot, c.to.index));
        if let Some(l) = inside.filter(|l| !l.is_empty()) {
            let standing = l
                .iter()
                .any(|&id| veh.get(id).is_some_and(|o| o.speed < 0.1));
            legit |= !standing;
            stuck_behind |= standing;
        }
        if c.higher
            && let Some(n) = g.car[c.from_slot as usize].as_ref()
            && let Some(o) = st.lanes[c.from_slot as usize]
                .first()
                .and_then(|&id| veh.get(id))
            && n.s_out - o.s <= PRIORITY_ZONE
            && o.path
                .steps
                .get(o.hop as usize)
                .is_some_and(|x| x.1 == c.to)
        {
            legit |= o.speed >= 0.1;
            stuck_behind |= o.speed < 0.1;
        }
    }
    if stuck_behind {
        return true;
    }
    if legit || has_room(g, veh, st, ex.to_slot, 0) {
        return false;
    }
    // No room beyond the junction: gridlock, unless the lane ahead is only backed up behind a
    // legitimate wait (a red light further on). A ring of full lanes runs out of depth.
    let Some(front) = st.lanes[ex.to_slot as usize]
        .first()
        .and_then(|&id| veh.get(id))
    else {
        return false;
    };
    if front.speed >= 0.1 {
        return false;
    }
    match g.car[ex.to_slot as usize].as_ref() {
        Some(t) if depth < LEADER_CHAIN => {
            front_suspicious(g, veh, st, signal, front, t, depth + 1)
        }
        _ => true,
    }
}

/// For every standing vehicle: is it standing for a suspicious reason? Front to back per place,
/// one pass; a car behind a standing leader inherits the leader's reason.
fn suspicious_flags(
    veh: &Arena<Vehicle>,
    st: &DriveState,
    g: &PathGraph,
    signal: u64,
) -> Vec<(VehicleId, bool)> {
    let mut out = Vec::with_capacity(veh.len());
    let mut walk = |list: &[VehicleId], front: Option<bool>| {
        let mut lead: Option<bool> = None;
        for (i, &id) in list.iter().enumerate() {
            let Some(v) = veh.get(id) else { continue };
            let standing = v.speed < 0.1;
            let sus = match (i, lead) {
                (0, _) => standing && front.unwrap_or(false),
                (_, Some(l)) => standing && l,
                (_, None) => false,
            };
            out.push((id, sus));
            lead = standing.then_some(sus);
        }
    };
    for (slot, list) in st.lanes.iter().enumerate() {
        let Some(&first) = list.first() else { continue };
        let front = match (veh.get(first), g.car[slot].as_ref()) {
            (Some(v), Some(node)) if v.speed < 0.1 => {
                Some(front_suspicious(g, veh, st, signal, v, node, 0))
            }
            _ => Some(false),
        };
        walk(list, front);
    }
    for list in st.conns.values() {
        // A car standing inside a junction waits for room beyond it.
        walk(list, Some(true));
    }
    for &id in &st.waiting {
        if veh.contains(id) {
            out.push((id, false));
        }
    }
    out
}

/// One tick of driving: [`MOVE_SUBSTEPS`] substeps. `pool` runs the place chunks in parallel
/// (the result is the same for any pool size, or none).
pub(crate) fn drive_tick(
    vehicles: &mut Arena<Vehicle>,
    st: &mut DriveState,
    g: &PathGraph,
    tick: u64,
    pool: Option<&TaskPool>,
    stuck_ticks: u32,
) -> Finished {
    let mut finished = Finished::default();
    spawn_waiting(vehicles, st, g, &mut finished);
    for sub in 0..MOVE_SUBSTEPS {
        substep(vehicles, st, g, signal_tick(tick, sub), pool, &mut finished);
    }
    // Stuck timers (§9.4 v1.1): only a standstill for a suspicious reason counts.
    let flags = suspicious_flags(vehicles, st, g, signal_tick(tick, MOVE_SUBSTEPS - 1));
    let mut stuck = Vec::new();
    for (id, suspicious) in flags {
        let v = vehicles.get_mut(id).expect("flagged vehicle");
        if v.speed >= 0.1 {
            v.stuck_timer = 0;
            continue;
        }
        let (still, sus) = (v.stuck_timer >> 16, v.stuck_timer & 0xFFFF);
        let still = (still + 1).min(0xFFFF);
        let sus = if suspicious { (sus + 1).min(0xFFFF) } else { 0 };
        v.stuck_timer = still << 16 | sus;
        if sus > stuck_ticks || still > stuck_ticks.saturating_mul(HARD_CAP_FACTOR) {
            stuck.push(id);
        }
    }
    for id in stuck {
        if let Some(v) = vehicles.remove(id) {
            st.remove(&v, id);
            finished.stuck.push(v);
        }
    }
    finished
}

/// One substep at plan time `signal`.
pub(crate) fn substep(
    vehicles: &mut Arena<Vehicle>,
    st: &mut DriveState,
    g: &PathGraph,
    signal: u64,
    pool: Option<&TaskPool>,
    finished: &mut Finished,
) {
    let mut places: Vec<Place> = Vec::new();
    for (slot, l) in st.lanes.iter().enumerate() {
        if !l.is_empty() {
            places.push(Place::Lane(slot as u32));
        }
    }
    for (&(a, b), l) in &st.conns {
        if !l.is_empty() {
            places.push(Place::Conn(a, b));
        }
    }
    let ctx = Ctx {
        g,
        veh: vehicles,
        st,
        signal,
    };
    let updates: Vec<Upd> = match pool {
        Some(pool) if vehicles.len() >= PARALLEL_MIN => {
            let chunks: Vec<Vec<Upd>> = pool.scope(|scope| {
                for chunk in places.chunks(CHUNK) {
                    let ctx = &ctx;
                    scope.spawn(async move {
                        let mut out = Vec::new();
                        for &p in chunk {
                            step_place(ctx, p, &mut out);
                        }
                        out
                    });
                }
            });
            chunks.into_iter().flatten().collect()
        }
        _ => {
            let mut out = Vec::new();
            for &p in &places {
                step_place(&ctx, p, &mut out);
            }
            out
        }
    };
    apply(vehicles, st, g, signal, updates, finished);
}

fn apply(
    vehicles: &mut Arena<Vehicle>,
    st: &mut DriveState,
    g: &PathGraph,
    signal: u64,
    updates: Vec<Upd>,
    finished: &mut Finished,
) {
    for u in updates {
        let Some(v) = vehicles.get(u.id) else {
            continue;
        };
        match u.ev {
            Ev::Stay => {
                let v = vehicles.get_mut(u.id).expect("vehicle");
                v.s = u.s;
                v.speed = u.v;
            }
            Ev::Arrive => {
                if let Some(v) = vehicles.remove(u.id) {
                    st.remove(&v, u.id);
                    finished.arrived.push(v);
                }
            }
            Ev::Invalid => {
                if let Some(v) = vehicles.remove(u.id) {
                    st.remove(&v, u.id);
                    finished.invalid.push(v);
                }
            }
            Ev::Cross if !v.in_conn => {
                let (from, to) = v.path.steps[v.hop as usize];
                let (Some(fs), Some(ex), Some(node)) =
                    (g.car_slot(from), find_exit(g, from, to), lane_node(g, from))
                else {
                    drop_invalid(vehicles, st, u.id, finished);
                    continue;
                };
                // Live check: another car may have taken the junction in this substep.
                let open = gate_open(g, vehicles, st, signal, v, ex, node.s_out);
                let v = vehicles.get_mut(u.id).expect("vehicle");
                if !open {
                    v.s = (node.s_out - 0.01).max(v.s.min(node.s_out - 0.01));
                    v.speed = 0.0;
                    continue;
                }
                let over = (u.s - node.s_out).clamp(0.0, ex.length);
                v.s = over;
                v.speed = u.v;
                v.in_conn = true;
                st.lanes[fs as usize].retain(|w| *w != u.id);
                st.conns.entry((fs, ex.to_slot)).or_default().push(u.id);
                st.entering[ex.to_slot as usize] += 1;
            }
            Ev::Cross => {
                let (from, to) = v.path.steps[v.hop as usize];
                let (Some(fs), Some(ex)) = (g.car_slot(from), find_exit(g, from, to)) else {
                    drop_invalid(vehicles, st, u.id, finished);
                    continue;
                };
                let t = g.car[ex.to_slot as usize].as_ref().expect("target lane");
                let over = (u.s - ex.length).max(0.0);
                let s_new = t.s_in + over;
                let clear = st.lanes[ex.to_slot as usize]
                    .last()
                    .and_then(|&id| vehicles.get(id))
                    .is_none_or(|o| s_new <= o.s - CAR_LENGTH - HARD_GAP);
                let v = vehicles.get_mut(u.id).expect("vehicle");
                if !clear {
                    v.s = (ex.length - 0.01).max(0.0);
                    v.speed = 0.0;
                    continue;
                }
                v.s = s_new;
                v.speed = u.v;
                v.in_conn = false;
                v.hop += 1;
                let key = (fs, ex.to_slot);
                if let Some(l) = st.conns.get_mut(&key) {
                    l.retain(|w| *w != u.id);
                    if l.is_empty() {
                        st.conns.remove(&key);
                    }
                }
                st.entering[ex.to_slot as usize] =
                    st.entering[ex.to_slot as usize].saturating_sub(1);
                st.lanes[ex.to_slot as usize].push(u.id);
            }
        }
    }
}

fn drop_invalid(
    vehicles: &mut Arena<Vehicle>,
    st: &mut DriveState,
    id: VehicleId,
    finished: &mut Finished,
) {
    if let Some(v) = vehicles.remove(id) {
        st.remove(&v, id);
        finished.invalid.push(v);
    }
}
