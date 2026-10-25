//! What opening a build menu does (T-289): the menu picks up the last build tool the player used
//! in it this session — the first time its first tool (the 2-lane road, the wind turbine, the
//! water pump) — and a Power or Water menu turns that network's overlay on. The Power and Water
//! menus are also the layers of the bulldozer ([`cp_tools::BulldozeLayer`]). Nothing here is
//! saved between runs.

use cp_content::{BuildingDefId, ContentDb, UtilityKind};
use cp_render::{Overlay, OverlayState};
use cp_sim::SimView;
use cp_tools::{ActiveTool, BulldozeLayer, ToolState};

use crate::palette::Category;
use crate::panels::UiState;

/// A build tool a menu remembers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Remembered {
    /// A tool with nothing more to pick (the road tool, zoning).
    Tool(ActiveTool),
    /// The placement tool with this building.
    Place(BuildingDefId),
    /// The pipe or power-line tool.
    Utility(UtilityKind),
}

impl Remembered {
    /// The build tool `tools` has in hand, if it is one a menu remembers.
    fn of(tools: &ToolState) -> Option<Self> {
        match tools.active {
            ActiveTool::Road
            | ActiveTool::Upgrade
            | ActiveTool::Flip
            | ActiveTool::Trees
            | ActiveTool::Junctions
            | ActiveTool::Interchange
            | ActiveTool::Zone => Some(Self::Tool(tools.active)),
            ActiveTool::Place => tools.building.def.map(Self::Place),
            ActiveTool::Utility => Some(Self::Utility(tools.utility.kind)),
            _ => None,
        }
    }

    /// Takes the tool up.
    fn take_up(self, tools: &mut ToolState) {
        match self {
            Self::Tool(t) => tools.select(t),
            Self::Place(def) => crate::buildings::select(tools, def),
            Self::Utility(kind) => {
                tools.select(ActiveTool::Utility);
                tools.utility.set_kind(kind);
            }
        }
    }
}

/// The menus that remember a tool: the ones that build with a single current tool.
fn remembers(c: Category) -> bool {
    matches!(
        c,
        Category::Roads | Category::Zones | Category::Buildings | Category::Power | Category::Water
    )
}

/// The layer of the bulldozer while `c` is the open menu.
pub fn layer_of(c: Option<Category>) -> BulldozeLayer {
    match c {
        Some(Category::Power) => BulldozeLayer::Power,
        Some(Category::Water) => BulldozeLayer::Water,
        _ => BulldozeLayer::All,
    }
}

/// The overlay a menu shows when it opens.
fn overlay_of(c: Category) -> Option<Overlay> {
    match c {
        Category::Power => Some(Overlay::Power),
        Category::Water => Some(Overlay::Water),
        _ => None,
    }
}

/// The first tool of a menu that has been used in none yet.
fn first_tool(c: Category, view: Option<&SimView>) -> Option<Remembered> {
    let content = view?.content.as_deref()?;
    let by_id = |id: &str| content.building_id(id).map(Remembered::Place);
    match c {
        Category::Roads => Some(Remembered::Tool(ActiveTool::Road)),
        Category::Zones => Some(Remembered::Tool(ActiveTool::Zone)),
        Category::Power => by_id("wind_turbine"),
        Category::Water => by_id("water_pump_station"),
        Category::Buildings => first_open_building(view?, content),
        _ => None,
    }
}

/// The first service building of the Buildings menu that is not locked yet.
fn first_open_building(view: &SimView, content: &ContentDb) -> Option<Remembered> {
    crate::buildings::sections(content)
        .into_iter()
        .filter(|s| crate::buildings::in_scope(s.kind, crate::buildings::Scope::Buildings))
        .flat_map(|s| s.cards)
        .find(|c| crate::buildings::lock_text(view, content, c.id).is_none())
        .map(|c| Remembered::Place(c.id))
}

/// Called once per frame, before the panels draw: a menu that was just opened takes up its last
/// (or first) tool and shows its overlay, and the bulldozer's layer follows the open menu.
pub fn apply(
    state: &mut UiState,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
    view: Option<&SimView>,
) {
    if let Some(c) = state.entered.take()
        && remembers(c)
    {
        let pick = state
            .last_tool
            .iter()
            .find(|(k, _)| *k == c)
            .map(|(_, r)| *r)
            .or_else(|| first_tool(c, view));
        if let Some(r) = pick {
            r.take_up(tools);
        }
        if let Some(o) = overlay_of(c) {
            overlay.open_auto(o);
        }
    }
    tools.menu_layer = layer_of(state.category);
}

/// Called once per frame, after the panels: the build tool in hand while a menu is open is that
/// menu's last tool.
pub fn remember(state: &mut UiState, tools: &ToolState, content: Option<&ContentDb>) {
    let Some(c) = state.category.filter(|c| remembers(*c)) else {
        return;
    };
    let Some(r) = Remembered::of(tools) else {
        return;
    };
    // A tool belongs to the menu it was taken up in: the Power menu's turbine is not the
    // Buildings menu's last tool (they share the placement tool).
    if !belongs(c, r, content) {
        return;
    }
    match state.last_tool.iter_mut().find(|(k, _)| *k == c) {
        Some(slot) => slot.1 = r,
        None => state.last_tool.push((c, r)),
    }
}

/// The bulldozer layer of the building `def`: a water or power service's, `All` for the rest.
fn layer_of_def(content: Option<&ContentDb>, def: BuildingDefId) -> BulldozeLayer {
    let Some(db) = content else {
        return BulldozeLayer::All;
    };
    db.building(def)
        .service
        .as_deref()
        .and_then(|s| db.service_id(s))
        .map_or(BulldozeLayer::All, |s| {
            BulldozeLayer::of_service(db.service(s).kind)
        })
}

/// Whether `r` is a tool of menu `c` (a Power building in the Power menu, ...).
fn belongs(c: Category, r: Remembered, content: Option<&ContentDb>) -> bool {
    match (c, r) {
        (Category::Roads, Remembered::Tool(t)) => {
            matches!(
                t,
                ActiveTool::Road
                    | ActiveTool::Upgrade
                    | ActiveTool::Flip
                    | ActiveTool::Trees
                    | ActiveTool::Junctions
                    | ActiveTool::Interchange
            )
        }
        (Category::Zones, Remembered::Tool(t)) => t == ActiveTool::Zone,
        (Category::Power, Remembered::Utility(k)) => k == UtilityKind::Power,
        (Category::Water, Remembered::Utility(k)) => k == UtilityKind::Water,
        // The placement tool is shared: the building's layer says which menu it is of.
        (Category::Power, Remembered::Place(d)) => layer_of_def(content, d) == BulldozeLayer::Power,
        (Category::Water, Remembered::Place(d)) => layer_of_def(content, d) == BulldozeLayer::Water,
        (Category::Buildings, Remembered::Place(d)) => {
            layer_of_def(content, d) == BulldozeLayer::All
        }
        _ => false,
    }
}
