//! `cp_net` — simulation crate skeleton. See `architecture/system-design.md` §1.

pub mod build;
pub mod command;
pub mod connections;
pub mod control;
pub mod graph;
pub mod junction;
pub mod model;
pub mod plan;
pub mod reach;
pub mod run;
pub mod stretches;

pub use build::{
    AppliedSnap, CONTROL_LINE_TOL, CONTROL_SPACING, ClearanceWarning, ControlMark, DemolishRef,
    ElevationMode, MAX_TUNNEL_DEPTH, NodeRef, OneWayWarning, PIER_SPACING, PlannedSegment,
    PlannedSplit, RoadBuildError, RoadBuildPlan, RoadPath, RoadPathRequest, SAME_DEPTH_TOL,
    SnapKind, SnapOptions, SnapReport, SnapReportKind, StepContext, TUNNEL_CLEARANCE, ZONE_CELL,
    clearance_report, in_junction, junction_reach, one_way_report, pier_positions, quantize_path,
    segment_cost, validate_build_plan, validate_road_path, validate_road_path_report,
};
pub use command::{
    NetCommand, NetError, PART_SNAP, apply, apply_build_plan, build_unowned, curve_outside_owned,
    plan_applied, plan_build,
};
pub use connections::{LaneConnection, Links, PedLink, PedLinkKind, Turn};
pub use control::{ControlPlan, LightPlan, Phase, Priority, Signal};
pub use graph::{
    MAX_LEN, MIN_LEN, MIN_SEGMENT_LENGTH, NODE_REUSE_EPS, NetGraph, NetOpError, SplitResult,
    Violation,
};
pub use junction::{
    Arm, Junction, JunctionGeometry, compute_junction, refresh_geometry, update_junctions,
};
pub use model::*;
pub use plan::{Anchor, BuildPlan, PlanPiece};
pub use reach::{
    EntryReach, RailReach, is_rail_segment, node_is_rail, update_entry_reach, update_rail_reach,
};
pub use run::road_run;
pub use stretches::{
    CORNER_DEG, Stretch, StretchPoint, cell_ticks, control_points, locate, span, stops, stretch,
    unlocate,
};

/// Crate name, used by the skeleton smoke test.
pub const CRATE: &str = "cp_net";

#[cfg(test)]
mod tests {
    #[test]
    fn skeleton_links() {
        assert_eq!(super::CRATE, "cp_net");
    }
}
