use super::*;

#[test]
fn an_empty_state_is_empty_and_looks_up_nothing() {
    let s = ParkState::default();
    assert!(s.is_empty());
    assert!(s.get(cp_district::AreaId(1)).is_none());
}
