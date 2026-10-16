//! The frame every window of the UI shares (T-082): an icon, a title and a close button in
//! one header row, then the content. Windows with the same look are drawn through
//! [`window`], so the look changes in one place.

use bevy_egui::egui;

use crate::icons::Icon;
use crate::style::Sizes;

/// A window under construction; [`Win::show`] draws it.
pub struct Win<'a> {
    id: &'a str,
    icon: Icon,
    title: &'a str,
    open: &'a mut bool,
    anchor: Option<(egui::Align2, egui::Vec2)>,
}

/// A window with `icon` and `title`, shown while `*open`; the header's close button clears it.
pub fn window<'a>(id: &'a str, icon: Icon, title: &'a str, open: &'a mut bool) -> Win<'a> {
    Win {
        id,
        icon,
        title,
        open,
        anchor: None,
    }
}

impl Win<'_> {
    /// Pins the window to a corner (the player cannot drag it away).
    pub fn anchor(mut self, align: egui::Align2, offset: egui::Vec2) -> Self {
        self.anchor = Some((align, offset));
        self
    }

    /// Draws the window when open and returns what `add_contents` returned.
    pub fn show<R>(
        self,
        ctx: &egui::Context,
        add_contents: impl FnOnce(&mut egui::Ui) -> R,
    ) -> Option<R> {
        if !*self.open {
            return None;
        }
        let mut close = false;
        let mut win = egui::Window::new(self.id)
            .id(egui::Id::new(("cp_window", self.id)))
            .title_bar(false)
            .resizable(false);
        if let Some((align, offset)) = self.anchor {
            win = win.anchor(align, offset);
        }
        let out = win.show(ctx, |ui| {
            ui.horizontal(|ui| {
                self.icon.show(ui, Sizes::WINDOW_ICON);
                ui.strong(self.title);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("✖").on_hover_text("Close").clicked() {
                        close = true;
                    }
                });
            });
            ui.separator();
            add_contents(ui)
        });
        if close {
            *self.open = false;
        }
        out.and_then(|o| o.inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(t: f64, events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            time: Some(t),
            events,
            ..Default::default()
        }
    }

    #[test]
    fn a_closed_window_draws_nothing_and_an_open_one_runs_its_content() {
        let ctx = egui::Context::default();
        let mut open = false;
        let mut ran = 0;
        crate::testutil::frame(&ctx, input(0.0, vec![]), |ui| {
            window("w", Icon::Info, "Info", &mut open).show(ui.ctx(), |_| ran += 1);
        });
        assert_eq!(ran, 0);
        open = true;
        for t in [0.1, 0.2] {
            crate::testutil::frame(&ctx, input(t, vec![]), |ui| {
                window("w", Icon::Info, "Info", &mut open).show(ui.ctx(), |_| ran += 1);
            });
        }
        assert_eq!(ran, 2);
        assert!(open);
    }

    #[test]
    fn the_close_button_closes_the_window() {
        let ctx = egui::Context::default();
        let mut open = true;
        let mut bounds = egui::Rect::NOTHING;
        let draw = |ctx: &egui::Context, t, events, open: &mut bool, bounds: &mut egui::Rect| {
            crate::testutil::frame(ctx, input(t, events), |ui| {
                window("w", Icon::Info, "Info", open)
                    .anchor(egui::Align2::LEFT_TOP, egui::vec2(10.0, 10.0))
                    .show(ui.ctx(), |ui| {
                        ui.label("content");
                    });
                *bounds = ui.ctx().globally_used_rect();
            });
        };
        draw(&ctx, 0.0, vec![], &mut open, &mut bounds);
        draw(&ctx, 0.05, vec![], &mut open, &mut bounds);
        // The close button is at the right end of the header row.
        let pos = egui::pos2(
            bounds.right() - 14.0,
            bounds.top() + 10.0 + Sizes::WINDOW_ICON * 0.5,
        );
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        draw(
            &ctx,
            0.1,
            vec![egui::Event::PointerMoved(pos), button(true)],
            &mut open,
            &mut bounds,
        );
        draw(&ctx, 0.2, vec![button(false)], &mut open, &mut bounds);
        assert!(!open);
    }
}
