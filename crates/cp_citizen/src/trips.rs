//! Trips and the trip book (`architecture/system-design.md` §9.2).
//!
//! A [`Trip`] is `{ citizen, from, to, purpose, mode }` plus its state. Life cycle:
//!
//! ```text
//! Scheduled{depart_at} ─depart→ Planned ─cp_traffic submits a path→ AwaitingPath{request}
//!        ─path found, agent from the pool→ Agent         ─agent arrives→ (done)
//!        ─path found, pool empty        → InTransit{arrive_at} ─timer→ (done)   [abstract]
//!        ─no path / building gone       → cancelled (the citizen goes home)
//! ```
//!
//! `Planned`, `AwaitingPath` and `Agent` belong to `cp_traffic` (which finishes the trip
//! through [`crate::routine::complete_trip`]); `Scheduled` and `InTransit` are handled by
//! the routine's timer. An abstract trip arrives when its path cost says so (§3, §9.4).
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
    /// A new household driving in from the `Outside` node of the entry highway to its home
    /// (§5.1). `from == to == home`; `cp_traffic` starts the car at the Outside node. Appended
    /// in save format 26.
    MoveIn,
    /// A visit to a park gate, a city park or a unique building (T-224). Appended in save
    /// format 48.
    Leisure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TripMode {
    Walk,
    Car,
    /// By bus (or later metro/train): owned by `cp_transit` from the moment `choose_transit`
    /// picks it (T-159). Appended in save format 31.
    Transit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TripState {
    /// Waiting at `from` until `depart_at` (e.g. the dwell time at a shop).
    Scheduled { depart_at: Tick },
    /// On the way without an agent (pool exhausted): arrives at `arrive_at`.
    InTransit { arrive_at: Tick },
    /// Created by the routine; `cp_traffic` asks the path service for a route next tick.
    Planned,
    /// A path request is in flight (`cp_traffic::RequestId` as a number).
    AwaitingPath { request: u64 },
    /// An agent (pedestrian or vehicle, owned by `cp_traffic`) is carrying out the trip.
    Agent,
    /// Carried out by `cp_transit` (a `Journey` in `TransitNet::riders`); it completes the
    /// trip itself. Appended in save format 31.
    Transit,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Trip {
    pub citizen: CitizenId,
    pub from: BuildingId,
    pub to: BuildingId,
    pub purpose: TripPurpose,
    pub mode: TripMode,
    pub state: TripState,
    /// When the trip set off (T-318b: the minutes to work); a scheduled trip is re-stamped at
    /// its departure. Saved since format 70.
    pub started: Tick,
}

impl Trip {
    /// The tick at which the routine's timer acts next (departure or abstract arrival);
    /// `None` while the trip is in `cp_traffic`'s hands.
    pub fn event_at(&self) -> Option<Tick> {
        match self.state {
            TripState::Scheduled { depart_at } => Some(depart_at),
            TripState::InTransit { arrive_at } => Some(arrive_at),
            TripState::Planned
            | TripState::AwaitingPath { .. }
            | TripState::Agent
            | TripState::Transit => None,
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
    /// Finished trips to work since the last contentment pass: the home and the minutes on the
    /// way, in completion order (T-318b). Saved: the pass that folds them in runs every 256 ticks.
    pub commutes: Vec<(BuildingId, u16)>,
    /// `(event tick, id)`; derived from the trips, rebuilt lazily after a load.
    #[serde(skip)]
    events: BTreeSet<(u64, TripId)>,
    /// Trips in `Planned` (waiting for `cp_traffic` to route them); derived.
    #[serde(skip)]
    planned: BTreeSet<TripId>,
    #[serde(skip)]
    by_citizen: BTreeMap<CitizenId, TripId>,
    #[serde(skip)]
    ready: bool,
    /// Leisure trips that arrived since `cp_park` last looked (T-224); transient, drained every
    /// tick.
    #[serde(skip)]
    pub leisure_arrivals: Vec<LeisureArrival>,
    /// How long a leisure visit lasts, ticks; set by the routine from the balance each tick
    /// (0 until it has run once).
    #[serde(skip)]
    pub leisure_dwell: u64,
}

/// A citizen who reached the building of a leisure trip (T-224) and stays until `leave_at`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeisureArrival {
    pub citizen: CitizenId,
    pub building: BuildingId,
    pub leave_at: cp_core::Tick,
}

impl PartialEq for TripBook {
    fn eq(&self, other: &Self) -> bool {
        self.trips == other.trips
            && self.departures_by_hour == other.departures_by_hour
            && self.completed == other.completed
            && self.cancelled == other.cancelled
            && self.last_shop_day == other.last_shop_day
            && self.commutes == other.commutes
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
            .filter_map(|(id, t)| t.event_at().map(|e| (e.0, id)))
            .collect();
        self.planned = self
            .trips
            .iter()
            .filter(|(_, t)| t.state == TripState::Planned)
            .map(|(id, _)| id)
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
        let (citizen, at, planned) = (
            trip.citizen,
            trip.event_at(),
            trip.state == TripState::Planned,
        );
        let id = self.trips.insert(trip);
        if let Some(at) = at {
            self.events.insert((at.0, id));
        }
        if planned {
            self.planned.insert(id);
        }
        self.by_citizen.insert(citizen, id);
        Some(id)
    }

    pub fn remove(&mut self, id: TripId) -> Option<Trip> {
        let t = self.trips.remove(id)?;
        if let Some(at) = t.event_at() {
            self.events.remove(&(at.0, id));
        }
        self.planned.remove(&id);
        self.by_citizen.remove(&t.citizen);
        Some(t)
    }

    /// Moves a trip to a new state (re-keying its event).
    pub fn set_state(&mut self, id: TripId, state: TripState) {
        if let Some(t) = self.trips.get_mut(id) {
            if let Some(at) = t.event_at() {
                self.events.remove(&(at.0, id));
            }
            self.planned.remove(&id);
            t.state = state;
            if let Some(at) = t.event_at() {
                self.events.insert((at.0, id));
            }
            if state == TripState::Planned {
                self.planned.insert(id);
            }
        }
    }

    /// Changes the mode of a trip (`cp_transit::choose_transit`, T-159).
    pub fn set_mode(&mut self, id: TripId, mode: TripMode) {
        if let Some(t) = self.trips.get_mut(id) {
            t.mode = mode;
        }
    }

    /// Changes where a trip starts (the household that comes by train starts at the station,
    /// T-167).
    pub fn set_from(&mut self, id: TripId, from: BuildingId) {
        if let Some(t) = self.trips.get_mut(id) {
            t.from = from;
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

    /// Trips waiting to be routed, oldest id first.
    pub fn planned(&self) -> Vec<TripId> {
        self.planned.iter().copied().collect()
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
