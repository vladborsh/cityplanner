//! Player commands of this domain (`architecture/system-design.md` §4, level 2).
//! M1 variants are added by the domain tasks; new variants are only appended
//! (save stability rule).

use std::fmt;

use bevy_ecs::world::World;
use cp_core::Cost;
use serde::{Deserialize, Serialize};

/// Commands handled by this crate. No variants yet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EconomyCommand {}

/// Validation failure of a EconomyCommand; wrapped into `CommandError::Rejected` by the dispatcher.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EconomyError {}

impl fmt::Display for EconomyError {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {}
    }
}

impl std::error::Error for EconomyError {}

/// Validates and applies `cmd`, returning its cost.
pub fn apply(_world: &mut World, cmd: &EconomyCommand) -> Result<Cost, EconomyError> {
    match *cmd {}
}
