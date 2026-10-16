use bevy_math::{Vec2, Vec3};
use cp_building::{BuildingFlags, BuildingGrid, Footprint, Lot};
use cp_content::BuildingDefId;
use cp_core::{Id, Tick};

use super::*;

fn building(state: BuildingState) -> Building {
    Building {
        def: BuildingDefId(0),
        footprint: Footprint::Lot(Lot {
            block: Id::new(0, 0),
            col: 0,
            w: 2,
            d: 2,
        }),
        level: 1,
        state,
        ready_at: Tick(0),
        pos: Vec2::ZERO,
        rot: 0.0,
        access: None,
        flags: BuildingFlags(0),
        level_progress: 0.0,
    }
}

fn bounds(_: BuildingId) -> Option<(Vec3, Vec3)> {
    Some((Vec3::new(90.0, 0.0, 90.0), Vec3::new(110.0, 12.0, 106.0)))
}

fn facing() -> Facing {
    Facing {
        position: Vec3::new(100.0, 60.0, 200.0),
        right: Vec3::X,
        up: Vec3::Y,
    }
}

fn world(state: BuildingState) -> (BuildingGrid, ServiceState, BuildingId) {
    let mut grid = BuildingGrid::new();
    let id = grid.insert(building(state));
    (grid, ServiceState::default(), id)
}

fn set_fire(st: &mut ServiceState, id: BuildingId, on: bool) {
    st.get_mut(id).problems.set(ServiceProblems::FIRE, on);
}

#[test]
fn fire_appearing_creates_an_effect_and_going_out_removes_it() {
    let (grid, mut st, id) = world(BuildingState::Active);
    assert!(burning(grid.iter(), &st, bounds).is_empty());
    set_fire(&mut st, id, true);
    let list = burning(grid.iter(), &st, bounds);
    assert_eq!(list.len(), 1);
    let mesh = build_mesh(&list, &facing(), 1.3);
    assert!(
        mesh.triangle_count() >= (2 * FLAMES) as usize,
        "flames and smoke"
    );
    assert!(
        mesh.colors.iter().any(|c| c[3] < 0.9),
        "smoke is translucent"
    );
    set_fire(&mut st, id, false);
    assert!(burning(grid.iter(), &st, bounds).is_empty());
    assert_eq!(build_mesh(&[], &facing(), 1.3).triangle_count(), 0);
}

#[test]
fn only_active_buildings_burn_on_screen() {
    for state in [
        BuildingState::Burned,
        BuildingState::UnderConstruction,
        BuildingState::Abandoned,
    ] {
        let (grid, mut st, id) = world(state);
        set_fire(&mut st, id, true);
        assert!(burning(grid.iter(), &st, bounds).is_empty(), "{state:?}");
    }
}

#[test]
fn flames_stand_on_the_roof_inside_the_footprint_and_move_with_time() {
    let (grid, mut st, id) = world(BuildingState::Active);
    set_fire(&mut st, id, true);
    let b = burning(grid.iter(), &st, bounds)[0];
    let (a, c) = (flames(&b, 0.0), flames(&b, 0.37));
    assert_eq!(a.len(), FLAMES as usize);
    for f in &a {
        assert_eq!(f.foot.y, b.max.y);
        assert!((b.min.x..=b.max.x).contains(&f.foot.x));
        assert!((b.min.z..=b.max.z).contains(&f.foot.z));
        assert!(f.height > 1.0 && f.height < 5.0);
    }
    assert_ne!(a, c, "the flames flicker");
    assert_eq!(flames(&b, 0.0), a, "a function of time and id only");
    let smoke = puffs(&b, 2.0);
    assert!(smoke.iter().all(|p| p.pos.y > b.max.y && p.alpha >= 0.0));
    assert!(
        smoke
            .iter()
            .map(|p| p.pos.y.to_bits())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            > 1
    );
}

#[test]
fn far_buildings_show_smoke_only_and_very_far_ones_nothing() {
    let (grid, mut st, id) = world(BuildingState::Active);
    set_fire(&mut st, id, true);
    let list = burning(grid.iter(), &st, bounds);
    let at = |d: f32| Facing {
        position: Vec3::new(100.0, 60.0, 98.0 + d),
        ..facing()
    };
    let near = build_mesh(&list, &at(100.0), 1.0).triangle_count();
    let mid = build_mesh(&list, &at(800.0), 1.0).triangle_count();
    let far = build_mesh(&list, &at(3000.0), 1.0).triangle_count();
    assert!(near > mid && mid > 0, "{near} {mid}");
    assert_eq!(far, 0);
}
