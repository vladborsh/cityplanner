//! BUG-030: a grown city uses many styles and neighbours rarely share one. Headless: builds
//! `grid_city` 10k, reads the `BuildingGrid` and asks `variant()` for each building's style.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use cp_content::ContentDb;
use cp_render::building::variant_for;
use cp_sim::scenario::{GridCityConfig, grid_city};
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn a_ten_thousand_city_mixes_its_styles() {
    let root = Path::new(ROOT).join("../..");
    let terrain = Arc::new(Terrain::load(&root.join("assets/maps/flatlands")).unwrap());
    let content = Arc::new(ContentDb::load_dir(root.join("assets/content")).unwrap());
    let (sim, report) = grid_city(
        10_000,
        1,
        terrain,
        Arc::clone(&content),
        &GridCityConfig::default(),
    );
    assert!(report.reached, "{report:?}");
    let grid = sim.view().buildings.clone();

    let mut per_level: BTreeMap<_, (usize, BTreeSet<String>)> = BTreeMap::new();
    // (block, first column, end column, style) of every lot building.
    let mut lots = Vec::new();
    for (id, b) in grid.iter() {
        let def = content.building(b.def);
        if def.service.is_some() {
            continue;
        }
        let style = variant_for(&content, id, b, def).0.id.clone();
        let e = per_level.entry((def.zone, def.level)).or_default();
        e.0 += 1;
        e.1.insert(style.clone());
        if let Some(l) = b.lot() {
            lots.push((l.block, l.col, l.col + u16::from(l.w), style));
        }
    }
    for (k, (n, styles)) in &per_level {
        if *n >= 10 {
            assert!(styles.len() >= 4, "{k:?}: {n} buildings, styles {styles:?}");
        }
    }
    lots.sort();
    let (mut pairs, mut same) = (0usize, 0usize);
    for w in lots.windows(2) {
        if w[0].0 == w[1].0 && w[0].2 == w[1].1 {
            pairs += 1;
            same += usize::from(w[0].3 == w[1].3);
        }
    }
    assert!(pairs > 100, "only {pairs} neighbour pairs");
    let share = same as f32 / pairs as f32;
    assert!(
        share <= 0.15,
        "{same} of {pairs} neighbour pairs share a style ({share:.2})"
    );
}
