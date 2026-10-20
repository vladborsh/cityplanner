//! The milestone window (T-149, T-283): when the city reaches a population milestone the game
//! pauses and a centred window congratulates the player: the status title, the population, what
//! the milestone unlocks (buildings, policies, zones as square icons), the land tiles it adds and
//! the next milestone. One window per milestone reached, in order; when the last is closed the
//! game goes on at speed x1 whatever speed it had. Nothing shows when a save is loaded or
//! a game starts.

use std::collections::VecDeque;

use bevy_egui::egui;
use cp_content::{ContentDb, ZoneType};
use cp_core::time::SimSpeed;
use cp_sim::SimView;
use cp_tools::CommandOutbox;

use crate::icons::zone_icon;
use crate::style::Sizes;
use crate::tile::{TileIcon, tool_tile};

/// A rise of the milestone counts as the city growing, not as a load, when the tick moved on by
/// less than this since the last frame (a progress pass runs every 256 ticks; a load jumps).
const LIVE_TICKS: u64 = 512;

/// What a milestone opens, for the window.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Unlocked {
    /// `(icon, name)` of the buildings.
    pub buildings: Vec<(String, String)>,
    /// `(icon, name)` of the policies.
    pub policies: Vec<(String, String)>,
    pub zones: Vec<ZoneType>,
    /// Land tiles it adds to what the player may buy.
    pub tiles: u8,
}

/// What milestone `m` of the content unlocks.
pub fn unlocked_by(content: &ContentDb, m: usize) -> Unlocked {
    let Some(ms) = content.milestones().get(m) else {
        return Unlocked::default();
    };
    Unlocked {
        buildings: ms
            .unlocks
            .iter()
            .filter_map(|id| content.building_id(id))
            .map(|id| {
                let d = content.building(id);
                (d.icon.clone(), d.name.clone())
            })
            .collect(),
        policies: ms
            .policies
            .iter()
            .filter_map(|&k| content.policy(k))
            .map(|p| (p.icon.clone(), p.name.clone()))
            .collect(),
        zones: ms.zones.clone(),
        tiles: ms.tiles,
    }
}

/// "Next: Town — 3,000 residents", or `None` at the last milestone.
pub fn next_text(content: &ContentDb, m: usize) -> Option<String> {
    let next = content.milestones().get(m + 1)?;
    Some(format!(
        "Next: {} — {} residents",
        next.name,
        crate::buildings::thousands(next.population)
    ))
}

/// Which milestone was last seen, the windows still to show.
#[derive(Default)]
pub struct MilestoneWindow {
    seen: Option<usize>,
    last_tick: Option<u64>,
    queue: VecDeque<usize>,
}

impl MilestoneWindow {
    /// The milestone of the window on screen, if any.
    pub fn current(&self) -> Option<usize> {
        self.queue.front().copied()
    }

    /// Windows waiting, the one on screen included.
    pub fn pending(&self) -> usize {
        self.queue.len()
    }

    /// Compares the view's milestone with the last one seen. A rise while the city grows queues
    /// one window per milestone and pauses the game; the first look (a new game, a load) and a
    /// jump of the clock queue nothing.
    pub fn observe(&mut self, view: &SimView, outbox: &mut CommandOutbox) {
        let Some(content) = view.content.as_deref() else {
            return;
        };
        let list = content.milestones();
        if list.is_empty() {
            return;
        }
        let reached = usize::from(view.progress.reached).min(list.len() - 1);
        let tick = view.tick.0;
        let live = self
            .last_tick
            .is_some_and(|t| tick >= t && tick - t < LIVE_TICKS);
        if let Some(seen) = self.seen
            && reached > seen
            && live
        {
            if self.queue.is_empty() {
                outbox.push(crate::set_speed(SimSpeed::Pause));
            }
            self.queue.extend(seen + 1..=reached);
        }
        self.seen = Some(reached);
        self.last_tick = Some(tick);
    }

    /// Closes the window on screen; after the last one the game always goes on at x1 (the
    /// user's rule, whatever speed it had).
    pub fn close(&mut self, outbox: &mut CommandOutbox) {
        if self.queue.pop_front().is_some() && self.queue.is_empty() {
            outbox.push(crate::set_speed(SimSpeed::X1));
        }
    }
}

/// Observes the view and draws the window of the milestone on screen.
pub fn milestone_window(
    ctx: &egui::Context,
    view: &SimView,
    window: &mut MilestoneWindow,
    outbox: &mut CommandOutbox,
) {
    window.observe(view, outbox);
    let (Some(content), Some(m)) = (view.content.as_deref(), window.current()) else {
        return;
    };
    let ms = &content.milestones()[m];
    let unlocked = unlocked_by(content, m);
    let next = next_text(content, m);
    let mut close = ctx.input(|i| i.key_pressed(egui::Key::Enter));
    let modal = egui::Modal::new(egui::Id::new("milestone_window")).show(ctx, |ui| {
        ui.set_width(460.0);
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new(&ms.name).size(34.0).strong());
            ui.label(format!(
                "{} residents",
                crate::buildings::thousands(ms.population)
            ));
        });
        ui.add_space(8.0);
        egui::ScrollArea::vertical()
            .max_height((ctx.content_rect().height() * 0.62).max(240.0))
            .show(ui, |ui| {
                group(ui, "Buildings", &unlocked.buildings, &[]);
                group(ui, "Policies", &unlocked.policies, &[]);
                group(ui, "Zones", &[], &unlocked.zones);
                if unlocked.tiles > 0 {
                    ui.add_space(4.0);
                    ui.label(format!(
                        "+{} land tile{}",
                        unlocked.tiles,
                        if unlocked.tiles == 1 { "" } else { "s" }
                    ));
                }
            });
        ui.add_space(8.0);
        ui.vertical_centered(|ui| {
            ui.weak(next.as_deref().unwrap_or("The highest milestone"));
            if ui.button("Continue").clicked() {
                close = true;
            }
        });
    });
    if close || modal.should_close() {
        window.close(outbox);
    }
}

/// One group of unlocked items as square tiles under a heading; nothing when it is empty.
fn group(ui: &mut egui::Ui, title: &str, defs: &[(String, String)], zones: &[ZoneType]) {
    if defs.is_empty() && zones.is_empty() {
        return;
    }
    ui.add_space(4.0);
    ui.strong(title);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(Sizes::TILE_GAP, Sizes::TILE_GAP);
        for (icon, name) in defs {
            tool_tile(ui, TileIcon::Def(icon), name, false);
        }
        for &z in zones {
            tool_tile(
                ui,
                TileIcon::Icon(zone_icon(z)),
                cp_tools::zone_name(z),
                false,
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(reached: u8, tick: u64, speed: SimSpeed) -> SimView {
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
            .insert_resource(cp_sim::Progress::at(reached));
        sim.world_mut().insert_resource(cp_core::Tick(tick));
        sim.world_mut().insert_resource(speed);
        sim.view()
    }

    fn speeds(outbox: &CommandOutbox) -> Vec<SimSpeed> {
        outbox
            .iter()
            .filter_map(|c| match c {
                cp_sim::SimCommand::SetSpeed { speed } => Some(*speed),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn two_milestones_at_once_are_two_windows_in_order_and_the_last_resumes_the_game() {
        let mut w = MilestoneWindow::default();
        let mut outbox = CommandOutbox::default();
        w.observe(&view(1, 1000, SimSpeed::X3), &mut outbox);
        assert_eq!(w.pending(), 0, "the first look shows nothing");
        w.observe(&view(3, 1100, SimSpeed::X3), &mut outbox);
        assert_eq!(w.pending(), 2, "Large Village and Small Town");
        assert_eq!(w.current(), Some(2));
        assert_eq!(speeds(&outbox), [SimSpeed::Pause]);
        // More frames while the windows are up queue nothing and pause nothing again.
        w.observe(&view(3, 1100, SimSpeed::Pause), &mut outbox);
        assert_eq!(speeds(&outbox), [SimSpeed::Pause]);
        w.close(&mut outbox);
        assert_eq!(w.current(), Some(3));
        assert_eq!(speeds(&outbox), [SimSpeed::Pause], "not yet");
        w.close(&mut outbox);
        assert_eq!(w.current(), None);
        assert_eq!(
            speeds(&outbox),
            [SimSpeed::Pause, SimSpeed::X1],
            "always x1, not the speed it had"
        );
    }

    #[test]
    fn a_paused_game_resumes_at_x1() {
        let mut w = MilestoneWindow::default();
        let mut outbox = CommandOutbox::default();
        w.observe(&view(0, 1000, SimSpeed::Pause), &mut outbox);
        w.observe(&view(1, 1050, SimSpeed::Pause), &mut outbox);
        w.close(&mut outbox);
        assert_eq!(speeds(&outbox), [SimSpeed::Pause, SimSpeed::X1]);
    }

    #[test]
    fn no_window_after_a_load_or_a_new_game() {
        let mut w = MilestoneWindow::default();
        let mut outbox = CommandOutbox::default();
        // A new game at Hamlet, then a loaded save of Busy Town thousands of ticks away.
        w.observe(&view(0, 0, SimSpeed::X1), &mut outbox);
        w.observe(&view(5, 90_000, SimSpeed::X1), &mut outbox);
        assert_eq!(w.pending(), 0);
        // A save from an earlier tick, then a milestone reached by growing is shown.
        w.observe(&view(2, 500, SimSpeed::X1), &mut outbox);
        assert_eq!(w.pending(), 0);
        w.observe(&view(3, 600, SimSpeed::X1), &mut outbox);
        assert_eq!(w.pending(), 1);
        assert!(speeds(&outbox).contains(&SimSpeed::Pause));
    }

    #[test]
    fn the_window_lists_what_the_milestone_opens_and_what_comes_next() {
        let v = view(0, 0, SimSpeed::X1);
        let c = v.content.as_deref().unwrap();
        let village = unlocked_by(c, 1);
        let names: Vec<_> = village.buildings.iter().map(|b| b.1.as_str()).collect();
        assert!(names.contains(&"Police Station"), "{names:?}");
        assert_eq!(village.policies.len(), 1);
        assert_eq!(village.tiles, 1);
        let busy = unlocked_by(c, 5);
        assert_eq!(
            busy.zones,
            [ZoneType::ResHigh, ZoneType::ComHigh, ZoneType::Office]
        );
        assert_eq!(
            next_text(c, 4).as_deref(),
            Some("Next: Busy Town — 5,000 residents")
        );
        assert_eq!(next_text(c, 11), None);
    }

    #[test]
    fn the_window_draws_and_continue_closes_it() {
        use crate::testutil::{frame, input, key};
        let mut w = MilestoneWindow::default();
        let mut outbox = CommandOutbox::default();
        let ctx = egui::Context::default();
        let mut t = 0.0;
        let mut run = |w: &mut MilestoneWindow, outbox: &mut CommandOutbox, v: &SimView, ev| {
            t += 0.05;
            frame(&ctx, input(t, ev), |ui| {
                milestone_window(ui.ctx(), v, w, outbox)
            });
        };
        let first = view(1, 1000, SimSpeed::X1);
        run(&mut w, &mut outbox, &first, vec![]);
        run(&mut w, &mut outbox, &view(5, 1100, SimSpeed::X1), vec![]);
        assert_eq!(w.pending(), 4);
        // Enter closes the window on screen.
        run(
            &mut w,
            &mut outbox,
            &view(5, 1100, SimSpeed::Pause),
            vec![key(egui::Key::Enter)],
        );
        assert_eq!(w.pending(), 3);
    }
}
