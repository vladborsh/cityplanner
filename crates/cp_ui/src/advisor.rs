//! The city advisor (T-188): a snackbar above the demand bar that names the city's current
//! problems and needs, one at a time, at most once every couple of game days per subject.
//!
//! UI only: it reads the [`SimView`] once per game hour ([`collect`]), keeps its own cooldowns
//! in [`Advisor`] and changes nothing but UI state (overlay, camera goal, windows). When a
//! snackbar appears it records [`Advisor::just_shown`]; `draw_ui` turns that into an
//! [`AdvisorShown`] message, the hook for a chime (T-189).

use std::collections::{BTreeMap, BTreeSet};

use bevy::ecs::message::Message;
use bevy::math::Vec2;
use bevy_egui::egui;
use cp_building::{BuildingFlags, BuildingState};
use cp_content::ServiceKind;
use cp_core::time::TICKS_PER_DAY;
use cp_locale::{tr, trf, trn};
use cp_render::Overlay;
use cp_service::ServiceProblems;
use cp_sim::SimView;

use crate::icons::Icon;
use crate::palette::Category;
use crate::panels::UiState;
use crate::style::Sizes;

/// Seconds (UI time) a snackbar stays.
pub const SHOWN_SECS: f64 = 8.0;
/// Game days before the same advice may come again, unless it got much worse (BUG-115).
pub const COOLDOWN_DAYS: u64 = 3;
/// BUG-115: it takes this many times the count (at least one more) to lift the cooldown.
pub const WORSE_FACTOR: u32 = 2;
/// BUG-115: a problem notifies only after it has stood this many game hours (half a day).
pub const PERSIST_HOURS: u64 = 12;
/// BUG-115: at most one new city-problem notification per this many game hours (a day).
pub const PROBLEM_GAP_HOURS: u64 = 24;
/// BUG-115: a problem of fewer buildings than this, or than [`MIN_SHARE_PERCENT`] of the
/// active buildings, is not worth a notification.
pub const MIN_BUILDINGS: u32 = 5;
pub const MIN_SHARE_PERCENT: u32 = 3;
/// The bike lanes hint (T-322c): its id, the population and the overloaded segments it needs.
const BIKE_HINT: &str = "bike_lanes";
const BIKE_HINT_POP: u32 = 1_500;
const BIKE_HINT_OVERLOADED: u32 = 10;

/// BUG-115: this many problems at once become one "N problems" entry.
pub const MERGE_FROM: usize = 3;
/// The advice ids that count buildings (the small-count rule applies to these).
const BUILDING_COUNTS: [&str; 10] = [
    "no_water",
    "no_power",
    "no_sewage",
    "burned",
    "abandoned",
    "garbage",
    "sick",
    "dead",
    "fire",
    "crime",
];
/// Demand (±100) from which the city "wants" a zone type.
const HIGH_DEMAND: f32 = 30.0;
/// Side of the snackbar's square icon.
const ICON: f32 = Sizes::VIEW_MENU_ICON;

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
    /// The Service budgets tab with the row of a kind highlighted (§20.3).
    BudgetRow(ServiceKind),
    /// The City policies window (T-193).
    Policies,
    /// Opens a window of the top bar (T-307).
    Window(crate::panels::UiWindow),
    /// The Pollution overlay and the camera over the dirtiest water source (T-300).
    PollutedSource(Vec2),
    /// The Noise overlay and the Trees tool in hand (T-323, §24.2): trees along the road cut noise.
    Trees,
    /// The Roads tray on one tab (T-322c).
    RoadTab(crate::roads::RoadTab),
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

/// The text of a service problem advice (`id` of [`SERVICE_ADVICE`]) for `n` buildings, with
/// the name of a policy that helps, if any.
fn service_text(id: &str, n: u32, policy: Option<&str>) -> String {
    match (id, policy) {
        ("garbage", None) => trn("{n} buildings buried in garbage", i64::from(n)),
        ("garbage", Some(p)) => trf!(
            plural "{n} buildings buried in garbage — try the {policy} policy",
            n = n,
            policy = tr(p)
        ),
        ("sick", None) => trn("{n} buildings with sick residents", i64::from(n)),
        ("sick", Some(p)) => trf!(
            plural "{n} buildings with sick residents — try the {policy} policy",
            n = n,
            policy = tr(p)
        ),
        ("dead", None) => trn("{n} buildings with uncollected dead", i64::from(n)),
        ("dead", Some(p)) => trf!(
            plural "{n} buildings with uncollected dead — try the {policy} policy",
            n = n,
            policy = tr(p)
        ),
        ("fire", None) => trn("{n} buildings on fire", i64::from(n)),
        ("fire", Some(p)) => trf!(
            plural "{n} buildings on fire — try the {policy} policy",
            n = n,
            policy = tr(p)
        ),
        (_, None) => trn("{n} buildings hit by crime", i64::from(n)),
        (_, Some(p)) => trf!(
            plural "{n} buildings hit by crime — try the {policy} policy",
            n = n,
            policy = tr(p)
        ),
    }
}

/// Counts and first position of the active buildings with a problem flag.
fn flagged(view: &SimView, bit: u16) -> (u32, Option<Vec2>) {
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

/// Service problems (`ServiceProblems` bit, advice id, icon, the service's overlay); the text is
/// [`service_text`].
const SERVICE_ADVICE: [(u16, &str, Icon, Overlay); 5] = [
    (
        ServiceProblems::GARBAGE,
        "garbage",
        Icon::OverlayGarbage,
        Overlay::Garbage,
    ),
    (
        ServiceProblems::SICK,
        "sick",
        Icon::OverlayHealth,
        Overlay::Health,
    ),
    (
        ServiceProblems::DEAD,
        "dead",
        Icon::OverlayDeathcare,
        Overlay::Deathcare,
    ),
    (
        ServiceProblems::FIRE,
        "fire",
        Icon::OverlayFire,
        Overlay::Fire,
    ),
    (
        ServiceProblems::CRIME,
        "crime",
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
    for (monument, id, icon) in [
        (false, "unique_unlocked", Icon::Buildings),
        (true, "monument_unlocked", Icon::Buildings),
    ] {
        let of_kind = |u: &&cp_sim::UniqueStatus| u.monument == monument && u.unlocked;
        let names: Vec<String> = view
            .uniques
            .iter()
            .filter(of_kind)
            .filter(|u| !before.contains(&u.id))
            .map(|u| {
                view.content
                    .as_deref()
                    .map_or(u.id.as_str(), |db| db.building(u.def).name.as_str())
            })
            .map(tr)
            .collect();
        if names.is_empty() {
            continue;
        }
        // Each opening raises the count by a whole step, which lifts the cooldown.
        let total = view.uniques.iter().filter(of_kind).count() as u32;
        out.push(Advice {
            id,
            severity: Severity::Info,
            text: if monument {
                trf!("Monument unlocked: {names}", names = names.join(", "))
            } else {
                trf!(
                    "Unique building unlocked: {names}",
                    names = names.join(", ")
                )
            },
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
    let now: BTreeMap<u8, u8> = view.areas.iter().map(|(id, a)| (id.0, a.level)).collect();
    let Some(before) = seen.replace(now.clone()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (id, a) in view.areas.iter() {
        if before.get(&id.0).is_some_and(|&l| a.level > l) {
            let (adv_id, icon, panel) = match a.kind {
                cp_district::AreaKind::Industry(_) => {
                    ("industry_level_up", Icon::Industry, Category::Industry)
                }
                cp_district::AreaKind::Park(_) => {
                    ("park_level_up", Icon::CategoryParks, Category::Parks)
                }
            };
            // T-337: an industry area says what the level opened.
            let opened = match (a.kind, view.content.as_deref()) {
                (cp_district::AreaKind::Industry(res), Some(db)) => {
                    let names: Vec<String> = crate::industry_data::area_unlocks(db, res, a.level)
                        .into_iter()
                        .take(2)
                        .map(|d| tr(&db.building(d).name))
                        .collect();
                    (!names.is_empty()).then(|| names.join(", "))
                }
                _ => None,
            };
            let text = match opened {
                Some(buildings) => trf!(
                    "{name}: level {level}. Opened: {buildings} · storage +10%",
                    name = a.name,
                    level = a.level,
                    buildings = buildings
                ),
                None => trf!(
                    "{name} reached level {level}",
                    name = a.name,
                    level = a.level
                ),
            };
            out.push(Advice {
                id: adv_id,
                severity: Severity::Info,
                text,
                count: u32::from(a.level) * 100,
                icon,
                action: Some(match a.kind {
                    cp_district::AreaKind::Industry(_) => {
                        Action::Window(crate::panels::UiWindow::Industry)
                    }
                    _ => Action::Panel(panel),
                }),
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
            trf!(
                "{name} has no main building — its buildings stand idle",
                name = name
            )
        } else {
            trf!(
                plural "{n} industry areas have no main building — their buildings stand idle",
                n = n
            )
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
    let mut push = |id, n: u32, key: &str, at: Option<Vec2>| {
        if n > 0 {
            out.push(Advice {
                id,
                severity: Severity::Warn,
                text: trn(key, i64::from(n)),
                count: n,
                icon: Icon::Industry,
                action: Some(at.map_or(Action::Panel(Category::Industry), Action::Jump)),
            });
        }
    };
    push(
        "industry_depleted",
        depleted,
        "{n} buildings on spent ground — move the extractors to a richer lot",
        at_depleted,
    );
    push(
        "industry_no_input",
        no_input,
        "{n} buildings waiting for input — feed the processors from the extractors or a warehouse",
        at_no_input,
    );
    push(
        "warehouse_full",
        full,
        "{n} buildings full warehouses — export, or set a warehouse to Empty",
        at_full,
    );
}

/// Park advice (T-229): an area without a gate, a full park, a town with no leisure at all.
fn park_advice(view: &SimView, out: &mut Vec<Advice>) {
    use cp_district::AreaKind;
    let Some(db) = view.content.as_deref() else {
        return;
    };
    let (mut no_gate, mut full) = (Vec::new(), 0u32);
    let mut full_at = None;
    for (id, a) in view.areas.iter() {
        if !matches!(a.kind, AreaKind::Park(_)) || view.area_map.cell_count(id) == 0 {
            continue;
        }
        let Some(s) = crate::parks::park_summary(view, id) else {
            continue;
        };
        if s.gates == 0 {
            no_gate.push(a.name.as_str());
        } else if s.capacity > 0 && s.inside * 100 >= s.capacity * 95 {
            full += 1;
            full_at = full_at.or(s.gate_at);
        }
    }
    if !no_gate.is_empty() {
        let n = no_gate.len() as u32;
        let text = if let [name] = no_gate[..] {
            trf!("{name} has no gate — nobody can visit it", name = name)
        } else {
            trf!(
                plural "{n} park areas have no gate — nobody can visit them",
                n = n
            )
        };
        out.push(Advice {
            id: "park_no_gate",
            severity: Severity::Warn,
            text,
            count: n,
            icon: Icon::CategoryParks,
            action: Some(Action::Panel(Category::Parks)),
        });
    }
    if full > 0 {
        out.push(Advice {
            id: "park_full",
            severity: Severity::Info,
            text: trn(
                "{n} parks full — add attractions, or raise the fee",
                i64::from(full),
            ),
            count: full,
            icon: Icon::CategoryParks,
            action: full_at.map(Action::Jump),
        });
    }
    let has_leisure = view
        .buildings
        .iter()
        .any(|(_, b)| db.building(b.def).park.is_some());
    if view.city.population >= 2_000 && !has_leisure {
        out.push(Advice {
            id: "leisure_none",
            severity: Severity::Info,
            text: tr("The town has no park — people want somewhere to go"),
            count: 1,
            icon: Icon::CategoryParks,
            action: Some(Action::Panel(Category::Buildings)),
        });
    }
}

/// Mail advice (T-249), only once the post office is open: mail piling up in buildings, and the
/// post offices' collecting stopped because the unsorted backlog is over the sorting capacity.
fn mail_advice(view: &SimView, out: &mut Vec<Advice>) {
    let Some(db) = view.content.as_deref() else {
        return;
    };
    let open = |id: &str| {
        db.building_id(id)
            .is_some_and(|def| view.progress.locked_by(db, def).is_none())
    };
    if !open("post_office") {
        return;
    }
    let n = view.service_state.summary().counts[5];
    if n > 0 {
        out.push(Advice {
            id: "mail",
            severity: if n >= 10 {
                Severity::Warn
            } else {
                Severity::Info
            },
            text: trn(
                "Mail is piling up in {n} buildings — build a post office",
                i64::from(n),
            ),
            count: n,
            icon: Icon::ProblemMail,
            action: Some(Action::Overlay(Overlay::Mail)),
        });
    }
    if view.mail.unsorted > 0 && view.mail.backlog_full(db.balance().mail_backlog_days) {
        let hint = if open("post_sorting_facility") {
            "Post offices are full — build a sorting facility"
        } else {
            "Post offices are full — the sorting facility is not open yet"
        };
        out.push(Advice {
            id: "mail_sorting",
            severity: Severity::Warn,
            text: tr(hint),
            count: view.mail.unsorted,
            icon: Icon::OverlayMail,
            action: Some(Action::Panel(Category::Buildings)),
        });
    }
}

/// "Bad water in N homes" (T-300): homes drinking from a polluted pool. "Show" opens the
/// Pollution overlay and moves the camera to the dirtiest water source.
fn bad_water_advice(view: &SimView, out: &mut Vec<Advice>) {
    let n = view.service_state.summary().counts[6];
    if n == 0 {
        return;
    }
    out.push(Advice {
        id: "bad_water",
        severity: if n >= 10 {
            Severity::Critical
        } else {
            Severity::Warn
        },
        text: trn(
            "Bad water in {n} homes — polluted ground at a pump",
            i64::from(n),
        ),
        count: n,
        icon: Icon::ProblemBadWater,
        action: Some(
            dirtiest_water_source(view)
                .map_or(Action::Overlay(Overlay::Pollution), Action::PollutedSource),
        ),
    });
}

/// The water source (pump or tower) standing on the most polluted ground.
fn dirtiest_water_source(view: &SimView) -> Option<Vec2> {
    let db = view.content.as_deref()?;
    let grid = view.overlays.pollution.as_deref()?;
    let cell = cp_terrain::MAP_SIZE / grid.side as f32;
    let at = |p: Vec2| {
        let c = |v: f32| ((v / cell).floor().max(0.0) as usize).min(grid.side - 1);
        grid.values[c(p.y) * grid.side + c(p.x)]
    };
    view.buildings
        .iter()
        .filter(|(_, b)| {
            db.building(b.def)
                .service
                .as_deref()
                .and_then(|s| db.service_id(s))
                .is_some_and(|s| db.service(s).kind == cp_content::ServiceKind::Water)
        })
        .map(|(_, b)| (at(b.pos), b.pos))
        .filter(|(dirt, _)| *dirt > 0.0)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, p)| p)
}

/// All the advice the view gives now, unordered.
/// The ids of the budget advice per budgeted kind with a sector: `(kind, low, waste)`.
const BUDGET_ADVICE: [(ServiceKind, &str, &str); 10] = [
    (ServiceKind::Water, "budget_low_water", "budget_waste_water"),
    (
        ServiceKind::Sewage,
        "budget_low_sewage",
        "budget_waste_sewage",
    ),
    (ServiceKind::Power, "budget_low_power", "budget_waste_power"),
    (
        ServiceKind::Garbage,
        "budget_low_garbage",
        "budget_waste_garbage",
    ),
    (
        ServiceKind::Health,
        "budget_low_health",
        "budget_waste_health",
    ),
    (
        ServiceKind::Deathcare,
        "budget_low_deathcare",
        "budget_waste_deathcare",
    ),
    (
        ServiceKind::Police,
        "budget_low_police",
        "budget_waste_police",
    ),
    (ServiceKind::Fire, "budget_low_fire", "budget_waste_fire"),
    (
        ServiceKind::Education,
        "budget_low_education",
        "budget_waste_education",
    ),
    (ServiceKind::Post, "budget_low_post", "budget_waste_post"),
];
/// A sector this far below its demand (balance) with its budget under 100 % asks for more.
pub const BUDGET_LOW_BALANCE: f32 = -0.2;
/// ... this far above it with the budget over 100 % may be cut.
pub const BUDGET_WASTE_BALANCE: f32 = 0.5;

/// §20.3: "Water is short: raise its budget (now 60 %)" when a sector falls short and its
/// budget is under 100 % (a warning, so it waits [`PERSIST_HOURS`] like the others); "You can
/// cut the Police budget" when it has plenty over 100 %. Short at 100 % or more stays the
/// "build more" advice. "Show" opens the budget tab on the row.
fn budget_advice(view: &SimView, out: &mut Vec<Advice>) {
    let open = crate::budget::budget_rows(view);
    for (kind, low, waste) in BUDGET_ADVICE {
        if !open.contains(&kind) {
            continue;
        }
        let Some(balance) = crate::budget::budget_balance(view, kind) else {
            continue;
        };
        let pct = view.budgets.get(kind);
        let name = tr(crate::buildings::kind_label(kind).as_str());
        let icon = crate::buildings::kind_icon(kind);
        if balance < BUDGET_LOW_BALANCE && pct < 100 {
            out.push(Advice {
                id: low,
                severity: Severity::Warn,
                text: trf!(
                    "{name} is short: raise its budget (now {pct}%)",
                    name = name,
                    pct = pct
                ),
                count: (-balance * 100.0) as u32,
                icon,
                action: Some(Action::BudgetRow(kind)),
            });
        } else if balance > BUDGET_WASTE_BALANCE && pct > 100 {
            out.push(Advice {
                id: waste,
                severity: Severity::Info,
                text: trf!("You can cut the {name} budget", name = name),
                count: 1,
                icon,
                action: Some(Action::BudgetRow(kind)),
            });
        }
    }
}

/// The ids of the market advice per cargo, in [`cp_content::CargoKind::ALL`] order:
/// `(shortage, glut, peak)` (§21.3).
const MARKET_ADVICE: [(&str, &str, &str); 13] = [
    (
        "market_shortage_goods",
        "market_glut_goods",
        "market_peak_goods",
    ),
    (
        "market_shortage_crops",
        "market_glut_crops",
        "market_peak_crops",
    ),
    (
        "market_shortage_logs",
        "market_glut_logs",
        "market_peak_logs",
    ),
    ("market_shortage_ore", "market_glut_ore", "market_peak_ore"),
    ("market_shortage_oil", "market_glut_oil", "market_peak_oil"),
    (
        "market_shortage_animal_products",
        "market_glut_animal_products",
        "market_peak_animal_products",
    ),
    (
        "market_shortage_flour",
        "market_glut_flour",
        "market_peak_flour",
    ),
    (
        "market_shortage_planks",
        "market_glut_planks",
        "market_peak_planks",
    ),
    (
        "market_shortage_paper",
        "market_glut_paper",
        "market_peak_paper",
    ),
    (
        "market_shortage_metals",
        "market_glut_metals",
        "market_peak_metals",
    ),
    (
        "market_shortage_glass",
        "market_glut_glass",
        "market_peak_glass",
    ),
    (
        "market_shortage_petroleum",
        "market_glut_petroleum",
        "market_peak_petroleum",
    ),
    (
        "market_shortage_plastics",
        "market_glut_plastics",
        "market_peak_plastics",
    ),
];
/// A cargo whose export pays this many times its content price is "dear" (§21.3).
pub const MARKET_PEAK: f32 = 1.3;

/// §21.3: a shortage of a cargo the city imported last week ("fill your warehouses"), a glut of
/// one it exported, and a dear cargo held by a warehouse in `Fill` ("switch it to Empty"; Show
/// goes to the warehouse that holds the most). [`Advisor::observe`] shows at most one a day.
fn market_advice(view: &SimView, out: &mut Vec<Advice>) {
    use cp_content::{CargoKind, MarketEventKind};
    use cp_industry::WarehouseMode;
    let Some(db) = view.content.as_deref() else {
        return;
    };
    let k = &db.balance().market;
    let day = view.tick.0 / TICKS_PER_DAY;
    let last = &view.industry_ledger.last_by_cargo;
    for (cargo, (shortage, glut, peak)) in CargoKind::ALL.into_iter().zip(MARKET_ADVICE) {
        let i = cargo as usize;
        let name = tr(&format!("{cargo:?}"));
        let m = view.market.get(cargo);
        // Goods come in to the shops through the transit ledger (M3), the chains' cargo through
        // the industry ledger.
        let imported = last[i].import > 0
            || (cargo == CargoKind::Goods && view.transit_ledger.import_loads > 0);
        match m.event {
            Some(e) if e.kind == MarketEventKind::Shortage && imported => {
                let n = u64::from(e.until_day).saturating_sub(day).max(1) as u32;
                out.push(Advice {
                    id: shortage,
                    severity: Severity::Warn,
                    text: trf!(plural
                        "Shortage of {cargo}: imports fail half the time and cost x2 for {n} days — fill your warehouses",
                        n = n,
                        cargo = name
                    ),
                    count: 1,
                    icon: Icon::Industry,
                    action: Some(Action::Window(crate::panels::UiWindow::Metrics)),
                });
                continue;
            }
            Some(e) if e.kind == MarketEventKind::Glut && last[i].export > 0 => {
                out.push(Advice {
                    id: glut,
                    severity: Severity::Info,
                    text: trf!("{cargo} sells for half this week", cargo = name),
                    count: 1,
                    icon: Icon::Industry,
                    action: Some(Action::Window(crate::panels::UiWindow::Metrics)),
                });
                continue;
            }
            _ => {}
        }
        if view.market.export_factor(k, cargo, view.tick) < MARKET_PEAK {
            continue;
        }
        let fullest = view
            .warehouses
            .list
            .iter()
            .filter(|(_, s)| s.mode == WarehouseMode::Fill && s.handles(cargo))
            .map(|(&id, _)| (id, view.industry.stock_of(id, cargo)))
            .filter(|&(_, held)| held > 0.0)
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some(pos) = fullest.and_then(|(id, _)| view.buildings.get(id).map(|b| b.pos)) {
            out.push(Advice {
                id: peak,
                severity: Severity::Info,
                text: trf!(
                    "{cargo} is dear: switch the warehouse to Empty",
                    cargo = name
                ),
                count: 1,
                icon: Icon::Industry,
                action: Some(Action::Jump(pos)),
            });
        }
    }
}

pub fn collect(view: &SimView, seen: &mut UniqueSeen) -> Vec<Advice> {
    let mut out = unique_advice(view, seen);
    budget_advice(view, &mut out);
    market_advice(view, &mut out);
    industry_advice(view, &mut out);
    park_advice(view, &mut out);
    mail_advice(view, &mut out);
    bad_water_advice(view, &mut out);
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
    // T-242: most trips by car and the roads are jammed.
    let traffic = &city.traffic;
    let moving: u32 = traffic.trips_by_mode.iter().sum();
    if moving >= 40
        && traffic.trips_by_mode[1] * 10 >= moving * 7
        && (traffic.overloaded >= 3 || traffic.avg_load > 0.3)
    {
        push(
            "traffic_jam",
            Severity::Warn,
            tr("Traffic jams: add public transport or footpaths"),
            traffic.overloaded.max(1),
            Icon::OverlayTraffic,
            Some(Action::Overlay(Overlay::Traffic)),
        );
    }
    if view.treasury.balance < cp_core::Cost::ZERO {
        push(
            "debt",
            Severity::Critical,
            tr("The treasury is negative — raise taxes or cut budgets"),
            1,
            Icon::Budget,
            Some(Action::Budget),
        );
    } else if view.economy.last_week.week > 0 && view.economy.last_week.balance().0 < 0 {
        let loss = -view.economy.last_week.balance().0 / 100;
        push(
            "deficit",
            Severity::Warn,
            trf!("The budget lost ${loss} last week", loss = loss),
            u32::try_from(loss).unwrap_or(u32::MAX),
            Icon::Budget,
            Some(Action::Budget),
        );
    }
    if !city.connected_to_highway && st.active > 0 {
        push(
            "highway",
            Severity::Critical,
            tr("No road to the highway — nobody can move in"),
            1,
            Icon::Road,
            None,
        );
    }
    for (id, n, key, icon, overlay) in [
        (
            "no_water",
            st.no_water,
            "{n} buildings without water",
            Icon::OverlayWater,
            Overlay::Water,
        ),
        (
            "no_power",
            st.no_power,
            "{n} buildings without power",
            Icon::OverlayPower,
            Overlay::Power,
        ),
        (
            "no_sewage",
            st.no_sewage,
            "{n} buildings without sewage",
            Icon::OverlayWater,
            Overlay::Water,
        ),
    ] {
        if n > 0 {
            push(
                id,
                sev(n),
                trn(key, i64::from(n)),
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
            trn("{n} buildings burned down", i64::from(st.burned)),
            st.burned,
            Icon::OverlayFire,
            Some(Action::Overlay(Overlay::Fire)),
        );
    }
    if st.abandoned > 0 {
        push(
            "abandoned",
            Severity::Warn,
            trn("{n} buildings abandoned", i64::from(st.abandoned)),
            st.abandoned,
            Icon::Building,
            None,
        );
    }
    // BUG-058: say why a shop has no goods.
    let no_goods = if view.city.connected_to_highway {
        "{n} buildings out of goods — the factories are too small or short of workers, importing"
    } else {
        "{n} buildings out of goods — no road to the highway for imports"
    };
    for (id, bit, key) in [
        (
            "no_workers",
            BuildingFlags::NO_WORKERS,
            "{n} buildings short of workers",
        ),
        ("no_goods", BuildingFlags::NO_GOODS, no_goods),
    ] {
        let (n, at) = flagged(view, bit);
        if n > 0 {
            push(
                id,
                sev(n),
                trn(key, i64::from(n)),
                n,
                Icon::Building,
                at.map(Action::Jump),
            );
        }
    }
    // Service problems per building (T-134 state): a count per kind.
    let counts = view.service_state.summary().counts;
    for (n, (_, id, icon, overlay)) in counts.into_iter().zip(SERVICE_ADVICE) {
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
            let line = service_text(id, n, help.map(|p| p.name.as_str()));
            let action = if help.is_some() {
                Action::Policies
            } else {
                Action::Overlay(overlay)
            };
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
            trf!(
                "Policies cost {money} a day — over a fifth of the income",
                money = cp_tools::format_money(day.policies)
            ),
            u32::try_from(day.policies.0 / 100).unwrap_or(u32::MAX),
            Icon::Budget,
            Some(Action::Budget),
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
            // BUG-076b: wanted, but no free cell can take a building: say so instead.
            let text = crate::windows::land_shortage(view, zone).unwrap_or_else(|| tr(text));
            push(
                id,
                Severity::Info,
                text,
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
    /// Advice id -> the game hour it was first seen in the current run (BUG-115).
    since: BTreeMap<&'static str, u64>,
    /// The game day the work and education problem began (T-307); `None` while there is none.
    jobs_problem_since: Option<u64>,
    /// The game day of the last reminder about it.
    jobs_last: Option<u64>,
    /// T-321: the game day the hotels were last looked at, and the days running they were
    /// nearly full or nearly empty; whether the first summer was announced.
    hotel_day: Option<u64>,
    hotels_full_days: u32,
    hotels_empty_days: u32,
    summer_told: bool,
    /// The game hour of the last problem notification.
    last_problem_hour: Option<u64>,
    /// The game day of the last market advice (§21.3: at most one a day).
    last_market_day: Option<u64>,
    /// The bike lanes hint (T-322c) was shown in this session.
    bike_hint_shown: bool,
    /// Severity of the snackbar that appeared in the last frame (taken by `draw_ui`).
    pub just_shown: Option<Severity>,
    /// The offers of the inbox (deal, last day) and the scandals the toasts of the Power window
    /// have seen (T-335); `None` until the first look, so a loaded game does not toast its past.
    power_seen: Option<(BTreeSet<(String, u32)>, usize)>,
    /// The game hour each chain building was first seen with a problem (T-337; UI state, not
    /// the sim's), the day of the last idle toast of each area and the hour of the last look.
    idle_since: BTreeMap<cp_building::BuildingId, u64>,
    idle_toasted: BTreeMap<u8, u64>,
    idle_hour: Option<u64>,
}

/// A chain building with a problem for this many game hours is idle (T-337).
const IDLE_HOURS: u64 = 12;

impl Advisor {
    /// The toast of a chain building that has stood idle for half a day (T-337 §2.6): at most
    /// one a day for each area. The timer is the advisor's, not the sim's.
    fn idle_toast(&mut self, view: &SimView) -> Option<Advice> {
        use cp_industry::IndustryProblem as P;
        let db = view.content.as_deref()?;
        let hour = view.tick.0 * 24 / TICKS_PER_DAY;
        if self.idle_hour == Some(hour) {
            return None;
        }
        self.idle_hour = Some(hour);
        let mut stuck: BTreeSet<cp_building::BuildingId> = BTreeSet::new();
        for (id, p) in &view.industry.problems {
            if matches!(p, P::NoInput | P::Full | P::Depleted) && view.buildings.get(*id).is_some()
            {
                stuck.insert(*id);
                self.idle_since.entry(*id).or_insert(hour);
            }
        }
        self.idle_since.retain(|id, _| stuck.contains(id));
        let day = hour / 24;
        let (id, _) = self
            .idle_since
            .iter()
            .find(|&(id, &since)| {
                hour >= since + IDLE_HOURS
                    && view
                        .buildings
                        .get(*id)
                        .and_then(|b| cp_district::area_at(&view.area_map, b.pos))
                        .is_some_and(|a| self.idle_toasted.get(&a.0).is_none_or(|&d| d < day))
            })
            .map(|(id, h)| (*id, *h))?;
        let b = view.buildings.get(id)?;
        let area = cp_district::area_at(&view.area_map, b.pos)?;
        self.idle_toasted.insert(area.0, day);
        let def = db.building(b.def);
        let building = tr(&def.name);
        let name = view.areas.get(area).map(|a| a.name.clone())?;
        let text = match view.industry.problem_of(id) {
            P::NoInput => {
                let lacking = def.industry.as_ref().and_then(|s| {
                    s.inputs.iter().map(|&(c, _)| c).min_by(|&x, &y| {
                        view.industry
                            .stock_of(id, x)
                            .total_cmp(&view.industry.stock_of(id, y))
                    })
                });
                trf!(
                    "{area}: {building} has stood idle for half a day, {cargo} is missing",
                    area = name,
                    building = building,
                    cargo = lacking.map_or_else(String::new, |c| tr(&format!("{c:?}")))
                )
            }
            P::Full => trf!(
                "{area}: {building} has stood idle for half a day, its storage is full",
                area = name,
                building = building
            ),
            _ => trf!(
                "{area}: {building} has stood idle for half a day, the ground is spent",
                area = name,
                building = building
            ),
        };
        Some(Advice {
            id: "industry_idle",
            severity: Severity::Warn,
            text,
            count: 1,
            icon: Icon::Industry,
            action: Some(Action::Jump(b.pos)),
        })
    }

    /// The toast of a new offer or a new scandal (T-335, `architecture/corruption-ui.md` §2.4).
    fn power_toast(&mut self, view: &SimView) -> Option<Advice> {
        let db = view.content.as_deref()?;
        let c = &view.corruption;
        let offers: BTreeSet<(String, u32)> = c
            .inbox
            .iter()
            .map(|o| (o.deal.clone(), o.expires_day))
            .collect();
        let (seen_offers, seen_scandals) = self
            .power_seen
            .replace((offers.clone(), c.scandals.len()))?;
        if c.scandals.len() > seen_scandals
            && let Some(s) = c.scandals.last()
        {
            return Some(Advice {
                id: "power_scandal",
                severity: Severity::Warn,
                text: crate::offers::scandal_text(db, s),
                count: 1,
                icon: Icon::Scandal,
                action: Some(Action::Window(crate::panels::UiWindow::Scandals)),
            });
        }
        let new = c
            .inbox
            .iter()
            .find(|o| !seen_offers.contains(&(o.deal.clone(), o.expires_day)))?;
        let def = db.deal(&new.deal)?;
        let chips: Vec<String> = crate::offers::chips(def)
            .into_iter()
            .take(3)
            .map(|c| c.text)
            .collect();
        Some(Advice {
            id: "power_offer",
            severity: Severity::Info,
            text: trf!(
                "New offer: {name} · {chips}",
                name = tr(&def.name),
                chips = chips.join(" · ")
            ),
            count: 1,
            icon: crate::offers::deal_icon(def),
            action: Some(Action::Window(crate::panels::UiWindow::Offers)),
        })
    }

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
        self.observe_hinted(view, now, problems_only, None);
    }

    /// [`observe`](Self::observe), and the one-time hints of the corruption axis (T-328,
    /// [`crate::axis::hints`]): when no problem is due, the first hint not in `hints_seen` comes
    /// (also with `problems_only`, the hints explain the rules) and joins it.
    pub fn observe_hinted(
        &mut self,
        view: &SimView,
        now: f64,
        problems_only: bool,
        hints_seen: Option<&mut std::collections::BTreeSet<String>>,
    ) {
        if let Some(toast) = self.power_toast(view).or_else(|| self.idle_toast(view)) {
            self.just_shown = Some(toast.severity);
            self.shown = Some(Shown {
                advice: toast,
                at: now,
            });
        }
        let hour = view.tick.0 * 24 / TICKS_PER_DAY;
        if self.last_hour == Some(hour) {
            return;
        }
        self.last_hour = Some(hour);
        let day = view.tick.0 / TICKS_PER_DAY;
        let on_screen = self.current(now).map(|a| a.severity);
        let mut all = collect(view, &mut self.unique_seen);
        all.extend(level_advice(view, &mut self.level_seen));
        all.extend(self.jobs_advice(view, day));
        all.extend(self.bike_lanes_hint(view));
        all.extend(self.hotel_advice(view, day));
        all.extend(complaint_advice(view));
        all.retain(|a| !(problems_only && a.severity == Severity::Info));
        // BUG-115: a few buildings are not a crisis.
        let active = view.city.buildings.active;
        all.retain(|a| {
            !BUILDING_COUNTS.contains(&a.id)
                || (a.count >= MIN_BUILDINGS && a.count * 100 >= MIN_SHARE_PERCENT * active)
        });
        // How long each advice has stood; the ones gone are forgotten.
        self.since.retain(|id, _| all.iter().any(|a| a.id == *id));
        for a in &all {
            self.since.entry(a.id).or_insert(hour);
        }
        let problem = |a: &Advice| a.severity != Severity::Info;
        all.retain(|a| !problem(a) || hour >= self.since[a.id] + PERSIST_HOURS);
        all.retain(|a| match self.last.get(a.id) {
            None => true,
            Some(&(shown_day, was)) => {
                day >= shown_day + COOLDOWN_DAYS || a.count >= (was * WORSE_FACTOR).max(was + 1)
            }
        });
        // At most one new problem notification a game day.
        if self
            .last_problem_hour
            .is_some_and(|h| hour < h + PROBLEM_GAP_HOURS)
        {
            all.retain(|a| !problem(a));
        }
        if self.last_market_day == Some(day) {
            all.retain(|a| !a.id.starts_with("market_"));
        }
        all.retain(|a| on_screen.is_none_or(|s| a.severity > s));
        // Most urgent first; the biggest problem of a severity, then the id, for a stable pick.
        all.sort_by(|a, b| {
            b.severity
                .cmp(&a.severity)
                .then(b.count.cmp(&a.count))
                .then(a.id.cmp(b.id))
        });
        let problems: Vec<&Advice> = all.iter().filter(|a| problem(a)).collect();
        let advice = if problems.len() >= MERGE_FROM {
            // Many at once: one entry that names the worst and counts the rest.
            let worst = problems[0];
            let n = problems.len() as u32;
            for a in &problems {
                self.last.insert(a.id, (day, a.count));
            }
            Some(Advice {
                id: "problems",
                severity: worst.severity,
                text: trf!(plural "{n} city problems — the worst: {worst}", n = n, worst = worst.text),
                count: n,
                icon: worst.icon,
                action: worst.action,
            })
        } else {
            all.into_iter().next()
        };
        let advice = match hints_seen {
            Some(seen) if on_screen.is_none() && advice.as_ref().is_none_or(|a| !problem(a)) => {
                let hint = crate::axis::hints(view)
                    .into_iter()
                    .find(|h| !seen.contains(h.id));
                if let Some(h) = &hint {
                    seen.insert(h.id.to_string());
                }
                hint.or(advice)
            }
            _ => advice,
        };
        if let Some(advice) = advice {
            self.last.insert(advice.id, (day, advice.count));
            if advice.id == "jobs" {
                self.jobs_last = Some(day);
            }
            if advice.id == BIKE_HINT {
                self.bike_hint_shown = true;
            }
            if advice.id == "tourism_season_info" {
                self.summer_told = true;
            }
            if problem(&advice) {
                self.last_problem_hour = Some(hour);
            }
            if advice.id.starts_with("market_") {
                self.last_market_day = Some(day);
            }
            self.just_shown = Some(advice.severity);
            self.shown = Some(Shown { advice, at: now });
        }
    }

    /// T-307: a mild reminder to look at the Jobs & education window once its problem has stood
    /// for `jobs_persist_days` game days, and again every `jobs_repeat_days` while it stands.
    fn jobs_advice(&mut self, view: &SimView, day: u64) -> Option<Advice> {
        let knobs = view
            .content
            .as_ref()
            .map_or_else(cp_content::Balance::default, |db| db.balance().clone());
        if !jobs_problem(&view.city, &knobs) {
            self.jobs_problem_since = None;
            return None;
        }
        let since = *self.jobs_problem_since.get_or_insert(day);
        let due = self
            .jobs_last
            .is_none_or(|d| day >= d + u64::from(knobs.jobs_repeat_days));
        (day >= since + u64::from(knobs.jobs_persist_days) && due).then(|| Advice {
            id: "jobs",
            severity: Severity::Info,
            text: tr("Jobs & education: take a look"),
            count: 1,
            icon: Icon::Jobs,
            action: Some(Action::Window(crate::panels::UiWindow::Jobs)),
        })
    }

    /// T-322c (§23.6): once a session, a town of [`BIKE_HINT_POP`] people with
    /// [`BIKE_HINT_OVERLOADED`] jammed roads and no bike lane anywhere hears that bike lanes help;
    /// the action opens the Bike tab of Roads.
    fn bike_lanes_hint(&self, view: &SimView) -> Option<Advice> {
        let db = view.content.as_ref()?;
        let any_bike_lane = || {
            view.net.segments().any(|(_, seg)| {
                db.road(seg.road)
                    .lanes
                    .iter()
                    .any(|l| l.kind == cp_content::LaneKind::Bike)
            })
        };
        (!self.bike_hint_shown
            && view.city.population >= BIKE_HINT_POP
            && view.city.traffic.overloaded >= BIKE_HINT_OVERLOADED
            && !any_bike_lane())
        .then(|| Advice {
            id: BIKE_HINT,
            severity: Severity::Info,
            text: tr("Roads are jammed: bike lanes take short trips off them"),
            count: 1,
            icon: Icon::OverlayTraffic,
            action: Some(Action::RoadTab(crate::roads::RoadTab::Bike)),
        })
    }

    /// T-321 (§22.5): `hotels_full` when the hotels were over 90 % taken three days running and
    /// tourists were turned away this week; `hotels_empty` when they stood under 20 % taken for a
    /// week outside winter; `tourism_season_info` once, on the first summer with tourists.
    fn hotel_advice(&mut self, view: &SimView, day: u64) -> Vec<Advice> {
        let mut out = Vec::new();
        let Some(db) = view.content.as_ref() else {
            return out;
        };
        let (guests, rooms) = view.hotels.occupancy(db, &view.buildings);
        if rooms > 0 && self.hotel_day != Some(day) {
            self.hotel_day = Some(day);
            let full = guests * 10 > rooms * 9;
            let empty = guests * 5 < rooms;
            self.hotels_full_days = if full { self.hotels_full_days + 1 } else { 0 };
            self.hotels_empty_days = if empty { self.hotels_empty_days + 1 } else { 0 };
        }
        let season = cp_core::time::season(view.tick);
        if rooms > 0 && self.hotels_full_days >= 3 && view.freight.turned_away_week > 0 {
            out.push(Advice {
                id: "hotels_full",
                severity: Severity::Info,
                text: tr("Tourists are turned away: build another hotel"),
                count: view.freight.turned_away_week,
                icon: Icon::Tourism,
                action: Some(Action::Panel(Category::Buildings)),
            });
        }
        if rooms > 0 && self.hotels_empty_days >= 7 && season != cp_core::time::Season::Winter {
            out.push(Advice {
                id: "hotels_empty",
                severity: Severity::Info,
                text: tr(
                    "Hotels stand empty: more shops, parks or a unique building draw tourists",
                ),
                count: 1,
                icon: Icon::Tourism,
                action: Some(Action::Panel(Category::Buildings)),
            });
        }
        if !self.summer_told
            && season == cp_core::time::Season::Summer
            && view.freight.tourists_week > 0
        {
            let mul = db.balance().tourism.season_of(season);
            out.push(Advice {
                id: "tourism_season_info",
                severity: Severity::Info,
                text: trf!("Summer: tourists x{mul}", mul = format!("{mul:.1}")),
                count: 1,
                icon: Icon::Tourism,
                action: None,
            });
        }
        out
    }

    fn close(&mut self) {
        self.shown = None;
    }
}

/// T-318c: "N homes complain: <complaint>" for the city's main complaint, by the BUG-115
/// rules counted in homes (at least [`MIN_BUILDINGS`] and [`MIN_SHARE_PERCENT`] of them); the
/// action is the inspector's. Id `complaint_<code>`.
pub fn complaint_advice(view: &SimView) -> Option<Advice> {
    let c = &view.city_contentment;
    let &(complaint, n) = c.complaints.first()?;
    (n >= MIN_BUILDINGS && n * 100 >= MIN_SHARE_PERCENT * c.homes).then(|| Advice {
        id: complaint_id(complaint),
        severity: Severity::Warn,
        text: trf!(
            plural "{n} homes complain: {complaint}",
            n = n,
            complaint = tr(complaint.label())
        ),
        count: n,
        icon: crate::icons::overlay_icon(Overlay::Happiness),
        action: crate::happiness::action(complaint),
    })
}

/// The advice id of a complaint (`complaint_<code>`, §19.5); a `&'static str` like the others.
fn complaint_id(c: cp_building::Complaint) -> &'static str {
    use cp_building::Complaint as C;
    match c {
        C::None => "complaint_none",
        C::NoHealth => "complaint_no_health",
        C::NoPolice => "complaint_no_police",
        C::NoFire => "complaint_no_fire",
        C::NoSchool => "complaint_no_school",
        C::Pollution => "complaint_pollution",
        C::Noise => "complaint_noise",
        C::NoRoad => "complaint_no_road",
        C::NoWater => "complaint_no_water",
        C::NoPower => "complaint_no_power",
        C::Garbage => "complaint_garbage",
        C::Crime => "complaint_crime",
        C::BadWater => "complaint_bad_water",
        C::Dead => "complaint_dead",
        C::Sick => "complaint_sick",
        C::Jobless => "complaint_jobless",
        C::LongCommute => "complaint_long_commute",
        C::NoPark => "complaint_no_park",
        C::HighTax => "complaint_high_tax",
        C::Distrust => "complaint_distrust",
    }
}

/// Whether work and education have a problem worth a look (T-307): unemployment, too few
/// workers for the free slots, or too few educated ones.
fn jobs_problem(city: &cp_sim::stats::CityStats, k: &cp_content::Balance) -> bool {
    if city.adults == 0 {
        return false;
    }
    let adults = city.adults as f32;
    let looking: u32 = city.education.iter().map(|e| e.looking).sum();
    let free: u32 = city.jobs.iter().map(|j| j.free).sum();
    let free_educated: u32 = city.jobs[2..].iter().map(|j| j.free).sum();
    looking as f32 / adults > k.jobs_unemployment_warn
        || free as f32 > k.jobs_shortage_share * adults
        || free_educated as f32 > k.jobs_educated_share * adults
}

/// Runs the action of a "Show" click.
pub fn run(action: Action, state: &mut UiState, overlay: &mut cp_render::OverlayState) {
    match action {
        Action::Overlay(o) => overlay.open(o),
        Action::Jump(at) => state.camera_goal = Some(at),
        Action::Panel(c) => state.category = Some(c),
        Action::Budget => state.budget_open = true,
        Action::BudgetRow(kind) => {
            state.budget_open = true;
            state.budget.tab = crate::budget::BudgetTab::Budgets;
            state.budget.highlight = Some(kind);
        }
        Action::Policies => state.policies_open = true,
        Action::Window(w) => state.open_window(w),
        Action::Trees => {
            overlay.open(Overlay::Noise);
            state.tool_goal = Some(cp_tools::ActiveTool::Trees);
        }
        Action::PollutedSource(at) => {
            overlay.open(Overlay::Pollution);
            state.camera_goal = Some(at);
        }
        Action::RoadTab(tab) => {
            state.category = Some(Category::Roads);
            let key = crate::roads::tab_label(tab);
            state.tray_tab.retain(|(c, _)| *c != Category::Roads);
            state.tray_tab.push((Category::Roads, key));
        }
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
    let prefs = &mut state.prefs;
    advisor.observe_hinted(
        view,
        now,
        prefs.advisor_problems_only,
        Some(&mut prefs.hints_seen),
    );
    let Some(advice) = advisor.current(now).cloned() else {
        return;
    };
    let lift = Sizes::DEMAND_H
        + if state.category.is_some() {
            Sizes::TRAY_H
        } else {
            0.0
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
                            show = ui.button(tr("Show")).clicked();
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

    /// §20.3: a short sector with its budget under 100 % asks to raise it and opens the tab on
    /// the row; at 100 % it does not; plenty over 100 % says it can be cut.
    #[test]
    fn a_short_sector_under_a_low_budget_asks_to_raise_it() {
        use cp_sim::city_services::{Sector, SectorBalance};
        let with = |pct: u8, supply: f32| {
            let mut v = view_with(|sim| {
                sim.world_mut()
                    .resource_mut::<cp_content::Budgets>()
                    .set(ServiceKind::Water, pct);
            });
            std::sync::Arc::make_mut(&mut v.city).services = vec![SectorBalance {
                sector: Sector::Water,
                supply,
                demand: 100.0,
                unit: "units",
                locked: false,
            }];
            let mut out = Vec::new();
            budget_advice(&v, &mut out);
            out
        };
        let low = with(60, 50.0);
        assert_eq!(low.len(), 1, "{low:?}");
        assert_eq!(low[0].id, "budget_low_water");
        assert_eq!(low[0].severity, Severity::Warn);
        assert_eq!(low[0].text, "Water is short: raise its budget (now 60%)");
        assert_eq!(low[0].action, Some(Action::BudgetRow(ServiceKind::Water)));
        assert!(with(100, 50.0).is_empty());
        let waste = with(140, 200.0);
        assert_eq!(waste[0].id, "budget_waste_water");
        assert_eq!(waste[0].text, "You can cut the Water budget");

        let mut state = UiState::default();
        let mut overlay = cp_render::OverlayState::default();
        run(
            Action::BudgetRow(ServiceKind::Water),
            &mut state,
            &mut overlay,
        );
        assert!(state.budget_open);
        assert_eq!(state.budget.tab, crate::budget::BudgetTab::Budgets);
        assert_eq!(state.budget.highlight, Some(ServiceKind::Water));
    }

    /// T-318c: the main complaint becomes advice only from MIN_BUILDINGS homes and
    /// MIN_SHARE_PERCENT of them; its id and action follow the complaint.
    #[test]
    fn the_main_complaint_is_advice_once_enough_homes_share_it() {
        use cp_building::Complaint;
        let mut v = view_with(|_| {});
        let mut set = |homes: u32, n: u32| {
            v.city_contentment.homes = homes;
            v.city_contentment.complaints = vec![(Complaint::Jobless, n), (Complaint::Noise, 1)];
            complaint_advice(&v)
        };
        assert_eq!(set(100, MIN_BUILDINGS - 1), None, "too few homes");
        assert_eq!(set(1000, 20), None, "under MIN_SHARE_PERCENT of the homes");
        let a = set(100, 10).expect("advice");
        assert_eq!(a.id, "complaint_jobless");
        assert_eq!(a.count, 10);
        assert_eq!(a.severity, Severity::Warn);
        assert_eq!(
            a.action,
            Some(Action::Window(crate::panels::UiWindow::Jobs))
        );
        assert!(a.text.contains("No work"), "{}", a.text);
        v.city_contentment.complaints.clear();
        assert_eq!(complaint_advice(&v), None);
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

    /// The first tick of game hour `h`.
    fn hour_tick(h: u64) -> u64 {
        (h * TICKS_PER_DAY).div_ceil(24)
    }

    /// An advisor that has seen `v` for the persistence time: the next look may notify.
    fn warmed(v: &SimView) -> Advisor {
        let mut adv = Advisor::default();
        adv.observe(&at_tick(v.clone(), 0), 0.0, true);
        assert!(adv.current(0.1).is_none(), "a new problem waits");
        adv
    }

    /// T-328: a corruption hint comes once when its trigger holds, is remembered in the
    /// player's `hints_seen`, and comes again only after a reset.
    #[test]
    fn a_hint_comes_once_and_again_after_a_reset() {
        let v = view_with(|sim| {
            let mut c = sim
                .world_mut()
                .resource_mut::<cp_economy::CorruptionState>();
            c.scandals.push(cp_economy::Scandal {
                deal: "land".into(),
                day: 1,
                weight: 1.0,
                trust: 1.0,
                fine: 1,
                undone: false,
            });
        });
        let mut seen = std::collections::BTreeSet::new();
        let mut adv = Advisor::default();
        adv.observe_hinted(
            &at_tick(v.clone(), hour_tick(1)),
            0.0,
            true,
            Some(&mut seen),
        );
        assert_eq!(adv.current(0.1).map(|a| a.id), Some("hint_scandal"));
        assert_eq!(
            adv.current(0.1).unwrap().action,
            Some(Action::Window(crate::panels::UiWindow::Scandals))
        );
        assert!(seen.contains("hint_scandal"));
        adv.close();
        adv.observe_hinted(
            &at_tick(v.clone(), hour_tick(2)),
            1.0,
            true,
            Some(&mut seen),
        );
        assert_ne!(
            adv.current(1.1).map(|a| a.id),
            Some("hint_scandal"),
            "a seen hint does not come again (the next unseen one may)"
        );
        adv.close();
        seen.clear();
        adv.observe_hinted(
            &at_tick(v.clone(), hour_tick(3)),
            2.0,
            true,
            Some(&mut seen),
        );
        assert_eq!(
            adv.current(2.1).map(|a| a.id),
            Some("hint_scandal"),
            "after the reset"
        );
        // Without the player's hints (the tests, a hidden run) nothing comes.
        let mut plain = Advisor::default();
        plain.observe(&at_tick(v, hour_tick(1)), 0.0, false);
        assert!(plain.current(0.1).is_none());
    }

    /// T-322c: a town of 1 500 with ten jammed roads and no bike lane hears about bike lanes,
    /// once a session; the action opens the Bike tab of Roads; a bike lane anywhere silences it.
    #[test]
    fn the_bike_lanes_hint_comes_once_and_opens_the_bike_tab() {
        let jammed = |v: &mut SimView| {
            let city = std::sync::Arc::make_mut(&mut v.city);
            city.population = 1_500;
            city.traffic.overloaded = 10;
        };
        let mut v = view_with(|_| {});
        jammed(&mut v);
        let mut adv = Advisor::default();
        let a = adv.bike_lanes_hint(&v).expect("the hint");
        assert_eq!(a.action, Some(Action::RoadTab(crate::roads::RoadTab::Bike)));
        let mut state = UiState::default();
        let mut overlay = cp_render::OverlayState::default();
        run(a.action.unwrap(), &mut state, &mut overlay);
        assert_eq!(state.category, Some(Category::Roads));
        assert_eq!(
            state.tray_tab,
            [(
                Category::Roads,
                crate::roads::tab_label(crate::roads::RoadTab::Bike)
            )]
        );
        adv.bike_hint_shown = true;
        assert!(adv.bike_lanes_hint(&v).is_none(), "once a session");
        let mut lanes = view_with(|sim| {
            let db = std::sync::Arc::clone(&sim.world().resource::<cp_content::SimContent>().0);
            let road = db.road_id("basic_2l_bike").unwrap();
            let mut g = sim.world_mut().resource_mut::<cp_net::NetGraph>();
            let (a, b) = (
                bevy::math::Vec3::new(100.0, 0.0, 100.0),
                bevy::math::Vec3::new(200.0, 0.0, 100.0),
            );
            g.add_segment(
                db.road(road),
                road,
                cp_core::curve::CubicBezier3::line(a, b),
                cp_net::Elevation::Ground,
            )
            .unwrap();
            sim.tick_once();
        });
        jammed(&mut lanes);
        assert!(Advisor::default().bike_lanes_hint(&lanes).is_none());
    }

    /// T-321c (§22.7 item 7): `hotels_full` only after three full days with tourists turned away
    /// this week; `hotels_empty` after a week under 20 % taken, never in winter.
    #[test]
    fn hotel_advice_needs_three_full_days_and_turned_away_and_is_quiet_in_winter() {
        use cp_core::time::{SEASON_DAYS, Season, TICKS_PER_DAY};
        let mut hotel = None;
        let mut v = view_with(|sim| {
            let db = std::sync::Arc::clone(&sim.world().resource::<cp_content::SimContent>().0);
            let id = sim
                .world_mut()
                .resource_mut::<cp_building::BuildingGrid>()
                .insert(cp_building::Building {
                    def: db.building_id("hotel_small").unwrap(),
                    footprint: cp_building::Footprint::Lot(cp_building::Lot {
                        block: cp_core::Id::new(0, 0),
                        col: 0,
                        w: 2,
                        d: 2,
                    }),
                    level: 1,
                    state: cp_building::BuildingState::Active,
                    ready_at: cp_core::Tick(0),
                    pos: bevy::math::Vec2::new(10.0, 10.0),
                    rot: 0.0,
                    access: None,
                    flags: cp_building::BuildingFlags(0),
                    level_progress: 0.0,
                });
            hotel = Some(id);
        });
        let id = hotel.unwrap();
        let rooms = u32::from(
            v.content
                .as_ref()
                .unwrap()
                .building(v.buildings.get(id).unwrap().def)
                .hotel
                .unwrap()
                .rooms,
        );
        let at_season = |s: Season| {
            let i = Season::ALL.iter().position(|&x| x == s).unwrap() as u64;
            cp_core::Tick(i * SEASON_DAYS * TICKS_PER_DAY + TICKS_PER_DAY / 2)
        };
        let has = |out: &[Advice], id: &str| out.iter().any(|a| a.id == id);
        let set_guests = |v: &mut SimView, g: u32| {
            v.hotels.slots.insert(
                id,
                cp_transit::HotelSlot {
                    guests: g as u16,
                    ..Default::default()
                },
            );
        };

        // Full (over 90 %) but nobody turned away: no advice, however many days.
        set_guests(&mut v, rooms);
        v.tick = at_season(Season::Spring);
        let mut adv = Advisor::default();
        for day in 0..5 {
            assert!(!has(&adv.hotel_advice(&v, day), "hotels_full"), "day {day}");
        }
        // Turned away too: only on the third full day.
        v.freight.turned_away_week = 4;
        let mut adv = Advisor::default();
        assert!(!has(&adv.hotel_advice(&v, 0), "hotels_full"));
        assert!(!has(&adv.hotel_advice(&v, 1), "hotels_full"));
        let out = adv.hotel_advice(&v, 2);
        let full = out
            .iter()
            .find(|a| a.id == "hotels_full")
            .expect("third full day");
        assert_eq!(full.count, 4);
        assert_eq!(full.action, Some(Action::Panel(Category::Buildings)));

        // Empty (under 20 %): a week outside winter gives the advice, winter never does.
        v.freight.turned_away_week = 0;
        set_guests(&mut v, 0);
        let mut adv = Advisor::default();
        for day in 0..6 {
            assert!(
                !has(&adv.hotel_advice(&v, day), "hotels_empty"),
                "day {day}"
            );
        }
        assert!(
            has(&adv.hotel_advice(&v, 6), "hotels_empty"),
            "after a week"
        );
        v.tick = at_season(Season::Winter);
        let mut adv = Advisor::default();
        for day in 0..10 {
            assert!(
                !has(&adv.hotel_advice(&v, day), "hotels_empty"),
                "winter day {day}"
            );
        }
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

    /// A view whose adults are `looking` jobless out of 100.
    fn jobless(looking: u32) -> SimView {
        let mut v = view_with(|_| {});
        let mut city = (*v.city).clone();
        city.adults = 100;
        city.education[1].adults = 100;
        city.education[1].looking = looking;
        city.connected_to_highway = true;
        v.city = std::sync::Arc::new(city);
        v
    }

    /// T-307: unemployment above the knob for 5 game days gives one soft advice that opens the
    /// Jobs window; it repeats after 5 more days and goes with the problem.
    #[test]
    fn the_jobs_reminder_comes_after_five_days_repeats_and_goes_with_the_problem() {
        let day = |d: u64| hour_tick(24 * d);
        let mut adv = Advisor::default();
        let sick = jobless(30);
        assert!(
            collect(&sick, &mut None)
                .iter()
                .all(|a| a.id != "unemployed")
        );
        // Days 0..=4: quiet.
        for d in 0..5 {
            adv.observe(&at_tick(sick.clone(), day(d)), d as f64 * 100.0, false);
            assert!(adv.current(d as f64 * 100.0).is_none(), "day {d}");
        }
        // Day 5: the advice.
        adv.observe(&at_tick(sick.clone(), day(5)), 500.0, false);
        let a = adv.current(500.1).expect("advice on day 5").clone();
        assert_eq!(a.id, "jobs");
        assert_eq!(a.severity, Severity::Info);
        assert_eq!(
            a.action,
            Some(Action::Window(crate::panels::UiWindow::Jobs))
        );
        let mut state = UiState::default();
        run(a.action.unwrap(), &mut state, &mut Default::default());
        assert!(state.jobs_open);
        // Days 6..=9: quiet; day 10: again.
        for d in 6..10 {
            adv.observe(&at_tick(sick.clone(), day(d)), d as f64 * 100.0, false);
            assert!(adv.current(d as f64 * 100.0 + 0.1).is_none(), "day {d}");
        }
        adv.observe(&at_tick(sick.clone(), day(10)), 1000.0, false);
        assert_eq!(adv.current(1000.1).map(|a| a.id), Some("jobs"));
        // The problem goes: no advice, and a new one starts its five days again.
        let well = jobless(2);
        adv.observe(&at_tick(well, day(11)), 1100.0, false);
        assert!(adv.current(1100.1).is_none());
        assert_eq!(adv.jobs_problem_since, None);
        for d in 12..17 {
            adv.observe(&at_tick(sick.clone(), day(d)), d as f64 * 100.0, false);
            assert!(adv.current(d as f64 * 100.0 + 0.1).is_none(), "day {d}");
        }
        adv.observe(&at_tick(sick, day(17)), 1700.0, false);
        assert_eq!(adv.current(1700.1).map(|a| a.id), Some("jobs"));
    }

    /// T-300: homes with BAD_WATER give "Bad water in N homes"; its action opens the Pollution
    /// overlay (and moves the camera when a polluted source is known).
    #[test]
    fn bad_water_homes_give_an_advice_that_opens_the_pollution_overlay() {
        use cp_service::{BuildingServices, ServiceProblems, ServiceState};
        let mut v = no_water(0);
        assert!(collect(&v, &mut None).iter().all(|a| a.id != "bad_water"));
        let bad = BuildingServices {
            problems: ServiceProblems(ServiceProblems::BAD_WATER),
            ..BuildingServices::default()
        };
        v.service_state = std::sync::Arc::new(ServiceState::from_parts(
            vec![bad, bad, bad],
            Default::default(),
        ));
        let all = collect(&v, &mut None);
        let a = all.iter().find(|a| a.id == "bad_water").expect("advice");
        assert_eq!(a.count, 3);
        assert_eq!(a.icon, Icon::ProblemBadWater);
        assert_eq!(a.action, Some(Action::Overlay(Overlay::Pollution)));
        let mut state = UiState::default();
        let mut overlay = cp_render::OverlayState::default();
        run(
            Action::PollutedSource(Vec2::new(5.0, 6.0)),
            &mut state,
            &mut overlay,
        );
        assert_eq!(overlay.active, Overlay::Pollution);
        assert_eq!(state.camera_goal, Some(Vec2::new(5.0, 6.0)));
    }

    /// BUG-115: a problem notifies after half a day, the same one is quiet for three days unless
    /// it doubled.
    #[test]
    fn a_repeat_waits_three_days_unless_it_doubled() {
        let mut adv = warmed(&no_water(5));
        adv.observe(&at_tick(no_water(5), hour_tick(12)), 20.0, true);
        assert_eq!(adv.just_shown.take(), Some(Severity::Warn));
        // Snackbar gone; the same problem a day and two days later: silent.
        adv.observe(&at_tick(no_water(5), hour_tick(36)), 40.0, true);
        adv.observe(&at_tick(no_water(5), hour_tick(60)), 60.0, true);
        assert_eq!(adv.just_shown, None);
        // Worse, but not twice as bad: silent.
        adv.observe(&at_tick(no_water(9), hour_tick(61)), 80.0, true);
        assert_eq!(adv.just_shown, None);
        // Twice as bad: shown at once.
        adv.observe(&at_tick(no_water(10), hour_tick(62)), 100.0, true);
        assert_eq!(adv.just_shown.take(), Some(Severity::Critical));
        // Three days after that: again, though not worse.
        adv.observe(&at_tick(no_water(10), hour_tick(62 + 72)), 120.0, true);
        assert_eq!(adv.just_shown.take(), Some(Severity::Critical));
    }

    /// BUG-115: a problem that comes and goes inside half a day never notifies; one that stands
    /// does, once.
    #[test]
    fn a_problem_notifies_only_after_it_has_stood_half_a_day() {
        let mut adv = Advisor::default();
        adv.observe(&at_tick(no_water(5), hour_tick(0)), 0.0, true);
        adv.observe(&at_tick(no_water(5), hour_tick(11)), 10.0, true);
        assert_eq!(adv.just_shown, None);
        // It clears for an hour: the clock starts again.
        adv.observe(&at_tick(no_water(0), hour_tick(12)), 20.0, true);
        adv.observe(&at_tick(no_water(5), hour_tick(13)), 30.0, true);
        adv.observe(&at_tick(no_water(5), hour_tick(24)), 40.0, true);
        assert_eq!(adv.just_shown, None);
        adv.observe(&at_tick(no_water(5), hour_tick(25)), 50.0, true);
        assert_eq!(adv.just_shown.take(), Some(Severity::Warn));
    }

    /// BUG-115: under 5 buildings, or under 3 % of the active ones, is no notification.
    #[test]
    fn a_small_problem_does_not_notify() {
        let mut few = no_water(4);
        let mut adv = warmed(&few);
        adv.observe(&at_tick(few.clone(), hour_tick(30)), 20.0, true);
        assert_eq!(adv.just_shown, None);
        // 5 buildings of 1000 active: under 3 %.
        let mut city = (*few.city).clone();
        city.buildings.no_water = 5;
        city.buildings.active = 1000;
        few.city = std::sync::Arc::new(city);
        let mut adv = warmed(&few);
        adv.observe(&at_tick(few.clone(), hour_tick(30)), 20.0, true);
        assert_eq!(adv.just_shown, None);
        // 30 of 1000: 3 %.
        let mut city = (*few.city).clone();
        city.buildings.no_water = 30;
        few.city = std::sync::Arc::new(city);
        let mut adv = warmed(&few);
        adv.observe(&at_tick(few.clone(), hour_tick(30)), 20.0, true);
        assert_eq!(adv.just_shown.take(), Some(Severity::Critical));
    }

    /// BUG-115: one new problem notification a game day; a second problem waits for the day.
    #[test]
    fn at_most_one_new_problem_notification_a_day() {
        let mut v = no_water(5);
        let mut city = (*v.city).clone();
        city.buildings.no_power = 6;
        city.buildings.active = 6;
        v.city = std::sync::Arc::new(city);
        let mut adv = warmed(&v);
        adv.observe(&at_tick(v.clone(), hour_tick(12)), 20.0, true);
        let first = adv.current(21.0).unwrap().id;
        assert!(adv.just_shown.take().is_some());
        // The next hours, snackbar gone: silent until a day after the first.
        for h in 13..36 {
            adv.observe(
                &at_tick(v.clone(), hour_tick(h)),
                20.0 + 10.0 * h as f64,
                true,
            );
            assert_eq!(adv.just_shown, None, "hour {h}");
        }
        adv.observe(&at_tick(v, hour_tick(36)), 1000.0, true);
        assert_ne!(adv.current(1001.0).unwrap().id, first);
    }

    /// BUG-115: three problems at once are one "N city problems" entry (the worst one's action).
    #[test]
    fn many_problems_at_once_are_one_entry() {
        let mut v = no_water(6);
        let mut city = (*v.city).clone();
        city.buildings.no_power = 7;
        city.buildings.no_sewage = 8;
        city.buildings.active = 8;
        v.city = std::sync::Arc::new(city);
        let mut adv = warmed(&v);
        adv.observe(&at_tick(v.clone(), hour_tick(12)), 20.0, true);
        let a = adv.current(21.0).unwrap();
        assert_eq!(a.id, "problems");
        assert_eq!(a.count, 3);
        assert!(a.text.starts_with("3 city problems"), "{}", a.text);
        // All three are on cooldown now: no second entry the same day.
        adv.observe(&at_tick(v, hour_tick(40)), 1000.0, true);
        assert!(adv.current(1001.0).is_none());
    }

    #[test]
    fn a_critical_advice_beats_a_milder_one_and_replaces_it() {
        let mut v = no_water(5);
        let mut city = (*v.city).clone();
        city.buildings.no_power = 12;
        v.city = std::sync::Arc::new(city);
        let mut adv = warmed(&v);
        adv.observe(&at_tick(v, hour_tick(12)), 1.0, true);
        assert_eq!(adv.current(1.1).unwrap().id, "no_power");
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
        let mut adv = warmed(&no_water(5));
        adv.observe(&at_tick(no_water(5), hour_tick(12)), 0.0, true);
        assert!(adv.current(SHOWN_SECS - 0.1).is_some());
        assert!(adv.current(SHOWN_SECS + 0.1).is_none());
        let mut state = UiState::default();
        let mut overlay = cp_render::OverlayState::default();
        run(Action::Overlay(Overlay::Water), &mut state, &mut overlay);
        assert_eq!(overlay.active, Overlay::Water);
        run(Action::Jump(Vec2::new(1.0, 2.0)), &mut state, &mut overlay);
        assert_eq!(state.camera_goal, Some(Vec2::new(1.0, 2.0)));
    }

    /// T-323: the Noise complaint's link shows the noise and puts the Trees tool in hand.
    #[test]
    fn the_noise_complaint_opens_the_trees_tool() {
        let action = crate::happiness::action(cp_building::Complaint::Noise).unwrap();
        assert_eq!(action, Action::Trees);
        let mut state = UiState::default();
        let mut overlay = cp_render::OverlayState::default();
        run(action, &mut state, &mut overlay);
        assert_eq!(overlay.active, Overlay::Noise);
        assert_eq!(state.tool_goal, Some(cp_tools::ActiveTool::Trees));
    }

    /// §21.6 item 6 (T-320b): a shortage is advised only for a cargo the city imported last
    /// week, a glut only for one it exported, a dear cargo only when a warehouse in `Fill`
    /// holds it; and at most one market advice a game day.
    #[test]
    fn market_advice_is_for_the_cargo_the_city_trades_and_comes_once_a_day() {
        use crate::industry::tests::{put, world_with_areas};
        use cp_content::{CargoKind, Market, MarketEvent, MarketEventKind, MarketFrozen};
        use cp_industry::{
            IndustryLedger, IndustryState, WarehouseMode, WarehouseSetting, WarehouseSettings,
        };
        let (mut sim, db, at) = world_with_areas(1);
        sim.world_mut().insert_resource(MarketFrozen);
        let ids = |sim: &cp_sim::SimRunner| -> Vec<&'static str> {
            collect(&sim.view(), &mut None)
                .iter()
                .map(|a| a.id)
                .filter(|id| id.starts_with("market_"))
                .collect()
        };
        {
            let mut m = sim.world_mut().resource_mut::<Market>();
            for (c, kind) in [
                (CargoKind::Metals, MarketEventKind::Shortage),
                (CargoKind::Logs, MarketEventKind::Shortage),
                (CargoKind::Planks, MarketEventKind::Glut),
            ] {
                m.get_mut(c).event = Some(MarketEvent { kind, until_day: 9 });
            }
            m.get_mut(CargoKind::Paper).index = 160;
        }
        assert_eq!(
            ids(&sim),
            Vec::<&str>::new(),
            "nothing traded, nothing advised"
        );
        {
            let mut l = sim.world_mut().resource_mut::<IndustryLedger>();
            l.last_by_cargo[CargoKind::Logs as usize].import = 20;
            l.last_by_cargo[CargoKind::Planks as usize].export = 20;
            // Exported, not imported: a shortage of it is no worry.
            l.last_by_cargo[CargoKind::Metals as usize].export = 20;
        }
        assert_eq!(ids(&sim), ["market_shortage_logs", "market_glut_planks"]);
        let store = put(&mut sim, &db, "forest_warehouse", at[0]);
        sim.world_mut()
            .resource_mut::<WarehouseSettings>()
            .list
            .insert(
                store,
                WarehouseSetting {
                    cargo: None,
                    mode: WarehouseMode::Fill,
                },
            );
        assert!(!ids(&sim).contains(&"market_peak_paper"), "it holds none");
        sim.world_mut()
            .resource_mut::<IndustryState>()
            .stock
            .insert(store, vec![(CargoKind::Paper, 100.0)]);
        let all = collect(&sim.view(), &mut None);
        let peak = all.iter().find(|a| a.id == "market_peak_paper").unwrap();
        assert_eq!(peak.action, Some(Action::Jump(at[0])));

        // Once a day: the glut and the peak are info and wait for nothing, yet only one shows.
        let v = sim.view();
        let mut adv = Advisor::default();
        let mut shown = vec![];
        for h in 1..24 {
            adv.observe(&at_tick(v.clone(), hour_tick(h)), h as f64 * 100.0, false);
            if let Some(a) = adv.current(h as f64 * 100.0) {
                shown.push(a.id);
            }
        }
        let market = shown.iter().filter(|id| id.starts_with("market_")).count();
        assert_eq!(market, 1, "{shown:?}");
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

    /// T-249: mail advice waits for the post office to open, then names the piling mail and the
    /// full post offices.
    #[test]
    fn mail_advice_comes_only_after_the_post_office_opens() {
        use crate::industry::tests::world_with_areas;
        let (mut sim, db, at) = world_with_areas(1);
        let ids = |sim: &cp_sim::SimRunner| -> Vec<&'static str> {
            collect(&sim.view(), &mut None)
                .iter()
                .map(|a| a.id)
                .collect()
        };
        let house = crate::industry::tests::put(&mut sim, &db, "forest_extractor_s", at[0]);
        sim.world_mut()
            .resource_mut::<cp_service::ServiceState>()
            .get_mut(house)
            .problems
            .set(ServiceProblems::MAIL, true);
        {
            let mut mail = sim.world_mut().resource_mut::<cp_service::MailState>();
            mail.unsorted = 90_000;
            mail.sort_capacity = 1_000;
        }
        sim.world_mut().insert_resource(cp_sim::Progress::at(1));
        let got = ids(&sim);
        assert!(
            !got.contains(&"mail") && !got.contains(&"mail_sorting"),
            "{got:?}"
        );
        // T-283: the post office opens at Small Town (3), the sorting facility at Busy Town (5).
        sim.world_mut().insert_resource(cp_sim::Progress::at(5));
        let advice = collect(&sim.view(), &mut None);
        let mail = advice.iter().find(|a| a.id == "mail").expect("mail");
        assert!(mail.text.contains("build a post office"), "{}", mail.text);
        assert_eq!(mail.icon, Icon::ProblemMail);
        assert!(matches!(mail.action, Some(Action::Overlay(Overlay::Mail))));
        let full = advice
            .iter()
            .find(|a| a.id == "mail_sorting")
            .expect("sorting");
        assert!(
            full.text.contains("build a sorting facility"),
            "{}",
            full.text
        );
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

    /// T-337: a chain building with a problem for 12 game hours gives one toast, and an area
    /// not more than one a day.
    #[test]
    fn an_idle_chain_building_toasts_after_half_a_day_once_a_day_per_area() {
        use crate::industry::tests::{put, world_with_areas};
        use cp_industry::{IndustryProblem, IndustryState};
        let (mut sim, db, at) = world_with_areas(1);
        put(&mut sim, &db, "forest_main", at[0]);
        let saw = put(&mut sim, &db, "forest_extractor_s", at[0]);
        let mill = put(&mut sim, &db, "forest_processor_a", at[0]);
        let mut st = IndustryState::default();
        st.problems.insert(saw, IndustryProblem::Full);
        st.problems.insert(mill, IndustryProblem::NoInput);
        sim.world_mut().insert_resource(st);
        let v = sim.view();
        let mut adv = Advisor::default();
        adv.observe(&at_tick(v.clone(), hour_tick(1)), 0.0, false);
        adv.observe(&at_tick(v.clone(), hour_tick(12)), 1.0, false);
        assert_ne!(adv.current(1.1).map(|a| a.id), Some("industry_idle"));
        adv.observe(&at_tick(v.clone(), hour_tick(13)), 2.0, false);
        let toast = adv.current(2.1).expect("an idle toast");
        assert_eq!(toast.id, "industry_idle");
        assert!(toast.text.starts_with("Forestry: "), "{}", toast.text);
        assert!(matches!(toast.action, Some(Action::Jump(_))));
        // The other idle building of the area waits for tomorrow.
        adv.shown = None;
        adv.observe(&at_tick(v.clone(), hour_tick(14)), 3.0, false);
        assert_ne!(adv.current(3.1).map(|a| a.id), Some("industry_idle"));
        adv.observe(&at_tick(v, hour_tick(24 + 14)), 4.0, false);
        assert_eq!(adv.current(4.1).map(|a| a.id), Some("industry_idle"));
    }

    /// T-335: a new offer and a new scandal come as toasts with a link to their tab; what was
    /// there on the first look does not.
    #[test]
    fn a_new_offer_and_a_new_scandal_come_as_toasts() {
        let mut v = view_with(|_| {});
        let mut adv = Advisor::default();
        adv.observe(&v, 0.0, true);
        assert!(adv.current(0.1).is_none());
        v.corruption.inbox.push(cp_economy::Offer {
            deal: "land".into(),
            target: None,
            expires_day: 9,
        });
        adv.observe(&v, 1.0, true);
        let toast = adv.current(1.1).expect("an offer toast");
        assert_eq!(toast.id, "power_offer");
        assert!(
            toast.text.starts_with("New offer: Land for friends"),
            "{}",
            toast.text
        );
        assert_eq!(
            toast.action,
            Some(Action::Window(crate::panels::UiWindow::Offers))
        );
        adv.observe(&v, 2.0, true);
        assert_eq!(adv.current(2.1).map(|a| a.id), Some("power_offer"));
        v.corruption.scandals.push(cp_economy::Scandal {
            deal: "land".into(),
            day: 3,
            weight: 2.0,
            trust: 10.0,
            fine: 5000,
            undone: false,
        });
        adv.observe(&v, 3.0, true);
        let toast = adv.current(3.1).expect("a scandal toast");
        assert_eq!(
            (toast.id, toast.severity),
            ("power_scandal", Severity::Warn)
        );
    }
}
