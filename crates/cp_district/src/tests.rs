use bevy_ecs::world::World;
use bevy_math::Vec2;
use std::sync::Arc;

use cp_content::{ContentDb, ParkKind, PolicyKind, ResourceKind, SimContent, SpecKind, ZoneType};

use crate::*;

fn world() -> World {
    let mut w = World::new();
    w.init_resource::<DistrictMap>();
    w.init_resource::<Districts>();
    w.init_resource::<CityPolicies>();
    w.init_resource::<AreaMap>();
    w.init_resource::<Areas>();
    w
}

fn policy_src(id: &str, kind: &str, scope: &str) -> (String, String) {
    (
        format!("p/{id}.ron"),
        format!(
            r#"Policy((id: "{id}", name: "{id}", kind: {kind}, icon: "i", scope: {scope},
            cost_per_building_week: 1.0, value: 0.3))"#
        ),
    )
}

/// A world with Recycling (both), PowerSaving (district only) and TourismPromotion (city only).
fn world_with_policies() -> World {
    let mut w = world();
    let db = ContentDb::from_sources(vec![
        policy_src("recycling", "Recycling", "Both"),
        policy_src("power_saving", "PowerSaving", "District"),
        policy_src("tourism", "TourismPromotion", "City"),
    ])
    .unwrap_or_else(|e| panic!("{e}"));
    w.insert_resource(SimContent(Arc::new(db)));
    w
}

fn set_policy(
    w: &mut World,
    district: Option<DistrictId>,
    policy: PolicyKind,
    on: bool,
) -> Result<cp_core::Cost, DistrictError> {
    apply(
        w,
        &DistrictCommand::SetPolicy {
            district,
            policy,
            on,
        },
    )
}

fn on_at(w: &World, x: f32, z: f32, kind: PolicyKind) -> bool {
    policy_at(
        w.resource::<DistrictMap>(),
        w.resource::<Districts>(),
        w.resource::<CityPolicies>(),
        Vec2::new(x, z),
        kind,
    )
}

#[test]
fn a_district_policy_holds_inside_the_district_and_not_outside() {
    let mut w = world_with_policies();
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
    assert!(!on_at(&w, 330.0, 650.0, PolicyKind::Recycling));
    set_policy(&mut w, Some(d), PolicyKind::Recycling, true).unwrap();
    assert!(on_at(&w, 330.0, 650.0, PolicyKind::Recycling));
    assert!(!on_at(&w, 400.0, 650.0, PolicyKind::Recycling));
    assert!(!on_at(&w, 330.0, 650.0, PolicyKind::PowerSaving));
    assert_eq!(w.resource::<Districts>().get(d).unwrap().policies, 1);
    set_policy(&mut w, Some(d), PolicyKind::Recycling, false).unwrap();
    assert!(!on_at(&w, 330.0, 650.0, PolicyKind::Recycling));
}

#[test]
fn a_city_policy_holds_everywhere() {
    let mut w = world_with_policies();
    set_policy(&mut w, None, PolicyKind::Recycling, true).unwrap();
    assert!(on_at(&w, 5000.0, 5000.0, PolicyKind::Recycling));
    assert!(on_at(&w, 1.0, 1.0, PolicyKind::Recycling));
    assert!(!on_at(&w, 1.0, 1.0, PolicyKind::PowerSaving));
    assert_eq!(
        w.resource::<CityPolicies>().mask,
        PolicyKind::Recycling.bit()
    );
}

#[test]
fn a_policy_of_the_wrong_scope_or_for_a_missing_district_is_an_error_and_changes_nothing() {
    let mut w = world_with_policies();
    create(&mut w, "Old town");
    let d = DistrictId(1);
    // City-only in a district, district-only for the city, undefined, missing district.
    assert_eq!(
        set_policy(&mut w, Some(d), PolicyKind::TourismPromotion, true),
        Err(DistrictError::PolicyScope(PolicyKind::TourismPromotion))
    );
    assert_eq!(
        set_policy(&mut w, None, PolicyKind::PowerSaving, true),
        Err(DistrictError::PolicyScope(PolicyKind::PowerSaving))
    );
    assert_eq!(
        set_policy(&mut w, None, PolicyKind::FreeTransit, true),
        Err(DistrictError::NoSuchPolicy(PolicyKind::FreeTransit))
    );
    assert_eq!(
        set_policy(&mut w, Some(DistrictId(9)), PolicyKind::Recycling, true),
        Err(DistrictError::NoSuchDistrict(DistrictId(9)))
    );
    assert_eq!(w.resource::<Districts>().get(d).unwrap().policies, 0);
    assert_eq!(w.resource::<CityPolicies>().mask, 0);
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

fn spec_src(kind: &str, zones: &str) -> (String, String) {
    (
        format!("sp/{kind}.ron"),
        format!(
            r#"Spec((kind: {kind}, name: "{kind}", icon: "i", zones: [{zones}], tax_mul: 1.0,
            tourist_weight: 1.0, noise_add: 0.0, land_value_add: 0.0, goods_mul: 1.0))"#
        ),
    )
}

#[test]
fn a_specialisation_is_set_for_the_zones_it_covers_and_refused_elsewhere() {
    let mut w = world();
    let db = ContentDb::from_sources(vec![spec_src("Tourism", "ComLow, ComHigh")])
        .unwrap_or_else(|e| panic!("{e}"));
    w.insert_resource(SimContent(Arc::new(db)));
    create(&mut w, "Old town");
    let d = DistrictId(1);
    let set = |w: &mut World, zone, spec| {
        apply(
            w,
            &DistrictCommand::SetSpec {
                district: d,
                zone,
                spec,
            },
        )
    };
    assert!(set(&mut w, ZoneType::ComLow, Some(SpecKind::Tourism)).is_ok());
    assert!(set(&mut w, ZoneType::ComHigh, Some(SpecKind::Tourism)).is_ok());
    assert_eq!(
        w.resource::<Districts>().get(d).unwrap().spec[2..4],
        [Some(SpecKind::Tourism); 2]
    );
    // Not for housing, not an undefined one, not for a missing district or for no zone.
    assert_eq!(
        set(&mut w, ZoneType::ResLow, Some(SpecKind::Tourism)),
        Err(DistrictError::SpecZone(SpecKind::Tourism, ZoneType::ResLow))
    );
    assert_eq!(
        set(&mut w, ZoneType::ComLow, Some(SpecKind::Organic)),
        Err(DistrictError::NoSuchSpec(SpecKind::Organic))
    );
    assert!(
        apply(
            &mut w,
            &DistrictCommand::SetSpec {
                district: DistrictId(7),
                zone: ZoneType::ComLow,
                spec: None
            }
        )
        .is_err()
    );
    assert!(set(&mut w, ZoneType::None, None).is_err());
    assert!(set(&mut w, ZoneType::ComLow, None).is_ok());
    assert_eq!(w.resource::<Districts>().get(d).unwrap().spec[2], None);
}

// ---- Areas (T-206) ----

fn run(w: &mut World, cmd: DistrictCommand) -> Result<cp_core::Cost, DistrictError> {
    apply(w, &cmd)
}

fn forestry(w: &mut World, name: &str) -> AreaId {
    run(
        w,
        DistrictCommand::CreateArea {
            kind: AreaKind::Industry(ResourceKind::Forestry),
            name: name.into(),
        },
    )
    .unwrap();
    w.resource::<Areas>()
        .iter()
        .find(|(_, a)| a.name == name.trim())
        .map(|(id, _)| id)
        .unwrap()
}

fn area_here(w: &World, x: f32, z: f32) -> Option<AreaId> {
    area_at(w.resource::<AreaMap>(), Vec2::new(x, z))
}

#[test]
fn a_painted_area_is_at_its_cell_and_not_outside() {
    let mut w = world();
    let a = forestry(&mut w, "Forest 1");
    assert_eq!(a, AreaId(1));
    let area = w.resource::<Areas>().get(a).unwrap().clone();
    assert_eq!(area.kind, AreaKind::Industry(ResourceKind::Forestry));
    assert_eq!((area.level, area.xp), (1, 0));
    run(
        &mut w,
        DistrictCommand::PaintArea {
            cells: vec![(3, 4), (4, 4)],
            area: a,
        },
    )
    .unwrap();
    assert_eq!(area_here(&w, 3.0 * 32.0 + 5.0, 4.0 * 32.0 + 5.0), Some(a));
    assert_eq!(area_here(&w, 4.0 * 32.0 + 5.0, 4.0 * 32.0 + 5.0), Some(a));
    assert_eq!(area_here(&w, 5.0 * 32.0 + 5.0, 4.0 * 32.0 + 5.0), None);
    assert_eq!(area_here(&w, -1.0, 5.0), None);
    assert_eq!(w.resource::<AreaMap>().cell_count(a), 2);
}

#[test]
fn painting_a_cell_with_another_area_takes_it_over_and_erase_clears_it() {
    let mut w = world();
    let a = forestry(&mut w, "A");
    run(
        &mut w,
        DistrictCommand::CreateArea {
            kind: AreaKind::Park(ParkKind::Zoo),
            name: "B".into(),
        },
    )
    .unwrap();
    let b = AreaId(2);
    let paint = |area, cells| DistrictCommand::PaintArea { cells, area };
    run(&mut w, paint(a, vec![(1, 1), (2, 1)])).unwrap();
    run(&mut w, paint(b, vec![(2, 1)])).unwrap();
    let m = w.resource::<AreaMap>();
    assert_eq!((m.get((1, 1)), m.get((2, 1))), (Some(a), Some(b)));
    run(
        &mut w,
        DistrictCommand::EraseArea {
            cells: vec![(1, 1)],
        },
    )
    .unwrap();
    assert_eq!(w.resource::<AreaMap>().get((1, 1)), None);
    assert_eq!(w.resource::<AreaMap>().get((2, 1)), Some(b));
}

#[test]
fn delete_clears_the_cells_and_frees_the_slot_and_rename_works() {
    let mut w = world();
    let a = forestry(&mut w, "A");
    let b = {
        run(
            &mut w,
            DistrictCommand::CreateArea {
                kind: AreaKind::Industry(ResourceKind::Ore),
                name: "B".into(),
            },
        )
        .unwrap();
        AreaId(2)
    };
    run(
        &mut w,
        DistrictCommand::PaintArea {
            cells: vec![(0, 0)],
            area: a,
        },
    )
    .unwrap();
    run(
        &mut w,
        DistrictCommand::RenameArea {
            area: a,
            name: "  Woods ".into(),
        },
    )
    .unwrap();
    assert_eq!(w.resource::<Areas>().get(a).unwrap().name, "Woods");
    run(&mut w, DistrictCommand::DeleteArea { area: a }).unwrap();
    assert_eq!(w.resource::<AreaMap>().get((0, 0)), None);
    assert!(w.resource::<Areas>().get(a).is_none());
    assert!(w.resource::<Areas>().get(b).is_some());
    // The lowest free slot is reused.
    assert_eq!(forestry(&mut w, "C"), AreaId(1));
    assert_eq!(w.resource::<Areas>().get(AreaId(1)).unwrap().name, "C");
}

#[test]
fn areas_and_districts_overlap_and_painting_one_leaves_the_other() {
    let mut w = world();
    run(
        &mut w,
        DistrictCommand::Create {
            name: "Old Town".into(),
        },
    )
    .unwrap();
    run(
        &mut w,
        DistrictCommand::Paint {
            cells: vec![(5, 5), (6, 5)],
            district: DistrictId(1),
        },
    )
    .unwrap();
    let districts_before = w.resource::<DistrictMap>().clone();
    let a = forestry(&mut w, "A");
    run(
        &mut w,
        DistrictCommand::PaintArea {
            cells: vec![(5, 5), (7, 5)],
            area: a,
        },
    )
    .unwrap();
    assert_eq!(*w.resource::<DistrictMap>(), districts_before);
    assert_eq!(w.resource::<DistrictMap>().get((5, 5)), Some(DistrictId(1)));
    assert_eq!(w.resource::<AreaMap>().get((5, 5)), Some(a));
    run(
        &mut w,
        DistrictCommand::Erase {
            cells: vec![(5, 5)],
        },
    )
    .unwrap();
    assert_eq!(w.resource::<AreaMap>().get((5, 5)), Some(a));
}

#[test]
fn invalid_area_commands_change_nothing() {
    let mut w = world();
    let a = forestry(&mut w, "A");
    let before = (
        w.resource::<AreaMap>().clone(),
        w.resource::<Areas>().clone(),
    );
    let bad = [
        DistrictCommand::CreateArea {
            kind: AreaKind::Park(ParkKind::Nature),
            name: "  ".into(),
        },
        DistrictCommand::RenameArea {
            area: AreaId(9),
            name: "x".into(),
        },
        DistrictCommand::DeleteArea { area: AreaId(9) },
        DistrictCommand::PaintArea {
            cells: vec![(0, 0)],
            area: AreaId(9),
        },
        DistrictCommand::PaintArea {
            cells: vec![(0, 0), (SIDE as u16, 0)],
            area: a,
        },
        DistrictCommand::EraseArea {
            cells: vec![(0, SIDE as u16)],
        },
    ];
    for cmd in bad {
        assert!(run(&mut w, cmd.clone()).is_err(), "{cmd:?}");
    }
    assert_eq!(
        before,
        (
            w.resource::<AreaMap>().clone(),
            w.resource::<Areas>().clone()
        )
    );
}

#[test]
fn the_same_commands_give_the_same_areas() {
    let play = || {
        let mut w = world();
        let a = forestry(&mut w, "A");
        run(
            &mut w,
            DistrictCommand::PaintArea {
                cells: (0..50).map(|i| (i, i)).collect(),
                area: a,
            },
        )
        .unwrap();
        run(
            &mut w,
            DistrictCommand::EraseArea {
                cells: vec![(3, 3)],
            },
        )
        .unwrap();
        (
            w.resource::<AreaMap>().clone(),
            w.resource::<Areas>().clone(),
        )
    };
    assert_eq!(play(), play());
}
