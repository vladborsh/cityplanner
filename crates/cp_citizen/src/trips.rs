//! Trips and the trip book (`architecture/system-design.md` §9.2).
//!
//! A [`Trip`] is `{ citizen, from, to, purpose, mode }` plus its state. Travel is
//! **abstract** for now: a trip in transit arrives `distance / speed` ticks after it
//! departs. The seam for the traffic tasks is [`TripState::InTransit`]: a later task makes
//! departure ask the path service for a route and spawn an agent, and arrival come from the
//! agent instead of the timer (add a state variant for "awaiting agent"; the citizen's
//! location state machine stays as it is).
//!
//! Save stability: serialised with postcard; changing fields is a save format change and
//! enums are append-only.

use std::collections::{BTreeMap, BTreeSet};

use bevy_ecs::resource::Resource;
use cp_building::BuildingId;
use cp_core::{Arena, Id, Tick};
use serde::{Deserialize, Serialize};

use crate::model::{CitizenId, HouseholdId};

pub type TripId = Id<Trip>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TripPurpose {
    Work,
    Shop,
    Home,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TripMode {
    Walk,
    Car,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TripState {
    /// Waiting at `from` until `depart_at` (e.g. the dwell time at a shop).
    Scheduled { depart_at: Tick },
    /// On the way; arrives at `arrive_at`.
    InTransit { arrive_at: Tick },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Trip {
    pub citizen: CitizenId,
    pub from: BuildingId,
    pub to: BuildingId,
    pub purpose: TripPurpose,
    pub mode: TripMode,
    pub state: TripState,
}

impl Trip {
    /// The tick at which something happens next (departure or arrival).
    pub fn event_at(&self) -> Tick {
        match self.state {
            TripState::Scheduled { depart_at } => depart_at,
            TripState::InTransit { arrive_at } => arrive_at,
        }
    }
}

/// All trips, plus the routine's small bits of state.
#[derive(Resource, Clone, Debug, Default, Serialize, Deserialize)]
pub struct TripBook {
    trips: Arena<Trip>,
    /// Departures per hour of day (0..24), counted when a trip starts moving.
    pub departures_by_hour: [u32; 24],
    pub completed: u64,
    pub cancelled: u64,
    /// Game day of each household's last shopping trip (missing = day 0).
    last_shop_day: BTreeMap<HouseholdId, u32>,
    /// `(event tick, id)`; derived from the trips, rebuilt lazily after a load.
    #[serde(skip)]
    events: BTreeSet<(u64, TripId)>,
    #[serde(skip)]
    by_citizen: BTreeMap<CitizenId, TripId>,
    #[serde(skip)]
    ready: bool,
}

impl PartialEq for TripBook {
    fn eq(&self, other: &Self) -> bool {
        self.trips == other.trips
            && self.departures_by_hour == other.departures_by_hour
            && self.completed == other.completed
            && self.cancelled == other.cancelled
            && self.last_shop_day == other.last_shop_day
    }
}

impl TripBook {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn ensure_derived(&mut self) {
        if self.ready {
            return;
        }
        self.events = self
            .trips
            .iter()
            .map(|(id, t)| (t.event_at().0, id))
            .collect();
        self.by_citizen = self.trips.iter().map(|(id, t)| (t.citizen, id)).collect();
        self.ready = true;
    }

    pub fn get(&self, id: TripId) -> Option<&Trip> {
        self.trips.get(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (TripId, &Trip)> {
        self.trips.iter()
    }

    pub fn len(&self) -> usize {
        self.trips.len()
    }

    pub fn is_empty(&self) -> bool {
        self.trips.is_empty()
    }

    /// The citizen's current trip, if any (needs [`Self::ensure_derived`]).
    pub fn active_of(&self, citizen: CitizenId) -> Option<TripId> {
        self.by_citizen.get(&citizen).copied()
    }

    /// Adds a trip. A citizen has at most one; returns `None` if they already have one.
    pub fn add(&mut self, trip: Trip) -> Option<TripId> {
        if self.by_citizen.contains_key(&trip.citizen) {
            return None;
        }
        let (citizen, at) = (trip.citizen, trip.event_at().0);
        let id = self.trips.insert(trip);
        self.events.insert((at, id));
        self.by_citizen.insert(citizen, id);
        Some(id)
    }

    pub fn remove(&mut self, id: TripId) -> Option<Trip> {
        let t = self.trips.remove(id)?;
        self.events.remove(&(t.event_at().0, id));
        self.by_citizen.remove(&t.citizen);
        Some(t)
    }

    /// Moves a trip to a new state (re-keying its event).
    pub fn set_state(&mut self, id: TripId, state: TripState) {
        if let Some(t) = self.trips.get_mut(id) {
            self.events.remove(&(t.event_at().0, id));
            t.state = state;
            self.events.insert((t.event_at().0, id));
        }
    }

    /// Trips whose event tick is `<= now`, oldest first.
    pub fn due(&self, now: u64) -> Vec<TripId> {
        self.events
            .iter()
            .take_while(|(t, _)| *t <= now)
            .map(|(_, id)| *id)
            .collect()
    }

    /// Days since the household last shopped.
    pub fn days_since_shop(&self, h: HouseholdId, today: u32) -> u32 {
        today.saturating_sub(self.last_shop_day.get(&h).copied().unwrap_or(0))
    }

    pub fn note_shopped(&mut self, h: HouseholdId, today: u32) {
        self.last_shop_day.insert(h, today);
    }

    /// Forgets households that no longer exist.
    pub fn prune_households(&mut self, alive: impl Fn(HouseholdId) -> bool) {
        self.last_shop_day.retain(|h, _| alive(*h));
    }
}
