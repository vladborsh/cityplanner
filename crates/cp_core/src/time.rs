//! Sim time: the [`Tick`] counter and calendar values derived from it.

use bevy_ecs::resource::Resource;
use serde::{Deserialize, Serialize};

/// One tick is 50 ms of game time.
pub const TICK_MILLIS: u32 = 50;
pub const TICKS_PER_DAY: u64 = 4096;

/// The **movement clock** (`architecture/system-design.md` §2): one tick is this many
/// seconds of agent movement. Kinematics, path costs and abstract trip durations use it,
/// with content speeds in real units (m/s, km/h); schedules use the day clock
/// (4096 ticks = 24 h) instead. Change only together with a `balance.ron` recalibration.
pub const MOVE_SECONDS_PER_TICK: f32 = 2.0;
/// Integration substeps per tick for agent movement (`MOVE_SECONDS_PER_TICK / 4` = 0.5 s).
pub const MOVE_SUBSTEPS: u32 = 4;
pub const DAYS_PER_WEEK: u64 = 7;
pub const TICKS_PER_WEEK: u64 = TICKS_PER_DAY * DAYS_PER_WEEK;

/// Game speed. Which one is active is sim state (a resource set by `SetSpeed`).
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SimSpeed {
    Pause,
    #[default]
    X1,
    X2,
    X3,
}

/// The sim clock. The only source of time in sim code.
#[derive(
    Resource,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
)]
pub struct Tick(pub u64);

impl Tick {
    pub fn advance(&mut self) {
        self.0 += 1;
    }

    pub fn game_time(self) -> GameTime {
        GameTime::from_tick(self)
    }
}

/// Calendar view of a [`Tick`]. Day 0 / tick-of-day 0 is midnight of the first Monday.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameTime {
    /// Days since start.
    pub day: u64,
    /// `0..TICKS_PER_DAY`.
    pub tick_of_day: u32,
}

impl GameTime {
    pub fn from_tick(tick: Tick) -> Self {
        Self {
            day: tick.0 / TICKS_PER_DAY,
            tick_of_day: (tick.0 % TICKS_PER_DAY) as u32,
        }
    }

    /// Weeks since start.
    pub fn week(self) -> u64 {
        self.day / DAYS_PER_WEEK
    }

    /// `0..7`.
    pub fn day_of_week(self) -> u32 {
        (self.day % DAYS_PER_WEEK) as u32
    }

    /// Time of day in `[0, 1)`; 0 = midnight, 0.5 = noon.
    pub fn day_fraction(self) -> f32 {
        self.tick_of_day as f32 / TICKS_PER_DAY as f32
    }

    /// Hour of day, `0..24`.
    pub fn hour(self) -> u32 {
        self.tick_of_day * 24 / TICKS_PER_DAY as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_and_week_boundaries() {
        let t = Tick(0).game_time();
        assert_eq!((t.day, t.tick_of_day, t.week(), t.hour()), (0, 0, 0, 0));
        let t = Tick(TICKS_PER_DAY - 1).game_time();
        assert_eq!((t.day, t.tick_of_day, t.hour()), (0, 4095, 23));
        let t = Tick(TICKS_PER_DAY).game_time();
        assert_eq!((t.day, t.tick_of_day), (1, 0));
        let t = Tick(TICKS_PER_WEEK - 1).game_time();
        assert_eq!((t.week(), t.day_of_week()), (0, 6));
        let t = Tick(TICKS_PER_WEEK).game_time();
        assert_eq!((t.week(), t.day_of_week()), (1, 0));
        assert_eq!(TICKS_PER_WEEK, 28_672);
    }

    #[test]
    fn day_fraction_and_hour() {
        let noon = Tick(TICKS_PER_DAY / 2).game_time();
        assert_eq!(noon.day_fraction(), 0.5);
        assert_eq!(noon.hour(), 12);
    }

    #[test]
    fn advance_and_serde() {
        let mut t = Tick::default();
        t.advance();
        assert_eq!(t, Tick(1));
        let bytes = postcard::to_stdvec(&Tick(123_456)).unwrap();
        assert_eq!(postcard::from_bytes::<Tick>(&bytes).unwrap(), Tick(123_456));
    }
}
