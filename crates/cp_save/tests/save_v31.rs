//! Format 31 (T-159): riders, line figures and fares. The v30 fixture (made by the real v30
//! writer) loads without them and round-trips on the new version.

use std::path::Path;

use cp_content::ContentDb;
use cp_save::*;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v30_fixture_loads_without_riders_and_round_trips_on_the_new_version() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v30-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 30, "made by the v30 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    assert!(l.data.transit.riders.is_empty() && l.data.transit.stats.is_empty());
    assert!(l.data.ledger.is_empty());
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
}

mod mid_trip {
    use std::sync::Arc;

    use cp_building::{AccessPoint, BuildingGrid, BuildingState};
    use cp_citizen::{CitizenGrid, Location, Trip, TripBook, TripMode, TripPurpose, TripState};
    use cp_core::{SimLimits, Tick};
    use cp_net::NetGraph;
    use cp_sim::{SimCommand, SimRunner};
    use cp_terrain::Terrain;
    use cp_traffic::Side;
    use cp_transit::{StopRef, TransitCommand, TransitMode, TransitNet};

    use super::*;

    fn runner(content: &Arc<ContentDb>) -> SimRunner {
        let terrain =
            Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
        let mut sim = SimRunner::with_limits(1, SimLimits::desktop());
        sim.set_terrain(terrain);
        sim.set_content(Arc::clone(content));
        sim
    }

    #[test]
    fn a_save_with_riders_on_the_way_loads_to_the_same_checksum_and_goes_on_identically() {
        let content =
            Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
        let hash = content.content_hash();
        let bytes =
            std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v30-basic.sav")).unwrap();
        let mut a = runner(&content);
        load_into_runner(&mut a, &bytes, hash).unwrap();
        a.world_mut()
            .insert_resource(cp_citizen::RoutineSettings { decide: false });
        // An 800 m square of streets off to the side of the fixture city: a depot and a home
        // on the first side, a workplace on the third, a line over all four.
        let road = content.road_id("basic_2l").unwrap();
        let def = content.road(road).clone();
        let corners = [
            (-1000.0, -1000.0),
            (-200.0, -1000.0),
            (-200.0, -200.0),
            (-1000.0, -200.0),
            (-1000.0, -1000.0),
        ];
        let sides: Vec<cp_net::SegmentId> = corners
            .windows(2)
            .map(|w| {
                a.world_mut()
                    .resource_mut::<NetGraph>()
                    .add_segment(
                        &def,
                        road,
                        cp_core::curve::CubicBezier3::line(
                            bevy_math::Vec3::new(w[0].0, 0.0, w[0].1),
                            bevy_math::Vec3::new(w[1].0, 0.0, w[1].1),
                        ),
                        cp_net::Elevation::Ground,
                    )
                    .unwrap()
            })
            .collect();
        let (s1, s2) = (sides[0], sides[2]);
        let (mid1, mid2) = (
            bevy_math::Vec2::new(-600.0, -1000.0),
            bevy_math::Vec2::new(-600.0, -200.0),
        );
        let mut template = a
            .world()
            .resource::<BuildingGrid>()
            .iter()
            .next()
            .unwrap()
            .1
            .clone();
        template.state = BuildingState::Active;
        let put = |a: &mut SimRunner, def: &str, pos, seg| {
            let mut b = template.clone();
            b.def = content.building_id(def).unwrap();
            b.pos = pos;
            b.set_access(Some(AccessPoint {
                segment: seg,
                t: 0.5,
            }));
            a.world_mut().resource_mut::<BuildingGrid>().insert(b)
        };
        put(&mut a, "bus_depot", mid1, s1);
        let home = put(&mut a, "house_low_1", mid1, s1);
        let work = put(&mut a, "com_low_1", mid2, s2);
        a.tick_once();
        let stops = [s1, s2]
            .iter()
            .map(|&seg| StopRef::NewRoad {
                seg,
                s: a.world()
                    .resource::<NetGraph>()
                    .segment(seg)
                    .unwrap()
                    .length
                    / 2.0
                    + 20.0,
                side: Side::Right,
            })
            .collect();
        a.push_command(
            SimCommand::Transit(TransitCommand::CreateLine {
                mode: TransitMode::Bus,
                stops,
            }),
            Tick(a.tick().0 + 1),
        );
        for _ in 0..150 {
            a.tick_once();
        }
        {
            let w = a.world_mut();
            let mut g = w.resource_mut::<CitizenGrid>();
            g.ensure_derived();
            g.add_household(home, 0, Tick(0), &[(30 * 365, 0, false); 4]);
            let ids: Vec<_> = g.citizens().map(|(i, _)| i).collect();
            for id in &ids {
                g.set_location(*id, Location::Home);
            }
            let homes: Vec<_> = ids
                .into_iter()
                .filter(|&i| g.citizen(i).unwrap().home == Some(home))
                .collect();
            let mut book = w.resource_mut::<TripBook>();
            book.ensure_derived();
            for citizen in homes {
                book.add(Trip {
                    citizen,
                    from: home,
                    to: work,
                    purpose: TripPurpose::Work,
                    mode: TripMode::Walk,
                    state: TripState::Scheduled { depart_at: Tick(0) },
                });
            }
        }
        for _ in 0..60 {
            a.tick_once();
        }
        let riders = a.world().resource::<TransitNet>().riders.len();
        assert!(riders > 0, "riders on the way at the save");
        let bytes = save_runner(&a, hash, "flatlands").unwrap();
        let mut b = runner(&content);
        load_into_runner(&mut b, &bytes, hash).unwrap();
        b.world_mut()
            .insert_resource(cp_citizen::RoutineSettings { decide: false });
        assert_eq!(
            a.world().resource::<TransitNet>(),
            b.world().resource::<TransitNet>()
        );
        assert_eq!(a.checksum(), b.checksum());
        for _ in 0..300 {
            a.tick_once();
            b.tick_once();
        }
        assert_eq!(a.checksum(), b.checksum());
    }
}
