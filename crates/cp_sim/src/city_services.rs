//! The city's supply and demand per sector (T-232), for the City services window. Derived from
//! what the systems already hold (utility capacity and consumption, the service buildings'
//! daily capacity, the garbage the buildings make, the sick, the dead, the crime, the school
//! age citizens); no new mechanics, nothing saved. Recomputed with [`CityStats`](crate::stats)
//! and published as `SimView::city_services`.

use cp_building::{BuildingFlags, BuildingGrid, BuildingState};
use cp_citizen::CitizenGrid;
use cp_content::{ContentDb, ServiceKind, ServiceUnlocks, ZoneType};
use cp_service::{ServiceCoverage, education::tier_of_age};

/// A row of the City services window, in the order of [`Sector::ALL`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Sector {
    Water,
    Sewage,
    Power,
    Garbage,
    Health,
    Deathcare,
    Police,
    Fire,
    Elementary,
    HighSchool,
    University,
    /// Mail made against the post offices' collecting (T-247).
    Mail,
    /// Mail to sort against the sorting capacity of the post offices and sorting facilities.
    Sorting,
}

impl Sector {
    pub const ALL: [Sector; 13] = [
        Sector::Water,
        Sector::Sewage,
        Sector::Power,
        Sector::Garbage,
        Sector::Health,
        Sector::Deathcare,
        Sector::Police,
        Sector::Fire,
        Sector::Elementary,
        Sector::HighSchool,
        Sector::University,
        Sector::Mail,
        Sector::Sorting,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Sector::Water => "Water",
            Sector::Sewage => "Sewage",
            Sector::Power => "Electricity",
            Sector::Garbage => "Garbage",
            Sector::Health => "Health",
            Sector::Deathcare => "Deathcare",
            Sector::Police => "Police",
            Sector::Fire => "Fire",
            Sector::Elementary => "Elementary school",
            Sector::HighSchool => "High school",
            Sector::University => "University",
            Sector::Mail => "Mail",
            Sector::Sorting => "Mail sorting",
        }
    }

    /// The service kind whose milestone opens the sector.
    pub fn kind(self) -> ServiceKind {
        match self {
            Sector::Water => ServiceKind::Water,
            Sector::Sewage => ServiceKind::Sewage,
            Sector::Power => ServiceKind::Power,
            Sector::Garbage => ServiceKind::Garbage,
            Sector::Health => ServiceKind::Health,
            Sector::Deathcare => ServiceKind::Deathcare,
            Sector::Police => ServiceKind::Police,
            Sector::Fire => ServiceKind::Fire,
            Sector::Elementary | Sector::HighSchool | Sector::University => ServiceKind::Education,
            Sector::Mail | Sector::Sorting => ServiceKind::Post,
        }
    }

    fn unit(self) -> &'static str {
        match self {
            Sector::Water | Sector::Sewage | Sector::Power => "units/day",
            Sector::Garbage => "garbage/day",
            Sector::Health => "patients",
            Sector::Deathcare => "bodies/day",
            Sector::Police => "crimes/day",
            Sector::Fire => "buildings",
            Sector::Elementary | Sector::HighSchool | Sector::University => "seats",
            Sector::Mail | Sector::Sorting => "mail/day",
        }
    }

    fn tier(self) -> u8 {
        match self {
            Sector::Elementary => 1,
            Sector::HighSchool => 2,
            Sector::University => 3,
            _ => 0,
        }
    }
}

/// One sector's supply against its demand.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SectorBalance {
    pub sector: Sector,
    pub supply: f32,
    pub demand: f32,
    pub unit: &'static str,
    /// The service is not open yet (milestone): the window greys the row, no bar.
    pub locked: bool,
}

impl SectorBalance {
    /// `(supply - demand) / max(demand, 1)` clamped to `-1..=1`: the bar's marker.
    pub fn balance(&self) -> f32 {
        ((self.supply - self.demand) / self.demand.max(1.0)).clamp(-1.0, 1.0)
    }
}

/// What the balance reads besides the buildings and citizens.
pub struct BalanceInputs<'a> {
    pub db: &'a ContentDb,
    pub buildings: &'a BuildingGrid,
    pub citizens: &'a CitizenGrid,
    pub coverage: &'a ServiceCoverage,
    pub unlocks: ServiceUnlocks,
    /// `CityStats::utilities`: `(capacity, consumption)` of water, sewage and power.
    pub utilities: [(f32, f32); 3],
    /// Deaths in the last game day.
    pub died_today: u32,
}

/// The balance of every sector, in [`Sector::ALL`] order.
pub fn city_services(i: &BalanceInputs) -> Vec<SectorBalance> {
    let bal = i.db.balance();
    let mut supply = [0.0f32; Sector::ALL.len()];
    let mut demand = [0.0f32; Sector::ALL.len()];
    for (n, (cap, use_)) in i.utilities.iter().enumerate() {
        supply[n] = *cap;
        demand[n] = *use_;
    }
    let idx = |s: Sector| s as usize;
    for (id, b) in i.buildings.iter() {
        if b.state != BuildingState::Active {
            continue;
        }
        let def = i.db.building(b.def);
        let svc = def
            .service
            .as_deref()
            .and_then(|s| i.db.service_id(s))
            .map(|s| i.db.service(s));
        if let Some(svc) = svc {
            // A service without power does not work (as the facility finder).
            if !b.flags.has(BuildingFlags::NO_POWER) {
                let sector = match svc.kind {
                    ServiceKind::Garbage => Some(Sector::Garbage),
                    ServiceKind::Health => Some(Sector::Health),
                    ServiceKind::Deathcare => Some(Sector::Deathcare),
                    ServiceKind::Police => Some(Sector::Police),
                    ServiceKind::Post => {
                        // A post office collects and sorts a little; a facility only sorts.
                        if svc.tier == cp_service::mail::POST_OFFICE_TIER {
                            supply[idx(Sector::Mail)] += svc.capacity as f32;
                            supply[idx(Sector::Sorting)] += bal.mail_post_sort;
                        } else {
                            supply[idx(Sector::Sorting)] += svc.capacity as f32;
                        }
                        None
                    }
                    ServiceKind::Education => match svc.tier {
                        1 => Some(Sector::Elementary),
                        2 => Some(Sector::HighSchool),
                        3 => Some(Sector::University),
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(s) = sector {
                    supply[idx(s)] += svc.capacity as f32;
                }
            }
            continue;
        }
        // Homes and workplaces: the garbage they make, the crime they breed, the fire cover.
        let workers: u16 = (0..4).map(|l| i.citizens.jobs_filled(id, l)).sum();
        demand[idx(Sector::Garbage)] += f32::from(i.citizens.households_in(id))
            * bal.garbage_household
            + f32::from(workers) * bal.garbage_job;
        let job_mul = match def.zone {
            ZoneType::ComLow | ZoneType::ComHigh => bal.mail_commercial_mul,
            ZoneType::Office => bal.mail_office_mul,
            _ => 1.0,
        };
        let mail = (f32::from(i.citizens.households_in(id)) * bal.mail_household
            + f32::from(workers) * bal.mail_job * job_mul)
            .round();
        demand[idx(Sector::Mail)] += mail;
        demand[idx(Sector::Sorting)] += mail;
        if matches!(
            def.zone,
            ZoneType::ResLow | ZoneType::ResHigh | ZoneType::ComLow | ZoneType::ComHigh
        ) {
            let cover = i
                .coverage
                .coverage_of(ServiceKind::Police, b)
                .clamp(0.0, 1.0);
            demand[idx(Sector::Police)] += bal.crime_base * (1.0 - bal.police_cover * cover);
        }
        demand[idx(Sector::Fire)] += 1.0;
        if i.coverage.coverage_of(ServiceKind::Fire, b) > 0.0 {
            supply[idx(Sector::Fire)] += 1.0;
        }
    }
    // Police: the stations clear crime; a city without any breeds it all.
    // People: the sick now, the dead of the last day, the pupils of each tier.
    demand[idx(Sector::Health)] = i
        .citizens
        .citizens()
        .filter(|(_, c)| c.sick_since.is_some())
        .count() as f32;
    let waiting: u32 = i.citizens.dead.values().map(|n| u32::from(*n)).sum();
    demand[idx(Sector::Deathcare)] = i.died_today.max(waiting) as f32;
    for (_, c) in i.citizens.citizens() {
        if let Some(t) = tier_of_age(c.age_days) {
            let sector = match t {
                1 => Sector::Elementary,
                2 => Sector::HighSchool,
                _ => Sector::University,
            };
            if c.education < sector.tier() {
                demand[idx(sector)] += 1.0;
            }
        }
    }
    Sector::ALL
        .iter()
        .map(|&sector| SectorBalance {
            sector,
            supply: supply[idx(sector)],
            demand: demand[idx(sector)],
            unit: sector.unit(),
            locked: !i.unlocks.open(sector.kind()),
        })
        .collect()
}
