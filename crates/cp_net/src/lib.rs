//! `cp_net` — simulation crate skeleton. See `architecture/system-design.md` §1.

pub mod build;
pub mod command;
pub mod connections;
pub mod control;
pub mod graph;
pub mod junction;
pub mod model;
pub mod run;

pub use build::{
    AppliedSnap, DemolishRef, ElevationMode, NodeRef, OneWayWarning, PIER_SPACING, PlannedSegment,
    PlannedSplit, RoadBuildError, RoadBuildPlan, RoadPath, RoadPathRequest, SnapKind, SnapOptions,
    SnapReport, SnapReportKind, one_way_report, pier_positions, segment_cost, validate_road_path,
    validate_road_path_report,
};
pub use command::{NetCommand, NetError, apply};
pub use connections::{LaneConnection, Links, PedLink, PedLinkKind, Turn};
pub use control::{ControlPlan, LightPlan, Phase, Priority, Signal};
pub use graph::{MIN_SEGMENT_LENGTH, NODE_REUSE_EPS, NetGraph, NetOpError, SplitResult};
pub use junction::{
    Arm, Junction, JunctionGeometry, compute_junction, refresh_geometry, update_junctions,
};
pub use model::*;
pub use run::road_run;

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_net";

#[cfg(test)]
mod tests {
    #[test]
    fn skeleton_links() {
        assert_eq!(super::CRATE, "cp_net");
    }
}
