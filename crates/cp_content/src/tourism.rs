//! Tourism content (§22, T-321): the hotel of a building ([`HotelSpec`]) and the knobs of
//! `balance.ron` `tourism` ([`TourismKnobs`]): the season of the flow, the lodgers' stays and
//! outings.

use cp_core::Tick;
use cp_core::time::{Season, season};
use serde::{Deserialize, Serialize};

/// A hotel (§22.2): a placed one (`hotel_small|mid|grand`) or a grown Tourism building.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HotelSpec {
    pub rooms: u16,
    /// 1 to 3; a tourist picks a hotel by `stars`.
    pub stars: u8,
    /// What a guest pays a night, currency units.
    pub price_night: f32,
}

/// The tourism knobs of `balance.ron` (`tourism`, §22). // tune later
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TourismKnobs {
    /// The flow of tourists a day (cars and train) times this, spring to winter.
    pub season: [f32; 4],
    /// The share of new tourists who try to stay at a hotel.
    pub stay_share: f32,
    /// A stay is `1..=max_nights` nights (uniform).
    pub max_nights: u8,
    /// A lodger's daily outing starts at a random hour of `outing_from..outing_to`.
    pub outing_from: u8,
    pub outing_to: u8,
    /// Goals nearer the hotel than this (straight line, metres) are walked to.
    pub walk_m: f32,
}

impl Default for TourismKnobs {
    fn default() -> Self {
        Self {
            season: [1.0, 1.5, 0.9, 0.6],
            stay_share: 0.5,
            max_nights: 3,
            outing_from: 10,
            outing_to: 20,
            walk_m: 800.0,
        }
    }
}

impl TourismKnobs {
    /// Every knob as `f32`, for the content checks and the content hash.
    pub fn knobs(&self) -> Vec<f32> {
        let mut v = self.season.to_vec();
        v.extend([
            self.stay_share,
            f32::from(self.max_nights),
            f32::from(self.outing_from),
            f32::from(self.outing_to),
            self.walk_m,
        ]);
        v
    }

    /// The flow multiplier of the season of `tick`.
    pub fn season_mul(&self, tick: Tick) -> f32 {
        self.season_of(season(tick))
    }

    pub fn season_of(&self, s: Season) -> f32 {
        self.season[s as usize]
    }
}
