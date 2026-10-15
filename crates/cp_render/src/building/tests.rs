use std::path::Path;

use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Lot,
};
use cp_content::{BuildingDefId, ContentDb, RoofKind};
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
    for (def_id, def) in e.db.buildings() {
        let b = building(def_id, &e.db, BuildingState::Active, 0);
        let p = build_building(id(1), &b, def, &e.net, &e.terrain, &e.db, Tick(0));
        let st = facade_style(def.zone, def.level);
        let (style, vp) = variant(&e.db, id(1), def);
        let walls = f32::from(vp.floors) * st.floor_h;
        let (_, hi) = extent(&p.far);
        let rise = hi.y - ground - PLINTH - walls;
        match style.roof {
            RoofKind::Flat | RoofKind::Shed | RoofKind::Mansard => {
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
            .find(|(_, d)| d.styles[0].starts_with("legacy_suburb_warm"))
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

/// T-111: the mesh of every shipped def has the vertex counts it had before styles moved into
/// content (`baseline_counts.txt`, recorded on the code before the change): the shipped defs
/// each use one legacy style, so nothing about the geometry changes.
#[test]
fn meshes_keep_their_vertex_counts_after_the_style_move() {
    let e = env();
    let mut seen = 0;
    for line in include_str!("baseline_counts.txt").lines() {
        let [name, near, mid, far] = line.split(' ').collect::<Vec<_>>()[..] else {
            panic!("bad line {line}");
        };
        let (def_id, def) =
            e.db.buildings()
                .find(|(_, d)| d.id == name)
                .unwrap_or_else(|| panic!("def {name} is gone"));
        let b = building(def_id, &e.db, BuildingState::Active, 0);
        let p = build_building(id(1), &b, def, &e.net, &e.terrain, &e.db, Tick(0));
        let got = [
            p.near.positions.len(),
            p.mid.positions.len(),
            p.far.positions.len(),
        ];
        let want = [near, mid, far].map(|v| v.parse::<usize>().unwrap());
        assert_eq!(got, want, "{name}: vertices near/mid/far");
        seen += 1;
    }
    assert_eq!(seen, e.db.buildings().count());
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
