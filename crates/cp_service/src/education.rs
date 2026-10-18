//! Education (§16 M2 D "Образование"): children and students enrol in the nearest school of
//! their tier that reaches their home and has a place left; once they outgrow the tier they
//! graduate and their education goes up. There are no trips to school.
//!
//! Tiers by age (a year is `DAYS_PER_YEAR` days): 6–13 elementary (1), 14–17 high school (2),
//! 18–21 university (3). A citizen is visited 16 times a game day (the citizen bucket
//! `id % 256 == tick % 256`):
//! * enrolled and past the end of the school's tier → `education = max(education, tier)`, no
//!   school;
//! * not enrolled, of the age of a tier, and (for adults) without a job → the nearest powered
//!   school of that tier within its radius by road with a place left (places = `capacity`,
//!   taken = citizens whose `school` it is). The count is kept in `Facility::stored`.
//!
//! A student of the university takes no job (the job search skips citizens with a school).

use std::collections::BTreeMap;

use bevy_ecs::system::{Res, ResMut};
use cp_building::{BuildingGrid, BuildingId, BuildingState};
use cp_citizen::system::CITIZEN_BUCKETS;
use cp_citizen::{CitizenGrid, DAYS_PER_YEAR};
use cp_content::{ServiceKind, SimContent};
use cp_core::Tick;

use crate::state::ServiceState;

/// The tier a citizen of `age_days` studies at, `None` outside school age.
pub fn tier_of_age(age_days: u32) -> Option<u8> {
    match age_days / DAYS_PER_YEAR {
        6..=13 => Some(1),
        14..=17 => Some(2),
        18..=21 => Some(3),
        _ => None,
    }
}

/// First age in years at which a student of `tier` has outgrown it.
fn graduation_years(tier: u8) -> u32 {
    match tier {
        1 => 14,
        2 => 18,
        _ => 22,
    }
}

/// Services phase, after `update_fire`.
pub fn update_education(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    net: crate::ServiceNet,
    buildings: Res<BuildingGrid>,
    mut grid: ResMut<CitizenGrid>,
    mut state: ResMut<ServiceState>,
) {
    let Some(content) = content else { return };
    let db = &content.0;
    let tier_of = |id: BuildingId| {
        let b = buildings
            .get(id)
            .filter(|b| b.state == BuildingState::Active)?;
        let svc = db
            .building(b.def)
            .service
            .as_deref()
            .and_then(|s| db.service_id(s))
            .map(|s| db.service(s))?;
        (svc.kind == ServiceKind::Education).then_some(svc.tier)
    };
    let schools: Vec<BuildingId> = buildings
        .iter()
        .filter(|(id, _)| tier_of(*id).is_some())
        .map(|(id, _)| id)
        .collect();
    if schools.is_empty() {
        return;
    }
    // The places taken, counted from the citizens and kept in `stored`.
    let mut taken: BTreeMap<BuildingId, u32> = BTreeMap::new();
    for (_, c) in grid.citizens() {
        if let Some(s) = c.school {
            *taken.entry(s).or_default() += 1;
        }
    }
    for &s in &schools {
        let n = taken.get(&s).copied().unwrap_or(0);
        if state.facility(s).is_none_or(|f| f.stored != n) {
            state.facility_mut(s).stored = n;
        }
    }

    let bucket = (tick.0 % u64::from(CITIZEN_BUCKETS)) as u32;
    let visited: Vec<_> = grid
        .citizens_bucket(CITIZEN_BUCKETS, bucket)
        .map(|(id, c)| (id, c.clone()))
        .collect();
    // One finder for the whole bucket (T-256: one per citizen rebuilt the road graph each time).
    let roads = net.graph();
    let mut finder = crate::FacilityFinder::new(&roads, &buildings, db, ServiceKind::Education);
    for (id, c) in visited {
        if let Some(school) = c.school {
            let tier = tier_of(school);
            // A school that is gone or is not a school any more: back to no school.
            let Some(tier) = tier else {
                grid.set_school(id, None);
                continue;
            };
            if c.age_days / DAYS_PER_YEAR >= graduation_years(tier) {
                grid.set_education(id, c.education.max(tier));
                grid.set_school(id, None);
                if state.facility(school).is_some_and(|f| f.stored > 0) {
                    state.facility_mut(school).stored -= 1;
                }
            }
            continue;
        }
        let Some(tier) = tier_of_age(c.age_days) else {
            continue;
        };
        // An adult with a job does not study; a child must not have been through the tier.
        if c.work.is_some() || c.education >= tier {
            continue;
        }
        let Some(home) = c.home else { continue };
        let Some((school, metres)) = finder.nearest_with(&state, home, |used, svc| {
            svc.tier == tier && used.stored < svc.capacity
        }) else {
            continue;
        };
        let radius = buildings
            .get(school)
            .and_then(|b| db.building(b.def).service.as_deref())
            .and_then(|s| db.service_id(s))
            .map_or(0.0, |s| db.service(s).radius_m);
        if metres >= radius {
            continue;
        }
        grid.set_school(id, Some(school));
        state.facility_mut(school).stored += 1;
    }
}
