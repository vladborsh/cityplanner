//! The city advisor (T-188): a snackbar above the demand bar that names the city's current
//! problems and needs, one at a time, at most once every couple of game days per subject.
//!
//! UI only: it reads the [`SimView`] once per game hour ([`collect`]), keeps its own cooldowns
//! in [`Advisor`] and changes nothing but UI state (overlay, camera goal, windows). When a
//! snackbar appears it records [`Advisor::just_shown`]; `draw_ui` turns that into an
//! [`AdvisorShown`] message, the hook for a chime (T-189).

use std::collections::BTreeMap;

use bevy::ecs::message::Message;
use bevy::math::Vec2;
use bevy_egui::egui;
use cp_building::{BuildingFlags, BuildingState};
use cp_core::time::TICKS_PER_DAY;
use cp_render::Overlay;
use cp_service::ServiceProblems;
use cp_sim::SimView;

use crate::icons::Icon;
use crate::palette::Category;
use crate::panels::UiState;
use crate::style::Sizes;

/// Seconds (UI time) a snackbar stays.
pub const SHOWN_SECS: f64 = 8.0;
/// Game days before the same advice may come again, unless it got worse.
pub const COOLDOWN_DAYS: u64 = 2;
/// Demand (±100) from which the city "wants" a zone type.
const HIGH_DEMAND: f32 = 30.0;
/// Side of the snackbar's square icon.
const ICON: f32 = Sizes::DEMAND_ICON;

/// Sent when a snackbar appears (a chime plays on it).
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdvisorShown {
    pub severity: Severity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warn,
    Critical,
}

/// What the "Show" button does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Overlay(Overlay),
    /// Moves the camera over the worst building (XZ, metres).
    Jump(Vec2),
    Panel(Category),
    Budget,
    /// The City policies window (T-193).
    Policies,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Advice {
    pub id: &'static str,
    pub severity: Severity,
    pub text: String,
    /// How bad it is (buildings affected, residents…); a rise lifts the cooldown.
    pub count: u32,
    pub icon: Icon,
    pub action: Option<Action>,
}

fn plural(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Counts and first position of the active buildings with a problem flag.
fn flagged(view: &SimView, bit: u8) -> (u32, Option<Vec2>) {
    let mut n = 0;
    let mut first = None;
    for (_, b) in view.buildings.iter() {
        if b.state == BuildingState::Active && b.flags.has(bit) {
            n += 1;
            first.get_or_insert(b.pos);
        }
    }
    (n, first)
}

/// Service problems (`ServiceProblems` bit, advice id, text, icon, the service's overlay).
const SERVICE_ADVICE: [(u16, &str, &str, Icon, Overlay); 5] = [
    (
        ServiceProblems::GARBAGE,
        "garbage",
        "buried in garbage",
        Icon::OverlayGarbage,
        Overlay::Garbage,
    ),
    (
        ServiceProblems::SICK,
        "sick",
        "with sick residents",
        Icon::OverlayHealth,
        Overlay::Health,
    ),
    (
        ServiceProblems::DEAD,
        "dead",
        "with uncollected dead",
        Icon::OverlayDeathcare,
        Overlay::Deathcare,
    ),
    (
        ServiceProblems::FIRE,
        "fire",
        "on fire",
        Icon::OverlayFire,
        Overlay::Fire,
    ),
    (
        ServiceProblems::CRIME,
        "crime",
        "hit by crime",
        Icon::OverlayPolice,
        Overlay::Police,
    ),
];

/// The unique buildings that were open at the last look (`None` before the first look, which
/// only takes the baseline: a loaded city does not announce what it already had).
pub type UniqueSeen = Option<std::collections::BTreeSet<String>>;

/// "Opened" advice (T-201) for the unique buildings and monuments that went from closed to
/// open since `seen`, one per kind with the names; `seen` is brought up to date.
fn unique_advice(view: &SimView, seen: &mut UniqueSeen) -> Vec<Advice> {
    if view.uniques.is_empty() {
        return Vec::new();
    }
    let now: std::collections::BTreeSet<String> = view
        .uniques
        .iter()
        .filter(|u| u.unlocked)
        .map(|u| u.id.clone())
        .collect();
    let Some(before) = seen.replace(now.clone()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (monument, id, what, icon) in [
        (false, "unique_unlocked", "Unique building", Icon::Buildings),
        (true, "monument_unlocked", "Monument", Icon::Buildings),
    ] {
        let of_kind = |u: &&cp_sim::UniqueStatus| u.monument == monument && u.unlocked;
        let names: Vec<&str> = view
            .uniques
            .iter()
            .filter(of_kind)
            .filter(|u| !before.contains(&u.id))
            .map(|u| {
                view.content
                    .as_deref()
                    .map_or(u.id.as_str(), |db| db.building(u.def).name.as_str())
            })
            .collect();
        if names.is_empty() {
            continue;
        }
        // Each opening raises the count by a whole step, which lifts the cooldown.
        let total = view.uniques.iter().filter(of_kind).count() as u32;
        out.push(Advice {
            id,
            severity: Severity::Info,
            text: format!("{what} unlocked: {}", names.join(", ")),
            count: total * 100,
            icon,
            action: Some(Action::Panel(Category::Buildings)),
        });
    }
    out
}

/// The level of each industry area (by id) at the last look; `None` before the first look,
/// which only takes the baseline.
pub type LevelSeen = Option<BTreeMap<u8, u8>>;

/// "Level up" advice (T-219) for the industry areas whose level rose since `seen`; `seen` is
/// brought up to date. Each rise comes once (the count carries the level, which lifts the
/// cooldown).
pub fn level_advice(view: &SimView, seen: &mut LevelSeen) -> Vec<Advice> {
    let now: BTreeMap<u8, u8> = view
        .areas
        .iter()
        .filter(|(_, a)| matches!(a.kind, cp_district::AreaKind::Industry(_)))
        .map(|(id, a)| (id.0, a.level))
        .collect();
    let Some(before) = seen.replace(now.clone()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (id, a) in view.areas.iter() {
        if before.get(&id.0).is_some_and(|&l| a.level > l) {
            out.push(Advice {
                id: "industry_level_up",
                severity: Severity::Info,
                text: format!("{} reached level {}", a.name, a.level),
                count: u32::from(a.level) * 100,
                icon: Icon::Industry,
                action: Some(Action::Panel(Category::Industry)),
            });
        }
    }
    out
}

/// Industry problems (T-219): areas without a main building, depleted and starved extractors
/// and processors, full warehouses.
fn industry_advice(view: &SimView, out: &mut Vec<Advice>) {
    use cp_content::IndustryRole;
    use cp_industry::IndustryProblem;
    let Some(db) = view.content.as_deref() else {
        return;
    };
    let bal = db.balance();
    // Per industry area: (has a main building, other chain buildings).
    let mut per_area: BTreeMap<u8, (bool, u32)> = BTreeMap::new();
    let (mut depleted, mut no_input, mut full) = (0u32, 0u32, 0u32);
    let (mut at_depleted, mut at_no_input, mut at_full) = (None, None, None);
    for (id, b) in view.buildings.iter() {
        let Some(spec) = &db.building(b.def).industry else {
            continue;
        };
        let area = cp_district::area_at(&view.area_map, b.pos);
        if let Some(a) = area {
            let e = per_area.entry(a.0).or_default();
            if spec.role == IndustryRole::Main {
                e.0 = true;
            } else {
                e.1 += 1;
            }
        }
        if b.state != BuildingState::Active {
            continue;
        }
        match view.industry.problem_of(id) {
            IndustryProblem::Depleted => {
                depleted += 1;
                at_depleted.get_or_insert(b.pos);
            }
            IndustryProblem::NoInput => {
                no_input += 1;
                at_no_input.get_or_insert(b.pos);
            }
            _ => {}
        }
        if spec.role == IndustryRole::Warehouse {
            let level = area.and_then(|a| view.areas.get(a)).map_or(1, |a| a.level);
            let cap = cp_industry::storage_cap(spec.storage, bal.industry_storage_per_level, level);
            if cap > 0.0 && view.industry.total(id) >= cap * 0.98 {
                full += 1;
                at_full.get_or_insert(b.pos);
            }
        }
    }
    let idle: Vec<&str> = view
        .areas
        .iter()
        .filter(|(id, a)| {
            matches!(a.kind, cp_district::AreaKind::Industry(_))
                && per_area
                    .get(&id.0)
                    .is_some_and(|&(main, others)| !main && others > 0)
        })
        .map(|(_, a)| a.name.as_str())
        .collect();
    if !idle.is_empty() {
        let n = idle.len() as u32;
        let text = if let [name] = idle[..] {
            format!("{name} has no main building — its buildings stand idle")
        } else {
            format!("{n} industry areas have no main building — their buildings stand idle")
        };
        out.push(Advice {
            id: "industry_no_main",
            severity: Severity::Warn,
            text,
            count: n,
            icon: Icon::Industry,
            action: Some(Action::Panel(Category::Industry)),
        });
    }
    let mut push = |id, n: u32, what: &str, at: Option<Vec2>| {
        if n > 0 {
            out.push(Advice {
                id,
                severity: Severity::Warn,
                text: format!("{} {what}", plural(n, "building", "buildings")),
                count: n,
                icon: Icon::Industry,
                action: Some(at.map_or(Action::Panel(Category::Industry), Action::Jump)),
            });
        }
    };
    push(
        "industry_depleted",
        depleted,
        "on spent ground — move the extractors to a richer lot",
        at_depleted,
    );
    push(
        "industry_no_input",
        no_input,
        "waiting for input — feed the processors from the extractors or a warehouse",
        at_no_input,
    );
    push(
        "warehouse_full",
        full,
        "full warehouses — export, or set a warehouse to Empty",
        at_full,
    );
}

/// All the advice the view gives now, unordered.
pub fn collect(view: &SimView, seen: &mut UniqueSeen) -> Vec<Advice> {
    let mut out = unique_advice(view, seen);
    industry_advice(view, &mut out);
    let city = &*view.city;
    let st = &city.buildings;
    let sev = |n: u32| {
        if n >= 10 {
            Severity::Critical
        } else {
            Severity::Warn
        }
    };
    let mut push = |id, severity, text: String, count, icon, action| {
        out.push(Advice {
            id,
            severity,
            text,
            count,
            icon,
            action,
        });
    };
    if view.treasury.balance < cp_core::Cost::ZERO {
        push(
            "debt",
            Severity::Critical,
            "The treasury is negative — raise taxes or cut budgets".into(),
            1,
            Icon::Budget,
            Some(Action::Budget),
        );
    } else if view.economy.last_week.week > 0 && view.economy.last_week.balance().0 < 0 {
        let loss = -view.economy.last_week.balance().0 / 100;
        push(
            "deficit",
            Severity::Warn,
            format!("The budget lost ${loss} last week"),
            u32::try_from(loss).unwrap_or(u32::MAX),
            Icon::Budget,
            Some(Action::Budget),
        );
    }
    if !city.connected_to_highway && st.active > 0 {
        push(
            "highway",
            Severity::Critical,
            "No road to the highway — nobody can move in".into(),
            1,
            Icon::Road,
            None,
        );
    }
    for (id, n, what, icon, overlay) in [
        (
            "no_water",
            st.no_water,
            "without water",
            Icon::OverlayWater,
            Overlay::Water,
        ),
        (
            "no_power",
            st.no_power,
            "without power",
            Icon::OverlayPower,
            Overlay::Power,
        ),
        (
            "no_sewage",
            st.no_sewage,
            "without sewage",
            Icon::OverlayWater,
            Overlay::Water,
        ),
    ] {
        if n > 0 {
            push(
                id,
                sev(n),
                format!("{} {what}", plural(n, "building", "buildings")),
                n,
                icon,
                Some(Action::Overlay(overlay)),
            );
        }
    }
    if st.burned > 0 {
        push(
            "burned",
            Severity::Warn,
            format!("{} burned down", plural(st.burned, "building", "buildings")),
            st.burned,
            Icon::OverlayFire,
            Some(Action::Overlay(Overlay::Fire)),
        );
    }
    if st.abandoned > 0 {
        push(
            "abandoned",
            Severity::Warn,
            format!(
                "{} abandoned",
                plural(st.abandoned, "building", "buildings")
            ),
            st.abandoned,
            Icon::Building,
            None,
        );
    }
    // BUG-058: say why a shop has no goods.
    let no_goods = if view.city.connected_to_highway {
        "out of goods — the factories are too small or short of workers, importing"
    } else {
        "out of goods — no road to the highway for imports"
    };
    for (id, bit, what) in [
        ("no_workers", BuildingFlags::NO_WORKERS, "short of workers"),
        ("no_goods", BuildingFlags::NO_GOODS, no_goods),
    ] {
        let (n, at) = flagged(view, bit);
        if n > 0 {
            push(
                id,
                sev(n),
                format!("{} {what}", plural(n, "building", "buildings")),
                n,
                Icon::Building,
                at.map(Action::Jump),
            );
        }
    }
    // Service problems per building (T-134 state): a count per kind.
    let counts = view.service_state.summary().counts;
    for (n, (_, id, text, icon, overlay)) in counts.into_iter().zip(SERVICE_ADVICE) {
        if n > 0 {
            // A policy of the city that helps, if it is defined and still off (T-193).
            let help = match id {
                "garbage" => Some(cp_content::PolicyKind::Recycling),
                "fire" => Some(cp_content::PolicyKind::SmokeDetectors),
                "crime" => Some(cp_content::PolicyKind::NeighborhoodWatch),
                _ => None,
            }
            .filter(|k| !view.city_policies.has(*k))
            .and_then(|k| view.content.as_deref()?.policy(k))
            .filter(|p| p.scope.allows_city());
            let mut line = format!("{} {text}", plural(n, "building", "buildings"));
            let mut action = Action::Overlay(overlay);
            if let Some(p) = help {
                line.push_str(&format!(" — try the {} policy", p.name));
                action = Action::Policies;
            }
            push(id, sev(n), line, n, icon, Some(action));
        }
    }
    // The policies in force eat more than a fifth of the income while the day ends in the red.
    if let Some(day) = view.flow.last_day(&view.economy)
        && day.net().0 < 0
        && day.policies.0 > 0
        && day.policies.0 * 5 > day.income().0
    {
        push(
            "policy_cost",
            Severity::Warn,
            format!(
                "Policies cost {} a day — over a fifth of the income",
                cp_tools::format_money(day.policies)
            ),
            u32::try_from(day.policies.0 / 100).unwrap_or(u32::MAX),
            Icon::Budget,
            Some(Action::Budget),
        );
    }
    if city.unemployed >= 20 && city.unemployed * 10 >= city.adults {
        push(
            "unemployed",
            Severity::Warn,
            format!(
                "{} without a job — zone industry or commerce",
                plural(city.unemployed, "resident", "residents")
            ),
            city.unemployed,
            Icon::ZoneIndustrial,
            Some(Action::Panel(Category::Zones)),
        );
    }
    let d = &view.demand;
    for (id, zone, v, text, icon) in [
        (
            "want_homes",
            cp_content::ZoneType::ResLow,
            d.r,
            "Residents want homes — zone residential",
            Icon::ZoneResLow,
        ),
        (
            "want_shops",
            cp_content::ZoneType::ComLow,
            d.c,
            "Shops are wanted — zone commercial",
            Icon::ZoneComLow,
        ),
        (
            "want_industry",
            cp_content::ZoneType::Industrial,
            d.i,
            "Industry is wanted — zone industrial",
            Icon::ZoneIndustrial,
        ),
        (
            "want_offices",
            cp_content::ZoneType::Office,
            d.o,
            "Offices are wanted — zone office",
            Icon::ZoneOffice,
        ),
    ] {
        // A zone type that is still locked is not advised (BUG-060).
        if v >= HIGH_DEMAND && crate::windows::zone_lock(view, zone).is_none() {
            push(
                id,
                Severity::Info,
                text.into(),
                v as u32,
                icon,
                Some(Action::Panel(Category::Zones)),
            );
        }
    }
    out
}

/// The snackbar on screen.
#[derive(Clone, Debug)]
struct Shown {
    advice: Advice,
    at: f64,
}

/// What the advisor remembers: cooldowns, the snackbar on screen, the last game hour looked at.
#[derive(Default)]
pub struct Advisor {
    last_hour: Option<u64>,
    /// Advice id -> (game day shown, count then).
    last: BTreeMap<&'static str, (u64, u32)>,
    shown: Option<Shown>,
    unique_seen: UniqueSeen,
    level_seen: LevelSeen,
    /// Severity of the snackbar that appeared in the last frame (taken by `draw_ui`).
    pub just_shown: Option<Severity>,
}

impl Advisor {
    /// The advice on screen at `now`.
    pub fn current(&self, now: f64) -> Option<&Advice> {
        self.shown
            .as_ref()
            .filter(|s| now - s.at < SHOWN_SECS)
            .map(|s| &s.advice)
    }

    /// Looks at the view when a new game hour has begun and shows the most urgent advice that
    /// is not on cooldown (a critical one replaces a milder snackbar). `problems_only` drops
    /// the info advice.
    pub fn observe(&mut self, view: &SimView, now: f64, problems_only: bool) {
        let hour = view.tick.0 * 24 / TICKS_PER_DAY;
        if self.last_hour == Some(hour) {
            return;
        }
        self.last_hour = Some(hour);
        let day = view.tick.0 / TICKS_PER_DAY;
        let on_screen = self.current(now).map(|a| a.severity);
        let mut all = collect(view, &mut self.unique_seen);
        all.extend(level_advice(view, &mut self.level_seen));
        all.retain(|a| !(problems_only && a.severity == Severity::Info));
        all.retain(|a| match self.last.get(a.id) {
            None => true,
            Some(&(shown_day, was)) => {
                day >= shown_day + COOLDOWN_DAYS || a.count > was + (was / 4).max(1)
            }
        });
        all.retain(|a| on_screen.is_none_or(|s| a.severity > s));
        // Most urgent first; the biggest problem of a severity, then the id, for a stable pick.
        all.sort_by(|a, b| {
            b.severity
                .cmp(&a.severity)
                .then(b.count.cmp(&a.count))
                .then(a.id.cmp(b.id))
        });
        if let Some(advice) = all.into_iter().next() {
            self.last.insert(advice.id, (day, advice.count));
            self.just_shown = Some(advice.severity);
            self.shown = Some(Shown { advice, at: now });
        }
    }

    fn close(&mut self) {
        self.shown = None;
    }
}

/// Runs the action of a "Show" click.
fn run(action: Action, state: &mut UiState, overlay: &mut cp_render::OverlayState) {
    match action {
        Action::Overlay(o) => overlay.active = o,
        Action::Jump(at) => state.camera_goal = Some(at),
        Action::Panel(c) => state.category = Some(c),
        Action::Budget => state.budget_open = true,
        Action::Policies => state.policies_open = true,
    }
}

fn colour(s: Severity) -> egui::Color32 {
    match s {
        Severity::Info => egui::Color32::from_rgb(90, 150, 220),
        Severity::Warn => egui::Color32::from_rgb(230, 170, 50),
        Severity::Critical => egui::Color32::from_rgb(220, 70, 60),
    }
}

/// Observes the view and draws the snackbar bottom-centre above the demand bar.
pub fn advisor_snackbar(
    ctx: &egui::Context,
    view: &SimView,
    state: &mut UiState,
    overlay: &mut cp_render::OverlayState,
    advisor: &mut Advisor,
) {
    let now = ctx.input(|i| i.time);
    if !state.prefs.advisor {
        advisor.close();
        return;
    }
    advisor.observe(view, now, state.prefs.advisor_problems_only);
    let Some(advice) = advisor.current(now).cloned() else {
        return;
    };
    let lift = if state.demand_collapsed {
        Sizes::DEMAND_COLLAPSED_H
    } else {
        Sizes::DEMAND_H
    };
    let (mut show, mut close) = (false, false);
    egui::Area::new("advisor".into())
        .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -(lift + 16.0)))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .stroke(egui::Stroke::new(2.0, colour(advice.severity)))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        advice.icon.show(ui, ICON);
                        ui.label(&advice.text);
                        if advice.action.is_some() {
                            show = ui.button("Show").clicked();
                        }
                        close = ui.small_button("×").clicked();
                    });
                });
        });
    if show && let Some(action) = advice.action {
        run(action, state, overlay);
    }
    if close || show {
        advisor.close();
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view_with(f: impl FnOnce(&mut cp_sim::SimRunner)) -> SimView {
        let content = std::sync::Arc::new(
            cp_content::ContentDb::load_dir(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/content"
            ))
            .unwrap(),
        );
        let mut sim = cp_sim::SimRunner::new(1);
        sim.set_content(content);
        f(&mut sim);
        sim.view()
    }

    /// T-201: an opening is announced once, by kind and with the building's name; the first
    /// look (a loaded city) only takes the baseline.
    #[test]
    fn unique_unlocked_comes_once_when_a_building_goes_from_closed_to_open() {
        let mut v = view_with(|s| {
            s.world_mut().insert_resource(cp_sim::Progress::START);
        });
        assert!(v.uniques.iter().all(|u| !u.unlocked));
        let ids = |a: &[Advice]| a.iter().map(|a| a.id).collect::<Vec<_>>();
        let mut seen = None;
        assert!(
            ids(&collect(&v, &mut seen))
                .iter()
                .all(|i| !i.ends_with("_unlocked"))
        );
        let open = |v: &mut SimView, id: &str| {
            let mut all = (*v.uniques).clone();
            all.iter_mut().find(|u| u.id == id).unwrap().unlocked = true;
            v.uniques = std::sync::Arc::new(all);
        };
        open(&mut v, "city_plaza");
        let a = collect(&v, &mut seen);
        let a = a
            .iter()
            .find(|a| a.id == "unique_unlocked")
            .expect("announced");
        assert!(a.text.contains("City plaza"), "{}", a.text);
        assert_eq!(a.action, Some(Action::Panel(Category::Buildings)));
        assert_eq!(a.severity, Severity::Info);
        // Again: nothing. A monument is its own advice.
        assert!(
            collect(&v, &mut seen)
                .iter()
                .all(|a| !a.id.ends_with("_unlocked"))
        );
        open(&mut v, "golden_arch");
        let a = collect(&v, &mut seen);
        assert!(
            a.iter()
                .any(|a| a.id == "monument_unlocked" && a.text.contains("Golden arch"))
        );
        assert!(a.iter().all(|a| a.id != "unique_unlocked"));
    }

    /// BUG-060: no "wanted" advice for a zone type that is still locked.
    #[test]
    fn no_wanted_advice_for_a_locked_zone_type() {
        let mut locked = view_with(|s| {
            s.world_mut().insert_resource(cp_sim::Progress::START);
        });
        locked.demand.o = 80.0;
        locked.demand.i = 80.0;
        let ids = |v: &SimView| {
            collect(v, &mut None)
                .iter()
                .map(|a| a.id)
                .collect::<Vec<_>>()
        };
        let got = ids(&locked);
        assert!(!got.contains(&"want_offices"), "{got:?}");
        assert!(got.contains(&"want_industry"), "industry is open: {got:?}");
        let open = view_with(|_| {});
        let mut open = open;
        open.demand.o = 80.0;
        assert!(ids(&open).contains(&"want_offices"));
    }

    /// A view whose city stats say `n` buildings lack water.
    fn no_water(n: u32) -> SimView {
        let mut v = view_with(|_| {});
        let mut city = (*v.city).clone();
        city.buildings.no_water = n;
        city.buildings.active = n.max(1);
        city.connected_to_highway = true;
        v.city = std::sync::Arc::new(city);
        v
    }

    /// T-193: garbage with the Recycling policy off adds "try the Recycling policy" and opens the
    /// City policies window; with it on the plain text and the overlay come back. A day in the
    /// red with the policies over a fifth of the income gives policy_cost.
    #[test]
    fn the_garbage_advice_suggests_the_policy_while_it_is_off_and_policy_cost_fires() {
        use cp_service::{BuildingServices, ServiceProblems, ServiceState};
        let mut v = no_water(0);
        let dirty = BuildingServices {
            problems: ServiceProblems(ServiceProblems::GARBAGE),
            ..BuildingServices::default()
        };
        v.service_state = std::sync::Arc::new(ServiceState::from_parts(
            vec![dirty, dirty],
            Default::default(),
        ));
        let all = collect(&v, &mut None);
        let a = all.iter().find(|a| a.id == "garbage").unwrap();
        assert!(a.text.ends_with("— try the Recycling policy"), "{}", a.text);
        assert_eq!(a.action, Some(Action::Policies));
        v.city_policies.mask = cp_content::PolicyKind::Recycling.bit();
        let all = collect(&v, &mut None);
        let a = all.iter().find(|a| a.id == "garbage").unwrap();
        assert!(!a.text.contains("try the"), "{}", a.text);
        assert_eq!(a.action, Some(Action::Overlay(Overlay::Garbage)));

        // No policy_cost on a quiet view; then a day with 30 of income against 12 of policies
        // and 40 of other costs.
        assert!(collect(&v, &mut None).iter().all(|a| a.id != "policy_cost"));
        let c = |units: f32| cp_core::Cost::from_units(units);
        let mut flow = cp_economy::Flow::default();
        flow.income_by_zone[0] = c(30.0);
        flow.policies = c(12.0);
        flow.road_upkeep = c(40.0);
        v.flow.hours.push_back((0, flow));
        let all = collect(&v, &mut None);
        let a = all
            .iter()
            .find(|a| a.id == "policy_cost")
            .expect("policy_cost");
        assert_eq!(a.severity, Severity::Warn);
        assert_eq!(a.action, Some(Action::Budget));
        // Policies under a fifth of the income: no advice even in the red.
        let mut flow = cp_economy::Flow::default();
        flow.income_by_zone[0] = c(100.0);
        flow.policies = c(10.0);
        flow.road_upkeep = c(120.0);
        v.flow.hours.clear();
        v.flow.hours.push_back((0, flow));
        assert!(collect(&v, &mut None).iter().all(|a| a.id != "policy_cost"));
    }

    fn at_tick(mut v: SimView, tick: u64) -> SimView {
        v.tick = cp_core::time::Tick(tick);
        v
    }

    #[test]
    fn five_houses_without_water_give_the_water_advice_with_the_overlay_action() {
        let all = collect(&no_water(5), &mut None);
        let a = all.iter().find(|a| a.id == "no_water").unwrap();
        assert_eq!(a.count, 5);
        assert_eq!(a.severity, Severity::Warn);
        assert!(
            a.text.starts_with("5 buildings without water"),
            "{}",
            a.text
        );
        assert_eq!(a.action, Some(Action::Overlay(Overlay::Water)));
    }

    #[test]
    fn service_problems_give_advice_with_the_matching_overlay() {
        use cp_service::{BuildingServices, ServiceProblems, ServiceState};
        let mut v = no_water(0);
        let sick = BuildingServices {
            problems: ServiceProblems(ServiceProblems::SICK | ServiceProblems::GARBAGE),
            ..BuildingServices::default()
        };
        let crime = BuildingServices {
            problems: ServiceProblems(ServiceProblems::CRIME),
            ..BuildingServices::default()
        };
        v.service_state = std::sync::Arc::new(ServiceState::from_parts(
            vec![sick, sick, crime],
            Default::default(),
        ));
        let all = collect(&v, &mut None);
        let get = |id| all.iter().find(|a| a.id == id).unwrap();
        assert_eq!(get("sick").count, 2);
        assert_eq!(get("sick").action, Some(Action::Overlay(Overlay::Health)));
        // The policies that help are off, so these two point at the City policies window.
        assert_eq!(get("garbage").action, Some(Action::Policies));
        assert_eq!(get("crime").count, 1);
        assert_eq!(get("crime").action, Some(Action::Policies));
        assert!(all.iter().all(|a| a.id != "fire" && a.id != "dead"));
    }

    #[test]
    fn a_repeat_waits_two_days_unless_it_got_worse() {
        let mut adv = Advisor::default();
        // A game hour is 170.67 ticks: 171 is the first tick of hour 1.
        let hour = TICKS_PER_DAY / 24 + 1;
        adv.observe(&at_tick(no_water(5), 0), 0.0, true);
        assert_eq!(adv.just_shown.take(), Some(Severity::Warn));
        // Snackbar gone, same problem an hour later and a day later: silent.
        adv.observe(&at_tick(no_water(5), hour), 20.0, true);
        adv.observe(&at_tick(no_water(5), TICKS_PER_DAY), 40.0, true);
        assert_eq!(adv.just_shown, None);
        // Clearly worse: shown again at once.
        adv.observe(&at_tick(no_water(9), TICKS_PER_DAY + 2 * hour), 60.0, true);
        assert_eq!(adv.just_shown.take(), Some(Severity::Warn));
        // Two days after that: again, though not worse.
        adv.observe(&at_tick(no_water(9), 4 * TICKS_PER_DAY), 80.0, true);
        assert_eq!(adv.just_shown.take(), Some(Severity::Warn));
    }

    #[test]
    fn a_critical_advice_beats_a_milder_one_and_replaces_it() {
        let mut v = no_water(5);
        let mut city = (*v.city).clone();
        city.buildings.no_power = 12;
        v.city = std::sync::Arc::new(city);
        let mut adv = Advisor::default();
        adv.observe(&at_tick(v, 0), 0.0, true);
        assert_eq!(adv.current(1.0).unwrap().id, "no_power");
        assert_eq!(adv.just_shown, Some(Severity::Critical));
        // Info never shows with "only problems", and a milder one does not push out a shown one.
        let mut info = no_water(0);
        info.demand.r = 80.0;
        let mut adv = Advisor::default();
        adv.observe(&at_tick(info.clone(), 0), 0.0, true);
        assert!(adv.current(1.0).is_none());
        adv.observe(&at_tick(info, TICKS_PER_DAY / 24 + 1), 1.0, false);
        assert_eq!(adv.current(2.0).unwrap().id, "want_homes");
    }

    #[test]
    fn the_snackbar_hides_after_its_time_and_show_runs_the_action() {
        let mut adv = Advisor::default();
        adv.observe(&at_tick(no_water(5), 0), 0.0, true);
        assert!(adv.current(SHOWN_SECS - 0.1).is_some());
        assert!(adv.current(SHOWN_SECS + 0.1).is_none());
        let mut state = UiState::default();
        let mut overlay = cp_render::OverlayState::default();
        run(Action::Overlay(Overlay::Water), &mut state, &mut overlay);
        assert_eq!(overlay.active, Overlay::Water);
        run(Action::Jump(Vec2::new(1.0, 2.0)), &mut state, &mut overlay);
        assert_eq!(state.camera_goal, Some(Vec2::new(1.0, 2.0)));
    }

    /// T-219: an area with chain buildings and no Main building gives `industry_no_main`; a
    /// Main in the area takes it away. Spent ground, starved processors and a full warehouse
    /// give their own advice.
    #[test]
    fn industry_advice_names_an_area_without_main_and_the_stuck_buildings() {
        use crate::industry::tests::{put, world_with_areas};
        use cp_industry::{IndustryProblem, IndustryState};
        let (mut sim, db, at) = world_with_areas(1);
        let ids = |sim: &cp_sim::SimRunner| -> Vec<&'static str> {
            collect(&sim.view(), &mut None)
                .iter()
                .map(|a| a.id)
                .collect()
        };
        assert!(!ids(&sim).contains(&"industry_no_main"));
        let ex = put(&mut sim, &db, "forest_extractor_s", at[0]);
        assert!(ids(&sim).contains(&"industry_no_main"), "{:?}", ids(&sim));
        put(&mut sim, &db, "forest_main", at[0]);
        assert!(!ids(&sim).contains(&"industry_no_main"));

        let wh = put(&mut sim, &db, "forest_warehouse", at[0]);
        let mut st = IndustryState::default();
        st.problems.insert(ex, IndustryProblem::Depleted);
        sim.world_mut().insert_resource(st.clone());
        let got = ids(&sim);
        assert!(got.contains(&"industry_depleted"), "{got:?}");
        assert!(!got.contains(&"warehouse_full"));
        st.problems.insert(ex, IndustryProblem::NoInput);
        st.stock
            .insert(wh, vec![(cp_content::CargoKind::Logs, 1.0e6)]);
        sim.world_mut().insert_resource(st);
        let got = ids(&sim);
        assert!(got.contains(&"industry_no_input"), "{got:?}");
        assert!(got.contains(&"warehouse_full"), "{got:?}");
        assert!(!got.contains(&"industry_depleted"));
    }

    /// T-219: a rise of an area's level comes once; the first look is only the baseline.
    #[test]
    fn industry_level_up_comes_once_per_rise() {
        use crate::industry::tests::world_with_areas;
        let (mut sim, _, _) = world_with_areas(1);
        let mut seen = None;
        assert!(level_advice(&sim.view(), &mut seen).is_empty());
        assert!(level_advice(&sim.view(), &mut seen).is_empty());
        sim.world_mut()
            .resource_mut::<cp_district::Areas>()
            .get_mut(cp_district::AreaId(1))
            .unwrap()
            .level = 2;
        let got = level_advice(&sim.view(), &mut seen);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "industry_level_up");
        assert!(got[0].text.contains("level 2"), "{}", got[0].text);
        assert!(level_advice(&sim.view(), &mut seen).is_empty());
    }
}
