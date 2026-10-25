//! T-267: a service stands at the road edge (§6.8.3 item 8). The building is a rectangle with an
//! access point: its front is flush against the edge of the road it faces (the half width from
//! the axis, [`cp_zone::ROAD_EPS`] of slack, like the zone cells), it slides along the road to
//! where it fits, and two four-cell-deep services fit back to back between streets 64 m apart.
//! An old service on a lot of cells that is moved becomes such a rectangle and frees its cells.

use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::{Vec2, Vec3};
use cp_building::{
    Building, BuildingCommand, BuildingFlags, BuildingGrid, BuildingState, Footprint, Lot,
};
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_core::{CommandOutcome, Tick};
use cp_net::{Elevation, NetGraph};
use cp_sim::{SimCommand, SimRunner};
use cp_zone::{ZoneCellRef, ZoneGrid};

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

/// `basic_2l` streets (half width 6 m) along x at the given `z`, owned land around.
fn town(zs: &[f32]) -> SimRunner {
    let db = content();
    let road = db.road_id("basic_2l").unwrap();
    let def = db.road(road).clone();
    let mut sim = SimRunner::new(1);
    sim.set_content(Arc::clone(&db));
    sim.world_mut()
        .insert_resource(cp_terrain::TileOwnership::only((1, 1)));
    for &z in zs {
        sim.world_mut()
            .resource_mut::<NetGraph>()
            .add_segment(
                &def,
                road,
                CubicBezier3::line(Vec3::new(3000.0, 0.0, z), Vec3::new(3200.0, 0.0, z)),
                Elevation::Ground,
            )
            .unwrap();
    }
    sim.tick_once();
    sim.world_mut().resource_mut::<cp_core::Treasury>().balance =
        cp_core::Cost::from_units(1_000_000.0);
    sim
}

fn place(sim: &mut SimRunner, name: &str, pos: Vec2) -> Result<(), String> {
    let def = sim
        .world()
        .resource::<cp_content::SimContent>()
        .0
        .building_id(name)
        .unwrap();
    sim.push_command(
        SimCommand::Building(BuildingCommand::Place { def, pos, rot: 0.0 }),
        sim.tick(),
    );
    sim.apply_due_commands();
    let out: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    out.last()
        .unwrap()
        .result
        .clone()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn rects(sim: &SimRunner) -> Vec<[Vec2; 4]> {
    sim.world()
        .resource::<BuildingGrid>()
        .iter()
        .map(|(_, b)| b.footprint_polygon())
        .collect()
}

/// The distances of a rectangle's corners from the line z = `axis`.
fn gap_to(q: &[Vec2; 4], axis: f32) -> (f32, f32) {
    let d: Vec<f32> = q.iter().map(|p| (p.y - axis).abs()).collect();
    (
        d.iter().copied().fold(f32::INFINITY, f32::min),
        d.iter().copied().fold(0.0, f32::max),
    )
}

#[test]
fn a_service_stands_flush_against_the_edge_facing_the_road() {
    let mut sim = town(&[3000.0]);
    // The cursor is 30 m north of the street (half width 6 m).
    place(&mut sim, "fire_station", Vec2::new(3100.0, 3030.0)).unwrap();
    let g = sim.world().resource::<BuildingGrid>();
    let (_, b) = g.iter().next().unwrap();
    assert!(
        matches!(b.footprint, Footprint::Free { .. }),
        "no lot of cells"
    );
    let access = b.access().expect("an access point on the road it faces");
    assert_eq!(
        access.segment,
        sim.world()
            .resource::<NetGraph>()
            .segments()
            .next()
            .unwrap()
            .0
    );
    // 3 x 2 cells, 3 along the street and 2 deep: the front on the edge (8 m from the axis: the
    // road is 2 cells wide, T-275), the back 16 m further.
    let q = b.footprint_polygon();
    let (near, far) = gap_to(&q, 3000.0);
    assert!((near - 8.0).abs() < 0.06, "front at {near} m from the axis");
    assert!((far - 24.0).abs() < 0.06, "back at {far} m");
    // On the cursor's side, and the access point is the foot of its centre.
    assert!(b.pos.y > 3000.0);
    assert_eq!(b.state, BuildingState::Active);
}

#[test]
fn two_four_cell_deep_services_fit_back_to_back_between_streets_64_m_apart() {
    // Edge to edge 64 m: axes 80 m apart, one section (half width 8, T-275).
    let mut sim = town(&[3000.0, 3080.0]);
    place(&mut sim, "university", Vec2::new(3100.0, 3022.0)).unwrap();
    place(&mut sim, "landfill", Vec2::new(3100.0, 3058.0)).unwrap();
    let r = rects(&sim);
    assert_eq!(r.len(), 2);
    assert!(!cp_zone::geom::quads_overlap(&r[0], &r[1]));
    // Each has its front on its own street's edge, and their backs meet.
    let on = |q: &[Vec2; 4]| {
        let c = (q[0] + q[2]) * 0.5;
        if c.y < 3040.0 {
            gap_to(q, 3000.0)
        } else {
            gap_to(q, 3080.0)
        }
    };
    for q in &r {
        let (near, far) = on(q);
        assert!((near - 8.0).abs() < 0.06, "front at {near}");
        assert!((far - 40.0).abs() < 0.06, "back at {far}");
    }
}

#[test]
fn an_old_service_on_cells_that_is_moved_becomes_a_rectangle_and_frees_its_cells() {
    let mut sim = town(&[3000.0]);
    let clinic = sim
        .world()
        .resource::<cp_content::SimContent>()
        .0
        .building_id("clinic")
        .unwrap();
    // The clinic as an old save has it: on the 2 x 2 lot from column 4, holding its cells.
    let block = sim
        .world()
        .resource::<ZoneGrid>()
        .blocks()
        .next()
        .unwrap()
        .0;
    let lot = Lot {
        block,
        col: 4,
        w: 2,
        d: 2,
    };
    let (pos, rot, access) = {
        let w = sim.world();
        cp_building::place(
            w.resource::<ZoneGrid>(),
            w.resource::<NetGraph>(),
            block,
            &lot,
        )
        .unwrap()
    };
    let id = sim
        .world_mut()
        .resource_mut::<BuildingGrid>()
        .insert(Building {
            def: clinic,
            footprint: Footprint::Lot(lot),
            level: 1,
            state: BuildingState::Active,
            ready_at: Tick(0),
            pos,
            rot,
            access: Some(access),
            flags: BuildingFlags(0),
            level_progress: 0.0,
        });
    let cells: Vec<ZoneCellRef> = lot.cells().collect();
    sim.world_mut().resource_mut::<ZoneGrid>().occupy(&cells);
    assert!(
        cells
            .iter()
            .all(|&c| sim.world().resource::<ZoneGrid>().is_occupied(c))
    );

    sim.push_command(
        SimCommand::Building(BuildingCommand::Move {
            building: id,
            pos: Vec2::new(3150.0, 3030.0),
            rot: 0.0,
        }),
        sim.tick(),
    );
    sim.apply_due_commands();
    let out: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    out.last().unwrap().result.clone().expect("the move");
    let g = sim.world().resource::<BuildingGrid>();
    let b = g.get(id).unwrap();
    assert!(matches!(b.footprint, Footprint::Free { w: 2, d: 2 }));
    assert!(b.access().is_some());
    let z = sim.world().resource::<ZoneGrid>();
    assert!(
        cells.iter().all(|&c| !z.is_occupied(c)),
        "the old cells are free"
    );
}

/// A T: a street along x at z = 3000 and one going north from its middle (3100, 3000..3200).
fn t_junction() -> SimRunner {
    let mut sim = town(&[3000.0]);
    let db = content();
    let road = db.road_id("basic_2l").unwrap();
    sim.world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            db.road(road),
            road,
            CubicBezier3::line(
                Vec3::new(3100.0, 0.0, 3000.0),
                Vec3::new(3100.0, 0.0, 3200.0),
            ),
            Elevation::Ground,
        )
        .unwrap();
    sim.tick_once();
    sim
}

/// The user's case: the very corner cell of a block at a T. A 1 x 1 free-standing prop and a
/// free-standing water tower stand there flush against both streets (they used to keep 1 m
/// clear, and an 8 m cell between two edges has no room for that), and so does a service at the
/// road edge.
#[test]
fn the_corner_cell_of_a_block_takes_free_standing_buildings_and_a_service() {
    let mut sim = t_junction();
    let db = content();
    // The corner cell north-east of the junction: edges at x = 3108 and z = 3008 (roads 2 cells
    // wide, T-275), 8 m cells.
    let corner = Vec2::new(3112.0, 3012.0);
    let w = sim.world();
    let (zones, net, buildings) = (
        w.resource::<ZoneGrid>(),
        w.resource::<NetGraph>(),
        w.resource::<BuildingGrid>(),
    );
    for name in ["prop_bench", "prop_statue"] {
        let def = db.building_id(name).unwrap();
        let plan = cp_building::validate_placement_for(
            &db,
            zones,
            net,
            buildings,
            None,
            None,
            def,
            corner,
            0.0,
            &[],
            &[],
        )
        .unwrap_or_else(|e| panic!("{name} in the corner: {e:?}"));
        assert!(matches!(plan.footprint, Footprint::Free { .. }), "{name}");
        // Where the user put it: the cell at the corner, not pushed away.
        assert!(plan.pos.distance(corner) < 4.5, "{name} at {:?}", plan.pos);
    }
    // The services at the road edge: the fire station (3 x 2) in the same corner.
    place(&mut sim, "fire_station", corner).expect("a service in the corner");
}

/// The user's scene: the coal power plant (3 x 3, a service at the road edge) with the cursor on
/// the very corner cell of a block at a T. It stands in the corner, its two sides flush against
/// the edges of both streets (it slides there from the cursor), not 1 m away and not refused.
#[test]
fn the_coal_plant_goes_flush_into_the_corner_of_a_block_at_a_t() {
    let mut sim = t_junction();
    let corner = Vec2::new(3112.0, 3012.0);
    place(&mut sim, "coal_power_plant", corner).expect("the plant in the corner");
    let g = sim.world().resource::<BuildingGrid>();
    let (_, b) = g.iter().next().unwrap();
    assert!(matches!(b.footprint, Footprint::Free { w: 3, d: 3 }));
    // Edges of the streets: x = 3100 + 8 and z = 3000 + 8; the plant is 24 m square in that corner.
    let q = b.footprint_polygon();
    let (lo, hi) = cp_zone::geom::quad_aabb(&q);
    assert!(
        (lo.x - 3108.0).abs() < 0.06 && (lo.y - 3008.0).abs() < 0.06,
        "{lo:?}"
    );
    assert!(
        (hi.x - 3132.0).abs() < 0.06 && (hi.y - 3032.0).abs() < 0.06,
        "{hi:?}"
    );
    assert!(b.access().is_some());
}

/// T-267 on whole-cell roads (T-275): a service dropped anywhere along a street slides onto the
/// street's cells: its sides stand on cell lines (here every 8 m from the street's start at
/// x = 3000), so the cells beside it stay whole; only the cells under it go.
#[test]
fn a_service_stands_on_the_cell_lines_of_its_street() {
    for x in [3093.3f32, 3101.0, 3110.7, 3127.9] {
        let mut sim = town(&[3000.0]);
        place(&mut sim, "fire_station", Vec2::new(x, 3030.0)).unwrap();
        let r = rects(&sim);
        let (lo, hi) = cp_zone::geom::quad_aabb(&r[0]);
        for side in [lo.x, hi.x] {
            let k = (side - 3000.0) / 8.0;
            assert!(
                (k - k.round()).abs() * 8.0 < 0.06,
                "cursor x {x}: a side at x = {side} is off the cell lines"
            );
        }
        // The cells on that side of the street: whole outside the building, none cut by it.
        sim.tick_once();
        let z = sim.world().resource::<ZoneGrid>();
        let cut = z
            .blocks()
            .flat_map(|(id, _)| z.block_cells(id))
            .filter_map(|c| z.cell_footprint(c))
            .filter(|o| {
                let (a, b) = cp_zone::geom::quad_aabb(&o.corners());
                let overlap_x = (b.x.min(hi.x) - a.x.max(lo.x)).max(0.0);
                let overlap_z = (b.y.min(hi.y) - a.y.max(lo.y)).max(0.0);
                overlap_x > 0.05 && overlap_x < 7.95 && overlap_z > 0.05
            })
            .count();
        assert_eq!(cut, 0, "cursor x {x}: {cut} cells cut by the building");
    }
}
