//! Player commands of this domain (`architecture/system-design.md` §4, level 2).
//! New variants are only appended (save stability rule). Every command costs nothing; an
//! invalid one is an error and changes nothing.

use std::fmt;

use bevy_ecs::world::World;
use cp_content::{PolicyDef, PolicyKind, SimContent, SpecDef, SpecKind, ZoneType};
use cp_core::Cost;
use serde::{Deserialize, Serialize};

use crate::area::{Area, AreaId, AreaKind, AreaMap, Areas, MAX_AREAS};
use crate::map::{
    Cell, CityPolicies, District, DistrictId, DistrictMap, Districts, MAX_DISTRICTS, NAME_MAX,
    PALETTE_LEN, SIDE, zone_slot,
};

/// District tax override limits, percent (the same as the city rate's).
pub const TAX_MIN: u8 = 1;
pub const TAX_MAX: u8 = 29;
/// Cells one `Paint` / `Erase` may carry.
pub const MAX_CELLS_PER_COMMAND: usize = 4096;

/// Commands handled by this crate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DistrictCommand {
    /// A new district in the lowest free slot, with the palette colour of its id.
    Create {
        name: String,
    },
    Rename {
        district: DistrictId,
        name: String,
    },
    /// Removes the district and clears its cells.
    Delete {
        district: DistrictId,
    },
    /// Gives the cells to `district` (cells of other districts are taken over).
    Paint {
        cells: Vec<Cell>,
        district: DistrictId,
    },
    /// Clears the cells.
    Erase {
        cells: Vec<Cell>,
    },
    /// Sets (`Some`) or removes (`None`) the district's tax rate for a zone type.
    SetTax {
        district: DistrictId,
        zone: ZoneType,
        pct: Option<u8>,
    },
    /// Switches a policy on or off for a district (`Some`) or the whole city (`None`).
    SetPolicy {
        district: Option<DistrictId>,
        policy: PolicyKind,
        on: bool,
    },
    /// Sets (`Some`) or removes (`None`) the district's specialisation for a zone type (T-196).
    SetSpec {
        district: DistrictId,
        zone: ZoneType,
        spec: Option<SpecKind>,
    },
    /// A new area in the lowest free slot, level 1 (T-206).
    CreateArea {
        kind: AreaKind,
        name: String,
    },
    RenameArea {
        area: AreaId,
        name: String,
    },
    /// Removes the area and clears its cells.
    DeleteArea {
        area: AreaId,
    },
    /// Gives the cells to `area` (cells of other areas are taken over; districts are untouched).
    PaintArea {
        cells: Vec<Cell>,
        area: AreaId,
    },
    /// Clears the cells of the area map.
    EraseArea {
        cells: Vec<Cell>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DistrictError {
    /// The world has no district state.
    Missing,
    BadName,
    NoFreeSlot,
    NoSuchDistrict(DistrictId),
    CellOffMap(Cell),
    TooManyCells(usize),
    /// `ZoneType::None` pays no tax.
    NoTaxForZone,
    TaxOutOfRange(u8),
    /// The content has no definition of this policy.
    NoSuchPolicy(PolicyKind),
    /// The policy cannot be switched on here (a city policy in a district or the other way).
    PolicyScope(PolicyKind),
    /// The content has no definition of this specialisation.
    NoSuchSpec(SpecKind),
    /// The specialisation does not apply to this zone type.
    SpecZone(SpecKind, ZoneType),
    NoSuchArea(AreaId),
    NoFreeArea,
}

impl fmt::Display for DistrictError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(f, "no districts in the sim world"),
            Self::BadName => write!(f, "a district name is 1..={NAME_MAX} characters"),
            Self::NoFreeSlot => write!(f, "all {MAX_DISTRICTS} districts exist"),
            Self::NoSuchDistrict(d) => write!(f, "district {} does not exist", d.0),
            Self::CellOffMap((c, r)) => {
                write!(f, "cell ({c}, {r}) is outside the {SIDE}x{SIDE} map")
            }
            Self::TooManyCells(n) => {
                write!(
                    f,
                    "{n} cells in one command (at most {MAX_CELLS_PER_COMMAND})"
                )
            }
            Self::NoTaxForZone => write!(f, "this zone type pays no tax"),
            Self::TaxOutOfRange(p) => {
                write!(f, "tax rate {p}% is outside {TAX_MIN}..={TAX_MAX}%")
            }
            Self::NoSuchPolicy(p) => write!(f, "there is no {p:?} policy"),
            Self::PolicyScope(p) => write!(f, "{p:?} cannot be set here"),
            Self::NoSuchSpec(s) => write!(f, "there is no {s:?} specialisation"),
            Self::SpecZone(s, z) => write!(f, "{s:?} does not apply to {z:?}"),
            Self::NoSuchArea(a) => write!(f, "area {} does not exist", a.0),
            Self::NoFreeArea => write!(f, "all {MAX_AREAS} areas exist"),
        }
    }
}

impl std::error::Error for DistrictError {}

/// A trimmed, non-empty name of at most [`NAME_MAX`] characters.
pub fn valid_name(name: &str) -> Result<String, DistrictError> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > NAME_MAX {
        return Err(DistrictError::BadName);
    }
    Ok(n.to_owned())
}

/// Every cell on the map and not too many.
pub fn validate_cells(cells: &[Cell]) -> Result<(), DistrictError> {
    if cells.len() > MAX_CELLS_PER_COMMAND {
        return Err(DistrictError::TooManyCells(cells.len()));
    }
    match cells
        .iter()
        .find(|&&(c, r)| usize::from(c) >= SIDE || usize::from(r) >= SIDE)
    {
        Some(&bad) => Err(DistrictError::CellOffMap(bad)),
        None => Ok(()),
    }
}

/// `None` is always valid; a rate must be in [`TAX_MIN`]..=[`TAX_MAX`].
pub fn validate_tax(pct: Option<u8>) -> Result<(), DistrictError> {
    match pct {
        Some(p) if !(TAX_MIN..=TAX_MAX).contains(&p) => Err(DistrictError::TaxOutOfRange(p)),
        _ => Ok(()),
    }
}

/// A policy command is valid when the policy is defined and its scope allows the target
/// (`None` = the city); that a district exists is checked by the caller.
pub fn validate_policy(
    def: Option<&PolicyDef>,
    kind: PolicyKind,
    district: Option<DistrictId>,
) -> Result<(), DistrictError> {
    let def = def.ok_or(DistrictError::NoSuchPolicy(kind))?;
    let allowed = match district {
        Some(_) => def.scope.allows_district(),
        None => def.scope.allows_city(),
    };
    if allowed {
        Ok(())
    } else {
        Err(DistrictError::PolicyScope(kind))
    }
}

/// A specialisation is valid for a zone type when it is defined and covers that zone type.
pub fn validate_spec(
    def: Option<&SpecDef>,
    kind: SpecKind,
    zone: ZoneType,
) -> Result<(), DistrictError> {
    let def = def.ok_or(DistrictError::NoSuchSpec(kind))?;
    if def.zones.contains(&zone) {
        Ok(())
    } else {
        Err(DistrictError::SpecZone(kind, zone))
    }
}

/// Validates and applies `cmd`, returning its cost (always zero).
pub fn apply(world: &mut World, cmd: &DistrictCommand) -> Result<Cost, DistrictError> {
    if matches!(
        cmd,
        DistrictCommand::CreateArea { .. }
            | DistrictCommand::RenameArea { .. }
            | DistrictCommand::DeleteArea { .. }
            | DistrictCommand::PaintArea { .. }
            | DistrictCommand::EraseArea { .. }
    ) {
        return apply_area(world, cmd);
    }
    world.resource_scope(|world, mut districts: bevy_ecs::prelude::Mut<Districts>| {
        let mut map = world
            .get_resource_mut::<DistrictMap>()
            .ok_or(DistrictError::Missing)?;
        let exists = |d: DistrictId, ds: &Districts| {
            ds.get(d)
                .map(|_| ())
                .ok_or(DistrictError::NoSuchDistrict(d))
        };
        match cmd {
            DistrictCommand::Create { name } => {
                let name = valid_name(name)?;
                let slot = districts
                    .list
                    .iter()
                    .position(Option::is_none)
                    .unwrap_or(districts.list.len());
                if slot >= MAX_DISTRICTS {
                    return Err(DistrictError::NoFreeSlot);
                }
                let d = District {
                    name,
                    color: (slot % usize::from(PALETTE_LEN)) as u8,
                    tax: [None; 6],
                    policies: 0,
                    spec: [None; 6],
                };
                if slot == districts.list.len() {
                    districts.list.push(Some(d));
                } else {
                    districts.list[slot] = Some(d);
                }
            }
            DistrictCommand::Rename { district, name } => {
                exists(*district, &districts)?;
                let name = valid_name(name)?;
                if let Some(d) = districts.get_mut(*district) {
                    d.name = name;
                }
            }
            DistrictCommand::Delete { district } => {
                exists(*district, &districts)?;
                map.clear_district(*district);
                districts.list[usize::from(district.0) - 1] = None;
                while districts.list.last().is_some_and(Option::is_none) {
                    districts.list.pop();
                }
            }
            DistrictCommand::Paint { cells, district } => {
                exists(*district, &districts)?;
                validate_cells(cells)?;
                for &c in cells {
                    map.set(c, Some(*district));
                }
            }
            DistrictCommand::Erase { cells } => {
                validate_cells(cells)?;
                for &c in cells {
                    map.set(c, None);
                }
            }
            DistrictCommand::SetTax {
                district,
                zone,
                pct,
            } => {
                exists(*district, &districts)?;
                let slot = zone_slot(*zone).ok_or(DistrictError::NoTaxForZone)?;
                validate_tax(*pct)?;
                if let Some(d) = districts.get_mut(*district) {
                    d.tax[slot] = *pct;
                }
            }
            DistrictCommand::SetPolicy {
                district,
                policy,
                on,
            } => {
                if let Some(d) = district {
                    exists(*d, &districts)?;
                }
                let def = world
                    .get_resource::<SimContent>()
                    .and_then(|c| c.0.policy(*policy));
                validate_policy(def, *policy, *district)?;
                let bit = policy.bit();
                let set = |mask: &mut u32| {
                    if *on {
                        *mask |= bit;
                    } else {
                        *mask &= !bit;
                    }
                };
                match district {
                    Some(d) => {
                        if let Some(d) = districts.get_mut(*d) {
                            set(&mut d.policies);
                        }
                    }
                    None => {
                        let mut city = world
                            .get_resource_mut::<CityPolicies>()
                            .ok_or(DistrictError::Missing)?;
                        set(&mut city.mask);
                    }
                }
            }
            DistrictCommand::SetSpec {
                district,
                zone,
                spec,
            } => {
                exists(*district, &districts)?;
                let slot = zone_slot(*zone).ok_or(DistrictError::NoTaxForZone)?;
                if let Some(kind) = spec {
                    let def = world
                        .get_resource::<SimContent>()
                        .and_then(|c| c.0.spec(*kind));
                    validate_spec(def, *kind, *zone)?;
                }
                if let Some(d) = districts.get_mut(*district) {
                    d.spec[slot] = *spec;
                }
            }
            DistrictCommand::CreateArea { .. }
            | DistrictCommand::RenameArea { .. }
            | DistrictCommand::DeleteArea { .. }
            | DistrictCommand::PaintArea { .. }
            | DistrictCommand::EraseArea { .. } => unreachable!("handled by apply_area"),
        }
        Ok(Cost::ZERO)
    })
}

/// The area commands: validate, then change the area map and records.
fn apply_area(world: &mut World, cmd: &DistrictCommand) -> Result<Cost, DistrictError> {
    world.resource_scope(|world, mut areas: bevy_ecs::prelude::Mut<Areas>| {
        let mut map = world
            .get_resource_mut::<AreaMap>()
            .ok_or(DistrictError::Missing)?;
        let exists =
            |a: AreaId, areas: &Areas| areas.get(a).map(|_| ()).ok_or(DistrictError::NoSuchArea(a));
        match cmd {
            DistrictCommand::CreateArea { kind, name } => {
                let name = valid_name(name)?;
                let slot = areas
                    .list
                    .iter()
                    .position(Option::is_none)
                    .unwrap_or(areas.list.len());
                if slot >= MAX_AREAS {
                    return Err(DistrictError::NoFreeArea);
                }
                let a = Area {
                    name,
                    kind: *kind,
                    level: 1,
                    xp: 0,
                };
                if slot == areas.list.len() {
                    areas.list.push(Some(a));
                } else {
                    areas.list[slot] = Some(a);
                }
            }
            DistrictCommand::RenameArea { area, name } => {
                exists(*area, &areas)?;
                let name = valid_name(name)?;
                if let Some(a) = areas.get_mut(*area) {
                    a.name = name;
                }
            }
            DistrictCommand::DeleteArea { area } => {
                exists(*area, &areas)?;
                map.clear_area(*area);
                areas.list[usize::from(area.0) - 1] = None;
                while areas.list.last().is_some_and(Option::is_none) {
                    areas.list.pop();
                }
            }
            DistrictCommand::PaintArea { cells, area } => {
                exists(*area, &areas)?;
                validate_cells(cells)?;
                for &c in cells {
                    map.set(c, Some(*area));
                }
            }
            DistrictCommand::EraseArea { cells } => {
                validate_cells(cells)?;
                for &c in cells {
                    map.set(c, None);
                }
            }
            _ => unreachable!("not an area command"),
        }
        Ok(Cost::ZERO)
    })
}
