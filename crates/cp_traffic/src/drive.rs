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
//! **Lane changes.** A path step `(a, b)` that is not a junction connection but joins two
//! neighbouring lanes of one segment is a lateral hop: before each substep a car whose next
//! step is a hop moves to the target lane at the same `s` when that lane has room there
//! (no car within a car length plus the standstill gap), else it keeps driving and retries.
//! Cars that have just left the target lane into a junction connection are not in its list but
//! still cover its stop line, so they count as well ([`hop_blockers`], BUG-020): a hopper at the
//! stop line would otherwise enter another connection of the lane on top of them. A car whose
//! next connection leaves a lane that a sibling connection's car has not yet cleared waits
//! (`gate_open`). A hop held shut only by moving cars is not a suspicious wait.
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
use crate::astar::{CarPath, find_class_path_lanes};
use cp_content::VehicleClass;

/// The lane permissions class of a vehicle (T-302), from its kind byte.
pub fn class_of(v: &Vehicle) -> VehicleClass {
    cp_content::VehicleKind::ALL
        .get(usize::from(v.kind_id()))
        .map_or(VehicleClass::Car, |k| k.class())
}
use crate::graph::{CongestionMap, ExitConn, LaneNode, PathGraph};

/// Seconds of movement per substep.
pub const DT: f32 = MOVE_SECONDS_PER_TICK / MOVE_SUBSTEPS as f32;
/// Default vehicle length, metres: only for runs without content (tests); the game reads
/// `VehicleDef::length` ([`VehicleSpecs`]).
pub const CAR_LENGTH: f32 = 4.2;
/// Default standstill bumper-to-bumper gap, metres (see [`CAR_LENGTH`]).
pub const MIN_GAP: f32 = 2.0;
/// Closest two cars may ever get, metres (hard clamp).
pub const HARD_GAP: f32 = 0.5;
/// Default acceleration, m/s² (§9.4; see [`CAR_LENGTH`]).
pub const ACCEL: f32 = 2.0;
/// Default braking, m/s² (§9.4; see [`CAR_LENGTH`]).
pub const DECEL: f32 = 4.0;
/// Reaction time in the safe-speed formula, seconds.
pub const REACTION: f32 = 1.0;
/// Distance before a stop line inside which a car with priority blocks yielding traffic.
pub const PRIORITY_ZONE: f32 = 30.0;
/// Seconds of movement standing for a suspicious reason (no room beyond the junction, or behind
/// such a car) before a car is removed (§9.4). A red light or yielding does not count.
pub const STUCK_SECONDS: f32 = 120.0;
/// [`STUCK_SECONDS`] on the movement clock (T-270: in ticks it follows `MOVE_SECONDS_PER_TICK`).
pub const STUCK_TICKS: u32 = (STUCK_SECONDS / MOVE_SECONDS_PER_TICK) as u32;
/// Hard cap on a standstill for any reason, in multiples of the stuck threshold (§9.4 v1.1).
pub const HARD_CAP_FACTOR: u32 = 10;
/// A car waiting to enter the road gives up after this many times the hard cap (BUG-072b).
const WAITING_CAP_FACTOR: u32 = 6;
/// How many full lanes ahead the suspicion of a blocked car is followed. 16 (BUG-078, was 8): in
/// a 12-street lattice (18 000 residents) queues behind a red light run longer than 8 lanes, and
/// the front car of each was taken for a gridlock ring and removed (57 of 86 a day); 16 and 32
/// remove the same 21, so no ring is that long.
const LEADER_CHAIN: u32 = 16;
/// Metres a keep vehicle stops short of its goal point (the front bumper), and the slack within
/// which it counts as there.
const KEEP_STOP_GAP: f32 = 0.3;
/// Vehicles per parallel chunk of places.
const CHUNK: usize = 64;
/// Fewer vehicles than this are stepped on the calling thread.
const PARALLEL_MIN: usize = 512;

/// Dimensions and dynamics of one vehicle kind, from its `VehicleDef`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VehicleSpec {
    /// Bumper to bumper, metres.
    pub length: f32,
    /// Standstill gap to the car ahead, metres.
    pub min_gap: f32,
    /// Acceleration, m/s².
    pub accel: f32,
    /// Braking, m/s².
    pub decel: f32,
    /// Top speed, m/s.
    pub max_speed: f32,
}

impl Default for VehicleSpec {
    /// The stand-in for runs without content.
    fn default() -> Self {
        Self {
            length: CAR_LENGTH,
            min_gap: MIN_GAP,
            accel: ACCEL,
            decel: DECEL,
            max_speed: f32::MAX,
        }
    }
}

/// [`VehicleSpec`] per `cp_content::VehicleKind` number (`Vehicle::kind`): the first definition
/// of a kind, in id order, sets it (same rule as the render looks); a kind without a definition
/// keeps the default.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VehicleSpecs {
    by_kind: [VehicleSpec; VEHICLE_KINDS],
}

/// Number of `cp_content::VehicleKind` variants.
pub const VEHICLE_KINDS: usize = cp_content::VehicleKind::COUNT;

impl VehicleSpecs {
    pub fn from_content(db: &cp_content::ContentDb) -> Self {
        let mut specs = Self::default();
        let mut seen = [false; VEHICLE_KINDS];
        for (_, def) in db.vehicles() {
            let k = def.kind as usize;
            if k < VEHICLE_KINDS && !seen[k] {
                seen[k] = true;
                specs.by_kind[k] = VehicleSpec {
                    length: def.length,
                    min_gap: def.min_gap,
                    accel: def.accel,
                    decel: def.decel,
                    max_speed: def.max_speed_kmh / 3.6,
                };
            }
        }
        specs
    }

    /// The spec of `v`'s kind.
    pub fn of(&self, v: &Vehicle) -> &VehicleSpec {
        let kind = usize::from(v.kind_id());
        debug_assert!(kind < VEHICLE_KINDS, "vehicle kind {kind} has no spec");
        &self.by_kind[kind.min(VEHICLE_KINDS - 1)]
    }
}

/// Plan ticks (50 ms of movement) in one substep: [`DT`] / 0.05 (T-270: 2 at 0.1 s, was 10).
const PLAN_TICKS_PER_SUBSTEP: u64 = (DT * 20.0 + 0.5) as u64;

/// Plan time of the signal at `substep` of `tick`: the plan counts 50 ms of movement, one
/// substep is [`DT`] seconds of movement.
pub fn signal_tick(tick: u64, substep: u32) -> u64 {
    (tick * u64::from(MOVE_SUBSTEPS) + u64::from(substep)) * PLAN_TICKS_PER_SUBSTEP
}

/// Is `(from, to)` a lateral hop between two lanes of one segment (not a junction connection)?
pub(crate) fn is_lane_change(g: &PathGraph, from: LaneId, to: LaneId) -> bool {
    match (lane_node(g, from), lane_node(g, to)) {
        (Some(a), Some(b)) => a.segment == b.segment && find_exit(g, from, to).is_none(),
        _ => false,
    }
}

/// Whether the rest of the vehicle's route still exists in `g`: every lane ahead, every junction
/// connection (or lane change) between them, and the destination lane. The place it is on
/// now is not checked here (a missing place removes the car, see [`DriveState::rebuild`]).
pub(crate) fn path_ok(g: &PathGraph, v: &Vehicle) -> bool {
    let ahead = usize::from(v.hop) + usize::from(v.in_conn);
    g.car_slot(v.path.goal).is_some()
        && v.path.steps.iter().skip(ahead).all(|&(a, b)| {
            g.car_slot(a).is_some()
                && g.car_slot(b).is_some()
                && (find_exit(g, a, b).is_some() || is_lane_change(g, a, b))
        })
}

/// Cars that keep a hop of `me` to lane `to` at position `s` shut: cars of that lane too close
/// for either bumper gap, and cars that have just left it into a junction connection and still
/// cover its stop line.
fn hop_blockers<'a>(
    g: &PathGraph,
    veh: &'a Arena<Vehicle>,
    st: &DriveState,
    me: &Vehicle,
    to: LaneId,
    s: f32,
) -> Vec<&'a Vehicle> {
    let (Some(ts), Some(t)) = (
        g.car_slot(to),
        g.car_slot(to).and_then(|sl| g.car[sl as usize].as_ref()),
    ) else {
        return Vec::new();
    };
    let s = s.clamp(t.s_in, t.s_out);
    let mine = st.specs.of(me);
    let mut out: Vec<&Vehicle> = st.lanes[ts as usize]
        .iter()
        .filter_map(|&o| veh.get(o))
        .filter(|o| {
            let theirs = st.specs.of(o);
            if o.s >= s {
                // Ahead: its rear against my front.
                o.s - theirs.length - s < mine.min_gap
            } else {
                // Behind: my rear against its front.
                s - mine.length - o.s < theirs.min_gap
            }
        })
        .collect();
    out.extend(left_the_lane(veh, st, ts, t.s_out, s, mine.min_gap));
    out
}

/// Cars that have left lane slot `slot` (stop line at `s_out`) into a junction connection and
/// whose rear still comes closer than `gap` to position `s` on the lane.
fn left_the_lane<'v, 's>(
    veh: &'v Arena<Vehicle>,
    st: &'s DriveState,
    slot: u32,
    s_out: f32,
    s: f32,
    gap: f32,
) -> impl Iterator<Item = &'v Vehicle> + 's
where
    'v: 's,
{
    let none = st.leaving.get(slot as usize).is_none_or(|&n| n == 0);
    let range = if none {
        st.conns.range((0, 0)..(0, 0))
    } else {
        st.conns.range((slot, 0)..=(slot, u32::MAX))
    };
    range
        .flat_map(|(_, l)| l.iter())
        .filter_map(move |&o| veh.get(o))
        .filter(move |o| s_out + o.s - st.specs.of(o).length < s + gap)
}

/// Is the next step of `v` a lateral hop to a neighbouring lane?
fn pending_hop(g: &PathGraph, v: &Vehicle) -> bool {
    v.path
        .steps
        .get(usize::from(v.hop))
        .is_some_and(|&(from, to)| is_lane_change(g, from, to))
}

impl DriveState {
    /// Recomputes [`DriveState::hop_lanes`] from the cars on the lanes.
    fn refresh_hop_lanes(
        &mut self,
        vehicles: &Arena<Vehicle>,
        g: &PathGraph,
        pool: Option<&TaskPool>,
    ) {
        let slots: Vec<usize> = self.occ.iter().collect();
        let lanes = &self.lanes;
        let hops = par_map(pool, &slots, |&slot| {
            lanes[slot]
                .iter()
                .any(|&id| vehicles.get(id).is_some_and(|v| pending_hop(g, v)))
        });
        self.hop_lanes = OccBits::with_len(self.lanes.len());
        for (slot, hop) in slots.into_iter().zip(hops) {
            if hop {
                self.hop_lanes.set(slot, true);
            }
        }
    }
}

/// Performs the lateral hops that have room, in lane-slot then queue order (deterministic).
fn lane_changes(vehicles: &mut Arena<Vehicle>, st: &mut DriveState, g: &PathGraph) {
    // Lanes with a hop pending in ascending slot order, looking at the live marks: a hop can
    // make a later lane one of them, and it is then visited like any other. (A lane without a
    // pending hop has nothing to do, so skipping it changes nothing.)
    let mut next = st.hop_lanes.next_from(0);
    while let Some(slot) = next {
        next = st.hop_lanes.next_from(slot + 1);
        // (A lane visited later may have been emptied by a hop in the meantime.)
        if st.lanes[slot].is_empty() {
            st.hop_lanes.set(slot, false);
            continue;
        }
        let ids = st.lanes[slot].clone();
        for id in ids {
            while let Some(v) = vehicles.get(id) {
                let Some((from, to)) = v.path.steps.get(usize::from(v.hop)).copied() else {
                    break;
                };
                if !is_lane_change(g, from, to) {
                    break;
                }
                let (Some(fs), Some(ts)) = (g.car_slot(from), g.car_slot(to)) else {
                    break;
                };
                let t = g.car[ts as usize].as_ref().expect("lane");
                let s = v.s.clamp(t.s_in, t.s_out);
                if !hop_blockers(g, vehicles, st, v, to, s).is_empty() {
                    // Standing at the stop line with no gap to change lanes: turn from this lane
                    // when it has an exit to the same street (BUG-090), else wait; after a long
                    // wait, take another way.
                    let at_end = g.car[fs as usize]
                        .as_ref()
                        .is_some_and(|f| v.s >= f.s_out - LANE_END_M && v.speed < 0.1)
                        && v.stuck_timer >> 16 > TURN_WAIT_TICKS;
                    let head = st.lanes[fs as usize].first() == Some(&id);
                    if at_end
                        && head
                        && let Some(steps) = turn_from_here(
                            g,
                            class_of(v),
                            &v.path,
                            usize::from(v.hop),
                            v.stuck_timer >> 16 > STUCK_TICKS / 2,
                        )
                    {
                        vehicles.get_mut(id).expect("vehicle").path.steps = steps;
                        continue;
                    }
                    break;
                }
                st.lane_remove(fs, id);
                let at = st.lanes[ts as usize]
                    .partition_point(|&o| vehicles.get(o).is_some_and(|o| o.s > s));
                st.lane_insert(ts, at, id);
                let v = vehicles.get_mut(id).expect("vehicle");
                v.s = s;
                v.hop += 1;
                if pending_hop(g, v) {
                    st.hop_lanes.set(ts as usize, true);
                }
            }
        }
        // Still a car waiting for room to hop? Then the lane stays on the list.
        let pending = st.lanes[slot]
            .iter()
            .any(|&id| vehicles.get(id).is_some_and(|v| pending_hop(g, v)));
        st.hop_lanes.set(slot, pending);
    }
}

/// How near the stop line (metres) a car waiting for a lane change takes the turn from its own
/// lane instead (BUG-090).
const LANE_END_M: f32 = 1.0;

/// How long (ticks standing) a car waits at the stop line for a gap to change lanes before it
/// turns from its own lane (BUG-090): a red light or a short queue beside it is waited out, so
/// the cars keep their routes and lanes.
const TURN_WAIT_TICKS: u32 = STUCK_TICKS / 4;

/// The route of `path` with the lane change(s) at `hop` replaced by the turn from the lane the car
/// is on (BUG-090: on a wide road a car that met its lane change too late, behind a queue in the
/// lane it wants, waited at the stop line until it was removed as stuck). When that lane has an
/// exit into the street the route turns into, the car takes it (and changes to the planned lane
/// on that street if it lands in another one). When it has no such exit and `reroute` (the car
/// has stood for half of [`STUCK_TICKS`]), it takes the exit of its lane with the quickest way on
/// to its goal; a shorter wait keeps the route, the queue beside it usually moves first (a car
/// that starts near the junction in a lane it must leave starts in the right one instead, see
/// [`spawn_waiting`]).
fn turn_from_here(
    g: &PathGraph,
    class: VehicleClass,
    path: &CarPath,
    hop: usize,
    reroute: bool,
) -> Option<Vec<(LaneId, LaneId)>> {
    let steps = &path.steps;
    let (from, _) = *steps.get(hop)?;
    // The junction exit the lane changes lead to.
    let k = (hop..steps.len()).find(|&i| !is_lane_change(g, steps[i].0, steps[i].1))?;
    let (_, planned) = steps[k];
    let street = lane_node(g, planned)?.segment;
    let slot = g.car_slot(from)?;
    let exits = &g.exits[slot as usize];
    let mut out = steps[..hop].to_vec();
    if let Some(exit) = exits
        .iter()
        .filter(|e| lane_node(g, e.to).is_some_and(|n| n.segment == street))
        .min_by_key(|e| (e.to != planned, e.to.index))
    {
        out.push((from, exit.to));
        if exit.to != planned {
            out.push((exit.to, planned));
        }
        out.extend_from_slice(&steps[k + 1..]);
        return Some(out);
    }
    if !reroute {
        return None;
    }
    // Another way: from every exit of this lane, the quickest route on to the goal.
    let cong = CongestionMap::default();
    let (exit, rest) = exits
        .iter()
        .filter_map(|e| {
            let n = lane_node(g, e.to)?;
            let p =
                find_class_path_lanes(g, &cong, class, (e.to, n.s_in), (path.goal, path.goal_s))
                    .ok()?;
            Some((e.length + p.cost, e.to.index, e, p))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
        .map(|(_, _, e, p)| (e, p))?;
    out.push((from, exit.to));
    out.extend(rest.steps);
    Some(out)
}

/// Which lane slots hold a car, as a bitset: the passes of a substep visit the few hundred busy
/// lanes of a 60k-lane network, not every lane.
#[derive(Clone, Debug, Default)]
pub(crate) struct OccBits(Vec<u64>);

impl OccBits {
    fn with_len(n: usize) -> Self {
        Self(vec![0; n.div_ceil(64)])
    }

    fn set(&mut self, i: usize, on: bool) {
        let (w, b) = (i / 64, i % 64);
        if w < self.0.len() {
            if on {
                self.0[w] |= 1 << b;
            } else {
                self.0[w] &= !(1 << b);
            }
        }
    }

    /// The first set slot `>= from`.
    fn next_from(&self, from: usize) -> Option<usize> {
        let mut w = from / 64;
        if w >= self.0.len() {
            return None;
        }
        let mut bits = self.0[w] & (!0u64 << (from % 64));
        loop {
            if bits != 0 {
                return Some(w * 64 + bits.trailing_zeros() as usize);
            }
            w += 1;
            if w >= self.0.len() {
                return None;
            }
            bits = self.0[w];
        }
    }

    /// Set slots in ascending order.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        let mut next = self.next_from(0);
        std::iter::from_fn(move || {
            let cur = next?;
            next = self.next_from(cur + 1);
            Some(cur)
        })
    }
}

/// Derived occupancy of lanes and connections.
#[derive(Clone, Debug, Default)]
pub(crate) struct DriveState {
    /// Lane slots with at least one car (kept in step with `lanes` by the `lane_*` methods).
    pub occ: OccBits,
    /// Lane slots that may hold a car whose next step is a lateral hop: the only lanes
    /// [`lane_changes`] has to look at. Refreshed from the cars at the start of every tick
    /// (routes can be replaced between ticks) and marked when a car enters a lane with a hop
    /// pending during one; a lane whose cars have no hop left is dropped when visited.
    pub hop_lanes: OccBits,
    /// Vehicles per lane slot, front-most first.
    pub lanes: Vec<Vec<VehicleId>>,
    /// Vehicles per connection `(from slot, to slot)`, front-most first.
    pub conns: BTreeMap<(u32, u32), Vec<VehicleId>>,
    /// Vehicles inside a junction heading for each lane slot.
    pub entering: Vec<u16>,
    /// Vehicles inside a junction that came from each lane slot (T-256: most lanes have none,
    /// and then the `conns` range of the slot need not be searched).
    pub leaving: Vec<u16>,
    /// Vehicles not yet on the road (waiting for room at their start), in id order.
    pub waiting: Vec<VehicleId>,
    pub ready: bool,
    pub generation: u64,
    /// Vehicle dimensions and dynamics of the current tick (set by [`drive_tick`]).
    pub specs: VehicleSpecs,
    /// Why the stuck cars were removed, by [`StuckWhy`] (diagnostics; not saved).
    pub stuck_why: [u64; STUCK_WHY_N],
}

/// Number of [`StuckWhy`] reasons.
pub const STUCK_WHY_N: usize = 8;

/// Why a stuck car was removed (BUG-078 diagnostics).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StuckWhy {
    /// Never got on the road (waited for a gap at its start).
    Waiting = 0,
    /// Standing inside a junction (no room beyond).
    InJunction = 1,
    /// Front of its lane, held by a lane change that finds no gap.
    LaneChange = 2,
    /// Front of its lane, a conflicting car that itself stands is in the way.
    ConflictStands = 3,
    /// Front of its lane, no room in the lane beyond the junction.
    FullBeyond = 4,
    /// Front of its lane, something else (red/yield chain).
    FrontOther = 5,
    /// Hard cap on any standstill, not the head of a line.
    HardCap = 6,
    /// Hard cap at the head of a line.
    HardCapHead = 7,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ev {
    Stay,
    /// Leaves its lane/connection forwards.
    Cross,
    Arrive,
    /// A keep vehicle reached its goal: it stays there (T-154).
    Park,
    Invalid,
}

struct Upd {
    id: VehicleId,
    s: f32,
    v: f32,
    ev: Ev,
}

/// Adds `u` for `v`, unless it is a plain move to exactly where `v` already is at the speed it
/// already has (a standing car): applying it would change nothing, and the sequential apply
/// is bound by the writes to the vehicles (T-259).
fn push_upd(out: &mut Vec<Upd>, v: &Vehicle, u: Upd) {
    if u.ev == Ev::Stay && u.s.to_bits() == v.s.to_bits() && u.v.to_bits() == v.speed.to_bits() {
        return;
    }
    out.push(u);
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
    /// Keep vehicles that stopped at their goal this tick.
    pub parked: Vec<VehicleId>,
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
    fn lane_push(&mut self, slot: u32, id: VehicleId) {
        self.lanes[slot as usize].push(id);
        self.occ.set(slot as usize, true);
    }

    fn lane_insert(&mut self, slot: u32, at: usize, id: VehicleId) {
        self.lanes[slot as usize].insert(at, id);
        self.occ.set(slot as usize, true);
    }

    fn lane_remove(&mut self, slot: u32, id: VehicleId) {
        let l = &mut self.lanes[slot as usize];
        l.retain(|w| *w != id);
        if l.is_empty() {
            self.occ.set(slot as usize, false);
        }
    }

    /// Rebuilds the occupancy from the vehicles; returns those whose place no longer exists.
    pub fn rebuild(
        &mut self,
        vehicles: &Arena<Vehicle>,
        g: &PathGraph,
        generation: u64,
    ) -> Vec<VehicleId> {
        let n = g.car.len();
        self.lanes = vec![Vec::new(); n];
        self.occ = OccBits::with_len(n);
        self.hop_lanes = OccBits::with_len(n);
        self.conns.clear();
        self.entering = vec![0; n];
        self.leaving = vec![0; n];
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
                        self.leaving[fs as usize] += 1;
                    }
                    None => invalid.push(id),
                }
            } else {
                match g.car_slot(v.lane()) {
                    Some(slot) => self.lane_push(slot, id),
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
                    let before = l.len();
                    l.retain(|w| *w != id);
                    let removed = l.len() < before;
                    if l.is_empty() {
                        self.conns.remove(&key);
                    }
                    if let Some(e) = self.entering.get_mut(to.index as usize) {
                        *e = e.saturating_sub(1);
                    }
                    if removed && let Some(e) = self.leaving.get_mut(from.index as usize) {
                        *e = e.saturating_sub(1);
                    }
                }
            }
        } else if (v.lane().index as usize) < self.lanes.len() {
            self.lane_remove(v.lane().index, id);
        }
    }
}

/// Front vehicle's view of what lies ahead: `(rear bumper position, speed, standstill gap)`
/// in the coordinates of the place it is on.
type Leader = (f32, f32, f32);

fn follow(v: &Vehicle, spec: &VehicleSpec, limit: f32, leader: Option<Leader>) -> (f32, f32) {
    let (accel, decel) = (spec.accel, spec.decel);
    let mut vt = (v.speed + accel * DT).min(limit.min(spec.max_speed));
    if let Some((rear, vl, g0)) = leader {
        let gap = (rear - v.s - g0).max(0.0);
        let bt = decel * REACTION;
        let safe = -bt + (bt * bt + vl * vl + 2.0 * decel * gap).sqrt();
        vt = vt.min(safe);
    }
    vt = vt.max(v.speed - decel * DT).max(0.0);
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
fn has_room(
    g: &PathGraph,
    veh: &Arena<Vehicle>,
    st: &DriveState,
    me: &Vehicle,
    slot: u32,
    extra: u16,
) -> bool {
    let Some(t) = g.car[slot as usize].as_ref() else {
        return false;
    };
    let span = t.s_out - t.s_in;
    let free = match st.lanes[slot as usize].last().and_then(|&id| veh.get(id)) {
        Some(o) => o.s - st.specs.of(o).length - t.s_in,
        None => span,
    };
    // Room for `me`, and for each car already on its way, taken to be like `me`.
    let slot_len = {
        let m = st.specs.of(me);
        m.length + m.min_gap
    };
    let need = slot_len.min(0.5 * span);
    free - f32::from(st.entering[slot as usize] + extra) * slot_len >= need
}

/// May the front vehicle `v` of its lane (`s_out` = its stop line) enter connection `ex`?
/// Whether the front car `o` of a lane could enter its junction now as far as room beyond it
/// goes. A car with priority that has no room to go on holds nobody (BUG-072): letting the cars
/// it crosses wait for it only builds a ring of waiting cars when the lane ahead of it is full.
fn could_enter(
    g: &PathGraph,
    veh: &Arena<Vehicle>,
    st: &DriveState,
    o: &Vehicle,
    node: &LaneNode,
    to: LaneId,
) -> bool {
    o.speed >= 0.1
        || find_exit(g, node.id, to).is_none_or(|e| has_room(g, veh, st, o, e.to_slot, 0))
}

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
                if v.speed * v.speed / (2.0 * st.specs.of(v).decel) <= s_out - v.s {
                    return false;
                }
            }
            Signal::Red => return false,
        }
    }
    // A car that left this lane into another connection a moment ago is still on the same spot:
    // wait until its rear has cleared the stop line by the standstill gap.
    if let Some(from_slot) = g.car_slot(v.lane())
        && st.leaving.get(from_slot as usize).is_some_and(|&n| n > 0)
    {
        let sibling_in_the_way = st
            .conns
            .range((from_slot, 0)..=(from_slot, u32::MAX))
            .filter(|((_, to), _)| *to != ex.to_slot)
            .flat_map(|(_, l)| l.iter())
            .any(|&o| {
                veh.get(o)
                    .is_some_and(|o| o.s - st.specs.of(o).length < st.specs.of(v).min_gap)
            });
        if sibling_in_the_way {
            return false;
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
                && could_enter(g, veh, st, o, node, c.to)
            {
                return false;
            }
        }
    }
    has_room(g, veh, st, v, ex.to_slot, 0)
}

fn spec_of<'a>(ctx: &'a Ctx, v: &Vehicle) -> &'a VehicleSpec {
    ctx.st.specs.of(v)
}

fn step_lane(ctx: &Ctx, slot: u32, out: &mut Vec<Upd>) {
    let list = &ctx.st.lanes[slot as usize];
    let Some(node) = ctx.g.car[slot as usize].as_ref() else {
        return;
    };
    let mut prev: Option<&Vehicle> = None;
    for &id in list {
        let Some(v) = ctx.veh.get(id) else { continue };
        if v.is_parked() {
            // Stands at its goal until a run continues it; it still holds its place.
            push_upd(
                out,
                v,
                Upd {
                    id,
                    s: v.s,
                    v: 0.0,
                    ev: Ev::Stay,
                },
            );
            prev = Some(v);
            continue;
        }
        let mut ev = Ev::Stay;
        let mut open = false;
        let leader: Option<Leader> = match prev {
            Some(l) => Some((
                l.s - spec_of(ctx, l).length,
                l.speed,
                spec_of(ctx, v).min_gap,
            )),
            None => match v.path.steps.get(v.hop as usize).copied() {
                None => None,
                Some((_, to)) => match find_exit(ctx.g, node.id, to) {
                    None if is_lane_change(ctx.g, node.id, to) => {
                        // Waiting for a gap in the next lane: stop at the stop line.
                        Some((node.s_out + HARD_GAP, 0.0, HARD_GAP))
                    }
                    // The connection is gone but a new route is on its way: wait at the stop
                    // line (T-066).
                    None if v.reroute.is_some() => Some((node.s_out + HARD_GAP, 0.0, HARD_GAP)),
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
                            ahead_of_stop_line(ctx, v, node, ex)
                        } else {
                            Some((node.s_out + HARD_GAP, 0.0, HARD_GAP))
                        }
                    }
                },
            },
        };
        // The front car also keeps clear of cars that left the lane into a connection, whichever
        // one: a long vehicle still covers the stop line while its front is already inside.
        let leader = match (prev, conn_leader(ctx, v, slot, node)) {
            (None, Some(c)) if leader.is_none_or(|l| c.0 < l.0) => Some(c),
            _ => leader,
        };
        let next = v.path.steps.get(v.hop as usize);
        // A keep vehicle brakes for its goal as for a standing leader and stops on it.
        let goal = v.path.goal_s.clamp(node.s_in, node.s_out);
        let leader = if next.is_none() && v.is_keep() {
            let stop = (goal, 0.0, KEEP_STOP_GAP);
            Some(leader.map_or(stop, |l| if l.0 < stop.0 { l } else { stop }))
        } else {
            leader
        };
        let (mut s, nv) = follow(v, spec_of(ctx, v), node.speed, leader);
        match next {
            Some(_) if open && s >= node.s_out => ev = Ev::Cross,
            Some(_) => s = s.min(node.s_out),
            None if v.is_keep() => {
                if goal - s <= KEEP_STOP_GAP + 0.5 && nv < 0.3 {
                    ev = Ev::Park;
                }
            }
            None => {
                if s >= goal {
                    ev = Ev::Arrive;
                }
            }
        }
        push_upd(out, v, Upd { id, s, v: nv, ev });
        prev = Some(v);
    }
}

/// The rear-most car among those that have left lane `slot` into any connection, as the leader
/// of the lane's front car `v`.
fn conn_leader(ctx: &Ctx, v: &Vehicle, slot: u32, node: &LaneNode) -> Option<Leader> {
    if ctx.st.leaving.get(slot as usize).is_none_or(|&n| n == 0) {
        return None;
    }
    ctx.st
        .conns
        .range((slot, 0)..=(slot, u32::MAX))
        .flat_map(|(_, l)| l.iter())
        .filter_map(|&id| ctx.veh.get(id))
        .map(|o| {
            (
                node.s_out + o.s - spec_of(ctx, o).length,
                o.speed,
                spec_of(ctx, v).min_gap,
            )
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
}

/// The nearest thing past the stop line when the gate is open, in lane coordinates.
fn ahead_of_stop_line(ctx: &Ctx, v: &Vehicle, node: &LaneNode, ex: &ExitConn) -> Option<Leader> {
    let gap = spec_of(ctx, v).min_gap;
    let fs = ctx.g.car_slot(node.id)?;
    if let Some(l) = ctx.st.conns.get(&(fs, ex.to_slot))
        && let Some(o) = l.last().and_then(|&id| ctx.veh.get(id))
    {
        return Some((node.s_out + o.s - spec_of(ctx, o).length, o.speed, gap));
    }
    let t = ctx.g.car[ex.to_slot as usize].as_ref()?;
    let o = ctx.st.lanes[ex.to_slot as usize]
        .last()
        .and_then(|&id| ctx.veh.get(id))?;
    Some((
        node.s_out + ex.length + o.s - spec_of(ctx, o).length - t.s_in,
        o.speed,
        gap,
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
            Some(l) => Some((
                l.s - spec_of(ctx, l).length,
                l.speed,
                spec_of(ctx, v).min_gap,
            )),
            None => ctx.st.lanes[ex.to_slot as usize]
                .last()
                .and_then(|&id| ctx.veh.get(id))
                .map(|o| {
                    (
                        ex.length + o.s - spec_of(ctx, o).length - target.s_in,
                        o.speed,
                        spec_of(ctx, v).min_gap,
                    )
                }),
        };
        let (s, nv) = follow(v, spec_of(ctx, v), limit, leader);
        let ev = if prev.is_none() && s >= ex.length {
            Ev::Cross
        } else {
            Ev::Stay
        };
        push_upd(
            out,
            v,
            Upd {
                id,
                s: if prev.is_some() { s.min(ex.length) } else { s },
                v: nv,
                ev,
            },
        );
        prev = Some(v);
    }
}

fn step_place(ctx: &Ctx, p: Place, out: &mut Vec<Upd>) {
    match p {
        Place::Lane(slot) => step_lane(ctx, slot, out),
        Place::Conn(a, b) => step_conn(ctx, (a, b), out),
    }
}

/// How near its junction (metres) a car starting in a lane it must leave starts in the lane its
/// route needs instead (BUG-090).
const START_CHANGE_M: f32 = 40.0;

/// A car not yet on the road whose route begins with lane changes, starting within
/// [`START_CHANGE_M`] of the junction, starts in the lane those changes lead to: it pulls out
/// into the lane it needs, rather than meeting its lane change at the stop line behind a queue
/// and waiting there until it is removed as stuck (BUG-090, wide roads).
fn start_in_the_lane_it_needs(g: &PathGraph, v: &mut Vehicle) {
    let Some(node) = lane_node(g, v.path.start) else {
        return;
    };
    if v.path.start_s < node.s_out - START_CHANGE_M {
        return;
    }
    let mut lane = v.path.start;
    let mut n = 0;
    while let Some(&(a, b)) = v.path.steps.get(n) {
        if a != lane || !is_lane_change(g, a, b) {
            break;
        }
        lane = b;
        n += 1;
    }
    if n > 0 {
        v.path.start = lane;
        v.path.steps.drain(..n);
    }
}

/// Puts waiting vehicles on the road where there is room. Returns those that cannot ever
/// start (their start lane is gone).
pub(crate) fn spawn_waiting(
    vehicles: &mut Arena<Vehicle>,
    st: &mut DriveState,
    g: &PathGraph,
    pool: Option<&TaskPool>,
    finished: &mut Finished,
) {
    // Id order, whatever the order they arrived in: a load rebuilds the list from the arena.
    let mut waiting = std::mem::take(&mut st.waiting);
    waiting.sort_unstable();
    for id in waiting {
        if let Some(v) = vehicles.get_mut(id) {
            start_in_the_lane_it_needs(g, v);
        }
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
        let mine = *st.specs.of(v);
        let ahead_ok = at == 0
            || vehicles
                .get(list[at - 1])
                .is_some_and(|o| o.s - st.specs.of(o).length - mine.min_gap >= s);
        let behind_ok = at == list.len()
            || vehicles
                .get(list[at])
                .is_some_and(|o| o.s + mine.length + st.specs.of(o).min_gap <= s);
        // A car that has just crossed the stop line into a connection still covers it.
        let clear_of_junction = || {
            left_the_lane(vehicles, st, slot, node.s_out, s, mine.min_gap)
                .next()
                .is_none()
        };
        if ahead_ok && behind_ok && clear_of_junction() {
            st.lane_insert(slot, at, id);
            let v = vehicles.get_mut(id).expect("vehicle");
            v.spawned = true;
            v.s = s;
            v.speed = 0.0;
            v.stuck_timer = 0;
        } else {
            st.waiting.push(id);
        }
    }
    // Routes can change between ticks and cars appear: look at all lanes once per tick.
    st.refresh_hop_lanes(vehicles, g, pool);
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
        // Held at the stop line by a lane change that finds no gap: only a gap kept shut by
        // cars that stand is suspicious; cars that are moving will clear it.
        return is_lane_change(g, node.id, to)
            && hop_blockers(g, veh, st, v, to, node.s_out)
                .into_iter()
                .any(|o| o.speed < 0.1);
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
            && could_enter(g, veh, st, o, n, c.to)
        {
            legit |= o.speed >= 0.1;
            stuck_behind |= o.speed < 0.1;
        }
    }
    if stuck_behind {
        return true;
    }
    if legit || has_room(g, veh, st, v, ex.to_slot, 0) {
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

/// The flags of the cars of one place, front to back, given the verdict on its head (`front`):
/// a car behind a standing leader inherits the leader's reason.
fn walk_flags(
    veh: &Arena<Vehicle>,
    list: &[VehicleId],
    front: bool,
    root: bool,
    out: &mut Vec<(VehicleId, bool, bool)>,
) {
    let mut lead: Option<bool> = None;
    for (i, &id) in list.iter().enumerate() {
        let Some(v) = veh.get(id) else { continue };
        let standing = v.speed < 0.1;
        let sus = match (i, lead) {
            (0, _) => standing && front,
            (_, Some(l)) => standing && l,
            (_, None) => false,
        };
        // Only the car at the head of the line is the cause; the ones behind it wait for it.
        out.push((id, sus, i == 0 || root));
        lead = standing.then_some(sus);
    }
}

/// For every standing vehicle: is it standing for a suspicious reason? Front to back per place,
/// one pass; a car behind a standing leader inherits the leader's reason.
fn suspicious_flags(
    veh: &Arena<Vehicle>,
    st: &DriveState,
    g: &PathGraph,
    signal: u64,
    pool: Option<&TaskPool>,
) -> Vec<(VehicleId, bool, bool)> {
    // Each lane on its own, only reading: the verdict on its head (the costly part, T-259) and
    // the walk along it (T-292), in parallel; joined in lane order, as one pass made them.
    let slots: Vec<usize> = st.occ.iter().collect();
    let mut out = par_flat_map(pool, &slots, |&slot, out| {
        let list = &st.lanes[slot];
        let Some(&first) = list.first() else { return };
        let front = match (veh.get(first), g.car[slot].as_ref()) {
            (Some(v), Some(node)) if v.speed < 0.1 => {
                front_suspicious(g, veh, st, signal, v, node, 0)
            }
            _ => false,
        };
        walk_flags(veh, list, front, false, out);
    });
    for list in st.conns.values() {
        // A car standing inside a junction waits for room beyond it.
        walk_flags(veh, list, true, true, &mut out);
    }
    for &id in &st.waiting {
        if veh.contains(id) {
            out.push((id, false, true));
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
    specs: &VehicleSpecs,
) -> Finished {
    st.specs = *specs;
    let mut finished = Finished::default();
    // The drive's parallel parts use the pool from the same size on as the substeps.
    let pool = pool.filter(|_| vehicles.len() >= PARALLEL_MIN);
    spawn_waiting(vehicles, st, g, pool, &mut finished);
    for sub in 0..MOVE_SUBSTEPS {
        substep(vehicles, st, g, signal_tick(tick, sub), pool, &mut finished);
    }
    // Stuck timers (§9.4 v1.1): only a standstill for a suspicious reason counts.
    let flags = suspicious_flags(vehicles, st, g, signal_tick(tick, MOVE_SUBSTEPS - 1), pool);
    let mut stuck = Vec::new();
    for (id, suspicious, root) in flags {
        let v = vehicles.get_mut(id).expect("flagged vehicle");
        if v.speed >= 0.1 || v.is_parked() {
            v.stuck_timer = 0;
            continue;
        }
        let (still, sus) = (v.stuck_timer >> 16, v.stuck_timer & 0xFFFF);
        let still = (still + 1).min(0xFFFF);
        let sus = if suspicious { (sus + 1).min(0xFFFF) } else { 0 };
        v.stuck_timer = still << 16 | sus;
        let cap = stuck_ticks.saturating_mul(HARD_CAP_FACTOR);
        // A car that has not left its parking spot yet only waits for a gap; it takes much
        // longer to give up than one that blocks the road (BUG-072b).
        let cap = if still > cap && st.waiting.contains(&id) {
            cap.saturating_mul(WAITING_CAP_FACTOR)
        } else {
            cap
        };
        // A queue behind a stuck car clears when that car is removed: only its head goes.
        if (root && sus > stuck_ticks) || still > cap {
            stuck.push(id);
        }
    }
    for &id in &stuck {
        let why = stuck_why(
            vehicles,
            st,
            g,
            signal_tick(tick, MOVE_SUBSTEPS - 1),
            id,
            stuck_ticks,
        );
        st.stuck_why[why as usize] += 1;
    }
    for id in stuck {
        if let Some(v) = vehicles.remove(id) {
            st.remove(&v, id);
            finished.stuck.push(v);
        }
    }
    finished
}

/// Why `id` is being removed as stuck (diagnostics only).
fn stuck_why(
    veh: &Arena<Vehicle>,
    st: &DriveState,
    g: &PathGraph,
    signal: u64,
    id: VehicleId,
    stuck_ticks: u32,
) -> StuckWhy {
    let Some(v) = veh.get(id) else {
        return StuckWhy::HardCap;
    };
    if !v.spawned {
        return StuckWhy::Waiting;
    }
    if v.in_conn {
        return StuckWhy::InJunction;
    }
    let sus = v.stuck_timer & 0xFFFF;
    let slot = v.lane().index as usize;
    let head = st.lanes.get(slot).and_then(|l| l.first()) == Some(&id);
    if !head {
        return StuckWhy::HardCap;
    }
    if sus <= stuck_ticks {
        return StuckWhy::HardCapHead;
    }
    let (Some(node), Some((_, to))) = (
        g.car.get(slot).and_then(|n| n.as_ref()),
        v.path.steps.get(v.hop as usize).copied(),
    ) else {
        return StuckWhy::FrontOther;
    };
    let Some(ex) = find_exit(g, node.id, to) else {
        return StuckWhy::LaneChange;
    };
    let conflict_stands = ex.conflicts.iter().any(|c| {
        st.conns
            .get(&(c.from_slot, c.to.index))
            .is_some_and(|l| l.iter().any(|&o| veh.get(o).is_some_and(|o| o.speed < 0.1)))
    });
    if conflict_stands {
        return StuckWhy::ConflictStands;
    }
    if !has_room(g, veh, st, v, ex.to_slot, 0) {
        return StuckWhy::FullBeyond;
    }
    let _ = signal;
    StuckWhy::FrontOther
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
    lane_changes(vehicles, st, g);
    let mut places: Vec<Place> = Vec::new();
    for slot in st.occ.iter() {
        places.push(Place::Lane(slot as u32));
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
            concat(chunks)
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

/// The chunk results in chunk order, copied in bulk (T-259: a `flatten().collect()` moved them
/// one at a time and was a seventh of the drive's sequential time at 100k).
pub(crate) fn concat<T>(chunks: Vec<Vec<T>>) -> Vec<T> {
    let mut out = Vec::with_capacity(chunks.iter().map(Vec::len).sum());
    for mut c in chunks {
        out.append(&mut c);
    }
    out
}

/// `f` of every item, in item order: on `pool` in fixed chunks of [`CHUNK`] items when there
/// is one, else in sequence. `f` only reads, so the result is the same either way.
fn par_map<T: Sync, R: Send + 'static>(
    pool: Option<&TaskPool>,
    items: &[T],
    f: impl Fn(&T) -> R + Sync,
) -> Vec<R> {
    match pool {
        Some(pool) if items.len() > CHUNK => {
            let f = &f;
            concat(pool.scope(|scope| {
                for chunk in items.chunks(CHUNK) {
                    scope.spawn(async move { chunk.iter().map(f).collect::<Vec<R>>() });
                }
            }))
        }
        _ => items.iter().map(f).collect(),
    }
}

/// What `f` appends for every item, in item order: on `pool` in fixed chunks of [`CHUNK`] items
/// (one output per chunk, joined in chunk order) when there is one, else in sequence. `f` only
/// reads, so the result is the same either way.
fn par_flat_map<T: Sync, R: Send + 'static>(
    pool: Option<&TaskPool>,
    items: &[T],
    f: impl Fn(&T, &mut Vec<R>) + Sync,
) -> Vec<R> {
    match pool {
        Some(pool) if items.len() > CHUNK => {
            let f = &f;
            concat(pool.scope(|scope| {
                for chunk in items.chunks(CHUNK) {
                    scope.spawn(async move {
                        let mut out = Vec::new();
                        for item in chunk {
                            f(item, &mut out);
                        }
                        out
                    });
                }
            }))
        }
        _ => {
            let mut out = Vec::new();
            for item in items {
                f(item, &mut out);
            }
            out
        }
    }
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
        // Most updates are plain moves: one lookup.
        if u.ev == Ev::Stay {
            if let Some(v) = vehicles.get_mut(u.id) {
                v.s = u.s;
                v.speed = u.v;
            }
            continue;
        }
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
            Ev::Park => {
                let v = vehicles.get_mut(u.id).expect("vehicle");
                let lane = v.lane();
                v.path = crate::astar::CarPath {
                    start: lane,
                    start_s: u.s,
                    steps: Vec::new(),
                    goal: lane,
                    goal_s: u.s,
                    cost: 0.0,
                };
                v.hop = 0;
                v.s = u.s;
                v.speed = 0.0;
                v.stuck_timer = 0;
                v.kind |= crate::agents::KIND_PARKED;
                finished.parked.push(u.id);
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
                st.lane_remove(fs, u.id);
                st.conns.entry((fs, ex.to_slot)).or_default().push(u.id);
                st.entering[ex.to_slot as usize] += 1;
                st.leaving[fs as usize] += 1;
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
                    .is_none_or(|o| s_new <= o.s - st.specs.of(o).length - HARD_GAP);
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
                if pending_hop(g, v) {
                    st.hop_lanes.set(ex.to_slot as usize, true);
                }
                let key = (fs, ex.to_slot);
                if let Some(l) = st.conns.get_mut(&key) {
                    l.retain(|w| *w != u.id);
                    if l.is_empty() {
                        st.conns.remove(&key);
                    }
                    st.leaving[fs as usize] = st.leaving[fs as usize].saturating_sub(1);
                }
                st.entering[ex.to_slot as usize] =
                    st.entering[ex.to_slot as usize].saturating_sub(1);
                st.lane_push(ex.to_slot, u.id);
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

/// BUG-100 diagnostics: why the head `id` of its lane may not enter its next connection, check
/// by check (light, sibling, each conflict, room beyond). Not used by the sim.
pub(crate) fn gate_report(
    g: &PathGraph,
    veh: &Arena<Vehicle>,
    st: &DriveState,
    signal: u64,
    id: VehicleId,
) -> String {
    // Follows a full lane beyond to its own head, up to 12 lanes or a lane seen before.
    let (mut out, mut id, mut seen) = (String::new(), id, Vec::new());
    for _ in 0..12 {
        let mut next = None;
        out += &gate_report_one(g, veh, st, signal, id, &mut next);
        let Some(slot) = next else { break };
        if seen.contains(&slot) {
            out += &format!(" => LOOP at lane {slot}");
            break;
        }
        seen.push(slot);
        let Some(&h) = st.lanes[slot as usize].first() else {
            break;
        };
        let Some(o) = veh.get(h) else { break };
        out += &format!(
            " => lane {slot} ({} cars) head s {:.0} v {:.1} still {} spawned {} parked {}: ",
            st.lanes[slot as usize].len(),
            o.s,
            o.speed,
            o.stuck_timer >> 16,
            o.spawned,
            o.is_parked()
        );
        id = h;
    }
    out
}

fn gate_report_one(
    g: &PathGraph,
    veh: &Arena<Vehicle>,
    st: &DriveState,
    signal: u64,
    id: VehicleId,
    next: &mut Option<u32>,
) -> String {
    let Some(v) = veh.get(id) else {
        return "gone".into();
    };
    let slot = v.lane().index as usize;
    let (Some(node), Some((_, to))) = (
        g.car.get(slot).and_then(|n| n.as_ref()),
        v.path.steps.get(v.hop as usize).copied(),
    ) else {
        return "no node/next".into();
    };
    let Some(ex) = find_exit(g, node.id, to) else {
        return format!("lane change to {}", to.index);
    };
    let mut out = format!(
        "to {} open {}",
        to.index,
        gate_open(g, veh, st, signal, v, ex, node.s_out)
    );
    if ex.light != u32::MAX {
        out += &format!(
            " light {:?}",
            g.lights[ex.light as usize].connection_signal(ex.index, signal)
        );
    }
    if let Some(from_slot) = g.car_slot(v.lane()) {
        out += &format!(
            " leaving {}",
            st.leaving.get(from_slot as usize).copied().unwrap_or(0)
        );
    }
    for c in &ex.conflicts {
        let inside = st
            .conns
            .get(&(c.from_slot, c.to.index))
            .map_or(0, |l| l.len());
        out += &format!(" | c {}->{} in {inside}", c.from_slot, c.to.index);
        if c.higher {
            out += " HIGHER";
            if let Some(cn) = g.car[c.from_slot as usize].as_ref()
                && let Some(&oid) = st.lanes[c.from_slot as usize].first()
                && let Some(o) = veh.get(oid)
            {
                let next = o.path.steps.get(o.hop as usize).map(|s| s.1);
                out += &format!(
                    " head s {:.0}/{:.0} v {:.1} next {:?} enter {} still {}",
                    o.s,
                    cn.s_out,
                    o.speed,
                    next.map(|n| n.index),
                    could_enter(g, veh, st, o, cn, c.to),
                    o.stuck_timer >> 16
                );
            }
        }
    }
    *next = Some(ex.to_slot);
    let t = g.car[ex.to_slot as usize].as_ref();
    let last = st.lanes[ex.to_slot as usize]
        .last()
        .and_then(|&o| veh.get(o));
    out += &format!(
        " | room {} (span {:.0} last s {:?} entering {})",
        has_room(g, veh, st, v, ex.to_slot, 0),
        t.map_or(0.0, |t| t.s_out - t.s_in),
        last.map(|o| o.s),
        st.entering[ex.to_slot as usize]
    );
    out
}
