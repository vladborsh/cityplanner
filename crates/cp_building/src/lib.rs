//! `cp_building` — buildings: model, lots, spawning and construction. See
//! `architecture/system-design.md` §8.

pub mod command;
pub mod free;
pub mod grid;
pub mod industry;
pub mod level;
pub mod model;
pub mod park;
pub mod park_site;
pub mod respec;
pub mod spawn;

pub use command::{
    BuildingCommand, BuildingError, Clearance, MOVE_COST_FRACTION, Placed, PlacementPlan, apply,
    apply_placed, is_growable, oriented_lot, platform_request, platform_request_behind,
    platform_shift, validate_placement, validate_placement_except, validate_placement_over,
};
pub use free::{
    ROAD_CLEARANCE_M, is_free_standing, sync_blockers, validate_free_placement,
    validate_free_placement_except, validate_free_placement_ignoring,
};
pub use grid::{
    BuildingGrid, burn, cells_under_lot, demolish_building, held_cells, zone_mismatches,
};
pub use industry::{
    IndustryWorld, footprint_points, lot_richness, plan_points, validate_industry_site,
};
pub use level::{rebuild, rebuild_candidate};
pub use model::{
    AccessPoint, Building, BuildingFlags, BuildingId, BuildingState, FOOTPRINT_CELL_M, Footprint,
    Lot, build_ticks, footprint_quad,
};
pub use park::{
    GREEN_MOVE_IN_BONUS, GREEN_SICK_CUT, ParkSite, green_at, move_in_passes, park_sites,
    sick_chance,
};
pub use park_site::{ParkWorld, validate_park_site};
pub use respec::{SPEC_STREAM, respec};
pub use spawn::{
    BLOCK_BUCKETS, DEMAND_PERIOD, Demand, SPAWN_STREAM, pick_def, place, spawnable,
    update_buildings,
};

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
