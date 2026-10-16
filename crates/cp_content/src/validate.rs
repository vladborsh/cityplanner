//! Per-definition range checks. Each returns `(field, message)` pairs.

use crate::defs::{
    BuildingDef, LaneKind, MilestonesDef, RoadClass, RoadDef, RoadVariant, ServiceDef, StyleDef,
    TunnelVariant, UtilityDef, VehicleDef,
};
use crate::error::ContentError;
use crate::kinds::ZoneType;

/// Steepest grade a Highway may declare.
pub const HIGHWAY_MAX_SLOPE: f32 = 0.06;

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

fn tunnel(c: &mut Check, v: &Option<TunnelVariant>) {
    if let Some(v) = v {
        c.non_negative("tunnel.cost_per_m", v.cost_per_m);
        c.non_negative("tunnel.upkeep_per_m_week", v.upkeep_per_m_week);
        c.not_empty("tunnel.mesh", &v.mesh);
        c.positive("tunnel.depth", v.depth);
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
    if d.class == RoadClass::Highway {
        if !d.is_one_way() {
            c.bad(
                "lanes",
                "a Highway has car lanes and all of them are Forward",
            );
        }
        if let Some(i) = d.lanes.iter().position(|l| l.kind == LaneKind::Pedestrian) {
            c.bad(
                format!("lanes[{i}].kind"),
                "a Highway has no Pedestrian lane",
            );
        }
        if d.allows_zoning {
            c.bad("allows_zoning", "a Highway does not allow zoning");
        }
        if d.max_slope > HIGHWAY_MAX_SLOPE {
            c.bad("max_slope", "a Highway's max_slope must be <= 0.06");
        }
    }
    if d.class == RoadClass::Rail {
        for (i, l) in d.lanes.iter().enumerate() {
            if l.kind != LaneKind::Rail {
                c.bad(
                    format!("lanes[{i}].kind"),
                    "a Rail road has Rail lanes only",
                );
            }
        }
        if d.allows_zoning {
            c.bad("allows_zoning", "a Rail road does not allow zoning");
        }
    } else if let Some(i) = d.lanes.iter().position(|l| l.kind == LaneKind::Rail) {
        c.bad(
            format!("lanes[{i}].kind"),
            "a Rail lane belongs to a road of class Rail",
        );
    }
    if !d.ground && d.tunnel.is_none() {
        c.bad("ground", "a road without a ground version needs a tunnel");
    }
    tunnel(&mut c, &d.tunnel);
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
    c.non_negative("cost", d.cost);
    c.non_negative("upkeep_week", d.upkeep_week);
    c.non_negative("build_hours", d.build_hours);
    if d.styles.is_empty() {
        c.bad("styles", "needs at least one style");
    }
    for (k, s) in d.styles.iter().enumerate() {
        c.not_empty(&format!("styles[{k}]"), s);
    }
    c.0
}

pub fn style(d: &StyleDef) -> Problems {
    let mut c = Check(Vec::new());
    c.not_empty("id", &d.id);
    let (lo, hi) = d.floors;
    if lo < 1 || hi > 60 || lo > hi {
        c.bad("floors", "must be (min, max) with 1 <= min <= max <= 60");
    }
    let (lo, hi) = d.footprint_fill;
    if !(lo.is_finite() && hi.is_finite() && lo > 0.0 && lo <= hi && hi <= 1.0) {
        c.bad(
            "footprint_fill",
            "must be (min, max) with 0 < min <= max <= 1",
        );
    }
    let p = &d.palette;
    for (name, list) in [
        ("palette.walls", &p.walls),
        ("palette.roof", &p.roof),
        ("palette.trim", &p.trim),
    ] {
        if list.is_empty() {
            c.bad(name, "needs at least one colour");
        }
        if list.iter().flatten().any(|v| !(0.0..=1.0).contains(v)) {
            c.bad(name, "channels must be in 0..=1");
        }
    }
    if p.glass.iter().any(|v| !(0.0..=1.0).contains(v)) {
        c.bad("palette.glass", "channels must be in 0..=1");
    }
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
    c.positive("decel", d.decel);
    c.non_negative("min_gap", d.min_gap);
    c.not_empty("mesh", &d.mesh);
    c.0
}

pub fn utility(d: &UtilityDef) -> Problems {
    let mut c = Check(Vec::new());
    c.not_empty("id", &d.id);
    if d.kind == crate::kinds::UtilityKind::Sewage {
        c.bad(
            "kind",
            "Sewage is not a utility: water and sewage share one pipe (kind: Water)",
        );
    }
    c.non_negative("cost_per_m", d.cost_per_m);
    c.non_negative("upkeep_per_m_week", d.upkeep_per_m_week);
    c.positive("min_link_m", d.min_link_m);
    c.positive("max_link_m", d.max_link_m);
    if d.max_link_m < d.min_link_m * 2.0 {
        c.bad("max_link_m", "must be at least twice min_link_m");
    }
    c.not_empty("mesh", &d.mesh);
    c.0
}

/// Checks `milestones.ron`: population 0 first and strictly rising, unique names, every unlock
/// an existing building def named by one milestone only.
pub fn milestones(
    path: &str,
    m: &MilestonesDef,
    buildings: &std::collections::BTreeMap<String, u32>,
    errors: &mut Vec<ContentError>,
) {
    let mut bad = |field: String, msg: String| {
        errors.push(ContentError::new(path, Some(&field), msg));
    };
    if m.milestones.is_empty() {
        bad("milestones".into(), "needs at least one milestone".into());
    }
    if m.milestones.len() > usize::from(u8::MAX) {
        bad("milestones".into(), "at most 255 milestones".into());
    }
    let mut seen: std::collections::BTreeSet<&str> = Default::default();
    for (i, ms) in m.milestones.iter().enumerate() {
        if ms.name.trim().is_empty() {
            bad(format!("milestones[{i}].name"), "must not be empty".into());
        }
        if i == 0 && ms.population != 0 {
            bad(
                "milestones[0].population".into(),
                "the first milestone is the start: population 0".into(),
            );
        }
        if i > 0 && ms.population <= m.milestones[i - 1].population {
            bad(
                format!("milestones[{i}].population"),
                "must be above the previous milestone".into(),
            );
        }
        for u in &ms.unlocks {
            if !buildings.contains_key(u) {
                bad(
                    format!("milestones[{i}].unlocks"),
                    format!("unknown building `{u}`"),
                );
            } else if !seen.insert(u) {
                bad(
                    format!("milestones[{i}].unlocks"),
                    format!("`{u}` is unlocked by more than one milestone"),
                );
            }
        }
    }
}
