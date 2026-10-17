use bevy_ecs::world::World;
use bevy_math::Vec2;
use std::sync::Arc;

use cp_content::{ContentDb, PolicyKind, SimContent, SpecKind, ZoneType};

use crate::*;

fn world() -> World {
    let mut w = World::new();
    w.init_resource::<DistrictMap>();
    w.init_resource::<Districts>();
    w.init_resource::<CityPolicies>();
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
