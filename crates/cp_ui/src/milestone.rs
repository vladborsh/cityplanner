//! The milestone notice (T-149): a toast at the top of the screen for a few seconds when the
//! city reaches a population milestone.

use bevy_egui::egui;
use cp_sim::SimView;

/// Seconds the toast stays.
const SHOWN_SECS: f64 = 6.0;

/// Which milestone the toast last saw, and the toast on screen.
#[derive(Default)]
pub struct MilestoneToast {
    seen: Option<usize>,
    text: Option<(String, f64)>,
}

impl MilestoneToast {
    /// Compares the view's milestone with the last one seen; a rise starts the toast at `now`.
    pub fn observe(&mut self, view: &SimView, now: f64) {
        let Some(content) = view.content.as_deref() else {
            return;
        };
        let list = content.milestones();
        if list.is_empty() {
            return;
        }
        let reached = usize::from(view.progress.reached).min(list.len() - 1);
        if let Some(seen) = self.seen
            && reached > seen
        {
            self.text = Some((format!("Milestone reached: {}", list[reached].name), now));
        }
        self.seen = Some(reached);
    }

    /// The text on screen at `now`.
    pub fn text(&self, now: f64) -> Option<&str> {
        self.text
            .as_ref()
            .filter(|(_, at)| now - at < SHOWN_SECS)
            .map(|(t, _)| t.as_str())
    }
}

/// Observes the view and draws the toast.
pub fn milestone_toast(ctx: &egui::Context, view: &SimView, toast: &mut MilestoneToast) {
    let now = ctx.input(|i| i.time);
    toast.observe(view, now);
    let Some(text) = toast.text(now) else {
        return;
    };
    egui::Area::new("milestone_toast".into())
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 64.0))
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.strong(text);
            });
        });
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(reached: u8) -> SimView {
        let content = std::sync::Arc::new(
            cp_content::ContentDb::load_dir(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/content"
            ))
            .unwrap(),
        );
        let mut sim = cp_sim::SimRunner::new(1);
        sim.set_content(content);
        sim.world_mut()
            .insert_resource(cp_sim::Progress { reached });
        sim.view()
    }

    #[test]
    fn a_rise_shows_the_toast_for_a_few_seconds_and_the_first_look_does_not() {
        let mut t = MilestoneToast::default();
        t.observe(&view(0), 0.0);
        assert!(t.text(0.0).is_none());
        t.observe(&view(1), 10.0);
        assert_eq!(t.text(11.0), Some("Milestone reached: Small town"));
        assert!(t.text(17.0).is_none());
        t.observe(&view(1), 18.0);
        assert!(t.text(18.0).is_none());
    }
}
