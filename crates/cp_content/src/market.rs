//! The intercity market (§21, T-320): a price level per cargo that drifts day by day, shortages
//! and gluts, and the seasons. Every price the sim pays or is paid goes through
//! [`Market::export_price`] / [`Market::import_price`]; `cp_industry::market` updates it once a
//! day. A resource of its own (like `Budgets`, §20) because `cp_transit`, `cp_industry` and the UI
//! read it.

use bevy_ecs::resource::Resource;
use cp_core::Tick;
use cp_core::time::{Season, season};
use serde::{Deserialize, Serialize};

use crate::{CargoKind, ContentDb};

/// Days of price history kept per cargo (the UI's sparkline).
pub const MARKET_HISTORY: usize = 30;
/// The neutral price level, percent.
pub const MARKET_NORMAL: u16 = 100;

/// A shortage (the world wants it: exports pay more, imports cost more) or a glut.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarketEventKind {
    Shortage,
    Glut,
}

/// A shortage or glut of one cargo until the start of game day `until_day`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketEvent {
    pub kind: MarketEventKind,
    pub until_day: u32,
}

/// One cargo's market: its price level and event, and the level of the last days.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CargoMarket {
    /// Price level, percent of the content price.
    pub index: u16,
    pub event: Option<MarketEvent>,
    /// `index / 2` of the last [`MARKET_HISTORY`] days, a ring written at [`Market::cursor`].
    pub history: [u8; MARKET_HISTORY],
}

impl Default for CargoMarket {
    fn default() -> Self {
        Self {
            index: MARKET_NORMAL,
            event: None,
            history: [(MARKET_NORMAL / 2) as u8; MARKET_HISTORY],
        }
    }
}

/// The market of every cargo, in [`CargoKind`] order. Serialized sim state.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Market {
    pub cargo: [CargoMarket; 13],
    /// The history slot the next day writes.
    pub cursor: u8,
}

/// A test hook (§21.6): while this resource is in the world the daily update leaves the market
/// as it is, so a test can set a level or an event and see it priced. Not saved.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MarketFrozen;

/// The market knobs of `balance.ron` (`market`, §21.1). // tune later
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct MarketKnobs {
    /// The price level stays within `index_min..=index_max` percent.
    pub index_min: u16,
    pub index_max: u16,
    /// Each day the level moves `(100 - index) / revert_div` back to normal, plus a whole noise
    /// of `-noise..=noise`.
    pub revert_div: i32,
    pub noise: i32,
    /// The daily chance of a cargo without an event to get one, its length in days (uniform)
    /// and the most events at once.
    pub event_chance: f32,
    pub event_days: (u32, u32),
    pub events_max: u32,
    /// Price multipliers of a shortage and a glut.
    pub shortage_export_mul: f32,
    pub glut_export_mul: f32,
    pub shortage_import_mul: f32,
    pub glut_import_mul: f32,
    /// The share of import orders that fail during a shortage (§21.1 item 3).
    pub shortage_fail: f32,
    /// The seasonal price multiplier of a cargo, spring to winter; cargo not listed: 1.
    pub season: Vec<(CargoKind, [f32; 4])>,
}

impl Default for MarketKnobs {
    fn default() -> Self {
        Self {
            index_min: 60,
            index_max: 160,
            revert_div: 16,
            noise: 4,
            event_chance: 1.0 / 120.0,
            event_days: (7, 14),
            events_max: 2,
            shortage_export_mul: 1.5,
            glut_export_mul: 0.5,
            shortage_import_mul: 2.0,
            glut_import_mul: 0.8,
            shortage_fail: 0.5,
            season: vec![
                (CargoKind::Crops, [1.0, 0.9, 0.8, 1.2]),
                (CargoKind::AnimalProducts, [1.0, 0.9, 0.8, 1.2]),
                (CargoKind::Oil, [1.0, 0.9, 1.0, 1.2]),
                (CargoKind::Petroleum, [1.0, 0.9, 1.0, 1.2]),
            ],
        }
    }
}

impl MarketKnobs {
    /// The plain numbers, in declaration order: validation and the content hash.
    pub fn knobs(&self) -> Vec<f32> {
        let mut v = vec![
            f32::from(self.index_min),
            f32::from(self.index_max),
            self.revert_div as f32,
            self.noise as f32,
            self.event_chance,
            self.event_days.0 as f32,
            self.event_days.1 as f32,
            self.events_max as f32,
            self.shortage_export_mul,
            self.glut_export_mul,
            self.shortage_import_mul,
            self.glut_import_mul,
            self.shortage_fail,
        ];
        for (c, m) in &self.season {
            v.push(*c as u8 as f32);
            v.extend(m);
        }
        v
    }

    /// The seasonal multiplier of `cargo` in `s`.
    pub fn season_mul(&self, cargo: CargoKind, s: Season) -> f32 {
        self.season
            .iter()
            .find(|(c, _)| *c == cargo)
            .map_or(1.0, |(_, m)| m[s as usize])
    }
}

impl Market {
    pub fn get(&self, cargo: CargoKind) -> &CargoMarket {
        &self.cargo[cargo as usize]
    }

    pub fn get_mut(&mut self, cargo: CargoKind) -> &mut CargoMarket {
        &mut self.cargo[cargo as usize]
    }

    /// The factor on the content export price of `cargo` at `tick`: level × season × event.
    pub fn export_factor(&self, k: &MarketKnobs, cargo: CargoKind, tick: Tick) -> f32 {
        let m = self.get(cargo);
        let event = match m.event.map(|e| e.kind) {
            Some(MarketEventKind::Shortage) => k.shortage_export_mul,
            Some(MarketEventKind::Glut) => k.glut_export_mul,
            None => 1.0,
        };
        f32::from(m.index) / 100.0 * k.season_mul(cargo, season(tick)) * event
    }

    /// The factor on the content import price of `cargo` at `tick`.
    pub fn import_factor(&self, k: &MarketKnobs, cargo: CargoKind, tick: Tick) -> f32 {
        let m = self.get(cargo);
        let event = match m.event.map(|e| e.kind) {
            Some(MarketEventKind::Shortage) => k.shortage_import_mul,
            Some(MarketEventKind::Glut) => k.glut_import_mul,
            None => 1.0,
        };
        f32::from(m.index) / 100.0 * k.season_mul(cargo, season(tick)) * event
    }

    /// Whether the import order of `cargo` that building `who` (its index) places on game day
    /// `day` fails (§21.1 item 3): only during a shortage, for `shortage_fail` of the orders. A
    /// hash of the sim seed and the keys, no RNG draw, like `cp_transit::trade_roll`: the same
    /// in any order, and a building that failed retries on its next pass (the same all day).
    pub fn import_fails(
        &self,
        k: &MarketKnobs,
        seed: [u8; 32],
        who: u32,
        cargo: CargoKind,
        day: u64,
    ) -> bool {
        self.get(cargo).event.map(|e| e.kind) == Some(MarketEventKind::Shortage)
            && cp_core::hash_unit(seed, u64::from(who) | 1 << 40, (day << 8) | cargo as u64)
                < k.shortage_fail
    }

    /// Money one unit of `cargo` sold to the outside brings at `tick` (0 for a cargo without a
    /// definition).
    pub fn export_price(&self, db: &ContentDb, cargo: CargoKind, tick: Tick) -> f32 {
        db.cargo(cargo).map_or(0.0, |c| {
            c.export_price * self.export_factor(&db.balance().market, cargo, tick)
        })
    }

    /// Money one unit of `cargo` bought from the outside costs at `tick`.
    pub fn import_price(&self, db: &ContentDb, cargo: CargoKind, tick: Tick) -> f32 {
        db.cargo(cargo).map_or(0.0, |c| {
            c.import_price * self.import_factor(&db.balance().market, cargo, tick)
        })
    }

    /// The level of `cargo` `days_ago` days back (0 = the last day written), percent.
    pub fn history_at(&self, cargo: CargoKind, days_ago: usize) -> u16 {
        let i = (usize::from(self.cursor) + MARKET_HISTORY * 2 - 1 - days_ago % MARKET_HISTORY)
            % MARKET_HISTORY;
        u16::from(self.get(cargo).history[i]) * 2
    }

    /// Writes today's level of every cargo into the history and moves the cursor.
    pub fn record_day(&mut self) {
        let at = usize::from(self.cursor);
        for m in &mut self.cargo {
            m.history[at] = (m.index / 2).min(255) as u8;
        }
        self.cursor = ((at + 1) % MARKET_HISTORY) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_core::time::TICKS_PER_DAY;

    #[test]
    fn at_the_normal_level_without_events_the_factor_is_the_season() {
        let k = MarketKnobs::default();
        let mut m = Market::default();
        let spring = Tick(0);
        let autumn = Tick(TICKS_PER_DAY * 14);
        assert_eq!(m.export_factor(&k, CargoKind::Logs, spring), 1.0);
        assert_eq!(m.export_factor(&k, CargoKind::Crops, autumn), 0.8);
        m.get_mut(CargoKind::Logs).index = 150;
        assert_eq!(m.export_factor(&k, CargoKind::Logs, spring), 1.5);
        m.get_mut(CargoKind::Logs).event = Some(MarketEvent {
            kind: MarketEventKind::Shortage,
            until_day: 9,
        });
        assert_eq!(m.export_factor(&k, CargoKind::Logs, spring), 2.25);
        assert_eq!(m.import_factor(&k, CargoKind::Logs, spring), 3.0);
    }

    #[test]
    fn a_shortage_fails_about_half_of_the_import_orders_and_nothing_else_does() {
        let k = MarketKnobs::default();
        let mut m = Market::default();
        let seed = [7; 32];
        let orders = |m: &Market| {
            (0..4u32)
                .flat_map(|who| (0..10u64).map(move |day| (who, day)))
                .filter(|&(who, day)| m.import_fails(&k, seed, who, CargoKind::Metals, day))
                .count()
        };
        assert_eq!(orders(&m), 0);
        m.get_mut(CargoKind::Metals).event = Some(MarketEvent {
            kind: MarketEventKind::Glut,
            until_day: 10,
        });
        assert_eq!(orders(&m), 0);
        m.get_mut(CargoKind::Metals).event = Some(MarketEvent {
            kind: MarketEventKind::Shortage,
            until_day: 10,
        });
        let failed = orders(&m);
        assert!((14..=26).contains(&failed), "{failed} of 40 failed");
    }

    #[test]
    fn the_history_is_a_ring_of_the_last_days() {
        let mut m = Market::default();
        for i in 0..35u16 {
            m.get_mut(CargoKind::Ore).index = 60 + i * 2;
            m.record_day();
        }
        assert_eq!(m.history_at(CargoKind::Ore, 0), 60 + 34 * 2);
        assert_eq!(m.history_at(CargoKind::Ore, 29), 60 + 5 * 2);
    }
}
