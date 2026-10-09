//! `SimView`: the read-only snapshot that client crates (render, UI, tools) read.
//! See `architecture/system-design.md` §13.1.
//!
//! Render and UI code never touch the sim `World`; the app calls [`SimRunner::view`]
//! after stepping and publishes the result as a resource. Domains extend [`SimView`]
//! with their own fields (agent positions, dirty sets, overlay values, stats) as they land.
//! Everything in it must be cheap to clone (`Arc`s and plain values).

use std::sync::Arc;

use bevy_ecs::resource::Resource;
use cp_core::{GameTime, SimSpeed, Tick};
use cp_terrain::Terrain;

use crate::runner::SimRunner;

/// The loaded map, as sim state. Immutable for the lifetime of a game.
#[derive(Resource, Clone)]
pub struct SimTerrain(pub Arc<Terrain>);

/// Read-only snapshot of the sim after a tick.
#[derive(Resource, Clone)]
pub struct SimView {
    pub tick: Tick,
    pub game_time: GameTime,
    pub speed: SimSpeed,
    /// Shared handle to the terrain; `None` for a sim without a map (headless tests).
    pub terrain: Option<Arc<Terrain>>,
}

impl SimRunner {
    /// Puts the loaded map into the sim world.
    pub fn set_terrain(&mut self, terrain: Arc<Terrain>) {
        self.world_mut().insert_resource(SimTerrain(terrain));
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_terrain::{GRID, MapMeta};

    fn flat_terrain() -> Arc<Terrain> {
        let meta = MapMeta {
            name: "t".into(),
            sea_level: 1.0,
            start_tile: (2, 2),
            external_connections: vec![],
        };
        let n = GRID * GRID;
        Arc::new(Terrain::from_samples(meta, &vec![1000; n], &vec![0; n]).unwrap())
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
    }
}
