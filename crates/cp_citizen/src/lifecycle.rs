//! Lifecycle (§16 M2 D "Жизненный цикл"): ageing, death and births, once per game day.
//!
//! At `tick % TICKS_PER_DAY == 0` ([`update_lifecycle`], `Citizens` phase):
//! * every citizen ages one day (arena order);
//! * a citizen may die: by age (`Balance::death_curve`, chance per day) or, sick for longer than
//!   `sick_death_days`, with `sick_death_chance`. The dead leave their household and the arena
//!   (job and school are freed), `CitizenGrid::dead[home]` grows by one — the deathcare row
//!   collects them — and a household left empty is removed;
//! * a household of two adults aged 20–40 with a free place and arrived at its home has a child
//!   with chance `birth_chance`: age 0, education 0.
//!
//! RNG: `SimRng::fork(LIFECYCLE_STREAM, tick)`; all numbers are `// tune later`.

use bevy_ecs::prelude::*;
use cp_content::{Balance, SimContent};
use cp_core::time::TICKS_PER_DAY;
use cp_core::{SimLimits, SimRng, Tick};

use crate::grid::CitizenGrid;
use crate::model::{CitizenId, DAYS_PER_YEAR, HouseholdId};

pub const LIFECYCLE_STREAM: u64 = 0x11FE;

/// Age range (years) of the parents of a birth.
const PARENT_YEARS: std::ops::Range<u32> = 20..40;

/// Chance per day to die at `age_days`: linear between the `death_curve` points, 0 before the
/// first, flat after the last.
fn death_chance(curve: &[(f32, f32)], age_days: u32) -> f32 {
    let years = age_days as f32 / DAYS_PER_YEAR as f32;
    let Some(&(first, _)) = curve.first() else {
        return 0.0;
    };
    if years < first {
        return 0.0;
    }
    for w in curve.windows(2) {
        let ((a0, c0), (a1, c1)) = (w[0], w[1]);
        if years < a1 {
            let t = if a1 > a0 {
                (years - a0) / (a1 - a0)
            } else {
                1.0
            };
            return c0 + (c1 - c0) * t;
        }
    }
    curve.last().map_or(0.0, |&(_, c)| c)
}

/// One game day of ageing, deaths and births at tick `now`.
pub fn run_lifecycle(
    grid: &mut CitizenGrid,
    bal: &Balance,
    rng: &SimRng,
    now: Tick,
    limits: &SimLimits,
) {
    grid.ensure_derived();
    let mut rng = rng.fork(LIFECYCLE_STREAM, now);
    let ids: Vec<CitizenId> = grid.citizens().map(|(id, _)| id).collect();
    let sick_after = (bal.sick_death_days * TICKS_PER_DAY as f32).round() as u64;
    for id in ids {
        let Some(c) = grid.citizen_mut(id) else {
            continue;
        };
        c.age_days += 1;
        let age = c.age_days;
        let sick_long = c
            .sick_since
            .is_some_and(|s| now.0.saturating_sub(s.0) >= sick_after);
        let home = c.home;
        let mut p = death_chance(&bal.death_curve, age);
        if sick_long {
            p = p.max(bal.sick_death_chance);
        }
        if p > 0.0 && rng.next_f32() < p {
            grid.remove_citizen(id);
            if let Some(h) = home {
                *grid.dead.entry(h).or_insert(0) += 1;
            }
        }
    }

    let parents: Vec<HouseholdId> = grid
        .households()
        .filter(|(_, h)| now >= h.arrive_at && h.member_ids().count() < crate::HOUSEHOLD_MAX)
        .filter(|(_, h)| {
            // Two adults aged 20-40 (more adults, or older ones, do not stop a birth).
            h.member_ids()
                .filter_map(|cid| grid.citizen(cid))
                .filter(|c| PARENT_YEARS.contains(&(c.age_days / DAYS_PER_YEAR)))
                .count()
                >= 2
        })
        .map(|(id, _)| id)
        .collect();
    for hid in parents {
        if grid.citizen_count() >= limits.citizens as usize {
            break;
        }
        if rng.next_f32() < bal.birth_chance {
            grid.add_newborn(hid);
        }
    }
}

/// `Citizens` phase: runs [`run_lifecycle`] on the first tick of each game day.
pub fn update_lifecycle(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    rng: Res<SimRng>,
    limits: Res<SimLimits>,
    mut grid: ResMut<CitizenGrid>,
) {
    let Some(content) = content else { return };
    if !tick.0.is_multiple_of(TICKS_PER_DAY) {
        return;
    }
    run_lifecycle(&mut grid, content.0.balance(), &rng, *tick, &limits);
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_building::BuildingId;

    fn setup(members: &[(u32, u8, bool)]) -> (CitizenGrid, BuildingId) {
        let mut g = CitizenGrid::new();
        g.ensure_derived();
        let home = BuildingId::new(1, 0);
        g.add_household(home, 0, Tick(0), members);
        (g, home)
    }

    fn days(g: &mut CitizenGrid, n: u64, seed: u64) {
        let bal = Balance::default();
        let rng = SimRng::from_seed([seed as u8; 32]);
        let limits = SimLimits::desktop();
        for d in 1..=n {
            run_lifecycle(g, &bal, &rng, Tick(d * TICKS_PER_DAY), &limits);
        }
    }

    #[test]
    fn everyone_ages_a_day_per_day() {
        let (mut g, _) = setup(&[(30 * DAYS_PER_YEAR, 1, true), (5 * DAYS_PER_YEAR, 0, false)]);
        days(&mut g, 28, 1);
        let ages: Vec<u32> = g.citizens().map(|(_, c)| c.age_days).collect();
        assert!(ages.contains(&(30 * DAYS_PER_YEAR + 28)));
        assert!(ages.contains(&(5 * DAYS_PER_YEAR + 28)));
    }

    #[test]
    fn the_very_old_die_and_wait_at_home_for_the_hearse() {
        let (mut g, home) = setup(&[(100 * DAYS_PER_YEAR, 1, false)]);
        days(&mut g, 60, 2);
        assert_eq!(g.citizen_count(), 0);
        assert_eq!(g.household_count(), 0, "an empty household goes");
        assert_eq!(g.dead.get(&home), Some(&1));
    }

    #[test]
    fn a_young_couple_has_a_child_and_runs_replay_the_same() {
        let adults = [(25 * DAYS_PER_YEAR, 1, true), (26 * DAYS_PER_YEAR, 1, true)];
        let (mut a, _) = setup(&adults);
        days(&mut a, 400, 3);
        assert!(
            a.citizens().any(|(_, c)| c.age_days < 400),
            "a child is born"
        );
        let (mut b, _) = setup(&adults);
        days(&mut b, 400, 3);
        assert_eq!(a, b);
    }

    /// T-179: the flow counters tell who came, was born, left and died.
    #[test]
    fn the_flow_counts_arrivals_births_departures_and_deaths() {
        let (mut g, home) = setup(&[(30 * 365, 0, false), (30 * 365, 0, false)]);
        assert_eq!(g.flow.arrived, 2);
        let hid = g.households().next().map(|(id, _)| id).unwrap();
        g.add_newborn(hid).unwrap();
        assert_eq!(g.flow.born, 1);
        let victim = g.citizens().next().map(|(id, _)| id).unwrap();
        g.remove_citizen(victim);
        assert_eq!(g.flow.died, 1);
        g.remove_household(hid);
        assert_eq!(g.flow.left, 2, "the two who were left");
        let _ = home;
    }
}
