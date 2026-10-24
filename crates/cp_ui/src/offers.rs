//! Offers (T-325, `architecture/corruption.md` §4, §10): the grey deals waiting in the inbox. A
//! card per offer says what it brings now, what it costs in corruption and evidence, what it is
//! about and how long it waits; Accept and Decline send `SimCommand::Corruption`. The window
//! button in the top bar shows how many wait. Below them, the scandals that broke (T-326, §6),
//! newest first.

use bevy_egui::egui;
use cp_content::{ContentDb, DealDef};
use cp_core::Cost;
use cp_economy::{CorruptionCommand, DealTarget, Offer, Scandal};
use cp_locale::{tr, trf, trn};
use cp_sim::{SimCommand, SimView};
use cp_tools::{CommandOutbox, format_money};

use crate::icons::Icon;

/// The weight of the evidence in words.
pub fn evidence_text(weight: f32) -> String {
    if weight <= 0.0 {
        tr("none")
    } else if weight < 2.0 {
        tr("a little")
    } else if weight < 4.0 {
        tr("some")
    } else {
        tr("a lot")
    }
}

/// "Now · later · risk" of a deal (§10, layer 2), one line each.
pub fn terms(def: &DealDef) -> Vec<String> {
    let mut now = Vec::new();
    if def.money != 0 {
        now.push(trf!(
            "+{money} now",
            money = format_money(Cost::from_units(def.money as f32))
        ));
    }
    if def.weekly != 0 {
        now.push(trf!(
            "+{money}/wk",
            money = format_money(Cost::from_units(def.weekly as f32))
        ));
    }
    if def.build_discount > 0.0 {
        now.push(trf!(
            "building {pct}% cheaper",
            pct = (def.build_discount * 100.0).round() as i32
        ));
    }
    if def.lift_policy {
        now.push(tr("the district's ban is lifted"));
    }
    if def.tourists > 0.0 {
        now.push(trf!(
            "{pct}% more tourists",
            pct = (def.tourists * 100.0).round() as i32
        ));
    }
    if def.trust > 0.0 {
        now.push(trf!("trust +{n}", n = format!("{:.0}", def.trust)));
    }
    let mut later = Vec::new();
    if def.trust < 0.0 {
        later.push(trf!("trust {n}", n = format!("{:.0}", def.trust)));
    }
    if def.pollution > 0.0 {
        later.push(trf!(
            "{pct}% more pollution from it",
            pct = (def.pollution * 100.0).round() as i32
        ));
    }
    if def.crime > 0.0 {
        later.push(trf!(
            "{pct}% more crime around it",
            pct = (def.crime * 100.0).round() as i32
        ));
    }
    let mut out = Vec::new();
    if !now.is_empty() {
        out.push(trf!("Now: {what}", what = now.join(" · ")));
    }
    if def.days > 0 {
        out.push(trn("For {n} days", i64::from(def.days)));
    }
    if !later.is_empty() {
        out.push(trf!("Later: {what}", what = later.join(" · ")));
    }
    out.push(trf!(
        "Risk: corruption +{c} · evidence: {e}",
        c = format!("{:.0}", def.corruption),
        e = evidence_text(def.evidence)
    ));
    out
}

/// What the offer is about, if anything.
fn target_text(view: &SimView, db: &ContentDb, offer: &Offer) -> Option<String> {
    match offer.target? {
        DealTarget::District(d) => {
            let name = view.districts.get(d)?.name.clone();
            Some(trf!("In the district {name}", name = name))
        }
        DealTarget::Building(b) => {
            let def = db.building(view.buildings.get(b)?.def);
            Some(trf!("At {name}", name = tr(&def.name)))
        }
    }
}

/// The Offers window.
pub fn offers_window(
    ctx: &egui::Context,
    open: &mut bool,
    view: &SimView,
    outbox: &mut CommandOutbox,
) {
    crate::frame::window("offers", Icon::OverlayMail, &tr("Offers"), open).show(ctx, |ui| {
        let Some(db) = view.content.as_deref() else {
            return;
        };
        let k = &db.balance().corruption;
        let c = &view.corruption;
        ui.label(trf!(
            "Trust in the mayor: {n}",
            n = format!("{:.0}", c.trust_now(k))
        ));
        if c.inbox.is_empty() {
            ui.weak(trf!(
                "No offers. Grey deals start to come at {n} residents; the game does not pause for them.",
                n = format!("{:.0}", k.deals_population)
            ));
        }
        let today = cp_economy::corruption::day_of(view.tick);
        for offer in &c.inbox {
            let Some(def) = db.deal(&offer.deal) else {
                continue;
            };
            ui.separator();
            ui.strong(tr(&def.name));
            ui.label(tr(&def.text));
            if let Some(t) = target_text(view, db, offer) {
                ui.weak(t);
            }
            for line in terms(def) {
                ui.label(line);
            }
            ui.weak(trn(
                "Answer within {n} days",
                i64::from(offer.expires_day.saturating_sub(today)),
            ));
            ui.horizontal(|ui| {
                if ui.button(tr("Accept")).clicked() {
                    outbox.push(SimCommand::Corruption(CorruptionCommand::Accept {
                        deal: offer.deal.clone(),
                    }));
                }
                if ui.button(tr("Decline")).clicked() {
                    outbox.push(SimCommand::Corruption(CorruptionCommand::Decline {
                        deal: offer.deal.clone(),
                    }));
                }
            });
        }
        if !c.scandals.is_empty() {
            ui.separator();
            ui.strong(tr("Scandals"));
            for s in c.scandals.iter().rev().take(SCANDALS_SHOWN) {
                ui.label(scandal_text(db, s));
            }
        }
    });
}

/// Scandals listed in the window, newest first.
pub const SCANDALS_SHOWN: usize = 5;

/// One line about a scandal that broke.
pub fn scandal_text(db: &ContentDb, s: &Scandal) -> String {
    let deal = db
        .deal(&s.deal)
        .map_or_else(|| s.deal.clone(), |d| tr(&d.name));
    let line = trf!(
        "Day {day}: {deal} came out · trust -{n} · fine {money}",
        day = s.day.to_string(),
        deal = deal,
        n = format!("{:.0}", s.trust),
        money = format_money(Cost::from_units(s.fine as f32))
    );
    if s.undone {
        format!("{line} · {}", tr("the deal is undone"))
    } else {
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn db() -> Arc<ContentDb> {
        Arc::new(
            ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
                .unwrap(),
        )
    }

    #[test]
    fn a_card_says_now_later_and_risk() {
        let db = db();
        let casino = terms(db.deal("casino").unwrap());
        assert!(casino[0].starts_with("Now: +"), "{casino:?}");
        assert!(casino[0].contains("15% more tourists"), "{casino:?}");
        assert!(casino.iter().any(|l| l.contains("more crime around it")));
        assert_eq!(
            casino.last().unwrap(),
            "Risk: corruption +6 · evidence: some"
        );
        let kickback = terms(db.deal("kickback").unwrap());
        assert!(
            kickback.contains(&"For 30 days".to_string()),
            "{kickback:?}"
        );
    }

    #[test]
    fn a_scandal_line_names_the_deal_and_its_cost() {
        let db = db();
        let s = Scandal {
            deal: "land".into(),
            day: 12,
            weight: 4.0,
            trust: 12.0,
            fine: 20000,
            undone: true,
        };
        let line = scandal_text(&db, &s);
        assert!(
            line.starts_with("Day 12: Land for friends came out · trust -12 · fine "),
            "{line}"
        );
        assert!(line.ends_with(" · the deal is undone"), "{line}");
    }

    #[test]
    fn accept_sends_the_corruption_command() {
        let mut sim = cp_sim::SimRunner::new(1);
        sim.set_content(db());
        sim.world_mut()
            .resource_mut::<cp_economy::CorruptionState>()
            .inbox
            .push(Offer {
                deal: "land".into(),
                target: None,
                expires_day: 5,
            });
        let view = sim.view();
        let ctx = egui::Context::default();
        let mut outbox = CommandOutbox::default();
        let mut open = true;
        let mut draw = |outbox: &mut CommandOutbox, ev| {
            crate::testutil::frame_texts(&ctx, crate::testutil::input(0.0, ev), |ui| {
                offers_window(ui.ctx(), &mut open, &view, outbox)
            })
        };
        // Twice: a window lays itself out on its first frame.
        draw(&mut outbox, vec![]);
        let texts = draw(&mut outbox, vec![]);
        assert!(
            texts.iter().any(|(t, _)| t == "Land for friends"),
            "{texts:?}"
        );
        let at = texts
            .iter()
            .find(|(t, _)| t == "Accept")
            .map(|(_, p)| *p + egui::vec2(4.0, 4.0))
            .expect("an Accept button");
        let [down, up] = crate::testutil::click(at);
        draw(&mut outbox, down);
        draw(&mut outbox, up);
        assert_eq!(
            outbox.iter().cloned().collect::<Vec<_>>(),
            [SimCommand::Corruption(CorruptionCommand::Accept {
                deal: "land".into()
            })]
        );
    }
}
