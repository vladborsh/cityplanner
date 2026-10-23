//! Corruption, institutions and trust (`architecture/corruption.md`, T-324): the city's state
//! and its daily update. Phase 1 grows it task by task: T-324 the state, trust and its pull on
//! the tax take; T-325 the deals that fill the inbox and the evidence; T-326 scandals and
//! captured police stations; T-327 the institutions.
//!
//! * **Daily** (the first tick of every game day after tick 0): corruption falls by
//!   `corruption_decay_day`, trust climbs by `trust_recover_day` towards its ceiling
//!   (`trust_ceiling`, `trust_ceiling_clan` on the clan edge of the axis `C - I`). A new city
//!   gets `trust_start` on its first day; until then [`CorruptionState::trust_now`] reads it.
//! * **Tax** ([`CorruptionState::tax_collection`]): below `tax_trust_full` trust a share of
//!   the tax is lost, linearly up to `tax_loss_max` at no trust. `settle_week` applies it.
//! * **Move-in** is not here: trust is factor 8 of the home contentment (`system-design.md`
//!   §19.2), which reads [`CorruptionState::trust_now`].
//!
//! Saved whole; points are `f32` (same binary and platform determinism, AGENTS.md).

use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Res, ResMut};
use cp_building::BuildingId;
use cp_content::{CorruptionKnobs, SimContent};
use cp_core::Tick;
use cp_core::time::TICKS_PER_DAY;
use cp_district::DistrictId;
use serde::{Deserialize, Serialize};

/// An accepted deal nobody has found out about yet (§2): it stays until a scandal reveals it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    /// The deal's content id.
    pub deal: String,
    pub accepted_day: u32,
    pub weight: f32,
}

/// What an offer is about, lit on the map (§4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DealTarget {
    District(DistrictId),
    Building(BuildingId),
}

/// A deal waiting in the inbox for an answer until `expires_day` (§4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Offer {
    /// The deal's content id.
    pub deal: String,
    pub target: Option<DealTarget>,
    pub expires_day: u32,
}

/// The city's corruption state (§2). See the module docs.
#[derive(Resource, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CorruptionState {
    /// Corruption `C`, `0..=100`.
    pub corruption: f32,
    /// Institutions `I`, `0..=100`.
    pub institutions: f32,
    /// Trust in the mayor, `0..=100`; meaningful once `started`.
    pub trust: f32,
    /// Whether the first day has seeded `trust` from the content.
    pub started: bool,
    pub evidence: Vec<Evidence>,
    pub inbox: Vec<Offer>,
    /// Police stations on the take, sorted.
    pub captured: Vec<BuildingId>,
    pub next_offer_day: u32,
    pub scandal_cooldown_until: u32,
}

impl Default for CorruptionState {
    fn default() -> Self {
        Self {
            corruption: 0.0,
            institutions: 0.0,
            trust: 0.0,
            started: false,
            evidence: Vec::new(),
            inbox: Vec::new(),
            captured: Vec::new(),
            next_offer_day: 0,
            scandal_cooldown_until: 0,
        }
    }
}

/// Where the city stands on the axis of power.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    /// Clan power: `C - I > edge`.
    Clan,
    /// The grey middle.
    Middle,
    /// Rule of law: `I - C > edge`.
    Law,
}

impl CorruptionState {
    /// The axis `C - I` (§2).
    pub fn axis(&self) -> f32 {
        self.corruption - self.institutions
    }

    pub fn edge(&self, k: &CorruptionKnobs) -> Edge {
        let a = self.axis();
        if a > k.edge {
            Edge::Clan
        } else if a < -k.edge {
            Edge::Law
        } else {
            Edge::Middle
        }
    }

    /// Trust as it stands: `trust_start` before the first day.
    pub fn trust_now(&self, k: &CorruptionKnobs) -> f32 {
        if self.started {
            self.trust
        } else {
            k.trust_start
        }
    }

    /// The highest trust can climb to (§3).
    pub fn trust_ceiling(&self, k: &CorruptionKnobs) -> f32 {
        match self.edge(k) {
            Edge::Clan => k.trust_ceiling_clan,
            _ => k.trust_ceiling,
        }
    }

    /// The share of the tax the city collects (§3), `1 - tax_loss_max ..= 1`.
    pub fn tax_collection(&self, k: &CorruptionKnobs) -> f64 {
        let full = k.tax_trust_full;
        if full <= 0.0 {
            return 1.0;
        }
        let short = ((full - self.trust_now(k)) / full).clamp(0.0, 1.0);
        1.0 - f64::from(short * k.tax_loss_max)
    }

    /// One game day of drift (see the module docs).
    pub fn day(&mut self, k: &CorruptionKnobs) {
        if !self.started {
            self.trust = k.trust_start;
            self.started = true;
        }
        self.corruption = (self.corruption - k.corruption_decay_day).clamp(0.0, 100.0);
        let ceiling = self.trust_ceiling(k);
        // Above the ceiling (the city slid to the clan edge) trust sinks to it at the same pace.
        self.trust = if self.trust < ceiling {
            (self.trust + k.trust_recover_day).min(ceiling)
        } else {
            (self.trust - k.trust_recover_day).max(ceiling)
        }
        .clamp(0.0, 100.0);
    }
}

/// `Economy`-phase system: [`CorruptionState::day`] in the first tick of every game day after
/// tick 0.
pub fn update_corruption(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    mut state: ResMut<CorruptionState>,
) {
    if tick.0 == 0 || !tick.0.is_multiple_of(TICKS_PER_DAY) {
        return;
    }
    let Some(content) = content else { return };
    state.day(&content.0.balance().corruption);
}
