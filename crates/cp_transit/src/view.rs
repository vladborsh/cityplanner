//! What clients read of transit (T-157): the stops and lines plus the route of every line as a
//! ground-plane polyline. Built by the runner for `SimView`; the tool's ghost builds a
//! [`LineRoute`] with the same function from a plan of its own.

use std::collections::BTreeMap;
use std::sync::Arc;

use bevy_math::Vec2;
use cp_traffic::PathGraph;

use crate::model::{LineId, PALETTE_LEN, TransitNet};
use crate::plan::{LinePlan, LinePlans};

/// Metres between the points of a route polyline.
pub const ROUTE_STEP: f32 = 4.0;

/// Colours of the line palette (`Line::color` indexes it), sRGB. Distinct from each other and
/// from the road class colours.
pub const LINE_PALETTE_RGB: [[u8; 3]; PALETTE_LEN as usize] = [
    [230, 57, 70],
    [29, 131, 230],
    [46, 175, 90],
    [244, 162, 40],
    [155, 89, 210],
    [0, 188, 188],
    [233, 91, 160],
    [140, 190, 30],
    [120, 80, 50],
    [60, 70, 200],
    [255, 214, 10],
    [120, 130, 140],
];

/// The colour a new line gets: the palette in turn, as `CreateLine` assigns it.
pub fn next_line_color(net: &TransitNet) -> u8 {
    ((net.lines.len()) % usize::from(PALETTE_LEN)) as u8
}

/// The route of one line, as drawn.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineRoute {
    /// The ring as one polyline, leg after leg (a broken leg is left out).
    pub points: Vec<Vec2>,
    pub length_m: f32,
    pub cycle_s: f32,
    /// Every leg has a path.
    pub ok: bool,
    /// Height of each of `points` for a route on the track (metro, T-164); empty for a route
    /// on the street. The ground view leaves such a route out: it shows in the Underground view.
    pub heights: Vec<f32>,
}

impl LineRoute {
    pub fn of_plan(graph: &PathGraph, plan: &LinePlan) -> Self {
        let mut points = Vec::new();
        for leg in &plan.legs {
            points.extend(graph.car_path_points(&leg.path, ROUTE_STEP));
        }
        Self {
            points,
            length_m: plan.length_m,
            cycle_s: plan.cycle_s,
            ok: plan.is_ok(),
            heights: Vec::new(),
        }
    }

    /// The route of a plan over the track graph: the way the trains of a metro line ride.
    pub fn of_rail_plan(graph: &PathGraph, plan: &LinePlan) -> Self {
        let mut points = Vec::new();
        let mut heights = Vec::new();
        for leg in &plan.legs {
            for p in graph.rail_path_points3(&leg.path, ROUTE_STEP) {
                points.push(Vec2::new(p.x, p.z));
                heights.push(p.y);
            }
        }
        Self {
            points,
            length_m: plan.length_m,
            cycle_s: plan.cycle_s,
            ok: plan.is_ok(),
            heights,
        }
    }

    /// Whether the route runs on the track (under the ground).
    pub fn underground(&self) -> bool {
        !self.heights.is_empty()
    }
}

/// Snapshot of transit for the clients.
#[derive(Clone, Debug, Default)]
pub struct TransitView {
    pub net: TransitNet,
    pub routes: BTreeMap<LineId, LineRoute>,
    /// Identifies the plans the routes were made from (see [`LinePlans::fingerprint`]).
    pub plans_key: u64,
    /// The lane graph, for the tool's ghost (`plan_line` + [`LineRoute::of_plan`]).
    pub graph: Arc<PathGraph>,
}

impl TransitView {
    pub fn build(net: &TransitNet, plans: &LinePlans, graph: &Arc<PathGraph>) -> Self {
        Self {
            net: net.clone(),
            routes: plans
                .iter()
                .map(|(id, p)| {
                    let metro = net
                        .lines
                        .get(id)
                        .is_some_and(|l| l.mode == crate::TransitMode::Metro);
                    (
                        id,
                        if metro {
                            LineRoute::of_rail_plan(graph, p)
                        } else {
                            LineRoute::of_plan(graph, p)
                        },
                    )
                })
                .collect(),
            plans_key: plans.fingerprint(),
            graph: Arc::clone(graph),
        }
    }
}
