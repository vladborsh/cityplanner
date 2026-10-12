//! Player commands of this domain (`architecture/system-design.md` §4, level 2).
//! New variants are only appended (save stability rule).
//!
//! Both commands change a setting and cost nothing. A value outside the allowed range is an
//! error and changes nothing.

use std::fmt;

use bevy_ecs::world::World;
use cp_content::{ServiceKind, ZoneType};
use cp_core::Cost;
use serde::{Deserialize, Serialize};

use crate::state::{BUDGET_MAX, BUDGET_MIN, EconomyState, TAX_MAX, TAX_MIN};

/// Commands handled by this crate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EconomyCommand {
    /// Sets the tax rate of a zone type, [`TAX_MIN`]..=[`TAX_MAX`] percent.
    SetTaxRate { zone: ZoneType, pct: u8 },
    /// Sets the budget of a service kind, [`BUDGET_MIN`]..=[`BUDGET_MAX`] percent of its upkeep.
    SetBudget { service: ServiceKind, pct: u8 },
}

/// Validation failure of a EconomyCommand; wrapped into `CommandError::Rejected` by the dispatcher.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EconomyError {
    /// The world has no economy state.
    Missing,
    /// `ZoneType::None` pays no tax.
    NoTaxForZone,
    TaxOutOfRange(u8),
    BudgetOutOfRange(u8),
}

impl fmt::Display for EconomyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(f, "no economy in the sim world"),
            Self::NoTaxForZone => write!(f, "this zone type pays no tax"),
            Self::TaxOutOfRange(p) => {
                write!(f, "tax rate {p}% is outside {TAX_MIN}..={TAX_MAX}%")
            }
            Self::BudgetOutOfRange(p) => {
                write!(f, "budget {p}% is outside {BUDGET_MIN}..={BUDGET_MAX}%")
            }
        }
    }
}

impl std::error::Error for EconomyError {}

/// Validates and applies `cmd`, returning its cost (always zero).
pub fn apply(world: &mut World, cmd: &EconomyCommand) -> Result<Cost, EconomyError> {
    let mut state = world
        .get_resource_mut::<EconomyState>()
        .ok_or(EconomyError::Missing)?;
    match *cmd {
        EconomyCommand::SetTaxRate { zone, pct } => {
            if !(TAX_MIN..=TAX_MAX).contains(&pct) {
                return Err(EconomyError::TaxOutOfRange(pct));
            }
            if !state.taxes.set(zone, pct) {
                return Err(EconomyError::NoTaxForZone);
            }
        }
        EconomyCommand::SetBudget { service, pct } => {
            if !(BUDGET_MIN..=BUDGET_MAX).contains(&pct) {
                return Err(EconomyError::BudgetOutOfRange(pct));
            }
            state.budgets.set(service, pct);
        }
    }
    Ok(Cost::ZERO)
}
