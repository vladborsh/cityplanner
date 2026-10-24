//! The coverage the client shows (`architecture/system-design.md` §13.5 v6.2, T-254 step 2): for
//! the one service kind on screen, `cp_service::kind_coverage` of the current snapshot — what the
//! sim's next recompute (every 64 ticks) will give. A building placed while the game is paused
//! shows its coverage at once; the sim keeps its own boundary values for the rules.

use std::sync::Arc;

use bevy::prelude::*;
use cp_content::ServiceKind;
use cp_service::{RoadGraph, kind_coverage};
use cp_sim::SimView;

use super::{CoverageGhost, OverlayState};

/// Recompute at most this often while the snapshot keeps changing, seconds.
pub const LIVE_PERIOD_S: f64 = 0.25;

/// The live coverage of one kind per segment index.
#[derive(Resource, Default)]
pub struct LiveCoverage {
    kind: Option<ServiceKind>,
    values: Arc<Vec<f32>>,
    /// Pointers of the network and the buildings the values were computed from.
    key: (usize, usize),
    /// Changes at every recompute (the layers' cache key).
    pub generation: u64,
    last_s: f64,
    graph: Option<(usize, RoadGraph)>,
}

impl LiveCoverage {
    /// The values of `kind` when they are the live ones.
    pub fn of(&self, kind: ServiceKind) -> Option<&[f32]> {
        (self.kind == Some(kind)).then_some(self.values.as_slice())
    }

    /// Coverage of `kind` on the segment with arena index `index`, live when computed for
    /// `kind`, else `fallback`.
    pub fn value_or(&self, kind: ServiceKind, index: u32, fallback: impl Fn() -> f32) -> f32 {
        match self.of(kind) {
            Some(v) => v.get(index as usize).copied().unwrap_or(0.0),
            None => fallback(),
        }
    }
}

/// Whether the live values are out of date: another kind, or a new snapshot and the last
/// recompute at least [`LIVE_PERIOD_S`] ago.
pub fn needs_update(
    live: &LiveCoverage,
    kind: Option<ServiceKind>,
    key: (usize, usize),
    now_s: f64,
) -> bool {
    match kind {
        None => live.kind.is_some(),
        Some(_) if live.kind != kind => true,
        Some(_) => live.key != key && now_s - live.last_s >= LIVE_PERIOD_S,
    }
}

/// The kind on screen: the service overlay's, else the one of a service ghost.
pub fn shown_kind(state: &OverlayState, ghost: Option<&CoverageGhost>) -> Option<ServiceKind> {
    state
        .active
        .service_kind()
        .or_else(|| ghost.and_then(|g| g.site.as_ref()).map(|s| s.kind))
}

pub(super) fn update_live_coverage(
    view: Option<Res<SimView>>,
    state: Res<OverlayState>,
    ghost: Option<Res<CoverageGhost>>,
    time: Res<Time<Real>>,
    mut live: ResMut<LiveCoverage>,
) {
    let Some(view) = view else { return };
    let kind = shown_kind(&state, ghost.as_deref());
    let net_key = Arc::as_ptr(&view.net) as usize;
    let key = (net_key, Arc::as_ptr(&view.buildings) as usize);
    let now = time.elapsed_secs_f64();
    if !needs_update(&live, kind, key, now) {
        return;
    }
    let (Some(kind), Some(db)) = (kind, view.content.as_deref()) else {
        live.kind = None;
        live.generation += 1;
        return;
    };
    if live.graph.as_ref().is_none_or(|(k, _)| *k != net_key) {
        live.graph = Some((net_key, RoadGraph::new(&view.net)));
    }
    let Some((_, graph)) = live.graph.as_ref() else {
        return;
    };
    let values = kind_coverage(graph, &view.buildings, db, &view.budgets, kind);
    live.values = Arc::new(values);
    live.kind = Some(kind);
    live.key = key;
    live.last_s = now;
    live.generation += 1;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Overlay;

    #[test]
    fn the_values_follow_the_kind_at_once_and_a_new_snapshot_at_most_four_times_a_second() {
        let mut live = LiveCoverage::default();
        let k = Some(ServiceKind::Police);
        assert!(needs_update(&live, k, (1, 2), 0.0));
        live.kind = k;
        live.key = (1, 2);
        live.last_s = 10.0;
        // Same snapshot: nothing to do; a new one waits for the period.
        assert!(!needs_update(&live, k, (1, 2), 30.0));
        assert!(!needs_update(&live, k, (1, 3), 10.1));
        assert!(needs_update(&live, k, (1, 3), 10.25));
        // Another kind, or none: at once.
        assert!(needs_update(&live, Some(ServiceKind::Fire), (1, 2), 10.0));
        assert!(needs_update(&live, None, (1, 2), 10.0));
        assert!(!needs_update(&LiveCoverage::default(), None, (1, 2), 10.0));
    }

    #[test]
    fn the_kind_on_screen_is_the_overlays_else_the_ghosts() {
        let fire = OverlayState {
            active: Overlay::Fire,
            ..Default::default()
        };
        let off = OverlayState::default();
        let ghost = CoverageGhost {
            site: Some(super::super::CoverageSite {
                kind: ServiceKind::Police,
                access: cp_building::AccessPoint {
                    segment: cp_core::Id::new(0, 0),
                    t: 0.0,
                },
                radius_m: 500.0,
                capacity: 1,
                lot: Vec::new(),
            }),
        };
        assert_eq!(shown_kind(&fire, Some(&ghost)), Some(ServiceKind::Fire));
        assert_eq!(shown_kind(&off, Some(&ghost)), Some(ServiceKind::Police));
        assert_eq!(shown_kind(&off, None), None);
        let live = LiveCoverage {
            kind: Some(ServiceKind::Fire),
            values: Arc::new(vec![0.5]),
            ..Default::default()
        };
        assert_eq!(live.value_or(ServiceKind::Fire, 0, || 9.0), 0.5);
        assert_eq!(live.value_or(ServiceKind::Fire, 7, || 9.0), 0.0);
        assert_eq!(live.value_or(ServiceKind::Police, 0, || 9.0), 9.0);
    }
}
