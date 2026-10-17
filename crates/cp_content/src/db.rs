//! Loading, validation, hashing and lookup of the content set.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::defs::*;
use crate::error::{ContentError, ContentErrors};
use crate::kinds::{CargoKind, IndustryRole, PolicyKind, SpecKind, ZoneType};
use crate::validate;

/// Default content root, relative to the working directory.
pub const DEFAULT_CONTENT_ROOT: &str = "assets/content";

/// A minimal style with id `p`, for tests that build a content set from inline defs. Not
/// part of the shipped content.
#[doc(hidden)]
pub const TEST_STYLE_RON: &str = r#"Style((id: "p", massing: Box, roof: Flat, floors: (1, 1),
    footprint_fill: (1.0, 1.0), facade: Punched,
    palette: (walls: [(0.5, 0.5, 0.5)], roof: [(0.3, 0.3, 0.3)], trim: [(0.4, 0.4, 0.4)],
        glass: (0.2, 0.2, 0.3))))"#;

/// A `(path, RON)` source of a policy of `kind` (a `PolicyKind` variant name) with effect
/// `value`, switchable for the city and districts, for tests that build content from inline defs.
pub fn test_policy_source(kind: &str, value: f32) -> (String, String) {
    (
        format!("p/{kind}.ron"),
        format!(
            r#"Policy((id: "{kind}", name: "{kind}", kind: {kind}, icon: "i", scope: Both,
            cost_per_building_week: 1.0, value: {value:?}))"#
        ),
    )
}

/// FNV-1a 64-bit: fixed algorithm, stable across runs, platforms and Rust releases.
fn fnv1a(hash: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *hash ^= u64::from(*b);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

/// One definition table: defs in sorted-id order plus the string-id lookup.
#[derive(Clone, Debug, PartialEq)]
struct Table<T> {
    defs: Vec<T>,
    by_id: BTreeMap<String, u32>,
}

impl<T> Default for Table<T> {
    fn default() -> Self {
        Self {
            defs: Vec::new(),
            by_id: BTreeMap::new(),
        }
    }
}

impl<T> Table<T> {
    fn get_id(&self, s: &str) -> Option<u32> {
        self.by_id.get(s).copied()
    }
}

/// The content set as sim-world state, for command handlers. Not in saves (saves keep
/// only the content hash).
#[derive(bevy_ecs::resource::Resource, Clone)]
pub struct SimContent(pub std::sync::Arc<ContentDb>);

/// Which service kinds the city can use yet (BUG-051): a need whose remedy is locked by a
/// population milestone is not generated at all. Derived from `cp_sim`'s `Progress` (never
/// saved); the default has everything open, so worlds built in code have nothing locked.
#[derive(bevy_ecs::resource::Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceUnlocks {
    /// Bit `ServiceKind as u8` set = open.
    pub open_mask: u32,
}

impl Default for ServiceUnlocks {
    fn default() -> Self {
        Self {
            open_mask: u32::MAX,
        }
    }
}

impl ServiceUnlocks {
    pub fn open(&self, kind: crate::kinds::ServiceKind) -> bool {
        self.open_mask & (1 << (kind as u8)) != 0
    }

    pub fn set(&mut self, kind: crate::kinds::ServiceKind, open: bool) {
        let bit = 1u32 << (kind as u8);
        if open {
            self.open_mask |= bit;
        } else {
            self.open_mask &= !bit;
        }
    }
}

/// Which definition table a placeholder belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DefKind {
    Road,
    Building,
    Service,
    Vehicle,
    Utility,
}

impl DefKind {
    pub const ALL: [DefKind; 5] = [
        DefKind::Road,
        DefKind::Building,
        DefKind::Service,
        DefKind::Vehicle,
        DefKind::Utility,
    ];

    /// Lower-case name for messages.
    pub fn label(self) -> &'static str {
        match self {
            DefKind::Road => "road",
            DefKind::Building => "building",
            DefKind::Service => "service",
            DefKind::Vehicle => "vehicle",
            DefKind::Utility => "utility",
        }
    }
}

/// The index every `*DefId` of an unknown definition gets: past the end of every table, so it
/// never collides with a real definition. The accessors (`road(id)`, ...) answer it with the
/// built-in placeholder; `roads()` and the other iterators never list it.
pub const PLACEHOLDER_INDEX: u32 = u32::MAX;

/// Built-in stand-ins for definitions a save names but the loaded content does not have
/// (`system-design.md` §12). Not part of the content hash.
#[derive(Clone, Debug, PartialEq)]
struct Placeholders {
    road: RoadDef,
    building: BuildingDef,
    service: ServiceDef,
    vehicle: VehicleDef,
    utility: UtilityDef,
    style: StyleDef,
}

impl Placeholders {
    fn builtin() -> Self {
        fn parse(text: &str) -> ContentFile {
            ron::from_str(text).expect("built-in placeholder definition")
        }
        let ContentFile::Road(road) = parse(
            r#"Road((id: "placeholder_road", name: "Missing road type", width: 12.0,
                lanes: [
                    (kind: Pedestrian, dir: Forward, width: 2.5, offset: 4.75),
                    (kind: Car, dir: Backward, width: 3.5, offset: 1.75),
                    (kind: Car, dir: Forward, width: 3.5, offset: -1.75),
                    (kind: Pedestrian, dir: Forward, width: 2.5, offset: -4.75),
                ],
                median_width: 0.0, speed_kmh: 40, max_slope: 0.12, cost_per_m: 10.0,
                upkeep_per_m_week: 0.1, allows_zoning: true, rank: 1, elevated: None,
                bridge: None, mesh: "road/basic_2l", material: "road/asphalt"))"#,
        ) else {
            unreachable!()
        };
        let ContentFile::Building(building) = parse(
            r#"Building((id: "placeholder_building", name: "Missing building", zone: None,
                level: 1, lot: (1, 1), consumption: {}, tax_base: 0.0,
                styles: ["placeholder_style"], service: None))"#,
        ) else {
            unreachable!()
        };
        let ContentFile::Service(service) = parse(
            r#"Service((id: "placeholder_service", name: "Missing service", kind: Roads,
                radius_m: 0.0, capacity: 0, vehicles: []))"#,
        ) else {
            unreachable!()
        };
        let ContentFile::Vehicle(vehicle) = parse(
            r#"Vehicle((id: "placeholder_vehicle", name: "Missing vehicle", kind: Car,
                length: 4.2, width: 1.8, max_speed_kmh: 120.0, accel: 2.0, decel: 4.0,
                min_gap: 2.0, mesh: "vehicle/car_basic"))"#,
        ) else {
            unreachable!()
        };
        let ContentFile::Utility(utility) = parse(
            r#"Utility((id: "placeholder_utility", name: "Missing pipe", kind: Water,
                cost_per_m: 0.0, upkeep_per_m_week: 0.0, min_link_m: 4.0, max_link_m: 100.0,
                mesh: "utility/water_pipe"))"#,
        ) else {
            unreachable!()
        };
        let ContentFile::Style(style) = parse(
            r#"Style((id: "placeholder_style", massing: Box, roof: Flat, floors: (1, 1),
                footprint_fill: (1.0, 1.0), facade: Punched,
                palette: (walls: [(0.6, 0.6, 0.6)], roof: [(0.35, 0.35, 0.35)],
                    trim: [(0.4, 0.4, 0.4)], glass: (0.2, 0.25, 0.3))))"#,
        ) else {
            unreachable!()
        };
        Self {
            road,
            building,
            service,
            vehicle,
            utility,
            style,
        }
    }
}

/// The validated, immutable content set.
#[derive(Clone, Debug, PartialEq)]
pub struct ContentDb {
    roads: Table<RoadDef>,
    buildings: Table<BuildingDef>,
    services: Table<ServiceDef>,
    vehicles: Table<VehicleDef>,
    utilities: Table<UtilityDef>,
    styles: Table<StyleDef>,
    placeholders: Placeholders,
    balance: Balance,
    milestones: Vec<MilestoneDef>,
    /// The policies, one per kind at most, in kind order (T-190).
    policies: Vec<PolicyDef>,
    /// The specialisations, one per kind at most, in kind order (T-196).
    specs: Vec<SpecDef>,
    /// The cargo definitions, one per kind at most, in kind order (T-208).
    cargos: Vec<CargoDef>,
    content_hash: u64,
}

macro_rules! accessors {
    ($field:ident, $ph:ident, $def:ty, $id:ident, $get:ident, $by:ident, $iter:ident) => {
        /// The definition, or the built-in placeholder for [`PLACEHOLDER_INDEX`].
        pub fn $get(&self, id: $id) -> &$def {
            self.$field
                .defs
                .get(id.index())
                .unwrap_or(&self.placeholders.$ph)
        }
        pub fn $by(&self, name: &str) -> Option<$id> {
            self.$field.get_id(name).map($id)
        }
        /// All definitions in id order.
        pub fn $iter(&self) -> impl Iterator<Item = ($id, &$def)> {
            self.$field
                .defs
                .iter()
                .enumerate()
                .map(|(i, d)| ($id(i as u32), d))
        }
    };
}

impl ContentDb {
    accessors!(roads, road, RoadDef, RoadDefId, road, road_id, roads);
    accessors!(
        buildings,
        building,
        BuildingDef,
        BuildingDefId,
        building,
        building_id,
        buildings
    );
    accessors!(
        services,
        service,
        ServiceDef,
        ServiceDefId,
        service,
        service_id,
        services
    );
    accessors!(
        vehicles,
        vehicle,
        VehicleDef,
        VehicleDefId,
        vehicle,
        vehicle_id,
        vehicles
    );
    accessors!(
        utilities,
        utility,
        UtilityDef,
        UtilityDefId,
        utility,
        utility_id,
        utilities
    );

    /// The style, or the built-in grey one for [`PLACEHOLDER_INDEX`].
    pub fn style(&self, id: StyleDefId) -> &StyleDef {
        self.styles
            .defs
            .get(id.index())
            .unwrap_or(&self.placeholders.style)
    }
    pub fn style_id(&self, name: &str) -> Option<StyleDefId> {
        self.styles.get_id(name).map(StyleDefId)
    }
    /// All styles in id order.
    pub fn styles(&self) -> impl Iterator<Item = (StyleDefId, &StyleDef)> {
        self.styles
            .defs
            .iter()
            .enumerate()
            .map(|(i, d)| (StyleDefId(i as u32), d))
    }

    /// The `*DefId` index of the built-in placeholder of `kind` ([`PLACEHOLDER_INDEX`]): what a
    /// save's unknown definition name becomes on load.
    pub fn placeholder(kind: DefKind) -> u32 {
        let _ = kind;
        PLACEHOLDER_INDEX
    }

    /// The string ids of one table in id order: the names a save records for the dense indices.
    pub fn def_names(&self, kind: DefKind) -> Vec<String> {
        match kind {
            DefKind::Road => self.roads.defs.iter().map(|d| d.id.clone()).collect(),
            DefKind::Building => self.buildings.defs.iter().map(|d| d.id.clone()).collect(),
            DefKind::Service => self.services.defs.iter().map(|d| d.id.clone()).collect(),
            DefKind::Vehicle => self.vehicles.defs.iter().map(|d| d.id.clone()).collect(),
            DefKind::Utility => self.utilities.defs.iter().map(|d| d.id.clone()).collect(),
        }
    }

    /// The index of definition `name` in `kind`'s table, if the content has it.
    pub fn def_index(&self, kind: DefKind, name: &str) -> Option<u32> {
        match kind {
            DefKind::Road => self.roads.get_id(name),
            DefKind::Building => self.buildings.get_id(name),
            DefKind::Service => self.services.get_id(name),
            DefKind::Vehicle => self.vehicles.get_id(name),
            DefKind::Utility => self.utilities.get_id(name),
        }
    }

    /// The utility definition for `kind`, if the content has one. There is one pipe for water
    /// and sewage (`system-design.md` §10): `Sewage` is the same definition as `Water`.
    pub fn utility_for_kind(
        &self,
        kind: crate::kinds::UtilityKind,
    ) -> Option<(UtilityDefId, &UtilityDef)> {
        let kind = match kind {
            crate::kinds::UtilityKind::Sewage => crate::kinds::UtilityKind::Water,
            k => k,
        };
        self.utilities().find(|(_, d)| d.kind == kind)
    }

    /// The policies in kind order (at most one per kind).
    pub fn policies(&self) -> &[PolicyDef] {
        &self.policies
    }

    /// The specialisations in kind order (at most one per kind).
    pub fn specs(&self) -> &[SpecDef] {
        &self.specs
    }

    /// The cargo definitions in kind order (at most one per kind).
    pub fn cargos(&self) -> &[CargoDef] {
        &self.cargos
    }

    /// The definition of the cargo `kind`, if the content has one.
    pub fn cargo(&self, kind: CargoKind) -> Option<&CargoDef> {
        self.cargos.iter().find(|c| c.kind == kind)
    }

    /// The definition of the specialisation `kind`, if the content has one.
    pub fn spec(&self, kind: SpecKind) -> Option<&SpecDef> {
        self.specs.iter().find(|s| s.kind == kind)
    }

    /// The definition of `kind`, if the content has one.
    pub fn policy(&self, kind: PolicyKind) -> Option<&PolicyDef> {
        self.policies.iter().find(|p| p.kind == kind)
    }

    /// The population milestones (`milestones.ron`); empty when the file is absent, and then
    /// nothing is locked.
    pub fn milestones(&self) -> &[MilestoneDef] {
        &self.milestones
    }

    /// The milestone (index into [`Self::milestones`]) that unlocks the building def `id`;
    /// `None` for a def no milestone names, which is always available.
    pub fn unlock_milestone(&self, id: BuildingDefId) -> Option<usize> {
        let name = &self.buildings.defs.get(id.index())?.id;
        self.milestones
            .iter()
            .position(|m| m.unlocks.iter().any(|u| u == name))
    }

    /// The milestone (index into [`Self::milestones`]) that opens painting `zone`; `None` for a
    /// zone type no milestone names, which is always open (T-231).
    pub fn zone_unlock_milestone(&self, zone: ZoneType) -> Option<usize> {
        self.milestones.iter().position(|m| m.zones.contains(&zone))
    }

    /// The balance knobs (`balance.ron`; defaults when the file is absent).
    pub fn balance(&self) -> &Balance {
        &self.balance
    }

    /// Stable hash of the semantic content (independent of file layout, formatting,
    /// line endings and directory iteration order).
    pub fn content_hash(&self) -> u64 {
        self.content_hash
    }

    /// Loads every `**/*.ron` under `root`.
    pub fn load_dir(root: impl AsRef<Path>) -> Result<Self, ContentErrors> {
        let root = root.as_ref();
        let shown_root = root.to_string_lossy().replace('\\', "/");
        let mut files = Vec::new();
        collect(root, "", &mut files).map_err(|e| {
            ContentErrors(vec![ContentError::new(
                &shown_root,
                None,
                format!("cannot read: {e}"),
            )])
        })?;
        files.sort();
        let mut sources = Vec::new();
        let mut errors = Vec::new();
        for rel in files {
            let shown = format!("{}/{}", shown_root.trim_end_matches('/'), rel);
            match fs::read_to_string(root.join(&rel)) {
                Ok(text) => sources.push((shown, text)),
                Err(e) => errors.push(ContentError::new(&shown, None, format!("cannot read: {e}"))),
            }
        }
        if !errors.is_empty() {
            return Err(ContentErrors(errors));
        }
        Self::from_sources(sources)
    }

    /// Builds the set from `(display path, RON text)` pairs. Order of the input does not
    /// matter.
    pub fn from_sources(mut sources: Vec<(String, String)>) -> Result<Self, ContentErrors> {
        sources.sort();
        let mut errors = Vec::new();
        let mut roads: Vec<(String, RoadDef)> = Vec::new();
        let mut buildings: Vec<(String, BuildingDef)> = Vec::new();
        let mut services: Vec<(String, ServiceDef)> = Vec::new();
        let mut vehicles: Vec<(String, VehicleDef)> = Vec::new();
        let mut utilities: Vec<(String, UtilityDef)> = Vec::new();
        let mut styles: Vec<(String, StyleDef)> = Vec::new();
        let mut balances: Vec<(String, Balance)> = Vec::new();
        let mut milestone_files: Vec<(String, MilestonesDef)> = Vec::new();
        let mut policy_files: Vec<(String, PolicyDef)> = Vec::new();
        let mut spec_files: Vec<(String, SpecDef)> = Vec::new();
        let mut cargo_files: Vec<(String, CargoDef)> = Vec::new();
        for (path, text) in sources {
            match ron::from_str::<ContentFile>(&text) {
                Ok(ContentFile::Road(d)) => roads.push((path, d)),
                Ok(ContentFile::Building(d)) => buildings.push((path, d)),
                Ok(ContentFile::Service(d)) => services.push((path, d)),
                Ok(ContentFile::Vehicle(d)) => vehicles.push((path, d)),
                Ok(ContentFile::Utility(d)) => utilities.push((path, d)),
                Ok(ContentFile::Style(d)) => styles.push((path, d)),
                Ok(ContentFile::Balance(d)) => balances.push((path, *d)),
                Ok(ContentFile::Milestones(d)) => milestone_files.push((path, d)),
                Ok(ContentFile::Policy(d)) => policy_files.push((path, d)),
                Ok(ContentFile::Spec(d)) => spec_files.push((path, d)),
                Ok(ContentFile::Cargo(d)) => cargo_files.push((path, d)),
                Err(e) => errors.push(ContentError::new(&path, None, format!("parse error: {e}"))),
            }
        }
        let roads = table(roads, |d| &d.id, validate::road, &mut errors);
        let buildings = table(buildings, |d| &d.id, validate::building, &mut errors);
        let services = table(services, |d| &d.id, validate::service, &mut errors);
        let vehicles = table(vehicles, |d| &d.id, validate::vehicle, &mut errors);
        let utilities = table(utilities, |d| &d.id, validate::utility, &mut errors);
        let styles = table(styles, |d| &d.id, validate::style, &mut errors);
        let (mut buildings, mut utilities) = (buildings, utilities);
        // The icon of a def is data; a def without one uses the icon named like itself.
        for d in &mut buildings.defs {
            if d.icon.is_empty() {
                d.icon = d.id.clone();
            }
        }
        for d in &mut utilities.defs {
            if d.icon.is_empty() {
                d.icon = d.id.clone();
            }
        }

        // A Highway outranks every other class at a junction.
        let top_other = roads
            .defs
            .iter()
            .filter(|d| d.class != RoadClass::Highway)
            .map(|d| d.rank)
            .max();
        if let Some(top) = top_other {
            for (i, d) in roads.defs.iter().enumerate() {
                if d.class == RoadClass::Highway && d.rank <= top {
                    errors.push(ContentError::new(
                        &roads.files[i],
                        Some("rank"),
                        format!(
                            "a Highway's rank must be above {top}, the highest of the other classes"
                        ),
                    ));
                }
            }
        }

        // Cross references.
        for (i, b) in buildings.defs.iter().enumerate() {
            if let Some(s) = &b.service
                && services.get_id(s).is_none()
            {
                errors.push(ContentError::new(
                    &buildings.files[i],
                    Some("service"),
                    format!("unknown service `{s}`"),
                ));
            }
        }
        for (i, b) in buildings.defs.iter().enumerate() {
            for (k, name) in b.styles.iter().enumerate() {
                if styles.get_id(name).is_none() {
                    errors.push(ContentError::new(
                        &buildings.files[i],
                        Some(&format!("styles[{k}]")),
                        format!("unknown style `{name}`"),
                    ));
                }
            }
        }
        // The track of a station is a tunnel of a Rail road (T-162).
        for (i, b) in buildings.defs.iter().enumerate() {
            let Some(t) = &b.track else { continue };
            let ok = roads
                .get_id(&t.road)
                .map(|r| &roads.defs[r as usize])
                .is_some_and(|r| {
                    r.class == RoadClass::Rail && (t.depth <= 0.0 || r.tunnel.is_some())
                });
            if !ok {
                errors.push(ContentError::new(
                    &buildings.files[i],
                    Some("track.road"),
                    format!(
                        "`{}` is not a Rail road (with a tunnel variant when depth is above 0)",
                        t.road
                    ),
                ));
            }
        }
        // A free-standing building (`road_access: false`, §8.1) has nobody who could reach it.
        for (i, b) in buildings.defs.iter().enumerate() {
            let Some(s) = b.service.as_deref().and_then(|s| services.get_id(s)) else {
                continue;
            };
            let service = &services.defs[s as usize];
            if service.road_access {
                continue;
            }
            if b.jobs.iter().any(|&j| j != 0) {
                errors.push(ContentError::new(
                    &buildings.files[i],
                    Some("jobs"),
                    "a free-standing building (road_access: false) has no jobs".to_string(),
                ));
            }
            if b.households != 0 {
                errors.push(ContentError::new(
                    &buildings.files[i],
                    Some("households"),
                    "a free-standing building (road_access: false) has no households".to_string(),
                ));
            }
            if service.site != crate::kinds::ServiceSite::Land {
                errors.push(ContentError::new(
                    &services.files[s as usize],
                    Some("site"),
                    "a free-standing service must have site: Land".to_string(),
                ));
            }
        }
        for (i, s) in services.defs.iter().enumerate() {
            let tier_ok = if s.kind == crate::kinds::ServiceKind::Education {
                (1..=3).contains(&s.tier)
            } else {
                s.tier == 0
            };
            if !tier_ok {
                errors.push(ContentError::new(
                    &services.files[i],
                    Some("tier"),
                    "tier 1..=3 is for Education services only (and required there)".to_string(),
                ));
            }
            for (k, v) in s.vehicles.iter().enumerate() {
                if vehicles.get_id(v).is_none() {
                    errors.push(ContentError::new(
                        &services.files[i],
                        Some(&format!("vehicles[{k}]")),
                        format!("unknown vehicle `{v}`"),
                    ));
                }
            }
        }
        policy_files.sort_by(|a, b| a.1.kind.cmp(&b.1.kind).then_with(|| a.0.cmp(&b.0)));
        let mut policies: Vec<PolicyDef> = Vec::new();
        let mut policy_paths: Vec<String> = Vec::new();
        for (path, d) in policy_files {
            for (field, msg) in validate::policy(&d) {
                errors.push(ContentError::new(&path, Some(&field), msg));
            }
            if let Some(i) = policies.iter().position(|p| p.kind == d.kind) {
                errors.push(ContentError::new(
                    &path,
                    Some("kind"),
                    format!("{:?} is already defined in {}", d.kind, policy_paths[i]),
                ));
                continue;
            }
            policies.push(d);
            policy_paths.push(path);
        }
        spec_files.sort_by(|a, b| a.1.kind.cmp(&b.1.kind).then_with(|| a.0.cmp(&b.0)));
        let mut specs: Vec<SpecDef> = Vec::new();
        let mut spec_paths: Vec<String> = Vec::new();
        for (path, d) in spec_files {
            for (field, msg) in validate::spec(&d) {
                errors.push(ContentError::new(&path, Some(&field), msg));
            }
            if let Some(i) = specs.iter().position(|p| p.kind == d.kind) {
                errors.push(ContentError::new(
                    &path,
                    Some("kind"),
                    format!("{:?} is already defined in {}", d.kind, spec_paths[i]),
                ));
                continue;
            }
            specs.push(d);
            spec_paths.push(path);
        }
        cargo_files.sort_by(|a, b| a.1.kind.cmp(&b.1.kind).then_with(|| a.0.cmp(&b.0)));
        let mut cargos: Vec<CargoDef> = Vec::new();
        let mut cargo_paths: Vec<String> = Vec::new();
        for (path, d) in cargo_files {
            for (field, msg) in validate::cargo(&d) {
                errors.push(ContentError::new(&path, Some(&field), msg));
            }
            if let Some(i) = cargos.iter().position(|p| p.kind == d.kind) {
                errors.push(ContentError::new(
                    &path,
                    Some("kind"),
                    format!("{:?} is already defined in {}", d.kind, cargo_paths[i]),
                ));
                continue;
            }
            cargos.push(d);
            cargo_paths.push(path);
        }
        // An industrial building: a service building on a road whose role fits its resource,
        // with cargo that has a definition; and once there is any cargo or chain building,
        // every cargo kind has a definition.
        let mut any_industry = false;
        for (i, b) in buildings.defs.iter().enumerate() {
            let Some(ind) = &b.industry else { continue };
            any_industry = true;
            let mut bad = |field: &str, msg: String| {
                errors.push(ContentError::new(&buildings.files[i], Some(field), msg));
            };
            let on_road = b
                .service
                .as_deref()
                .and_then(|s| services.defs.iter().find(|d| d.id == s))
                .is_some_and(|s| s.road_access);
            if !on_road {
                bad(
                    "industry",
                    "an industrial building is a service building on a road".to_string(),
                );
            }
            if (ind.role == IndustryRole::Factory) != ind.resource.is_none() {
                bad(
                    "industry",
                    "a resource is set for every role but Factory, and only for those".to_string(),
                );
            }
            if !(1..=5).contains(&ind.min_level) {
                bad("industry", "min_level is 1..=5".to_string());
            }
            if !(ind.storage.is_finite() && ind.storage >= 0.0) {
                bad("industry", "storage is not negative".to_string());
            }
            for (kind, amount) in ind.inputs.iter().chain(&ind.outputs) {
                if !(amount.is_finite() && *amount > 0.0) {
                    bad("industry", format!("{kind:?} amount is positive"));
                }
                if !cargos.iter().any(|c| c.kind == *kind) {
                    bad("industry", format!("there is no cargo {kind:?}"));
                }
            }
        }
        if any_industry || !cargos.is_empty() {
            for kind in CargoKind::ALL {
                if !cargos.iter().any(|c| c.kind == kind) {
                    errors.push(ContentError::new(
                        "cargo.ron",
                        Some("kind"),
                        format!("cargo {kind:?} has no definition"),
                    ));
                }
            }
        }
        // A specialised building belongs to a zone its specialisation covers, at level 1.
        for (i, b) in buildings.defs.iter().enumerate() {
            let Some(kind) = b.spec else { continue };
            match specs.iter().find(|s| s.kind == kind) {
                None => errors.push(ContentError::new(
                    &buildings.files[i],
                    Some("spec"),
                    format!("there is no {kind:?} specialisation"),
                )),
                Some(s) => {
                    if !s.zones.contains(&b.zone) {
                        errors.push(ContentError::new(
                            &buildings.files[i],
                            Some("spec"),
                            format!("{kind:?} does not apply to the zone {:?}", b.zone),
                        ));
                    }
                    if b.level != 1 || b.service.is_some() {
                        errors.push(ContentError::new(
                            &buildings.files[i],
                            Some("spec"),
                            "a specialised building is level 1 and not a service".to_string(),
                        ));
                    }
                }
            }
        }
        if milestone_files.len() > 1 {
            errors.push(ContentError::new(
                &milestone_files[1].0,
                None,
                "more than one Milestones file".to_string(),
            ));
        }
        if let Some((path, m)) = milestone_files.first() {
            validate::milestones(path, m, &buildings.by_id, &mut errors);
        }
        if balances.len() > 1 {
            errors.push(ContentError::new(
                &balances[1].0,
                None,
                "more than one Balance file".to_string(),
            ));
        }
        if let Some((path, b)) = balances.first()
            && !(b.walk_limit_m.is_finite() && b.walk_limit_m >= 0.0)
        {
            errors.push(ContentError::new(
                path,
                Some("walk_limit_m"),
                "must be finite and >= 0".to_string(),
            ));
        }
        if let Some((path, b)) = balances.first()
            && b.spawn_one_in == 0
        {
            errors.push(ContentError::new(
                path,
                Some("spawn_one_in"),
                "must be >= 1".to_string(),
            ));
        }
        if let Some((path, b)) = balances.first() {
            let finite = |vs: &[f32]| vs.iter().all(|v| v.is_finite());
            if !(b.demand_ema.is_finite() && (0.0..=1.0).contains(&b.demand_ema)) {
                errors.push(ContentError::new(
                    path,
                    Some("demand_ema"),
                    "must be in 0..=1".to_string(),
                ));
            }
            let knobs = [
                ("demand_base", &b.demand_base[..], false),
                ("demand_job_weight", &b.demand_job_weight[..], true),
                (
                    "demand_adults_per_home",
                    &[b.demand_adults_per_home][..],
                    true,
                ),
                (
                    "demand_shop_jobs_per_citizen",
                    &[b.demand_shop_jobs_per_citizen][..],
                    true,
                ),
                (
                    "demand_industry_jobs_per_shop_job",
                    &[b.demand_industry_jobs_per_shop_job][..],
                    true,
                ),
            ];
            for (name, values, non_negative) in knobs {
                if !finite(values) || (non_negative && values.iter().any(|&v| v < 0.0)) {
                    let what = if non_negative {
                        "must be finite and >= 0"
                    } else {
                        "must be finite"
                    };
                    errors.push(ContentError::new(path, Some(name), what.to_string()));
                }
            }
        }
        if let Some((path, b)) = balances.first() {
            if b.land_knobs().iter().any(|v| !v.is_finite() || *v < 0.0) {
                errors.push(ContentError::new(
                    path,
                    Some("pollution_*, noise_*, land_*, level_*, abandon_days, demolish_days"),
                    "must be finite and >= 0".to_string(),
                ));
            }
            if b.loan_offers.len() > 255
                || b.loan_offers.iter().any(|o| {
                    !o.amount.is_finite()
                        || o.amount <= 0.0
                        || !o.interest_pct.is_finite()
                        || o.interest_pct < 0.0
                        || o.weeks == 0
                })
            {
                errors.push(ContentError::new(
                    path,
                    Some("loan_offers"),
                    "at most 255 offers, each with amount > 0, interest_pct >= 0, weeks >= 1"
                        .to_string(),
                ));
            }
            if b.networks
                .knobs()
                .iter()
                .any(|v| !(*v > 0.0 && *v <= Networks::MAX_M))
            {
                errors.push(ContentError::new(
                    path,
                    Some("networks"),
                    format!("power_gap_m, line_reach_m and pipe_reach_m must be in 0..={} (exclusive of 0)", Networks::MAX_M),
                ));
            }
            if b.level_checks == 0 {
                errors.push(ContentError::new(
                    path,
                    Some("level_checks"),
                    "must be >= 1".to_string(),
                ));
            }
        }
        if !errors.is_empty() {
            return Err(ContentErrors(errors));
        }

        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        hash_table(&mut hash, "road", &roads.defs, |d| &d.id);
        hash_table(&mut hash, "building", &buildings.defs, |d| &d.id);
        hash_table(&mut hash, "service", &services.defs, |d| &d.id);
        hash_table(&mut hash, "vehicle", &vehicles.defs, |d| &d.id);
        hash_table(&mut hash, "utility", &utilities.defs, |d| &d.id);
        hash_table(&mut hash, "style", &styles.defs, |d| &d.id);
        // Absent file = defaults = no contribution, so content sets without one keep their hash.
        let balance = balances.into_iter().next().map(|(_, b)| b);
        if let Some(b) = &balance {
            fnv1a(&mut hash, b"balance");
            fnv1a(&mut hash, &b.walk_limit_m.to_bits().to_le_bytes());
            fnv1a(&mut hash, &b.spawn_one_in.to_le_bytes());
            let demand = [b.demand_ema, b.demand_adults_per_home]
                .into_iter()
                .chain(b.demand_base)
                .chain(b.demand_job_weight)
                .chain([
                    b.demand_shop_jobs_per_citizen,
                    b.demand_industry_jobs_per_shop_job,
                ]);
            for v in demand {
                fnv1a(&mut hash, &v.to_bits().to_le_bytes());
            }
            // The T-042 knobs only count when they differ from the defaults, so content that
            // does not touch them keeps the hash (and the saves) it had before they existed.
            let defaults = Balance::default();
            if b.land_knobs() != defaults.land_knobs()
                || b.level_checks != defaults.level_checks
                || b.decay_mask != defaults.decay_mask
            {
                fnv1a(&mut hash, b"land");
                for v in b.land_knobs() {
                    fnv1a(&mut hash, &v.to_bits().to_le_bytes());
                }
                fnv1a(&mut hash, &b.level_checks.to_le_bytes());
                fnv1a(&mut hash, &[b.decay_mask]);
            }
            if b.networks != defaults.networks {
                fnv1a(&mut hash, b"networks");
                for v in b.networks.knobs() {
                    fnv1a(&mut hash, &v.to_bits().to_le_bytes());
                }
            }
            if b.loan_offers != defaults.loan_offers {
                fnv1a(&mut hash, b"loans");
                for o in &b.loan_offers {
                    fnv1a(&mut hash, &o.amount.to_bits().to_le_bytes());
                    fnv1a(&mut hash, &o.interest_pct.to_bits().to_le_bytes());
                    fnv1a(&mut hash, &o.weeks.to_le_bytes());
                    fnv1a(&mut hash, &o.min_population.to_le_bytes());
                }
            }
        }
        let milestones = milestone_files
            .into_iter()
            .next()
            .map_or_else(Vec::new, |(_, m)| m.milestones);
        if !milestones.is_empty() {
            fnv1a(&mut hash, b"milestones");
            fnv1a(
                &mut hash,
                &postcard::to_stdvec(&milestones).expect("milestones are serialisable"),
            );
        }
        if !policies.is_empty() {
            fnv1a(&mut hash, b"policies");
            fnv1a(
                &mut hash,
                &postcard::to_stdvec(&policies).expect("policies are serialisable"),
            );
        }
        if !specs.is_empty() {
            fnv1a(&mut hash, b"specs");
            fnv1a(
                &mut hash,
                &postcard::to_stdvec(&specs).expect("specs are serialisable"),
            );
        }
        if !cargos.is_empty() {
            fnv1a(&mut hash, b"cargos");
            fnv1a(
                &mut hash,
                &postcard::to_stdvec(&cargos).expect("cargos are serialisable"),
            );
        }
        Ok(Self {
            roads: roads.strip(),
            buildings: buildings.strip(),
            services: services.strip(),
            vehicles: vehicles.strip(),
            utilities: utilities.strip(),
            styles: styles.strip(),
            placeholders: Placeholders::builtin(),
            balance: balance.unwrap_or_default(),
            milestones,
            policies,
            specs,
            cargos,
            content_hash: hash,
        })
    }
}

fn hash_table<T: serde::Serialize>(
    hash: &mut u64,
    kind: &str,
    defs: &[T],
    id: impl Fn(&T) -> &String,
) {
    for d in defs {
        fnv1a(hash, kind.as_bytes());
        fnv1a(hash, &[0]);
        fnv1a(hash, id(d).as_bytes());
        fnv1a(hash, &[0]);
        // postcard: fixed field order from the schema, varint/IEEE-754 bit patterns, no
        // formatting — independent of the RON text and of the `ron` crate version.
        let bytes = postcard::to_stdvec(d).expect("content defs are serialisable");
        fnv1a(hash, &(bytes.len() as u64).to_le_bytes());
        fnv1a(hash, &bytes);
    }
}

/// A table plus the source file of each definition (for error reporting).
struct Built<T> {
    defs: Vec<T>,
    by_id: BTreeMap<String, u32>,
    files: Vec<String>,
}

impl<T> Built<T> {
    fn get_id(&self, s: &str) -> Option<u32> {
        self.by_id.get(s).copied()
    }
    fn strip(self) -> Table<T> {
        Table {
            defs: self.defs,
            by_id: self.by_id,
        }
    }
}

/// Validates, rejects duplicate ids and orders by id. Input is in file-path order, so the
/// "first definition" in a duplicate pair is deterministic.
fn table<T>(
    items: Vec<(String, T)>,
    id: impl Fn(&T) -> &String,
    check: impl Fn(&T) -> validate::Problems,
    errors: &mut Vec<ContentError>,
) -> Built<T> {
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    let mut keep: Vec<(String, String, T)> = Vec::new();
    for (path, def) in items {
        for (field, msg) in check(&def) {
            errors.push(ContentError::new(&path, Some(&field), msg));
        }
        let name = id(&def).clone();
        if let Some(first) = seen.get(&name) {
            errors.push(ContentError::new(
                &path,
                Some("id"),
                format!("duplicate id `{name}` (first defined in {first})"),
            ));
            continue;
        }
        seen.insert(name.clone(), path.clone());
        keep.push((name, path, def));
    }
    keep.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = Built {
        defs: Vec::new(),
        by_id: BTreeMap::new(),
        files: Vec::new(),
    };
    for (i, (name, path, def)) in keep.into_iter().enumerate() {
        out.by_id.insert(name, i as u32);
        out.files.push(path);
        out.defs.push(def);
    }
    out
}

fn collect(root: &Path, rel: &str, out: &mut Vec<String>) -> std::io::Result<()> {
    let dir = if rel.is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    };
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let child = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        let ty = entry.file_type()?;
        if ty.is_dir() {
            collect(root, &child, out)?;
        } else if child.ends_with(".ron") {
            out.push(child);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_answer_the_reserved_index_and_never_appear_in_the_tables() {
        let db = ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
            .unwrap();
        for kind in DefKind::ALL {
            assert_eq!(ContentDb::placeholder(kind), PLACEHOLDER_INDEX);
        }
        assert_eq!(db.road(RoadDefId(PLACEHOLDER_INDEX)).id, "placeholder_road");
        assert_eq!(
            db.building(BuildingDefId(PLACEHOLDER_INDEX)).id,
            "placeholder_building"
        );
        assert_eq!(
            db.service(ServiceDefId(PLACEHOLDER_INDEX)).id,
            "placeholder_service"
        );
        assert_eq!(
            db.vehicle(VehicleDefId(PLACEHOLDER_INDEX)).id,
            "placeholder_vehicle"
        );
        assert_eq!(
            db.utility(UtilityDefId(PLACEHOLDER_INDEX)).id,
            "placeholder_utility"
        );
        assert!(
            db.roads()
                .all(|(id, d)| id.0 != PLACEHOLDER_INDEX && d.id != "placeholder_road")
        );
        // The names of a table are its ids in index order, and look up to the same index.
        for kind in DefKind::ALL {
            for (i, n) in db.def_names(kind).iter().enumerate() {
                assert_eq!(db.def_index(kind, n), Some(i as u32));
            }
        }
        assert_eq!(db.def_index(DefKind::Road, "nope"), None);
    }
    use crate::kinds::{UtilityKind, ZoneType};

    /// Re-pinned for T-078: `RoadDef.class` is part of the hashed encoding (it was
    /// 4353786976156915245 before); and for T-161: `RoadDef.tunnel` and `RoadDef.ground` are
    /// (it was 3543843456772795126 before).
    const PINNED_SINGLE_ROAD_HASH: u64 = 10696934927935380879;

    const ROAD: &str = r#"Road((id: "r", name: "R", width: 10.0,
        lanes: [(kind: Car, dir: Forward, width: 3.0, offset: 0.0)],
        speed_kmh: 40.0, max_slope: 0.12, cost_per_m: 1.0, upkeep_per_m_week: 0.1,
        allows_zoning: true, rank: 1, mesh: "m", material: "x"))"#;

    fn src(files: &[(&str, &str)]) -> Vec<(String, String)> {
        files
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    fn errors(files: &[(&str, &str)]) -> Vec<String> {
        let e = ContentDb::from_sources(src(files)).unwrap_err();
        e.0.iter().map(ToString::to_string).collect()
    }

    fn real() -> ContentDb {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content");
        ContentDb::load_dir(root).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn loads_shipped_content() {
        let db = real();
        assert_eq!(db.roads().count(), 11);
        // T-190: the eight policies, one per kind, in kind order.
        let kinds: Vec<_> = db.policies().iter().map(|p| p.kind).collect();
        assert_eq!(kinds, PolicyKind::ALL);
        assert!(
            db.policy(PolicyKind::TourismPromotion)
                .is_some_and(|p| { p.scope == crate::kinds::PolicyScope::City && p.value == 0.25 })
        );
        // Dense ids follow sorted string ids.
        let names: Vec<_> = db.roads().map(|(_, d)| d.id.as_str()).collect();
        assert_eq!(
            names,
            [
                "basic_1w_2l",
                "basic_2l",
                "highway_1w_1l",
                "highway_1w_2l",
                "highway_1w_3l",
                "highway_1w_4l",
                "large_6l",
                "medium_1w_3l",
                "medium_4l",
                "metro_track",
                "rail_2t"
            ]
        );
        let hw = db.road(db.road_id("highway_1w_3l").unwrap());
        assert!(!hw.allows_zoning && hw.elevated.is_some() && hw.bridge.is_some());
        assert_eq!(db.road(db.road_id("basic_2l").unwrap()).lanes.len(), 4);
        assert!(db.building_id("water_pump_station").is_some());
        assert!(db.service_id("water_pump").is_some());
        assert!(db.vehicle_id("car_basic").is_some());
    }

    #[test]
    fn zone_field_parses_into_zone_type() {
        let db = real();
        let house = db.building(db.building_id("house_low_1").unwrap());
        assert_eq!(house.zone, ZoneType::ResLow);
        assert_eq!(house.consumption[&UtilityKind::Power], 1.5);
    }

    #[test]
    fn hash_is_stable_and_order_independent() {
        let a = real();
        let b = real();
        assert_eq!(a.content_hash(), b.content_hash());
        let d1 = ContentDb::from_sources(src(&[("a.ron", ROAD)])).unwrap();
        let swapped = ROAD.replace(r#"id: "r""#, r#"id: "s""#);
        let two = [("a.ron", ROAD), ("b.ron", swapped.as_str())];
        let rev = [("b.ron", swapped.as_str()), ("a.ron", ROAD)];
        let h1 = ContentDb::from_sources(src(&two)).unwrap().content_hash();
        let h2 = ContentDb::from_sources(src(&rev)).unwrap().content_hash();
        assert_eq!(h1, h2);
        assert_ne!(h1, d1.content_hash());
        // Whitespace/CRLF in files does not matter.
        let crlf = ROAD.replace('\n', "\r\n");
        assert_eq!(
            ContentDb::from_sources(src(&[("a.ron", &crlf)]))
                .unwrap()
                .content_hash(),
            d1.content_hash()
        );
    }

    fn hash_of(files: &[(&str, &str)]) -> u64 {
        ContentDb::from_sources(src(files)).unwrap().content_hash()
    }

    #[test]
    fn hash_ignores_formatting_and_file_layout_but_not_values() {
        let other = ROAD.replace(r#"id: "r""#, r#"id: "s""#);
        let base = hash_of(&[("a.ron", ROAD), ("b.ron", &other)]);

        // Different number spellings, whitespace, comments, file names, file order.
        let respelled = ROAD
            .replace("width: 10.0", "width: 1e1")
            .replace("max_slope: 0.12", "max_slope: 0.1200")
            .replace("cost_per_m: 1.0", "cost_per_m:1.00")
            .replace(", ", ",\n\t  ");
        let commented = format!("// a comment\n{respelled}\n\n");
        assert_eq!(
            hash_of(&[("z/other.ron", &other), ("deep/dir/r.ron", &commented)]),
            base
        );

        // Changing a numeric field changes the hash (even in the last decimal), and so does
        // changing an id.
        for changed in [
            ROAD.replace("speed_kmh: 40.0", "speed_kmh: 41.0"),
            ROAD.replace("max_slope: 0.12", "max_slope: 0.1201"),
            ROAD.replace("rank: 1", "rank: 2"),
            ROAD.replace("allows_zoning: true", "allows_zoning: false"),
        ] {
            assert_ne!(hash_of(&[("a.ron", &changed), ("b.ron", &other)]), base);
        }
    }

    #[test]
    fn hash_value_is_pinned() {
        // Guards the hash algorithm and encoding (FNV-1a over postcard in (kind, id) order):
        // if this changes, every existing save will show the content-mismatch warning.
        assert_eq!(hash_of(&[("a.ron", ROAD)]), PINNED_SINGLE_ROAD_HASH);
    }

    #[test]
    fn duplicate_id_is_reported_with_file() {
        let e = errors(&[("c/a.ron", ROAD), ("c/b.ron", ROAD)]);
        assert_eq!(e.len(), 1);
        assert_eq!(
            e[0],
            "c/b.ron: id: duplicate id `r` (first defined in c/a.ron)"
        );
    }

    #[test]
    fn broken_reference_is_reported() {
        let b = r#"Building((id: "b", name: "B", zone: ComLow, level: 1, lot: (1, 1),
            tax_base: 1.0, styles: ["p"], service: Some("nope")))"#;
        let e = errors(&[("c/b.ron", b), ("c/p.ron", TEST_STYLE_RON)]);
        assert_eq!(e, ["c/b.ron: service: unknown service `nope`"]);
    }

    /// T-111: a building's style references are checked on load, and it needs at least one.
    #[test]
    fn unknown_or_missing_styles_are_load_errors() {
        let b = |styles: &str| {
            format!(
                r#"Building((id: "b", name: "B", zone: ComLow, level: 1, lot: (1, 1),
                tax_base: 1.0, styles: {styles}, service: None))"#
            )
        };
        let e = errors(&[
            ("c/b.ron", &b(r#"["p", "ghost"]"#)),
            ("c/p.ron", TEST_STYLE_RON),
        ]);
        assert_eq!(e, ["c/b.ron: styles[1]: unknown style `ghost`"]);
        let e = errors(&[("c/b.ron", &b("[]")), ("c/p.ron", TEST_STYLE_RON)]);
        assert_eq!(e, ["c/b.ron: styles: needs at least one style"]);
        let ok = ContentDb::from_sources(src(&[
            ("c/b.ron", &b(r#"["p"]"#)),
            ("c/p.ron", TEST_STYLE_RON),
        ]))
        .unwrap();
        assert_eq!(ok.styles().count(), 1);
        assert_eq!(ok.style(ok.style_id("p").unwrap()).id, "p");
        // An unknown id answers with the built-in grey style.
        assert_eq!(
            ok.style(StyleDefId(PLACEHOLDER_INDEX)).id,
            "placeholder_style"
        );
    }

    #[test]
    fn style_ranges_are_validated() {
        for (from, to, field) in [
            ("floors: (1, 1)", "floors: (3, 2)", "floors"),
            ("floors: (1, 1)", "floors: (0, 2)", "floors"),
            (
                "footprint_fill: (1.0, 1.0)",
                "footprint_fill: (0.5, 1.2)",
                "footprint_fill",
            ),
            ("walls: [(0.5, 0.5, 0.5)]", "walls: []", "palette.walls"),
            (
                "glass: (0.2, 0.2, 0.3)",
                "glass: (2.0, 0.2, 0.3)",
                "palette.glass",
            ),
        ] {
            let bad = TEST_STYLE_RON.replace(from, to);
            let e = errors(&[("s.ron", &bad)]);
            assert!(
                e.iter().any(|m| m.starts_with(&format!("s.ron: {field}:"))),
                "{e:?}"
            );
        }
    }

    #[test]
    fn negative_width_is_reported() {
        let bad = ROAD.replace("width: 3.0", "width: -3.0");
        let e = errors(&[("assets/content/roads/r.ron", &bad)]);
        assert!(
            e.contains(&"assets/content/roads/r.ron: lanes[0].width: must be > 0".to_string()),
            "{e:?}"
        );
    }

    #[test]
    fn parse_error_names_file() {
        let e = errors(&[("x.ron", "Road((id: ")]);
        assert!(e[0].starts_with("x.ron: parse error"), "{e:?}");
    }

    #[test]
    fn unknown_field_is_rejected() {
        let bad = ROAD.replace("rank: 1,", "rank: 1, bogus: 2,");
        assert!(errors(&[("x.ron", &bad)])[0].starts_with("x.ron: parse error"));
    }

    #[test]
    fn balance_defaults_without_a_file_and_is_part_of_the_hash() {
        let none = ContentDb::from_sources(src(&[("a.ron", ROAD)])).unwrap();
        assert_eq!(none.balance().walk_limit_m, 1000.0);
        let b = r#"Balance((walk_limit_m: 600.0))"#;
        let with = ContentDb::from_sources(src(&[("a.ron", ROAD), ("balance.ron", b)])).unwrap();
        assert_eq!(with.balance().walk_limit_m, 600.0);
        assert_ne!(none.content_hash(), with.content_hash());
        let b2 = b.replace("600.0", "700.0");
        let other = ContentDb::from_sources(src(&[("a.ron", ROAD), ("balance.ron", &b2)])).unwrap();
        assert_ne!(with.content_hash(), other.content_hash());
    }

    #[test]
    fn network_distances_are_validated_and_hashed() {
        let ok = |n: &str| {
            let b = format!("Balance((networks: ({n})))");
            ContentDb::from_sources(src(&[("a.ron", ROAD), ("balance.ron", &b)]))
        };
        let base = ok("power_gap_m: 64.0").unwrap();
        assert_eq!(base.balance().networks.line_reach_m, 80.0);
        assert_ne!(
            base.content_hash(),
            ok("power_gap_m: 70.0").unwrap().content_hash()
        );
        for bad in [
            "power_gap_m: 0.0",
            "line_reach_m: -1.0",
            "pipe_reach_m: 1000.0",
        ] {
            assert!(ok(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn two_balance_files_or_a_bad_value_are_errors() {
        let b = r#"Balance((walk_limit_m: 600.0))"#;
        let e = errors(&[("a.ron", b), ("b.ron", b)]);
        assert!(
            e.iter().any(|m| m.contains("more than one Balance")),
            "{e:?}"
        );
        let bad = r#"Balance((walk_limit_m: -1.0))"#;
        let e = errors(&[("a.ron", bad)]);
        assert!(e.iter().any(|m| m.contains("walk_limit_m")), "{e:?}");
    }

    /// T-105: a def's icon is data: the id when the RON has none, the RON's when it has.
    #[test]
    fn a_def_icon_defaults_to_its_id_and_an_explicit_one_is_read() {
        let db = ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
            .unwrap();
        let (_, apt) = db
            .buildings()
            .find(|(_, d)| d.id == "apt_high_1")
            .expect("a zoned building");
        assert_eq!(apt.icon, "apt_high_1");
        for (_, d) in db.buildings() {
            assert!(!d.icon.is_empty(), "{}", d.id);
        }
        for (_, d) in db.utilities() {
            assert!(!d.icon.is_empty(), "{}", d.id);
        }
        let (_, pump) = db
            .buildings()
            .find(|(_, d)| d.id == "water_pump_station")
            .unwrap();
        assert_eq!(pump.icon, "water_pump_station");
        // One pipe (T-101): no sewage pipe any more.
        assert!(db.utilities().all(|(_, d)| d.id != "sewage_pipe"));
    }

    /// T-106 (§8.1): a free-standing service (`road_access: false`) has no jobs and no
    /// households, stands on land, and its lot stays 1×1…4×4.
    #[test]
    fn a_free_standing_building_must_have_no_jobs_no_households_and_a_land_site() {
        let building = |jobs: &str, households: u16| {
            (
                "content/buildings/tower.ron".to_string(),
                format!(
                    r#"Building((id: "tower", name: "Tower", zone: Industrial, level: 1,
                    lot: (1, 1), jobs: {jobs}, households: {households}, consumption: {{}},
                    tax_base: 0.0, cost: 1.0, upkeep_week: 1.0,
                    styles: ["p"],
                    service: Some("tower_service")))"#
                ),
            )
        };
        let service = |site: &str, road_access: &str| {
            (
                "content/services/tower.ron".to_string(),
                format!(
                    r#"Service((id: "tower_service", name: "Tower", kind: Water, radius_m: 100.0,
                    capacity: 10, site: {site}, {road_access} vehicles: []))"#
                ),
            )
        };
        let load = |b, s| {
            ContentDb::from_sources(vec![
                b,
                s,
                ("content/styles/p.ron".into(), TEST_STYLE_RON.into()),
            ])
        };
        // Free-standing, on land, nobody works or lives there: fine.
        assert!(
            load(
                building("(0, 0, 0, 0)", 0),
                service("Land", "road_access: false,")
            )
            .is_ok()
        );
        // With a job: an error on the building's `jobs`.
        let err = load(
            building("(0, 1, 0, 0)", 0),
            service("Land", "road_access: false,"),
        )
        .expect_err("jobs are refused");
        assert!(err.to_string().contains("jobs"), "{err}");
        // With households.
        let err = load(
            building("(0, 0, 0, 0)", 2),
            service("Land", "road_access: false,"),
        )
        .expect_err("households are refused");
        assert!(err.to_string().contains("households"), "{err}");
        // On water.
        let err = load(
            building("(0, 0, 0, 0)", 0),
            service("OnWater", "road_access: false,"),
        )
        .expect_err("a water site is refused");
        assert!(err.to_string().contains("site"), "{err}");
        // The same jobs with the default (a road is needed): fine.
        assert!(load(building("(0, 1, 0, 0)", 0), service("OnWater", "")).is_ok());
    }
}
