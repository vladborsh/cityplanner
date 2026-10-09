//! Loading, validation, hashing and lookup of the content set.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::defs::*;
use crate::error::{ContentError, ContentErrors};
use crate::validate;

/// Default content root, relative to the working directory.
pub const DEFAULT_CONTENT_ROOT: &str = "assets/content";

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

/// The validated, immutable content set.
#[derive(Clone, Debug, PartialEq)]
pub struct ContentDb {
    roads: Table<RoadDef>,
    buildings: Table<BuildingDef>,
    services: Table<ServiceDef>,
    vehicles: Table<VehicleDef>,
    utilities: Table<UtilityDef>,
    balance: Balance,
    content_hash: u64,
}

macro_rules! accessors {
    ($field:ident, $def:ty, $id:ident, $get:ident, $by:ident, $iter:ident) => {
        pub fn $get(&self, id: $id) -> &$def {
            &self.$field.defs[id.index()]
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
    accessors!(roads, RoadDef, RoadDefId, road, road_id, roads);
    accessors!(
        buildings,
        BuildingDef,
        BuildingDefId,
        building,
        building_id,
        buildings
    );
    accessors!(
        services,
        ServiceDef,
        ServiceDefId,
        service,
        service_id,
        services
    );
    accessors!(
        vehicles,
        VehicleDef,
        VehicleDefId,
        vehicle,
        vehicle_id,
        vehicles
    );
    accessors!(
        utilities,
        UtilityDef,
        UtilityDefId,
        utility,
        utility_id,
        utilities
    );

    /// The utility definition for `kind` (the first by sorted id), if the content has one.
    pub fn utility_for_kind(
        &self,
        kind: crate::kinds::UtilityKind,
    ) -> Option<(UtilityDefId, &UtilityDef)> {
        self.utilities().find(|(_, d)| d.kind == kind)
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
        let mut balances: Vec<(String, Balance)> = Vec::new();
        for (path, text) in sources {
            match ron::from_str::<ContentFile>(&text) {
                Ok(ContentFile::Road(d)) => roads.push((path, d)),
                Ok(ContentFile::Building(d)) => buildings.push((path, d)),
                Ok(ContentFile::Service(d)) => services.push((path, d)),
                Ok(ContentFile::Vehicle(d)) => vehicles.push((path, d)),
                Ok(ContentFile::Utility(d)) => utilities.push((path, d)),
                Ok(ContentFile::Balance(d)) => balances.push((path, d)),
                Err(e) => errors.push(ContentError::new(&path, None, format!("parse error: {e}"))),
            }
        }
        let roads = table(roads, |d| &d.id, validate::road, &mut errors);
        let buildings = table(buildings, |d| &d.id, validate::building, &mut errors);
        let services = table(services, |d| &d.id, validate::service, &mut errors);
        let vehicles = table(vehicles, |d| &d.id, validate::vehicle, &mut errors);
        let utilities = table(utilities, |d| &d.id, validate::utility, &mut errors);

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
        for (i, s) in services.defs.iter().enumerate() {
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
        if !errors.is_empty() {
            return Err(ContentErrors(errors));
        }

        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        hash_table(&mut hash, "road", &roads.defs, |d| &d.id);
        hash_table(&mut hash, "building", &buildings.defs, |d| &d.id);
        hash_table(&mut hash, "service", &services.defs, |d| &d.id);
        hash_table(&mut hash, "vehicle", &vehicles.defs, |d| &d.id);
        hash_table(&mut hash, "utility", &utilities.defs, |d| &d.id);
        // Absent file = defaults = no contribution, so content sets without one keep their hash.
        let balance = balances.into_iter().next().map(|(_, b)| b);
        if let Some(b) = &balance {
            fnv1a(&mut hash, b"balance");
            fnv1a(&mut hash, &b.walk_limit_m.to_bits().to_le_bytes());
        }
        Ok(Self {
            roads: roads.strip(),
            buildings: buildings.strip(),
            services: services.strip(),
            vehicles: vehicles.strip(),
            utilities: utilities.strip(),
            balance: balance.unwrap_or_default(),
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
    use crate::kinds::{UtilityKind, ZoneType};

    const PINNED_SINGLE_ROAD_HASH: u64 = 4353786976156915245;

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
        assert_eq!(db.roads().count(), 4);
        // Dense ids follow sorted string ids.
        let names: Vec<_> = db.roads().map(|(_, d)| d.id.as_str()).collect();
        assert_eq!(
            names,
            ["basic_2l", "highway_1w_3l", "large_6l", "medium_4l"]
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
            tax_base: 1.0, style: (floors: 1, roof: Flat, palette: "p"), service: Some("nope")))"#;
        let e = errors(&[("c/b.ron", b)]);
        assert_eq!(e, ["c/b.ron: service: unknown service `nope`"]);
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
}
