//! The corruption axis (T-328, `architecture/corruption.md` §8-10): the scale `C − I`, what each
//! edge gives, what each institution brings now, later and at what risk, and the advisor's
//! one-time hints (§10, layer 1). The Power window ([`crate::power`]) draws them.
//!
//! UI only: it reads the [`SimView`]. Which hints the player has seen lives in the player's
//! settings ([`crate::Prefs::hints_seen`]), not in the save, with a button that resets them.

use bevy_egui::egui;
use cp_content::{ContentDb, CorruptionKnobs, InstitutionKind};
use cp_economy::corruption::Edge;
use cp_locale::{tr, trf};
use cp_sim::SimView;

use crate::advisor::{Action, Advice, Severity};
use crate::icons::Icon;
use crate::panels::UiWindow;

/// The ends of the scale: `C − I` runs from −[`SCALE`] (rule of law) to +[`SCALE`] (clan rule).
pub const SCALE: f32 = 100.0;
/// Without the audit the city's place on the scale is a guess this wide either way.
pub const GUESS: f32 = 15.0;
/// The hint "the axis nears the edge" comes this close to the clan edge.
pub const NEAR_EDGE: f32 = 10.0;
/// Corruption from which "rumours are everywhere" and its hint comes (as [`crate::offers::rumour`]).
pub const HINT_CORRUPTION: f32 = 30.0;

pub const CLAN: egui::Color32 = egui::Color32::from_rgb(184, 74, 60);
pub const LAW: egui::Color32 = egui::Color32::from_rgb(63, 127, 196);
pub const MIDDLE: egui::Color32 = egui::Color32::from_rgb(110, 110, 118);

pub fn pct(f: f32) -> String {
    format!("{:.0}", f * 100.0)
}

pub fn num(f: f32) -> String {
    format!("{f:.0}")
}

/// Where the city stands, in words.
pub fn edge_text(edge: Edge) -> String {
    match edge {
        Edge::Clan => tr("The city is under clan rule."),
        Edge::Middle => tr("The city is in the grey middle."),
        Edge::Law => tr("The city is under the rule of law."),
    }
}

/// What the clan edge gives and takes (§8), with the knobs' numbers.
pub fn clan_lines(k: &CorruptionKnobs) -> [String; 2] {
    [
        trf!(
            "Plus: offers come {x} times as often and pay {m} times as much",
            x = num(k.clan_offers),
            m = format!("{:.1}", k.clan_money)
        ),
        trf!(
            "Minus: trust climbs no higher than {n}; crime grows {pct}% more",
            n = num(k.trust_ceiling_clan),
            pct = pct(k.clan_crime)
        ),
    ]
}

/// What the rule of law gives and takes (§8).
pub fn law_lines(k: &CorruptionKnobs) -> [String; 2] {
    [
        trf!(
            "Plus: taxes +{tax}%; land worth +{land}%",
            tax = pct(k.law_tax),
            land = pct(k.law_land)
        ),
        trf!(
            "Minus: offers hardly come; institutions cost {pct}% more upkeep",
            pct = pct(k.law_upkeep)
        ),
    ]
}

/// The name of an institution (its building's name in the content, translated).
pub fn institution_name(db: &ContentDb, kind: InstitutionKind) -> String {
    db.buildings()
        .find(|(_, d)| d.institution == Some(kind))
        .map_or_else(|| format!("{kind:?}"), |(_, d)| tr(&d.name))
}

/// The institutions `I` an institution adds.
pub fn institution_gain(k: &CorruptionKnobs, kind: InstitutionKind) -> f32 {
    match kind {
        InstitutionKind::Audit => k.i_audit,
        InstitutionKind::Press => k.i_press,
        InstitutionKind::Court => k.i_court,
    }
}

/// How much more often the player's own deals come out with an institution.
pub fn institution_exposure(k: &CorruptionKnobs, kind: InstitutionKind) -> f32 {
    match kind {
        InstitutionKind::Audit => k.exposure_audit,
        InstitutionKind::Press => k.exposure_press,
        InstitutionKind::Court => k.exposure_court,
    }
}

/// "Now · later · risk" of an institution (§10, layer 2).
pub fn institution_terms(k: &CorruptionKnobs, kind: InstitutionKind) -> [String; 3] {
    let (i, exposure, now, later) = match kind {
        InstitutionKind::Audit => (
            k.i_audit,
            k.exposure_audit,
            tr("the exact corruption and the bought police stations"),
            tr("its upkeep"),
        ),
        InstitutionKind::Press => (
            k.i_press,
            k.exposure_press,
            trf!(
                "trust recovers {x} times faster; a declined deal earns trust +{n}",
                x = num(k.press_trust_recover),
                n = num(k.press_decline_trust)
            ),
            tr("garbage, fires and crime it reports cost trust every day"),
        ),
        InstitutionKind::Court => (
            k.i_court,
            k.exposure_court,
            trf!(
                "crime grows {pct}% less; bought stations are freed",
                pct = pct(k.court_police)
            ),
            tr("corruption falls faster, so deals pay off less long"),
        ),
    };
    [
        trf!("Now: institutions +{i} · {what}", i = num(i), what = now),
        trf!("Later: {what}", what = later),
        trf!(
            "Risk: your own deals come out {pct}% more often",
            pct = pct(exposure)
        ),
    ]
}

/// "Now · later · risk" of the police budget (§10, layer 2): a station is easier to buy below
/// 100 %.
pub fn police_terms(k: &CorruptionKnobs, budget: u8) -> String {
    let low = (100.0 - f32::from(budget)).max(0.0) / 100.0;
    let x = 1.0 + k.capture_budget * low;
    trf!(
        "Police budget {pct}%: risk a station is bought ×{x}",
        pct = budget.to_string(),
        x = format!("{x:.1}")
    )
}

/// Whether the institution's building is open to build.
pub fn unlocked(view: &SimView, db: &ContentDb, kind: InstitutionKind) -> bool {
    view.uniques
        .iter()
        .any(|u| u.unlocked && db.building(u.def).institution == Some(kind))
}

fn hint(id: &'static str, icon: Icon, text: String, window: UiWindow) -> Advice {
    Advice {
        id,
        severity: Severity::Info,
        text,
        count: 1,
        icon,
        action: Some(Action::Window(window)),
    }
}

/// The hints (§10, layer 1) whose trigger holds now, in the order they would come; the advisor
/// shows each once (the ones in the player's `hints_seen` never again).
pub fn hints(view: &SimView) -> Vec<Advice> {
    let Some(db) = view.content.as_deref() else {
        return Vec::new();
    };
    let k = &db.balance().corruption;
    let c = &view.corruption;
    let mut out = Vec::new();
    if view.city.population as f32 >= k.deals_population {
        out.push(hint(
            "hint_deals",
            Icon::WindowPower,
            tr("Grey deals now come to the Power window: money now, corruption and evidence later."),
            UiWindow::Offers,
        ));
    }
    if !c.active.is_empty() || !c.evidence.is_empty() {
        out.push(hint(
            "hint_first_deal",
            Icon::WindowPower,
            tr("A deal taken: corruption rose and its evidence stays; one day it may come out as a scandal."),
            UiWindow::Power,
        ));
    }
    if !c.scandals.is_empty() {
        out.push(hint(
            "hint_scandal",
            Icon::Scandal,
            tr("A scandal broke: trust fell and the city paid a fine. Institutions make scandals likelier."),
            UiWindow::Scandals,
        ));
    }
    if c.corruption >= HINT_CORRUPTION {
        out.push(hint(
            "hint_corruption",
            Icon::WindowPower,
            tr("Rumours of bribes are everywhere: police stations start to be bought and crime rises."),
            UiWindow::Power,
        ));
    }
    if !c.visible_captured().is_empty() {
        out.push(hint(
            "hint_captured",
            Icon::OverlayPolice,
            tr("The audit found police stations on the take: they fight crime less. A court frees them."),
            UiWindow::Power,
        ));
    }
    let a = c.axis();
    if a > k.edge - NEAR_EDGE && a <= k.edge {
        let mean = {
            let deals = db.deals();
            let sum: f32 = deals.iter().map(|d| d.corruption).sum();
            (sum / deals.len().max(1) as f32).max(1.0)
        };
        let n = ((k.edge - a) / mean).ceil().max(1.0) as i64;
        out.push(hint(
            "hint_near_clan",
            Icon::WindowPower,
            trf!(
                plural "About {n} more deal and the city slides to clan rule: more offers, less trust",
                n = n
            ),
            UiWindow::Power,
        ));
    }
    match c.edge(k) {
        Edge::Clan => out.push(hint(
            "hint_clan",
            Icon::WindowPower,
            tr("The city is under clan rule: deals come twice as often, trust stays low, crime grows."),
            UiWindow::Power,
        )),
        Edge::Law => out.push(hint(
            "hint_law",
            Icon::WindowPower,
            tr("The city is under the rule of law: more tax and dearer land, hardly any offers."),
            UiWindow::Power,
        )),
        Edge::Middle => {}
    }
    for (kind, id) in [
        (InstitutionKind::Audit, "hint_audit"),
        (InstitutionKind::Press, "hint_press"),
        (InstitutionKind::Court, "hint_court"),
    ] {
        if unlocked(view, db, kind) {
            out.push(hint(
                id,
                Icon::Buildings,
                trf!(
                    "{name} can be built: institutions +{i}, deals come out more often.",
                    name = institution_name(db, kind),
                    i = num(institution_gain(k, kind))
                ),
                UiWindow::Institutions,
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_economy::CorruptionState;
    use std::sync::Arc;

    fn view_with(f: impl FnOnce(&mut CorruptionState)) -> SimView {
        let db = Arc::new(
            ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
                .unwrap(),
        );
        let mut sim = cp_sim::SimRunner::new(1);
        sim.set_content(db);
        f(&mut sim.world_mut().resource_mut::<CorruptionState>());
        sim.view()
    }

    const INSTITUTIONS: [&str; 3] = ["hint_audit", "hint_press", "hint_court"];

    /// The hints but the institutions' (a test city has every unique building open).
    fn ids(view: &SimView) -> Vec<&'static str> {
        hints(view)
            .iter()
            .map(|a| a.id)
            .filter(|id| !INSTITUTIONS.contains(id))
            .collect()
    }

    #[test]
    fn each_hint_comes_with_its_trigger() {
        let new = view_with(|_| {});
        assert!(ids(&new).is_empty(), "a new city has no axis hints");
        let all: Vec<_> = hints(&new).iter().map(|a| a.id).collect();
        assert_eq!(all, INSTITUTIONS, "each open institution has its hint");
        let v = view_with(|c| {
            c.corruption = 35.0;
            c.institutions = 0.0;
            c.scandals.push(cp_economy::Scandal {
                deal: "land".into(),
                day: 1,
                weight: 1.0,
                trust: 1.0,
                fine: 1,
                undone: false,
            });
            c.evidence.push(cp_economy::Evidence {
                deal: "land".into(),
                accepted_day: 1,
                weight: 1.0,
            });
        });
        assert_eq!(
            ids(&v),
            [
                "hint_first_deal",
                "hint_scandal",
                "hint_corruption",
                "hint_near_clan"
            ]
        );
        let v = view_with(|c| {
            c.corruption = 50.0;
            c.present = InstitutionKind::Audit.bit();
            c.captured.push(cp_core::Id::new(2, 0));
        });
        assert_eq!(ids(&v), ["hint_corruption", "hint_captured", "hint_clan"]);
        let v = view_with(|c| c.institutions = 45.0);
        assert_eq!(ids(&v), ["hint_law"]);
    }

    #[test]
    fn the_near_edge_hint_counts_the_deals_left() {
        let v = view_with(|c| c.corruption = 39.0);
        let near = hints(&v)
            .into_iter()
            .find(|a| a.id == "hint_near_clan")
            .unwrap();
        assert_eq!(
            near.text,
            "About 1 more deal and the city slides to clan rule: more offers, less trust"
        );
    }

    #[test]
    fn the_terms_carry_the_knobs() {
        let k = CorruptionKnobs::default();
        assert_eq!(
            institution_terms(&k, InstitutionKind::Court),
            [
                "Now: institutions +30 · crime grows 20% less; bought stations are freed",
                "Later: corruption falls faster, so deals pay off less long",
                "Risk: your own deals come out 50% more often",
            ]
        );
        assert_eq!(
            police_terms(&k, 100),
            "Police budget 100%: risk a station is bought ×1.0"
        );
        assert_eq!(law_lines(&k)[0], "Plus: taxes +10%; land worth +5%");
    }
}
