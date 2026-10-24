//! The daily market update (§21.1, T-320): each cargo's price level drifts back to normal with a
//! little noise, shortages and gluts start and end, the history ring moves on. Runs on the first
//! tick of every game day, before any trade of the day, with its own RNG stream.

use bevy_ecs::system::{Res, ResMut};
use cp_content::market::{MarketEvent, MarketEventKind, MarketFrozen, MarketKnobs};
use cp_content::{CargoKind, Market, SimContent};
use cp_core::time::TICKS_PER_DAY;
use cp_core::{SimRng, Tick};

/// RNG stream of the market.
pub const MARKET_STREAM: u64 = 0x3A2C_E700;

/// `Agents` phase, before the goods and the freight: on the first tick of a day, one step of
/// [`step_market`].
pub fn update_market(
    tick: Res<Tick>,
    rng: Res<SimRng>,
    content: Option<Res<SimContent>>,
    mut market: ResMut<Market>,
    frozen: Option<Res<MarketFrozen>>,
) {
    if frozen.is_some() || !tick.0.is_multiple_of(TICKS_PER_DAY) {
        return;
    }
    let mut rng = rng.fork(MARKET_STREAM, *tick);
    let day = (tick.0 / TICKS_PER_DAY) as u32;
    // Without content (a bare bench world) the market runs on the default knobs, so a world
    // with and one without content step alike.
    let fallback = MarketKnobs::default();
    let k = content
        .as_deref()
        .map_or(&fallback, |c| &c.0.balance().market);
    step_market(&mut market, k, &mut rng, day);
}

/// One day of the market at game day `day`, cargo by cargo in [`CargoKind::ALL`] order:
/// 1. the level moves `(100 - index) / revert_div` back to 100 plus a noise of
///    `-noise..=noise` (one draw per cargo), clamped;
/// 2. events that ran out end; then a cargo without one gets a shortage or a glut with
///    `event_chance` (one draw per cargo, always taken) while fewer than `events_max` run;
/// 3. the level is written into the history.
pub fn step_market(market: &mut Market, k: &MarketKnobs, rng: &mut SimRng, day: u32) {
    let span = (2 * k.noise.max(0) + 1) as u32;
    for cargo in CargoKind::ALL {
        let m = market.get_mut(cargo);
        let noise = rng.below(span) as i32 - k.noise.max(0);
        let index = i32::from(m.index);
        let next = index + (100 - index) / k.revert_div.max(1) + noise;
        m.index = next.clamp(i32::from(k.index_min), i32::from(k.index_max)) as u16;
    }
    for cargo in CargoKind::ALL {
        let m = market.get_mut(cargo);
        if m.event.is_some_and(|e| e.until_day <= day) {
            m.event = None;
        }
    }
    let mut running = market.cargo.iter().filter(|m| m.event.is_some()).count() as u32;
    for cargo in CargoKind::ALL {
        let roll = rng.next_f32();
        let m = market.get_mut(cargo);
        if m.event.is_some() || running >= k.events_max || roll >= k.event_chance {
            continue;
        }
        let kind = if rng.below(2) == 0 {
            MarketEventKind::Shortage
        } else {
            MarketEventKind::Glut
        };
        let (lo, hi) = k.event_days;
        let days = lo + rng.below(hi.saturating_sub(lo) + 1);
        m.event = Some(MarketEvent {
            kind,
            until_day: day + days,
        });
        running += 1;
    }
    market.record_day();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn year(seed: u64) -> (Market, Vec<[u16; 13]>, u32) {
        let k = MarketKnobs::default();
        let base = SimRng::seed_from_u64(seed);
        let mut market = Market::default();
        let mut days = Vec::new();
        let mut events = 0;
        for day in 0..365u32 {
            let mut rng = base.fork(MARKET_STREAM, Tick(u64::from(day) * TICKS_PER_DAY));
            let before = market.cargo.iter().filter(|m| m.event.is_some()).count();
            step_market(&mut market, &k, &mut rng, day);
            let running = market.cargo.iter().filter(|m| m.event.is_some()).count();
            assert!(
                running as u32 <= k.events_max,
                "day {day}: {running} events"
            );
            events += running.saturating_sub(before) as u32;
            days.push(market.cargo.map(|m| m.index));
        }
        (market, days, events)
    }

    /// §21.6 item 1: a year of days keeps every level in range, each cargo's mean near 100,
    /// the same seed gives the same history and another seed another one.
    #[test]
    fn a_year_of_drift_stays_in_range_around_normal_and_follows_the_seed() {
        let k = MarketKnobs::default();
        let (market, days, events) = year(7);
        for (i, cargo) in CargoKind::ALL.into_iter().enumerate() {
            let levels: Vec<u16> = days.iter().map(|d| d[i]).collect();
            assert!(
                levels
                    .iter()
                    .all(|&v| (k.index_min..=k.index_max).contains(&v)),
                "{cargo:?}"
            );
            let mean = levels.iter().map(|&v| f64::from(v)).sum::<f64>() / levels.len() as f64;
            assert!((90.0..=110.0).contains(&mean), "{cargo:?}: mean {mean}");
            assert_eq!(market.history_at(cargo, 0), levels[364] / 2 * 2);
        }
        assert!(events > 0, "a year has shortages and gluts");
        assert_eq!(year(7).1, days, "same seed, same prices");
        assert_ne!(year(8).1, days, "another seed, other prices");
    }
}
