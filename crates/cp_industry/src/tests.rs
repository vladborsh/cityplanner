use super::*;

#[test]
fn levels_follow_the_xp_thresholds_and_storage_grows_per_level() {
    let t = [100, 400, 1000, 2000];
    assert_eq!(level_for_xp(0, &t), 1);
    assert_eq!(level_for_xp(99, &t), 1);
    assert_eq!(level_for_xp(100, &t), 2);
    assert_eq!(level_for_xp(1999, &t), 4);
    assert_eq!(level_for_xp(u32::MAX, &t), MAX_AREA_LEVEL);
    assert_eq!(storage_cap(200.0, 0.1, 1), 200.0);
    assert!((storage_cap(200.0, 0.1, 3) - 240.0).abs() < 1e-4);
}

#[test]
fn stock_is_kept_sorted_without_zeros() {
    let mut s = IndustryState::default();
    let b = BuildingId::new(1, 0);
    s.add(b, CargoKind::Planks, 5.0);
    s.add(b, CargoKind::Logs, 7.0);
    assert_eq!(
        s.stock[&b],
        vec![(CargoKind::Logs, 7.0), (CargoKind::Planks, 5.0)]
    );
    s.add(b, CargoKind::Logs, -7.0);
    assert_eq!(s.stock[&b], vec![(CargoKind::Planks, 5.0)]);
    s.add(b, CargoKind::Planks, -9.0);
    assert!(s.is_empty());
}

#[test]
fn warehouse_modes_take_in_and_give_out_as_designed() {
    use crate::warehouse::{WarehouseMode::*, WarehouseSetting};
    let s = |mode, cargo| WarehouseSetting { cargo, mode };
    let (cap, low, high) = (1000.0, 100.0, 900.0);
    let any = CargoKind::Logs;
    assert!(s(Fill, None).accepts(any, high, cap) && !s(Fill, None).gives(any, high, cap));
    assert!(!s(Empty, None).accepts(any, low, cap) && s(Empty, None).gives(any, low, cap));
    assert!(s(Balanced, None).accepts(any, low, cap) && !s(Balanced, None).accepts(any, high, cap));
    assert!(s(Balanced, None).gives(any, high, cap) && !s(Balanced, None).gives(any, low, cap));
    // T-320b: in a shortage a Fill warehouse gives like a Balanced one; the others stay.
    assert!(s(Fill, None).in_market(true).gives(any, high, cap));
    assert!(!s(Fill, None).in_market(true).gives(any, low, cap));
    assert!(!s(Fill, None).in_market(false).gives(any, high, cap));
    assert_eq!(s(Empty, None).in_market(true), s(Empty, None));
    // A chosen cargo is the only one it handles.
    let only = s(Fill, Some(CargoKind::Planks));
    assert!(only.accepts(CargoKind::Planks, 0.0, cap) && !only.accepts(any, 0.0, cap));
}
