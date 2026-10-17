//! Visitors, entrance fees and walkers of the park areas (T-224, `architecture/system-design.md`
//! §16 M6 (D)).
//!
//! * [`ParkVisits`] holds who is inside (an area and the tick the visit ends) and the walkers
//!   that show people in the park; [`ParkLedger`] the entrance fees of the week.
//! * [`run_park`] (see the crate root) counts the arrivals at the gates, lets the visits end,
//!   and refills [`cp_citizen::LeisureTargets`] once a game hour.
//! * [`run_walkers`] keeps up to `walkers_per_park` pedestrians walking from the gate to an
//!   attraction and back while a park has visitors. They are citizen-less walking runs
//!   (`ServiceRuns::start_walk`, like the tourists who come by train) from the shared pool.

use std::collections::BTreeMap;

use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Res, ResMut};
use bevy_math::Vec2;
use cp_building::{BuildingGrid, BuildingId, BuildingState};
use cp_content::{ContentDb, ParkKind, ParkRole, RoadClass, SimContent};
use cp_core::{Cost, Tick};
use cp_district::{AreaId, AreaKind, AreaMap, Areas, area_at};
use cp_net::NetGraph;
use cp_traffic::{Access, Agents, RunId, RunPoint, RunState, ServiceRuns, Side};
use serde::{Deserialize, Serialize};

use crate::ParkState;

/// One visitor inside a park area until `leave_at`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Visit {
    pub area: u8,
    pub leave_at: Tick,
}

/// A pedestrian walking from the gate to a point beside an attraction (`back` = on the way
/// back).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Walker {
    pub area: u8,
    pub gate: BuildingId,
    pub far: Access,
    pub back: bool,
}

/// Who is in the parks. Saved and in the checksum once non-empty.
#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ParkVisits {
    pub inside: Vec<Visit>,
    pub walkers: BTreeMap<RunId, Walker>,
}

impl ParkVisits {
    pub fn is_empty(&self) -> bool {
        self.inside.is_empty() && self.walkers.is_empty()
    }

    /// Visitors inside the area now.
    pub fn inside_of(&self, area: u8) -> u32 {
        self.inside.iter().filter(|v| v.area == area).count() as u32
    }
}

/// The entrance fees: this week's and the last settled week's. Saved and in the checksum once
/// non-empty. `cp_economy` books the week (T-225).
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParkLedger {
    pub fees: Cost,
    pub last_fees: Cost,
}

impl ParkLedger {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// A uniform `0..1` from `h` (a splitmix step), for the deterministic choices.
pub(crate) fn unit(h: u64) -> f32 {
    let mut z = h.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32
}

/// The park area of `kind` a gate at `pos` touches: the nearest area cell of that kind within
/// 24 m.
pub fn gate_area(areas: &Areas, area_map: &AreaMap, kind: ParkKind, pos: Vec2) -> Option<AreaId> {
    let mut offsets: Vec<(i32, i32)> = (-3..=3)
        .flat_map(|i| (-3..=3).map(move |j| (i * 8, j * 8)))
        .collect();
    offsets.sort_by_key(|&(x, z)| (x * x + z * z, x, z));
    offsets.into_iter().find_map(|(x, z)| {
        let id = area_at(area_map, pos + Vec2::new(x as f32, z as f32))?;
        matches!(areas.get(id)?.kind, AreaKind::Park(k) if k == kind).then_some(id)
    })
}

/// The point of the nearest pedestrian path within `max_m` of `pos`, as the path service wants
/// it; `None` without a path there.
pub fn path_access(net: &NetGraph, db: &ContentDb, pos: Vec2, max_m: f32) -> Option<Access> {
    let to_xz = |v: bevy_math::Vec3| Vec2::new(v.x, v.z);
    let (d, id, t) = net
        .segments()
        .filter(|(_, s)| db.road(s.road).class == RoadClass::Path)
        .map(|(id, s)| {
            let (t, d) = s.curve.nearest_xz(pos);
            (d, id, t)
        })
        .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))?;
    if d > max_m {
        return None;
    }
    let seg = net.segment(id)?;
    let tg = seg.curve.tangent(t);
    let left = Vec2::new(tg.z, -tg.x);
    let side = if (pos - to_xz(seg.curve.point(t))).dot(left) >= 0.0 {
        Side::Left
    } else {
        Side::Right
    };
    Some(Access {
        segment: id,
        s: seg.curve.arc_table().t_to_s(t),
        side,
    })
}

/// How far from an attraction the walkers' path end may be.
const PATH_REACH_M: f32 = 60.0;

/// `Agents` phase, after `run_park`: keeps the walkers of every park with visitors going.
#[allow(clippy::too_many_arguments)]
pub fn run_walkers(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    buildings: Res<BuildingGrid>,
    net: Res<NetGraph>,
    area_map: Res<AreaMap>,
    areas: Res<Areas>,
    state: Res<ParkState>,
    mut visits: ResMut<ParkVisits>,
    mut runs: ResMut<ServiceRuns>,
    mut agents: ResMut<Agents>,
) {
    let Some(content) = content else {
        return;
    };
    let db = &*content.0;
    let now = tick.0;

    // 1. The walkers under way.
    let ids: Vec<RunId> = visits.walkers.keys().copied().collect();
    for id in ids {
        let w = visits.walkers[&id];
        match runs.get(id).map(|r| r.state) {
            None | Some(RunState::Failed) => {
                runs.release(id, &mut agents);
                visits.walkers.remove(&id);
            }
            Some(RunState::Arrived { .. }) => {
                runs.release(id, &mut agents);
                visits.walkers.remove(&id);
                // Out at the attraction: walk back while the park still has visitors.
                if !w.back && state.areas.get(&w.area).is_some_and(|a| a.inside > 0) {
                    let back = runs.start_walk(RunPoint::Access(w.far), RunPoint::Building(w.gate));
                    visits.walkers.insert(back, Walker { back: true, ..w });
                }
            }
            Some(_) => {}
        }
    }

    // 2. New walkers for the parks that have visitors, at most one per area every 32 ticks.
    let per_park = usize::from(db.balance().walkers_per_park);
    let wanted: Vec<(u8, u32)> = state
        .areas
        .iter()
        .filter(|(_, a)| a.inside > 0)
        .map(|(&id, a)| (id, a.inside))
        .collect();
    for (area, inside) in wanted {
        let have = visits.walkers.values().filter(|w| w.area == area).count();
        if have >= per_park.min(inside as usize) || now % 32 != u64::from(area) % 32 {
            continue;
        }
        let Some(AreaKind::Park(kind)) = areas.get(AreaId(area)).map(|a| a.kind) else {
            continue;
        };
        let mut gate = None;
        let mut attractions: Vec<Vec2> = Vec::new();
        for (id, b) in buildings.iter() {
            if b.state != BuildingState::Active {
                continue;
            }
            let Some(spec) = db
                .building(b.def)
                .park
                .as_ref()
                .filter(|s| s.park == Some(kind))
            else {
                continue;
            };
            match spec.role {
                ParkRole::Gate => {
                    if gate.is_none()
                        && gate_area(&areas, &area_map, kind, b.pos) == Some(AreaId(area))
                    {
                        gate = Some(id);
                    }
                }
                ParkRole::Attraction => {
                    if area_at(&area_map, b.pos) == Some(AreaId(area)) {
                        attractions.push(b.pos);
                    }
                }
                ParkRole::Prop => {}
            }
        }
        let (Some(gate), false) = (gate, attractions.is_empty()) else {
            continue;
        };
        let pick = (unit(now ^ (u64::from(area) << 40)) * attractions.len() as f32) as usize;
        let at = attractions[pick.min(attractions.len() - 1)];
        let Some(far) = path_access(&net, db, at, PATH_REACH_M) else {
            continue;
        };
        let run = runs.start_walk(RunPoint::Building(gate), RunPoint::Access(far));
        visits.walkers.insert(
            run,
            Walker {
                area,
                gate,
                far,
                back: false,
            },
        );
    }
}
