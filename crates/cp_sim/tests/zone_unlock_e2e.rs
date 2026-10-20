//! T-231: ResHigh, ComHigh and Office are painted only once the milestone that names them in
//! `milestones.ron` is reached; the refusal is the sim's, cells already painted stay, and the
//! unlock comes from content.

use std::sync::Arc;

use bevy_math::Vec3;
use cp_content::{ContentDb, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::{CommandError, CommandOutcome};
use cp_net::{Elevation, NetGraph};
use cp_sim::{Progress, SimCommand, SimRunner};
use cp_terrain::TileOwnership;
use cp_zone::{ZoneCellRef, ZoneCommand, ZoneGrid};

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

fn world(content: &Arc<ContentDb>) -> SimRunner {
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    let mut sim = SimRunner::new(1);
    sim.set_content(Arc::clone(content));
    sim.world_mut().insert_resource(TileOwnership::only((1, 1)));
    sim.world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            &def,
            road,
            CubicBezier3::line(
                Vec3::new(3000.0, 0.0, 3000.0),
                Vec3::new(3200.0, 0.0, 3000.0),
            ),
            Elevation::Ground,
        )
        .unwrap();
    sim.tick_once();
    sim
}

fn paint(sim: &mut SimRunner, cell: u16, zone: ZoneType) -> Result<(), CommandError> {
    let (block, _) = sim.world().resource::<ZoneGrid>().blocks().next().unwrap();
    sim.push_command(
        SimCommand::Zone(ZoneCommand::PaintZones {
            cells: vec![ZoneCellRef { block, cell }],
            zone,
        }),
        sim.tick(),
    );
    sim.apply_due_commands();
    let out: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<bevy_ecs::message::Messages<CommandOutcome>>()
        .drain()
        .collect();
    out.last().unwrap().result.clone().map(|_| ())
}

fn zone_at(sim: &SimRunner, cell: u16) -> ZoneType {
    let zones = sim.world().resource::<ZoneGrid>();
    let (block, _) = zones.blocks().next().unwrap();
    zones.zone_of(ZoneCellRef { block, cell }).unwrap()
}

#[test]
fn high_density_and_offices_wait_for_busy_town() {
    let c = content();
    let mut sim = world(&c);
    for reached in [0, 5] {
        sim.world_mut().insert_resource(Progress::at(reached));
        for (i, zone) in [ZoneType::ResHigh, ZoneType::ComHigh, ZoneType::Office]
            .into_iter()
            .enumerate()
        {
            let r = paint(&mut sim, i as u16 * 4 + 1, zone);
            if reached == 0 {
                assert!(
                    matches!(&r, Err(CommandError::Locked { milestone, population: 5000 })
                        if milestone == "Busy Town"),
                    "{zone:?}: {r:?}"
                );
                assert_eq!(zone_at(&sim, i as u16 * 4 + 1), ZoneType::None);
            } else {
                r.unwrap();
                assert_eq!(zone_at(&sim, i as u16 * 4 + 1), zone);
            }
        }
    }
}

#[test]
fn low_density_industry_and_erasing_are_never_locked_and_painted_cells_stay() {
    let c = content();
    let mut sim = world(&c);
    // Painted while everything is open (an old save, a code world) ...
    sim.world_mut().insert_resource(Progress::ALL);
    paint(&mut sim, 1, ZoneType::ResHigh).unwrap();
    // ... stays when the progress says it was ahead.
    sim.world_mut().insert_resource(Progress::START);
    sim.tick_once();
    assert_eq!(zone_at(&sim, 1), ZoneType::ResHigh);
    for (cell, zone) in [
        (5, ZoneType::ResLow),
        (9, ZoneType::ComLow),
        (13, ZoneType::Industrial),
    ] {
        paint(&mut sim, cell, zone).unwrap();
    }
    paint(&mut sim, 1, ZoneType::None).unwrap();
    assert_eq!(zone_at(&sim, 1), ZoneType::None);
}

#[test]
fn the_unlock_is_read_from_content() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let dir = std::env::temp_dir().join(format!("cp_zone_unlock_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(&src, &dir);
    let ms = std::fs::read_to_string(dir.join("milestones.ron")).unwrap();
    // Industry now waits for "Town" (T-283: index 4); the high-density types are no longer named
    // anywhere.
    let ms = ms
        .replace(", zones: [ResHigh, ComHigh, Office]", "")
        .replace("zones: [ResLow, ComLow, Industrial]", "zones: [ResLow, ComLow]")
        .replace(
            r#"name: "Town", unlocks"#,
            r#"name: "Town", zones: [Industrial], unlocks"#,
        );
    std::fs::write(dir.join("milestones.ron"), ms).unwrap();
    let c = Arc::new(ContentDb::load_dir(&dir).unwrap());
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(c.zone_unlock_milestone(ZoneType::Industrial), Some(4));
    assert_eq!(c.zone_unlock_milestone(ZoneType::ResHigh), None);

    let mut sim = world(&c);
    sim.world_mut().insert_resource(Progress::at(3));
    paint(&mut sim, 1, ZoneType::ResHigh).unwrap();
    assert!(matches!(
        paint(&mut sim, 5, ZoneType::Industrial),
        Err(CommandError::Locked { milestone, population: 3000 }) if milestone == "Town"
    ));
    sim.world_mut().insert_resource(Progress::at(4));
    paint(&mut sim, 5, ZoneType::Industrial).unwrap();
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let t = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &t);
        } else {
            std::fs::copy(e.path(), t).unwrap();
        }
    }
}

/// BUG-060: a locked zone type has no demand; once it opens the economy's value shows again.
#[test]
fn a_locked_zone_type_has_no_demand() {
    let c = content();
    let mut sim = world(&c);
    sim.world_mut().insert_resource(Progress::at(0));
    {
        let mut d = sim.world_mut().resource_mut::<cp_building::Demand>();
        d.o = 40.0;
        d.r = 40.0;
    }
    sim.tick_once();
    let d = *sim.world().resource::<cp_building::Demand>();
    assert_eq!(d.o, 0.0, "offices are locked");
    assert_eq!(d.r, 40.0, "low-density housing is open");
    sim.world_mut().insert_resource(Progress::at(u8::MAX - 1));
    sim.world_mut().resource_mut::<cp_building::Demand>().o = 40.0;
    sim.tick_once();
    assert_eq!(sim.world().resource::<cp_building::Demand>().o, 40.0);
}
