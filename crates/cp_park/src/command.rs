//! Commands handled by this crate (T-225). Variants are only appended.

use std::fmt;

use bevy_ecs::world::World;
use cp_content::SimContent;
use cp_core::Cost;
use cp_district::{AreaId, AreaKind, Areas};
use serde::{Deserialize, Serialize};

use crate::ParkState;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ParkCommand {
    /// Sets the entrance fee of a park area, `0..=park_fee_max`.
    SetFee { area: AreaId, fee: f32 },
}

#[derive(Clone, Debug, PartialEq)]
pub enum ParkError {
    /// The world has no content or areas.
    Missing,
    NoSuchArea,
    NotAParkArea,
    /// The fee is outside `0..=max`.
    FeeRange {
        max: f32,
    },
}

impl fmt::Display for ParkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(f, "no content in the sim world"),
            Self::NoSuchArea => write!(f, "no such area"),
            Self::NotAParkArea => write!(f, "this area is not a park"),
            Self::FeeRange { max } => write!(f, "the fee is between 0 and {max}"),
        }
    }
}

impl std::error::Error for ParkError {}

/// Validates and applies `cmd`; every command is free.
pub fn apply(world: &mut World, cmd: &ParkCommand) -> Result<Cost, ParkError> {
    let ParkCommand::SetFee { area, fee } = cmd;
    let max = world
        .get_resource::<SimContent>()
        .map(|c| c.0.balance().park_fee_max)
        .ok_or(ParkError::Missing)?;
    let kind = world
        .get_resource::<Areas>()
        .ok_or(ParkError::Missing)?
        .get(*area)
        .map(|a| a.kind)
        .ok_or(ParkError::NoSuchArea)?;
    if !matches!(kind, AreaKind::Park(_)) {
        return Err(ParkError::NotAParkArea);
    }
    if !fee.is_finite() || *fee < 0.0 || *fee > max {
        return Err(ParkError::FeeRange { max });
    }
    let mut state = world
        .get_resource_mut::<ParkState>()
        .ok_or(ParkError::Missing)?;
    // The state of a new area is made by the next count; the fee waits there.
    state.areas.entry(area.0).or_default().fee = *fee;
    Ok(Cost::ZERO)
}
