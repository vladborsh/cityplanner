//! Per-definition range checks. Each returns `(field, message)` pairs.

use crate::defs::{BuildingDef, RoadDef, RoadVariant, ServiceDef, VehicleDef};
use crate::kinds::ZoneType;

pub type Problems = Vec<(String, String)>;

struct Check(Problems);

impl Check {
    fn bad(&mut self, field: impl Into<String>, msg: impl Into<String>) {
        self.0.push((field.into(), msg.into()));
    }
    fn positive(&mut self, field: impl Into<String>, v: f32) {
        if !(v.is_finite() && v > 0.0) {
            self.bad(field, "must be > 0");
        }
    }
    fn non_negative(&mut self, field: impl Into<String>, v: f32) {
        if !(v.is_finite() && v >= 0.0) {
            self.bad(field, "must be >= 0");
        }
    }
    fn not_empty(&mut self, field: &str, v: &str) {
        if v.trim().is_empty() {
            self.bad(field, "must not be empty");
        }
    }
}

fn variant(c: &mut Check, name: &str, v: &Option<RoadVariant>) {
    if let Some(v) = v {
        c.non_negative(format!("{name}.cost_per_m"), v.cost_per_m);
        c.non_negative(format!("{name}.upkeep_per_m_week"), v.upkeep_per_m_week);
        c.not_empty(&format!("{name}.mesh"), &v.mesh);
    }
}

pub fn road(d: &RoadDef) -> Problems {
    let mut c = Check(Vec::new());
    c.not_empty("id", &d.id);
    c.positive("width", d.width);
    c.non_negative("median_width", d.median_width);
    c.positive("speed_kmh", d.speed_kmh);
    if !(d.max_slope > 0.0 && d.max_slope <= 1.0) {
        c.bad("max_slope", "must be in (0, 1]");
    }
    c.non_negative("cost_per_m", d.cost_per_m);
    c.non_negative("upkeep_per_m_week", d.upkeep_per_m_week);
    if d.lanes.is_empty() {
        c.bad("lanes", "must have at least one lane");
    }
    for (i, l) in d.lanes.iter().enumerate() {
        c.positive(format!("lanes[{i}].width"), l.width);
        if !l.offset.is_finite() || l.offset.abs() + l.width * 0.5 > d.width * 0.5 + 1e-3 {
            c.bad(
                format!("lanes[{i}].offset"),
                "lane lies outside the road width",
            );
        }
    }
    variant(&mut c, "elevated", &d.elevated);
    variant(&mut c, "bridge", &d.bridge);
    c.not_empty("mesh", &d.mesh);
    c.not_empty("material", &d.material);
    c.0
}

pub fn building(d: &BuildingDef) -> Problems {
    let mut c = Check(Vec::new());
    c.not_empty("id", &d.id);
    if d.zone == ZoneType::None {
        c.bad("zone", "must not be None");
    }
    if !(1..=5).contains(&d.level) {
        c.bad("level", "must be in 1..=5");
    }
    if !(1..=4).contains(&d.lot.0) {
        c.bad("lot.0", "must be in 1..=4");
    }
    if !(1..=4).contains(&d.lot.1) {
        c.bad("lot.1", "must be in 1..=4");
    }
    for (k, v) in &d.consumption {
        c.non_negative(format!("consumption.{k:?}"), *v);
    }
    c.non_negative("tax_base", d.tax_base);
    if !(1..=60).contains(&d.style.floors) {
        c.bad("style.floors", "must be in 1..=60");
    }
    c.not_empty("style.palette", &d.style.palette);
    c.0
}

pub fn service(d: &ServiceDef) -> Problems {
    let mut c = Check(Vec::new());
    c.not_empty("id", &d.id);
    c.positive("radius_m", d.radius_m);
    if d.capacity == 0 {
        c.bad("capacity", "must be > 0");
    }
    c.0
}

pub fn vehicle(d: &VehicleDef) -> Problems {
    let mut c = Check(Vec::new());
    c.not_empty("id", &d.id);
    c.positive("length", d.length);
    c.positive("width", d.width);
    c.positive("max_speed_kmh", d.max_speed_kmh);
    c.positive("accel", d.accel);
    c.not_empty("mesh", &d.mesh);
    c.0
}
