//! What a road command would take down (BUG-096), from the view, by the same pure functions the
//! command layer uses (`cp_net::command_strips`, `cp_building::buildings_under_road`): the ghost
//! tints these buildings red, and a click over a protected one asks the player first
//! ([`ConfirmDemolish`]).

use cp_building::BuildingId;
use cp_sim::{SimCommand, SimView};

/// The buildings under the roads of some commands.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnderRoad {
    /// Every building the commands would demolish, in id order.
    pub buildings: Vec<BuildingId>,
    /// The player-facing names of the protected ones (a service, a unique building, a park):
    /// the click asks before it takes them.
    pub protected: Vec<String>,
}

/// [`UnderRoad`] of the road commands among `commands` (others are ignored).
pub fn under_road(view: &SimView, commands: &[SimCommand]) -> UnderRoad {
    let (Some(terrain), Some(content)) = (view.terrain.as_deref(), view.content.as_deref()) else {
        return UnderRoad::default();
    };
    let mut buildings: Vec<BuildingId> = commands
        .iter()
        .filter_map(|c| match c {
            SimCommand::Net(n) | SimCommand::NetDemolish(n) => Some(n),
            _ => None,
        })
        .flat_map(|n| {
            let strips = cp_net::command_strips(&view.net, terrain, &view.tiles, content, n);
            cp_building::buildings_under_road(&view.buildings, &strips)
        })
        .collect();
    buildings.sort();
    buildings.dedup();
    let protected = buildings
        .iter()
        .filter_map(|id| view.buildings.get(*id))
        .filter(|b| !cp_building::is_growable(content.building(b.def)))
        .map(|b| content.building(b.def).name.clone())
        .collect();
    UnderRoad {
        buildings,
        protected,
    }
}

/// A road click that would demolish protected buildings, held until the player answers.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfirmDemolish {
    /// What the question names.
    pub names: Vec<String>,
    /// What a yes sends (the road commands as `NetDemolish`).
    pub commands: Vec<SimCommand>,
    /// The road tool as it was before the click: a no puts it back.
    pub restore: Option<crate::RoadTool>,
}

/// The commands of a road click with what they would take down: sent now when nothing protected
/// is hit (the grown buildings go with the road), else held as a question.
pub fn road_click(
    view: Option<&SimView>,
    commands: Vec<SimCommand>,
) -> Result<Vec<SimCommand>, ConfirmDemolish> {
    let Some(view) = view else {
        return Ok(commands);
    };
    let hit = under_road(view, &commands);
    if hit.protected.is_empty() {
        return Ok(commands);
    }
    Err(ConfirmDemolish {
        names: hit.protected,
        commands: commands
            .into_iter()
            .map(|c| match c {
                SimCommand::Net(n) => SimCommand::NetDemolish(n),
                other => other,
            })
            .collect(),
        restore: None,
    })
}
