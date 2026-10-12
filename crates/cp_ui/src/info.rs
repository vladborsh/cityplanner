//! The info panel of the selected thing (T-044): a window with the rows of
//! [`cp_sim::Inspection::rows`] and a few buttons that move the selection (a vehicle to its
//! driver, a citizen to the home or workplace). It draws exactly what the sim reports.

use bevy_egui::egui;
use cp_sim::{Inspection, Selection};
use cp_tools::{Inspected, Picked};

/// Where the buttons of the last frame were (tests click them).
#[derive(Clone, Debug, Default)]
pub struct InfoRects {
    /// `(label, rectangle, selection it makes)`.
    pub goto: Vec<(String, egui::Rect, Selection)>,
}

/// Shows the panel for `inspected` (nothing when empty). Close and the go-to buttons write
/// `picked`.
pub fn info_panel(ctx: &egui::Context, inspected: &Inspected, picked: &mut Picked) -> InfoRects {
    let mut rects = InfoRects::default();
    let Some(info) = &inspected.0 else {
        return rects;
    };
    let mut open = true;
    egui::Window::new(info.title())
        .id("info_panel".into())
        .open(&mut open)
        .anchor(egui::Align2::LEFT_TOP, egui::vec2(8.0, 40.0))
        .resizable(false)
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
                _ => {}
            }
        });
    if !open {
        picked.0 = None;
    }
    rects
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
        crate::testutil::frame(ctx, input(t, events), |ui| {
            rects = info_panel(ui.ctx(), info, picked);
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
}
