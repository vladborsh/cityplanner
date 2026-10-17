//! Population milestones (T-149, design §16 M2): [`Progress::reached`] is the index of the
//! highest milestone of `milestones.ron` the city has reached. A pass every [`PROGRESS_PERIOD`]
//! ticks raises it to the highest milestone the population meets; it never goes down. A
//! building def a milestone unlocks cannot be placed before that milestone is reached.
//!
//! **Unique buildings (T-200).** The same pass opens the unique buildings and monuments whose
//! conditions ([`UnlockCond`]) all hold, for good: [`Progress::unlocked`] keeps their ids. A
//! `Progress` with `reached == u8::MAX` (`ALL`: worlds built in code, old saves) has everything
//! open.

use std::collections::BTreeSet;

use bevy_ecs::prelude::{Res, ResMut, Resource};
use cp_building::BuildingGrid;
use cp_content::{BuildingDefId, ContentDb, SimContent, UnlockCond};
use cp_core::{CommandError, Tick};
use serde::{Deserialize, Serialize};

/// Ticks between two passes.
pub const PROGRESS_PERIOD: u64 = 256;

/// How far the city has come. Saved since v25; the unique buildings opened since v38.
#[derive(Resource, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// Index of the highest milestone reached; any value past the last milestone means all of
    /// them. The default is [`Progress::ALL`], so worlds built in code and old saves have
    /// nothing locked; a new game starts at 0 (`SimRunner::new_game`).
    pub reached: u8,
    /// Ids of the unique building defs whose conditions have held (permanently open).
    pub unlocked: BTreeSet<String>,
}

impl Progress {
    pub const ALL: Self = Self {
        reached: u8::MAX,
        unlocked: BTreeSet::new(),
    };
    pub const START: Self = Self::at(0);

    /// Milestone `reached`, no unique building open.
    pub const fn at(reached: u8) -> Self {
        Self {
            reached,
            unlocked: BTreeSet::new(),
        }
    }

    /// The milestone a building def waits for, if it is still ahead: its index.
    pub fn locked_by(&self, content: &ContentDb, def: BuildingDefId) -> Option<usize> {
        content
            .unlock_milestone(def)
            .filter(|&m| m > usize::from(self.reached))
    }

    /// Whether the unique building `id` may be placed as far as its conditions go.
    pub fn unique_open(&self, id: &str) -> bool {
        self.reached == u8::MAX || self.unlocked.contains(id)
    }
}

impl Default for Progress {
    fn default() -> Self {
        Self::ALL
    }
}

/// What the unlock conditions read, counted from the world.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnlockInputs {
    pub population: u32,
    pub districts: u32,
    pub tourists_week: u32,
    pub uniques_built: u32,
    /// Buildings per service kind (index `ServiceKind as usize`).
    pub services: [u32; 16],
}

impl UnlockInputs {
    pub fn gather(
        db: &ContentDb,
        population: u32,
        buildings: &BuildingGrid,
        districts: u32,
        tourists_week: u32,
    ) -> Self {
        let mut out = Self {
            population,
            districts,
            tourists_week,
            ..Self::default()
        };
        for (_, b) in buildings.iter() {
            let def = db.building(b.def);
            if def.unique.is_some() {
                out.uniques_built += 1;
            }
            if let Some(svc) = def.service.as_deref().and_then(|s| db.service_id(s)) {
                out.services[db.service(svc).kind as usize] += 1;
            }
        }
        out
    }
}

/// `(current, needed)` of one condition.
pub fn cond_progress(cond: &UnlockCond, i: &UnlockInputs) -> (u32, u32) {
    match cond {
        UnlockCond::Population(n) => (i.population, *n),
        UnlockCond::ServiceBuildings { kind, n } => (i.services[*kind as usize], u32::from(*n)),
        UnlockCond::TouristsWeek(n) => (i.tourists_week, *n),
        // Specialisations (T-196) are not in the sim yet: nothing counts.
        UnlockCond::SpecBuildings { n, .. } => (0, u32::from(*n)),
        UnlockCond::UniquesBuilt(n) => (i.uniques_built, u32::from(*n)),
        UnlockCond::Districts(n) => (i.districts, u32::from(*n)),
    }
}

/// Whether every condition of the unique def `id` holds right now.
pub fn conditions_hold(conds: &[UnlockCond], i: &UnlockInputs) -> bool {
    conds.iter().all(|c| {
        let (cur, need) = cond_progress(c, i);
        cur >= need
    })
}

/// What a condition counts, for the UI ("Population", "Tourists a week").
pub fn cond_name(c: &UnlockCond) -> String {
    match c {
        UnlockCond::Population(_) => "Population".to_string(),
        UnlockCond::ServiceBuildings { kind, .. } => format!("{kind:?} buildings"),
        UnlockCond::TouristsWeek(_) => "Tourists a week".to_string(),
        UnlockCond::SpecBuildings { spec, .. } => format!("{spec} buildings"),
        UnlockCond::UniquesBuilt(_) => "Unique buildings".to_string(),
        UnlockCond::Districts(_) => "Districts".to_string(),
    }
}

/// A readable form of the first condition that does not hold yet (the reason for a lock).
pub fn unmet_text(conds: &[UnlockCond], i: &UnlockInputs) -> Option<(String, u32)> {
    let c = conds.iter().find(|c| {
        let (cur, need) = cond_progress(c, i);
        cur < need
    })?;
    let (cur, need) = cond_progress(c, i);
    let name = cond_name(c);
    let population = if matches!(c, UnlockCond::Population(_)) {
        need
    } else {
        0
    };
    Some((format!("{name} {cur} / {need}"), population))
}

/// `Err(Locked)` when placing `def` is still ahead of the city's progress: a milestone, or for
/// a unique building the conditions that do not hold yet (`inputs` counts them).
pub fn check_unlocked(
    progress: &Progress,
    content: &ContentDb,
    def: BuildingDefId,
    inputs: &UnlockInputs,
) -> Result<(), CommandError> {
    if let Some(m) = progress.locked_by(content, def) {
        let ms = &content.milestones()[m];
        return Err(CommandError::Locked {
            milestone: ms.name.clone(),
            population: ms.population,
        });
    }
    let d = content.building(def);
    if let Some(u) = &d.unique
        && !progress.unique_open(&d.id)
    {
        let (what, population) = unmet_text(&u.unlock, inputs).unwrap_or_default();
        return Err(CommandError::Locked {
            milestone: what,
            population,
        });
    }
    Ok(())
}

/// The pass: raises `reached` to the highest milestone the population meets, and opens the
/// unique buildings whose conditions hold.
#[allow(clippy::too_many_arguments)]
pub fn update_progress(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    citizens: Res<cp_citizen::CitizenGrid>,
    buildings: Res<BuildingGrid>,
    districts: Res<cp_district::Districts>,
    tourists: Res<cp_transit::Tourists>,
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
    if progress.reached == u8::MAX {
        return;
    }
    let inputs = UnlockInputs::gather(
        &content.0,
        population,
        &buildings,
        districts.iter().count() as u32,
        tourists.visits_week,
    );
    let open: Vec<String> = content
        .0
        .buildings()
        .filter_map(|(_, d)| d.unique.as_ref().map(|u| (d, u)))
        .filter(|(d, u)| !progress.unlocked.contains(&d.id) && conditions_hold(&u.unlock, &inputs))
        .map(|(d, _)| d.id.clone())
        .collect();
    progress.unlocked.extend(open);
}
