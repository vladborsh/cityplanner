//! The info panel of the selected thing (T-044): a window with the rows of
//! [`cp_sim::Inspection::rows`] and a few buttons that move the selection (a vehicle to its
//! driver, a citizen to the home or workplace). It draws exactly what the sim reports.

use bevy_egui::egui;
use cp_building::BuildingCommand;
use cp_sim::{Inspection, Selection, SimCommand, SimView};
use cp_tools::{CommandOutbox, Inspected, Picked, ToolState};

use crate::icons::Icon;

/// Where the buttons of the last frame were (tests click them).
#[derive(Clone, Debug, Default)]
pub struct InfoRects {
    /// `(label, rectangle, selection it makes)`.
    pub goto: Vec<(String, egui::Rect, Selection)>,
    /// `(label, rectangle)` of the Move / Demolish buttons of a service building (T-177).
    pub actions: Vec<(String, egui::Rect)>,
}

/// Shows the panel for `inspected` (nothing when empty). Close and the go-to buttons write
/// `picked`; a service building also gets Move (picks it up in `tools`) and Demolish (asks
/// again, then sends the command).
pub fn info_panel(
    ctx: &egui::Context,
    inspected: &Inspected,
    picked: &mut Picked,
    view: Option<&SimView>,
    tools: &mut ToolState,
    outbox: &mut CommandOutbox,
) -> InfoRects {
    let mut rects = InfoRects::default();
    let Some(info) = &inspected.0 else {
        return rects;
    };
    // A line has its own panel with controls (`lines::line_info_panel`).
    if matches!(info, Inspection::Line(_)) {
        return rects;
    }
    let mut open = true;
    let title = info.title();
    crate::frame::window("info_panel", Icon::Info, &title, &mut open)
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-8.0, -8.0))
        .show(ctx, |ui| {
            egui::Grid::new("info_rows").num_columns(2).show(ui, |ui| {
                for (label, value) in info.rows() {
                    ui.weak(label);
                    ui.label(value);
                    ui.end_row();
                }
            });
            let mut go = |ui: &mut egui::Ui, label: &str, to: Selection| {
                let r = ui.button(label);
                rects.goto.push((label.to_string(), r.rect, to));
                if r.clicked() {
                    picked.0 = Some(to);
                }
            };
            match info {
                Inspection::Vehicle(v) => {
                    ui.separator();
                    go(ui, "Driver", Selection::Citizen(v.driver));
                }
                Inspection::Citizen(c) => {
                    ui.separator();
                    ui.horizontal(|ui| {
                        if let Some((h, _)) = &c.home {
                            go(ui, "Home", Selection::Building(*h));
                        }
                        if let Some((w, _, _)) = &c.work {
                            go(ui, "Work", Selection::Building(*w));
                        }
                    });
                }
                Inspection::Building(b) => {
                    let is_service = view.and_then(|v| v.content.as_deref()).is_some_and(|db| {
                        db.building_id(&b.def)
                            .is_some_and(|d| db.building(d).service.is_some())
                    });
                    if let Some(db) = view.and_then(|v| v.content.as_deref()) {
                        let mut rows = crate::buildings::unique_rows(db, &b.def);
                        if let Some(v) = view {
                            rows.extend(crate::industry::industry_rows(v, b.id));
                        }
                        if !rows.is_empty() {
                            egui::Grid::new("unique_rows")
                                .num_columns(2)
                                .show(ui, |ui| {
                                    for (label, value) in rows {
                                        ui.weak(label);
                                        ui.label(value);
                                        ui.end_row();
                                    }
                                });
                        }
                    }
                    if let Some(view) = view {
                        crate::industry::warehouse_controls(
                            ui,
                            b.id,
                            view,
                            outbox,
                            &mut rects.actions,
                        );
                    }
                    if let (true, Some(view)) = (is_service, view) {
                        ui.separator();
                        building_actions(ui, b.id, view, picked, tools, outbox, &mut rects);
                    }
                }
                _ => {}
            }
        });
    if !open {
        picked.0 = None;
    }
    rects
}

/// Move and Demolish of a service building. Demolish asks once more before it sends.
fn building_actions(
    ui: &mut egui::Ui,
    id: cp_building::BuildingId,
    view: &SimView,
    picked: &mut Picked,
    tools: &mut ToolState,
    outbox: &mut CommandOutbox,
    rects: &mut InfoRects,
) {
    let key = egui::Id::new("demolish_confirm");
    let asking = ui.data(|d| d.get_temp::<cp_building::BuildingId>(key)) == Some(id);
    ui.horizontal(|ui| {
        let r = ui.button("Move");
        rects.actions.push(("Move".into(), r.rect));
        if r.clicked() && tools.begin_move(view, id) {
            ui.data_mut(|d| d.remove::<cp_building::BuildingId>(key));
            picked.0 = None;
        }
        let label = if asking {
            "Confirm demolish"
        } else {
            "Demolish"
        };
        let r = ui.button(label);
        rects.actions.push((label.into(), r.rect));
        if r.clicked() {
            if asking {
                outbox.push(SimCommand::Building(BuildingCommand::Demolish {
                    building: id,
                }));
                ui.data_mut(|d| d.remove::<cp_building::BuildingId>(key));
                picked.0 = None;
            } else {
                ui.data_mut(|d| d.insert_temp(key, id));
            }
        }
        if asking {
            let r = ui.button("Cancel");
            rects.actions.push(("Cancel".into(), r.rect));
            if r.clicked() {
                ui.data_mut(|d| d.remove::<cp_building::BuildingId>(key));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_core::Id;
    use cp_sim::{CitizenInfo, SegmentInfo, VehicleInfo};

    fn input(t: f64, events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            time: Some(t),
            events,
            ..Default::default()
        }
    }

    fn frame(
        ctx: &egui::Context,
        info: &Inspected,
        picked: &mut Picked,
        t: f64,
        events: Vec<egui::Event>,
    ) -> InfoRects {
        let mut rects = InfoRects::default();
        let (mut tools, mut outbox) = (ToolState::default(), CommandOutbox::default());
        crate::testutil::frame(ctx, input(t, events), |ui| {
            rects = info_panel(ui.ctx(), info, picked, None, &mut tools, &mut outbox);
        });
        rects
    }

    fn click(ctx: &egui::Context, info: &Inspected, picked: &mut Picked, pos: egui::Pos2, t: f64) {
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(
            ctx,
            info,
            picked,
            t + 0.1,
            vec![egui::Event::PointerMoved(pos), button(true)],
        );
        frame(ctx, info, picked, t + 0.2, vec![button(false)]);
    }

    fn segment() -> Inspection {
        Inspection::Segment(SegmentInfo {
            id: Id::new(3, 0),
            road: "basic_2l".into(),
            name: "Basic street".into(),
            length: 80.0,
            elevation: cp_net::Elevation::Ground,
            lanes: 4,
            speed_kmh: 40.0,
            load: 0.25,
            load_avg: 0.5,
            line: vec![],
        })
    }

    fn vehicle(driver: cp_citizen::CitizenId) -> Inspection {
        Inspection::Vehicle(VehicleInfo {
            key: 5,
            kind: 0,
            speed_mps: 10.0,
            driver,
            trip: None,
            lanes_left: 3,
            lanes_total: 5,
            waiting_to_spawn: false,
            route: vec![],
        })
    }

    #[test]
    fn a_segment_panel_has_no_jump_buttons_and_no_selection_no_panel() {
        let ctx = egui::Context::default();
        let seg = Inspected(Some(segment()));
        let mut picked = Picked(Some(Selection::Segment(Id::new(3, 0))));
        let r = frame(&ctx, &seg, &mut picked, 0.0, vec![]);
        assert!(r.goto.is_empty());
        let none = Inspected(None);
        frame(&ctx, &none, &mut picked, 0.1, vec![]);
        assert_eq!(picked.0, Some(Selection::Segment(Id::new(3, 0))));
    }

    #[test]
    fn the_vehicle_panel_jumps_to_its_driver_and_the_citizen_panel_to_home_and_work() {
        let ctx = egui::Context::default();
        let driver = Id::new(9, 0);
        let car = Inspected(Some(vehicle(driver)));
        let mut picked = Picked(Some(Selection::Vehicle(5)));
        frame(&ctx, &car, &mut picked, 0.0, vec![]);
        let r = frame(&ctx, &car, &mut picked, 0.05, vec![]);
        let (label, rect, to) = r.goto[0].clone();
        assert_eq!((label.as_str(), to), ("Driver", Selection::Citizen(driver)));
        click(&ctx, &car, &mut picked, rect.center(), 1.0);
        assert_eq!(picked.0, Some(Selection::Citizen(driver)));

        let (home, work) = (Id::new(1, 0), Id::new(2, 0));
        let citizen = Inspected(Some(Inspection::Citizen(CitizenInfo {
            id: driver,
            age_years: 30,
            education: 1,
            wellbeing: 50,
            has_car: true,
            location: cp_citizen::Location::Home,
            household_size: 3,
            home: Some((home, "House".into())),
            work: Some((work, "Shop".into(), 1)),
            trip: None,
        })));
        let mut r = InfoRects::default();
        for k in 0..6 {
            r = frame(
                &ctx,
                &citizen,
                &mut picked,
                2.0 + 0.05 * f64::from(k),
                vec![],
            );
        }
        let labels: Vec<_> = r.goto.iter().map(|g| g.0.as_str()).collect();
        assert_eq!(labels, ["Home", "Work"]);
        let work_rect = r.goto[1].1;
        click(&ctx, &citizen, &mut picked, work_rect.center(), 3.0);
        assert_eq!(picked.0, Some(Selection::Building(work)));
    }

    /// A world with one water tower; the view, its id and its inspection.
    fn tower_world() -> (SimView, cp_building::BuildingId, Inspected) {
        use std::sync::Arc;
        let db = Arc::new(
            cp_content::ContentDb::load_dir(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/content"
            ))
            .unwrap(),
        );
        let tower = db.building_id("water_tower").unwrap();
        let mut sim = cp_sim::SimRunner::new(1);
        sim.set_content(db);
        sim.world_mut()
            .insert_resource(cp_terrain::TileOwnership::only((2, 2)));
        sim.push_command(
            SimCommand::Building(BuildingCommand::Place {
                def: tower,
                pos: bevy::math::Vec2::new(5100.0, 5100.0),
                rot: 0.0,
            }),
            sim.tick(),
        );
        sim.apply_due_commands();
        sim.tick_once();
        let id = sim
            .world()
            .resource::<cp_building::BuildingGrid>()
            .iter()
            .next()
            .unwrap()
            .0;
        let info = Inspected(sim.inspect(Selection::Building(id)));
        (sim.view(), id, info)
    }

    fn building_frame(
        ctx: &egui::Context,
        t: f64,
        events: Vec<egui::Event>,
        world: &(SimView, cp_building::BuildingId, Inspected),
        picked: &mut Picked,
        tools: &mut ToolState,
        outbox: &mut CommandOutbox,
    ) -> InfoRects {
        let mut rects = InfoRects::default();
        crate::testutil::frame(ctx, input(t, events), |ui| {
            rects = info_panel(ui.ctx(), &world.2, picked, Some(&world.0), tools, outbox);
        });
        rects
    }

    fn click_at(
        ctx: &egui::Context,
        t: f64,
        pos: egui::Pos2,
        world: &(SimView, cp_building::BuildingId, Inspected),
        picked: &mut Picked,
        tools: &mut ToolState,
        outbox: &mut CommandOutbox,
    ) {
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let ev = vec![egui::Event::PointerMoved(pos), button(true)];
        building_frame(ctx, t + 0.1, ev, world, picked, tools, outbox);
        let ev = vec![button(false)];
        building_frame(ctx, t + 0.2, ev, world, picked, tools, outbox);
    }

    fn action(rects: &InfoRects, label: &str) -> egui::Pos2 {
        rects
            .actions
            .iter()
            .find(|(l, _)| l == label)
            .unwrap_or_else(|| panic!("no `{label}` button in {:?}", rects.actions))
            .1
            .center()
    }

    /// T-177: the card of a service building has Move (picks it up) and Demolish (asks first).
    #[test]
    fn a_service_building_card_moves_and_demolishes() {
        let world = tower_world();
        let id = world.1;
        let ctx = egui::Context::default();
        let (mut picked, mut tools, mut outbox) = (
            Picked(Some(Selection::Building(id))),
            ToolState::default(),
            CommandOutbox::default(),
        );
        for k in 0..3 {
            building_frame(
                &ctx,
                0.05 * f64::from(k),
                vec![],
                &world,
                &mut picked,
                &mut tools,
                &mut outbox,
            );
        }
        let r = building_frame(
            &ctx,
            0.2,
            vec![],
            &world,
            &mut picked,
            &mut tools,
            &mut outbox,
        );

        // Demolish asks once more; Cancel drops the question; the second Demolish sends.
        click_at(
            &ctx,
            1.0,
            action(&r, "Demolish"),
            &world,
            &mut picked,
            &mut tools,
            &mut outbox,
        );
        assert!(outbox.is_empty());
        let r = building_frame(
            &ctx,
            1.5,
            vec![],
            &world,
            &mut picked,
            &mut tools,
            &mut outbox,
        );
        click_at(
            &ctx,
            2.0,
            action(&r, "Cancel"),
            &world,
            &mut picked,
            &mut tools,
            &mut outbox,
        );
        let r = building_frame(
            &ctx,
            2.5,
            vec![],
            &world,
            &mut picked,
            &mut tools,
            &mut outbox,
        );
        assert!(r.actions.iter().all(|(l, _)| l != "Confirm demolish"));
        click_at(
            &ctx,
            3.0,
            action(&r, "Demolish"),
            &world,
            &mut picked,
            &mut tools,
            &mut outbox,
        );
        let r = building_frame(
            &ctx,
            3.5,
            vec![],
            &world,
            &mut picked,
            &mut tools,
            &mut outbox,
        );
        click_at(
            &ctx,
            4.0,
            action(&r, "Confirm demolish"),
            &world,
            &mut picked,
            &mut tools,
            &mut outbox,
        );
        assert_eq!(
            outbox.iter().collect::<Vec<_>>(),
            [&SimCommand::Building(BuildingCommand::Demolish {
                building: id
            })]
        );
        assert_eq!(picked.0, None);

        // Move picks the building up: the placement tool with it as the moving one.
        let (mut picked, mut tools, mut outbox) = (
            Picked(Some(Selection::Building(id))),
            ToolState::default(),
            CommandOutbox::default(),
        );
        let r = building_frame(
            &ctx,
            5.0,
            vec![],
            &world,
            &mut picked,
            &mut tools,
            &mut outbox,
        );
        click_at(
            &ctx,
            6.0,
            action(&r, "Move"),
            &world,
            &mut picked,
            &mut tools,
            &mut outbox,
        );
        assert_eq!(tools.active, cp_tools::ActiveTool::Place);
        assert_eq!(tools.building.moving, Some(id));
        assert_eq!(picked.0, None);
        assert!(outbox.is_empty());
    }

    /// A zoned house has no Move / Demolish here: only service buildings move.
    #[test]
    fn a_card_without_a_service_building_has_no_actions() {
        let ctx = egui::Context::default();
        let seg = Inspected(Some(segment()));
        let mut picked = Picked(None);
        let r = frame(&ctx, &seg, &mut picked, 0.0, vec![]);
        assert!(r.actions.is_empty());
    }

    /// T-219: a warehouse's card has the mode and cargo buttons; Fill and a cargo send
    /// `SetWarehouse`, and a building that is not a warehouse has none.
    #[test]
    fn a_warehouse_card_sends_set_warehouse_for_the_mode_and_the_cargo() {
        use crate::industry::tests::{put, world_with_areas};
        use cp_industry::{IndustryCommand, WarehouseMode};
        let (mut sim, db, at) = world_with_areas(1);
        let wh = put(&mut sim, &db, "forest_warehouse", at[0]);
        let other = put(&mut sim, &db, "forest_main", at[0]);
        let make = |sim: &cp_sim::SimRunner, id| {
            let info = Inspected(sim.inspect(Selection::Building(id)));
            (sim.view(), id, info)
        };
        let world = make(&sim, wh);
        let ctx = egui::Context::default();
        let (mut picked, mut tools, mut outbox) = (
            Picked(Some(Selection::Building(wh))),
            ToolState::default(),
            CommandOutbox::default(),
        );
        let mut r = InfoRects::default();
        for k in 0..3 {
            r = building_frame(
                &ctx,
                0.05 * f64::from(k),
                vec![],
                &world,
                &mut picked,
                &mut tools,
                &mut outbox,
            );
        }
        click_at(
            &ctx,
            1.0,
            action(&r, "Fill"),
            &world,
            &mut picked,
            &mut tools,
            &mut outbox,
        );
        click_at(
            &ctx,
            2.0,
            action(&r, "Logs"),
            &world,
            &mut picked,
            &mut tools,
            &mut outbox,
        );
        let sent: Vec<_> = outbox.iter().cloned().collect();
        assert_eq!(
            sent,
            [
                SimCommand::Industry(IndustryCommand::SetWarehouse {
                    building: wh,
                    cargo: None,
                    mode: WarehouseMode::Fill,
                }),
                SimCommand::Industry(IndustryCommand::SetWarehouse {
                    building: wh,
                    cargo: Some(cp_content::CargoKind::Logs),
                    mode: WarehouseMode::Balanced,
                }),
            ]
        );

        let world = make(&sim, other);
        picked.0 = Some(Selection::Building(other));
        let mut r = InfoRects::default();
        for k in 0..3 {
            r = building_frame(
                &ctx,
                5.0 + 0.05 * f64::from(k),
                vec![],
                &world,
                &mut picked,
                &mut tools,
                &mut outbox,
            );
        }
        assert!(
            r.actions.iter().all(|(l, _)| l != "Fill"),
            "{:?}",
            r.actions
        );
    }
}
