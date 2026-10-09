//! `SimView`: the read-only snapshot that client crates (render, UI, tools) read.
//! See `architecture/system-design.md` §13.1.
//!
//! Render and UI code never touch the sim `World`; the app calls [`SimRunner::view`]
//! after stepping and publishes the result as a resource. Domains extend [`SimView`]
//! with their own fields (agent positions, dirty sets, overlay values, stats) as they land.
//! Everything in it must be cheap to clone (`Arc`s and plain values).

use std::sync::Arc;

use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use cp_content::{ContentDb, SimContent};
use cp_core::{CommandLog, CommandQueue, GameTime, SimSpeed, Tick};
use cp_net::JunctionGeometry;
pub use cp_terrain::SimTerrain;
use cp_terrain::{Terrain, TileOwnership};

use crate::checksum::SimChecksum;
use crate::command::SimCommand;
use crate::runner::SimRunner;

/// How full one arena is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArenaFill {
    pub name: &'static str,
    pub used: u32,
    pub capacity: u32,
}

/// Registered arena size gauges. Domain plugins add one per arena through
/// `SimBuilder::register_arena_gauge`; `SimView::stats` reports them.
#[derive(Resource, Default)]
pub struct ArenaGauges(pub(crate) Vec<Gauge>);

/// `(name, capacity, live-element count)`.
pub(crate) type Gauge = (&'static str, u32, fn(&World) -> usize);

impl ArenaGauges {
    pub fn register(&mut self, name: &'static str, capacity: u32, len: fn(&World) -> usize) {
        self.0.push((name, capacity, len));
    }
}

/// Counters for the debug window. Cheap to compute; refreshed with every [`SimView`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SimStats {
    /// Most recent periodic checksum `(tick it was taken at, value)`; one every 256 ticks.
    pub checksum: Option<(Tick, u64)>,
    /// One entry per registered arena, in registration order.
    pub arenas: Vec<ArenaFill>,
    /// Commands queued for a future tick.
    pub commands_pending: usize,
    /// Commands applied so far (length of the command log).
    pub commands_applied: usize,
}

/// Read-only snapshot of the sim after a tick.
#[derive(Resource, Clone)]
pub struct SimView {
    pub tick: Tick,
    pub game_time: GameTime,
    pub speed: SimSpeed,
    /// Shared handle to the terrain; `None` for a sim without a map (headless tests).
    pub terrain: Option<Arc<Terrain>>,
    /// Which tiles the player owns (client camera bounds, build-area overlays).
    pub tiles: TileOwnership,
    pub stats: SimStats,
    /// Derived junction geometry (shortened-segment cut points and polygons) for mesh
    /// building. Compare `junctions.version()` with the last built version; junctions with a
    /// greater `revision` need a rebuild, ids that vanished from the map were removed.
    pub junctions: JunctionGeometry,
}

impl SimRunner {
    /// Puts the loaded map into the sim world. If nothing is owned yet (fresh sim),
    /// ownership starts as `TileOwnership::m1_around(meta.start_tile)`; ownership that
    /// is already set (e.g. restored from a save) is left alone. A save restored
    /// afterwards overwrites it, so call this before `load_into_runner`.
    pub fn set_terrain(&mut self, terrain: Arc<Terrain>) {
        let start = terrain.meta().start_tile;
        let world = self.world_mut();
        world.insert_resource(SimTerrain(terrain));
        if world
            .get_resource::<TileOwnership>()
            .is_none_or(|t| *t == TileOwnership::none())
        {
            world.insert_resource(TileOwnership::m1_around(start));
        }
    }

    /// Puts the content database into the sim world (read by command handlers, e.g.
    /// road defs for `NetCommand`). Call before pushing commands that need content.
    pub fn set_content(&mut self, content: Arc<ContentDb>) {
        self.world_mut().insert_resource(SimContent(content));
    }

    /// Snapshot of the current state for clients.
    pub fn view(&self) -> SimView {
        let tick = self.tick();
        SimView {
            tick,
            game_time: tick.game_time(),
            speed: self.speed(),
            terrain: self
                .world()
                .get_resource::<SimTerrain>()
                .map(|t| Arc::clone(&t.0)),
            tiles: self
                .world()
                .get_resource::<TileOwnership>()
                .copied()
                .unwrap_or_default(),
            stats: self.stats(),
            junctions: self
                .world()
                .get_resource::<JunctionGeometry>()
                .cloned()
                .unwrap_or_default(),
        }
    }

    fn stats(&self) -> SimStats {
        let world = self.world();
        SimStats {
            checksum: world.resource::<SimChecksum>().last,
            arenas: world
                .resource::<ArenaGauges>()
                .0
                .iter()
                .map(|&(name, capacity, len)| ArenaFill {
                    name,
                    used: len(world) as u32,
                    capacity,
                })
                .collect(),
            commands_pending: world.resource::<CommandQueue<SimCommand>>().len(),
            commands_applied: world.resource::<CommandLog<SimCommand>>().len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_terrain::{GRID, MapMeta};

    fn flat_terrain() -> Arc<Terrain> {
        flat_terrain_at((2, 2))
    }

    fn flat_terrain_at(start_tile: (u8, u8)) -> Arc<Terrain> {
        let meta = MapMeta {
            name: "t".into(),
            sea_level: 1.0,
            start_tile,
            external_connections: vec![],
        };
        let n = GRID * GRID;
        Arc::new(Terrain::from_samples(meta, &vec![1000; n], &vec![0; n]).unwrap())
    }

    #[test]
    fn stats_report_gauges_commands_and_checksum() {
        use crate::plugin::{SimBuilder, SimPlugin};
        struct P;
        impl SimPlugin for P {
            fn build(&self, sim: &mut SimBuilder) {
                sim.register_arena_gauge("dummy", 10, |_| 3);
            }
        }
        let mut sim = SimRunner::new(1);
        sim.add_plugin(&P);
        sim.push_command(
            SimCommand::SetSpeed {
                speed: cp_core::SimSpeed::X2,
            },
            Tick(1),
        );
        sim.push_command(
            SimCommand::SetSpeed {
                speed: cp_core::SimSpeed::X1,
            },
            Tick(9999),
        );
        let v0 = sim.view();
        assert_eq!(
            v0.stats.arenas,
            vec![ArenaFill {
                name: "dummy",
                used: 3,
                capacity: 10
            }]
        );
        assert_eq!(
            (v0.stats.commands_pending, v0.stats.commands_applied),
            (2, 0)
        );
        assert_eq!(v0.stats.checksum, None);
        for _ in 0..256 {
            sim.tick_once();
        }
        let s = sim.view().stats;
        assert_eq!((s.commands_pending, s.commands_applied), (1, 1));
        assert_eq!(s.checksum.unwrap().0, Tick(255));
    }

    #[test]
    fn view_tracks_tick_and_shares_terrain() {
        let mut sim = SimRunner::new(1);
        assert!(sim.view().terrain.is_none());
        let t = flat_terrain();
        sim.set_terrain(Arc::clone(&t));
        sim.tick_once();
        sim.tick_once();
        let v = sim.view();
        assert_eq!(v.tick, Tick(2));
        assert_eq!(v.game_time.tick_of_day, 2);
        assert!(Arc::ptr_eq(v.terrain.as_ref().unwrap(), &t));
        assert_eq!(v.tiles, TileOwnership::m1_around((2, 2)));
    }

    #[test]
    fn set_terrain_opens_the_start_tile_and_its_neighbours() {
        for (start, open) in [((0, 0), 4), ((2, 2), 9), ((4, 2), 6), ((4, 4), 4)] {
            let mut sim = SimRunner::new(1);
            assert_eq!(sim.view().tiles, TileOwnership::none());
            sim.set_terrain(flat_terrain_at(start));
            let tiles = sim.view().tiles;
            assert_eq!(tiles.open_count(), open, "{start:?}");
            assert!(tiles.is_open(start.0 as usize, start.1 as usize));
        }
    }

    #[test]
    fn set_terrain_keeps_existing_ownership() {
        let mut sim = SimRunner::new(1);
        sim.world_mut().insert_resource(TileOwnership::only((4, 4)));
        sim.set_terrain(flat_terrain_at((0, 0)));
        assert_eq!(sim.view().tiles, TileOwnership::only((4, 4)));
    }

    #[test]
    fn view_carries_junction_geometry_after_the_network_phase() {
        use bevy_math::Vec3;
        use cp_core::curve::CubicBezier3;
        use cp_net::{Elevation, NetGraph};

        let db = cp_content::ContentDb::load_dir(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/content"
        ))
        .unwrap();
        let road = db.road_id("basic_2l").unwrap();
        let def = db.road(road).clone();
        let mut sim = SimRunner::new(1);
        assert!(sim.view().junctions.is_empty());
        {
            let mut g = sim.world_mut().resource_mut::<NetGraph>();
            for tip in [Vec3::new(80.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 80.0)] {
                g.add_segment(
                    &def,
                    road,
                    CubicBezier3::line(Vec3::ZERO, tip),
                    Elevation::Ground,
                )
                .unwrap();
            }
        }
        sim.tick_once();
        let v = sim.view();
        assert_eq!(v.junctions.len(), 3);
        assert_eq!(v.junctions.version(), 1);
        let corner = v
            .junctions
            .iter()
            .map(|(_, j)| j)
            .find(|j| j.arms.len() == 2)
            .unwrap();
        assert_eq!(corner.polygon.len(), 4);
        // A quiet tick changes nothing; the snapshot shares its data.
        sim.tick_once();
        assert_eq!(sim.view().junctions.version(), 1);
    }
}
