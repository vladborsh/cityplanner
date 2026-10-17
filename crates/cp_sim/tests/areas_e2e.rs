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
