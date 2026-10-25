//! T-206: area commands through the sim: the view shows the areas, the checksum covers them,
//! and the same command log gives the same state.

use bevy_math::Vec2;
use cp_content::ResourceKind;
use cp_district::{AreaId, AreaKind, DistrictCommand, DistrictId, area_at};
use cp_sim::{SimCommand, SimRunner};

fn play() -> SimRunner {
    let mut sim = SimRunner::new(7);
    let cmds = [
        DistrictCommand::Create {
            name: "Town".into(),
        },
        DistrictCommand::Paint {
            cells: vec![(4, 4), (5, 4)],
            district: DistrictId(1),
        },
        DistrictCommand::CreateArea {
            kind: AreaKind::Industry(ResourceKind::Forestry),
            name: "Forest 1".into(),
        },
        DistrictCommand::PaintArea {
            cells: vec![(4, 4), (9, 9)],
            area: AreaId(1),
        },
    ];
    for c in cmds {
        sim.push_command(SimCommand::District(c), sim.tick());
        sim.tick_once();
    }
    for _ in 0..300 {
        sim.tick_once();
    }
    sim
}

#[test]
fn area_commands_reach_the_view_and_replay_identically() {
    let mut a = play();
    let mut b = play();
    let v = a.view();
    let a1 = v.areas.get(AreaId(1)).unwrap();
    assert_eq!(a1.kind, AreaKind::Industry(ResourceKind::Forestry));
    assert_eq!(
        area_at(&v.area_map, Vec2::new(4.0 * 32.0 + 1.0, 4.0 * 32.0 + 1.0)),
        Some(AreaId(1))
    );
    assert_eq!(
        area_at(&v.area_map, Vec2::new(5.0 * 32.0 + 1.0, 4.0 * 32.0 + 1.0)),
        None
    );
    // The district under the area is untouched.
    assert_eq!(v.district_map.get((4, 4)), Some(DistrictId(1)));
    assert_eq!(a.checksum(), b.checksum());

    // The areas are part of the checksum.
    let mut c = SimRunner::new(7);
    for _ in 0..305 {
        c.tick_once();
    }
    assert_ne!(a.checksum(), c.checksum());
}

/// BUG-062: the game starts paused, so area commands must show in the view without a tick.
#[test]
fn areas_painted_while_paused_show_in_the_view_at_once() {
    let mut sim = SimRunner::new(7);
    sim.world_mut().insert_resource(cp_core::SimSpeed::Pause);
    let _ = sim.view(); // a first snapshot, as the client takes one every frame
    let send = |sim: &mut SimRunner, c: DistrictCommand| {
        sim.push_command(SimCommand::District(c), sim.tick());
        sim.apply_due_commands();
    };
    send(
        &mut sim,
        DistrictCommand::CreateArea {
            kind: AreaKind::Industry(ResourceKind::Forestry),
            name: "Forestry area 1".into(),
        },
    );
    assert!(sim.view().areas.get(AreaId(1)).is_some(), "area listed");
    send(
        &mut sim,
        DistrictCommand::PaintArea {
            cells: vec![(4, 4)],
            area: AreaId(1),
        },
    );
    let v = sim.view();
    assert_eq!(v.area_map.get((4, 4)), Some(AreaId(1)), "cell painted");
    assert_eq!(v.area_map.cell_count(AreaId(1)), 1);
}
