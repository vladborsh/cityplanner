//! Player commands of this domain (`architecture/system-design.md` §4, level 2).
//! New variants are only appended (save stability rule).
//!
//! The setting commands cost nothing; the loan commands move money (T-129). A value outside the allowed range is an
//! error and changes nothing.

use std::fmt;

use bevy_ecs::world::World;
use cp_citizen::CitizenGrid;
use cp_content::{ServiceKind, SimContent, ZoneType};
use cp_core::{Cost, Tick, Treasury};
use serde::{Deserialize, Serialize};

use crate::state::{BUDGET_MAX, BUDGET_MIN, EconomyState, Loan, TAX_MAX, TAX_MIN};

/// Commands handled by this crate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EconomyCommand {
    /// Sets the tax rate of a zone type, [`TAX_MIN`]..=[`TAX_MAX`] percent.
    SetTaxRate { zone: ZoneType, pct: u8 },
    /// Sets the budget of a service kind, [`BUDGET_MIN`]..=[`BUDGET_MAX`] percent of its upkeep.
    SetBudget { service: ServiceKind, pct: u8 },
    /// Takes the loan `Balance::loan_offers[offer]`: the amount goes to the treasury. Needs the
    /// offer's population and a free slot (T-129).
    TakeLoan { offer: u8 },
    /// Pays the principal left of the loan on `offer` at once; the interest left is forgiven.
    RepayLoan { offer: u8 },
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
    /// No such loan offer.
    UnknownOffer(u8),
    /// The city is smaller than the offer wants.
    TooSmall {
        needed: u32,
        have: u32,
    },
    /// The offer is already taken.
    AlreadyTaken(u8),
    /// No active loan on this offer.
    NoSuchLoan(u8),
    /// The treasury is below the principal left.
    NoMoney {
        needed: Cost,
        available: Cost,
    },
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
            Self::UnknownOffer(o) => write!(f, "no loan offer {o}"),
            Self::TooSmall { needed, have } => {
                write!(f, "the bank wants {needed} residents, the city has {have}")
            }
            Self::AlreadyTaken(o) => write!(f, "loan offer {o} is already taken"),
            Self::NoSuchLoan(o) => write!(f, "no active loan on offer {o}"),
            Self::NoMoney { needed, available } => write!(
                f,
                "paying off needs {:.2}, the treasury has {:.2}",
                needed.0 as f64 / 100.0,
                available.0 as f64 / 100.0
            ),
        }
    }
}

impl std::error::Error for EconomyError {}

/// Validates and applies `cmd`, returning what it charged: zero, except that paying a loan
/// off charges its principal left.
pub fn apply(world: &mut World, cmd: &EconomyCommand) -> Result<Cost, EconomyError> {
    match *cmd {
        EconomyCommand::TakeLoan { offer } => return take_loan(world, offer),
        EconomyCommand::RepayLoan { offer } => return repay_loan(world, offer),
        _ => {}
    }
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
        EconomyCommand::TakeLoan { .. } | EconomyCommand::RepayLoan { .. } => {}
    }
    Ok(Cost::ZERO)
}

fn take_loan(world: &mut World, offer: u8) -> Result<Cost, EconomyError> {
    let o = world
        .get_resource::<SimContent>()
        .and_then(|c| c.0.balance().loan_offers.get(usize::from(offer)).cloned())
        .ok_or(EconomyError::UnknownOffer(offer))?;
    let have = world
        .get_resource::<CitizenGrid>()
        .map_or(0, |g| g.citizen_count() as u32);
    if have < o.min_population {
        return Err(EconomyError::TooSmall {
            needed: o.min_population,
            have,
        });
    }
    let tick = world.get_resource::<Tick>().copied().unwrap_or_default();
    let loan = Loan::new(offer, &o, tick);
    let mut state = world
        .get_resource_mut::<EconomyState>()
        .ok_or(EconomyError::Missing)?;
    if state.loan(offer).is_some() {
        return Err(EconomyError::AlreadyTaken(offer));
    }
    let at = state.loans.partition_point(|l| l.offer < offer);
    state.loans.insert(at, loan);
    if let Some(mut t) = world.get_resource_mut::<Treasury>() {
        t.balance += loan.principal_left;
    }
    Ok(Cost::ZERO)
}

fn repay_loan(world: &mut World, offer: u8) -> Result<Cost, EconomyError> {
    let state = world
        .get_resource::<EconomyState>()
        .ok_or(EconomyError::Missing)?;
    let due = state
        .loan(offer)
        .ok_or(EconomyError::NoSuchLoan(offer))?
        .principal_left;
    if let Some(t) = world.get_resource::<Treasury>()
        && t.balance < due
    {
        return Err(EconomyError::NoMoney {
            needed: due,
            available: t.balance,
        });
    }
    if let Some(mut t) = world.get_resource_mut::<Treasury>() {
        t.balance = t.balance - due;
    }
    world
        .resource_mut::<EconomyState>()
        .loans
        .retain(|l| l.offer != offer);
    Ok(due)
}
