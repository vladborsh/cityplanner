//! The city treasury (`architecture/system-design.md` §11). It lives in `cp_core` because the
//! command paths of the domains below `cp_economy` (roads, buildings, utilities) have to
//! check and charge it. Everything is integer cents ([`Cost`]).

use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::command::{CommandError, Cost};

/// Balance of a new city: 70 000.00.
pub const START_BALANCE: Cost = Cost(7_000_000);
/// Lowest balance a purchase may leave behind: −20 000.00. Weekly upkeep is not limited by it.
pub const DEBT_FLOOR: Cost = Cost(-2_000_000);

/// A purchase the treasury cannot cover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InsufficientFunds {
    pub needed: Cost,
    /// How much could still be spent: balance minus [`DEBT_FLOOR`].
    pub available: Cost,
}

impl From<InsufficientFunds> for CommandError {
    fn from(e: InsufficientFunds) -> Self {
        CommandError::InsufficientFunds {
            needed: e.needed,
            available: e.available,
        }
    }
}

/// The city's money. Serialized sim state.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Treasury {
    pub balance: Cost,
}

impl Default for Treasury {
    fn default() -> Self {
        Self {
            balance: START_BALANCE,
        }
    }
}

impl Treasury {
    /// Whether spending `cost` keeps the balance at or above [`DEBT_FLOOR`]. A free or
    /// negative (refund) cost is always fine, even in deep debt.
    pub fn check(&self, cost: Cost) -> Result<(), InsufficientFunds> {
        if cost.0 <= 0 || self.balance.0 - cost.0 >= DEBT_FLOOR.0 {
            Ok(())
        } else {
            Err(InsufficientFunds {
                needed: cost,
                available: Cost(self.balance.0 - DEBT_FLOOR.0),
            })
        }
    }

    /// Spends `cost` after [`Self::check`] passed.
    pub fn charge(&mut self, cost: Cost) {
        self.balance.0 -= cost.0;
    }
}

/// [`Treasury::check`] on the world's treasury. A world without one (a bare domain test)
/// spends freely.
pub fn check_funds(world: &World, cost: Cost) -> Result<(), InsufficientFunds> {
    world
        .get_resource::<Treasury>()
        .map_or(Ok(()), |t| t.check(cost))
}

/// Spends `cost` from the world's treasury, if it has one. Call it after the change it pays
/// for succeeded and [`check_funds`] passed before it.
pub fn charge_funds(world: &mut World, cost: Cost) {
    if let Some(mut t) = world.get_resource_mut::<Treasury>() {
        t.charge(cost);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_is_inclusive_and_free_things_always_pass() {
        let mut t = Treasury::default();
        assert_eq!(t.balance, Cost(7_000_000));
        // 70 000 − 20 000 floor: exactly 90 000.00 can be spent.
        assert!(t.check(Cost(9_000_000)).is_ok());
        let e = t.check(Cost(9_000_001)).unwrap_err();
        assert_eq!(e.available, Cost(9_000_000));
        t.charge(Cost(9_000_000));
        assert_eq!(t.balance, DEBT_FLOOR);
        assert!(t.check(Cost(1)).is_err());
        assert!(t.check(Cost::ZERO).is_ok());
        t.balance = Cost(-5_000_000);
        assert!(t.check(Cost::ZERO).is_ok());
        assert!(t.check(Cost(-100)).is_ok());
    }
}
