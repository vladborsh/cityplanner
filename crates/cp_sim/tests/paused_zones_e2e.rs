//! BUG-038: a road built while the sim is paused (commands applied, no tick) shows its zone cells
//! at once. The zone sync is part of the tick, so the view draws a synced clone until the next tick;
//! the world's own state (and its checksum) is untouched by that.

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec2;
use cp_content::ContentDb;
use cp_net::{ElevationMode, NetCommand, RoadPath};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn sim() -> SimRunner {
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let mut sim = SimRunner::new(1);
    sim.set_terrain(Arc::new(
        Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap(),
    ));
    sim.set_content(Arc::clone(&content));
    sim.push_command(
        SimCommand::Net(NetCommand::BuildRoad {
            road: content.road_id("basic_2l").unwrap(),
            path: RoadPath::Straight {
                from: Vec2::new(4600.0, 4700.0),
                to: Vec2::new(4800.0, 4700.0),
            },
            elevation: ElevationMode::Ground,
        }),
        sim.tick(),
    );
    sim
}

#[test]
fn a_road_built_while_paused_has_zone_cells_before_any_tick() {
    let mut paused = sim();
    paused.apply_due_commands();
    let view = paused.view();
    let cells = view.overlays.zones.cells.len();
    assert!(cells > 0, "no zone cells while paused");
    assert!(
        view.zones
            .cells_within(Vec2::new(4700.0, 4700.0), 300.0)
            .len()
            >= cells / 2
    );
    // The sim's own zone grid is not touched: the checksum equals one that was never viewed.
    let mut quiet = sim();
    quiet.apply_due_commands();
    assert_eq!(paused.checksum(), quiet.checksum());
    // And the tick gives the same cells the preview showed.
    paused.tick_once();
    assert_eq!(paused.view().overlays.zones.cells.len(), cells);
    quiet.tick_once();
    assert_eq!(paused.checksum(), quiet.checksum());
}

/// BUG-049: painting right after building a road, still paused, takes effect at once. The cells
/// the player clicks come from the view's synced clone; the paint must find them in the world.
#[test]
fn zones_painted_while_paused_take_effect_without_a_tick() {
    let mut paused = sim();
    paused.apply_due_commands();
    let cells: Vec<_> = paused
        .view()
        .zones
        .cells_within(Vec2::new(4700.0, 4700.0), 60.0)
        .into_iter()
        .filter(|c| paused.view().zones.is_valid(*c))
        .collect();
    assert!(cells.len() > 4, "{}", cells.len());
    paused.push_command(
        SimCommand::Zone(cp_zone::ZoneCommand::PaintZones {
            cells: cells.clone(),
            zone: cp_content::ZoneType::ResLow,
        }),
        paused.tick(),
    );
    paused.apply_due_commands();
    let view = paused.view();
    let zoned = cells
        .iter()
        .filter(|c| view.zones.zone_of(**c) == Some(cp_content::ZoneType::ResLow))
        .count();
    assert_eq!(zoned, cells.len(), "cells zoned in the view, no tick run");
    // The tick keeps what was painted.
    paused.tick_once();
    let view = paused.view();
    assert!(
        cells
            .iter()
            .all(|c| view.zones.zone_of(*c) == Some(cp_content::ZoneType::ResLow))
    );
}
