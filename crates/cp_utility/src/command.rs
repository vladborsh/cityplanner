//! Player commands of this domain (`architecture/system-design.md` §4, level 2).
//! New variants are only appended (save stability rule).
//!
//! `BuildLink` plans with [`plan_link_path`] and applies the plan only if it passed; an
//! `Err` leaves the world untouched. Reads: [`UtilityGraph`], [`SimContent`],
//! [`TileOwnership`] (all required), and optionally [`SimTerrain`] (heights), the
//! [`BuildingGrid`] and [`ZoneGrid`] (power lines avoid buildings). Money is not deducted
//! here (budget task); the cost is returned.

use std::fmt;

use bevy_ecs::world::World;
use bevy_math::Vec2;
use cp_building::BuildingGrid;
use cp_content::{SimContent, UtilityKind};
use cp_core::Cost;
use cp_terrain::{SimTerrain, TileOwnership};
use cp_zone::ZoneGrid;
use serde::{Deserialize, Serialize};

use crate::graph::{GraphError, UtilityGraph, UtilityLinkId};
use crate::plan::{PathProblem, PlanInputs, apply_plan, plan_link_path};

/// Commands handled by this crate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum UtilityCommand {
    /// Builds a pipe (`Water`, `Sewage`: one shared network) or power line (`Power`) along
    /// `path` (straight legs between the points, world XZ).
    BuildLink { kind: UtilityKind, path: Vec<Vec2> },
    /// Removes one link (nodes left without links go with it).
    RemoveLink { link: UtilityLinkId },
}

/// Validation failure of a UtilityCommand; wrapped into `CommandError::Rejected` by the dispatcher.
#[derive(Clone, Debug, PartialEq)]
pub enum UtilityError {
    /// A resource the command needs is not in the world.
    Missing(&'static str),
    /// The content has no definition for this kind.
    NoDefinition(UtilityKind),
    /// `BuildLink` failed validation; every problem found.
    Invalid(Vec<PathProblem>),
    UnknownLink(UtilityLinkId),
    Graph(GraphError),
}

impl fmt::Display for UtilityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(what) => write!(f, "no {what} in the sim world"),
            Self::NoDefinition(k) => write!(f, "no utility definition for {k:?}"),
            Self::Invalid(ps) => {
                write!(f, "link not buildable: ")?;
                for (i, p) in ps.iter().enumerate() {
                    if i > 0 {
                        write!(f, "; ")?;
                    }
                    write!(f, "{p:?}")?;
                }
                Ok(())
            }
            Self::UnknownLink(l) => write!(f, "unknown utility link {l:?}"),
            Self::Graph(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for UtilityError {}

impl From<GraphError> for UtilityError {
    fn from(e: GraphError) -> Self {
        Self::Graph(e)
    }
}

/// Validates and applies `cmd`, returning its cost.
pub fn apply(world: &mut World, cmd: &UtilityCommand) -> Result<Cost, UtilityError> {
    if !world.contains_resource::<UtilityGraph>() {
        return Err(UtilityError::Missing("utility graph"));
    }
    match cmd {
        UtilityCommand::BuildLink { kind, path } => {
            let content = world
                .get_resource::<SimContent>()
                .map(|c| c.0.clone())
                .ok_or(UtilityError::Missing("content"))?;
            let (_, def) = content
                .utility_for_kind(*kind)
                .ok_or(UtilityError::NoDefinition(*kind))?;
            let tiles = world
                .get_resource::<TileOwnership>()
                .copied()
                .ok_or(UtilityError::Missing("tile ownership"))?;
            let terrain = world.get_resource::<SimTerrain>().map(|t| t.0.clone());
            let def = def.clone();
            world.resource_scope(
                |world, mut graph: bevy_ecs::change_detection::Mut<UtilityGraph>| {
                    let buildings = world
                        .get_resource::<BuildingGrid>()
                        .zip(world.get_resource::<ZoneGrid>());
                    let view = PlanInputs {
                        tiles: &tiles,
                        terrain: terrain.as_deref(),
                        buildings,
                    };
                    let plan =
                        plan_link_path(&graph, &view, &def, path).map_err(UtilityError::Invalid)?;
                    let backup = graph.clone();
                    match apply_plan(&mut graph, &plan) {
                        Ok(()) => Ok(plan.cost),
                        Err(e) => {
                            *graph = backup;
                            Err(e.into())
                        }
                    }
                },
            )
        }
        UtilityCommand::RemoveLink { link } => {
            let mut graph = world.resource_mut::<UtilityGraph>();
            if graph.link(*link).is_none() {
                return Err(UtilityError::UnknownLink(*link));
            }
            graph.remove_link(*link)?;
            Ok(Cost::ZERO)
        }
    }
}
