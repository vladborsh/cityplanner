//! Player commands of this domain (`architecture/system-design.md` §4, level 2). Variants are
//! only appended. A command is validated whole before it changes anything and is charged
//! once: new stops come into being together with the line that names them.

use std::fmt;

use bevy_ecs::world::World;
use cp_building::BuildingId;
use cp_content::SimContent;
use cp_core::treasury::{charge_funds, check_funds};
use cp_core::{Cost, InsufficientFunds};
use cp_net::{NetGraph, SegmentId};
use cp_traffic::{PathNetwork, Side};
use serde::{Deserialize, Serialize};

use crate::model::{
    DEFAULT_LINE_VEHICLES, Line, LineId, LineState, MAX_LINE_STOPS, MAX_LINE_VEHICLES, PALETTE_LEN,
    Stop, StopId, StopKind, TransitMode, TransitNet,
};
use crate::plan::LinePlans;
use crate::rules::{StopError, check_road_stop};
use crate::sync::replan_line;

/// Longest line name, characters.
pub const MAX_LINE_NAME: usize = 24;

/// A stop named by a command.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum StopRef {
    Existing(StopId),
    /// A new bus stop `s` metres along `seg` on `side`.
    NewRoad {
        seg: SegmentId,
        s: f32,
        side: Side,
    },
    /// The stop of a station building (T-162).
    Station(BuildingId),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum TransitCommand {
    CreateLine {
        mode: TransitMode,
        stops: Vec<StopRef>,
    },
    /// Replaces the stops of a line.
    EditLine {
        line: LineId,
        stops: Vec<StopRef>,
    },
    DeleteLine {
        line: LineId,
    },
    SetLineVehicles {
        line: LineId,
        n: u8,
    },
    SetLineColor {
        line: LineId,
        color: u8,
    },
    /// Removes a stop from the map and from its lines (the bulldozer).
    RemoveStop {
        stop: StopId,
    },
    /// Gives a line a new name (T-160): 1..=[`MAX_LINE_NAME`] characters after trimming.
    RenameLine {
        line: LineId,
        name: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum TransitError {
    /// The world has no transit state or no content.
    Missing,
    NoSuchLine(LineId),
    NoSuchStop(StopId),
    /// A line has 2..=[`MAX_LINE_STOPS`] stops.
    StopCount(usize),
    /// The same stop twice in one line.
    StopRepeated(StopId),
    /// Two new stops in one command too close to each other.
    NewStopsTooClose,
    /// A stop that does not fit the line's mode (a road stop on a metro line, say).
    WrongStopKind,
    /// That building has no station stop.
    NoStation(BuildingId),
    /// The mode's depot (or station and track) is not built yet (T-302, station first).
    NotReady(TransitMode),
    /// The mode has no lines yet (intercity trains come later).
    ModeNotAvailable(TransitMode),
    Stop(StopError),
    Vehicles(u8),
    Color(u8),
    /// Empty or longer than [`MAX_LINE_NAME`] characters.
    Name,
    Funds(InsufficientFunds),
}

impl fmt::Display for TransitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(f, "no transit in the sim world"),
            Self::NoSuchLine(_) => write!(f, "no such line"),
            Self::NoSuchStop(_) => write!(f, "no such stop"),
            Self::StopCount(n) => write!(f, "a line has 2..={MAX_LINE_STOPS} stops, not {n}"),
            Self::StopRepeated(_) => write!(f, "a stop is on the line twice"),
            Self::NewStopsTooClose => write!(f, "new stops are too close to each other"),
            Self::WrongStopKind => write!(f, "the stop does not fit the line's kind"),
            Self::NoStation(_) => write!(f, "that building is not a station"),
            Self::ModeNotAvailable(m) => write!(f, "no {m:?} lines yet"),
            Self::NotReady(m) => write!(f, "{m:?} lines need their depot (and track) first"),
            Self::Stop(e) => write!(f, "{e}"),
            Self::Vehicles(n) => write!(f, "{n} vehicles: at most {MAX_LINE_VEHICLES}"),
            Self::Color(c) => write!(f, "colour {c}: the palette has {PALETTE_LEN}"),
            Self::Name => write!(f, "a line name has 1..={MAX_LINE_NAME} characters"),
            Self::Funds(e) => write!(
                f,
                "insufficient funds: need {}, have {}",
                e.needed.0, e.available.0
            ),
        }
    }
}

impl std::error::Error for TransitError {}

impl From<StopError> for TransitError {
    fn from(e: StopError) -> Self {
        Self::Stop(e)
    }
}

impl TransitError {
    pub fn insufficient_funds(&self) -> Option<InsufficientFunds> {
        match self {
            Self::Funds(f) => Some(*f),
            _ => None,
        }
    }
}

/// A new stop to create, after validation.
struct NewStop {
    seg: SegmentId,
    s: f32,
    side: Side,
    pos: bevy_math::Vec2,
}

/// What `stops` resolve to: the final stop list, with the stops to create (indexed by
/// `Err(i)` placeholders) — validation only, nothing is changed.
enum Resolved {
    Old(StopId),
    New(usize),
}

fn resolve(
    net: &NetGraph,
    db: &cp_content::ContentDb,
    transit: &TransitNet,
    mode: TransitMode,
    refs: &[StopRef],
) -> Result<(Vec<Resolved>, Vec<NewStop>), TransitError> {
    if !(2..=MAX_LINE_STOPS).contains(&refs.len()) {
        return Err(TransitError::StopCount(refs.len()));
    }
    let mut out = Vec::with_capacity(refs.len());
    let mut fresh: Vec<NewStop> = Vec::new();
    // The stops the command creates count as existing for the rule of the next one.
    let mut scratch = transit.stops.clone();
    for r in refs {
        let existing = match *r {
            StopRef::Existing(id) => Some(id),
            StopRef::Station(b) => {
                Some(crate::station::station_stop(transit, b).ok_or(TransitError::NoStation(b))?)
            }
            StopRef::NewRoad { .. } => None,
        };
        match *r {
            _ if existing.is_some() => {
                let id = existing.expect("checked");
                let st = transit.stops.get(id).ok_or(TransitError::NoSuchStop(id))?;
                if !kind_fits(mode, &st.kind) {
                    return Err(TransitError::WrongStopKind);
                }
                if out
                    .iter()
                    .any(|o| matches!(o, Resolved::Old(x) if *x == id))
                {
                    return Err(TransitError::StopRepeated(id));
                }
                out.push(Resolved::Old(id));
            }
            StopRef::NewRoad { seg, s, side } => {
                if !mode.on_road() {
                    return Err(TransitError::WrongStopKind);
                }
                let pos = check_road_stop(net, db, &scratch, mode.class(), seg, s, side, None)
                    .map_err(|e| match e {
                        StopError::NearStop(id) if !transit.stops.contains(id) => {
                            TransitError::NewStopsTooClose
                        }
                        e => e.into(),
                    })?;
                scratch.insert(Stop {
                    pos,
                    side,
                    kind: StopKind::Road { seg, s },
                    lines: Vec::new(),
                });
                out.push(Resolved::New(fresh.len()));
                fresh.push(NewStop { seg, s, side, pos });
            }
            _ => unreachable!("a station and an existing stop are resolved above"),
        }
    }
    Ok((out, fresh))
}

fn kind_fits(mode: TransitMode, kind: &StopKind) -> bool {
    match kind {
        StopKind::Road { .. } => mode.on_road(),
        StopKind::Station { .. } => matches!(mode, TransitMode::Metro | TransitMode::Train),
    }
}

fn price(db: &cp_content::ContentDb, new_stops: usize) -> Cost {
    Cost::scaled(db.balance().bus_stop_cost, new_stops as f64)
}

/// Creates the new stops and returns the ids of the final list.
fn commit_stops(
    transit: &mut TransitNet,
    resolved: Vec<Resolved>,
    fresh: Vec<NewStop>,
) -> Vec<StopId> {
    let made: Vec<StopId> = fresh
        .into_iter()
        .map(|n| {
            transit.stops.insert(Stop {
                pos: n.pos,
                side: n.side,
                kind: StopKind::Road { seg: n.seg, s: n.s },
                lines: Vec::new(),
            })
        })
        .collect();
    resolved
        .into_iter()
        .map(|r| match r {
            Resolved::Old(id) => id,
            Resolved::New(i) => made[i],
        })
        .collect()
}

/// Validates and applies `cmd`; gives what it charged.
pub fn apply(world: &mut World, cmd: &TransitCommand) -> Result<Cost, TransitError> {
    if !world.contains_resource::<TransitNet>() || !world.contains_resource::<SimContent>() {
        return Err(TransitError::Missing);
    }
    let content = world.resource::<SimContent>().0.clone();
    let db = &*content;
    world.resource_scope(|world, mut transit: bevy_ecs::prelude::Mut<TransitNet>| {
        let net = world
            .get_resource::<NetGraph>()
            .ok_or(TransitError::Missing)?;
        let (cost, touched) = match cmd {
            TransitCommand::CreateLine { mode, stops } => {
                if *mode == TransitMode::Train {
                    return Err(TransitError::ModeNotAvailable(*mode));
                }
                // T-302: station first — the depot (and the track) before the line.
                if let Some(buildings) = world.get_resource::<cp_building::BuildingGrid>()
                    && !crate::rules::prerequisites(db, buildings, net)[mode.kind() as usize].ok
                {
                    return Err(TransitError::NotReady(*mode));
                }
                let (resolved, fresh) = resolve(net, db, &transit, *mode, stops)?;
                let cost = price(db, fresh.len());
                check_funds(world, cost).map_err(TransitError::Funds)?;
                let ids = commit_stops(&mut transit, resolved, fresh);
                let n = transit.lines.len() + 1;
                let line = transit.lines.insert(Line {
                    mode: *mode,
                    name: format!(
                        "{} {n}",
                        match mode {
                            TransitMode::Metro => "Metro",
                            TransitMode::Tram => "Tram",
                            _ => "Bus",
                        }
                    ),
                    color: ((n - 1) % usize::from(PALETTE_LEN)) as u8,
                    stops: ids,
                    vehicles: DEFAULT_LINE_VEHICLES,
                    state: LineState::Ok,
                });
                (cost, Some(line))
            }
            TransitCommand::EditLine { line, stops } => {
                let mode = transit
                    .lines
                    .get(*line)
                    .ok_or(TransitError::NoSuchLine(*line))?
                    .mode;
                let (resolved, fresh) = resolve(net, db, &transit, mode, stops)?;
                let cost = price(db, fresh.len());
                check_funds(world, cost).map_err(TransitError::Funds)?;
                let ids = commit_stops(&mut transit, resolved, fresh);
                transit.lines.get_mut(*line).expect("checked").stops = ids;
                (cost, Some(*line))
            }
            TransitCommand::DeleteLine { line } => {
                transit
                    .lines
                    .remove(*line)
                    .ok_or(TransitError::NoSuchLine(*line))?;
                (Cost::ZERO, None)
            }
            TransitCommand::SetLineVehicles { line, n } => {
                if *n > MAX_LINE_VEHICLES {
                    return Err(TransitError::Vehicles(*n));
                }
                transit
                    .lines
                    .get_mut(*line)
                    .ok_or(TransitError::NoSuchLine(*line))?
                    .vehicles = *n;
                (Cost::ZERO, None)
            }
            TransitCommand::SetLineColor { line, color } => {
                if *color >= PALETTE_LEN {
                    return Err(TransitError::Color(*color));
                }
                transit
                    .lines
                    .get_mut(*line)
                    .ok_or(TransitError::NoSuchLine(*line))?
                    .color = *color;
                (Cost::ZERO, None)
            }
            TransitCommand::RenameLine { line, name } => {
                let name = name.trim();
                if name.is_empty() || name.chars().count() > MAX_LINE_NAME {
                    return Err(TransitError::Name);
                }
                transit
                    .lines
                    .get_mut(*line)
                    .ok_or(TransitError::NoSuchLine(*line))?
                    .name = name.to_string();
                (Cost::ZERO, None)
            }
            TransitCommand::RemoveStop { stop } => {
                if !transit.stops.contains(*stop) {
                    return Err(TransitError::NoSuchStop(*stop));
                }
                transit.remove_stop(*stop);
                (Cost::ZERO, None)
            }
        };
        transit.relink_stops();
        // Plans follow the stops at once, so the answer is right in the same tick.
        let graph = world
            .get_resource::<PathNetwork>()
            .map(|p| p.graph().clone());
        let mut plans = world.get_resource_mut::<LinePlans>();
        if let (Some(plans), Some(graph)) = (plans.as_mut(), graph.as_ref()) {
            if let Some(line) = touched {
                let plan = replan_line(&mut transit, graph, line);
                plans.by_line.insert(line, plan);
            }
            plans.by_line.retain(|id, _| transit.lines.contains(*id));
            plans.dirty = true;
        }
        charge_funds(world, cost);
        Ok(cost)
    })
}
