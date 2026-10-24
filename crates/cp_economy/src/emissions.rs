//! Pollution and noise on a 16 m grid (T-300, `architecture/system-design.md` §11.1).
//!
//! * **Sources** ([`emission_of`], the one rule the sim and the build ghost share): a building's
//!   own `emits`, else the balance rule of its zone or industry, else nothing. A source gives
//!   `s × (1 − d / r)` within its radius `r` (zone cells of 8 m) of its centre; pollution adds
//!   up, noise takes the loudest, both capped at 100. Highways are loud by their load.
//! * **Ground** (`ground`, hundredths, saved): every [`update_city`](crate::update_city) pass
//!   (256 ticks) it steps toward the source field: up by `src / rise_steps` (a source brings the
//!   ground to its strength in `pollution_rise_days`), down by `100 / decay_steps` (100 is gone
//!   in `pollution_decay_days`, less sooner). Integer arithmetic, so a replay matches.
//!
//! Buildings are visited by id and road segments by id, cells in index order: the float sums
//! come out the same every run.

use std::sync::atomic::{AtomicU64, Ordering};

use bevy_ecs::resource::Resource;
use bevy_math::Vec2;
use cp_building::{Building, BuildingGrid, BuildingId, BuildingState};
use cp_content::{BuildingDef, ContentDb, Emit, ResourceKind, RoadClass, ZoneType};
use cp_core::time::TICKS_PER_DAY;
use cp_net::NetGraph;
use cp_terrain::MAP_SIZE;
use cp_traffic::SegmentLoad;
use cp_utility::WaterPools;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::CITY_PERIOD;

/// Side of a cell, metres (2 × 2 zone cells).
pub const CELL_E: f32 = 16.0;
/// Cells per side.
pub const SIDE_E: usize = (MAP_SIZE / CELL_E) as usize;
/// Metres per zone cell, the unit of a source's radius.
pub const ZONE_CELL_M: f32 = 8.0;
/// Distance between the sample points along a highway, metres.
const SAMPLE_M: f32 = 32.0;
/// `ground` value of 100.
const FULL: u32 = 10_000;

/// The pollution and noise of a building of `def`: its own `emits`, else the zone or industry
/// rule of the balance (the strength of a zone building grows by `emit_level_step` per level
/// above 1), else none. Strengths are capped at 100.
pub fn emission_of(def: &BuildingDef, db: &ContentDb) -> (Option<Emit>, Option<Emit>) {
    if let Some(e) = def.emits {
        return (e.pollution, e.noise);
    }
    let bal = db.balance();
    let emit = |strength: f32, radius: f32| {
        (strength > 0.0 && radius >= 1.0).then(|| Emit {
            strength: strength.min(100.0),
            radius_cells: radius.min(16.0) as u8,
        })
    };
    if let Some(ind) = &def.industry {
        let (pollution, radius) = match ind.resource {
            Some(r) => (
                bal.pollution_industry_res[r as usize],
                if r == ResourceKind::Oil {
                    bal.pollution_oil_radius_cells
                } else {
                    bal.pollution_radius_cells
                },
            ),
            // A factory (products of any area) and any chain building without a resource count
            // as plain industry.
            None => (bal.pollution_industry, bal.pollution_radius_cells),
        };
        return (
            emit(pollution, radius),
            emit(bal.noise_industry, bal.noise_radius_cells),
        );
    }
    if !cp_building::is_growable(def) {
        return (None, None);
    }
    let level = 1.0 + bal.emit_level_step * f32::from(def.level.saturating_sub(1));
    match def.zone {
        ZoneType::Industrial => (
            emit(bal.pollution_industry * level, bal.pollution_radius_cells),
            emit(bal.noise_industry * level, bal.noise_radius_cells),
        ),
        ZoneType::ComHigh => (
            None,
            emit(bal.noise_com_high * level, bal.noise_com_radius_cells[0]),
        ),
        ZoneType::ComLow => (
            None,
            emit(bal.noise_com_low * level, bal.noise_com_radius_cells[1]),
        ),
        _ => (None, None),
    }
}

/// The 16 m pollution and noise grids. `ground` and `noise` are sim state (saved, in the
/// checksum): health reads the noise every tick, so a game loaded between two passes must have
/// the noise of the last pass, not a rebuild from the state at the save (highway loads and new
/// buildings change in between). The source field is only read by the pass that builds it.
#[derive(Resource, Clone, Debug, Serialize, Deserialize)]
pub struct Emissions {
    /// What the sources give each cell now, `0..=100`. Not saved.
    #[serde(skip, default = "zeros")]
    pub pollution_src: Vec<f32>,
    /// Pollution of the ground, hundredths (`0..=10 000`), row-major `z × SIDE_E + x`. Saved as
    /// the list of its non-zero cells.
    #[serde(
        serialize_with = "ser_ground",
        deserialize_with = "de_ground",
        default = "ground_zeros"
    )]
    pub ground: Vec<u16>,
    /// Noise of the last pass, hundredths (`0..=10 000`), row-major. Saved as the list of its
    /// non-zero cells.
    #[serde(
        serialize_with = "ser_ground",
        deserialize_with = "de_ground",
        default = "ground_zeros"
    )]
    pub noise: Vec<u16>,
    /// The loudest source of each cell while a pass builds the fields. Not saved.
    #[serde(skip, default = "zeros")]
    loud: Vec<f32>,
    /// The dirt of the water each home drinks (T-300 step 2), hundredths, ascending by id, only
    /// the homes whose water is not clean. Rebuilt every pass from the water pools and the
    /// ground; saved, so a game loaded between passes falls ill like the one that went on.
    #[serde(default)]
    pub water_dirt: Vec<(BuildingId, u16)>,
    /// Changes whenever the grids are rebuilt and is never shared between instances, so a
    /// snapshot cache can key on it. Not saved, not compared.
    #[serde(skip, default = "next_revision")]
    revision: u64,
}

static REVISIONS: AtomicU64 = AtomicU64::new(1);

fn next_revision() -> u64 {
    REVISIONS.fetch_add(1, Ordering::Relaxed)
}

fn zeros() -> Vec<f32> {
    vec![0.0; SIDE_E * SIDE_E]
}

fn ground_zeros() -> Vec<u16> {
    vec![0; SIDE_E * SIDE_E]
}

fn ser_ground<S: Serializer>(ground: &[u16], s: S) -> Result<S::Ok, S::Error> {
    let cells: Vec<(u32, u16)> = ground
        .iter()
        .enumerate()
        .filter(|(_, g)| **g > 0)
        .map(|(i, g)| (i as u32, *g))
        .collect();
    cells.serialize(s)
}

fn de_ground<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u16>, D::Error> {
    let cells = Vec::<(u32, u16)>::deserialize(d)?;
    let mut ground = ground_zeros();
    for (i, g) in cells {
        let slot = ground
            .get_mut(i as usize)
            .ok_or_else(|| serde::de::Error::custom(format!("ground cell {i} off the map")))?;
        *slot = g;
    }
    Ok(ground)
}

impl Default for Emissions {
    fn default() -> Self {
        Self {
            pollution_src: zeros(),
            ground: ground_zeros(),
            noise: ground_zeros(),
            loud: zeros(),
            water_dirt: Vec::new(),
            revision: next_revision(),
        }
    }
}

impl PartialEq for Emissions {
    /// The saved state: the ground, the noise and the water.
    fn eq(&self, other: &Self) -> bool {
        self.ground == other.ground
            && self.noise == other.noise
            && self.water_dirt == other.water_dirt
    }
}

/// Everything [`Emissions::update`] reads.
pub struct EmissionInputs<'a> {
    pub content: &'a ContentDb,
    pub buildings: &'a BuildingGrid,
    pub net: &'a NetGraph,
    pub load: &'a SegmentLoad,
    /// Extra pollution share of the buildings a grey deal targets, sorted by id (T-325,
    /// `CorruptionState::pollution_boost`).
    pub boost: &'a [(BuildingId, f32)],
}

/// Calls `f(cell, distance from its centre to p)` for every cell whose centre is within
/// `radius` of `p`, in index order.
fn cells_near(p: Vec2, radius: f32, mut f: impl FnMut(usize, f32)) {
    let lo = |v: f32| ((v - radius) / CELL_E).floor().max(0.0) as usize;
    let hi = |v: f32| (((v + radius) / CELL_E).floor().max(0.0) as usize).min(SIDE_E - 1);
    let r2 = radius * radius;
    for z in lo(p.y)..=hi(p.y) {
        let dz = (z as f32 + 0.5) * CELL_E - p.y;
        for x in lo(p.x)..=hi(p.x) {
            let dx = (x as f32 + 0.5) * CELL_E - p.x;
            let d2 = dx * dx + dz * dz;
            if d2 < r2 {
                f(z * SIDE_E + x, d2.sqrt());
            }
        }
    }
}

/// The cell of a world position (off the map clamps to the edge).
pub fn cell_of(p: Vec2) -> usize {
    let c = |v: f32| ((v / CELL_E).floor().max(0.0) as usize).min(SIDE_E - 1);
    c(p.y) * SIDE_E + c(p.x)
}

/// Passes of [`update_city`](crate::update_city) in `days` game days, at least 1.
fn passes(days: f32) -> u32 {
    ((days * (TICKS_PER_DAY / CITY_PERIOD) as f32).round() as u32).max(1)
}

/// One step of a ground cell toward its source `src` (both hundredths): up by `src / rise`, down
/// by `FULL / decay`, each at least 1, never past the source.
pub fn step_ground(g: u32, src: u32, rise: u32, decay: u32) -> u32 {
    if src >= g {
        (g + src.div_ceil(rise).max(1)).min(src)
    } else {
        g.saturating_sub(FULL.div_ceil(decay).max(1)).max(src)
    }
}

impl Emissions {
    /// See the field of the same name.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Ground pollution at a world position, `0..=100`.
    pub fn ground_at(&self, p: Vec2) -> f32 {
        f32::from(self.ground[cell_of(p)]) / 100.0
    }

    /// Noise at a world position, `0..=100`.
    pub fn noise_at(&self, p: Vec2) -> f32 {
        f32::from(self.noise[cell_of(p)]) / 100.0
    }

    /// One pass: [`Self::rebuild_fields`], then [`Self::step_ground`].
    pub fn update(&mut self, i: &EmissionInputs) {
        self.rebuild_fields(i);
        self.step_ground(i.content);
        self.revision = next_revision();
    }

    /// The source field and the noise of the city as it stands.
    pub fn rebuild_fields(&mut self, i: &EmissionInputs) {
        let db = i.content;
        let bal = db.balance();
        // The buffers of the last pass, cleared (two 1.6 MB grids, not allocated anew).
        let mut src = std::mem::take(&mut self.pollution_src);
        let mut noise = std::mem::take(&mut self.loud);
        for grid in [&mut src, &mut noise] {
            grid.clear();
            grid.resize(SIDE_E * SIDE_E, 0.0);
        }
        for (id, b) in i.buildings.iter() {
            if b.state != BuildingState::Active {
                continue;
            }
            let def = db.building(b.def);
            let (pollution, loud) = emission_of(def, db);
            if let Some(e) = pollution {
                let r = f32::from(e.radius_cells) * ZONE_CELL_M;
                let boost = i
                    .boost
                    .binary_search_by_key(&id, |&(b, _)| b)
                    .map_or(1.0, |k| 1.0 + i.boost[k].1);
                let s = e.strength * boost;
                cells_near(b.pos, r, |c, d| src[c] += s * (1.0 - d / r));
            }
            // A specialised building adds its own noise (T-196), the louder counting.
            let spec = def
                .spec
                .and_then(|k| db.spec(k))
                .map_or(0.0, |s| s.noise_add);
            let loud = match (loud, spec > 0.0) {
                (Some(e), true) if e.strength >= spec => Some(e),
                (_, true) => Some(Emit {
                    strength: spec.min(100.0),
                    radius_cells: bal.noise_radius_cells.clamp(1.0, 16.0) as u8,
                }),
                (e, false) => e,
            };
            if let Some(e) = loud {
                let r = f32::from(e.radius_cells) * ZONE_CELL_M;
                cells_near(b.pos, r, |c, d| {
                    noise[c] = noise[c].max(e.strength * (1.0 - d / r))
                });
            }
        }
        let r = bal.noise_highway_radius_cells * ZONE_CELL_M;
        if r > 0.0 && bal.noise_highway_per_load > 0.0 {
            for (id, seg) in i.net.segments() {
                if db.road(seg.road).class != RoadClass::Highway {
                    continue;
                }
                let loud = (bal.noise_highway_per_load
                    * i.load.ema.get(id.index as usize).copied().unwrap_or(0.0))
                .min(100.0);
                if loud <= 0.0 {
                    continue;
                }
                let n = (seg.length / SAMPLE_M).ceil().max(1.0) as usize;
                for k in 0..=n {
                    let p = seg.curve.point(k as f32 / n as f32);
                    cells_near(Vec2::new(p.x, p.z), r, |c, d| {
                        noise[c] = noise[c].max(loud * (1.0 - d / r))
                    });
                }
            }
        }
        for s in &mut src {
            *s = s.min(100.0);
        }
        if self.noise.len() != SIDE_E * SIDE_E {
            self.noise = ground_zeros();
        }
        for (out, n) in self.noise.iter_mut().zip(&noise) {
            *out = (n.min(100.0) * 100.0).round() as u16;
        }
        self.pollution_src = src;
        self.loud = noise;
    }

    /// One step of the ground toward the source field of [`Self::rebuild_fields`].
    pub fn step_ground(&mut self, db: &ContentDb) {
        let bal = db.balance();
        let (rise, decay) = (
            passes(bal.pollution_rise_days),
            passes(bal.pollution_decay_days),
        );
        for (c, s) in self.pollution_src.iter().enumerate() {
            let g = u32::from(self.ground[c]);
            if g == 0 && *s <= 0.0 {
                continue;
            }
            let target = (*s * 100.0).round() as u32;
            self.ground[c] = step_ground(g, target.min(FULL), rise, decay) as u16;
        }
    }

    /// The dirt of a water source (T-300): the most polluted ground under its lot, `0..=100`.
    pub fn source_dirt(&self, b: &Building) -> f32 {
        let (w, d) = b.size();
        let reach = (f32::from(w).hypot(f32::from(d))) * cp_building::FOOTPRINT_CELL_M * 0.5;
        let mut worst = self.ground[cell_of(b.pos)];
        cells_near(b.pos, reach + CELL_E, |c, _| {
            let centre = Vec2::new(
                ((c % SIDE_E) as f32 + 0.5) * CELL_E,
                ((c / SIDE_E) as f32 + 0.5) * CELL_E,
            );
            if b.footprint_contains(centre) {
                worst = worst.max(self.ground[c]);
            }
        });
        f32::from(worst) / 100.0
    }

    /// The dirt of the water `home` drinks, `0..=100` (0 when clean or without water).
    pub fn water_dirt_of(&self, home: BuildingId) -> f32 {
        self.water_dirt
            .binary_search_by_key(&home, |(id, _)| *id)
            .map_or(0.0, |i| f32::from(self.water_dirt[i].1) / 100.0)
    }

    /// Rebuilds [`Self::water_dirt`]: a pool's water is as dirty as the capacity-weighted mean
    /// of its sources' [`Self::source_dirt`]; every building that drinks from it gets that.
    pub fn update_water(&mut self, pools: &WaterPools, buildings: &BuildingGrid) {
        let dirt: Vec<u16> = pools
            .pools
            .iter()
            .map(|pool| {
                let (mut sum, mut cap) = (0.0f32, 0.0f32);
                for (id, c) in &pool.sources {
                    if let Some(b) = buildings.get(*id) {
                        sum += self.source_dirt(b) * c;
                        cap += c;
                    }
                }
                if cap > 0.0 {
                    ((sum / cap) * 100.0).round().clamp(0.0, FULL as f32) as u16
                } else {
                    0
                }
            })
            .collect();
        self.water_dirt = pools
            .pool_of
            .iter()
            .filter_map(|(id, p)| {
                let w = dirt.get(*p as usize).copied().unwrap_or(0);
                (w > 0).then_some((*id, w))
            })
            .collect();
    }

    /// The mean of the 4 × 4 cells under each cell of the 64 m land grid: `(pollution from the
    /// ground, noise)`, `0..=100`.
    pub fn downsample(&self, side: usize) -> (Vec<f32>, Vec<f32>) {
        let k = SIDE_E / side;
        let mut pollution = vec![0.0; side * side];
        let mut noise = vec![0.0; side * side];
        for z in 0..SIDE_E {
            let row = z * SIDE_E;
            let out = (z / k) * side;
            let (ground, loud) = (
                &self.ground[row..row + SIDE_E],
                &self.noise[row..row + SIDE_E],
            );
            for (o, (g, n)) in ground.chunks_exact(k).zip(loud.chunks_exact(k)).enumerate() {
                pollution[out + o] += g.iter().map(|v| u32::from(*v)).sum::<u32>() as f32;
                noise[out + o] += n.iter().map(|v| u32::from(*v)).sum::<u32>() as f32;
            }
        }
        let n = (k * k) as f32;
        for v in pollution.iter_mut().chain(&mut noise) {
            *v /= n * 100.0;
        }
        (pollution, noise)
    }
}
