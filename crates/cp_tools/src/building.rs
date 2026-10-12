//! The service-building placement tool and the building half of the bulldozer.
//!
//! Pure: placement validity comes from `cp_building::validate_placement`, the same function the
//! `Place` command commits with; the plugin turns clicks into `BuildingCommand`s.

use std::f32::consts::FRAC_PI_2;

use bevy_math::Vec2;
use cp_building::{BuildingId, validate_placement};
use cp_content::BuildingDefId;
use cp_core::Cost;
use cp_sim::SimView;
use cp_zone::ZoneCellRef;

/// Pick distance from a building's lot centre before the exact cell test, metres.
const PICK_REACH: f32 = 40.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BuildingTool {
    pub def: Option<BuildingDefId>,
    /// Quarter turns, 0..4.
    pub quarters: u8,
}

impl BuildingTool {
    /// R key: next quarter turn.
    pub fn rotate(&mut self) {
        self.quarters = (self.quarters + 1) % 4;
    }

    pub fn rot(&self) -> f32 {
        f32::from(self.quarters) * FRAC_PI_2
    }
}

/// What the cursor would place.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacementGhost {
    /// The lot's cells when valid; the cell under the cursor when not (to draw something red).
    pub cells: Vec<ZoneCellRef>,
    pub valid: bool,
    pub cost: Option<Cost>,
    pub error: Option<String>,
}

/// The ghost for `tool` with the cursor at `hover`; `None` without a selected type or content.
pub fn preview(view: &SimView, tool: &BuildingTool, hover: Vec2) -> Option<PlacementGhost> {
    let def = tool.def?;
    let db = view.content.as_deref()?;
    Some(
        match validate_placement(
            db,
            &view.zones,
            &view.net,
            view.terrain.as_deref(),
            Some(&view.tiles),
            def,
            hover,
            tool.rot(),
        ) {
            Ok(plan) => PlacementGhost {
                cells: plan.cells(),
                valid: true,
                cost: Some(plan.cost),
                error: None,
            },
            Err(e) => PlacementGhost {
                cells: view.zones.cell_at(hover).into_iter().collect(),
                valid: false,
                cost: None,
                error: Some(e.to_string()),
            },
        },
    )
}

/// The building whose lot covers `p` (lowest id wins).
pub fn pick_building(view: &SimView, p: Vec2) -> Option<BuildingId> {
    view.buildings
        .iter()
        .filter(|(_, b)| b.pos.distance(p) <= PICK_REACH)
        .find(|(_, b)| {
            b.lot.cells().any(|c| {
                view.zones
                    .cell_footprint(c)
                    .is_some_and(|o| o.distance_to_point(p) <= 1e-3)
            })
        })
        .map(|(id, _)| id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use bevy_math::Vec3;
    use cp_content::ContentDb;
    use cp_core::curve::CubicBezier3;
    use cp_net::{Elevation, NetGraph};
    use cp_sim::SimRunner;
    use cp_terrain::TileOwnership;

    fn sim() -> (SimRunner, Arc<ContentDb>) {
        let c = Arc::new(
            ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
                .unwrap(),
        );
        let road = c.road_id("basic_2l").unwrap();
        let def = c.road(road).clone();
        let mut sim = SimRunner::new(1);
        sim.set_content(Arc::clone(&c));
        sim.world_mut().insert_resource(TileOwnership::only((1, 1)));
        sim.world_mut()
            .resource_mut::<NetGraph>()
            .add_segment(
                &def,
                road,
                CubicBezier3::line(
                    Vec3::new(3000.0, 0.0, 3000.0),
                    Vec3::new(3200.0, 0.0, 3000.0),
                ),
                Elevation::Ground,
            )
            .unwrap();
        sim.tick_once();
        (sim, c)
    }

    #[test]
    fn ghost_is_valid_beside_the_road_and_red_elsewhere() {
        let (mut sim, c) = sim();
        let tool = BuildingTool {
            def: c.building_id("wind_turbine"),
            quarters: 0,
        };
        let view = sim.view();
        let near = Vec2::new(3100.0, 3012.0);
        let g = preview(&view, &tool, near).unwrap();
        assert!(
            g.valid && g.cells.len() == 4 && g.cost == Some(Cost::from_units(6000.0)),
            "{g:?}"
        );
        let far = preview(&view, &tool, Vec2::new(3100.0, 3300.0)).unwrap();
        assert!(!far.valid);
        assert_eq!(far.error.as_deref(), Some("must be next to a road"));
        assert!(preview(&view, &BuildingTool::default(), near).is_none());
        let _ = &mut sim;
    }

    #[test]
    fn rotation_cycles_and_picking_finds_the_building() {
        let (mut sim, c) = sim();
        let mut tool = BuildingTool {
            def: c.building_id("wind_turbine"),
            quarters: 0,
        };
        for _ in 0..4 {
            tool.rotate();
        }
        assert_eq!(tool.quarters, 0);
        let at = Vec2::new(3100.0, 3012.0);
        let g = preview(&sim.view(), &tool, at).unwrap();
        assert!(g.valid);
        sim.push_command(
            cp_sim::SimCommand::Building(cp_building::BuildingCommand::Place {
                def: tool.def.unwrap(),
                pos: at,
                rot: tool.rot(),
            }),
            sim.tick(),
        );
        sim.apply_due_commands();
        let view = sim.view();
        let id = pick_building(&view, at).expect("the new building is under the cursor");
        assert_eq!(view.buildings.iter().next().unwrap().0, id);
        assert!(pick_building(&view, Vec2::new(3100.0, 3300.0)).is_none());
        // The same spot is now taken.
        assert!(!preview(&view, &tool, at).unwrap().valid);
    }
}
