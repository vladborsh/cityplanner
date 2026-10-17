//! Map loading through the asset server, so the same code reads files natively and fetches
//! over HTTP in the browser. Files arrive as raw bytes and go through
//! `Terrain::from_bytes`; the sim then owns the terrain.

use bevy::asset::io::Reader;
use bevy::asset::{AssetLoader, LoadContext};
use bevy::prelude::*;
use cp_terrain::Terrain;

use crate::{GameConfig, Sim, build_sim};

/// An asset file read as-is (no decoding by Bevy).
#[derive(Asset, TypePath)]
pub struct RawFile(pub Vec<u8>);

#[derive(Default, TypePath)]
struct RawFileLoader;

impl AssetLoader for RawFileLoader {
    type Asset = RawFile;
    type Settings = ();
    type Error = std::io::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _: &(),
        _: &mut LoadContext<'_>,
    ) -> Result<RawFile, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        Ok(RawFile(bytes))
    }

    fn extensions(&self) -> &[&str] {
        &["ron", "png"]
    }
}

/// Set when the map cannot be loaded; the app should report it and exit.
#[derive(Resource, Debug)]
pub struct MapLoadFailed(pub String);

#[derive(Resource)]
struct PendingMap {
    ron: Handle<RawFile>,
    height: Handle<RawFile>,
    water: Handle<RawFile>,
    /// Optional: a map without `resources.png` has no resources (T-205).
    resources: Handle<RawFile>,
}

pub(crate) struct MapPlugin;

impl Plugin for MapPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<RawFile>()
            .register_asset_loader(RawFileLoader)
            .add_systems(Startup, start_loading)
            .add_systems(PreUpdate, finish_loading);
    }
}

fn start_loading(mut commands: Commands, assets: Res<AssetServer>, cfg: Res<GameConfig>) {
    let load = |file: &str| assets.load::<RawFile>(format!("maps/{}/{file}", cfg.map));
    commands.insert_resource(PendingMap {
        ron: load("map.ron"),
        height: load("height.png"),
        water: load("water.png"),
        resources: load("resources.png"),
    });
}

fn finish_loading(
    mut commands: Commands,
    pending: Option<Res<PendingMap>>,
    files: Res<Assets<RawFile>>,
    assets: Res<AssetServer>,
    cfg: Res<GameConfig>,
) {
    let Some(p) = pending else { return };
    for h in [&p.ron, &p.height, &p.water] {
        if let Some(bevy::asset::LoadState::Failed(e)) = assets.get_load_state(h) {
            commands.remove_resource::<PendingMap>();
            commands.insert_resource(MapLoadFailed(format!("map '{}': {e}", cfg.map)));
            return;
        }
    }
    let (Some(ron), Some(height), Some(water)) =
        (files.get(&p.ron), files.get(&p.height), files.get(&p.water))
    else {
        return;
    };
    // `resources.png` may be missing; wait for it only while it is still loading.
    let resources = match assets.get_load_state(&p.resources) {
        Some(bevy::asset::LoadState::Failed(_) | bevy::asset::LoadState::NotLoaded) => None,
        _ => match files.get(&p.resources) {
            Some(r) => Some(r.0.as_slice()),
            None => return,
        },
    };
    commands.remove_resource::<PendingMap>();
    let terrain = std::str::from_utf8(&ron.0)
        .map_err(|e| e.to_string())
        .and_then(|text| {
            Terrain::from_bytes_with_resources(text, &height.0, &water.0, resources)
                .map_err(|e| e.to_string())
        });
    match terrain {
        Ok(terrain) => {
            let sim = build_sim(
                terrain,
                cfg.limits,
                cfg.hour,
                cfg.phase_timing,
                cfg.scene.as_deref(),
            );
            commands.insert_resource(sim.view());
            commands.insert_resource(Sim(sim));
        }
        Err(e) => {
            commands.insert_resource(MapLoadFailed(format!("map '{}': {e}", cfg.map)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_core::SimLimits;
    use cp_sim::SimView;

    /// Headless app (no window, no renderer) that loads a map the way the real app does.
    fn load(map: &str, limits: SimLimits) -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin {
                file_path: concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets").into(),
                ..default()
            },
        ));
        // Only the loading half of GamePlugin: the render plugins need a GPU.
        app.insert_resource(GameConfig {
            map: map.into(),
            limits,
            hour: 12.0,
            phase_timing: false,
            scene: None,
        })
        .add_plugins(MapPlugin);
        for _ in 0..2000 {
            app.update();
            if app.world().contains_resource::<Sim>()
                || app.world().contains_resource::<MapLoadFailed>()
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        app
    }

    #[test]
    fn both_maps_load_through_the_asset_server_into_the_sim() {
        for map in ["flatlands", "river_valley"] {
            let app = load(map, SimLimits::wasm());
            let sim = app.world().resource::<Sim>();
            assert_eq!(
                *sim.0.world().resource::<SimLimits>(),
                SimLimits::wasm(),
                "{map}"
            );
            let view = app.world().resource::<SimView>();
            assert_eq!(view.terrain.as_ref().unwrap().meta().name, map);
            assert_eq!(view.game_time.hour(), 12);
        }
    }

    #[test]
    fn missing_map_reports_failure() {
        let app = load("nope", SimLimits::desktop());
        assert!(app.world().contains_resource::<MapLoadFailed>());
        assert!(!app.world().contains_resource::<Sim>());
    }
}
