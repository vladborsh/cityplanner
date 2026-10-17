//! Leisure targets (T-224, `architecture/system-design.md` §16 M6 (D)): where a resident
//! without a shift may spend a free hour. `cp_park` refills the list once a game hour (park
//! gates, city parks, unique buildings); the routine picks from it.

use bevy_ecs::resource::Resource;
use bevy_math::Vec2;
use cp_building::BuildingId;
use serde::{Deserialize, Serialize};

/// RNG stream of the leisure choices.
pub const LEISURE_STREAM: u64 = 0x1E15_0001;

/// One place to go.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LeisureTarget {
    pub building: BuildingId,
    pub pos: Vec2,
    /// How much it draws; `0` = nobody goes (no gate, full).
    pub weight: f32,
}

/// The targets, sorted by `BuildingId`. Saved: the routine reads the list as of the last hour,
/// so a loaded game must hold the same list as the continuous run.
#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LeisureTargets {
    pub list: Vec<LeisureTarget>,
}

impl LeisureTargets {
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

/// One target for a resident at `from`: weight `weight / (1 + d / dist_m)`; `pick` is the
/// uniform `0..1` draw. `None` when nothing draws.
pub fn pick_leisure(
    targets: &LeisureTargets,
    from: Vec2,
    dist_m: f32,
    pick: f32,
) -> Option<BuildingId> {
    let w = |t: &LeisureTarget| {
        if t.weight > 0.0 {
            t.weight / (1.0 + t.pos.distance(from) / dist_m.max(1.0))
        } else {
            0.0
        }
    };
    let total: f32 = targets.list.iter().map(w).sum();
    if total <= 0.0 {
        return None;
    }
    let mut at = pick * total;
    let mut last = None;
    for t in &targets.list {
        let x = w(t);
        if x <= 0.0 {
            continue;
        }
        last = Some(t.building);
        if at < x {
            return last;
        }
        at -= x;
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(i: u32, x: f32, weight: f32) -> LeisureTarget {
        LeisureTarget {
            building: BuildingId::new(i, 0),
            pos: Vec2::new(x, 0.0),
            weight,
        }
    }

    #[test]
    fn nothing_draws_without_weight_and_nearer_and_heavier_targets_win_more_often() {
        let none = LeisureTargets {
            list: vec![t(1, 0.0, 0.0)],
        };
        assert_eq!(pick_leisure(&none, Vec2::ZERO, 1000.0, 0.5), None);
        assert_eq!(
            pick_leisure(&LeisureTargets::default(), Vec2::ZERO, 1000.0, 0.5),
            None
        );

        // Equal weights, one 3 km away: the near one takes most of the draws.
        let two = LeisureTargets {
            list: vec![t(1, 100.0, 10.0), t(2, 3000.0, 10.0)],
        };
        let near = (0..100)
            .filter(|&k| {
                pick_leisure(&two, Vec2::ZERO, 1000.0, k as f32 / 100.0)
                    == Some(BuildingId::new(1, 0))
            })
            .count();
        assert!(
            (70..=90).contains(&near),
            "{near} of 100 go to the near one"
        );
        // A zero-weight target is never picked, whatever the draw.
        let gated = LeisureTargets {
            list: vec![t(1, 0.0, 0.0), t(2, 50.0, 1.0)],
        };
        for k in 0..20 {
            assert_eq!(
                pick_leisure(&gated, Vec2::ZERO, 1000.0, k as f32 / 20.0),
                Some(BuildingId::new(2, 0))
            );
        }
    }
}
