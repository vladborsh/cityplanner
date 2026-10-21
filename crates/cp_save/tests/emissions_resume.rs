//! T-300 (review): health reads the noise every tick, between the 256-tick passes that build
//! it. A game saved between two passes loads with the very noise the run that went on has, and
//! falls ill the same way: with the noise term made large, the two stay identical.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cp_citizen::CitizenGrid;
use cp_content::ContentDb;
use cp_core::SimLimits;
use cp_economy::Emissions;
use cp_save::{load_into_runner, save_runner};
use cp_sim::SimRunner;
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            files(&p, out);
        } else if p.extension().is_some_and(|x| x == "ron") {
            out.push(p);
        }
    }
}

/// The game's content with `sick_noise` 0.01 a day per point: noise dominates who falls ill.
fn loud_content() -> Arc<ContentDb> {
    let root = Path::new(ROOT).join("../../assets/content");
    let mut paths = Vec::new();
    files(&root, &mut paths);
    paths.sort();
    let sources = paths
        .into_iter()
        .map(|p| {
            let rel = p
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let mut text = std::fs::read_to_string(&p).unwrap();
            if rel == "balance.ron" {
                let at = text.find("sick_noise:").expect("sick_noise in balance.ron");
                let end = at + text[at..].find(',').unwrap();
                text.replace_range(at..end, "sick_noise: 0.01");
            }
            (rel, text)
        })
        .collect();
    let db = ContentDb::from_sources(sources).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(db.balance().sick_noise, 0.01);
    Arc::new(db)
}

fn runner(content: &Arc<ContentDb>, bytes: &[u8]) -> SimRunner {
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::with_limits(1, SimLimits::desktop());
    sim.set_terrain(terrain);
    sim.set_content(Arc::clone(content));
    load_into_runner(&mut sim, bytes, content.content_hash()).unwrap();
    sim
}

#[test]
fn a_game_saved_between_passes_keeps_the_noise_and_falls_ill_like_the_run_that_went_on() {
    let content = loud_content();
    let fixture = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/m1-small.sav")).unwrap();
    let mut a = runner(&content, &fixture);
    // Past a pass and into the middle of the next cycle.
    for _ in 0..(256 + 100) {
        a.tick_once();
    }
    assert_ne!(a.tick().0 % 256, 0, "between two passes");
    let loud = a
        .world()
        .resource::<Emissions>()
        .noise
        .iter()
        .filter(|n| **n > 0)
        .count();
    assert!(loud > 100, "the town makes noise: {loud} cells");
    let bytes = save_runner(&a, content.content_hash(), "flatlands").unwrap();
    let mut b = runner(&content, &bytes);
    assert!(
        a.world().resource::<Emissions>().noise == b.world().resource::<Emissions>().noise,
        "right after the load"
    );
    assert_eq!(a.checksum(), b.checksum(), "state at load");
    let sick = |s: &SimRunner| {
        s.world()
            .resource::<CitizenGrid>()
            .citizens()
            .filter(|(_, c)| c.sick_since.is_some())
            .count()
    };
    let before = sick(&a);
    // T-301: the day is 2048 ticks, so the per-day sickness takes more ticks to show, and a
    // clinic cures the sick as they fall ill: look at the most sick at any tick of the run.
    let mut peak = before;
    for _ in 0..512 {
        a.tick_once();
        b.tick_once();
        peak = peak.max(sick(&a));
    }
    assert!(peak > before, "the noise makes people ill");
    assert_eq!(sick(&a), sick(&b));
    assert_eq!(a.checksum(), b.checksum(), "load + 512 ticks");
}
