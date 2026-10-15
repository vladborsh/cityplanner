//! `cp_building` — buildings: model, lots, spawning and construction. See
//! `architecture/system-design.md` §8.

pub mod command;
pub mod grid;
pub mod level;
pub mod model;
pub mod spawn;

pub use command::{
    BuildingCommand, BuildingError, PlacementPlan, apply, oriented_lot, validate_placement,
};
pub use grid::{BuildingGrid, cells_under_lot, demolish_building, held_cells};
pub use level::{rebuild, rebuild_candidate};
pub use model::{
    AccessPoint, Building, BuildingFlags, BuildingId, BuildingState, FOOTPRINT_CELL_M, Footprint,
    Lot, build_ticks,
};
pub use spawn::{BLOCK_BUCKETS, DEMAND_PERIOD, Demand, SPAWN_STREAM, place, update_buildings};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_building";

#[cfg(test)]
mod tests {
    use cp_content::{ContentDb, ZoneType};

    #[test]
    fn skeleton_links() {
        assert_eq!(super::CRATE, "cp_building");
    }

    /// The M1 content set of the spec: levels per zone, two variants per level, lots 1x1..4x4.
    #[test]
    fn m1_content_covers_every_zone_level_with_two_variants() {
        let db = ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
            .unwrap();
        let spec = [
            (ZoneType::ResLow, 5),
            (ZoneType::ResHigh, 5),
            (ZoneType::ComLow, 3),
            (ZoneType::ComHigh, 3),
            (ZoneType::Industrial, 3),
            (ZoneType::Office, 3),
        ];
        for (zone, levels) in spec {
            for level in 1..=levels {
                let n = db
                    .buildings()
                    .filter(|(_, d)| d.zone == zone && d.level == level && d.service.is_none())
                    .count();
                assert!(n >= 2, "{zone:?} level {level}: {n} variants");
            }
            assert!(
                db.buildings().all(|(_, d)| d.zone != zone
                    || (1..=levels).contains(&d.level)
                    || d.service.is_some()),
                "{zone:?} has a level outside 1..={levels}"
            );
        }
        let lots: std::collections::BTreeSet<_> = db.buildings().map(|(_, d)| d.lot).collect();
        assert!(lots.contains(&(1, 1)) && lots.contains(&(4, 4)), "{lots:?}");
        // Level 1 of every zone has at least one small and one big-ish lot to choose from.
        for (zone, _) in spec {
            let sizes: std::collections::BTreeSet<_> = db
                .buildings()
                .filter(|(_, d)| d.zone == zone && d.level == 1 && d.service.is_none())
                .map(|(_, d)| d.lot)
                .collect();
            assert!(sizes.len() >= 2, "{zone:?} level 1 lots {sizes:?}");
        }
    }

    /// T-106: a free-standing type (`road_access: false`) is refused by `Place` with a plain
    /// error until T-107 gives it a placement rule; a type that needs a road is not.
    #[test]
    fn placing_a_free_standing_type_is_refused_with_a_clear_error() {
        use crate::{BuildingError, validate_placement};
        use bevy_math::Vec2;
        let tower = |road_access: &str| {
            ContentDb::from_sources(vec![
                (
                    "content/buildings/tower.ron".to_string(),
                    r#"Building((id: "tower", name: "Tower", zone: Industrial, level: 1,
                    lot: (1, 1), jobs: (0, 0, 0, 0), consumption: {}, tax_base: 0.0,
                    cost: 1.0, upkeep_week: 1.0,
                    style: (floors: 1, roof: Flat, palette: "utility_grey"),
                    service: Some("tower_service")))"#
                        .to_string(),
                ),
                (
                    "content/services/tower.ron".to_string(),
                    format!(
                        r#"Service((id: "tower_service", name: "Tower", kind: Water,
                        radius_m: 100.0, capacity: 10, {road_access} vehicles: []))"#
                    ),
                ),
            ])
            .unwrap()
        };
        let place = |db: &ContentDb| {
            validate_placement(
                db,
                &cp_zone::ZoneGrid::default(),
                &cp_net::NetGraph::default(),
                None,
                None,
                db.building_id("tower").unwrap(),
                Vec2::new(100.0, 100.0),
                0.0,
            )
        };
        let free = place(&tower("road_access: false,"));
        assert_eq!(free, Err(BuildingError::FreePlacementUnsupported));
        assert!(
            BuildingError::FreePlacementUnsupported
                .to_string()
                .contains("free-standing")
        );
        // A type that needs a road gets as far as looking for one.
        assert_eq!(place(&tower("")), Err(BuildingError::NoRoad));
    }
}
