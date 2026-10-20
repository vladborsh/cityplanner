//! T-287: the houses' moods when a city service comes or goes (`cp_render::mood`, pure). A
//! street of four 10-cell pieces with homes grown on both sides; a service is put in or taken
//! out of a copy of the building grid, and the moods are the houses whose coverage changed:
//!
//! * a police station: every house it newly covers is glad, nobody else; bulldozed, the same
//!   houses are sorry;
//! * a park (100 m): only the houses near it; moved to the other end, the houses it left are
//!   sorry, the ones it reaches glad, nobody both;
//! * a water pump: nobody reacts;
//! * a landfill among the homes: the homes within 8 cells of it are sorry, never glad; taken
//!   away, they are relieved;
//! * many services at once (a load): nobody reacts.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec2;
use cp_building::{
    AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Demand,
    Footprint,
};
use cp_content::{ContentDb, ServiceKind, ZoneType};
use cp_core::{Cost, Tick, Treasury};
use cp_net::{ElevationMode, NetCommand, NetGraph, RoadPath, SegmentId};
use cp_render::mood::{GARBAGE_M, LOAD_EVENTS, moods};
use cp_service::{RoadGraph, kind_coverage};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_zone::{ZoneCellRef, ZoneGrid};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const A: Vec2 = Vec2::new(4400.0, 4400.0);
const PIECE: f32 = 80.0;

/// The town: the roads, the homes and the content.
fn town() -> (NetGraph, BuildingGrid, Arc<ContentDb>) {
    let db = Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let t = Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::new(287);
    sim.set_terrain(t);
    sim.set_content(Arc::clone(&db));
    sim.tick_once();
    sim.world_mut().resource_mut::<Treasury>().balance = Cost::from_units(1_000_000_000.0);
    let road = db.road_id("basic_2l").unwrap();
    for k in 0..4 {
        sim.push_command(
            SimCommand::Net(NetCommand::BuildRoad {
                road,
                path: RoadPath::Straight {
                    from: A + Vec2::X * (k as f32 * PIECE),
                    to: A + Vec2::X * ((k + 1) as f32 * PIECE),
                },
                elevation: ElevationMode::Ground,
            }),
            sim.tick(),
        );
        sim.tick_once();
    }
    {
        let mut zones = sim.world_mut().resource_mut::<ZoneGrid>();
        let ids: Vec<_> = zones.blocks().map(|(id, b)| (id, b.cell_count())).collect();
        for (id, cells) in ids {
            for cell in 0..cells {
                zones.set_zone(
                    ZoneCellRef {
                        block: id,
                        cell: cell as u16,
                    },
                    ZoneType::ResLow,
                );
            }
        }
    }
    *sim.world_mut().resource_mut::<Demand>() = Demand::fixed(1.0);
    for _ in 0..8000 {
        sim.tick_once();
        let active = sim
            .world()
            .resource::<BuildingGrid>()
            .iter()
            .filter(|(_, b)| b.state == BuildingState::Active)
            .count();
        if active >= 20 {
            break;
        }
    }
    let net = sim.world().resource::<NetGraph>().clone();
    let grid = sim.world().resource::<BuildingGrid>().clone();
    assert!(net.segments().count() >= 4);
    (net, grid, db)
}

/// The ground segment nearest `p`.
fn segment_at(net: &NetGraph, p: Vec2) -> SegmentId {
    net.segments()
        .min_by(|a, b| {
            a.1.curve
                .nearest_xz(p)
                .1
                .total_cmp(&b.1.curve.nearest_xz(p).1)
        })
        .unwrap()
        .0
}

/// `grid` with an Active `def` standing 20 m south of the street at `x` metres along it.
fn with(
    grid: &BuildingGrid,
    net: &NetGraph,
    db: &ContentDb,
    def: &str,
    x: f32,
) -> (BuildingGrid, BuildingId) {
    let mut g = grid.clone();
    let pos = A + Vec2::new(x, 20.0);
    let seg = segment_at(net, A + Vec2::X * x);
    let t = net.segment(seg).unwrap().curve.nearest_xz(pos).0;
    let id = g.insert(Building {
        def: db.building_id(def).unwrap(),
        footprint: Footprint::Free { w: 2, d: 2 },
        level: 1,
        state: BuildingState::Active,
        ready_at: Tick(0),
        pos,
        rot: 0.0,
        access: Some(AccessPoint { segment: seg, t }),
        flags: BuildingFlags::default(),
        level_progress: 0.0,
    });
    (g, id)
}

fn homes(grid: &BuildingGrid, db: &ContentDb) -> Vec<(BuildingId, Building)> {
    grid.iter()
        .filter(|(_, b)| {
            b.state == BuildingState::Active
                && matches!(
                    db.building(b.def).zone,
                    ZoneType::ResLow | ZoneType::ResHigh
                )
        })
        .map(|(id, b)| (id, b.clone()))
        .collect()
}

/// The homes whose coverage of `kind` goes up (`+`) or down (`-`) from `old` to `new`: what
/// the sim's fold gives, read independently of `mood`.
fn changed(
    net: &NetGraph,
    old: &BuildingGrid,
    new: &BuildingGrid,
    db: &ContentDb,
    kind: ServiceKind,
) -> (BTreeSet<BuildingId>, BTreeSet<BuildingId>) {
    let g = RoadGraph::new(net);
    let (before, after) = (
        kind_coverage(&g, old, db, kind),
        kind_coverage(&g, new, db, kind),
    );
    let (mut up, mut down) = (BTreeSet::new(), BTreeSet::new());
    for (id, b) in homes(new, db) {
        let s = b.access().unwrap().segment.index as usize;
        let d = after[s] - before[s];
        if d > 0.01 {
            up.insert(id);
        } else if d < -0.01 {
            down.insert(id);
        }
    }
    (up, down)
}

fn split(m: &[cp_render::mood::Mood]) -> (BTreeSet<BuildingId>, BTreeSet<BuildingId>) {
    (
        m.iter().filter(|m| m.happy).map(|m| m.building).collect(),
        m.iter().filter(|m| !m.happy).map(|m| m.building).collect(),
    )
}

#[test]
fn a_service_gladdens_the_houses_it_covers_and_saddens_them_when_it_goes() {
    let (net, grid, db) = town();
    let n = homes(&grid, &db).len();
    assert!(n >= 20, "{n} homes");
    // A police station (850 m): every home is newly covered.
    let (with_police, _) = with(&grid, &net, &db, "police_station", 40.0);
    let m = moods(&net, &grid, &with_police, &db);
    let (glad, sorry) = split(&m);
    let (up, _) = changed(&net, &grid, &with_police, &db, ServiceKind::Police);
    assert_eq!(glad, up, "exactly the covered homes");
    assert_eq!(glad.len(), n);
    assert!(sorry.is_empty());
    // The wave runs out from the station: nearer homes first.
    assert!(m.windows(2).all(|w| w[0].delay <= w[1].delay));
    assert!(m.first().unwrap().delay < m.last().unwrap().delay);
    // Bulldozed: the same homes are sorry.
    let (glad, sorry) = split(&moods(&net, &with_police, &grid, &db));
    assert!(glad.is_empty());
    assert_eq!(sorry, up);
}

#[test]
fn a_park_reaches_only_its_neighbours_and_a_move_saddens_the_old_ones() {
    let (net, grid, db) = town();
    let (at_start, park) = with(&grid, &net, &db, "small_park", 20.0);
    let (glad, _) = split(&moods(&net, &grid, &at_start, &db));
    let (up, _) = changed(&net, &grid, &at_start, &db, ServiceKind::Park);
    assert_eq!(glad, up);
    assert!(
        !glad.is_empty() && glad.len() < homes(&grid, &db).len(),
        "only its neighbours"
    );
    // Moved to the far end of the street.
    let mut moved = at_start.clone();
    let seg = segment_at(&net, A + Vec2::X * 300.0);
    let pos = A + Vec2::new(300.0, 20.0);
    let t = net.segment(seg).unwrap().curve.nearest_xz(pos).0;
    {
        let b = moved.get_mut(park).unwrap();
        b.pos = pos;
        b.access = Some(AccessPoint { segment: seg, t });
    }
    let (glad, sorry) = split(&moods(&net, &at_start, &moved, &db));
    let (up, down) = changed(&net, &at_start, &moved, &db, ServiceKind::Park);
    assert_eq!((glad.clone(), sorry.clone()), (up, down));
    assert!(!glad.is_empty() && !sorry.is_empty());
    assert!(glad.is_disjoint(&sorry));
}

#[test]
fn a_water_pump_moves_nobody() {
    let (net, grid, db) = town();
    let (pumped, _) = with(&grid, &net, &db, "water_pump_station", 40.0);
    assert!(moods(&net, &grid, &pumped, &db).is_empty());
}

#[test]
fn a_landfill_among_homes_saddens_its_neighbours_and_its_removal_relieves_them() {
    let (net, grid, db) = town();
    let (dump, id) = with(&grid, &net, &db, "landfill", 160.0);
    let (glad, sorry) = split(&moods(&net, &grid, &dump, &db));
    assert!(glad.is_empty(), "garbage is never welcome");
    let fill = dump.get(id).unwrap();
    let q = fill.footprint_polygon();
    let near: BTreeSet<BuildingId> = homes(&dump, &db)
        .into_iter()
        .filter(|(_, b)| {
            fill.footprint_contains(b.pos)
                || (0..4).any(|i| {
                    let (a, c) = (q[i], q[(i + 1) % 4]);
                    let ab = c - a;
                    let k = ((b.pos - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
                    (a + ab * k).distance(b.pos) <= GARBAGE_M
                })
        })
        .map(|(id, _)| id)
        .collect();
    assert!(!near.is_empty());
    assert_eq!(sorry, near);
    let (glad, sorry) = split(&moods(&net, &dump, &grid, &db));
    assert!(sorry.is_empty());
    assert_eq!(glad, near, "relief when it goes");
}

#[test]
fn many_services_at_once_are_a_load_not_an_action() {
    let (net, grid, db) = town();
    let mut g = grid.clone();
    for k in 0..=LOAD_EVENTS {
        g = with(&g, &net, &db, "police_station", 10.0 + 20.0 * k as f32).0;
    }
    assert!(moods(&net, &grid, &g, &db).is_empty());
}
