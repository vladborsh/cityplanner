//! Population milestones (T-149, design §16 M2): [`Progress::reached`] is the index of the
//! highest milestone of `milestones.ron` the city has reached. A pass every [`PROGRESS_PERIOD`]
//! ticks raises it to the highest milestone the population meets; it never goes down. A
//! building def a milestone unlocks cannot be placed before that milestone is reached.

use bevy_ecs::prelude::{Res, ResMut, Resource};
use cp_content::{BuildingDefId, ContentDb, SimContent};
use cp_core::{CommandError, Tick};
use serde::{Deserialize, Serialize};

/// Ticks between two passes.
pub const PROGRESS_PERIOD: u64 = 256;

/// How far the city has come. Saved since v25.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// Index of the highest milestone reached; any value past the last milestone means all of
    /// them. The default is [`Progress::ALL`], so worlds built in code and old saves have
    /// nothing locked; a new game starts at 0 (`SimRunner::new_game`).
    pub reached: u8,
}

impl Progress {
    pub const ALL: Self = Self { reached: u8::MAX };
    pub const START: Self = Self { reached: 0 };

    /// The milestone a building def waits for, if it is still ahead: its index.
    pub fn locked_by(&self, content: &ContentDb, def: BuildingDefId) -> Option<usize> {
        content
            .unlock_milestone(def)
            .filter(|&m| m > usize::from(self.reached))
    }
}

impl Default for Progress {
    fn default() -> Self {
        Self::ALL
    }
}

/// `Err(Locked)` when placing `def` is still ahead of the city's progress.
pub fn check_unlocked(
    progress: &Progress,
    content: &ContentDb,
    def: BuildingDefId,
) -> Result<(), CommandError> {
    match progress.locked_by(content, def) {
        None => Ok(()),
        Some(m) => {
            let ms = &content.milestones()[m];
            Err(CommandError::Locked {
                milestone: ms.name.clone(),
                population: ms.population,
            })
        }
    }
}

/// The pass: raises `reached` to the highest milestone the population meets.
pub fn update_progress(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    citizens: Res<cp_citizen::CitizenGrid>,
    mut progress: ResMut<Progress>,
) {
    if !tick.0.is_multiple_of(PROGRESS_PERIOD) {
        return;
    }
    let Some(content) = content else { return };
    let population = citizens.citizen_count() as u32;
    let reached = content
        .0
        .milestones()
        .iter()
        .rposition(|m| m.population <= population);
    if let Some(r) = reached {
        let r = r.min(usize::from(u8::MAX - 1)) as u8;
        // `ALL` (or anything past the milestone list) stays: it never goes down.
        if r > progress.reached {
            progress.reached = r;
        }
    }
}
