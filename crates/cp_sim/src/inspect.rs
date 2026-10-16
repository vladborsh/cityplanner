//! Read-only details of one selected thing, for the info panels (`architecture/system-design.md`
//! §13.6, T-044). The client keeps a [`Selection`]; the app asks the sim for its
//! [`Inspection`] (`SimRunner::inspect`) after each step and the UI draws that. It is plain data
//! built from the sim world on demand, so a panel can never show anything but what the sim
//! holds.

use bevy_math::Vec3;
use cp_building::{BuildingFlags, BuildingId, BuildingState};
use cp_citizen::{
    CitizenGrid, CitizenId, DAYS_PER_YEAR, Location, TripBook, TripMode, TripPurpose, TripState,
};
use cp_content::SimContent;
use cp_core::Tick;
use cp_core::time::TICKS_PER_DAY;
use cp_economy::{Decay, LandGrids};
use cp_net::{Elevation, NetGraph, SegmentId};
use cp_traffic::{Agents, PedestrianId, VehicleId};

use crate::SimRunner;

/// What the player clicked on. Pedestrians stand for their citizen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    Building(BuildingId),
    Citizen(CitizenId),
    /// `AgentKey` of the vehicle (`generation << 32 | index`).
    Vehicle(u64),
    Pedestrian(u64),
    Segment(SegmentId),
}

/// Splits an `AgentKey` into arena `(index, generation)`.
pub fn key_parts(key: u64) -> (u32, u32) {
    (key as u32, (key >> 32) as u32)
}

#[derive(Clone, Debug, PartialEq)]
pub struct BuildingInfo {
    pub id: BuildingId,
    pub def: String,
    pub name: String,
    pub zone: cp_content::ZoneType,
    /// Level of the definition it is built as.
    pub level: u8,
    /// Progress of the level checks: towards `+1` (up) or `-1` (down).
    pub level_progress: f32,
    pub state: BuildingState,
    pub lot: (u8, u8),
    /// Names of the flags that are up (`"no water"`, ...).
    pub problems: Vec<&'static str>,
    pub households: u16,
    pub apartments: u16,
    pub residents: u32,
    /// Workers per education level and the slots the definition has.
    pub workers: [u16; 4],
    pub jobs: [u16; 4],
    /// Days of an unbroken problem, and the days left until abandoned / demolished.
    pub trouble: Option<Trouble>,
    pub land_value: f32,
    pub pollution: f32,
    pub noise: f32,
    pub tax_base: f32,
    pub footprint: Vec<cp_zone::ZoneCellRef>,
    /// Garbage piled up in the building (T-136).
    pub garbage: u16,
    /// Set when the building is a service facility (T-137).
    pub service: Option<ServiceInfo>,
}

/// The day's throughput and stock of a service building.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceInfo {
    pub kind: cp_content::ServiceKind,
    pub used_today: u32,
    pub capacity: u32,
    pub stored: u32,
    /// 0 when the building keeps no stock.
    pub storage: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trouble {
    pub days: f32,
    pub days_to_abandon: f32,
    pub days_to_demolition: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TripInfo {
    pub purpose: TripPurpose,
    pub mode: TripMode,
    pub state: TripState,
    pub from: BuildingId,
    pub to: BuildingId,
    pub from_name: String,
    pub to_name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CitizenInfo {
    pub id: CitizenId,
    pub age_years: u32,
    pub education: u8,
    pub wellbeing: u8,
    pub has_car: bool,
    pub location: Location,
    pub household_size: usize,
    pub home: Option<(BuildingId, String)>,
    pub work: Option<(BuildingId, String, u8)>,
    pub trip: Option<TripInfo>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VehicleInfo {
    pub key: u64,
    pub kind: u8,
    pub speed_mps: f32,
    pub driver: CitizenId,
    pub trip: Option<TripInfo>,
    /// Lanes of the route still to go, counting the one it is on.
    pub lanes_left: usize,
    pub lanes_total: usize,
    pub waiting_to_spawn: bool,
    /// The rest of the route as one centre line per lane, starting with the current lane.
    pub route: Vec<Vec<Vec3>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SegmentInfo {
    pub id: SegmentId,
    pub road: String,
    pub name: String,
    pub length: f32,
    pub elevation: Elevation,
    pub lanes: usize,
    pub speed_kmh: f32,
    /// Current load and the 2-hour average, `0..=1` of the capacity.
    pub load: f32,
    pub load_avg: f32,
    /// Centre line, for the highlight.
    pub line: Vec<Vec3>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Inspection {
    Building(BuildingInfo),
    Citizen(CitizenInfo),
    Vehicle(VehicleInfo),
    Segment(SegmentInfo),
}

pub fn flag_names(flags: BuildingFlags) -> Vec<&'static str> {
    [
        (BuildingFlags::NO_WATER, "no water"),
        (BuildingFlags::NO_POWER, "no power"),
        (BuildingFlags::NO_WORKERS, "no workers"),
        (BuildingFlags::NO_CUSTOMERS, "no customers"),
        (BuildingFlags::NO_GOODS, "no goods"),
        (BuildingFlags::NO_SEWAGE, "no sewage"),
        (BuildingFlags::NO_ROAD, "no road"),
        (BuildingFlags::NO_PATH, "no path"),
    ]
    .into_iter()
    .filter(|(bit, _)| flags.has(*bit))
    .map(|(_, name)| name)
    .collect()
}

impl SimRunner {
    /// Details of `sel`, or `None` if it no longer exists (a car that arrived, a demolished
    /// building).
    pub fn inspect(&self, sel: Selection) -> Option<Inspection> {
        let w = self.world();
        let content = w.get_resource::<SimContent>()?;
        let db = &*content.0;
        let building_name = |id: BuildingId| -> Option<String> {
            let b = w.resource::<cp_building::BuildingGrid>().get(id)?;
            Some(db.building(b.def).name.clone())
        };
        let trip_info = |c: CitizenId| -> Option<TripInfo> {
            let book = w.get_resource::<TripBook>()?;
            let t = book.get(book.active_of(c)?)?;
            Some(TripInfo {
                purpose: t.purpose,
                mode: t.mode,
                state: t.state,
                from: t.from,
                to: t.to,
                from_name: building_name(t.from).unwrap_or_default(),
                to_name: building_name(t.to).unwrap_or_default(),
            })
        };
        match sel {
            Selection::Building(id) => {
                let buildings = w.resource::<cp_building::BuildingGrid>();
                let b = buildings.get(id)?;
                let def = db.building(b.def);
                let citizens = w.resource::<CitizenGrid>();
                let households = citizens.households_in(id);
                let residents: u32 = citizens
                    .households()
                    .filter(|(_, h)| h.home == id)
                    .map(|(_, h)| h.member_ids().count() as u32)
                    .sum();
                let workers = std::array::from_fn(|l| citizens.jobs_filled(id, l));
                let bal = db.balance();
                let now = w.resource::<Tick>().0;
                let trouble = w.resource::<Decay>().since.get(&id).map(|since| {
                    let days = now.saturating_sub(since.0) as f32 / TICKS_PER_DAY as f32;
                    Trouble {
                        days,
                        days_to_abandon: (bal.abandon_days - days).max(0.0),
                        days_to_demolition: (bal.demolish_days - days).max(0.0),
                    }
                });
                let grids = w.resource::<LandGrids>();
                let svc = w.get_resource::<cp_service::ServiceState>();
                let service = def
                    .service
                    .as_deref()
                    .and_then(|s| db.service_id(s))
                    .map(|s| db.service(s))
                    .filter(|s| s.kind >= cp_content::ServiceKind::Garbage)
                    .map(|s| {
                        let f = svc
                            .and_then(|st| st.facility(id))
                            .copied()
                            .unwrap_or_default();
                        ServiceInfo {
                            kind: s.kind,
                            used_today: f.used_today,
                            capacity: s.capacity,
                            stored: f.stored,
                            storage: s.storage,
                        }
                    });
                let mut problems = flag_names(b.flags);
                if svc
                    .is_some_and(|st| st.problems_of(id).has(cp_service::ServiceProblems::GARBAGE))
                {
                    problems.push("garbage");
                }
                Some(Inspection::Building(BuildingInfo {
                    id,
                    def: def.id.clone(),
                    name: def.name.clone(),
                    zone: def.zone,
                    level: b.level,
                    level_progress: b.level_progress,
                    state: b.state,
                    lot: b.size(),
                    problems,
                    households,
                    apartments: def.households,
                    residents,
                    workers,
                    jobs: def.jobs,
                    trouble,
                    land_value: grids.value_at(b.pos),
                    pollution: grids.pollution_at(b.pos),
                    noise: grids.noise_at(b.pos),
                    tax_base: def.tax_base,
                    footprint: cp_building::held_cells(w.resource::<cp_zone::ZoneGrid>(), b),
                    garbage: svc.and_then(|st| st.get(id)).map_or(0, |r| r.garbage),
                    service,
                }))
            }
            Selection::Citizen(id) => {
                let citizens = w.resource::<CitizenGrid>();
                let c = citizens.citizen(id)?;
                let household_size = citizens
                    .household(c.household)
                    .map_or(0, |h| h.member_ids().count());
                Some(Inspection::Citizen(CitizenInfo {
                    id,
                    age_years: c.age_days / DAYS_PER_YEAR,
                    education: c.education,
                    wellbeing: c.wellbeing,
                    has_car: c.has_car,
                    location: c.location,
                    household_size,
                    home: c.home.and_then(|h| Some((h, building_name(h)?))),
                    work: c
                        .work
                        .and_then(|h| Some((h, building_name(h)?, c.work_level))),
                    trip: trip_info(id),
                }))
            }
            Selection::Pedestrian(key) => {
                let (index, generation) = key_parts(key);
                let p = w
                    .resource::<Agents>()
                    .pedestrian(PedestrianId::new(index, generation))?;
                self.inspect(Selection::Citizen(p.citizen))
            }
            Selection::Vehicle(key) => {
                let (index, generation) = key_parts(key);
                let agents = w.resource::<Agents>();
                let id = VehicleId::new(index, generation);
                let v = agents.vehicle(id)?;
                let net = w.resource::<NetGraph>();
                let lanes: Vec<_> = v.path.lanes().collect();
                // `hop` indexes the lane list: the lane it is on, or (inside a junction) the one
                // it is leaving.
                let at = usize::from(v.hop);
                let at = at.min(lanes.len().saturating_sub(1));
                let route = lanes[at..]
                    .iter()
                    .filter_map(|l| net.lane(*l))
                    .map(|lane| {
                        let mut pts: Vec<Vec3> = Vec::new();
                        for piece in &lane.curve {
                            if pts.is_empty() {
                                pts.push(piece.point(0.0));
                            }
                            for k in 1..=4 {
                                pts.push(piece.point(k as f32 / 4.0));
                            }
                        }
                        pts
                    })
                    .collect();
                Some(Inspection::Vehicle(VehicleInfo {
                    key,
                    kind: v.kind_id(),
                    speed_mps: v.speed,
                    driver: v.citizen,
                    trip: trip_info(v.citizen),
                    lanes_left: lanes.len() - at,
                    lanes_total: lanes.len(),
                    waiting_to_spawn: !v.spawned,
                    route,
                }))
            }
            Selection::Segment(id) => {
                let net = w.resource::<NetGraph>();
                let s = net.segment(id)?;
                let road = db.road(s.road);
                let agents = w.resource::<Agents>();
                let line = (0..=((s.length / 8.0).ceil().max(1.0) as usize))
                    .map(|k| {
                        let n = (s.length / 8.0).ceil().max(1.0);
                        s.curve.point(k as f32 / n)
                    })
                    .collect();
                Some(Inspection::Segment(SegmentInfo {
                    id,
                    road: road.id.clone(),
                    name: road.name.clone(),
                    length: s.length,
                    elevation: s.elevation,
                    lanes: s.lanes.len(),
                    speed_kmh: road.speed_kmh,
                    load: agents.load.inst(id),
                    load_avg: agents
                        .load
                        .ema
                        .get(id.index as usize)
                        .copied()
                        .unwrap_or(0.0),
                    line,
                }))
            }
        }
    }
}

/// The text of a panel: `(label, value)` rows, in the order they are shown. The UI draws
/// exactly these.
impl Inspection {
    pub fn title(&self) -> String {
        match self {
            Inspection::Building(b) => format!("{} (level {})", b.name, b.level),
            Inspection::Citizen(c) => format!("Citizen {}", c.id.index),
            Inspection::Vehicle(v) => format!("Vehicle {}", key_parts(v.key).0),
            Inspection::Segment(s) => format!("{} segment {}", s.name, s.id.index),
        }
    }

    pub fn rows(&self) -> Vec<(String, String)> {
        let row = |l: &str, v: String| (l.to_string(), v);
        let trip = |t: &Option<TripInfo>| -> String {
            t.as_ref().map_or("none".to_string(), |t| {
                format!(
                    "{:?} by {:?}: {} -> {} ({})",
                    t.purpose,
                    t.mode,
                    t.from_name,
                    t.to_name,
                    trip_state(&t.state)
                )
            })
        };
        match self {
            Inspection::Building(b) => {
                let mut r = vec![
                    row("Type", b.def.clone()),
                    row("Zone", format!("{:?}", b.zone)),
                    row("State", format!("{:?}", b.state)),
                    row(
                        "Level",
                        format!("{} (progress {:+.2})", b.level, b.level_progress),
                    ),
                    row("Lot", format!("{} x {}", b.lot.0, b.lot.1)),
                ];
                if b.apartments > 0 {
                    r.push(row(
                        "Households",
                        format!(
                            "{} of {} ({} residents)",
                            b.households, b.apartments, b.residents
                        ),
                    ));
                }
                if b.jobs.iter().any(|&j| j > 0) {
                    let workers: u32 = b.workers.iter().map(|&w| u32::from(w)).sum();
                    let jobs: u32 = b.jobs.iter().map(|&j| u32::from(j)).sum();
                    r.push(row("Workers", format!("{workers} of {jobs}")));
                    for l in 0..4 {
                        if b.jobs[l] > 0 {
                            r.push(row(
                                &format!("  education {l}"),
                                format!("{} of {}", b.workers[l], b.jobs[l]),
                            ));
                        }
                    }
                }
                if let Some(sv) = b.service {
                    r.push(row("Service", format!("{:?}", sv.kind)));
                    r.push(row(
                        "Served today",
                        format!("{} of {}", sv.used_today, sv.capacity),
                    ));
                    if sv.storage > 0 {
                        r.push(row("Stored", format!("{} of {}", sv.stored, sv.storage)));
                    }
                }
                if b.garbage > 0 {
                    r.push(row("Garbage", b.garbage.to_string()));
                }
                r.push(row(
                    "Problems",
                    if b.problems.is_empty() {
                        "none".to_string()
                    } else {
                        b.problems.join(", ")
                    },
                ));
                if let Some(t) = b.trouble {
                    r.push(row(
                        "In trouble for",
                        format!(
                            "{:.1} days (abandoned in {:.1}, demolished in {:.1})",
                            t.days, t.days_to_abandon, t.days_to_demolition
                        ),
                    ));
                }
                r.push(row("Land value", format!("{:.0}", b.land_value)));
                r.push(row("Pollution", format!("{:.0}", b.pollution)));
                r.push(row("Noise", format!("{:.0}", b.noise)));
                r
            }
            Inspection::Citizen(c) => vec![
                row("Age", format!("{} years", c.age_years)),
                row("Education", c.education.to_string()),
                row("Wellbeing", c.wellbeing.to_string()),
                row("Car", if c.has_car { "yes" } else { "no" }.to_string()),
                row("Household", format!("{} people", c.household_size)),
                row("Now", format!("{:?}", c.location)),
                row(
                    "Home",
                    c.home.as_ref().map_or("none".to_string(), |h| h.1.clone()),
                ),
                row(
                    "Work",
                    c.work.as_ref().map_or("none".to_string(), |w| {
                        format!("{} (education {})", w.1, w.2)
                    }),
                ),
                row("Trip", trip(&c.trip)),
            ],
            Inspection::Vehicle(v) => vec![
                row("Speed", format!("{:.1} km/h", v.speed_mps * 3.6)),
                row("Driver", format!("citizen {}", v.driver.index)),
                row("Trip", trip(&v.trip)),
                row(
                    "Route",
                    format!("{} of {} lanes to go", v.lanes_left, v.lanes_total),
                ),
                row(
                    "On the road",
                    if v.waiting_to_spawn {
                        "waiting for room"
                    } else {
                        "yes"
                    }
                    .to_string(),
                ),
            ],
            Inspection::Segment(s) => vec![
                row("Road", s.road.clone()),
                row("Length", format!("{:.0} m", s.length)),
                row("Lanes", s.lanes.to_string()),
                row("Elevation", format!("{:?}", s.elevation)),
                row("Speed limit", format!("{:.0} km/h", s.speed_kmh)),
                row("Load", format!("{:.0}%", s.load * 100.0)),
                row("Load (2 h average)", format!("{:.0}%", s.load_avg * 100.0)),
            ],
        }
    }
}

fn trip_state(s: &TripState) -> String {
    match s {
        TripState::Scheduled { depart_at } => format!("leaves at tick {}", depart_at.0),
        TripState::InTransit { arrive_at } => format!("arrives at tick {}", arrive_at.0),
        TripState::Planned => "planned".to_string(),
        TripState::AwaitingPath { .. } => "finding a route".to_string(),
        TripState::Agent => "under way".to_string(),
        TripState::Transit => "on public transport".to_string(),
    }
}
