//! Warehouses (T-214, `architecture/system-design.md` §16 M5 (D) «Склады»): how each takes in
//! and gives out, and the command that sets it.
//!
//! * **Fill** takes cargo in and never gives it out.
//! * **Empty** takes nothing in; it gives its cargo to processors that need it and sends the
//!   rest out of the map.
//! * **Balanced** keeps about half full: it takes cargo in while it is under half, and gives
//!   it to processors that need it while it is over half (it never exports).
//!
//! A warehouse is a target of a load before the export, and a source for a processor before
//! the import. With a cargo chosen it handles that cargo only; without one, every cargo of its
//! chain.

use std::collections::BTreeMap;
use std::fmt;

use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use cp_building::{BuildingGrid, BuildingId};
use cp_content::{CargoKind, ContentDb, IndustryRole, ResourceKind, SimContent};
use cp_core::Cost;
use serde::{Deserialize, Serialize};

/// How a warehouse takes in and gives out. Append-only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WarehouseMode {
    #[default]
    Balanced,
    Fill,
    Empty,
}

/// The setting of one warehouse.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WarehouseSetting {
    /// The one cargo it handles; `None` = all cargo of its chain.
    pub cargo: Option<CargoKind>,
    pub mode: WarehouseMode,
}

impl WarehouseSetting {
    /// Whether it handles `cargo` at all.
    pub fn handles(&self, cargo: CargoKind) -> bool {
        self.cargo.is_none_or(|c| c == cargo)
    }

    /// Whether it takes a load in, given what it holds (`held`, all cargo) and its `cap`.
    pub fn accepts(&self, cargo: CargoKind, held: f32, cap: f32) -> bool {
        self.handles(cargo)
            && match self.mode {
                WarehouseMode::Fill => true,
                WarehouseMode::Empty => false,
                WarehouseMode::Balanced => held < cap * 0.5,
            }
    }

    /// The setting it works by while `cargo` is or is not in a shortage (§21.1 item 3, T-320b):
    /// in a shortage a `Fill` warehouse gives like a `Balanced` one, the dry spell it was
    /// filled for; otherwise it is itself.
    pub fn in_market(self, shortage: bool) -> Self {
        if shortage && self.mode == WarehouseMode::Fill {
            Self {
                mode: WarehouseMode::Balanced,
                ..self
            }
        } else {
            self
        }
    }

    /// Whether it gives `cargo` out, given what it holds (`held`, all cargo) and its `cap`.
    pub fn gives(&self, cargo: CargoKind, held: f32, cap: f32) -> bool {
        self.handles(cargo)
            && match self.mode {
                WarehouseMode::Fill => false,
                WarehouseMode::Empty => true,
                WarehouseMode::Balanced => held > cap * 0.5,
            }
    }
}

/// The settings of the warehouses that have been set; the others are `Balanced`, any cargo.
/// Saved and in the checksum once non-empty.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WarehouseSettings {
    pub list: BTreeMap<BuildingId, WarehouseSetting>,
}

impl WarehouseSettings {
    pub fn get(&self, building: BuildingId) -> WarehouseSetting {
        self.list.get(&building).copied().unwrap_or_default()
    }
}

/// Commands handled by this crate. Variants are only appended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum IndustryCommand {
    /// Sets the mode and the cargo of a warehouse.
    SetWarehouse {
        building: BuildingId,
        cargo: Option<CargoKind>,
        mode: WarehouseMode,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IndustryError {
    /// The world has no content or buildings.
    Missing,
    NoSuchBuilding,
    NotAWarehouse,
    /// The cargo is not one of the warehouse's chain.
    WrongCargo(CargoKind),
}

impl fmt::Display for IndustryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(f, "no content in the sim world"),
            Self::NoSuchBuilding => write!(f, "no such building"),
            Self::NotAWarehouse => write!(f, "this building is not a warehouse"),
            Self::WrongCargo(c) => write!(f, "{c:?} is not a cargo of this warehouse's chain"),
        }
    }
}

impl std::error::Error for IndustryError {}

/// The cargo of a resource's chain: everything its buildings take in or put out.
pub fn chain_cargo(db: &ContentDb, resource: Option<ResourceKind>) -> Vec<CargoKind> {
    let mut out: Vec<CargoKind> = db
        .buildings()
        .filter_map(|(_, d)| d.industry.as_ref().filter(|i| i.resource == resource))
        .flat_map(|i| i.inputs.iter().chain(&i.outputs).map(|&(c, _)| c))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Validates and applies `cmd`; every command is free.
pub fn apply(world: &mut World, cmd: &IndustryCommand) -> Result<Cost, IndustryError> {
    let IndustryCommand::SetWarehouse {
        building,
        cargo,
        mode,
    } = cmd;
    let db = world
        .get_resource::<SimContent>()
        .map(|c| c.0.clone())
        .ok_or(IndustryError::Missing)?;
    let def = world
        .get_resource::<BuildingGrid>()
        .ok_or(IndustryError::Missing)?
        .get(*building)
        .map(|b| b.def)
        .ok_or(IndustryError::NoSuchBuilding)?;
    let spec = db
        .building(def)
        .industry
        .as_ref()
        .filter(|i| i.role == IndustryRole::Warehouse)
        .ok_or(IndustryError::NotAWarehouse)?;
    if let Some(c) = cargo
        && !chain_cargo(&db, spec.resource).contains(c)
    {
        return Err(IndustryError::WrongCargo(*c));
    }
    let setting = WarehouseSetting {
        cargo: *cargo,
        mode: *mode,
    };
    let mut settings = world
        .get_resource_mut::<WarehouseSettings>()
        .ok_or(IndustryError::Missing)?;
    if setting == WarehouseSetting::default() {
        settings.list.remove(building);
    } else {
        settings.list.insert(*building, setting);
    }
    Ok(Cost::ZERO)
}
