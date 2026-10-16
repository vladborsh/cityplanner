use std::path::Path;

use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Lot,
};
use cp_content::{BuildingDefId, ContentDb, Massing, RoofKind, ZoneType};
use cp_core::{Id, Tick};
use cp_net::{NetGraph, SegmentId};
use cp_terrain::Terrain;

use super::*;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

struct Env {
    db: ContentDb,
    terrain: Terrain,
    net: NetGraph,
}

fn env() -> Env {
    Env {
        db: ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap(),
        terrain: Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap(),
        net: NetGraph::new(),
    }
}

fn building(def: BuildingDefId, db: &ContentDb, state: BuildingState, ready_at: u64) -> Building {
    let d = db.building(def);
    Building {
        def,
        footprint: cp_building::Footprint::Lot(Lot {
            block: Id::new(0, 0),
            col: 0,
            w: d.lot.0,
            d: d.lot.1,
        }),
        level: d.level,
        state,
        ready_at: Tick(ready_at),
        pos: Vec2::new(5120.0, 5120.0),
        rot: 0.0,
        access: Some(AccessPoint {
            segment: SegmentId::new(0, 0),
            t: 0.5,
        }),
        flags: BuildingFlags::default(),
        level_progress: 0.0,
    }
}

fn id(i: u32) -> BuildingId {
    Id::new(i, 0)
}

fn extent(m: &MeshData) -> (Vec3, Vec3) {
    crate::road::bounds(m)
}

#[test]
fn every_def_builds_valid_meshes_that_fit_their_lot() {
    let e = env();
    for (def_id, def) in e.db.buildings() {
        let b = building(def_id, &e.db, BuildingState::Active, 0);
        let p = build_building(id(1), &b, def, &e.net, &e.terrain, &e.db, Tick(0));
        for (name, m) in [("near", &p.near), ("mid", &p.mid), ("far", &p.far)] {
            assert!(m.triangle_count() >= 10, "{} {name}", def.id);
            assert!(m.indices.iter().all(|&i| (i as usize) < m.positions.len()));
            assert_eq!(m.positions.len(), m.normals.len());
            assert_eq!(m.positions.len(), m.colors.len());
        }
        // Detail only goes down.
        assert!(
            p.near.triangle_count() > p.mid.triangle_count(),
            "{}",
            def.id
        );
        assert!(
            p.mid.triangle_count() >= p.far.triangle_count(),
            "{}",
            def.id
        );
        // The mesh stays inside its lot (rot 0: u = x, depth along z).
        let (lo, hi) = extent(&p.near);
        let (hw, hd) = (f32::from(def.lot.0) * 4.0, f32::from(def.lot.1) * 4.0);
        assert!(
            lo.x >= b.pos.x - hw - 1e-3 && hi.x <= b.pos.x + hw + 1e-3,
            "{} x",
            def.id
        );
        assert!(
            lo.z >= b.pos.y - hd - 1e-3 && hi.z <= b.pos.y + hd + 1e-3,
            "{} z",
            def.id
        );
    }
}

#[test]
fn height_follows_floors_and_roof() {
    let e = env();
    let ground = e.terrain.height(Vec2::new(5120.0, 5120.0));
    for (def_id, def) in e.db.buildings().filter(|(_, d)| d.service.is_none()) {
        let b = building(def_id, &e.db, BuildingState::Active, 0);
        let p = build_building(id(1), &b, def, &e.net, &e.terrain, &e.db, Tick(0));
        let st = facade_style(def.zone, def.level);
        let (style, vp) = variant_for(&e.db, id(1), &b, def);
        let walls = f32::from(vp.floors) * st.floor_h;
        let (_, hi) = extent(&p.far);
        let rise = hi.y - ground - PLINTH - walls;
        match style.roof {
            RoofKind::Flat if style.massing == Massing::Sawtooth => {
                assert!(rise > 1.0 && rise < 2.5, "{}: sawtooth rise {rise}", def.id)
            }
            RoofKind::Flat => {
                assert!(rise.abs() < 0.6, "{}: flat roof rise {rise}", def.id)
            }
            _ => assert!(rise > 1.0 && rise < 5.0, "{}: pitched rise {rise}", def.id),
        }
        assert!(walls > 2.5, "{}", def.id);
    }
}

/// Every triangle's winding agrees with its vertex normals (nothing is inside out).
#[test]
fn winding_matches_normals_for_every_state() {
    let e = env();
    for state in [
        BuildingState::Active,
        BuildingState::UnderConstruction,
        BuildingState::Abandoned,
        BuildingState::Burned,
    ] {
        for (def_id, def) in e.db.buildings().step_by(5) {
            let b = building(def_id, &e.db, state, 1000);
            let p = build_building(id(3), &b, def, &e.net, &e.terrain, &e.db, Tick(500));
            for m in [&p.near, &p.mid, &p.far] {
                for t in 0..m.triangle_count() {
                    let i = |k: usize| m.indices[t * 3 + k] as usize;
                    let pos = |k: usize| Vec3::from_array(m.positions[i(k)]);
                    let n = (pos(1) - pos(0)).cross(pos(2) - pos(0));
                    if n.length() < 1e-6 {
                        continue;
                    }
                    let vn = Vec3::from_array(m.normals[i(0)]);
                    assert!(
                        n.normalize().dot(vn) > 0.0,
                        "{} {state:?}: triangle {t} faces against its normal",
                        def.id
                    );
                }
            }
        }
    }
}

#[test]
fn construction_grows_through_four_stages() {
    let e = env();
    let (def_id, def) =
        e.db.buildings()
            .find(|(_, d)| variant(&e.db, id(1), d).1.floors >= 4)
            .unwrap();
    let ready = build_ticks(def);
    let b = building(def_id, &e.db, BuildingState::UnderConstruction, ready);
    let stages: Vec<u8> = [
        0,
        ready / 4 + 1,
        ready / 2 + 1,
        ready * 3 / 4 + 1,
        ready - 1,
    ]
    .iter()
    .map(|&t| construction_stage(&b, ready, Tick(t)))
    .collect();
    assert_eq!(stages, [0, 1, 2, 3, 3]);
    let height = |tick: u64| {
        let p = build_building(id(1), &b, def, &e.net, &e.terrain, &e.db, Tick(tick));
        extent(&p.far).1.y
    };
    let hs: Vec<f32> = [0, ready / 4 + 1, ready / 2 + 1, ready * 3 / 4 + 1]
        .iter()
        .map(|&t| height(t))
        .collect();
    assert!(hs.windows(2).all(|w| w[1] > w[0] + 0.5), "{hs:?}");
    // Near shows scaffolding (many more triangles than the plain core).
    let p = build_building(id(1), &b, def, &e.net, &e.terrain, &e.db, Tick(0));
    assert!(p.near.triangle_count() > 3 * p.far.triangle_count());
    // The finished building is taller than any construction stage.
    let done = building(def_id, &e.db, BuildingState::Active, 0);
    let pd = build_building(id(1), &done, def, &e.net, &e.terrain, &e.db, Tick(0));
    assert!(extent(&pd.far).1.y >= hs[3]);
}

#[test]
fn abandoned_is_duller_and_burned_is_lower_and_dark() {
    let e = env();
    let (def_id, def) =
        e.db.buildings()
            .find(|(_, d)| (3..8).contains(&variant(&e.db, id(2), d).1.floors))
            .unwrap();
    let mk = |s| {
        let b = building(def_id, &e.db, s, 0);
        build_building(id(2), &b, def, &e.net, &e.terrain, &e.db, Tick(0))
    };
    let (active, abandoned, burned) = (
        mk(BuildingState::Active),
        mk(BuildingState::Abandoned),
        mk(BuildingState::Burned),
    );
    let brightness = |m: &MeshData| -> f32 {
        m.colors.iter().map(|c| c[0] + c[1] + c[2]).sum::<f32>() / m.colors.len() as f32
    };
    assert!(brightness(&abandoned.far) < brightness(&active.far));
    assert!(brightness(&burned.far) < 0.75 * brightness(&abandoned.far));
    assert!(extent(&burned.far).1.y < extent(&active.far).1.y - 1.0);
}

#[test]
fn facade_variation_comes_from_the_id_and_is_deterministic() {
    let e = env();
    let (def_id, def) =
        e.db.buildings()
            .find(|(_, d)| d.zone == ZoneType::ResLow)
            .unwrap();
    let b = building(def_id, &e.db, BuildingState::Active, 0);
    let mesh = |i: u32| build_building(id(i), &b, def, &e.net, &e.terrain, &e.db, Tick(0)).near;
    assert_eq!(mesh(7).colors, mesh(7).colors);
    assert_eq!(mesh(7).positions, mesh(7).positions);
    let distinct: BTreeSet<Vec<u32>> = (0..24)
        .map(|i| {
            mesh(i)
                .colors
                .iter()
                .flatten()
                .map(|c| c.to_bits())
                .collect()
        })
        .collect();
    assert!(
        distinct.len() >= 3,
        "only {} distinct facades in 24 ids",
        distinct.len()
    );
}

#[test]
fn facade_patterns_differ_by_zone_and_level() {
    use cp_content::ZoneType::*;
    let a = facade_style(ResLow, 1);
    let b = facade_style(Office, 3);
    assert!(b.ribbon && !a.ribbon);
    assert!(facade_style(ComLow, 3).win_w > facade_style(ComLow, 1).win_w);
    assert!(facade_style(Industrial, 1).floor_h > facade_style(ResHigh, 1).floor_h);
    assert!(facade_style(ComHigh, 1).shopfront);
}

#[test]
fn detail_level_switches_with_distance_and_has_hysteresis() {
    assert_eq!(select_level(100.0, None), 0);
    assert_eq!(select_level(800.0, None), 1);
    assert_eq!(select_level(2000.0, None), 2);
    assert_eq!(select_level(470.0, Some(0)), 0);
    assert_eq!(select_level(520.0, Some(0)), 1);
    assert_eq!(select_level(1550.0, Some(1)), 1);
    assert_eq!(select_level(1700.0, Some(1)), 2);
    // No facade details beyond 1.5 km: the far mesh has no more triangles than the box.
    let e = env();
    let (def_id, def) = e.db.buildings().next().unwrap();
    let b = building(def_id, &e.db, BuildingState::Active, 0);
    let p = build_building(id(1), &b, def, &e.net, &e.terrain, &e.db, Tick(0));
    assert!(p.far.triangle_count() <= 40);
}

#[test]
fn set_rebuilds_only_changed_buildings_and_batches_by_chunk() {
    let e = env();
    let mut grid = BuildingGrid::new();
    let defs: Vec<BuildingDefId> = e.db.buildings().map(|(i, _)| i).collect();
    let mut ids = Vec::new();
    for k in 0..40usize {
        let mut b = building(defs[k % defs.len()], &e.db, BuildingState::Active, 0);
        b.pos = Vec2::new(
            4000.0 + (k % 8) as f32 * 80.0,
            4000.0 + (k / 8) as f32 * 80.0,
        );
        ids.push(grid.insert(b));
    }
    let mut set = BuildingSet::default();
    let first = set.update(&grid, &e.net, &e.db, &e.terrain, Tick(0));
    assert_eq!(first.rebuilt, 40);
    assert_eq!(set.count(), 40);
    assert!(set.chunks().count() >= 2 && set.chunks().count() <= 6);
    let idle = set.update(&grid, &e.net, &e.db, &e.terrain, Tick(0));
    assert_eq!((idle.rebuilt, idle.removed), (0, 0));

    grid.get_mut(ids[3]).unwrap().state = BuildingState::Burned;
    let up = set.update(&grid, &e.net, &e.db, &e.terrain, Tick(0));
    assert_eq!(up.rebuilt, 1);
    assert_eq!(up.dirty_chunks.len(), 1);

    grid.remove(ids[5]);
    let up = set.update(&grid, &e.net, &e.db, &e.terrain, Tick(0));
    assert_eq!((up.rebuilt, up.removed), (0, 1));
    assert_eq!(set.count(), 39);

    for lvl in 0..3 {
        let mut tris = 0;
        for &c in set.chunks().collect::<Vec<_>>() {
            let m = set.chunk_mesh(c, lvl).unwrap();
            assert!(m.indices.iter().all(|&i| (i as usize) < m.positions.len()));
            tris += m.triangle_count();
        }
        assert_eq!(tris, set.triangle_count(lvl));
    }
}

#[test]
fn construction_stage_changes_trigger_a_rebuild_only_at_stage_boundaries() {
    let e = env();
    let mut grid = BuildingGrid::new();
    let def = e.db.buildings().next().unwrap().0;
    grid.insert(building(
        def,
        &e.db,
        BuildingState::UnderConstruction,
        build_ticks(e.db.building(def)),
    ));
    let mut set = BuildingSet::default();
    assert_eq!(
        set.update(&grid, &e.net, &e.db, &e.terrain, Tick(0))
            .rebuilt,
        1
    );
    assert_eq!(set.constructing(), 1);
    // Same stage: nothing to do.
    assert_eq!(
        set.update(&grid, &e.net, &e.db, &e.terrain, Tick(100))
            .rebuilt,
        0
    );
    // Next stage.
    assert_eq!(
        set.update(
            &grid,
            &e.net,
            &e.db,
            &e.terrain,
            Tick(build_ticks(e.db.building(def)) / 4 + 1)
        )
        .rebuilt,
        1
    );
}

#[test]
fn construction_stage_spans_the_buildings_own_duration() {
    let e = env();
    let (def_id, _) = e.db.buildings().next().unwrap();
    // A 2 h and a 6 h building: stage 0 right after spawn, the last stage just before ready_at.
    for hours in [2.0f32, 6.0] {
        let total = (hours * cp_core::time::TICKS_PER_HOUR).round() as u64;
        let spawn = 1000;
        let b = building(
            def_id,
            &e.db,
            BuildingState::UnderConstruction,
            spawn + total,
        );
        assert_eq!(construction_stage(&b, total, Tick(spawn)), 0);
        assert_eq!(construction_stage(&b, total, Tick(spawn + 1)), 0);
        assert_eq!(
            construction_stage(&b, total, Tick(spawn + total - 1)),
            STAGES - 1
        );
    }
}

fn four_style_db() -> ContentDb {
    let style = |n: u8| {
        format!(
            r#"Style((id: "s{n}", massing: Box, roof: Flat, floors: (2, 6), footprint_fill: (0.6, 1.0),
            facade: Punched, palette: (walls: [(0.5, 0.5, 0.{n}), (0.4, 0.4, 0.{n}), (0.3, 0.3, 0.{n})],
            roof: [(0.3, 0.3, 0.3)], trim: [(0.4, 0.4, 0.4)], glass: (0.2, 0.2, 0.3))))"#
        )
    };
    let mut src = vec![(
        "b.ron".to_string(),
        r#"Building((id: "multi", name: "M", zone: ResLow, level: 1, lot: (2, 2), tax_base: 1.0,
            styles: ["s1", "s2", "s3", "s4"]))"#
            .to_string(),
    )];
    src.extend((1..=4).map(|n| (format!("s{n}.ron"), style(n))));
    ContentDb::from_sources(src).unwrap()
}

/// T-111: the variant is a pure function of `(index, generation, def id)`, spreads over the
/// def's styles, and stays inside the style's ranges.
#[test]
fn variant_is_deterministic_and_spread_over_the_styles() {
    let db = four_style_db();
    let (_, def) = db.buildings().next().unwrap();
    let mut per_style = BTreeMap::<String, u32>::new();
    let mut floors = BTreeSet::new();
    let mut walls = BTreeSet::new();
    for i in 0..1000u32 {
        let (s, vp) = variant(&db, id(i), def);
        assert_eq!((s, vp), variant(&db, id(i), def));
        *per_style.entry(s.id.clone()).or_default() += 1;
        assert!((2..=6).contains(&vp.floors));
        assert!((0.6..=1.0).contains(&vp.fill));
        floors.insert(vp.floors);
        walls.insert(vp.wall.map(f32::to_bits));
    }
    assert_eq!(per_style.len(), 4);
    for (name, n) in &per_style {
        assert!(*n >= 150, "{name}: only {n} of 1000");
    }
    assert_eq!(floors.len(), 5, "every floor count of the range appears");
    assert!(walls.len() >= 6, "wall colours differ between buildings");
    // The generation is part of the hash: a rebuilt building on the same index may look new.
    let differs =
        (0..50u32).any(|i| variant(&db, Id::new(i, 0), def) != variant(&db, Id::new(i, 1), def));
    assert!(differs);
    // It only reads the building id and the def: the same pair after a save/load round trip
    // (a fresh database from the same content) answers the same.
    let db2 = four_style_db();
    let (_, def2) = db2.buildings().next().unwrap();
    for i in 0..100u32 {
        assert_eq!(variant(&db, id(i), def).1, variant(&db2, id(i), def2).1);
    }
}

const MASSINGS: [&str; 6] = [
    "Box",
    "LShape",
    "Stepped",
    "TowerOnPodium",
    "TwinBlocks",
    "Sawtooth",
];
const ROOFS: [&str; 5] = ["Flat", "Gabled", "Hipped", "Shed", "Mansard"];
const LOTS: [(u8, u8); 3] = [(1, 1), (2, 3), (4, 4)];

/// Every massing × roof style (with all details) on three lot sizes.
fn shapes_db() -> ContentDb {
    let mut src = Vec::new();
    for m in MASSINGS {
        for r in ROOFS {
            src.push((
                format!("s/{m}_{r}.ron"),
                format!(
                    r#"Style((id: "{m}_{r}", massing: {m}, roof: {r}, floors: (1, 14),
                    footprint_fill: (0.6, 1.0), facade: Punched,
                    details: [Chimney, Balconies, Awning, RooftopUnits, Sign, LoadingDock, Tanks, Fence],
                    palette: (walls: [(0.6, 0.6, 0.6)], roof: [(0.3, 0.3, 0.3)], trim: [(0.4, 0.4, 0.4)],
                        glass: (0.2, 0.2, 0.3))))"#
                ),
            ));
            for (w, d) in LOTS {
                src.push((
                    format!("b/{m}_{r}_{w}x{d}.ron"),
                    format!(
                        r#"Building((id: "{m}_{r}_{w}x{d}", name: "x", zone: ComLow, level: 1,
                        lot: ({w}, {d}), tax_base: 1.0, styles: ["{m}_{r}"]))"#
                    ),
                ));
            }
        }
    }
    ContentDb::from_sources(src).unwrap()
}

/// Most vertices a building of this massing may have per detail level (near, mid, far).
fn vertex_limit(massing: &str) -> [usize; 3] {
    match massing {
        "Box" => [6500, 900, 160],
        "LShape" | "TwinBlocks" => [8000, 1800, 300],
        "Stepped" | "TowerOnPodium" => [8000, 1800, 300],
        _ => [6000, 1100, 260],
    }
}

/// Builds one building and checks that every detail level is a sound mesh; returns the vertex
/// counts (near, mid, far).
fn check_building(
    e: &Env,
    db: &ContentDb,
    def_id: BuildingDefId,
    i: u32,
    state: BuildingState,
    ready: u64,
) -> [usize; 3] {
    let def = db.building(def_id);
    let b = building(def_id, db, state, ready);
    let p = build_building(id(i), &b, def, &e.net, &e.terrain, db, Tick(300));
    let what = format!("{} #{i} {state:?}", def.id);
    let (hw, hd) = (f32::from(def.lot.0) * 4.0, f32::from(def.lot.1) * 4.0);
    // The foundation reaches 0.3 m below the lowest box corner, so nothing may sit
    // lower than the lowest ground under the lot.
    let lot_floor = (0..81)
        .map(|k| {
            let (i, j) = ((k / 9) as f32 / 4.0 - 1.0, (k % 9) as f32 / 4.0 - 1.0);
            e.terrain
                .height(Vec2::new(b.pos.x + i * hw, b.pos.y + j * hd))
        })
        .fold(f32::MAX, f32::min);
    for m in [&p.near, &p.mid, &p.far] {
        assert!(!m.positions.is_empty(), "{what}");
        assert_eq!(m.positions.len(), m.normals.len());
        assert_eq!(m.positions.len(), m.colors.len());
        assert!(m.indices.len().is_multiple_of(3));
        assert!(m.indices.iter().all(|&k| (k as usize) < m.positions.len()));
        for (pos, n) in m.positions.iter().zip(&m.normals) {
            assert!(pos.iter().chain(n).all(|v| v.is_finite()), "{what} NaN");
            let len = Vec3::from_array(*n).length();
            assert!((len - 1.0).abs() < 1e-3, "{what}: normal length {len}");
            // Inside the lot (x/z), and not below the ground by more than the
            // foundation's 0.3 m.
            assert!((pos[0] - b.pos.x).abs() <= hw + 1e-3, "{what}: x {pos:?}");
            assert!((pos[2] - b.pos.y).abs() <= hd + 1e-3, "{what}: z {pos:?}");
            assert!(pos[1] >= lot_floor - 0.31, "{what}: below ground {pos:?}");
        }
        for t in m.indices.chunks(3) {
            let q = |k: u32| Vec3::from_array(m.positions[k as usize]);
            let area = (q(t[1]) - q(t[0])).cross(q(t[2]) - q(t[0])).length() * 0.5;
            assert!(area > 1e-6, "{what}: degenerate triangle ({area})");
        }
    }
    if state != BuildingState::UnderConstruction {
        let n = [
            p.near.positions.len(),
            p.mid.positions.len(),
            p.far.positions.len(),
        ];
        assert!(n[2] <= n[1] && n[1] <= n[0], "{what}: LOD vertices {n:?}");
    }
    [
        p.near.positions.len(),
        p.mid.positions.len(),
        p.far.positions.len(),
    ]
}

/// T-112: every shape × roof is a sound mesh in every state and detail level.
#[test]
fn every_shape_and_roof_builds_a_sound_mesh_inside_its_lot() {
    let e = env();
    let db = shapes_db();
    let mut worst = BTreeMap::<&str, [usize; 3]>::new();
    for (def_id, def) in db.buildings() {
        let massing = MASSINGS.iter().find(|m| def.id.starts_with(*m)).unwrap();
        for i in 0..24u32 {
            for (state, ready) in STATES {
                let n = check_building(&e, &db, def_id, i, state, ready);
                let w = worst.entry(massing).or_default();
                for k in 0..3 {
                    w[k] = w[k].max(n[k]);
                }
            }
        }
    }
    for (massing, w) in &worst {
        let lim = vertex_limit(massing);
        eprintln!("{massing}: worst near/mid/far {w:?} (limit {lim:?})");
        for k in 0..3 {
            assert!(
                w[k] <= lim[k],
                "{massing}: LOD {k} has {} vertices, limit {}",
                w[k],
                lim[k]
            );
        }
    }
}

const STATES: [(BuildingState, u64); 4] = [
    (BuildingState::Active, 0),
    (BuildingState::Abandoned, 0),
    (BuildingState::Burned, 0),
    (BuildingState::UnderConstruction, 1000),
];

/// T-113: the same checks on every shipped def with the shipped styles.
#[test]
fn every_shipped_style_builds_a_sound_mesh() {
    let e = env();
    let mut seen = BTreeSet::new();
    for (def_id, def) in e.db.buildings() {
        for i in 0..60u32 {
            for (state, ready) in STATES {
                check_building(&e, &e.db, def_id, i, state, ready);
            }
            seen.insert(variant(&e.db, id(i), def).0.id.clone());
        }
    }
    assert_eq!(
        seen.len(),
        e.db.styles().count(),
        "a shipped style never drawn: {seen:?}"
    );
}

/// T-112: the shapes really differ: each massing and roof adds geometry to the plain box.
#[test]
fn massings_have_their_silhouettes() {
    let e = env();
    let db = shapes_db();
    let size = |name: &str| {
        let (def_id, def) = db.buildings().find(|(_, d)| d.id == name).unwrap();
        let mut total = 0usize;
        for i in 0..40 {
            let b = building(def_id, &db, BuildingState::Active, 0);
            let p = build_building(id(i), &b, def, &e.net, &e.terrain, &db, Tick(0));
            total += p.far.positions.len();
        }
        total
    };
    let boxy = size("Box_Flat_4x4");
    assert!(size("LShape_Flat_4x4") > boxy);
    assert!(size("TwinBlocks_Flat_4x4") > boxy);
    assert!(size("Stepped_Flat_4x4") > boxy);
    assert!(size("TowerOnPodium_Flat_4x4") > boxy);
    assert!(size("Sawtooth_Flat_4x4") > boxy);
    assert!(size("Box_Mansard_4x4") > boxy);
    assert!(size("Box_Shed_4x4") > boxy);
}

/// T-113 authoring aid: average vertices per style on its defs' lots (near / mid / far),
/// `cargo test -p cp_render style_cost_table -- --ignored --nocapture`.
#[test]
#[ignore = "prints a table"]
fn style_cost_table() {
    let e = env();
    let mut rows = BTreeMap::<String, (usize, [usize; 3])>::new();
    for (def_id, def) in e.db.buildings().filter(|(_, d)| d.service.is_none()) {
        let b = building(def_id, &e.db, BuildingState::Active, 0);
        for i in 0..200u32 {
            let (style, _) = variant_for(&e.db, id(i), &b, def);
            let p = build_building(id(i), &b, def, &e.net, &e.terrain, &e.db, Tick(0));
            let r = rows.entry(style.id.clone()).or_default();
            r.0 += 1;
            r.1[0] += p.near.positions.len();
            r.1[1] += p.mid.positions.len();
            r.1[2] += p.far.positions.len();
        }
    }
    for (name, (n, v)) in rows {
        println!("{name:<32} {:>6} {:>5} {:>4}", v[0] / n, v[1] / n, v[2] / n);
    }
}

/// T-113 authoring aid: per def, the average near / mid / far vertices of its looks against the
/// plain box it replaced.
#[test]
#[ignore = "prints a table"]
fn def_cost_table() {
    let e = env();
    let base: BTreeMap<&str, Vec<f32>> = include_str!("baseline_counts.txt")
        .lines()
        .map(|l| {
            let v: Vec<&str> = l.split(' ').collect();
            (v[0], (1..4).map(|k| v[k].parse().unwrap()).collect())
        })
        .collect();
    for (def_id, def) in e.db.buildings().filter(|(_, d)| d.service.is_none()) {
        let b = building(def_id, &e.db, BuildingState::Active, 0);
        let mut sum = [0usize; 3];
        for i in 0..200u32 {
            let p = build_building(id(i), &b, def, &e.net, &e.terrain, &e.db, Tick(0));
            sum[0] += p.near.positions.len();
            sum[1] += p.mid.positions.len();
            sum[2] += p.far.positions.len();
        }
        let bl = &base[def.id.as_str()];
        println!(
            "{:<12} lot {:?}  near {:>5} ({:.2}x)  mid {:>4} ({:.2}x)  far {:>3} ({:.2}x)",
            def.id,
            def.lot,
            sum[0] / 200,
            sum[0] as f32 / 200.0 / bl[0],
            sum[1] / 200,
            sum[1] as f32 / 200.0 / bl[1],
            sum[2] / 200,
            sum[2] as f32 / 200.0 / bl[2]
        );
    }
}

fn service_def<'a>(e: &'a Env, name: &str) -> (BuildingDefId, &'a BuildingDef) {
    e.db.buildings().find(|(_, d)| d.id == name).unwrap()
}

/// T-114: each service silhouette is a mesh of its own: three LODs shrinking, taller than a
/// plain box for the tall ones, and the six differ from each other.
#[test]
fn the_six_service_silhouettes_are_distinct_and_tall_where_they_should_be() {
    let e = env();
    let ground = e.terrain.height(Vec2::new(5120.0, 5120.0));
    let mut heights = BTreeMap::new();
    let mut shapes = BTreeSet::new();
    for name in [
        "water_pump_station",
        "sewage_outflow_station",
        "coal_power_plant",
        "wind_turbine",
        "water_tower",
        "sewage_treatment_plant",
    ] {
        let (def_id, def) = service_def(&e, name);
        let b = building(def_id, &e.db, BuildingState::Active, 0);
        let p = build_building(id(1), &b, def, &e.net, &e.terrain, &e.db, Tick(0));
        assert!(p.near.triangle_count() > p.mid.triangle_count(), "{name}");
        assert!(p.mid.triangle_count() >= p.far.triangle_count(), "{name}");
        let (lo, hi) = extent(&p.far);
        heights.insert(name, hi.y - ground);
        shapes.insert((
            p.near.positions.len(),
            p.mid.positions.len(),
            p.far.positions.len(),
        ));
        // Only the turbine has a rotor.
        assert_eq!(p.rotor.is_some(), name == "wind_turbine", "{name}");
        assert!(lo.y < hi.y);
    }
    assert_eq!(shapes.len(), 6, "{shapes:?}");
    // Heights: pump and outflow huts are low, the coal chimneys, the tower and the mast tall.
    assert!(heights["water_pump_station"] < 8.0 && heights["sewage_outflow_station"] < 6.0);
    assert!(heights["coal_power_plant"] > 28.0);
    assert!(heights["wind_turbine"] > 36.0);
    assert!(heights["water_tower"] > 18.0);
    assert!(heights["sewage_treatment_plant"] < 8.0);
}

/// T-114: the turbine's rotor is three blades around the hub, in the plane perpendicular to its
/// axis, turning only by the angle given (a view effect: the mesh is rebuilt from the angle).
#[test]
fn the_rotor_has_three_blades_turned_by_the_angle() {
    let e = env();
    let (def_id, def) = service_def(&e, "wind_turbine");
    let b = building(def_id, &e.db, BuildingState::Active, 0);
    let p = build_building(id(1), &b, def, &e.net, &e.terrain, &e.db, Tick(0));
    let r = p.rotor.unwrap();
    assert!(r.axis.y.abs() < 1e-4 && (r.axis.length() - 1.0).abs() < 1e-4);
    let tower_top = extent(&p.far).1.y;
    assert!(
        (r.hub.y - tower_top).abs() < 3.0,
        "hub {} tower {tower_top}",
        r.hub.y
    );
    let (a, c) = (rotor_world(&r, 0.0), rotor_world(&r, 0.5));
    assert_eq!(a.positions.len(), c.positions.len());
    assert_ne!(a.positions, c.positions);
    // Every vertex is within the blade radius of the hub and in the plane of the rotor.
    for pos in a.positions.iter().chain(&c.positions) {
        let d = Vec3::from_array(*pos) - r.hub;
        assert!(d.length() <= r.radius + 0.05, "{d:?}");
        assert!(d.dot(r.axis).abs() < 1e-3, "{d:?}");
    }
    // A full turn of 120 degrees looks the same.
    let third = rotor_world(&r, std::f32::consts::TAU / 3.0);
    let (lo0, hi0) = extent(&a);
    let (lo1, hi1) = extent(&third);
    assert!((lo0 - lo1).length() < 1e-2 && (hi0 - hi1).length() < 1e-2);
    // An abandoned or burned turbine does not turn.
    for s in [BuildingState::Abandoned, BuildingState::Burned] {
        let b = building(def_id, &e.db, s, 0);
        let p = build_building(id(1), &b, def, &e.net, &e.terrain, &e.db, Tick(0));
        assert!(p.rotor.is_none());
    }
}

/// T-114: a pump placed on water stands on the water surface, not on the bed.
#[test]
fn a_pump_on_water_stands_above_the_surface() {
    let e = env();
    let terrain = Terrain::load(&Path::new(ROOT).join("../../assets/maps/river_valley")).unwrap();
    // A spot whose whole 2x2 lot is under water.
    let mut spot = None;
    'scan: for iz in (0..1024).step_by(8) {
        for ix in (0..1024).step_by(8) {
            let p = Vec2::new(ix as f32 * 10.0, iz as f32 * 10.0);
            let wet = [(-8.0, -8.0), (8.0, -8.0), (8.0, 8.0), (-8.0, 8.0)]
                .iter()
                .all(|(dx, dz)| terrain.water_depth(p + Vec2::new(*dx, *dz)) > 1.0);
            if wet {
                spot = Some(p);
                break 'scan;
            }
        }
    }
    let p = spot.expect("river_valley has water");
    let (def_id, def) = service_def(&e, "water_pump_station");
    let mut b = building(def_id, &e.db, BuildingState::Active, 0);
    b.pos = p;
    let piece = build_building(id(1), &b, def, &e.net, &terrain, &e.db, Tick(0));
    let surface = terrain.height(p) + terrain.water_depth(p);
    let (lo, hi) = extent(&piece.near);
    assert!(hi.y > surface + 3.0, "hut top {} surface {surface}", hi.y);
    // Nothing of the pad or hut sits below the surface except the foundation and the intake
    // pipe down to the bed: the hut walls (vertices above the pad) all start at or above it.
    let hut_bottoms = piece.far.positions.iter().filter(|v| v[1] > surface - 0.05);
    assert!(hut_bottoms.count() > 0);
    assert!(lo.y < surface, "the foundation reaches the bed");
}
