//! Service budgets, 50 to 150 % per service kind (§20, T-319): the percentages ([`Budgets`],
//! a sim resource read by the utilities, the services, transit and the parks) and the one place
//! that turns a percentage into factors. The cost is linear; the capacity gains less above 100 %
//! than it loses below it, and the reach follows the capacity more slowly.

use bevy_ecs::resource::Resource;
use serde::{Deserialize, Serialize};

use crate::{Balance, ServiceDef, ServiceKind};

pub const BUDGET_MIN: u8 = 50;
pub const BUDGET_MAX: u8 = 150;
pub const BUDGET_DEFAULT: u8 = 100;
/// Number of service kinds (`ServiceKind` variants); new kinds are appended.
pub const SERVICE_KINDS: usize = 16;

/// Service kinds with a budget slider, in budget-table order (§20.1). The others (landmarks,
/// industry, cargo) run at 100 %.
pub const BUDGETED: [ServiceKind; 13] = [
    ServiceKind::Water,
    ServiceKind::Sewage,
    ServiceKind::Power,
    ServiceKind::Roads,
    ServiceKind::Garbage,
    ServiceKind::Health,
    ServiceKind::Deathcare,
    ServiceKind::Fire,
    ServiceKind::Police,
    ServiceKind::Education,
    ServiceKind::Post,
    ServiceKind::Transit,
    ServiceKind::Park,
];

/// Whether `kind` has a budget slider.
pub fn is_budgeted(kind: ServiceKind) -> bool {
    BUDGETED.contains(&kind)
}

/// Budget percentage per service kind (by `ServiceKind` discriminant). Serialized sim state.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budgets {
    pct: [u8; SERVICE_KINDS],
}

impl Default for Budgets {
    fn default() -> Self {
        Self {
            pct: [BUDGET_DEFAULT; SERVICE_KINDS],
        }
    }
}

impl Budgets {
    /// The percentage of `kind`; a kind without a slider is at [`BUDGET_DEFAULT`].
    pub fn get(&self, kind: ServiceKind) -> u8 {
        if !is_budgeted(kind) {
            return BUDGET_DEFAULT;
        }
        self.pct
            .get(kind as usize)
            .copied()
            .unwrap_or(BUDGET_DEFAULT)
    }

    /// Sets the percentage of `kind`; a kind without a slider ignores it. The range is the
    /// caller's to check (the command does).
    pub fn set(&mut self, kind: ServiceKind, pct: u8) {
        if is_budgeted(kind)
            && let Some(p) = self.pct.get_mut(kind as usize)
        {
            *p = pct;
        }
    }

    /// The daily capacity of a service building at its kind's budget ([`budgeted`]).
    pub fn capacity(&self, svc: &ServiceDef, bal: &Balance) -> u32 {
        budgeted(svc.capacity, self.get(svc.kind), bal)
    }

    /// The vans of a post office or sorting facility (its `storage`) at the Post budget; any
    /// other building's storage is ground, not money, and stays.
    pub fn storage(&self, svc: &ServiceDef, bal: &Balance) -> u32 {
        if svc.kind == ServiceKind::Post {
            budgeted(svc.storage, self.get(svc.kind), bal)
        } else {
            svc.storage
        }
    }

    /// `svc` as its kind's budget runs it: [`Self::capacity`], [`Self::storage`] and
    /// [`Self::radius`] in place of the definition's.
    pub fn def(&self, svc: &ServiceDef, bal: &Balance) -> ServiceDef {
        ServiceDef {
            capacity: self.capacity(svc, bal),
            storage: self.storage(svc, bal),
            radius_m: self.radius(svc, bal),
            ..svc.clone()
        }
    }

    /// The coverage radius of a service building at its kind's budget, metres.
    pub fn radius(&self, svc: &ServiceDef, bal: &Balance) -> f32 {
        if svc.radius_m <= 0.0 {
            return svc.radius_m;
        }
        svc.radius_m * reach_factor(self.get(svc.kind), bal)
    }
}

/// Upkeep multiplier of a budget percentage: linear, `pct / 100`.
pub fn cost_factor(pct: u8) -> f64 {
    f64::from(pct) / 100.0
}

/// Capacity multiplier: `pct / 100` up to 100 %, then `1 + (pct − 100) / 100 × over_gain`
/// (`budget_over_gain` 0.5: 150 % → ×1.25). Exactly 1 at 100 %.
pub fn power_factor(pct: u8, bal: &Balance) -> f32 {
    let p = f32::from(pct) / 100.0;
    if pct <= BUDGET_DEFAULT {
        p
    } else {
        1.0 + (p - 1.0) * bal.budget_over_gain
    }
}

/// Reach (radius) multiplier: the capacity factor to `budget_reach_exp` (0.5: 50 % → ×0.71,
/// 150 % → ×1.12). Exactly 1 at 100 %.
pub fn reach_factor(pct: u8, bal: &Balance) -> f32 {
    if pct == BUDGET_DEFAULT {
        return 1.0;
    }
    power_factor(pct, bal).powf(bal.budget_reach_exp)
}

/// A whole capacity `n` at budget `pct`: `max(1, round(n × power_factor))`, halves to even
/// (§20.6: a fire station of 9 at 50 % keeps 4), so a kind with a building never drops to
/// nothing; `0` (no capacity, or unlimited storage) stays `0`. Exactly `n` at 100 %.
pub fn budgeted(n: u32, pct: u8, bal: &Balance) -> u32 {
    if n == 0 || pct == BUDGET_DEFAULT {
        return n;
    }
    ((n as f64 * f64::from(power_factor(pct, bal))).round_ties_even() as u32).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_factors_follow_the_curves_and_are_exact_at_100() {
        let bal = Balance::default();
        assert_eq!(power_factor(100, &bal), 1.0);
        assert_eq!(reach_factor(100, &bal), 1.0);
        assert_eq!(cost_factor(100), 1.0);
        assert!((power_factor(150, &bal) - 1.25).abs() < 1e-6);
        assert!((power_factor(75, &bal) - 0.75).abs() < 1e-6);
        assert!((reach_factor(50, &bal) - 0.5f32.sqrt()).abs() < 1e-4);
        assert!((reach_factor(150, &bal) - 1.25f32.sqrt()).abs() < 1e-4);
        assert!((cost_factor(150) - 1.5).abs() < 1e-12);
        for n in [0, 1, 7, 180, 1000, 123_457] {
            assert_eq!(budgeted(n, 100, &bal), n);
        }
        assert_eq!(budgeted(1000, 50, &bal), 500);
        assert_eq!(budgeted(1000, 150, &bal), 1250);
        assert_eq!(budgeted(9, 50, &bal), 4);
        assert_eq!(budgeted(1, 50, &bal), 1);
        assert_eq!(budgeted(0, 50, &bal), 0);
    }

    #[test]
    fn only_kinds_with_a_slider_keep_a_percentage() {
        let mut b = Budgets::default();
        b.set(ServiceKind::Health, 60);
        b.set(ServiceKind::Landmark, 60);
        assert_eq!(b.get(ServiceKind::Health), 60);
        assert_eq!(b.get(ServiceKind::Landmark), 100);
        assert_eq!(b.get(ServiceKind::Water), 100);
    }
}
