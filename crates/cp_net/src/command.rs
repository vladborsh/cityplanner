//! Player commands of this domain (`architecture/system-design.md` §4, level 2).
//! M1 variants are added by the domain tasks; new variants are only appended
//! (save stability rule).

use std::fmt;

use bevy_ecs::world::World;
use cp_core::Cost;
use serde::{Deserialize, Serialize};

/// Commands handled by this crate. No variants yet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum NetCommand {}

/// Validation failure of a NetCommand; wrapped into `CommandError::Rejected` by the dispatcher.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetError {}

impl fmt::Display for NetError {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {}
    }
}

impl std::error::Error for NetError {}

/// Validates and applies `cmd`, returning its cost.
pub fn apply(_world: &mut World, cmd: &NetCommand) -> Result<Cost, NetError> {
    match *cmd {}
}
