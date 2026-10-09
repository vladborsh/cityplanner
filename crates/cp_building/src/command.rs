//! Player commands of this domain (`architecture/system-design.md` §4, level 2).
//! M1 variants are added by the domain tasks; new variants are only appended
//! (save stability rule).

use std::fmt;

use bevy_ecs::world::World;
use cp_core::Cost;
use serde::{Deserialize, Serialize};

/// Commands handled by this crate. No variants yet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum BuildingCommand {}

/// Validation failure of a BuildingCommand; wrapped into `CommandError::Rejected` by the dispatcher.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildingError {}

impl fmt::Display for BuildingError {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {}
    }
}

impl std::error::Error for BuildingError {}

/// Validates and applies `cmd`, returning its cost.
pub fn apply(_world: &mut World, cmd: &BuildingCommand) -> Result<Cost, BuildingError> {
    match *cmd {}
}
