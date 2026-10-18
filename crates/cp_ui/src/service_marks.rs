//! The icons over the buildings of the service on screen (`architecture/system-design.md` §13.5
//! v6.2, T-254 step 2): while a service overlay is on (a service card switches it on), every
//! building of that kind carries its own icon above it, so the existing ones stand out from the
//! city; the ones of the card in hand are drawn larger.

use bevy::math::{Vec2, Vec3};
use bevy_egui::egui;
use cp_building::{BuildingGrid, BuildingState};
use cp_content::{BuildingDefId, ContentDb, ServiceKind};
use cp_sim::SimView;
use cp_tools::{ActiveTool, ToolState};

/// Icons are left out beyond this distance from the camera, metres.
pub const ICON_RANGE_M: f32 = 3000.0;
/// Side of an icon, points; the one of the card in hand is larger.
pub const ICON_SIDE: f32 = 22.0;
pub const ICON_SIDE_HELD: f32 = 30.0;
/// Height above the ground the icon hangs at, metres.
const ICON_LIFT_M: f32 = 20.0;

/// One icon to draw: where (world, on the ground), which def and whether it is the held type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ServiceMark {
    pub at: Vec2,
    pub def: BuildingDefId,
    pub held: bool,
    /// Under construction: drawn faded.
    pub building: bool,
}

/// The buildings of `kind` (Active or under construction) within [`ICON_RANGE_M`] of `cam`, in
/// id order; `held` marks the ones of the def the placement tool holds.
pub fn service_marks(
    buildings: &BuildingGrid,
    db: &ContentDb,
    kind: ServiceKind,
    held: Option<BuildingDefId>,
    cam: Vec2,
) -> Vec<ServiceMark> {
    buildings
        .iter()
        .filter(|(_, b)| {
            matches!(
                b.state,
                BuildingState::Active | BuildingState::UnderConstruction
            ) && b.pos.distance(cam) <= ICON_RANGE_M
        })
        .filter(|(_, b)| {
            db.building(b.def)
                .service
                .as_deref()
                .and_then(|s| db.service_id(s))
                .is_some_and(|s| db.service(s).kind == kind)
        })
        .map(|(_, b)| ServiceMark {
            at: b.pos,
            def: b.def,
            held: held == Some(b.def),
            building: b.state == BuildingState::UnderConstruction,
        })
        .collect()
}

/// Draws the icons while a service overlay is on.
pub fn draw_service_marks(
    mut contexts: bevy_egui::EguiContexts,
    view: Option<bevy::prelude::Res<SimView>>,
    overlay: bevy::prelude::Res<cp_render::OverlayState>,
    tools: bevy::prelude::Res<ToolState>,
    cameras: bevy::prelude::Query<(&bevy::prelude::Camera, &bevy::prelude::GlobalTransform)>,
) {
    let (Some(view), Some(kind)) = (view, overlay.active.service_kind()) else {
        return;
    };
    let Some(db) = view.content.as_deref() else {
        return;
    };
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let Ok((camera, transform)) = cameras.single() else {
        return;
    };
    let cam = transform.translation();
    let held = (tools.active == ActiveTool::Place)
        .then_some(tools.building.def)
        .flatten();
    let marks = service_marks(&view.buildings, db, kind, held, Vec2::new(cam.x, cam.z));
    if marks.is_empty() {
        return;
    }
    egui::Area::new(egui::Id::new("service_marks"))
        .order(egui::Order::Background)
        .interactable(false)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            for m in marks {
                let y = view.terrain.as_deref().map_or(0.0, |t| t.height(m.at)) + ICON_LIFT_M;
                let Ok(p) = camera.world_to_viewport(transform, Vec3::new(m.at.x, y, m.at.y))
                else {
                    continue;
                };
                let side = if m.held { ICON_SIDE_HELD } else { ICON_SIDE };
                let rect =
                    egui::Rect::from_center_size(egui::pos2(p.x, p.y), egui::vec2(side, side));
                let painter = ui.painter();
                painter.rect_filled(rect.expand(3.0), 4.0, egui::Color32::from_black_alpha(170));
                if m.held {
                    painter.rect_stroke(
                        rect.expand(3.0),
                        4.0,
                        egui::Stroke::new(2.0, egui::Color32::from_rgb(190, 235, 255)),
                        egui::StrokeKind::Outside,
                    );
                }
                let tint = if m.building {
                    egui::Color32::from_white_alpha(110)
                } else {
                    egui::Color32::WHITE
                };
                egui::Image::new(crate::icons::def_icon(&db.building(m.def).icon))
                    .tint(tint)
                    .paint_at(ui, rect);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_building::{AccessPoint, Building, BuildingFlags, Footprint};
    use cp_core::{Id, Tick};

    fn db() -> ContentDb {
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap()
    }

    fn put(grid: &mut BuildingGrid, db: &ContentDb, def: &str, x: f32, state: BuildingState) {
        grid.insert(Building {
            def: db.building_id(def).unwrap(),
            footprint: Footprint::Free { w: 2, d: 2 },
            level: 1,
            state,
            ready_at: Tick(0),
            pos: Vec2::new(x, 0.0),
            rot: 0.0,
            access: Some(AccessPoint {
                segment: Id::new(0, 0),
                t: 0.0,
            }),
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        });
    }

    #[test]
    fn every_building_of_the_kind_in_range_gets_a_mark_and_the_held_type_is_flagged() {
        let db = db();
        let mut grid = BuildingGrid::default();
        put(&mut grid, &db, "police_station", 0.0, BuildingState::Active);
        put(
            &mut grid,
            &db,
            "police_station",
            100.0,
            BuildingState::UnderConstruction,
        );
        put(
            &mut grid,
            &db,
            "police_station",
            5000.0,
            BuildingState::Active,
        );
        put(&mut grid, &db, "fire_station", 50.0, BuildingState::Active);
        put(
            &mut grid,
            &db,
            "police_station",
            200.0,
            BuildingState::Burned,
        );
        let police = db.building_id("police_station");
        let marks = service_marks(&grid, &db, ServiceKind::Police, police, Vec2::ZERO);
        assert_eq!(
            marks
                .iter()
                .map(|m| (m.at.x, m.held, m.building))
                .collect::<Vec<_>>(),
            vec![(0.0, true, false), (100.0, true, true)]
        );
        let fire = service_marks(&grid, &db, ServiceKind::Fire, police, Vec2::ZERO);
        assert_eq!(fire.len(), 1);
        assert!(!fire[0].held);
    }
}
