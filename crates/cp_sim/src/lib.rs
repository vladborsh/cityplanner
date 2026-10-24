//! `cp_sim` — simulation crate skeleton. See `architecture/system-design.md` §1.

pub mod checksum;
pub mod city_services;
pub mod command;
pub mod inspect;
pub mod new_game;
pub mod overlay;
pub mod phase;
pub mod plugin;
pub mod progress;
pub mod refund;
pub mod road_cases;
pub mod runner;
pub mod scenario;
pub mod stats;
pub mod tiles;
pub mod view;

pub use checksum::{Fnv1a, SimChecksum};
pub use command::{
    DebugCommand, SimCommand, buildings_under, dispatch_commands, init_command_resources,
};
pub use cp_traffic::{AgentFrame, PedestrianPose, Portal, VehiclePose};
pub use inspect::{
    BuildingInfo, CitizenInfo, GoodsInfo, Happiness, Inspection, SegmentInfo, Selection,
    ServiceInfo, TripInfo, Trouble, VehicleInfo, budget_row, buses_row, flag_names, key_parts,
};
pub use overlay::{OverlayData, ScalarGrid, ZoneCellView, ZoneOverlay};
pub use phase::SimPhase;
pub use plugin::{SimBuilder, SimPlugin};
pub use progress::{PROGRESS_PERIOD, Progress, kind_open};
pub use runner::{MAX_TICKS_PER_STEP, SimRunner, run_headless, run_headless_with};
pub use stats::CityStats;
pub use view::{
    AgentSnapshot, ArenaFill, ArenaGauges, FreightView, LineView, LoanOfferView, SimStats,
    SimTerrain, SimView, UniqueStatus,
};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_sim";

#[cfg(test)]
mod tests {
    #[test]
    fn skeleton_links() {
        assert_eq!(super::CRATE, "cp_sim");
    }
}
