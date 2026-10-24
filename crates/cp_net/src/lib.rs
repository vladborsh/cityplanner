//! `cp_net` — simulation crate skeleton. See `architecture/system-design.md` §1.

pub mod build;
pub mod command;
pub mod connections;
pub mod control;
pub mod graph;
pub mod junction;
pub mod kerb;
pub mod model;
pub mod plan;
pub mod reach;
pub mod run;
pub mod stretches;

pub use build::{
    AppliedSnap, CONTROL_SPACING, ClearanceWarning, DemolishRef, ELEVATED_CLEARANCE, ElevationMode,
    GROUND_PIECE_H, LEVEL_TOL, MAX_TUNNEL_DEPTH, NodeRef, OneWayWarning, PIER_SPACING,
    PlannedSegment, PlannedSplit, RoadBuildError, RoadBuildPlan, RoadPath, RoadPathRequest,
    SAME_DEPTH_TOL, SnapKind, SnapOptions, SnapReport, SnapReportKind, TUNNEL_CLEARANCE, ZONE_CELL,
    clearance_report, in_junction, is_plain_cut, junction_reach, one_way_report,
    out_of_end_footprint, pier_positions, segment_cost, validate_build_plan, validate_road_path,
    validate_road_path_report,
};
pub use command::{
    NetCommand, NetError, PART_SNAP, apply, apply_build_plan, build_unowned, command_strips,
    curve_outside_owned, part_range, plan_applied, plan_build, price_per_m, removal_value,
};
pub use connections::{LaneConnection, Links, PedLink, PedLinkKind, Turn, turn_of_arms};
pub use control::{ControlPlan, LightPlan, Phase, Priority, Signal};
pub use graph::{
    MAX_LEN, MIN_LEN, MIN_SEGMENT_LENGTH, NODE_REUSE_EPS, NetGraph, NetOpError, SplitResult,
    Violation, trees_allowed,
};
pub use junction::{
    Arm, ArmPath, FLAT_GAP_DEG, Junction, JunctionGeometry, MIDDLE_DEG, NodeClass, SideLine,
    TRANSITION_LEN, compute_junction, refresh_geometry, side_line, update_junctions,
};
pub use model::*;
pub use plan::{Anchor, BuildPlan, PlanPiece};
pub use reach::{
    EntryReach, OutsideEnds, RailReach, is_rail_segment, node_is_rail, outside_ways,
    update_entry_reach, update_rail_reach,
};
pub use run::road_run;
pub use stretches::{
    CORNER_DEG, Stretch, StretchPoint, control_points, locate, span, stops, stretch, unlocate,
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
