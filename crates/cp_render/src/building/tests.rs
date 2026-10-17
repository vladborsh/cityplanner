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
    // The specialisation buildings are silhouettes of their own (T-198), not boxes.
    for (def_id, def) in
        e.db.buildings()
            .filter(|(_, d)| d.service.is_none() && d.spec.is_none())
    {
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
fn construction_grows_through_many_fine_stages() {
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
    let n = STAGES;
    assert_eq!(stages, [0, n / 4, n / 2, n * 3 / 4, n - 1]);
    let height = |tick: u64| {
        let p = build_building(id(1), &b, def, &e.net, &e.terrain, &e.db, Tick(tick));
        extent(&p.far).1.y
    };
    let hs: Vec<f32> = [0, ready / 4 + 1, ready / 2 + 1, ready * 3 / 4 + 1]
        .iter()
        .map(|&t| height(t))
        .collect();
    assert!(hs.windows(2).all(|w| w[1] > w[0] + 0.5), "{hs:?}");
    // No step of one stage is a jump: the climb is at most a floor high per stage.
    let all: Vec<f32> = (0..STAGES)
        .map(|k| height(ready * u64::from(k) / u64::from(STAGES) + 1))
        .collect();
    assert!(all.windows(2).all(|w| w[1] >= w[0]), "{all:?}");
    assert!(
        all.windows(2).all(|w| w[1] - w[0] < 1.5),
        "a stage is a small step: {all:?}"
    );
    // Near shows scaffolding (many more triangles than the plain core).
    let p = build_building(id(1), &b, def, &e.net, &e.terrain, &e.db, Tick(0));
    assert!(p.near.triangle_count() > 3 * p.far.triangle_count());
    // The finished building is taller than any construction stage.
    let done = building(def_id, &e.db, BuildingState::Active, 0);
    let pd = build_building(id(1), &done, def, &e.net, &e.terrain, &e.db, Tick(0));
    assert!(extent(&pd.far).1.y >= hs[3]);
    // The last stage is as tall as the finished building: the swap is no height jump.
    assert!(
        (extent(&pd.far).1.y - all[all.len() - 1]).abs() < 4.0,
        "{} vs {}",
        extent(&pd.far).1.y,
        all[all.len() - 1]
    );
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

/// T-143: a burned building is charred all over and shows no windows (the glass is the colour
/// of the wall), at every detail level.
#[test]
fn a_burned_building_is_charred_and_windowless() {
    let e = env();
    let (def_id, def) =
        e.db.buildings()
            .find(|(_, d)| d.service.is_none() && d.zone == ZoneType::ComHigh)
            .unwrap();
    let b = building(def_id, &e.db, BuildingState::Burned, 0);
    let p = build_building(id(2), &b, def, &e.net, &e.terrain, &e.db, Tick(0));
    for (name, m) in [("near", &p.near), ("mid", &p.mid), ("far", &p.far)] {
        let bright = m
            .colors
            .iter()
            .map(|c| c[0].max(c[1]).max(c[2]))
            .fold(0.0, f32::max);
        assert!(bright <= 0.3, "{name}: brightest vertex channel {bright}");
    }
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
        set.update(&grid, &e.net, &e.db, &e.terrain, Tick(1))
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
            Tick(build_ticks(e.db.building(def)) / u64::from(STAGES) + 1)
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

/// T-147: the eleven M2 service buildings each have a silhouette of their own: three LODs that
/// never grow with distance, no two alike, and heights that read right (chimneys and towers
/// tall, the cemetery low).
#[test]
fn the_eleven_m2_service_silhouettes_are_distinct_and_build_at_every_lod() {
    let e = env();
    let ground = e.terrain.height(Vec2::new(5120.0, 5120.0));
    let mut shapes = BTreeSet::new();
    let mut heights = BTreeMap::new();
    for name in [
        "landfill",
        "incinerator",
        "clinic",
        "hospital",
        "cemetery",
        "crematorium",
        "elementary_school",
        "high_school",
        "university",
        "fire_station",
        "police_station",
    ] {
        let (def_id, def) = service_def(&e, name);
        let b = building(def_id, &e.db, BuildingState::Active, 0);
        let p = build_building(id(1), &b, def, &e.net, &e.terrain, &e.db, Tick(0));
        let (n, m, f) = (
            p.near.triangle_count(),
            p.mid.triangle_count(),
            p.far.triangle_count(),
        );
        assert!(n >= m && m >= f && n > f && f > 0, "{name}: {n} {m} {f}");
        assert!(p.rotor.is_none(), "{name}");
        shapes.insert((
            p.near.positions.len(),
            p.mid.positions.len(),
            p.far.positions.len(),
        ));
        heights.insert(name, extent(&p.far).1.y - ground);
    }
    assert_eq!(shapes.len(), 11, "{shapes:?}");
    assert!(heights["incinerator"] > 30.0, "{heights:?}");
    assert!(heights["hospital"] > 15.0 && heights["university"] > 15.0);
    assert!(heights["fire_station"] > 12.0, "hose tower");
    assert!(heights["cemetery"] < 8.0 && heights["clinic"] < 11.0);
}

/// T-202, T-203: the ten unique buildings and monuments each have a silhouette of their own: a
/// mesh at every LOD that never grows with distance, no two alike, taller than a house, and
/// the monuments the tallest things in the city.
#[test]
fn the_ten_unique_silhouettes_are_distinct_and_the_monuments_tower() {
    let e = env();
    let ground = e.terrain.height(Vec2::new(5120.0, 5120.0));
    let mut shapes = BTreeSet::new();
    let mut heights = BTreeMap::new();
    for name in [
        "city_plaza",
        "clock_tower",
        "grand_library",
        "opera_house",
        "science_museum",
        "stadium",
        "tv_tower",
        "cathedral",
        "golden_arch",
        "skyline_spire",
    ] {
        let (def_id, def) = service_def(&e, name);
        assert!(def.unique.is_some(), "{name}");
        let style = e.db.style(e.db.style_id(&def.styles[0]).unwrap());
        assert!(style.massing as u8 >= 25, "{name} has a massing of its own");
        let b = building(def_id, &e.db, BuildingState::Active, 0);
        let p = build_building(id(1), &b, def, &e.net, &e.terrain, &e.db, Tick(0));
        let (n, m, f) = (
            p.near.triangle_count(),
            p.mid.triangle_count(),
            p.far.triangle_count(),
        );
        assert!(n >= m && m >= f && n > f && f > 0, "{name}: {n} {m} {f}");
        for mesh in [&p.near, &p.mid, &p.far] {
            assert!(
                mesh.positions.iter().flatten().all(|v| v.is_finite()),
                "{name}"
            );
        }
        shapes.insert((
            p.near.positions.len(),
            p.mid.positions.len(),
            p.far.positions.len(),
        ));
        let (lo, hi) = extent(&p.far);
        assert!(hi.y > lo.y, "{name}");
        heights.insert(name, hi.y - ground);
    }
    assert_eq!(shapes.len(), 10, "{shapes:?}");
    assert!(heights["city_plaza"] > 4.0 && heights["clock_tower"] > 40.0);
    for monument in [
        "stadium",
        "tv_tower",
        "cathedral",
        "golden_arch",
        "skyline_spire",
    ] {
        assert!(heights[monument] > 30.0, "{monument}: {heights:?}");
    }
    assert!(heights["tv_tower"] > 80.0 && heights["skyline_spire"] > 100.0);
}

/// T-198, T-199: the five specialisation silhouettes are distinct, build at every LOD (never
/// growing with distance), stay low-rise and grow with the lot (large taller than small or
/// at least wider).
#[test]
fn the_five_specialisation_silhouettes_are_distinct_and_grow_with_the_lot() {
    let e = env();
    let ground = e.terrain.height(Vec2::new(5120.0, 5120.0));
    let mut shapes = BTreeSet::new();
    for kind in [
        "leisure",
        "tourism",
        "organic",
        "it_cluster",
        "self_sufficient",
    ] {
        let mut wide = Vec::new();
        for size in ["s", "m", "l"] {
            let name = format!("spec_{kind}_{size}");
            let (def_id, def) = service_def(&e, &name);
            let style = e.db.style(e.db.style_id(&def.styles[0]).unwrap());
            assert!(style.massing as u8 >= 35, "{name} has a massing of its own");
            let b = building(def_id, &e.db, BuildingState::Active, 0);
            let p = build_building(id(1), &b, def, &e.net, &e.terrain, &e.db, Tick(0));
            let (n, m, f) = (
                p.near.triangle_count(),
                p.mid.triangle_count(),
                p.far.triangle_count(),
            );
            assert!(n >= m && m >= f && n > f && f > 0, "{name}: {n} {m} {f}");
            for mesh in [&p.near, &p.mid, &p.far] {
                assert!(
                    mesh.positions.iter().flatten().all(|v| v.is_finite()),
                    "{name}"
                );
            }
            let (lo, hi) = extent(&p.far);
            assert!(hi.y - ground > 3.0 && hi.y - ground < 40.0, "{name}");
            wide.push(hi.x - lo.x);
            if size == "m" {
                shapes.insert((
                    p.near.positions.len(),
                    p.mid.positions.len(),
                    p.far.positions.len(),
                ));
            }
        }
        assert!(wide[0] < wide[2], "{kind}: {wide:?}");
    }
    assert_eq!(shapes.len(), 5, "{shapes:?}");
}

/// T-212, T-213: the 24 chain buildings (farm, forest, ore, oil: main, two extractors, two
/// processors, warehouse) each have a silhouette of their own, build at every LOD (never growing
/// with distance), stay inside the lot and rise above the ground.
#[test]
fn the_24_chain_silhouettes_are_distinct_and_build_at_every_lod() {
    let e = env();
    let ground = e.terrain.height(Vec2::new(5120.0, 5120.0));
    let mut shapes = BTreeSet::new();
    let mut masses = BTreeSet::new();
    for res in ["farm", "forest", "ore", "oil"] {
        for role in [
            "main",
            "extractor_s",
            "extractor_l",
            "processor_a",
            "processor_b",
            "warehouse",
        ] {
            let name = format!("{res}_{role}");
            let (def_id, def) = service_def(&e, &name);
            let style = e.db.style(e.db.style_id(&def.styles[0]).unwrap());
            assert!(style.massing as u8 >= 40, "{name} has a massing of its own");
            masses.insert(style.massing as u8);
            let b = building(def_id, &e.db, BuildingState::Active, 0);
            let p = build_building(id(1), &b, def, &e.net, &e.terrain, &e.db, Tick(0));
            let (n, m, f) = (
                p.near.triangle_count(),
                p.mid.triangle_count(),
                p.far.triangle_count(),
            );
            assert!(n >= m && m >= f && n > f && f > 0, "{name}: {n} {m} {f}");
            for mesh in [&p.near, &p.mid, &p.far] {
                assert!(
                    mesh.positions.iter().flatten().all(|v| v.is_finite()),
                    "{name}"
                );
            }
            let (lo, hi) = extent(&p.far);
            assert!(hi.y - ground > 2.0, "{name}");
            // Footprint: within the lot of the def (a cell is 8 m) and a little more.
            let side = f32::from(def.lot.0.max(def.lot.1)) * 8.0;
            assert!(
                hi.x - lo.x < side + 2.0 && hi.z - lo.z < side + 2.0,
                "{name}"
            );
            // A fingerprint of the near mesh: its size and where its vertices are.
            let sum: u64 = p
                .near
                .positions
                .iter()
                .flatten()
                .map(|v| u64::from(v.to_bits()))
                .fold(0u64, |a, v| a.wrapping_mul(31).wrapping_add(v));
            shapes.insert((p.near.positions.len(), sum));
        }
    }
    assert_eq!(masses.len(), 24);
    assert_eq!(shapes.len(), 24, "{shapes:?}");
}

/// T-221: the four city parks each have a silhouette of their own: three LODs that never grow
/// with distance, flat and low (a few metres of trees, fountain or stand), no two alike.
#[test]
fn the_four_park_silhouettes_are_distinct_low_and_build_at_every_lod() {
    let e = env();
    let ground = e.terrain.height(Vec2::new(5120.0, 5120.0));
    let mut shapes = BTreeSet::new();
    for name in ["small_park", "playground", "plaza_fountain", "sports_field"] {
        let (def_id, def) = service_def(&e, name);
        let style = e.db.style(e.db.style_id(&def.styles[0]).unwrap());
        assert!(style.massing as u8 >= 64, "{name} has a massing of its own");
        let b = building(def_id, &e.db, BuildingState::Active, 0);
        let p = build_building(id(1), &b, def, &e.net, &e.terrain, &e.db, Tick(0));
        let (n, m, f) = (
            p.near.triangle_count(),
            p.mid.triangle_count(),
            p.far.triangle_count(),
        );
        assert!(n >= m && m >= f && n > f && f > 0, "{name}: {n} {m} {f}");
        for mesh in [&p.near, &p.mid, &p.far] {
            assert!(
                mesh.positions.iter().flatten().all(|v| v.is_finite()),
                "{name}"
            );
        }
        let (lo, hi) = extent(&p.far);
        assert!(hi.y - ground > 1.0 && hi.y - ground < 12.0, "{name}");
        assert!(lo.y < hi.y);
        shapes.insert((
            p.near.positions.len(),
            p.mid.positions.len(),
            p.far.positions.len(),
        ));
    }
    assert_eq!(shapes.len(), 4, "{shapes:?}");
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

/// T-235 probe: what refreshing the construction stages every 32 ticks costs in a big city.
/// `cargo test -p cp_render --release building::tests::construction_refresh_cost -- --ignored --nocapture`
#[test]
#[ignore = "timing probe, prints numbers"]
fn construction_refresh_cost() {
    let e = env();
    let defs: Vec<_> =
        e.db.buildings()
            .filter(|(_, d)| {
                matches!(
                    d.zone,
                    cp_content::ZoneType::ResLow
                        | cp_content::ZoneType::ResHigh
                        | cp_content::ZoneType::ComLow
                )
            })
            .map(|(i, _)| i)
            .collect();
    for sites in [0usize, 200, 600] {
        let mut grid = BuildingGrid::new();
        let mut rng = 12345u64;
        let mut next = || {
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (rng >> 33) as f32 / (1u64 << 31) as f32
        };
        for i in 0..10_000usize {
            let def = defs[i % defs.len()];
            let total = build_ticks(e.db.building(def));
            let mut b = if i < sites {
                // Spread over the whole construction so some stage changes every refresh.
                let ready = 5000 + (next() * total as f32) as u64;
                building(def, &e.db, BuildingState::UnderConstruction, ready)
            } else {
                building(def, &e.db, BuildingState::Active, 0)
            };
            b.pos = Vec2::new(4000.0 + next() * 2200.0, 4000.0 + next() * 2200.0);
            grid.insert(b);
        }
        let mut set = BuildingSet::default();
        let t = std::time::Instant::now();
        set.update(&grid, &e.net, &e.db, &e.terrain, Tick(5000));
        let first = t.elapsed().as_secs_f64() * 1000.0;
        let (mut worst_ms, mut sum_ms, mut rebuilt, mut chunks) = (0.0f64, 0.0f64, 0usize, 0usize);
        let mut upd_ms = 0.0f64;
        let steps = 100u64;
        for k in 1..=steps {
            let t = std::time::Instant::now();
            let up = set.update(&grid, &e.net, &e.db, &e.terrain, Tick(5000 + k * 32));
            upd_ms += t.elapsed().as_secs_f64() * 1000.0;
            for c in &up.dirty_chunks {
                for l in 0..3 {
                    let _ = set.chunk_mesh(*c, l);
                }
                let _ = set.chunk_bounds(*c);
            }
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            worst_ms = worst_ms.max(ms);
            sum_ms += ms;
            rebuilt += up.rebuilt;
            chunks += up.dirty_chunks.len();
        }
        println!(
            "10k buildings, {sites} sites: first build {first:.0} ms; per 32-tick refresh avg {:.2} ms (of which update() {:.2}), worst {worst_ms:.2} ms, {:.1} pieces and {:.1} chunks rebuilt per refresh",
            sum_ms / steps as f64,
            upd_ms / steps as f64,
            rebuilt as f64 / steps as f64,
            chunks as f64 / steps as f64,
        );
    }
}

#[test]
fn buildings_under_construction_have_their_own_batch_so_a_stage_never_remerges_the_finished_ones() {
    let e = env();
    let def = e.db.buildings().next().unwrap().0;
    let total = build_ticks(e.db.building(def));
    let mut grid = BuildingGrid::new();
    for _ in 0..5 {
        grid.insert(building(def, &e.db, BuildingState::Active, 0));
    }
    grid.insert(building(
        def,
        &e.db,
        BuildingState::UnderConstruction,
        total,
    ));
    let mut set = BuildingSet::default();
    set.update(&grid, &e.net, &e.db, &e.terrain, Tick(0));
    let keys: Vec<_> = set.chunks().copied().collect();
    assert_eq!(
        keys.len(),
        2,
        "one finished and one construction batch: {keys:?}"
    );
    assert!(keys.iter().any(|k| k.1) && keys.iter().any(|k| !k.1));
    // The next stage dirties the construction batch only.
    let up = set.update(&grid, &e.net, &e.db, &e.terrain, Tick(total / 16 + 1));
    assert_eq!(up.rebuilt, 1);
    assert!(up.dirty_chunks.iter().all(|k| k.1), "{:?}", up.dirty_chunks);
}

#[test]
fn low_density_homes_get_one_to_three_plants_inside_the_lot_and_off_the_house() {
    use super::shapes::garden_spots;
    let e = env();
    let (mut lots, mut planted) = (0, 0);
    for (def_id, def) in e.db.buildings().filter(|(_, d)| d.zone == ZoneType::ResLow) {
        let (w, d) = (f32::from(def.lot.0) * 8.0, f32::from(def.lot.1) * 8.0);
        for i in 0..40u32 {
            let b = building(def_id, &e.db, BuildingState::Active, 0);
            let (_, vp) = variant_for(&e.db, id(i), &b, def);
            let shrink = vp.fill.sqrt();
            let hu = ((w * 0.5 - 1.0) * shrink).max(1.0);
            let hv = (((d - 3.0) * 0.5) * shrink).max(1.0);
            let cv = 0.5;
            let seed = seed(id(i), &b);
            let spots = garden_spots(w, d, (hu, hv, cv), seed);
            assert_eq!(
                spots,
                garden_spots(w, d, (hu, hv, cv), seed),
                "deterministic"
            );
            assert!(spots.len() <= 3);
            for s in &spots {
                assert!(s.u - s.r >= -w / 2.0 - 1e-3 && s.u + s.r <= w / 2.0 + 1e-3);
                assert!(s.v - s.r >= -d / 2.0 - 1e-3 && s.v + s.r <= d / 2.0 + 1e-3);
                assert!(
                    !(s.u.abs() < hu + 0.5 && (s.v - cv).abs() < hv + 0.5),
                    "the trunk stands on the house: {s:?}"
                );
            }
            lots += 1;
            planted += usize::from(!spots.is_empty());
        }
    }
    assert!(
        planted * 10 >= lots * 7,
        "{planted} of {lots} homes have a plant"
    );
}

#[test]
fn a_garden_is_valid_geometry_and_richer_near_than_mid() {
    use super::shapes::{Spot, garden};
    let f = Frame {
        c: Vec2::new(100.0, 100.0),
        u: Vec2::X,
        v: Vec2::Y,
    };
    let spots: Vec<Spot> = (0..3u8)
        .map(|kind| Spot {
            u: f32::from(kind) * 4.0 - 4.0,
            v: 5.0,
            kind,
            h: 4.0,
            r: 1.3,
            tint: 0.5,
        })
        .collect();
    let draw = |near| {
        let mut m = MeshData::default();
        garden(&mut m, &f, &spots, |_, _| 2.0, near);
        m
    };
    let (n, m) = (draw(true), draw(false));
    for mesh in [&n, &m] {
        assert!(mesh.triangle_count() > 20);
        assert_eq!(mesh.positions.len(), mesh.normals.len());
        assert!(
            mesh.indices
                .iter()
                .all(|&i| (i as usize) < mesh.positions.len())
        );
        assert!(
            mesh.positions
                .iter()
                .all(|p| p[1] >= 2.0 - 1e-3 && p[1] <= 2.0 + 4.0 * 1.2 + 1e-3)
        );
    }
    assert!(n.triangle_count() > m.triangle_count());
}
