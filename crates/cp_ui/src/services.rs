//! The City services window (T-232): one row per utility and service with a bar centred on
//! the balance of its supply against its demand. Red to the left is a shortage, yellow in the
//! middle is balanced or tight, green to the right is a surplus. Reads
//! [`SimView::city_services`]; a click on a row opens the matching map overlay. Locked sectors
//! (their service is not open yet) are greyed with a padlock and have no bar.

use bevy_egui::egui;
use cp_render::{Overlay, OverlayState};
use cp_sim::SimView;
use cp_sim::city_services::{Sector, SectorBalance};

use crate::frame;
use crate::icons::Icon;
use crate::style::Sizes;

/// Share of the demand, either way, that still counts as balanced (the yellow band).
pub const BALANCED_BAND: f32 = 0.10;
/// Bar size in points.
const BAR: egui::Vec2 = egui::vec2(200.0, 14.0);

/// Where the balance falls on the bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Shortage,
    Balanced,
    Surplus,
}

/// The verdict for a balance in `-1..=1`.
pub fn verdict(balance: f32) -> Verdict {
    if balance < -BALANCED_BAND {
        Verdict::Shortage
    } else if balance > BALANCED_BAND {
        Verdict::Surplus
    } else {
        Verdict::Balanced
    }
}

/// The marker's place along the bar, `0..=1` from the left: the balance clamped to ±100 %.
pub fn marker_pos(balance: f32) -> f32 {
    (balance.clamp(-1.0, 1.0) + 1.0) * 0.5
}

/// The signed percentage shown at the right, `"+25 %"`, `"-100 %"`.
pub fn percent_text(balance: f32) -> String {
    format!("{:+.0} %", balance.clamp(-1.0, 1.0) * 100.0)
}

/// `"supply / demand unit"`.
pub fn numbers_text(row: &SectorBalance) -> String {
    format!("{:.0} / {:.0} {}", row.supply, row.demand, row.unit)
}

/// The overlay a click on the row opens.
pub fn overlay_of(sector: Sector) -> Overlay {
    match sector {
        Sector::Water | Sector::Sewage => Overlay::Water,
        Sector::Power => Overlay::Power,
        Sector::Garbage => Overlay::Garbage,
        Sector::Health => Overlay::Health,
        Sector::Deathcare => Overlay::Deathcare,
        Sector::Police => Overlay::Police,
        Sector::Fire => Overlay::Fire,
        Sector::Elementary | Sector::HighSchool | Sector::University => Overlay::Education,
        Sector::Mail | Sector::Sorting => Overlay::Mail,
    }
}

fn icon_of(sector: Sector) -> Icon {
    match sector {
        Sector::Water => Icon::Water,
        Sector::Sewage => Icon::SewageTreatment,
        Sector::Power => Icon::Power,
        Sector::Garbage => Icon::OverlayGarbage,
        Sector::Health => Icon::OverlayHealth,
        Sector::Deathcare => Icon::OverlayDeathcare,
        Sector::Police => Icon::OverlayPolice,
        Sector::Fire => Icon::OverlayFire,
        Sector::Elementary | Sector::HighSchool | Sector::University => Icon::OverlayEducation,
        Sector::Mail => Icon::OverlayMail,
        Sector::Sorting => Icon::CategoryPost,
    }
}

const RED: egui::Color32 = egui::Color32::from_rgb(200, 70, 65);
const YELLOW: egui::Color32 = egui::Color32::from_rgb(235, 195, 60);
const GREEN: egui::Color32 = egui::Color32::from_rgb(85, 180, 90);

/// The bar with its three zones and the marker.
fn bar(ui: &mut egui::Ui, balance: f32) {
    let (rect, _) = ui.allocate_exact_size(BAR, egui::Sense::hover());
    let p = ui.painter();
    let at = |t: f32| rect.left() + rect.width() * t;
    let (lo, hi) = (0.5 - BALANCED_BAND * 0.5, 0.5 + BALANCED_BAND * 0.5);
    for (a, b, c) in [(0.0, lo, RED), (lo, hi, YELLOW), (hi, 1.0, GREEN)] {
        p.rect_filled(
            egui::Rect::from_x_y_ranges(at(a)..=at(b), rect.y_range()),
            0.0,
            c.gamma_multiply(0.75),
        );
    }
    let x = at(marker_pos(balance));
    p.line_segment(
        [
            egui::pos2(x, rect.top() - 2.0),
            egui::pos2(x, rect.bottom() + 2.0),
        ],
        egui::Stroke::new(3.0, egui::Color32::WHITE),
    );
}

/// A small padlock.
fn padlock(ui: &mut egui::Ui) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
    let p = ui.painter();
    let grey = egui::Color32::from_gray(150);
    p.rect_filled(
        egui::Rect::from_min_max(
            r.lerp_inside(egui::vec2(0.15, 0.45)),
            r.lerp_inside(egui::vec2(0.85, 0.95)),
        ),
        2.0,
        grey,
    );
    p.add(egui::Shape::line(
        vec![
            r.lerp_inside(egui::vec2(0.3, 0.45)),
            r.lerp_inside(egui::vec2(0.3, 0.2)),
            r.lerp_inside(egui::vec2(0.7, 0.2)),
            r.lerp_inside(egui::vec2(0.7, 0.45)),
        ],
        egui::Stroke::new(2.0, grey),
    ));
}

/// The rows; returns the sector whose row was clicked.
pub fn services_contents(ui: &mut egui::Ui, rows: &[SectorBalance]) -> Option<Sector> {
    let mut clicked = None;
    ui.set_min_size(egui::vec2(480.0, 120.0));
    if rows.is_empty() {
        ui.weak("No figures yet");
        return None;
    }
    egui::Grid::new("city_services")
        .num_columns(5)
        .spacing([10.0, 8.0])
        .show(ui, |ui| {
            for row in rows {
                let sense = ui.scope(|ui| {
                    if row.locked {
                        ui.disable();
                    }
                    icon_of(row.sector).show(ui, Sizes::ROW_ICON);
                    ui.label(row.sector.label());
                    if row.locked {
                        ui.allocate_exact_size(BAR, egui::Sense::hover());
                        ui.label("locked");
                        padlock(ui);
                    } else {
                        bar(ui, row.balance());
                        ui.label(numbers_text(row));
                        ui.label(percent_text(row.balance()));
                    }
                });
                if !row.locked
                    && ui
                        .interact(
                            sense.response.rect,
                            egui::Id::new(("service_row", row.sector as u8)),
                            egui::Sense::click(),
                        )
                        .on_hover_text("Show the overlay")
                        .clicked()
                {
                    clicked = Some(row.sector);
                }
                ui.end_row();
            }
        });
    clicked
}

/// The rows of the City services window that belong to `overlay` and are open: Water gives
/// water and sewage, Education the three schools, a service its own row; others none.
pub fn overlay_rows(view: &SimView, overlay: Overlay) -> Vec<&SectorBalance> {
    view.city_services()
        .iter()
        .filter(|r| !r.locked && overlay_of(r.sector) == overlay)
        .collect()
}

/// Offset of the overlay panel from the top-right screen corner: right margin, and just
/// under the top bar.
fn overlay_panel_offset() -> egui::Vec2 {
    egui::vec2(-12.0, Sizes::TOP_BAR_H + 8.0)
}

/// A compact panel with the rows of the active overlay (T-241), top-right under the top bar:
/// the same bars as the City services window. Nothing for an overlay without a service.
pub fn overlay_services_panel(ctx: &egui::Context, view: &SimView, overlay: Overlay) {
    let rows = overlay_rows(view, overlay);
    if rows.is_empty() {
        return;
    }
    // BUG-074: top-right of the map, just under the top bar, so it never covers the build
    // menu (left) or the palette / demand panel.
    egui::Area::new(egui::Id::new("overlay_services"))
        .anchor(egui::Align2::RIGHT_TOP, overlay_panel_offset())
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                egui::Grid::new("overlay_services_rows")
                    .num_columns(5)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        for row in rows {
                            icon_of(row.sector).show(ui, Sizes::ROW_ICON);
                            ui.label(row.sector.label());
                            bar(ui, row.balance());
                            ui.label(numbers_text(row));
                            ui.label(percent_text(row.balance()));
                            ui.end_row();
                        }
                    });
            });
        });
}

/// The window; a click on a row switches the map overlay on.
pub fn services_window(
    ctx: &egui::Context,
    open: &mut bool,
    view: &SimView,
    overlay: &mut OverlayState,
) {
    let clicked = frame::window("services", Icon::Services, "City services", open)
        .show(ctx, |ui| services_contents(ui, view.city_services()))
        .flatten();
    if let Some(s) = clicked {
        overlay.active = overlay_of(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{frame_texts, input};

    fn row(sector: Sector, supply: f32, demand: f32, locked: bool) -> SectorBalance {
        SectorBalance {
            sector,
            supply,
            demand,
            unit: "units/day",
            locked,
        }
    }

    #[test]
    fn the_marker_is_the_balance_clamped_and_the_zones_follow_it() {
        // 300 against 28: far above +100 %, the marker stops at the right end.
        let surplus = row(Sector::Water, 300.0, 28.0, false);
        assert_eq!(marker_pos(surplus.balance()), 1.0);
        assert_eq!(verdict(surplus.balance()), Verdict::Surplus);
        // Nothing against something: the left end.
        let none = row(Sector::Health, 0.0, 40.0, false);
        assert_eq!(marker_pos(none.balance()), 0.0);
        assert_eq!(verdict(none.balance()), Verdict::Shortage);
        assert_eq!(percent_text(none.balance()), "-100 %");
        // Equal: the middle, balanced; five percent either way is still balanced.
        let even = row(Sector::Power, 100.0, 100.0, false);
        assert_eq!(marker_pos(even.balance()), 0.5);
        assert_eq!(verdict(even.balance()), Verdict::Balanced);
        assert_eq!(
            verdict(row(Sector::Power, 95.0, 100.0, false).balance()),
            Verdict::Balanced
        );
        assert_eq!(
            verdict(row(Sector::Power, 80.0, 100.0, false).balance()),
            Verdict::Shortage
        );
        assert_eq!(
            verdict(row(Sector::Power, 125.0, 100.0, false).balance()),
            Verdict::Surplus
        );
        // No demand: the divisor is 1, so a supply of 1 is already +100 %.
        assert_eq!(row(Sector::Fire, 1.0, 0.0, false).balance(), 1.0);
        assert_eq!(row(Sector::Fire, 0.0, 0.0, false).balance(), 0.0);
        assert_eq!(percent_text(0.256), "+26 %");
        assert_eq!(numbers_text(&surplus), "300 / 28 units/day");
    }

    #[test]
    fn a_locked_row_is_greyed_without_a_bar_and_a_click_opens_its_overlay() {
        let rows = [
            row(Sector::Water, 300.0, 28.0, false),
            row(Sector::University, 0.0, 5.0, true),
        ];
        let ctx = egui::Context::default();
        let mut texts = Vec::new();
        for t in [0.0, 0.1] {
            texts = frame_texts(&ctx, input(t, vec![]), |ui| {
                services_contents(ui, &rows);
            })
            .into_iter()
            .map(|(s, _)| s)
            .collect();
        }
        assert!(
            texts.contains(&"300 / 28 units/day".to_string()),
            "{texts:?}"
        );
        assert!(texts.contains(&"+100 %".to_string()));
        assert!(texts.contains(&"locked".to_string()));
        // The locked row shows no numbers and no percentage.
        assert!(!texts.iter().any(|t| t.starts_with("0 / 5")), "{texts:?}");
        assert_eq!(overlay_of(Sector::Sewage), Overlay::Water);
        assert_eq!(overlay_of(Sector::HighSchool), Overlay::Education);
        assert_eq!(overlay_of(Sector::Mail), Overlay::Mail);
        assert_eq!(overlay_of(Sector::Sorting), Overlay::Mail);
    }

    /// T-241: an active service overlay shows its rows (Water also sewage) in a compact panel;
    /// no overlay, or one without a service, shows none; a locked row stays out.
    #[test]
    fn the_overlay_panel_shows_the_rows_of_the_active_overlay_only() {
        let mut view = cp_sim::SimRunner::new(1).view();
        let mut stats = (*view.city).clone();
        stats.services = vec![
            row(Sector::Water, 300.0, 28.0, false),
            row(Sector::Sewage, 10.0, 40.0, false),
            row(Sector::Power, 50.0, 50.0, false),
            row(Sector::University, 0.0, 5.0, true),
        ];
        view.city = std::sync::Arc::new(stats);
        let sectors =
            |o| -> Vec<Sector> { overlay_rows(&view, o).iter().map(|r| r.sector).collect() };
        assert_eq!(sectors(Overlay::Water), [Sector::Water, Sector::Sewage]);
        assert_eq!(sectors(Overlay::Power), [Sector::Power]);
        assert!(sectors(Overlay::Education).is_empty(), "locked");
        assert!(sectors(Overlay::Off).is_empty());

        let drawn = |o| -> Vec<String> {
            let ctx = egui::Context::default();
            let mut out = Vec::new();
            for t in [0.0, 0.1] {
                out = frame_texts(&ctx, input(t, vec![]), |ui| {
                    overlay_services_panel(ui.ctx(), &view, o);
                })
                .into_iter()
                .map(|(s, _)| s)
                .collect();
            }
            out
        };
        let on = drawn(Overlay::Water);
        assert!(on.contains(&"300 / 28 units/day".to_string()), "{on:?}");
        assert!(on.contains(&"10 / 40 units/day".to_string()));
        assert!(!on.iter().any(|t| t.starts_with("50 / 50")));
        assert!(drawn(Overlay::Off).is_empty());
    }

    /// BUG-074: the overlay panel sits top-right under the top bar, clear of the palette and
    /// the tool panel (the build menu) at every window size.
    #[test]
    fn the_overlay_panel_is_top_right_clear_of_the_build_menu() {
        let mut view = cp_sim::SimRunner::new(1).view();
        let mut stats = (*view.city).clone();
        stats.services = vec![
            row(Sector::Water, 300.0, 28.0, false),
            row(Sector::Sewage, 10.0, 40.0, false),
        ];
        view.city = std::sync::Arc::new(stats);
        for (w, h) in [(1280.0, 720.0), (1920.0, 1080.0), (2560.0, 1440.0)] {
            let ctx = egui::Context::default();
            for t in [0.0, 0.1, 0.2, 0.3] {
                let mut raw = input(t, vec![]);
                raw.screen_rect = Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(w, h),
                ));
                frame_texts(&ctx, raw, |ui| {
                    overlay_services_panel(ui.ctx(), &view, Overlay::Water);
                });
            }
            let r = ctx
                .memory(|m| m.area_rect(egui::Id::new("overlay_services")))
                .expect("panel drawn");
            assert!(r.top() >= Sizes::TOP_BAR_H, "{w}: {r:?}");
            assert!(r.right() <= w, "{w}: {r:?}");
            assert!(
                r.left() >= Sizes::PALETTE_W + Sizes::TOOL_PANEL_W,
                "{w}: {r:?}"
            );
            assert!(r.bottom() <= h - Sizes::DEMAND_H, "{w}: {r:?}");
        }
    }
}
