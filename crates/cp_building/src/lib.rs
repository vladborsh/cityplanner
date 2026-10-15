//! `cp_building` — buildings: model, lots, spawning and construction. See
//! `architecture/system-design.md` §8.

pub mod command;
pub mod free;
pub mod grid;
pub mod level;
pub mod model;
pub mod spawn;

pub use command::{
    BuildingCommand, BuildingError, PlacementPlan, apply, oriented_lot, validate_placement,
};
pub use free::{ROAD_CLEARANCE_M, is_free_standing, sync_blockers, validate_free_placement};
pub use grid::{BuildingGrid, cells_under_lot, demolish_building, held_cells};
pub use level::{rebuild, rebuild_candidate};
pub use model::{
    AccessPoint, Building, BuildingFlags, BuildingId, BuildingState, FOOTPRINT_CELL_M, Footprint,
    Lot, build_ticks, footprint_quad,
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
}

#[cfg(test)]
mod free_tests;

#[cfg(test)]
mod treatment_tests;
