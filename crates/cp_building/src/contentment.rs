//! How content the residents of each home are (T-318, `architecture/system-design.md` §19).
//!
//! The types live here, below `cp_citizen` and `cp_economy`, for the same reason as
//! [`Demand`](crate::Demand): `cp_economy` fills them in its 256-tick pass and the citizens' move-in
//! reads them every tick in between (the move-out once a day). That is also why the table is **saved and
//! checksummed**: a load in the middle of a cycle must see the values of the last pass.
//!
//! Save stability: serialised with postcard; enum variants are only appended.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use bevy_ecs::resource::Resource;
use cp_content::ContentmentKnobs;
use cp_core::Tick;
use serde::{Deserialize, Serialize};

use crate::BuildingId;

/// The most complaints a home keeps (the worst first).
pub const COMPLAINTS_MAX: usize = 3;

/// The index a home without a record counts as (a new home before the first pass).
pub const NEUTRAL_INDEX: u8 = 50;

/// One concrete reason residents are unhappy: the worst factor of a home, in words the player
/// can act on (§19.2). Append-only: the numbers are in saves.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[repr(u8)]
pub enum Complaint {
    /// An empty place in a home's list.
    #[default]
    None = 0,
    NoHealth = 1,
    NoPolice = 2,
    NoFire = 3,
    NoSchool = 4,
    Pollution = 5,
    Noise = 6,
    NoRoad = 7,
    NoWater = 8,
    NoPower = 9,
    Garbage = 10,
    Crime = 11,
    BadWater = 12,
    Dead = 13,
    Sick = 14,
    Jobless = 15,
    LongCommute = 16,
    NoPark = 17,
    HighTax = 18,
    Distrust = 19,
}

impl Complaint {
    /// Every complaint but `None`, in code order.
    pub const ALL: [Complaint; 19] = [
        Complaint::NoHealth,
        Complaint::NoPolice,
        Complaint::NoFire,
        Complaint::NoSchool,
        Complaint::Pollution,
        Complaint::Noise,
        Complaint::NoRoad,
        Complaint::NoWater,
        Complaint::NoPower,
        Complaint::Garbage,
        Complaint::Crime,
        Complaint::BadWater,
        Complaint::Dead,
        Complaint::Sick,
        Complaint::Jobless,
        Complaint::LongCommute,
        Complaint::NoPark,
        Complaint::HighTax,
        Complaint::Distrust,
    ];

    /// The complaint in English, as the inspector and the panels show it (the UI translates it).
    pub fn label(self) -> &'static str {
        match self {
            Complaint::None => "",
            Complaint::NoHealth => "No clinic nearby",
            Complaint::NoPolice => "No police nearby",
            Complaint::NoFire => "No fire station nearby",
            Complaint::NoSchool => "No school nearby",
            Complaint::Pollution => "Polluted air",
            Complaint::Noise => "Too noisy",
            Complaint::NoRoad => "No road",
            Complaint::NoWater => "No water",
            Complaint::NoPower => "No power",
            Complaint::Garbage => "Garbage piles up",
            Complaint::Crime => "Crime",
            Complaint::BadWater => "Bad water",
            Complaint::Dead => "Uncollected dead",
            Complaint::Sick => "Illness",
            Complaint::Jobless => "No work",
            Complaint::LongCommute => "Long way to work",
            Complaint::NoPark => "No park nearby",
            Complaint::HighTax => "Taxes too high",
            Complaint::Distrust => "No trust in the mayor",
        }
    }

    /// A short stable code (`no_health`, ...): the advisor's id `complaint_<code>` (§19.5).
    pub fn code(self) -> &'static str {
        match self {
            Complaint::None => "none",
            Complaint::NoHealth => "no_health",
            Complaint::NoPolice => "no_police",
            Complaint::NoFire => "no_fire",
            Complaint::NoSchool => "no_school",
            Complaint::Pollution => "pollution",
            Complaint::Noise => "noise",
            Complaint::NoRoad => "no_road",
            Complaint::NoWater => "no_water",
            Complaint::NoPower => "no_power",
            Complaint::Garbage => "garbage",
            Complaint::Crime => "crime",
            Complaint::BadWater => "bad_water",
            Complaint::Dead => "dead",
            Complaint::Sick => "sick",
            Complaint::Jobless => "jobless",
            Complaint::LongCommute => "long_commute",
            Complaint::NoPark => "no_park",
            Complaint::HighTax => "high_tax",
            Complaint::Distrust => "distrust",
        }
    }
}

/// The word of a contentment index (§19.5): content from 60, unhappy from 35, miserable below.
pub fn mood_word(index: u8) -> &'static str {
    match index {
        60.. => "content",
        35.. => "unhappy",
        _ => "miserable",
    }
}

/// How much likelier than plain a household moves into a home of contentment `index` (§19.4):
/// linear from `move_in_mul_at_0` to `move_in_mul_at_100`, nothing below `move_in_min_index`.
pub fn move_in_mul(index: u8, k: &ContentmentKnobs) -> f32 {
    if index < k.move_in_min_index {
        return 0.0;
    }
    let t = f32::from(index.min(100)) / 100.0;
    (k.move_in_mul_at_0 + (k.move_in_mul_at_100 - k.move_in_mul_at_0) * t).max(0.0)
}

/// Whether a visited home takes a household this time: `gate` is the plain draw in
/// `0..one_in` (only `0` passes when the multiplier is 1), `bonus` a second uniform draw in
/// `0..100`; together they are one draw in `0..one_in x 100`, which passes below
/// `move_in_mul(index) x 100`, so the chance is `move_in_mul / one_in`.
pub fn move_in_passes(gate: u32, bonus: u32, index: u8, k: &ContentmentKnobs) -> bool {
    ((gate * 100 + bonus.min(99)) as f32) < move_in_mul(index, k) * 100.0
}

/// The record of one home.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomeSlot {
    /// Generation of the `BuildingId` this slot describes; [`HomeSlot::VACANT`] for none.
    generation: u32,
    /// Contentment `0..=100`, smoothed towards the factors' target every pass.
    pub index: u8,
    /// The worst factors' complaints, worst first; `Complaint::None` fills the rest.
    pub complaints: [Complaint; COMPLAINTS_MAX],
    /// Running average of the residents' minutes to work ([`HomeContentment::note_commute`]);
    /// `0` until a trip to work was seen.
    pub commute_min: u8,
}

impl HomeSlot {
    /// The generation of an unused slot (no id has it).
    pub const VACANT: u32 = u32::MAX;

    const EMPTY: HomeSlot = HomeSlot {
        generation: Self::VACANT,
        index: 0,
        complaints: [Complaint::None; COMPLAINTS_MAX],
        commute_min: 0,
    };
}

static REVISIONS: AtomicU64 = AtomicU64::new(1);

fn next_revision() -> u64 {
    REVISIONS.fetch_add(1, Ordering::Relaxed)
}

/// The contentment of every home, dense by `BuildingId.index` (a slot of another generation is
/// no record). Serialized sim state, part of the checksum.
#[derive(Resource, Clone, Debug, Serialize, Deserialize)]
pub struct HomeContentment {
    slots: Vec<HomeSlot>,
    /// Since when each home stands below the move-out line (`leave_below`), by id: set and
    /// cleared by the contentment pass, read by the daily move-out.
    pub unhappy_since: BTreeMap<BuildingId, Tick>,
    /// Changes whenever a record does and at every load: the cache key of the published
    /// snapshot. Not sim state: neither saved nor compared nor hashed.
    #[serde(skip, default = "next_revision")]
    revision: u64,
}

impl Default for HomeContentment {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            unhappy_since: BTreeMap::new(),
            revision: next_revision(),
        }
    }
}

impl PartialEq for HomeContentment {
    /// The saved state.
    fn eq(&self, other: &Self) -> bool {
        self.slots == other.slots && self.unhappy_since == other.unhappy_since
    }
}

impl HomeContentment {
    /// See the field of the same name.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The record of `id`; `None` for a building that is no home or has not been seen yet.
    pub fn get(&self, id: BuildingId) -> Option<&HomeSlot> {
        self.slots
            .get(id.index as usize)
            .filter(|s| s.generation == id.generation)
    }

    /// The index of `id`, [`NEUTRAL_INDEX`] without a record.
    pub fn index_of(&self, id: BuildingId) -> u8 {
        self.get(id).map_or(NEUTRAL_INDEX, |s| s.index)
    }

    /// Whether `id` has a record.
    pub fn contains(&self, id: BuildingId) -> bool {
        self.get(id).is_some()
    }

    /// Writes the index and complaints of `id`, creating its record (a slot of an older
    /// generation starts over, keeping nothing).
    pub fn set(&mut self, id: BuildingId, index: u8, complaints: [Complaint; COMPLAINTS_MAX]) {
        let i = id.index as usize;
        if self.slots.len() <= i {
            self.slots.resize(i + 1, HomeSlot::EMPTY);
        }
        let slot = &mut self.slots[i];
        if slot.generation != id.generation {
            *slot = HomeSlot {
                generation: id.generation,
                ..HomeSlot::EMPTY
            };
        }
        if slot.index != index || slot.complaints != complaints {
            slot.index = index;
            slot.complaints = complaints;
            self.revision = next_revision();
        }
    }

    /// Folds one finished trip to work of `minutes` into the commute average of `id`
    /// (`c += (m - c) / 8`, the first trip sets it). A home without a record is skipped: its
    /// record starts at the next pass.
    pub fn note_commute(&mut self, id: BuildingId, minutes: u16) {
        let Some(slot) = self
            .slots
            .get_mut(id.index as usize)
            .filter(|s| s.generation == id.generation)
        else {
            return;
        };
        let m = i32::from(minutes.min(u16::from(u8::MAX)));
        let c = i32::from(slot.commute_min);
        let next = if c == 0 { m } else { c + (m - c) / 8 } as u8;
        if next != slot.commute_min {
            slot.commute_min = next;
            self.revision = next_revision();
        }
    }

    /// Forgets `id` (no longer an active home); its move-out timer goes too.
    pub fn remove(&mut self, id: BuildingId) {
        if let Some(slot) = self.slots.get_mut(id.index as usize)
            && slot.generation == id.generation
        {
            *slot = HomeSlot::EMPTY;
            self.revision = next_revision();
        }
        self.unhappy_since.remove(&id);
    }

    /// The homes with a record, in index order.
    pub fn homes(&self) -> impl Iterator<Item = (BuildingId, &HomeSlot)> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.generation != HomeSlot::VACANT)
            .map(|(i, s)| (BuildingId::new(i as u32, s.generation), s))
    }

    /// Number of homes with a record.
    pub fn len(&self) -> usize {
        self.homes().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(index: u32, generation: u32) -> BuildingId {
        BuildingId::new(index, generation)
    }

    #[test]
    fn a_record_belongs_to_one_generation_of_its_slot() {
        let mut h = HomeContentment::default();
        assert_eq!(h.index_of(id(3, 0)), NEUTRAL_INDEX);
        h.set(
            id(3, 0),
            72,
            [Complaint::Noise, Complaint::None, Complaint::None],
        );
        assert_eq!(h.index_of(id(3, 0)), 72);
        // The slot reused by another building: no record for it, and setting starts over.
        assert!(h.get(id(3, 1)).is_none());
        h.set(id(3, 1), 40, [Complaint::None; COMPLAINTS_MAX]);
        assert!(h.get(id(3, 0)).is_none());
        assert_eq!(h.homes().map(|(b, _)| b).collect::<Vec<_>>(), [id(3, 1)]);
        h.remove(id(3, 1));
        assert!(h.is_empty());
    }

    #[test]
    fn the_revision_moves_on_a_change_only_and_is_not_saved() {
        let mut h = HomeContentment::default();
        h.set(id(0, 0), 60, [Complaint::None; COMPLAINTS_MAX]);
        let r = h.revision();
        h.set(id(0, 0), 60, [Complaint::None; COMPLAINTS_MAX]);
        assert_eq!(h.revision(), r);
        h.set(id(0, 0), 61, [Complaint::None; COMPLAINTS_MAX]);
        assert_ne!(h.revision(), r);
        h.unhappy_since.insert(id(0, 0), Tick(5));
        let bytes = postcard::to_allocvec(&h).unwrap();
        let back: HomeContentment = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(back, h);
        assert_ne!(back.revision(), h.revision());
    }

    #[test]
    fn move_in_grows_with_the_index_and_stops_below_the_line() {
        let k = ContentmentKnobs::default();
        // Every (gate, bonus) pair of one_in = 4 once: the share that passes is the chance.
        let passes = |index| {
            (0..4u32)
                .flat_map(|g| (0..100u32).map(move |b| (g, b)))
                .filter(|&(g, b)| move_in_passes(g, b, index, &k))
                .count()
        };
        // Neutral = the plain 1 in 4, the same draws as before the multiplier.
        assert_eq!(passes(NEUTRAL_INDEX), 100);
        assert!((0..100).all(|b| move_in_passes(0, b, NEUTRAL_INDEX, &k)));
        assert_eq!(passes(100), 150);
        assert_eq!(passes(20), 70);
        assert_eq!(passes(19), 0);
        assert!(passes(90) as f32 >= 1.5 * passes(30) as f32);
    }

    #[test]
    fn the_commute_average_follows_the_trips_of_a_recorded_home() {
        let mut h = HomeContentment::default();
        h.note_commute(id(2, 0), 40);
        assert!(h.get(id(2, 0)).is_none(), "no record, no average");
        h.set(id(2, 0), 60, [Complaint::None; COMPLAINTS_MAX]);
        h.note_commute(id(2, 0), 40);
        assert_eq!(h.get(id(2, 0)).unwrap().commute_min, 40);
        h.note_commute(id(2, 0), 8);
        assert_eq!(h.get(id(2, 0)).unwrap().commute_min, 36);
        let r = h.revision();
        for _ in 0..60 {
            h.note_commute(id(2, 0), 8);
        }
        assert!(h.get(id(2, 0)).unwrap().commute_min <= 15);
        assert_ne!(h.revision(), r);
    }

    #[test]
    fn complaint_codes_are_stable() {
        assert_eq!(Complaint::None as u8, 0);
        assert_eq!(Complaint::NoHealth as u8, 1);
        assert_eq!(Complaint::Distrust as u8, 19);
        for (i, c) in Complaint::ALL.iter().enumerate() {
            assert_eq!(*c as usize, i + 1);
        }
    }
}
