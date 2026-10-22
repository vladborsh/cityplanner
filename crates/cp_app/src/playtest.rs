//! T-293: a scripted new game, played like a player through the real commands and tools, to see
//! how the balance feels after the big changes (T-282 balance, T-280 traffic, T-283 milestones,
//! BUG-095 goods, BUG-097 refunds). The game starts as a new game does (flatlands, the entry
//! highway, the start money, milestone 0); every day the player
//!
//! 1. keeps the utilities ahead of the use (wind turbines, water towers, a sewage treatment plant,
//!    all on land: no water body needed), placed with the placement tool's own preview;
//! 2. builds the services as the milestones open them and the money allows (police, fire, clinic,
//!    landfill; school, cemetery; a park now and then), on the town's outer frontage; with
//!    `PLAYTEST_SERVICES=reactive` police, fire, clinic, landfill and cemetery only where their
//!    problem shows;
//! 3. when the zoned land is nearly full and there is money, adds a row or a column of blocks
//!    (streets drawn with the road tool by its rules, a power line and a pipe along each new
//!    block side) and zones each new block for the demand that is highest (R, C or I);
//! 4. writes down the day ([`Day`]).
//!
//! The town is the lattice of `town:N` (blocks of 20 x 10 cells between street centres) south of
//! the highway's stub. Nothing is set behind the game's back: no demand held, no money given.

use bevy::prelude::*;
use bevy_ecs::message::Messages;
use cp_building::{BuildingFlags, BuildingGrid, BuildingState};
use cp_content::{ContentDb, ServiceKind, SimContent, ZoneType};
use cp_core::time::TICKS_PER_DAY;
use cp_core::{CommandOutcome, Treasury};
use cp_net::build::ZONE_CELL;
use cp_service::{ServiceProblems, ServiceState};
use cp_sim::{SimCommand, SimRunner};
use cp_zone::{ZoneCommand, ZoneGrid};

use crate::grid_shots::Hand;

/// Between street centres: 20 and 10 cells (the blocks of `town:N`).
const DX: f32 = 20.0 * ZONE_CELL;
const DY: f32 = 10.0 * ZONE_CELL;
/// The highway's stub ends here; the town's first street junction is south of it.
const STUB_END: Vec2 = Vec2::new(5120.0, 4348.0);
/// The utility lane runs this far north of the top street: the utilities stand between the two,
/// within reach of one or the other.
const LANE_M: f32 = 110.0;
/// Reactive services (`PLAYTEST_SERVICES=reactive`): the problem each one cures.
const REACTIVE: [(u16, &str); 5] = [
    (ServiceProblems::CRIME, "police_station"),
    (ServiceProblems::FIRE, "fire_station"),
    (ServiceProblems::GARBAGE, "landfill"),
    (ServiceProblems::SICK, "clinic"),
    (ServiceProblems::DEAD, "cemetery"),
];
/// Buildings with a problem before its service is built.
const REACT_AT: u32 = 3;
/// Days a new service gets to cure its problem before another of its kind is built.
const REACT_DAYS: u32 = 3;
/// Money the player keeps back for the running costs before building more.
const RESERVE: f64 = 15_000.0;

/// One game day as the player sees it at its end.
#[derive(Clone, Debug, Default)]
pub struct Day {
    pub day: u32,
    pub money: f64,
    /// Change of the money over the day.
    pub balance: f64,
    pub population: u32,
    pub milestone: String,
    pub buildings: u32,
    pub no_power: u32,
    pub no_water: u32,
    pub no_sewage: u32,
    pub no_goods: u32,
    pub crime: u32,
    pub fire: u32,
    pub garbage: u32,
    pub sick: u32,
    /// T-300: sick residents in all; homes on polluted ground (ground ≥ 10) and their sick.
    pub sick_people: u32,
    pub homes_dirty: u32,
    pub sick_dirty: u32,
    /// Residents of the homes on polluted ground, and of the others.
    pub people_dirty: u32,
    pub people_clean: u32,
    /// Citizens who fell ill over the day: in homes on polluted ground, in the others.
    pub new_sick_dirty: u32,
    pub new_sick_clean: u32,
    pub dead: u32,
    pub abandoned: u32,
    /// Unemployed adults per adult, percent.
    pub unemployment: f32,
    pub avg_load: f32,
    pub overloaded: u32,
    /// Cars removed as stuck over the day.
    pub stuck: u64,
    /// The same by reason over the day (cp_traffic `StuckWhy` order: waiting, in junction, lane
    /// change, conflict stands, full beyond, front other, hard cap, hard cap head).
    pub stuck_why: [u64; 8],
    /// T-306: the visible traffic through the entry over the day (cars put on the road): goods
    /// trucks in and out, tourists' cars, households' move-in cars.
    pub entry: cp_traffic::EntryTraffic,
    /// Filled jobs in industry (the `Industrial` zone and the chains), which the entry's trucks
    /// should grow with (T-306).
    pub industry_jobs: u32,
    pub demand: [f32; 3],
    /// The move-in demand (households moving into homes that stand).
    pub move_in: f32,
    /// Free zoned cells [R, C, I].
    pub free: [usize; 3],
    /// Unemployed adults aged 65 and over (there is no retirement: they count as jobless).
    pub unemployed_old: u32,
    /// The demand's sums of the last cycle (cp_economy::DemandAcc), to read why it says what it
    /// says.
    pub acc: cp_economy::DemandAcc,
    /// What the player did that day.
    pub actions: Vec<String>,
    /// Commands the game refused that day.
    pub refused: Vec<String>,
}

/// The player: the game and the town's lattice.
pub struct Player {
    pub sim: SimRunner,
    db: std::sync::Arc<ContentDb>,
    /// World position of the lattice point (0, 0): a street junction.
    o: Vec2,
    /// Columns `c0..c1` and rows `0..r1` of blocks stand.
    c0: i32,
    c1: i32,
    r1: i32,
    /// Services placed, by building def name.
    placed: Vec<String>,
    /// The day each service was last placed (reactive mode).
    last_placed: std::collections::BTreeMap<String, u32>,
    /// Blocks laid out and not zoned yet (nothing was asked for when they were built).
    spare: Vec<(i32, i32)>,
    /// Buildings of [R, C, I] at the start of the day: a zone whose count did not move while its
    /// demand was up has no lot left (its free cells are rests no lot fits).
    built: [usize; 3],
    stalled: [bool; 3],
    /// The demand's last complete cycle of sums.
    last_acc: cp_economy::DemandAcc,
    /// New sick cases of the day so far: in homes on polluted ground, in the others (T-300).
    new_sick: [u32; 2],
    last_money: f64,
    last_stuck: u64,
    last_why: [u64; 8],
    last_entry: cp_traffic::EntryTraffic,
    day: u32,
    actions: Vec<String>,
    refused: Vec<String>,
}

impl Player {
    /// A new game on flatlands, the town's first two blocks laid out and joined to the highway.
    pub fn new_game() -> Self {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/maps/flatlands");
        let t = cp_terrain::Terrain::load(&root).expect("flatlands");
        let mut sim = crate::build_sim(t, cp_core::SimLimits::desktop(), 7.0, false, None);
        // `PLAYTEST_SEED=n`: another seed of the sim's RNG (1 otherwise), to tell a change's
        // effect from the noise of one run (BUG-102).
        if let Some(seed) = std::env::var("PLAYTEST_SEED")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
        {
            sim.world_mut()
                .insert_resource(cp_core::SimRng::seed_from_u64(seed));
        }
        // `PLAYTEST_CONTENT=dir`: the same game with another content set (a balance experiment
        // without editing the committed one).
        if let Ok(dir) = std::env::var("PLAYTEST_CONTENT") {
            let db = ContentDb::load_dir(&dir).expect("PLAYTEST_CONTENT loads");
            sim.set_content(std::sync::Arc::new(db));
        }
        let db = sim.world().resource::<SimContent>().0.clone();
        let money = sim.world().resource::<Treasury>().balance.0 as f64 / 100.0;
        let mut p = Self {
            sim,
            db,
            o: Vec2::new(STUB_END.x - DX, STUB_END.y + DY),
            c0: 0,
            c1: 0,
            r1: 0,
            placed: Vec::new(),
            last_placed: Default::default(),
            spare: Vec::new(),
            built: [0; 3],
            stalled: [false; 3],
            last_acc: cp_economy::DemandAcc::default(),
            new_sick: [0; 2],
            last_money: money,
            last_stuck: 0,
            last_why: [0; 8],
            last_entry: Default::default(),
            day: 0,
            actions: Vec::new(),
            refused: Vec::new(),
        };
        // The road from the stub to the first junction row, then the first blocks.
        let road = p.db.road_id("basic_2l").expect("basic_2l");
        let (a, b) = (STUB_END - p.o, Vec2::new(DX, 0.0));
        let mut h = Hand::new(&mut p.sim, p.o, road);
        h.street(a, b);
        p.refused.append(&mut h.rejected);
        p.c1 = 0;
        p.grow_east();
        p.grow_east();
        p.grow_south();
        // The utility lane behind the town, joined to the top street at its west end.
        let (a, b) = (p.o + p.at(0, 0), p.o + Vec2::new(0.0, -LANE_M));
        p.links(a, b);
        p.lane(0, p.c1);
        p
    }

    /// A power line and a pipe from `a` to `b` (world).
    fn links(&mut self, a: Vec2, b: Vec2) {
        for kind in [
            cp_content::UtilityKind::Power,
            cp_content::UtilityKind::Water,
        ] {
            self.command(SimCommand::Utility(cp_utility::UtilityCommand::BuildLink {
                kind,
                path: vec![a, b],
            }));
        }
    }

    /// The utility lane (a power line and a pipe [`LANE_M`] north of the top street) from column
    /// line `i0` to `i1`, one link per block width.
    fn lane(&mut self, i0: i32, i1: i32) {
        for i in i0..i1 {
            let y = Vec2::new(0.0, -LANE_M);
            let (a, b) = (self.o + self.at(i, 0) + y, self.o + self.at(i + 1, 0) + y);
            self.links(a, b);
        }
    }

    fn at(&self, i: i32, j: i32) -> Vec2 {
        Vec2::new(i as f32 * DX, j as f32 * DY)
    }

    fn street(&mut self, a: Vec2, b: Vec2) {
        let road = self.db.road_id("basic_2l").expect("basic_2l");
        let mut h = Hand::new(&mut self.sim, self.o, road);
        h.street(a, b);
        self.refused.append(&mut h.rejected);
        // A power line and a pipe along it, one link per block side.
        let (wa, wb) = (self.o + a, self.o + b);
        for kind in [
            cp_content::UtilityKind::Power,
            cp_content::UtilityKind::Water,
        ] {
            self.command(SimCommand::Utility(cp_utility::UtilityCommand::BuildLink {
                kind,
                path: vec![wa, wb],
            }));
        }
    }

    /// Runs one command now and keeps its refusal.
    fn command(&mut self, cmd: SimCommand) -> bool {
        let at = self.sim.tick();
        self.sim.push_command(cmd, at);
        self.sim.tick_once();
        let mut ok = true;
        for o in self
            .sim
            .world_mut()
            .resource_mut::<Messages<CommandOutcome>>()
            .drain()
        {
            if let Err(e) = o.result {
                ok = false;
                self.refused.push(e.to_string());
            }
        }
        ok
    }

    /// A column of blocks east of the town (the first call lays the first block's west side).
    fn grow_east(&mut self) {
        let i = self.c1;
        if self.r1 == 0 {
            // The first column: the top street piece and the side streets of one row.
            self.street(self.at(i, 0), self.at(i + 1, 0));
            self.street(self.at(i, 0), self.at(i, 1));
            self.street(self.at(i + 1, 0), self.at(i + 1, 1));
            self.street(self.at(i, 1), self.at(i + 1, 1));
            self.r1 = 1;
        } else {
            for j in 0..self.r1 {
                self.street(self.at(i + 1, j), self.at(i + 1, j + 1));
            }
            for j in 0..=self.r1 {
                self.street(self.at(i, j), self.at(i + 1, j));
            }
        }
        self.c1 += 1;
        // The lane follows the town east (the first columns get it in `new_game`).
        if self.c1 > 2 {
            self.lane(i, i + 1);
        }
        let blocks: Vec<(i32, i32)> = (0..self.r1).map(|j| (i, j)).collect();
        self.zone_blocks(&blocks);
        self.actions.push(format!("grew east (column {i})"));
    }

    fn grow_west(&mut self) {
        let i = self.c0 - 1;
        for j in 0..self.r1 {
            self.street(self.at(i, j), self.at(i, j + 1));
        }
        for j in 0..=self.r1 {
            self.street(self.at(i, j), self.at(i + 1, j));
        }
        self.c0 = i;
        self.lane(i, i + 1);
        let blocks: Vec<(i32, i32)> = (0..self.r1).map(|j| (i, j)).collect();
        self.zone_blocks(&blocks);
        self.actions.push(format!("grew west (column {i})"));
    }

    fn grow_south(&mut self) {
        let j = self.r1;
        for i in self.c0..=self.c1 {
            self.street(self.at(i, j), self.at(i, j + 1));
        }
        for i in self.c0..self.c1 {
            self.street(self.at(i, j + 1), self.at(i + 1, j + 1));
        }
        self.r1 += 1;
        let blocks: Vec<(i32, i32)> = (self.c0..self.c1).map(|i| (i, j)).collect();
        self.zone_blocks(&blocks);
        self.actions.push(format!("grew south (row {j})"));
    }

    /// Free (valid, unoccupied) cells zoned `zone`.
    fn free_cells(&self, zone: ZoneType) -> usize {
        let zones = self.sim.world().resource::<ZoneGrid>();
        zones
            .blocks()
            .flat_map(|(id, _)| zones.block_cells(id))
            .filter(|c| {
                zones.is_valid(*c) && zones.zone_of(*c) == Some(zone) && !zones.is_occupied(*c)
            })
            .count()
    }

    /// Buildings standing or being built in [R, C, I] zones.
    fn zone_counts(&self) -> [usize; 3] {
        let mut n = [0; 3];
        for (_, b) in self.sim.world().resource::<BuildingGrid>().iter() {
            match self.db.building(b.def).zone {
                ZoneType::ResLow | ZoneType::ResHigh => n[0] += 1,
                ZoneType::ComLow | ZoneType::ComHigh => n[1] += 1,
                ZoneType::Industrial if cp_building::is_growable(self.db.building(b.def)) => {
                    n[2] += 1
                }
                _ => {}
            }
        }
        n
    }

    /// The zone a player would paint next: the highest demand above 5 whose zone has less than
    /// a block of free cells (150) or built nothing yesterday (R first on a tie).
    fn wanted(&self) -> Option<ZoneType> {
        let d = *self.sim.world().resource::<cp_building::Demand>();
        [
            (ZoneType::ResLow, d.r, 0),
            (ZoneType::ComLow, d.c, 1),
            (ZoneType::Industrial, d.i, 2),
        ]
        .into_iter()
        .filter(|(z, v, k)| *v > 5.0 && (self.stalled[*k] || self.free_cells(*z) < 150))
        .map(|(z, v, _)| (z, v))
        .fold(None, |best: Option<(ZoneType, f32)>, (z, v)| match best {
            Some((_, bv)) if bv >= v => best,
            _ => Some((z, v)),
        })
        .map(|(z, _)| z)
    }

    /// New blocks wait as spare until a zone is wanted.
    fn zone_blocks(&mut self, blocks: &[(i32, i32)]) {
        for _ in 0..4 {
            self.sim.tick_once();
        }
        self.spare.extend_from_slice(blocks);
        self.zone_spare();
    }

    /// Zones spare blocks while a zone is wanted.
    fn zone_spare(&mut self) {
        while let Some(zone) = self.wanted() {
            let Some((i, j)) = (!self.spare.is_empty()).then(|| self.spare.remove(0)) else {
                return;
            };
            let (lo, hi) = (self.o + self.at(i, j), self.o + self.at(i + 1, j + 1));
            let cells: Vec<_> = {
                let zones = self.sim.world().resource::<ZoneGrid>();
                zones
                    .blocks()
                    .flat_map(|(id, _)| zones.block_cells(id))
                    .filter(|c| {
                        zones.is_valid(*c) && zones.zone_of(*c).is_none_or(|z| z == ZoneType::None)
                    })
                    .filter(|c| {
                        zones.cell_footprint(*c).is_some_and(|f| {
                            f.c.x > lo.x && f.c.y > lo.y && f.c.x < hi.x && f.c.y < hi.y
                        })
                    })
                    .collect()
            };
            if cells.is_empty() {
                continue;
            }
            let n = cells.len();
            if self.command(SimCommand::Zone(ZoneCommand::PaintZones { cells, zone })) {
                self.actions.push(format!("zoned {n} cells {zone:?}"));
                // The new block has room: the zone is not stalled any more.
                let k = match zone {
                    ZoneType::ResLow => 0,
                    ZoneType::ComLow => 1,
                    _ => 2,
                };
                self.stalled[k] = false;
            }
        }
    }

    fn money(&self) -> f64 {
        self.sim.world().resource::<Treasury>().balance.0 as f64 / 100.0
    }

    fn reached(&self) -> usize {
        self.sim.world().resource::<cp_sim::Progress>().reached as usize
    }

    fn unlocked(&self, def: &str) -> bool {
        self.db.building_id(def).is_some_and(|id| {
            self.db
                .unlock_milestone(id)
                .is_none_or(|m| m <= self.reached())
        })
    }

    fn cost(&self, def: &str) -> f64 {
        self.db
            .building_id(def)
            .map_or(f64::MAX, |id| f64::from(self.db.building(id).cost))
    }

    /// Places `def` on the town's outer frontage where the placement tool says it fits (no
    /// house in the way), as a click would. Whether it was placed.
    fn place(&mut self, def: &str) -> bool {
        let Some(id) = self.db.building_id(def) else {
            return false;
        };
        if !self.unlocked(def) || self.money() < self.cost(def) + RESERVE {
            return false;
        }
        let tool = cp_tools::BuildingTool {
            def: Some(id),
            ..Default::default()
        };
        // North of the top street, where the town never grows (it grows south, east and west):
        // its frontage first, then further back, every half block.
        let mut spots = Vec::new();
        // Within the top street's ends: a column added later must not run into them.
        let (x0, x1) = (self.c0 as f32 * DX + 20.0, self.c1 as f32 * DX - 20.0);
        // Utilities stand within reach of the top street's or the lane's pipe and line
        // (pipe_reach_m 64): further off they would count as capacity and supply nobody.
        let utility = UTILITIES.contains(&def);
        let rows: &[f32] = if utility {
            &[30.0, 50.0, 70.0, 90.0, 140.0]
        } else {
            &[30.0, 75.0, 165.0, 210.0]
        };
        for &back in rows {
            let mut x = x0;
            while x <= x1 {
                spots.push(Vec2::new(x, -back));
                x += DX / 4.0;
            }
        }
        for l in spots {
            let p = self.o + l;
            let view = self.sim.view();
            let Some(g) = cp_tools::building::preview(&view, &tool, p) else {
                continue;
            };
            drop(view);
            if !g.valid || !g.demolish.is_empty() {
                continue;
            }
            let (pos, rot) = match &g.free {
                Some(f) if !f.edge => (f.pos, f.rot),
                _ => (p, tool.rot()),
            };
            if self.command(SimCommand::Building(cp_building::BuildingCommand::Place {
                def: id,
                pos,
                rot,
            })) {
                self.placed.push(def.to_string());
                self.actions.push(format!("placed {def}"));
                return true;
            }
        }
        self.actions.push(format!("no spot for {def}"));
        false
    }

    fn count(&self, def: &str) -> usize {
        self.placed.iter().filter(|d| *d == def).count()
    }

    /// Step 1 and 2 of the module docs.
    fn build(&mut self) {
        let stats = self.sim.view().city.clone();
        // Utilities [water, sewage, power] (kind order): keep 30 % ahead of the use.
        let short = |k: usize| {
            let n = &stats.utilities[k];
            n.capacity < n.consumption * 1.3 + 1.0 || (n.sources == 0 && stats.buildings.total > 0)
        };
        // As many as it takes (a source adds its capacity at once; up to 6 of a kind a day).
        // T-308: the biggest one that is unlocked and affordable first, then the small one.
        for (k, defs) in [
            (2, &["gas_power_plant", "wind_farm", "wind_turbine"][..]),
            (0, &["water_reservoir", "water_tower"][..]),
            (1, &["sewage_treatment_large", "sewage_treatment_plant"][..]),
        ] {
            let def = defs
                .iter()
                .copied()
                .find(|d| self.unlocked(d) && self.money() >= self.cost(d) + RESERVE)
                .unwrap_or(defs[defs.len() - 1]);
            let n = &stats.utilities[k];
            let need = (n.consumption * 1.3 + 1.0 - n.capacity).max(0.0);
            let cap = self
                .db
                .building_id(def)
                .and_then(|id| self.db.building_service(id))
                .map_or(1.0, |s| self.db.service(s).capacity as f32)
                .max(1.0);
            let count = if short(k) {
                ((need / cap).ceil() as usize).clamp(1, 6)
            } else {
                0
            };
            for _ in 0..count {
                if !self.place(def) {
                    break;
                }
            }
        }
        // `PLAYTEST_SERVICES=reactive`: a care service only where its problem shows (T-295).
        if std::env::var("PLAYTEST_SERVICES").is_ok_and(|v| v == "reactive") {
            self.build_services_reactive(stats.population);
            return;
        }
        // Services, one each as they open; a second police and fire station from 3000.
        let pop = stats.population;
        for (def, from_pop, many) in [
            ("clinic", 0, 1),
            ("police_station", 0, 1),
            ("fire_station", 0, 1),
            ("landfill", 0, 1),
            ("elementary_school", 0, 1),
            ("cemetery", 0, 1),
            ("small_park", 400, 1),
            ("police_station", 3000, 2),
            ("fire_station", 3000, 2),
            ("high_school", 1500, 1),
            ("small_park", 2000, 2),
        ] {
            if pop >= from_pop && self.count(def) < many {
                self.place(def);
            }
        }
    }

    /// Services in reactive mode (T-295): police, fire, clinic, landfill and cemetery only once
    /// [`REACT_AT`] buildings have the problem it cures, then one more each [`REACT_DAYS`] days
    /// while it is still there; schools and parks as in the default mode. Shows each problem
    /// appear and get cured.
    fn build_services_reactive(&mut self, pop: u32) {
        let mut seen = [0u32; 5];
        {
            let w = self.sim.world();
            let st = w.resource::<ServiceState>();
            for (id, bl) in w.resource::<BuildingGrid>().iter() {
                if bl.state != BuildingState::Active {
                    continue;
                }
                let p = st.problems_of(id);
                for (k, (flag, _)) in REACTIVE.iter().enumerate() {
                    seen[k] += u32::from(p.has(*flag));
                }
            }
        }
        for (k, (_, def)) in REACTIVE.iter().enumerate() {
            let recent = self
                .last_placed
                .get(*def)
                .is_some_and(|&d| self.day < d + REACT_DAYS);
            if seen[k] >= REACT_AT && !recent {
                self.actions
                    .push(format!("{def}: {} buildings with the problem", seen[k]));
                if self.place(def) {
                    self.last_placed.insert(def.to_string(), self.day);
                }
            }
        }
        for (def, from_pop, many) in [
            ("elementary_school", 0, 1),
            ("small_park", 400, 1),
            ("high_school", 1500, 1),
            ("small_park", 2000, 2),
        ] {
            if pop >= from_pop && self.count(def) < many {
                self.place(def);
            }
        }
    }

    /// Step 3: zone a spare block for what is wanted; with none spare, grow (money allowing).
    fn grow(&mut self) {
        self.zone_spare();
        if self.wanted().is_none() || !self.spare.is_empty() || self.money() < 40_000.0 {
            return;
        }
        // Keep the town about square: rows are half as tall as columns are wide.
        let width = self.c1 - self.c0;
        if self.r1 < width * 2 {
            self.grow_south();
        } else if (self.c1 + self.c0) <= 1 {
            self.grow_east();
        } else {
            self.grow_west();
        }
    }

    /// Plays one day and returns it.
    pub fn play_day(&mut self) -> Day {
        let now = self.zone_counts();
        let d = *self.sim.world().resource::<cp_building::Demand>();
        for (k, v) in [d.r, d.c, d.i].into_iter().enumerate() {
            self.stalled[k] = self.day > 0 && v > 5.0 && now[k] <= self.built[k];
        }
        self.built = now;
        self.build();
        self.grow();
        let start = self.sim.tick().0;
        while self.sim.tick().0 < start + TICKS_PER_DAY {
            self.sim.tick_once();
            self.count_new_sick();
            let a = *self.sim.world().resource::<cp_economy::DemandAcc>();
            if a.seen == 64 {
                self.last_acc = a;
            }
            for o in self
                .sim
                .world_mut()
                .resource_mut::<Messages<CommandOutcome>>()
                .drain()
            {
                if let Err(e) = o.result {
                    self.refused.push(e.to_string());
                }
            }
        }
        self.day += 1;
        self.report()
    }

    /// The citizens who fell ill in the tick just run (the health system visits one bucket a
    /// tick), counted by whether their home stands on polluted ground (T-300).
    fn count_new_sick(&mut self) {
        let w = self.sim.world();
        let tick = cp_core::Tick(self.sim.tick().0 - 1);
        let bucket = (tick.0 % u64::from(cp_citizen::system::CITIZEN_BUCKETS)) as u32;
        let b = w.resource::<BuildingGrid>();
        let e = w.resource::<cp_economy::Emissions>();
        for (_, c) in w
            .resource::<cp_citizen::CitizenGrid>()
            .citizens_bucket(cp_citizen::system::CITIZEN_BUCKETS, bucket)
        {
            if c.sick_since == Some(tick) {
                let dirty = c
                    .home
                    .and_then(|h| b.get(h))
                    .is_some_and(|h| e.ground_at(h.pos) >= DIRTY_GROUND);
                self.new_sick[usize::from(!dirty)] += 1;
            }
        }
    }

    fn report(&mut self) -> Day {
        let s = self.sim.view().city.clone();
        let money = self.money();
        let reached = self.reached();
        let milestone = self
            .db
            .milestones()
            .get(reached.min(self.db.milestones().len().saturating_sub(1)))
            .map(|m| m.name.clone())
            .unwrap_or_default();
        let w = self.sim.world();
        let b = w.resource::<BuildingGrid>();
        let st = w.resource::<ServiceState>();
        let mut d = Day {
            day: self.day,
            money,
            balance: money - self.last_money,
            population: s.population,
            milestone,
            buildings: s.buildings.total,
            no_power: s.buildings.no_power,
            no_water: s.buildings.no_water,
            no_sewage: s.buildings.no_sewage,
            abandoned: s.buildings.abandoned,
            unemployment: if s.adults > 0 {
                100.0 * s.unemployed as f32 / s.adults as f32
            } else {
                0.0
            },
            avg_load: s.traffic.avg_load,
            overloaded: s.traffic.overloaded,
            stuck: s.traffic.despawned_stuck.saturating_sub(self.last_stuck),
            demand: {
                let dm = *w.resource::<cp_building::Demand>();
                [dm.r, dm.c, dm.i]
            },
            move_in: w.resource::<cp_building::Demand>().move_in,
            acc: self.last_acc,
            actions: std::mem::take(&mut self.actions),
            refused: std::mem::take(&mut self.refused),
            ..Default::default()
        };
        let db = &w.resource::<cp_content::SimContent>().0;
        let emissions = w.resource::<cp_economy::Emissions>();
        let dirty = |home: cp_building::BuildingId| {
            b.get(home)
                .is_some_and(|bl| emissions.ground_at(bl.pos) >= DIRTY_GROUND)
        };
        let industrial = |work: cp_building::BuildingId| {
            b.get(work).is_some_and(|bl| {
                let def = db.building(bl.def);
                def.zone == ZoneType::Industrial || def.industry.is_some()
            })
        };
        for (_, c) in w.resource::<cp_citizen::CitizenGrid>().citizens() {
            if c.is_adult() && c.work.is_none() && c.age_days >= 65 * cp_citizen::DAYS_PER_YEAR {
                d.unemployed_old += 1;
            }
            d.industry_jobs += u32::from(c.work.is_some_and(industrial));
            if let Some(home) = c.home {
                if dirty(home) {
                    d.people_dirty += 1;
                } else {
                    d.people_clean += 1;
                }
            }
            if c.sick_since.is_some() {
                d.sick_people += 1;
                d.sick_dirty += u32::from(c.home.is_some_and(dirty));
            }
        }
        [d.new_sick_dirty, d.new_sick_clean] = std::mem::take(&mut self.new_sick);
        d.homes_dirty = b
            .iter()
            .filter(|(id, bl)| {
                bl.state == BuildingState::Active
                    && db.building(bl.def).households > 0
                    && dirty(*id)
            })
            .count() as u32;
        for (id, bl) in b.iter() {
            if bl.state != BuildingState::Active {
                continue;
            }
            if bl.flags.0 & BuildingFlags::NO_GOODS != 0 {
                d.no_goods += 1;
            }
            let p = st.problems_of(id);
            d.crime += u32::from(p.has(ServiceProblems::CRIME));
            d.fire += u32::from(p.has(ServiceProblems::FIRE));
            d.garbage += u32::from(p.has(ServiceProblems::GARBAGE));
            d.sick += u32::from(p.has(ServiceProblems::SICK));
            d.dead += u32::from(p.has(ServiceProblems::DEAD));
        }
        d.free = [
            self.free_cells(ZoneType::ResLow),
            self.free_cells(ZoneType::ComLow),
            self.free_cells(ZoneType::Industrial),
        ];
        let agents = self.sim.world().resource::<cp_traffic::Agents>();
        let why = agents.stuck_reasons();
        for (k, w) in why.iter().enumerate() {
            d.stuck_why[k] = w - self.last_why[k];
        }
        self.last_why = why;
        let (e, l) = (agents.entry, self.last_entry);
        d.entry = cp_traffic::EntryTraffic {
            trucks_in: e.trucks_in - l.trucks_in,
            trucks_out: e.trucks_out - l.trucks_out,
            tourist_cars: e.tourist_cars - l.tourist_cars,
            move_in_cars: e.move_in_cars - l.move_in_cars,
        };
        self.last_entry = e;
        self.last_money = money;
        self.last_stuck = s.traffic.despawned_stuck;
        let _ = ServiceKind::Police;
        d
    }
}

impl Player {
    /// BUG-100 diagnostics: the cars that have not got onto the road yet (waiting for a gap at
    /// their start), by start lane: the lane's road, segment and where it is, how many wait there,
    /// how long the longest has stood (ticks), their vehicle kinds, and the cars driving on it.
    pub fn waiting_report(&self) -> String {
        use std::collections::BTreeMap;
        let w = self.sim.world();
        let agents = w.resource::<cp_traffic::Agents>();
        let net = w.resource::<cp_net::NetGraph>();
        let pnet = w.resource::<cp_traffic::PathNetwork>();
        let now = w.resource::<cp_core::Tick>().0;
        let mut by_lane: BTreeMap<cp_net::LaneId, (usize, u32, BTreeMap<String, usize>)> =
            BTreeMap::new();
        let mut driving: BTreeMap<cp_net::LaneId, usize> = BTreeMap::new();
        let mut total = 0;
        for (_, v) in agents.vehicles() {
            if v.spawned {
                *driving.entry(v.lane()).or_default() += 1;
                continue;
            }
            total += 1;
            let e = by_lane.entry(v.lane()).or_default();
            e.0 += 1;
            e.1 = e.1.max(v.stuck_timer >> 16);
            // What the car is: a citizen's trip by purpose, else a service run (tourists, trucks).
            let what = w
                .resource::<cp_citizen::TripBook>()
                .get(v.trip)
                .map_or(format!("run kind {}", v.kind_id()), |t| {
                    format!("{:?}", t.purpose)
                });
            *e.2.entry(what).or_default() += 1;
        }
        let mut rows: Vec<_> = by_lane.into_iter().collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.1.0));
        let mut out = format!("waiting {total} on {} lanes;", rows.len());
        // The fullest segments (cars on the road) with their mean speed: where a queue stands.
        let mut seg: BTreeMap<cp_net::SegmentId, (usize, f32)> = BTreeMap::new();
        for (_, v) in agents.vehicles().filter(|(_, v)| v.spawned && !v.in_conn) {
            if let Some(l) = net.lane(v.lane()) {
                let e = seg.entry(l.segment).or_default();
                e.0 += 1;
                e.1 += v.speed;
            }
        }
        // The pieces between the highway's end and the town (the stub and the road in).
        out += " entry pieces:";
        for (id, sg) in net.segments() {
            let (a, b) = (sg.curve.point(0.0), sg.curve.point(1.0));
            let near = |p: bevy::math::Vec3| {
                (5080.0..5160.0).contains(&p.x) && (4250.0..4440.0).contains(&p.z)
            };
            if !near(a) && !near(b) {
                continue;
            }
            let (n, sum) = seg.get(&id).copied().unwrap_or((0, 0.0));
            out += &format!(
                " [{} {} ({:.0},{:.0})->({:.0},{:.0}) {n} cars {:.1} m/s]",
                id.index,
                self.db.road(sg.road).id,
                a.x,
                a.z,
                b.x,
                b.z,
                if n > 0 { sum / n as f32 } else { 0.0 }
            );
        }
        out += ";";
        // The cars at and in the town's first crossing (5120, 4428): lane end (s vs length),
        // speed, inside the junction or not, ticks standing.
        out += " crossing cars:";
        for (vid, v) in agents.vehicles().filter(|(_, v)| v.spawned) {
            let Some(l) = net.lane(v.lane()) else {
                continue;
            };
            let Some(sg) = net.segment(l.segment) else {
                continue;
            };
            let end = if l.dir == cp_content::LaneDir::Forward {
                sg.curve.point(1.0)
            } else {
                sg.curve.point(0.0)
            };
            let at = |p: bevy::math::Vec3| (p.x - 5120.0).abs() < 2.0 && (p.z - 4428.0).abs() < 2.0;
            let at_end = at(end);
            // A car inside the junction reports a lane of either end there (its way in or out).
            let touches = at(sg.curve.point(0.0)) || at(sg.curve.point(1.0));
            let left = sg.length - v.s.min(sg.length);
            if (v.in_conn && touches) || (at_end && left < 12.0) {
                out += &format!(
                    " [seg {} {:?} s {:.0}/{:.0} v {:.1} {} still {}]",
                    l.segment.index,
                    l.dir,
                    v.s,
                    sg.length,
                    v.speed,
                    if v.in_conn { "IN-JUNCTION" } else { "lane" },
                    v.stuck_timer >> 16
                );
                if !v.in_conn {
                    out += &format!(" {{{}}}", agents.gate_report(pnet, now, vid));
                }
            }
        }
        out += ";";
        // The inbound highway, piece by piece from the map edge: where its queue ends.
        out += " highway:";
        for (id, sg) in net.segments() {
            if !self.db.road(sg.road).id.starts_with("highway") {
                continue;
            }
            let (n, sum) = seg.get(&id).copied().unwrap_or((0, 0.0));
            let a = sg.curve.point(0.0);
            out += &format!(
                " {}@{:.0}:{n}/{:.1}",
                id.index,
                a.z,
                if n > 0 { sum / n as f32 } else { 0.0 }
            );
        }
        out += ";";
        let mut full: Vec<_> = seg.into_iter().collect();
        full.sort_by_key(|r| std::cmp::Reverse(r.1.0));
        out += " fullest segments:";
        for (id, (n, sum)) in full.into_iter().take(8) {
            if let Some(sg) = net.segment(id) {
                let (a, b) = (sg.curve.point(0.0), sg.curve.point(1.0));
                out += &format!(
                    " [{} {:?} ({:.0},{:.0})->({:.0},{:.0}) {n} cars, mean {:.1} m/s]",
                    self.db.road(sg.road).id,
                    id,
                    a.x,
                    a.z,
                    b.x,
                    b.z,
                    sum / n as f32
                );
            }
        }
        out += ";";
        // The cars on the worst lane: where they stand and how fast they go.
        if let Some((lane, _)) = rows.first() {
            let mut on: Vec<(f32, f32, u32)> = agents
                .vehicles()
                .filter(|(_, v)| v.spawned && v.lane() == *lane)
                .map(|(_, v)| (v.s, v.speed, v.stuck_timer >> 16))
                .collect();
            on.sort_by(|a, b| a.0.total_cmp(&b.0));
            let len = net
                .lane(*lane)
                .and_then(|l| net.segment(l.segment))
                .map_or(0.0, |sg| sg.length);
            out += &format!(" worst lane {len:.0} m long, cars (s, speed, still ticks):");
            for (s_, v_, t_) in on.iter().take(14) {
                out += &format!(" ({s_:.0},{v_:.1},{t_})");
            }
            out += ";";
        }
        for (lane, (n, still, kinds)) in rows.into_iter().take(6) {
            let (road, seg, at) = net
                .lane(lane)
                .and_then(|l| Some((l, net.segment(l.segment)?)))
                .map_or(("?".to_string(), String::new(), Vec2::ZERO), |(l, sg)| {
                    let p = sg.curve.point(0.5);
                    (
                        self.db.road(sg.road).id.clone(),
                        format!("{:?} {:?}", l.segment, l.dir),
                        Vec2::new(p.x, p.z),
                    )
                });
            out += &format!(
                " [{road} {seg} at ({:.0},{:.0}): {n} waiting, longest {still} ticks, kinds {kinds:?}, {} driving]",
                at.x,
                at.y,
                driving.get(&lane).copied().unwrap_or(0)
            );
        }
        out
    }
}

/// Plays a new game until `target` residents or `max_days` days; `each` sees every day.
/// Ground pollution from which a home counts as standing on polluted ground in the report.
const DIRTY_GROUND: f32 = 10.0;

/// The water, sewage and power buildings, small ones and the bigger ones that unlock later.
const UTILITIES: [&str; 8] = [
    "water_tower",
    "water_reservoir",
    "sewage_treatment_plant",
    "sewage_treatment_large",
    "wind_turbine",
    "wind_farm",
    "gas_power_plant",
    "coal_power_plant",
];

pub fn play(target: u32, max_days: u32, mut each: impl FnMut(&Day)) -> Vec<Day> {
    let mut p = Player::new_game();
    let mut days = Vec::new();
    let waiting = std::env::var("PLAYTEST_WAITING").is_ok();
    for _ in 0..max_days {
        let d = p.play_day();
        each(&d);
        if waiting {
            eprintln!("      {}", p.waiting_report());
        }
        let done = d.population >= target;
        days.push(d);
        if done {
            break;
        }
    }
    // T-308: how many of each utility the bot needed.
    let mut utilities = std::collections::BTreeMap::new();
    for def in &p.placed {
        if UTILITIES.contains(&def.as_str()) {
            *utilities.entry(def.as_str()).or_insert(0u32) += 1;
        }
    }
    eprintln!("utility buildings placed: {utilities:?}");
    days
}
