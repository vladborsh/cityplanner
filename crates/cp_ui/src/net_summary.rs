//! The network summary of the Water and Power tool panels (§13.6): per network, how many
//! sources work, what they can give, what the buildings use and how many buildings lack the
//! supply, read from `CityStats.utilities`. Read-only.

use bevy_egui::egui;
use cp_content::UtilityKind;
use cp_sim::{CityStats, SimView};

/// Which networks a section of the Buildings panel summarises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetTab {
    /// Water and sewage (one pipe network).
    Water,
    Power,
}

/// One network's line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NetRow {
    pub name: &'static str,
    pub sources: u32,
    pub capacity: f32,
    pub consumption: f32,
    /// Active buildings that lack this supply.
    pub unsupplied: u32,
}

impl NetRow {
    /// Demand beyond what the sources give: shown as a warning.
    pub fn overloaded(&self) -> bool {
        self.consumption > self.capacity
    }
}

/// The networks of a panel: water and sewage on the Water panel, power on the Power panel.
pub fn rows(stats: &CityStats, tab: NetTab) -> Vec<NetRow> {
    let kinds: &[(UtilityKind, &'static str)] = match tab {
        NetTab::Power => &[(UtilityKind::Power, "Power")],
        _ => &[
            (UtilityKind::Water, "Water"),
            (UtilityKind::Sewage, "Sewage"),
        ],
    };
    kinds
        .iter()
        .map(|&(kind, name)| {
            let n = &stats.utilities[CityStats::utility_index(kind)];
            NetRow {
                name,
                sources: n.sources,
                capacity: n.capacity,
                consumption: n.consumption,
                unsupplied: n.unsupplied,
            }
        })
        .collect()
}

/// The text of a row, as drawn.
pub fn row_text(r: &NetRow) -> String {
    format!(
        "{}: {} source{} · capacity {:.0} · use {:.0} · {} without supply",
        r.name,
        r.sources,
        if r.sources == 1 { "" } else { "s" },
        r.capacity,
        r.consumption,
        r.unsupplied
    )
}

/// Draws the summary; a network whose use exceeds its capacity, or with buildings lacking the
/// supply, is coloured as a warning (red for an overload, amber for missing supply).
pub fn network_summary(ui: &mut egui::Ui, view: &SimView, tab: NetTab) {
    for r in rows(&view.city, tab) {
        let text = row_text(&r);
        if r.overloaded() {
            ui.colored_label(egui::Color32::LIGHT_RED, format!("⚠ {text}"));
        } else if r.unsupplied > 0 {
            ui.colored_label(egui::Color32::from_rgb(240, 190, 60), format!("⚠ {text}"));
        } else {
            ui.label(text);
        }
    }
    if view.city.generation == 0 {
        ui.weak("The summary appears with the next count (every 64 ticks).");
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use super::*;
    use crate::testutil::{frame_texts, input};
    use cp_sim::scenario::{GridCityConfig, grid_city};
    use cp_sim::stats::NetStats;

    fn drawn(view: &SimView, tab: NetTab) -> Vec<String> {
        let ctx = egui::Context::default();
        frame_texts(&ctx, input(0.0, vec![]), |ui| {
            network_summary(ui, view, tab);
        })
        .into_iter()
        .map(|(t, _)| t)
        .collect()
    }

    fn with_stats(f: impl FnOnce(&mut CityStats)) -> SimView {
        let mut view = cp_sim::SimRunner::new(1).view();
        let mut s = CityStats::default();
        s.generation = 1;
        f(&mut s);
        view.city = Arc::new(s);
        view
    }

    #[test]
    fn a_city_with_a_pump_and_one_without_show_their_numbers() {
        let net = |sources, capacity, consumption, unsupplied| NetStats {
            sources,
            capacity,
            consumption,
            unsupplied,
        };
        // With a pump and an outflow, the demand within capacity.
        let with = with_stats(|s| {
            s.utilities[0] = net(1, 1000.0, 400.0, 0);
            s.utilities[1] = net(1, 1000.0, 300.0, 0);
        });
        let texts = drawn(&with, NetTab::Water);
        assert!(texts.contains(&row_text(&rows(&with.city, NetTab::Water)[0])));
        assert!(texts.iter().any(|t| t.contains("Water: 1 source · capacity 1000 · use 400 · 0 without supply")));
        assert!(texts.iter().any(|t| t.starts_with("Sewage: 1 source")));
        assert!(
            rows(&with.city, NetTab::Water)
                .iter()
                .all(|r| !r.overloaded())
        );

        // Without any pump: no source, 25 buildings dry, use beyond (zero) capacity: warning.
        let without = with_stats(|s| s.utilities[0] = net(0, 0.0, 250.0, 25));
        let r = rows(&without.city, NetTab::Water);
        assert!(r[0].overloaded() && r[0].unsupplied == 25 && r[0].sources == 0);
        let texts = drawn(&without, NetTab::Water);
        assert!(
            texts
                .iter()
                .any(|t| t.contains("Water: 0 sources · capacity 0 · use 250 · 25 without supply"))
        );

        // The Power panel shows power only.
        let power = with_stats(|s| s.utilities[2] = net(2, 600.0, 100.0, 0));
        let texts = drawn(&power, NetTab::Power);
        assert!(texts.iter().any(|t| t.starts_with("Power: 2 sources")));
        assert!(!texts.iter().any(|t| t.starts_with("Water")));
        // Before the first count: a hint, not a wall of zeros.
        let fresh = with_stats(|s| s.generation = 0);
        assert!(
            drawn(&fresh, NetTab::Power)
                .iter()
                .any(|t| t.contains("next count"))
        );
    }

    /// A real city on flatlands (water towers and wind turbines): the rows are exactly
    /// `CityStats.utilities`.
    #[test]
    fn the_summary_of_a_real_city_is_city_stats() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let terrain =
            Arc::new(cp_terrain::Terrain::load(&root.join("assets/maps/flatlands")).unwrap());
        let content =
            Arc::new(cp_content::ContentDb::load_dir(root.join("assets/content")).unwrap());
        let (mut sim, _) = grid_city(400, 3, terrain, content, &GridCityConfig::default());
        while sim.tick().0 % 64 != 5 {
            sim.tick_once();
        }
        let view = sim.view();
        let stats = &view.city;
        assert!(stats.generation > 0);
        let water = &rows(stats, NetTab::Water);
        assert_eq!(water[0].unsupplied, stats.utilities[0].unsupplied);
        assert!(water[0].sources > 0, "the scenario places water towers");
        assert_eq!(water[0].unsupplied, 0, "and the buildings are supplied");
        let power = &rows(stats, NetTab::Power);
        assert!(power[0].sources > 0 && power[0].capacity > 0.0);
        assert_eq!(power[0].consumption, stats.utilities[2].consumption);
        let texts = drawn(&view, NetTab::Power);
        // A warning line carries a leading sign.
        assert!(
            texts.iter().any(|t| t.ends_with(&row_text(&power[0]))),
            "{texts:?}"
        );
    }
}
