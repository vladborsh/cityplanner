//! `--gallery`: lays out every model under `assets/models` in a grid, one row per category,
//! with a camera and a light. A self-contained viewer for checking imported CC0 assets.
//!
//! `cargo run -p cp_desktop -- --gallery [--screenshot out.png]`
//!
//! Model convention (see `assets/LICENSES.md`): metres, +Y up, +Z front, origin at the
//! ground centre of the bounding box.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

/// The repo's `assets/` directory.
pub const ASSETS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");

/// Rows in display order, with the spacing between models in that row.
const CATEGORIES: [(&str, f32); 4] = [
    ("vehicles", 6.0),
    ("people", 3.0),
    ("trees", 8.0),
    ("props", 4.0),
];
const ROW_SPACING: f32 = 14.0;

/// One model file: its category and path relative to `assets/` (e.g. `models/trees/x.glb`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub category: &'static str,
    pub asset_path: String,
}

/// All `.glb` files under `<assets>/models/<category>/`, sorted by category order then name.
pub fn discover(assets: &Path) -> Vec<Entry> {
    let mut out = Vec::new();
    for (category, _) in CATEGORIES {
        let dir = assets.join("models").join(category);
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut names: Vec<String> = rd
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".glb"))
            .collect();
        names.sort();
        out.extend(names.into_iter().map(|n| Entry {
            category,
            asset_path: format!("models/{category}/{n}"),
        }));
    }
    out
}

/// Grid position of each entry (same order): rows by category along -Z, models along +X,
/// each row centred on x = 0.
pub fn layout(entries: &[Entry]) -> Vec<Vec3> {
    let mut out = Vec::with_capacity(entries.len());
    for (row, (category, spacing)) in CATEGORIES.iter().enumerate() {
        let n = entries.iter().filter(|e| e.category == *category).count();
        for i in 0..n {
            let x = (i as f32 - (n as f32 - 1.0) / 2.0) * spacing;
            out.push(Vec3::new(x, 0.0, -(row as f32) * ROW_SPACING));
        }
    }
    out
}

#[derive(Resource)]
struct ScreenshotJob {
    path: PathBuf,
    frames: u32,
}

pub fn run() {
    let args: Vec<String> = std::env::args().collect();
    let screenshot = args
        .iter()
        .position(|a| a == "--screenshot")
        .and_then(|i| args.get(i + 1))
        .map(|p| ScreenshotJob {
            path: PathBuf::from(p),
            frames: 0,
        });

    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(AssetPlugin {
        file_path: ASSETS_DIR.into(),
        ..default()
    }))
    .add_systems(Startup, setup);
    if let Some(job) = screenshot {
        app.insert_resource(job)
            .add_systems(Update, take_screenshot);
    }
    app.run();
}

fn setup(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let entries = discover(Path::new(ASSETS_DIR));
    let positions = layout(&entries);
    for (entry, pos) in entries.iter().zip(&positions) {
        let scene = assets.load(GltfAssetLabel::Scene(0).from_asset(entry.asset_path.clone()));
        commands.spawn((WorldAssetRoot(scene), Transform::from_translation(*pos)));
    }

    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(140.0, 90.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.35, 0.45, 0.3))),
        Transform::from_xyz(0.0, -0.01, -20.0),
    ));
    commands.spawn((
        DirectionalLight {
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(30.0, 60.0, 30.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 46.0, 40.0).looking_at(Vec3::new(0.0, 0.0, -18.0), Vec3::Y),
    ));
}

/// Waits for assets to load, saves one screenshot, then exits.
fn take_screenshot(
    mut commands: Commands,
    mut job: ResMut<ScreenshotJob>,
    mut exit: MessageWriter<AppExit>,
) {
    job.frames += 1;
    if job.frames == 180 {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(job.path.clone()));
    } else if job.frames == 240 {
        exit.write(AppExit::Success);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assets() -> PathBuf {
        PathBuf::from(ASSETS_DIR)
    }

    /// Triangle count and ground-contact check for one glb, via the same `gltf` parser Bevy uses.
    fn load(path: &Path) -> (usize, f32) {
        let (doc, buffers, _) = gltf::import(path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
        let mut tris = 0;
        let mut min_y = f32::MAX;
        for mesh in doc.meshes() {
            for prim in mesh.primitives() {
                let reader = prim.reader(|b| Some(&buffers[b.index()]));
                let idx = reader.read_indices().map_or_else(
                    || reader.read_positions().map_or(0, |p| p.len()),
                    |i| i.into_u32().count(),
                );
                tris += idx / 3;
                for p in reader.read_positions().expect("positions") {
                    min_y = min_y.min(p[1]);
                }
            }
        }
        (tris, min_y)
    }

    fn budget(category: &str) -> usize {
        match category {
            "vehicles" => 1500,
            "people" => 800,
            _ => 1500,
        }
    }

    #[test]
    fn counts_meet_targets() {
        let entries = discover(&assets());
        let count = |c| entries.iter().filter(|e| e.category == c).count();
        assert!(count("vehicles") >= 8, "vehicles: {}", count("vehicles"));
        assert!(count("people") >= 6, "people: {}", count("people"));
        assert!(count("trees") >= 5, "trees: {}", count("trees"));
    }

    #[test]
    fn every_model_loads_within_budget_and_sits_on_the_ground() {
        for e in discover(&assets()) {
            let (tris, min_y) = load(&assets().join(&e.asset_path));
            assert!(tris > 0, "{} is empty", e.asset_path);
            assert!(
                tris <= budget(e.category),
                "{}: {tris} tris over budget",
                e.asset_path
            );
            assert!(
                min_y.abs() < 1e-3,
                "{}: pivot not on ground ({min_y})",
                e.asset_path
            );
        }
    }

    #[test]
    fn no_stray_files_under_models() {
        // discover() only sees known categories; make sure nothing else is lying around.
        let mut found = 0;
        for (cat, _) in CATEGORIES {
            found += std::fs::read_dir(assets().join("models").join(cat))
                .unwrap()
                .count();
        }
        assert_eq!(
            found,
            discover(&assets()).len(),
            "non-.glb file in assets/models"
        );
        let top = std::fs::read_dir(assets().join("models")).unwrap().count();
        assert_eq!(top, CATEGORIES.len(), "unknown directory in assets/models");
    }

    #[test]
    fn licenses_cover_every_model() {
        let text = std::fs::read_to_string(assets().join("LICENSES.md")).unwrap();
        for e in discover(&assets()) {
            let needle = format!("assets/{}", e.asset_path);
            assert!(text.contains(&needle), "{needle} missing from LICENSES.md");
        }
    }

    #[test]
    fn layout_is_a_centred_grid_without_overlap() {
        let entries = discover(&assets());
        let pos = layout(&entries);
        assert_eq!(pos.len(), entries.len());
        for (i, a) in pos.iter().enumerate() {
            for b in &pos[i + 1..] {
                assert!(a.distance(*b) >= 3.0, "{a} overlaps {b}");
            }
        }
        let veh: Vec<_> = entries
            .iter()
            .zip(&pos)
            .filter(|(e, _)| e.category == "vehicles")
            .map(|(_, p)| p.x)
            .collect();
        assert!((veh.iter().sum::<f32>()).abs() < 1e-3, "row not centred");
    }
}
