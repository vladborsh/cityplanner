//! Format 48 (T-224): leisure. The v47 fixture (made by the real v47 writer, a grid city with a
//! city-park area and one attraction) loads with empty leisure targets, nobody in the park and
//! an empty fee ledger, its park state kept; visitors and walkers survive a save and a load.

use std::path::Path;

use cp_building::BuildingId;
use cp_content::ContentDb;
use cp_core::{Cost, Tick};
use cp_park::{Visit, Walker};
use cp_save::*;
use cp_traffic::{Access, Side};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v47_fixture_loads_with_nobody_in_the_parks_and_visitors_round_trip() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v47-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 47, "made by the v47 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let mut l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    assert!(l.data.leisure.is_empty());
    assert!(l.data.park_visits.is_empty());
    assert!(l.data.park_ledger.is_empty());
    assert!(
        l.data.parks.areas.values().any(|a| a.attract > 0.0),
        "the park state of v47 is kept: {:?}",
        l.data.parks
    );

    l.data.park_visits.inside.push(Visit {
        area: 1,
        leave_at: Tick(2000),
    });
    l.data.park_visits.walkers.insert(
        7,
        Walker {
            area: 1,
            gate: BuildingId::new(3, 0),
            far: Access {
                segment: cp_core::Id::new(2, 0),
                s: 12.5,
                side: Side::Left,
            },
            back: true,
        },
    );
    l.data.park_ledger.fees = Cost::from_units(35.0);
    l.data.leisure.list.push(cp_citizen::LeisureTarget {
        building: BuildingId::new(3, 0),
        pos: bevy_math::Vec2::new(10.0, 20.0),
        weight: 4.5,
    });
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
    assert_eq!(l2.data.park_visits.inside.len(), 1);
}
