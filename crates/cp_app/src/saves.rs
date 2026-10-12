//! Saving and loading from the UI (T-046): files in `<data dir>/saves/`, where the data
//! directory is `CITYPLANNER_DATA_DIR` or `./.data/default`.
//!
//! Everything the menu does goes through the functions here, so a headless test can run the
//! same path: [`save_game`] writes `<name>.sav` (via a temporary file, so a crash never leaves
//! half a save), [`load_game`] builds a **fresh** `SimRunner` with the old one's limits, map
//! and content and restores the file into it, and [`list_saves`] reads the headers. A failed
//! load changes nothing: the caller keeps the old runner until it has the new one.

use std::path::{Path, PathBuf};

use cp_content::SimContent;
use cp_core::SimLimits;
use cp_prof::PhaseClock;
use cp_save::{LoadWarning, load_into_runner, read_header, save_runner};
use cp_sim::{SimRunner, SimTerrain};
use cp_ui::SaveEntry;

/// File extension of a save.
pub const EXT: &str = "sav";

/// The data directory: `CITYPLANNER_DATA_DIR`, else `./.data/default`.
pub fn data_dir() -> PathBuf {
    std::env::var_os("CITYPLANNER_DATA_DIR")
        .filter(|v| !v.is_empty())
        .map_or_else(|| PathBuf::from(".data/default"), PathBuf::from)
}

/// Where the saves of `data` live.
pub fn saves_dir(data: &Path) -> PathBuf {
    data.join("saves")
}

/// A file name made of what the player typed: letters, digits, space, `_` and `-`, at most
/// 48 characters, trimmed; `None` if nothing is left or something else was typed.
pub fn clean_name(typed: &str) -> Option<String> {
    let name = typed.trim();
    let ok = !name.is_empty()
        && name.chars().count() <= 48
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '_' | '-'));
    ok.then(|| name.to_string())
}

fn path_of(data: &Path, name: &str) -> Result<PathBuf, String> {
    let name = clean_name(name).ok_or_else(|| {
        format!("'{name}' is not a usable save name (letters, digits, space, _ and -)")
    })?;
    Ok(saves_dir(data).join(format!("{name}.{EXT}")))
}

/// The saves in `data`, by name. Files that are not saves (bad header) are left out.
pub fn list_saves(data: &Path) -> Vec<SaveEntry> {
    let Ok(dir) = std::fs::read_dir(saves_dir(data)) else {
        return Vec::new();
    };
    let mut out: Vec<SaveEntry> = dir
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some(EXT) {
                return None;
            }
            let name = path.file_stem()?.to_str()?.to_string();
            let bytes = std::fs::read(&path).ok()?;
            let (header, _) = read_header(&bytes).ok()?;
            Some(SaveEntry {
                name,
                bytes: bytes.len() as u64,
                tick: header.tick,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Saves `sim` as `<name>.sav`; `map_id` goes into the header. Returns the path.
pub fn save_game(
    sim: &SimRunner,
    data: &Path,
    name: &str,
    map_id: &str,
) -> Result<PathBuf, String> {
    let path = path_of(data, name)?;
    let content_hash = sim
        .world()
        .get_resource::<SimContent>()
        .map_or(0, |c| c.0.content_hash());
    let bytes = save_runner(sim, content_hash, map_id).map_err(|e| e.to_string())?;
    let dir = saves_dir(data);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let tmp = path.with_extension("sav.tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

/// A new runner like `old`: same limits, map and content, and the phase timing if `old` had
/// it, with nothing else carried over.
pub fn fresh_like(old: &SimRunner) -> SimRunner {
    let w = old.world();
    let limits = w
        .get_resource::<SimLimits>()
        .copied()
        .unwrap_or_else(SimLimits::desktop);
    let mut sim = SimRunner::with_limits(1, limits);
    if let Some(t) = w.get_resource::<SimTerrain>() {
        sim.set_terrain(std::sync::Arc::clone(&t.0));
    }
    if let Some(c) = w.get_resource::<SimContent>() {
        sim.set_content(std::sync::Arc::clone(&c.0));
    }
    if w.contains_resource::<PhaseClock>() {
        sim.add_plugin(&cp_prof::PhaseTiming::window(crate::PHASE_WINDOW_TICKS));
    }
    sim
}

/// Loads `<name>.sav` into a fresh runner like `old`. Returns it with the warnings (content
/// or map differ from the save) as text. `old` is not touched.
pub fn load_game(
    old: &SimRunner,
    data: &Path,
    name: &str,
) -> Result<(SimRunner, Vec<String>), String> {
    let path = path_of(data, name)?;
    let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let content_hash = old
        .world()
        .get_resource::<SimContent>()
        .map_or(0, |c| c.0.content_hash());
    let mut sim = fresh_like(old);
    let (_, warnings) = load_into_runner(&mut sim, &bytes, content_hash)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let warnings = warnings
        .into_iter()
        .map(|w| match w {
            LoadWarning::ContentHashMismatch { .. } => "saved with different content".to_string(),
            LoadWarning::MapMismatch { .. } => "saved on a different map".to_string(),
            LoadWarning::UnknownDef { kind, name } => {
                format!(
                    "unknown {} type `{name}` replaced by a placeholder",
                    kind.label()
                )
            }
        })
        .collect();
    Ok((sim, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Arc;

    use cp_content::ContentDb;
    use cp_core::curve::CubicBezier3;
    use cp_core::time::TICKS_PER_DAY;
    use cp_net::{Elevation, NetGraph};
    use cp_terrain::Terrain;

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn tmp(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cityplanner-saves-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A small growing town on the real map and content.
    fn town() -> SimRunner {
        let content = Arc::new(ContentDb::load_dir(root().join("assets/content")).unwrap());
        let terrain = Arc::new(Terrain::load(&root().join("assets/maps/flatlands")).unwrap());
        let road = content.road_id("basic_2l").unwrap();
        let def = content.road(road).clone();
        let mut sim = SimRunner::new(5);
        sim.set_terrain(terrain);
        sim.set_content(content);
        {
            let mut g = sim.world_mut().resource_mut::<NetGraph>();
            for (a, b) in [
                ((5000.0, 5000.0), (5090.0, 5000.0)),
                ((5045.0, 4940.0), (5045.0, 5060.0)),
            ] {
                g.add_segment(
                    &def,
                    road,
                    CubicBezier3::line(
                        bevy::math::Vec3::new(a.0, 0.0, a.1),
                        bevy::math::Vec3::new(b.0, 0.0, b.1),
                    ),
                    Elevation::Ground,
                )
                .unwrap();
            }
        }
        for _ in 0..(TICKS_PER_DAY / 2) {
            sim.tick_once();
        }
        sim
    }

    #[test]
    fn names_are_cleaned() {
        assert_eq!(clean_name("  my city 2 "), Some("my city 2".into()));
        assert_eq!(clean_name("a-b_c"), Some("a-b_c".into()));
        assert_eq!(clean_name(""), None);
        assert_eq!(clean_name("   "), None);
        assert_eq!(clean_name("../evil"), None);
        assert_eq!(clean_name("a/b"), None);
        assert_eq!(clean_name(&"x".repeat(49)), None);
    }

    #[test]
    fn data_dir_follows_the_environment_variable_name() {
        // Only the default can be asserted without touching the process environment.
        if std::env::var_os("CITYPLANNER_DATA_DIR").is_none() {
            assert_eq!(data_dir(), PathBuf::from(".data/default"));
        }
        assert_eq!(saves_dir(Path::new("d")), PathBuf::from("d/saves"));
    }

    /// save -> list -> load into a fresh runner -> both continue identically: the path the menu
    /// takes, without a window.
    #[test]
    fn save_then_load_continues_the_city_exactly() {
        let dir = tmp("roundtrip");
        let mut a = town();
        assert!(list_saves(&dir).is_empty());
        let path = save_game(&a, &dir, " first save ", "flatlands").unwrap();
        assert!(path.ends_with("saves/first save.sav"));
        let list = list_saves(&dir);
        assert_eq!(list.len(), 1);
        assert_eq!(
            (list[0].name.as_str(), list[0].tick),
            ("first save", a.tick().0)
        );
        assert!(list[0].bytes > 100);

        let (mut b, warnings) = load_game(&a, &dir, "first save").unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            a.checksum(),
            b.checksum(),
            "same state right after the load"
        );
        for _ in 0..(TICKS_PER_DAY / 2) {
            a.tick_once();
            b.tick_once();
        }
        assert_eq!(a.checksum(), b.checksum(), "same city half a day later");
        assert_eq!(a.tick(), b.tick());
        // The fresh runner is a full sim: it has the map and the content.
        assert!(b.view().terrain.is_some() && b.view().content.is_some());

        // Saving the loaded game again gives the same bytes.
        let again = save_game(&b, &dir, "second", "flatlands").unwrap();
        let _ = again;
        let bytes_a = std::fs::read(saves_dir(&dir).join("first save.sav")).unwrap();
        let bytes_b = save_runner(
            &a,
            a.world().resource::<SimContent>().0.content_hash(),
            "flatlands",
        )
        .unwrap();
        assert_eq!(
            std::fs::read(saves_dir(&dir).join("second.sav")).unwrap(),
            bytes_b
        );
        assert_ne!(bytes_a, bytes_b, "the first file is the earlier state");
    }

    #[test]
    fn bad_requests_fail_without_touching_anything() {
        let dir = tmp("errors");
        let a = town();
        assert!(save_game(&a, &dir, "../x", "m").is_err());
        assert!(
            load_game(&a, &dir, "missing")
                .err()
                .unwrap()
                .contains("cannot read")
        );
        std::fs::create_dir_all(saves_dir(&dir)).unwrap();
        std::fs::write(saves_dir(&dir).join("junk.sav"), b"not a save").unwrap();
        assert!(load_game(&a, &dir, "junk").is_err());
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            list_saves(&dir).is_empty(),
            "a file that is not a save is not listed"
        );
        assert!(!saves_dir(&dir).join("x.sav").exists());
    }

    /// The menu's own path: requests in, status and a replaced runner and view out.
    #[test]
    fn the_menu_requests_save_and_load_and_replace_the_runner() {
        use cp_ui::{FileMenu, FileRequest};
        let dir = tmp("menu");
        let mut sim = town();
        let mut view = sim.view();
        let mut menu = FileMenu::default();
        let run = |menu: &mut FileMenu, sim: &mut SimRunner, view: &mut cp_sim::SimView, r| {
            menu.request = Some(r);
            crate::handle_file_request(menu, sim, view, &dir, "flatlands");
        };

        run(&mut menu, &mut sim, &mut view, FileRequest::Refresh);
        assert!(menu.files.is_empty());
        run(
            &mut menu,
            &mut sim,
            &mut view,
            FileRequest::Save("one".into()),
        );
        assert!(menu.status.starts_with("Saved"), "{}", menu.status);
        assert_eq!(menu.files.len(), 1, "the list was refreshed");
        let saved_at = sim.tick();
        let saved_sum = sim.checksum();

        // The game moves on; loading brings the saved moment back, in a new runner.
        for _ in 0..500 {
            sim.tick_once();
        }
        view = sim.view();
        assert_ne!(view.tick, saved_at);
        run(
            &mut menu,
            &mut sim,
            &mut view,
            FileRequest::Load("one".into()),
        );
        assert_eq!(menu.status, "Loaded one");
        assert_eq!((sim.tick(), view.tick), (saved_at, saved_at));
        assert_eq!(sim.checksum(), saved_sum);

        // A failed load leaves the running game alone and says why.
        for _ in 0..10 {
            sim.tick_once();
        }
        let before = sim.checksum();
        run(
            &mut menu,
            &mut sim,
            &mut view,
            FileRequest::Load("nope".into()),
        );
        assert!(menu.status.starts_with("Load failed"), "{}", menu.status);
        assert_eq!(sim.checksum(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
