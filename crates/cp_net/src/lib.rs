//! `cp_net` — simulation crate skeleton. See `architecture/system-design.md` §1.

pub mod build;
pub mod command;
pub mod graph;
pub mod junction;
pub mod model;

pub use build::{
    AppliedSnap, DemolishRef, ElevationMode, NodeRef, PIER_SPACING, PlannedSegment, PlannedSplit,
    RoadBuildError, RoadBuildPlan, RoadPath, RoadPathRequest, SnapKind, SnapOptions,
    pier_positions, segment_cost, validate_road_path,
};
pub use command::{NetCommand, NetError, apply};
pub use graph::{MIN_SEGMENT_LENGTH, NODE_REUSE_EPS, NetGraph, NetOpError, SplitResult};
pub use junction::{Arm, Junction, JunctionGeometry, compute_junction, update_junctions};
pub use model::*;

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_net";

#[cfg(test)]
mod tests {
    #[test]
    fn skeleton_links() {
        assert_eq!(super::CRATE, "cp_net");
    }
}
