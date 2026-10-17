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
