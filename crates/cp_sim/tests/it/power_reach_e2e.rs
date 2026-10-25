//! BUG-087: the right-hand blocks of the user's town get no power although the houses stand
//! close together. The town of the shot, built from its buildings (the supply is a pure
//! function of the content, the buildings and the utility graph, so the roads themselves do not
//! matter; the street between two blocks is `STREET_M`, not a road type's width): two wind
//! turbines without a power line north-west of four blocks in a row, an industrial, a
//! commercial and two residential ones, each 64 m wide and 128 m long, filled with the real
//! buildings of the game content.

use std::path::Path;

use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Footprint, Lot,
};
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_core::{Id, Tick};
use cp_utility::{UtilityGraph, UtilityNet, compute_supply};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
/// The street between two blocks, kerb to kerb plus pavements (the shot: about 20-30 m).
const STREET_M: f32 = 24.0;
const BLOCK_W: f32 = 64.0;
const BLOCK_L: f32 = 128.0;
const CELL: f32 = cp_building::FOOTPRINT_CELL_M;

fn content() -> ContentDb {
    ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap()
}

fn add(grid: &mut BuildingGrid, db: &ContentDb, def: &str, centre: Vec2, free: bool) -> BuildingId {
    let id = db.building_id(def).unwrap_or_else(|| panic!("{def}"));
    let (w, d) = db.building(id).lot;
    grid.insert(Building {
        def: id,
        footprint: if free {
            Footprint::Free { w, d }
        } else {
            Footprint::Lot(Lot {
                block: Id::new(0, 0),
                col: 0,
                w,
                d,
            })
        },
        level: 1,
        state: BuildingState::Active,
        ready_at: Tick(0),
        pos: centre,
        rot: 0.0,
        access: (!free).then_some(AccessPoint {
            segment: Id::new(0, 0),
            t: 0.0,
        }),
        flags: BuildingFlags::default(),
        level_progress: 0.0,
    })
}

/// One block: its two long frontages (along z) filled with `defs` in turn, lots touching.
fn block(grid: &mut BuildingGrid, db: &ContentDb, x0: f32, defs: &[&str]) -> Vec<BuildingId> {
    let mut out = Vec::new();
    let mut k = 0;
    for side in [0, 1] {
        let mut z = 0.0;
        loop {
            let def = defs[k % defs.len()];
            let (w, d) = db.building(db.building_id(def).unwrap()).lot;
            let (along, deep) = (f32::from(w) * CELL, f32::from(d) * CELL);
            if z + along > BLOCK_L {
                break;
            }
            let x = if side == 0 {
                x0 + deep / 2.0
            } else {
                x0 + BLOCK_W - deep / 2.0
            };
            out.push(add(grid, db, def, Vec2::new(x, z + along / 2.0), false));
            z += along;
            k += 1;
        }
    }
    out
}

struct Town {
    grid: BuildingGrid,
    blocks: Vec<(&'static str, Vec<BuildingId>)>,
}

fn town(db: &ContentDb) -> Town {
    let mut grid = BuildingGrid::new();
    let o = Vec2::new(5000.0, 5000.0);
    for x in [12.0, 52.0] {
        add(&mut grid, db, "wind_turbine", o + Vec2::new(x, -40.0), true);
    }
    let kinds: [(&'static str, &[&str]); 4] = [
        ("industrial", &["ind_2", "ind_3", "ind_5", "ind_1", "ind_6"]),
        (
            "commercial",
            &["com_low_1", "com_low_2", "com_low_3", "com_low_1b"],
        ),
        (
            "residential 1",
            &[
                "house_low_2",
                "house_low_3",
                "apt_high_4",
                "house_low_1",
                "house_low_5",
            ],
        ),
        (
            "residential 2",
            &["apt_high_4", "apt_high_5", "house_low_2", "house_low_3"],
        ),
    ];
    let mut blocks = Vec::new();
    for (i, (name, defs)) in kinds.into_iter().enumerate() {
        let x0 = o.x + i as f32 * (BLOCK_W + STREET_M);
        let mut ids = block(&mut grid, db, x0, defs);
        // `block` lays at z from 0: move the block to the town's origin.
        for id in &ids {
            grid.get_mut(*id).unwrap().pos.y += o.y;
        }
        ids.sort();
        blocks.push((name, ids));
    }
    Town { grid, blocks }
}

fn power_need(db: &ContentDb, grid: &BuildingGrid, id: BuildingId) -> f32 {
    let def = db.building(grid.get(id).unwrap().def);
    def.consumption
        .get(&cp_content::UtilityKind::Power)
        .copied()
        .unwrap_or(0.0)
}

/// Per block: (name, buildings, without power, demand, demand served).
type Row = (&'static str, usize, usize, f32, f32);

fn report(db: &ContentDb, t: &Town, g: &UtilityGraph) -> Vec<Row> {
    let m = compute_supply(db, &t.grid, g);
    let off = |id: BuildingId| m.get(&id).is_some_and(|b| b & BuildingFlags::NO_POWER != 0);
    t.blocks
        .iter()
        .map(|(name, ids)| {
            let need: f32 = ids.iter().map(|&id| power_need(db, &t.grid, id)).sum();
            let served: f32 = ids
                .iter()
                .filter(|&&id| !off(id))
                .map(|&id| power_need(db, &t.grid, id))
                .sum();
            (
                *name,
                ids.len(),
                ids.iter().filter(|&&id| off(id)).count(),
                need,
                served,
            )
        })
        .collect()
}

fn totals(rows: &[Row]) -> (f32, f32) {
    rows.iter().fold((0.0, 0.0), |a, r| (a.0 + r.3, a.1 + r.4))
}

fn show(what: &str, rows: &[Row]) {
    let (need, served) = totals(rows);
    eprintln!("== {what}: demand {need:.1}, served {served:.1}");
    for (name, n, off, need, served) in rows {
        eprintln!(
            "   {name:<14} {n:>3} buildings, {off:>3} without power, demand {need:7.1}, served {served:7.1}"
        );
    }
}

/// The user's town (the shot): two turbines without a line give their 600 together, so the
/// three blocks nearest them are powered whole and only the farthest buildings are dark, by
/// the 70 the town needs more than 600. Before the fix the town got one turbine's 300: the
/// two right blocks were dark from end to end.
#[test]
fn two_free_turbines_power_the_town_together() {
    let db = content();
    let t = town(&db);
    let rows = report(&db, &t, &UtilityGraph::new());
    show("two turbines, no line", &rows);
    let (need, served) = totals(&rows);
    assert!(
        need > 600.0,
        "the town needs more than the turbines give: {need}"
    );
    assert!(served > 550.0 && served <= 600.0, "served {served} of 600");
    for r in &rows[..3] {
        assert_eq!(r.2, 0, "{}: every building has power", r.0);
    }
    // A line between the turbines changes nothing now: they were one pool already.
    let mut g = UtilityGraph::new();
    let p = |x: f32| Vec3::new(5000.0 + x, 0.0, 5000.0 - 40.0);
    let (a, b) = (
        g.add_node(UtilityNet::Power, p(12.0)),
        g.add_node(UtilityNet::Power, p(52.0)),
    );
    g.add_link(a, b, CubicBezier3::line(p(12.0), p(52.0)))
        .unwrap();
    assert_eq!(report(&db, &t, &g), rows, "a line between the turbines");
}

/// Enough plants without a line power everything: a third turbine (900 for 670), or a coal
/// plant beside the turbines (its 4000 used to be lost: the first turbine claimed the town).
#[test]
fn enough_free_plants_power_every_building() {
    let db = content();
    for extra in ["wind_turbine", "coal_power_plant"] {
        let mut t = town(&db);
        add(
            &mut t.grid,
            &db,
            extra,
            Vec2::new(5000.0 + 32.0, 5000.0 - 40.0),
            true,
        );
        let rows = report(&db, &t, &UtilityGraph::new());
        show(extra, &rows);
        for r in &rows {
            assert_eq!(r.2, 0, "plus a {extra}: {} all powered", r.0);
        }
    }
}

/// The same buildings and graph give the same flags and pools, twice over.
#[test]
fn the_power_pools_are_deterministic() {
    let db = content();
    let t = town(&db);
    let run = || {
        cp_utility::compute_supply_pools(
            &db,
            &t.grid,
            &UtilityGraph::new(),
            &mut cp_utility::AttachCache::default(),
            None,
            &Default::default(),
        )
    };
    assert_eq!(run(), run());
}

/// A building without power says why (BUG-087): a shortage in its pool (the output and the
/// demand), or no plant at all.
#[test]
fn a_building_without_power_says_why() {
    let db = content();
    let t = town(&db);
    let (flags, pools, _) = cp_utility::compute_supply_pools(
        &db,
        &t.grid,
        &UtilityGraph::new(),
        &mut cp_utility::AttachCache::default(),
        None,
        &Default::default(),
    );
    let dark = t.blocks[3]
        .1
        .iter()
        .copied()
        .find(|id| flags[id.index as usize] & BuildingFlags::NO_POWER != 0)
        .expect("a dark building in the farthest block");
    let p = pools.of(dark).expect("its pool");
    assert_eq!(p.capacity, 600.0);
    assert!(p.demand > 600.0, "{p:?}");
    // A house 200 m from everything: a pool of its own without a plant.
    let mut far = town(&db);
    let lone = add(
        &mut far.grid,
        &db,
        "house_low_1",
        Vec2::new(5000.0, 5000.0 + 400.0),
        false,
    );
    let (flags, pools, _) = cp_utility::compute_supply_pools(
        &db,
        &far.grid,
        &UtilityGraph::new(),
        &mut cp_utility::AttachCache::default(),
        None,
        &Default::default(),
    );
    assert_ne!(flags[lone.index as usize] & BuildingFlags::NO_POWER, 0);
    assert_eq!(pools.of(lone).map(|p| p.capacity), Some(0.0));
}
