use bevy_ecs::world::World;
use bevy_math::Vec2;
use cp_content::ZoneType;

use crate::*;

fn world() -> World {
    let mut w = World::new();
    w.init_resource::<DistrictMap>();
    w.init_resource::<Districts>();
    w
}

fn create(w: &mut World, name: &str) {
    apply(w, &DistrictCommand::Create { name: name.into() }).unwrap();
}

fn at(w: &World, x: f32, z: f32) -> Option<DistrictId> {
    district_at(w.resource::<DistrictMap>(), Vec2::new(x, z))
}

#[test]
fn paint_makes_district_at_true_inside_the_cell_and_false_outside() {
    let mut w = world();
    create(&mut w, "Old town");
    let d = DistrictId(1);
    apply(
        &mut w,
        &DistrictCommand::Paint {
            cells: vec![(10, 20)],
            district: d,
        },
    )
    .unwrap();
    // Cell (10, 20) spans x 320..352, z 640..672.
    assert_eq!(at(&w, 330.0, 650.0), Some(d));
    assert_eq!(at(&w, 351.9, 671.9), Some(d));
    assert_eq!(at(&w, 352.0, 650.0), None);
    assert_eq!(at(&w, 330.0, 639.0), None);
    assert_eq!(at(&w, -1.0, 5.0), None);
    assert_eq!(at(&w, 1e9, 5.0), None);
    apply(
        &mut w,
        &DistrictCommand::Erase {
            cells: vec![(10, 20)],
        },
    )
    .unwrap();
    assert_eq!(at(&w, 330.0, 650.0), None);
}

#[test]
fn delete_clears_the_cells_and_frees_the_slot() {
    let mut w = world();
    create(&mut w, "A");
    create(&mut w, "B");
    let (a, b) = (DistrictId(1), DistrictId(2));
    apply(
        &mut w,
        &DistrictCommand::Paint {
            cells: vec![(0, 0), (1, 0)],
            district: a,
        },
    )
    .unwrap();
    apply(
        &mut w,
        &DistrictCommand::Paint {
            cells: vec![(2, 0)],
            district: b,
        },
    )
    .unwrap();
    apply(&mut w, &DistrictCommand::Delete { district: a }).unwrap();
    assert_eq!(at(&w, 5.0, 5.0), None);
    assert_eq!(at(&w, 37.0, 5.0), None);
    assert_eq!(at(&w, 70.0, 5.0), Some(b));
    assert!(w.resource::<Districts>().get(a).is_none());
    // The freed slot is reused.
    create(&mut w, "C");
    assert_eq!(w.resource::<Districts>().get(a).unwrap().name, "C");
    assert_eq!(
        apply(
            &mut w,
            &DistrictCommand::Delete {
                district: DistrictId(9)
            }
        ),
        Err(DistrictError::NoSuchDistrict(DistrictId(9)))
    );
}

#[test]
fn tax_overrides_are_validated_and_removable() {
    let mut w = world();
    create(&mut w, "A");
    let d = DistrictId(1);
    let set = |w: &mut World, zone, pct| {
        apply(
            w,
            &DistrictCommand::SetTax {
                district: d,
                zone,
                pct,
            },
        )
    };
    for bad in [0, 30, 255] {
        assert_eq!(
            set(&mut w, ZoneType::ComLow, Some(bad)),
            Err(DistrictError::TaxOutOfRange(bad))
        );
    }
    assert_eq!(
        set(&mut w, ZoneType::None, Some(10)),
        Err(DistrictError::NoTaxForZone)
    );
    set(&mut w, ZoneType::ComLow, Some(20)).unwrap();
    assert_eq!(
        w.resource::<Districts>().tax_override(d, ZoneType::ComLow),
        Some(20)
    );
    assert_eq!(
        w.resource::<Districts>().tax_override(d, ZoneType::ResLow),
        None
    );
    set(&mut w, ZoneType::ComLow, None).unwrap();
    assert_eq!(
        w.resource::<Districts>().tax_override(d, ZoneType::ComLow),
        None
    );
}

#[test]
fn invalid_commands_change_nothing() {
    let mut w = world();
    create(&mut w, "A");
    let before = (
        w.resource::<DistrictMap>().clone(),
        w.resource::<Districts>().clone(),
    );
    for cmd in [
        DistrictCommand::Create { name: "  ".into() },
        DistrictCommand::Rename {
            district: DistrictId(1),
            name: "x".repeat(33),
        },
        DistrictCommand::Paint {
            cells: vec![(0, 0), (SIDE as u16, 0)],
            district: DistrictId(1),
        },
        DistrictCommand::Paint {
            cells: vec![(0, 0)],
            district: DistrictId(2),
        },
        DistrictCommand::Erase {
            cells: vec![(0, SIDE as u16)],
        },
    ] {
        assert!(apply(&mut world_with(&before), &cmd).is_err(), "{cmd:?}");
    }
    // The same commands on the real world are rejected and leave it as it was.
    for cmd in [
        DistrictCommand::Create { name: "".into() },
        DistrictCommand::Paint {
            cells: vec![(0, 0), (SIDE as u16, 0)],
            district: DistrictId(1),
        },
    ] {
        assert!(apply(&mut w, &cmd).is_err());
    }
    assert_eq!(
        before,
        (
            w.resource::<DistrictMap>().clone(),
            w.resource::<Districts>().clone()
        )
    );
}

fn world_with(s: &(DistrictMap, Districts)) -> World {
    let mut w = World::new();
    w.insert_resource(s.0.clone());
    w.insert_resource(s.1.clone());
    w
}
