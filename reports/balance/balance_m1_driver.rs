//! Balance driver of T-049 (M1 acceptance): a scripted player founds a city on a new game and
//! the sim runs N game days. One CSV row per game day, an event log on the side.
//!
//! ```text
//! cargo run --release -p cp_sim --example balance_m1 -- <map> <seed> <days> <out-prefix>
//! ```
//!
//! Everything goes through commands (roads, pipes, lines, buildings, zone paint), the player
//! starts with the new-game funds and pays like a human (no scenario funds, no frozen demand),
//! so the run is reproducible from `(map, seed, content)`. Not a sim feature; it only reads.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_building::{BuildingCommand, BuildingGrid, Demand};
use cp_content::{BuildingDefId, ContentDb, UtilityKind, ZoneType};
use cp_core::time::TICKS_PER_DAY;
use cp_core::{CommandOutcome, SimLimits, Tick, Treasury};
use cp_economy::EconomyState;
use cp_net::{ElevationMode, NetCommand, RoadPath};
use cp_sim::stats::{CityStats, ZONES};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::{Terrain, TileOwnership};
use cp_utility::UtilityCommand;
use cp_zone::{ZoneCellRef, ZoneCommand, ZoneGrid};

/// Distance between the streets of the player's lattice, metres (12 zoning cells).
const S: f32 = 96.0;
/// The player keeps this much money back (currency units).
const RESERVE: f64 = 800.0;
/// Ticks between two decisions of the player.
const DECIDE_EVERY: u64 = 256;

type Pt = (i32, i32);
type Run = (Pt, Pt);

#[derive(Clone, Debug)]
enum Task {
    Roads(Vec<Run>),
    Lines(Vec<Run>),
    WaterRoad,
    Pump,
    Outflow,
    Turbine,
    Coal,
    Tower,
    Treatment,
}

enum Outcome {
    Done,
    Wait,
    Failed(String),
}

struct Player {
    sim: SimRunner,
    content: Arc<ContentDb>,
    terrain: Arc<Terrain>,
    seed: u64,
    origin: Vec2,
    fwd: Vec2,
    side: Vec2,
    queue: Vec<Task>,
    stage: usize,
    last_expand_day: u64,
    built_runs: BTreeSet<Run>,
    net_nodes: Vec<Vec2>,
    counts: [u32; 8],
    pump_failed: bool,
    seeded: bool,
    unpainted: u32,
    events: String,
    rejected: u32,
    spent: f64,
}

/// (half-width I, length J) of the lattice per stage.
const STAGES: [(i32, i32); 9] = [
    (1, 2),
    (2, 3),
    (2, 4),
    (3, 5),
    (3, 6),
    (4, 7),
    (5, 8),
    (6, 9),
    (7, 10),
];

fn hash(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    x ^ (x >> 33)
}

impl Player {
    fn day(&self) -> u64 {
        self.sim.tick().0 / TICKS_PER_DAY
    }

    fn balance(&self) -> f64 {
        self.sim.world().resource::<Treasury>().balance.0 as f64 / 100.0
    }

    fn pt(&self, p: Pt) -> Vec2 {
        self.origin + self.fwd * (p.1 as f32 * S) + self.side * (p.0 as f32 * S)
    }

    fn log(&mut self, msg: String) {
        let _ = writeln!(self.events, "day {:>3} t{}: {msg}", self.day(), self.sim.tick().0);
    }

    /// Pushes the commands at consecutive ticks, runs them and returns the rejections.
    fn issue(&mut self, cmds: Vec<SimCommand>) -> Vec<String> {
        let start = self.sim.tick().0;
        let n = cmds.len() as u64;
        for (k, c) in cmds.into_iter().enumerate() {
            self.sim.push_command(c, Tick(start + k as u64));
        }
        let mut errs = Vec::new();
        for _ in 0..=n {
            self.sim.tick_once();
            let out: Vec<CommandOutcome> = self
                .sim
                .world_mut()
                .resource_mut::<Messages<CommandOutcome>>()
                .drain()
                .collect();
            errs.extend(out.into_iter().filter_map(|o| o.result.err().map(|e| e.to_string())));
        }
        self.rejected += errs.len() as u32;
        errs
    }

    fn owned(&self, p: Vec2) -> bool {
        self.sim.world().resource::<TileOwnership>().contains_point(p)
    }

    fn run_len(&self, r: Run) -> f32 {
        self.pt(r.0).distance(self.pt(r.1))
    }

    fn valid_site(&self, def: BuildingDefId, pos: Vec2) -> bool {
        let world = self.sim.world();
        let zones = world.resource::<ZoneGrid>();
        let net = world.resource::<cp_net::NetGraph>();
        let tiles = world.get_resource::<TileOwnership>();
        let buildings = world.resource::<BuildingGrid>();
        let content = &*self.content;
        let t = Some(&*self.terrain);
        if cp_building::is_free_standing(content, def) {
            cp_building::validate_free_placement(content, zones, net, buildings, t, tiles, def, pos, 0.0)
                .is_ok()
        } else {
            cp_building::validate_placement(content, zones, net, t, tiles, def, pos, 0.0).is_ok()
        }
    }

    fn site_error(&self, def: BuildingDefId, pos: Vec2) -> String {
        let world = self.sim.world();
        let r = cp_building::validate_placement(
            &self.content,
            world.resource::<ZoneGrid>(),
            world.resource::<cp_net::NetGraph>(),
            Some(&*self.terrain),
            world.get_resource::<TileOwnership>(),
            def,
            pos,
            0.0,
        );
        format!("{r:?}")
    }

    fn nearest_node(&self, p: Vec2) -> Option<Vec2> {
        self.net_nodes
            .iter()
            .copied()
            .min_by(|a, b| a.distance(p).total_cmp(&b.distance(p)))
    }

    /// A polyline from `from` to `to` with legs of at most 90 m.
    fn legs(from: Vec2, to: Vec2) -> Vec<Vec2> {
        let n = ((from.distance(to) / 90.0).ceil() as usize).max(1);
        (0..=n).map(|k| from.lerp(to, k as f32 / n as f32)).collect()
    }

    fn link(&self, kind: UtilityKind, from: Vec2, to: Vec2) -> SimCommand {
        SimCommand::Utility(UtilityCommand::BuildLink {
            kind,
            path: Self::legs(from, to),
        })
    }

    fn place(def: BuildingDefId, pos: Vec2) -> SimCommand {
        SimCommand::Building(BuildingCommand::Place { def, pos, rot: 0.0 })
    }

    /// Free-standing / shore / road-side site search around `near`.
    fn find_site(&self, def: BuildingDefId, near: Vec2, r0: f32, r1: f32, step: f32) -> Option<Vec2> {
        let mut r = r0;
        while r <= r1 {
            for k in 0..16 {
                let a = k as f32 * std::f32::consts::TAU / 16.0;
                let p = near + Vec2::new(a.cos(), a.sin()) * r;
                if self.owned(p) && self.valid_site(def, p) {
                    return Some(p);
                }
            }
            r += step;
        }
        None
    }

    /// A lot beside a street (for the coal plant and the treatment plant): the outermost block
    /// whose middle cell takes the building.
    fn find_block_site(&self, def: BuildingDefId) -> Option<Vec2> {
        let zones = self.sim.world().resource::<ZoneGrid>();
        let mut blocks: Vec<(f32, u32, cp_zone::ZoneBlockId, u16)> = zones
            .blocks()
            .filter(|(_, b)| b.cols >= 6)
            .map(|(id, b)| {
                let p = b.curve.point(0.5);
                (Vec2::new(p.x, p.z).distance(self.origin), id.index, id, b.cols)
            })
            .collect();
        blocks.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        blocks.into_iter().find_map(|(_, _, id, cols)| {
            let pos = zones
                .cell_footprint(ZoneCellRef {
                    block: id,
                    cell: (cols / 2) * cp_zone::BLOCK_DEPTH + 1,
                })?
                .c;
            self.valid_site(def, pos).then_some(pos)
        })
    }


    /// Cells (as positions) of block columns, optionally only wet ones, nearest to `to` first.
    fn block_cell_sites(&self, to: Vec2, wet: bool) -> Vec<Vec2> {
        let zones = self.sim.world().resource::<ZoneGrid>();
        let mut out: Vec<(f32, Vec2)> = Vec::new();
        for (id, b) in zones.blocks() {
            for col in 0..b.cols {
                for k in 0..cp_zone::BLOCK_DEPTH {
                    let cell = col * cp_zone::BLOCK_DEPTH + k;
                    if let Some(o) = zones.cell_footprint(ZoneCellRef { block: id, cell }) {
                        let w = self.terrain.is_water(o.c);
                        if (k == 0 || wet) && (w == wet || !wet) {
                            out.push((o.c.distance(to), o.c));
                        }
                    }
                }
            }
        }
        out.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.x.total_cmp(&b.1.x)).then(a.1.y.total_cmp(&b.1.y)));
        out.into_iter().map(|x| x.1).collect()
    }

    fn water_road(&mut self) -> Outcome {
        let road = self.content.road_id("basic_2l").unwrap();
        let c = self.pt((0, 1));
        let mut wet: Vec<Vec2> = Vec::new();
        let mut z = 0.0;
        while z < cp_terrain::MAP_SIZE {
            let mut x = 0.0;
            while x < cp_terrain::MAP_SIZE {
                let p = Vec2::new(x, z);
                if self.owned(p) && self.terrain.water_depth(p) >= 2.0 {
                    wet.push(p);
                }
                x += 16.0;
            }
            z += 16.0;
        }
        if wet.is_empty() {
            return Outcome::Failed("no water on the owned tiles".into());
        }
        // Candidate roads: from a node of the network straight at a wet sample, ending 2 m before
        // the water, then a 64 m leg along the shore with water within 10..30 m beside it (the
        // zoning cells a pump or an outflow stands on).
        let mut cands: Vec<(f32, Vec2, Vec2, Vec2)> = Vec::new();
        for &n in &self.net_nodes {
            for &w in wet.iter().step_by(3) {
                let dir = (w - n).normalize();
                let mut e = n;
                while self.terrain.water_depth(e + dir * 2.0) < 0.0 && self.owned(e + dir * 2.0) {
                    e += dir * 2.0;
                    if e.distance(n) > 1500.0 {
                        break;
                    }
                }
                if e.distance(n) < 24.0 || e.distance(n) > 1500.0 {
                    continue;
                }
                let perp = Vec2::new(-dir.y, dir.x);
                for sd in [1.0f32, -1.0] {
                    for len in [64.0f32, 48.0] {
                        let f = e + perp * sd * len;
                        if !self.road_ok(e, f) {
                            continue;
                        }
                        // Water beside the leg, on its far side from the city.
                        let wet_cells = (0..8)
                            .flat_map(|m| (0..3).map(move |k| (m, k)))
                            .filter(|&(m, k)| {
                                let q = e + perp * sd * (12.0 + 8.0 * m as f32) + dir * (14.0 + 8.0 * k as f32);
                                self.terrain.is_water(q)
                            })
                            .count();
                        if wet_cells >= 8 {
                            cands.push((e.distance(n), n, e, f));
                        }
                    }
                }
            }
        }
        cands.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.2.x.total_cmp(&b.2.x)));
        cands.dedup_by(|a, b| a.3.distance(b.3) < 12.0);
        let mut tried = 0;
        let mut first_err = None;
        for (len, n, e, f) in cands {
            if !self.road_ok(n, e) {
                continue;
            }
            let total = len + e.distance(f);
            let cost = f64::from(total) * 14.0;
            if self.balance() - cost < RESERVE {
                return Outcome::Wait;
            }
            tried += 1;
            let mk = |from, to| {
                SimCommand::Net(NetCommand::BuildRoad {
                    road,
                    path: RoadPath::Straight { from, to },
                    elevation: ElevationMode::Ground,
                })
            };
            let _ = mk;
            let errs = self.issue(vec![SimCommand::Net(NetCommand::BuildRoad {
                road,
                path: RoadPath::Free(vec![n, e, f]),
                elevation: ElevationMode::Ground,
            })]);
            if errs.is_empty() {
                for _ in 0..4 {
                    self.sim.tick_once();
                }
                let pipe = self.link(UtilityKind::Water, n, e);
                let pipe2 = self.link(UtilityKind::Water, e, f);
                self.issue(vec![pipe, pipe2]);
                self.spent += cost;
                self.net_nodes.push(e);
                self.net_nodes.push(f);
                self.log(format!("water road {len:.0} m from ({:.0},{:.0}) to ({:.0},{:.0}) + shore leg to ({:.0},{:.0}), {tried} tries, cost {cost:.0}", n.x, n.y, e.x, e.y, f.x, f.y));
                return Outcome::Done;
            }
            first_err.get_or_insert(errs[0].clone());
            if tried >= 12 {
                break;
            }
        }
        Outcome::Failed(format!("no buildable road to a wet zoning cell ({tried} tried, first error {first_err:?})"))
    }

    fn try_task(&mut self, task: &Task) -> Outcome {
        match task {
            Task::WaterRoad => self.water_road(),
            Task::Roads(runs) => {
                let road = self.content.road_id("basic_2l").unwrap();
                let cost: f64 = runs
                    .iter()
                    .map(|&r| f64::from(self.run_len(r)) * f64::from(self.content.road(road).cost_per_m))
                    .sum();
                if self.balance() - cost < RESERVE {
                    return Outcome::Wait;
                }
                let cmds = runs
                    .iter()
                    .map(|&r| {
                        SimCommand::Net(NetCommand::BuildRoad {
                            road,
                            path: RoadPath::Straight {
                                from: self.pt(r.0),
                                to: self.pt(r.1),
                            },
                            elevation: ElevationMode::Ground,
                        })
                    })
                    .collect();
                let errs = self.issue(cmds);
                for _ in 0..4 {
                    self.sim.tick_once();
                }
                self.spent += cost;
                self.log(format!("roads x{} ({cost:.0}) rejected {}: {:?}", runs.len(), errs.len(), errs.iter().take(2).collect::<Vec<_>>()));
                Outcome::Done
            }
            Task::Lines(runs) => {
                let cost: f64 = runs
                    .iter()
                    .map(|&r| f64::from(self.run_len(r)) * (3.0 + 4.0))
                    .sum();
                if self.balance() - cost < RESERVE {
                    return Outcome::Wait;
                }
                let mut cmds = Vec::new();
                for &r in runs {
                    for kind in [UtilityKind::Power, UtilityKind::Water] {
                        cmds.push(self.link(kind, self.pt(r.0), self.pt(r.1)));
                    }
                    for p in [r.0, r.1] {
                        let v = self.pt(p);
                        if !self.net_nodes.iter().any(|n| n.distance(v) < 1.0) {
                            self.net_nodes.push(v);
                        }
                    }
                }
                let errs = self.issue(cmds);
                self.spent += cost;
                self.log(format!("lines x{} ({cost:.0}) rejected {}: {:?}", runs.len(), errs.len(), errs.iter().take(2).collect::<Vec<_>>()));
                Outcome::Done
            }
            Task::Pump | Task::Outflow | Task::Tower | Task::Turbine | Task::Coal | Task::Treatment => {
                let name = match task {
                    Task::Pump => "water_pump_station",
                    Task::Outflow => "sewage_outflow_station",
                    Task::Tower => "water_tower",
                    Task::Turbine => "wind_turbine",
                    Task::Coal => "coal_power_plant",
                    _ => "sewage_treatment_plant",
                };
                let def = self.content.building_id(name).unwrap();
                let cost = f64::from(self.content.building(def).cost);
                if self.balance() - cost < RESERVE {
                    return Outcome::Wait;
                }
                let c = self.pt((0, 1));
                let (pos, pipe_to_net, line_kind) = match task {
                    Task::Pump => {
                        let sites = self.block_cell_sites(c, true);
                        (sites.into_iter().find(|&p| self.valid_site(def, p)), true, None)
                    }
                    Task::Outflow => {
                        let near = self.pump_pos().unwrap_or(c);
                        let sites = self.block_cell_sites(near, false);
                        (sites.into_iter().take(400).find(|&p| self.valid_site(def, p)), true, None)
                    }
                    Task::Tower => {
                        let n = self.counts[4] as i32;
                        let i = if n % 2 == 0 { 1 + n / 2 } else { -(1 + n / 2) };
                        (self.find_site(def, self.pt((i, 2)), 24.0, 38.0, 4.0), false, None)
                    }
                    Task::Turbine => {
                        let n = self.counts[3] as i32;
                        let i = if n % 2 == 0 { -(1 + n / 2) } else { 1 + n / 2 };
                        (self.find_site(def, self.pt((i, 1)), 24.0, 38.0, 4.0), false, None)
                    }
                    _ => (self.find_block_site(def), false, None),
                };
                let Some(pos) = pos else {
                    let why = if matches!(task, Task::Pump) {
                        {
                        let sites = self.block_cell_sites(c, true);
                        let mut kinds: Vec<String> = sites.iter().take(60).map(|&q| self.site_error(def, q)).collect();
                        kinds.sort();
                        kinds.dedup();
                        format!(" ({} wet zoning cells; errors {:?})", sites.len(), kinds)
                    }
                    } else {
                        String::new()
                    };
                    return Outcome::Failed(format!("no site for {name}{why}"));
                };
                let mut cmds = vec![Self::place(def, pos)];
                if pipe_to_net {
                    if let Some(n) = self.nearest_node(pos) {
                        cmds.push(self.link(UtilityKind::Water, pos, n));
                    }
                }
                if let Some(kind) = line_kind {
                    if let Some(n) = self.nearest_node(pos) {
                        cmds.push(self.link(kind, pos, n));
                    }
                }
                let errs = self.issue(cmds);
                if errs.is_empty() {
                    match task {
                        Task::Outflow => self.counts[1] += 1,
                        Task::Pump => self.counts[0] += 1,
                        Task::Turbine => self.counts[3] += 1,
                        Task::Tower => self.counts[4] += 1,
                        Task::Coal => self.counts[5] += 1,
                        Task::Treatment => self.counts[6] += 1,
                        _ => {}
                    }
                    self.spent += cost;
                    self.log(format!("{name} at ({:.0},{:.0}) cost {cost:.0}", pos.x, pos.y));
                    Outcome::Done
                } else {
                    Outcome::Failed(format!("{name}: {errs:?}"))
                }
            }
        }
    }

    fn pump_pos(&self) -> Option<Vec2> {
        let def = self.content.building_id("water_pump_station")?;
        let b = self.sim.world().resource::<BuildingGrid>();
        b.iter().find(|(_, x)| x.def == def).map(|(_, x)| x.pos)
    }

    /// The player's eye: a straight road from `a` to `b` stays on dry ground within the road
    /// slope limit and on owned tiles (the sim says no otherwise).
    fn road_ok(&self, a: Vec2, b: Vec2) -> bool {
        let n = ((a.distance(b) / 4.0).ceil() as usize).max(1);
        let mut prev = self.terrain.height(a);
        for k in 0..=n {
            let p = a.lerp(b, k as f32 / n as f32);
            let perp = (b - a).normalize_or_zero().perp() * 6.5;
            let near_end = p.distance(a) < 12.0 || p.distance(b) < 12.0;
            if !self.owned(p)
                || self.terrain.water_depth(p) > -2.0
                || (!near_end && (self.terrain.is_water(p + perp) || self.terrain.is_water(p - perp)))
            {
                return false;
            }
            let h = self.terrain.height(p);
            if k > 0 && (h - prev).abs() / (a.distance(b) / n as f32) > 0.105 {
                return false;
            }
            prev = h;
        }
        true
    }

    fn stage_runs_all(&self, stage: usize) -> Vec<Run> {
        let (ih, j) = STAGES[stage];
        let mut runs = Vec::new();
        for i in -ih..=ih {
            let j0 = if i == 0 { 0 } else { 1 };
            for jj in j0..j {
                runs.push(((i, jj), (i, jj + 1)));
            }
        }
        for jj in 1..=j {
            for i in -ih..ih {
                runs.push(((i, jj), (i + 1, jj)));
            }
        }
        runs
    }

    fn stage_runs(&self, stage: usize) -> Vec<Run> {
        let mut runs = self.stage_runs_all(stage);
        runs.retain(|r| !self.built_runs.contains(r) && self.road_ok(self.pt(r.0), self.pt(r.1)));
        runs
    }

    fn expand(&mut self) {
        let fresh = self.stage_runs_all(self.stage).into_iter().filter(|r| !self.built_runs.contains(r)).count();
        let runs = self.stage_runs(self.stage);
        for r in &runs {
            self.built_runs.insert(*r);
        }
        let skipped = fresh - runs.len();
        self.log(format!("stage {} planned: {} runs ({} streets skipped: steep, wet or off the owned tiles)", self.stage, runs.len(), skipped));
        self.queue.push(Task::Roads(runs.clone()));
        self.queue.push(Task::Lines(runs));
        self.stage += 1;
        self.last_expand_day = self.day();
    }

    /// Zone of a block at `p` for demand group `g` (0 R, 1 C, 2 I, 3 O).
    fn zone_in_group(&self, g: usize, p: Vec2) -> ZoneType {
        use ZoneType::*;
        let h = hash(((p.x / 16.0) as i64 as u64) << 20 ^ ((p.y / 16.0) as i64 as u64) ^ self.seed << 40);
        match g {
            0 => [ResLow, ResLow, ResHigh][(h % 3) as usize],
            1 => [ComLow, ComHigh][(h % 2) as usize],
            2 => Industrial,
            _ => Office,
        }
    }

    /// Unpainted blocks with their valid free cells, nearest to the entry first.
    fn unpainted_blocks(&self) -> Vec<(f32, Vec2, Vec<ZoneCellRef>)> {
        let zones = self.sim.world().resource::<ZoneGrid>();
        let mut out = Vec::new();
        for (id, b) in zones.blocks() {
            let cells: Vec<ZoneCellRef> = zones
                .block_cells(id)
                .into_iter()
                .filter(|c| {
                    zones.is_valid(*c)
                        && b.zones.get(usize::from(c.cell)).is_some_and(|z| *z == ZoneType::None)
                })
                .collect();
            if cells.len() >= 4 {
                let p = b.curve.point(0.5);
                let p = Vec2::new(p.x, p.z);
                out.push((p.distance(self.origin), p, cells));
            }
        }
        out.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.x.total_cmp(&b.1.x)).then(a.1.y.total_cmp(&b.1.y)));
        out
    }

    /// Paints like a player who watches the demand bars: the group with the highest demand and
    /// too few free painted cells gets the nearest unpainted block, up to three blocks a visit.
    /// The first visit paints a little of every zone type (zoning is free in M1).
    fn paint(&mut self) {
        let mut blocks = self.unpainted_blocks();
        self.unpainted = blocks.iter().map(|b| b.2.len() as u32).sum();
        if blocks.is_empty() {
            return;
        }
        let stats = self.sim.world().resource::<CityStats>().clone();
        let d = *self.sim.world().resource::<Demand>();
        let demand = [d.r, d.c, d.i, d.o];
        let mut free = [0u32; 4];
        for (k, z) in stats.zones.iter().enumerate() {
            let g = match k {
                0 | 1 => 0,
                2 | 3 => 1,
                4 => 2,
                _ => 3,
            };
            free[g] += z.valid.saturating_sub(z.occupied);
        }
        let target = [80u32, 48, 48, 32];
        let mut cmds = Vec::new();
        if !self.seeded && blocks.len() >= 6 {
            self.seeded = true;
            let plan = [
                (0, ZoneType::ResLow),
                (1, ZoneType::ComLow),
                (0, ZoneType::ResHigh),
                (2, ZoneType::Industrial),
                (1, ZoneType::ComHigh),
                (3, ZoneType::Office),
            ];
            for (g, zone) in plan {
                let (_, _, cells) = blocks.remove(0);
                free[g] += cells.len() as u32;
                cmds.push(SimCommand::Zone(ZoneCommand::PaintZones { cells, zone }));
            }
        }
        for _ in 0..3 {
            if blocks.is_empty() {
                break;
            }
            let best = (0..4)
                .filter(|&g| demand[g] > 8.0 && free[g] < target[g])
                .max_by(|&a, &b| demand[a].total_cmp(&demand[b]));
            let Some(g) = best else { break };
            let (_, p, cells) = blocks.remove(0);
            free[g] += cells.len() as u32;
            let zone = self.zone_in_group(g, p);
            cmds.push(SimCommand::Zone(ZoneCommand::PaintZones { cells, zone }));
        }
        if !cmds.is_empty() {
            self.issue(cmds);
        }
    }

    fn decide(&mut self) {
        let daily = self.sim.tick().0 % TICKS_PER_DAY == 0;
        self.paint();
        // Strict order: the front of the queue is bought first; a task that waits for money
        // blocks the ones behind it, like a saving player.
        while let Some(task) = self.queue.first().cloned() {
            match self.try_task(&task) {
                Outcome::Done => {
                    self.queue.remove(0);
                }
                Outcome::Wait => break,
                Outcome::Failed(msg) => {
                    self.log(format!("FAILED {task:?}: {msg}"));
                    match task {
                        Task::Pump => {
                            self.pump_failed = true;
                            self.queue.push(Task::Tower);
                        }
                        Task::Outflow => self.queue.push(Task::Treatment),
                        _ => {}
                    }
                    self.queue.remove(0);
                }
            }
        }
        if !daily {
            return;
        }
        // Once a day: utilities that run short, and room to grow.
        let stats = self.sim.world().resource::<CityStats>().clone();
        let queued = |q: &[Task], f: fn(&Task) -> bool| q.iter().any(f);
        let [water, sewage, power] = stats.utilities;
        if power.consumption > 0.8 * power.capacity
            && !queued(&self.queue, |t| matches!(t, Task::Turbine | Task::Coal))
        {
            let t = if self.counts[3] < 3 { Task::Turbine } else { Task::Coal };
            self.queue.push(t);
        }
        if water.consumption > 0.8 * water.capacity && !queued(&self.queue, |t| matches!(t, Task::Tower | Task::Pump)) {
            self.queue.push(if self.pump_failed || self.counts[0] > 0 { Task::Tower } else { Task::Pump });
        }
        if sewage.consumption > 0.8 * sewage.capacity && !queued(&self.queue, |t| matches!(t, Task::Treatment | Task::Outflow)) {
            self.queue.push(if self.counts[6] == 0 { Task::Treatment } else { Task::Outflow });
        }
        let busy = queued(&self.queue, |t| matches!(t, Task::Roads(_) | Task::Lines(_)));
        let wants = self.sim.world().resource::<Demand>();
        let wants = wants.r.max(wants.c).max(wants.i).max(wants.o) > 8.0;
        let crowded = self.unpainted < 100 && wants;
        let flush = self.balance() > 40_000.0 && self.day() >= self.last_expand_day + 7;
        let next_cost: f64 = if self.stage < STAGES.len() {
            self.stage_runs(self.stage).iter().map(|&r| f64::from(self.run_len(r)) * 17.0).sum()
        } else {
            0.0
        };
        let affordable = self.balance() - next_cost > 6000.0 && self.day() >= self.last_expand_day + 2;
        if !busy && self.stage < STAGES.len() && affordable && (crowded || flush) {
            self.expand();
        }
    }
}

fn csv_header() -> String {
    let mut h = String::from(
        "day,pop,households,adults,employed,unemployed,jobs_free,homes_free,demand_r,demand_c,demand_i,demand_o,move_in,\
         balance,week_income,week_upkeep,week_net,up_roads,up_buildings,up_utilities,spent_total,\
         bld_total,bld_active,bld_constructing,bld_abandoned,no_water,no_power,no_sewage,no_road,\
         trips_completed,trips_cancelled,despawned_stuck,no_path,abstract_trips,vehicles,pedestrians,in_transit,avg_load,max_load,overloaded,\
         connected_to_highway,water_cons,water_cap,sewage_cons,sewage_cap,power_cons,power_cap,pollution,land_value,roads_m",
    );
    for z in ZONES {
        let _ = write!(h, ",jobs_{z:?},painted_{z:?},valid_{z:?},occ_{z:?}");
        for l in 1..=5 {
            let _ = write!(h, ",L{l}_{z:?}");
        }
    }
    h
}

fn csv_row(day: u64, p: &Player) -> String {
    let w = p.sim.world();
    let s = w.resource::<CityStats>();
    let d = w.resource::<Demand>();
    let bal = w.resource::<Treasury>().balance.0 as f64 / 100.0;
    let wk = w.resource::<EconomyState>().last_week;
    let tb = w.resource::<cp_citizen::TripBook>();
    let u = &s.utilities;
    let jobs_free: u32 = s.zones.iter().map(|z| z.jobs_free).sum();
    let homes_free: u32 = s.zones.iter().map(|z| z.homes_free).sum();
    let roads_m: f32 = s.roads.iter().map(|r| r.meters).sum();
    let c = |x: cp_core::Cost| x.0 as f64 / 100.0;
    let mut r = String::new();
    let _ = write!(
        r,
        "{day},{},{},{},{},{},{jobs_free},{homes_free},{:.1},{:.1},{:.1},{:.1},{:.1},\
         {bal:.0},{:.0},{:.0},{:.0},{:.0},{:.0},{:.0},{:.0},\
         {},{},{},{},{},{},{},{},\
         {},{},{},{},{},{},{},{},{:.3},{:.3},{},\
         {},{:.0},{:.0},{:.0},{:.0},{:.0},{:.0},{:.1},{:.1},{:.0}",
        s.population, s.households, s.adults, s.employed, s.unemployed,
        d.r, d.c, d.i, d.o, d.move_in,
        c(wk.income()), c(wk.upkeep()), c(wk.balance()), c(wk.road_upkeep), c(wk.building_upkeep), c(wk.utility_upkeep), p.spent,
        s.buildings.total, s.buildings.active, s.buildings.constructing, s.buildings.abandoned,
        s.buildings.no_water, s.buildings.no_power, s.buildings.no_sewage, s.buildings.no_road,
        tb.completed, tb.cancelled, s.traffic.despawned_stuck, s.traffic.no_path, s.traffic.abstract_trips,
        s.traffic.vehicles, s.traffic.pedestrians, s.traffic.trips_in_transit,
        s.traffic.avg_load, s.traffic.max_load, s.traffic.overloaded,
        u8::from(s.connected_to_highway),
        u[0].consumption, u[0].capacity, u[1].consumption, u[1].capacity, u[2].consumption, u[2].capacity,
        s.land.avg_pollution, s.land.avg_value, roads_m,
    );
    for z in &s.zones {
        let _ = write!(r, ",{},{},{},{}", z.jobs, z.painted, z.valid, z.occupied);
        for l in 0..5 {
            let _ = write!(r, ",{}", z.by_level[l]);
        }
    }
    r
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!("usage: balance_m1 <map> <seed> <days> <out-prefix>");
        std::process::exit(2);
    }
    let map = &args[1];
    let seed: u64 = args[2].parse().expect("seed");
    let days: u64 = args[3].parse().expect("days");
    let prefix = &args[4];

    let content_dir = args.get(5).map_or("assets/content", String::as_str);
    let content = Arc::new(ContentDb::load_dir(content_dir).expect("content"));
    let terrain = Arc::new(
        Terrain::load(std::path::Path::new("assets/maps").join(map).as_path()).expect("map"),
    );
    if seed == 0 {
        // Probe: wet 32 m samples per tile row.
        let tiles = TileOwnership::m1_around(terrain.meta().start_tile);
        let (mut wet, mut owned) = (0, 0);
        let mut near: Option<(f32, Vec2)> = None;
        let e = terrain.meta().entry_road.clone().unwrap().stub_end();
        let mut z = 0.0;
        while z < cp_terrain::MAP_SIZE {
            let mut x = 0.0;
            while x < cp_terrain::MAP_SIZE {
                let p = Vec2::new(x, z);
                if tiles.contains_point(p) {
                    owned += 1;
                    if terrain.water_depth(p) >= 2.0 {
                        wet += 1;
                        let d = p.distance(e);
                        if near.is_none_or(|n| d < n.0) {
                            near = Some((d, p));
                        }
                    }
                }
                x += 32.0;
            }
            z += 32.0;
        }
        eprintln!("probe {map}: owned samples {owned}, wet {wet}, nearest wet to stub end {near:?}");
        return;
    }
    let entry = terrain.meta().entry_road.clone().expect("entry road");
    let (origin, fwd) = (entry.stub_end(), entry.direction());
    let side = Vec2::new(-fwd.y, fwd.x);

    let mut sim = SimRunner::with_limits(seed, SimLimits::desktop());
    sim.set_terrain(Arc::clone(&terrain));
    sim.set_content(Arc::clone(&content));
    sim.new_game().expect("new game");
    // Optional scenario funds (currency units), for the "money is not the limit" variant.
    if let Some(f) = args.get(6).and_then(|f| f.parse::<f64>().ok()) {
        sim.world_mut().resource_mut::<Treasury>().balance = cp_core::Cost((f * 100.0) as i64);
    }
    let mut p = Player {
        sim,
        content,
        terrain,
        seed,
        origin,
        fwd,
        side,
        queue: Vec::new(),
        stage: 0,
        last_expand_day: 0,
        built_runs: BTreeSet::new(),
        net_nodes: Vec::new(),
        counts: [0; 8],
        pump_failed: false,
        seeded: false,
        unpainted: 0,
        events: String::new(),
        rejected: 0,
        spent: 0.0,
    };
    // The player lays the lattice at the heading that fits the ground best.
    let base = fwd;
    let mut best: Option<(f32, f32)> = None;
    for deg in [0i32, 15, -15, 30, -30, 45, -45, 60, -60, 75, -75, 90, -90, 105, -105, 120, -120, 135, -135, 150, -150, 165, 180] {
        let a = (deg as f32).to_radians();
        p.fwd = Vec2::new(base.x * a.cos() - base.y * a.sin(), base.x * a.sin() + base.y * a.cos());
        p.side = Vec2::new(-p.fwd.y, p.fwd.x);
        let ok = p
            .stage_runs_all(3)
            .iter()
            .filter(|r| p.road_ok(p.pt(r.0), p.pt(r.1)))
            .count() as f32;
        let spine = p.road_ok(p.pt((0, 0)), p.pt((0, 2)));
        let score = if spine { ok - 0.05 * deg.abs() as f32 } else { -1.0 };
        if best.is_none_or(|b| score > b.0 + 1e-3) {
            best = Some((score, deg as f32));
        }
    }
    let a = best.unwrap().1.to_radians();
    p.fwd = Vec2::new(base.x * a.cos() - base.y * a.sin(), base.x * a.sin() + base.y * a.cos());
    p.side = Vec2::new(-p.fwd.y, p.fwd.x);
    p.log(format!("heading {:.0} deg off the stub (score {:.1})", best.unwrap().1, best.unwrap().0));
    // Day 0: the first streets, power, then water and sewage.
    p.expand();
    p.queue.push(Task::Turbine);
    p.queue.push(Task::WaterRoad);
    p.queue.push(Task::Pump);
    p.queue.push(Task::Outflow);
    let funds = p.balance();
    p.log(format!(
        "start: map {map} seed {seed}, origin ({:.0},{:.0}) fwd ({:.2},{:.2}), funds {funds:.0}",
        origin.x, origin.y, fwd.x, fwd.y
    ));

    let mut csv = csv_header();
    csv.push('\n');
    let end = days * TICKS_PER_DAY;
    let mut next_row = 0u64;
    while p.sim.tick().0 < end {
        let t = p.sim.tick().0;
        if t >= next_row * TICKS_PER_DAY + 96 && next_row <= days {
            csv.push_str(&csv_row(next_row, &p));
            csv.push('\n');
            let s = p.sim.world().resource::<CityStats>();
            eprintln!(
                "day {next_row:>3}: pop {:>6} hh {:>5} unemp {:>5} bal {:>9.0} bld {:>4} abandoned {}",
                s.population,
                s.households,
                s.unemployed,
                p.balance(),
                s.buildings.total,
                s.buildings.abandoned
            );
            next_row += 1;
        }
        p.decide();
        let target = (p.sim.tick().0 / DECIDE_EVERY + 1) * DECIDE_EVERY;
        while p.sim.tick().0 < target {
            p.sim.tick_once();
        }
    }
    csv.push_str(&csv_row(days, &p));
    csv.push('\n');
    // Why does nothing spawn any more? Columns where a level-1 lot of the painted zone fits.
    {
        let zones = p.sim.world().resource::<ZoneGrid>();
        for z in ZONES {
            let defs: Vec<(u8, u8)> = p
                .content
                .buildings()
                .filter(|(_, d)| d.zone == z && d.level == 1 && d.service.is_none())
                .map(|(_, d)| d.lot)
                .collect();
            let (mut cols_ok, mut cols_row0, mut painted) = (0, 0, 0);
            for (_, b) in zones.blocks() {
                for col in 0..b.cols {
                    let i = usize::from(col * cp_zone::BLOCK_DEPTH);
                    if b.zones[i] == z && b.valid[i] {
                        cols_row0 += 1;
                        let fits = defs.iter().any(|&(w, d)| {
                            col + u16::from(w) <= b.cols
                                && u16::from(d) <= cp_zone::BLOCK_DEPTH
                                && (col..col + u16::from(w)).all(|c| {
                                    (0..u16::from(d)).all(|r| {
                                        let j = usize::from(c * cp_zone::BLOCK_DEPTH + r);
                                        b.valid[j] && b.zones[j] == z
                                    })
                                })
                        });
                        cols_ok += u32::from(fits);
                    }
                    painted += u32::from(b.zones[i] == z);
                }
            }
            let _ = writeln!(p.events, "spawn check {z:?}: columns painted {painted}, row-0 valid {cols_row0}, a level-1 lot fits at {cols_ok}");
        }
        let d = p.sim.world().resource::<Demand>();
        let _ = writeln!(p.events, "final demand R {:.1} C {:.1} I {:.1} O {:.1} move_in {:.1}", d.r, d.c, d.i, d.o, d.move_in);
    }
    let _ = writeln!(p.events, "end: rejected commands {}, queue left {:?}", p.rejected, p.queue);
    std::fs::write(format!("{prefix}.csv"), csv).expect("write csv");
    std::fs::write(format!("{prefix}.log"), &p.events).expect("write log");
    eprintln!("done: {prefix}.csv");
}
