//! T-221: the city parks are service buildings (ServiceKind::Park) that stand ready at once, lift
//! the land value around them inside their radius and pollute nothing.

use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::{Vec2, Vec3};
use cp_building::{BuildingCommand, BuildingGrid, BuildingState};
use cp_content::{ContentDb, ServiceKind};
use cp_core::curve::CubicBezier3;
use cp_core::{CommandError, CommandOutcome};
use cp_economy::LandGrids;
use cp_net::{Elevation, NetGraph};
use cp_sim::{Progress, SimCommand, SimRunner};
use cp_terrain::TileOwnership;
use cp_zone::{ZoneCellRef, ZoneGrid};

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

fn world(content: &Arc<ContentDb>) -> SimRunner {
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    let mut sim = SimRunner::new(1);
    sim.set_content(Arc::clone(content));
    sim.world_mut().insert_resource(TileOwnership::only((1, 1)));
    sim.world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            &def,
            road,
            CubicBezier3::line(
                Vec3::new(3000.0, 0.0, 3000.0),
                Vec3::new(3200.0, 0.0, 3000.0),
            ),
            Elevation::Ground,
        )
        .unwrap();
    sim.tick_once();
    sim.world_mut().insert_resource(Progress::ALL);
    sim.world_mut().resource_mut::<cp_core::Treasury>().balance =
        cp_core::Cost::from_units(50_000_000.0);
    sim
}

fn spot(sim: &SimRunner, col: u16) -> Vec2 {
    let zones = sim.world().resource::<ZoneGrid>();
    let (id, _) = zones.blocks().next().unwrap();
    zones
        .cell_footprint(ZoneCellRef {
            block: id,
            cell: col * 4 + 1,
        })
        .unwrap()
        .c
}

fn place(sim: &mut SimRunner, name: &str, col: u16) -> Result<(), CommandError> {
    let def = sim
        .world()
        .resource::<cp_content::SimContent>()
        .0
        .building_id(name)
        .unwrap();
    let pos = spot(sim, col);
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
    out.last().unwrap().result.clone().map(|_| ())
}

#[test]
fn the_four_parks_are_park_service_buildings_with_a_land_effect() {
    let c = content();
    for name in ["small_park", "playground", "plaza_fountain", "sports_field"] {
        let d = c.building(c.building_id(name).unwrap());
        let svc = c.service(c.service_id(d.service.as_deref().unwrap()).unwrap());
        assert_eq!(svc.kind, ServiceKind::Park, "{name}");
        assert!(
            d.land_effect
                .is_some_and(|e| e.add > 0.0 && e.radius_m > 0.0),
            "{name}"
        );
    }
}

#[test]
fn a_park_stands_ready_lifts_the_land_value_inside_its_radius_only_and_does_not_pollute() {
    let c = content();
    let run = |with: bool| {
        let mut sim = world(&c);
        if with {
            place(&mut sim, "small_park", 2).unwrap();
            let g = sim.world().resource::<BuildingGrid>();
            assert!(g.iter().all(|(_, b)| b.state == BuildingState::Active));
        }
        for _ in 0..300 {
            sim.tick_once();
        }
        let p = spot(&sim, 2);
        let g = sim.world().resource::<LandGrids>();
        (
            g.value_at(p),
            g.value_at(p + Vec2::new(0.0, 600.0)),
            g.pollution_at(p),
        )
    };
    let (near_with, far_with, poll_with) = run(true);
    let (near_without, far_without, poll_without) = run(false);
    assert!(near_with > near_without, "{near_with} vs {near_without}");
    assert_eq!(far_with, far_without);
    assert_eq!(poll_with, poll_without);
}

/// T-237: the parks differ clearly and their effect is real: land value, fewer sick (the
/// move-in follows from the leisure factor of contentment since T-318b).
#[test]
fn the_four_parks_differ_and_help_their_neighbours() {
    use cp_building::{ParkSite, green_at, sick_chance};
    let db = content();
    let get = |id: &str| {
        db.building(db.building_id(id).unwrap())
            .land_effect
            .unwrap_or_else(|| panic!("{id}"))
    };
    let (small, play, plaza, sports) = (
        get("small_park"),
        get("playground"),
        get("plaza_fountain"),
        get("sports_field"),
    );
    // Dearer parks lift the land more or reach further; the plaza is the land value one, the
    // sports field the biggest reach, the playground the wellbeing one, the small park the cheap one.
    assert!(plaza.add > sports.add && sports.add > play.add && play.add > small.add);
    assert!(sports.radius_m > plaza.radius_m && plaza.radius_m > play.radius_m);
    assert!(play.green > small.green && sports.green > plaza.green);
    // Land value: +12 and more where it used to be +4..8, so a home beside a plaza climbs a level
    // threshold (30 -> 45 -> 55 -> 65) a park-less one does not.
    assert!(small.add >= 12.0);

    // Wellbeing falls off with the distance and adds up from several parks, to 1.
    let park = |p: Vec2, e: cp_content::LandEffect| ParkSite {
        pos: p,
        radius_m: e.radius_m,
        green: e.green,
    };
    let here = Vec2::new(100.0, 100.0);
    let one = [park(here, play)];
    assert!((green_at(&one, here) - play.green).abs() < 1e-6);
    assert!(green_at(&one, here + Vec2::new(60.0, 0.0)) < green_at(&one, here));
    assert_eq!(green_at(&one, here + Vec2::new(500.0, 0.0)), 0.0);
    let many = [park(here, play), park(here, sports), park(here, small)];
    assert_eq!(green_at(&many, here), 1.0);

    // Fewer fall ill beside a park.
    let (base, poll) = (0.003, 0.002);
    assert!(sick_chance(base, poll, 0.8) < 0.6 * sick_chance(base, poll, 0.0));
}

/// T-237: a home beside a plaza stands on land worth enough for a higher level than the same
/// home far from it (growth takes the level from `level_for(level_res_land, land value)`).
#[test]
fn a_house_beside_a_plaza_is_allowed_a_higher_level_than_one_far_away() {
    let c = content();
    let run = |with: bool| {
        let mut sim = world(&c);
        if with {
            place(&mut sim, "plaza_fountain", 2).unwrap();
        }
        for _ in 0..300 {
            sim.tick_once();
        }
        let p = spot(&sim, 2) + Vec2::new(0.0, 40.0);
        sim.world().resource::<LandGrids>().value_at(p)
    };
    let (beside, without) = (run(true), run(false));
    let thresholds = c.balance().level_res_land;
    let (up, base) = (
        cp_economy::growth::level_for(&thresholds, beside),
        cp_economy::growth::level_for(&thresholds, without),
    );
    assert!(beside >= without + 10.0, "{beside} against {without}");
    assert!(
        up > base,
        "level {up} against {base} (land {beside} against {without})"
    );
}
