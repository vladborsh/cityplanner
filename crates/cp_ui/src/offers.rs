//! The words and numbers of the grey deals (T-325, `architecture/corruption.md` §4, §10) and
//! what the mayor knows of the city (T-327, §9). The Power window ([`crate::power`]) draws them:
//! the chips of a deal, the lines of its tooltip, the rumours without an audit chamber.

use cp_content::{ContentDb, DealDef};
use cp_core::Cost;
use cp_economy::{CorruptionState, DealTarget, Offer, Scandal};
use cp_locale::{tr, trf, trn};
use cp_sim::SimView;
use cp_tools::format_money;

use crate::icons::Icon;

/// One effect of a deal as a chip: an icon and a few characters. `good`: a gain (green),
/// else a price (orange).
#[derive(Clone, Debug, PartialEq)]
pub struct Chip {
    pub icon: Icon,
    pub text: String,
    pub good: bool,
}

fn chip(icon: Icon, text: String, good: bool) -> Chip {
    Chip { icon, text, good }
}

/// `20000` as "$20k", `1500` as "$1.5k", `800` as "$800".
pub fn short_money(units: i64) -> String {
    let sign = if units < 0 { "-" } else { "+" };
    let n = units.unsigned_abs();
    let body = if n >= 1000 && n.is_multiple_of(1000) {
        format!("{}k", n / 1000)
    } else if n >= 1000 {
        format!("{:.1}k", n as f32 / 1000.0)
    } else {
        n.to_string()
    };
    format!("{sign}${body}")
}

/// The evidence of weight `w` as three dots: `●●○`.
pub fn evidence_dots(w: f32) -> String {
    let n = if w <= 0.0 {
        0
    } else if w < 2.0 {
        1
    } else if w < 4.0 {
        2
    } else {
        3
    };
    format!("{}{}", "●".repeat(n), "○".repeat(3 - n))
}

/// The chips of a deal (T-335): the gains first, then the price.
pub fn chips(def: &DealDef) -> Vec<Chip> {
    let mut out = Vec::new();
    if def.money != 0 {
        out.push(chip(Icon::DealMoney, short_money(def.money), true));
    }
    if def.weekly != 0 {
        out.push(chip(
            Icon::DealMoney,
            trf!("{money}/wk", money = short_money(def.weekly)),
            true,
        ));
    }
    if def.build_discount > 0.0 {
        out.push(chip(
            Icon::DealBuild,
            format!("-{}%", (def.build_discount * 100.0).round() as i32),
            true,
        ));
    }
    if def.days > 0 {
        out.push(chip(
            Icon::Deadline,
            trf!("{n} d", n = def.days.to_string()),
            true,
        ));
    }
    if def.lift_policy {
        out.push(chip(Icon::DealBan, tr("ban lifted"), true));
    }
    if def.tourists > 0.0 {
        out.push(chip(
            Icon::DealTourists,
            format!("+{}%", (def.tourists * 100.0).round() as i32),
            true,
        ));
    }
    if def.trust > 0.0 {
        out.push(chip(Icon::Trust, format!("+{:.0}", def.trust), true));
    }
    out.push(chip(
        Icon::Corruption,
        format!("C +{:.0}", def.corruption),
        false,
    ));
    out.push(chip(Icon::Evidence, evidence_dots(def.evidence), false));
    if def.trust < 0.0 {
        out.push(chip(Icon::Trust, format!("{:.0}", def.trust), false));
    }
    if def.pollution > 0.0 {
        out.push(chip(
            Icon::OverlayPollution,
            format!("+{}%", (def.pollution * 100.0).round() as i32),
            false,
        ));
    }
    if def.crime > 0.0 {
        out.push(chip(
            Icon::DealCrime,
            format!("+{}%", (def.crime * 100.0).round() as i32),
            false,
        ));
    }
    out
}

/// The icon of the card of a deal, by the effect it is about (no field of the def is needed).
pub fn deal_icon(def: &DealDef) -> Icon {
    if def.build_discount > 0.0 {
        Icon::DealBuild
    } else if def.lift_policy {
        Icon::DealBan
    } else if def.pollution > 0.0 {
        Icon::OverlayPollution
    } else if def.tourists > 0.0 {
        Icon::DealTourists
    } else {
        Icon::DealMoney
    }
}

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
pub fn target_text(view: &SimView, db: &ContentDb, offer: &Offer) -> Option<String> {
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

/// What the mayor knows of the city's corruption (§9): institutions always, corruption and the
/// captured stations exactly with the audit, as rumours without.
pub fn standing(c: &CorruptionState) -> Vec<String> {
    let mut out = vec![trf!(
        "Institutions: {n}",
        n = format!("{:.0}", c.institutions)
    )];
    if c.sees_exactly() {
        out.push(trf!("Corruption: {n}", n = format!("{:.0}", c.corruption)));
        out.push(trf!(
            "Police stations on the take: {n}",
            n = c.visible_captured().len().to_string()
        ));
    } else {
        out.push(rumour(c.corruption));
        out.push(tr("An audit chamber would give the exact numbers."));
    }
    out
}

/// Corruption `c` as the street tells it.
pub fn rumour(c: f32) -> String {
    if c < 10.0 {
        tr("Nobody talks of bribes.")
    } else if c < 30.0 {
        tr("There is talk of bribes.")
    } else if c < 60.0 {
        tr("Rumours of bribes are everywhere.")
    } else {
        tr("Everyone says the city is bought.")
    }
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
    fn chips_split_a_deal_into_gains_and_prices() {
        let db = db();
        let casino = chips(db.deal("casino").unwrap());
        let has = |icon, good| casino.iter().any(|c| c.icon == icon && c.good == good);
        assert!(has(Icon::DealTourists, true), "{casino:?}");
        assert!(has(Icon::DealCrime, false), "{casino:?}");
        assert!(has(Icon::Corruption, false), "{casino:?}");
        let kickback = chips(db.deal("kickback").unwrap());
        assert!(
            kickback
                .iter()
                .any(|c| c.icon == Icon::Deadline && c.good && c.text == "30 d"),
            "{kickback:?}"
        );
        assert_eq!(evidence_dots(3.0), "●●○");
        assert_eq!(short_money(20_000), "+$20k");
    }

    #[test]
    fn the_audit_turns_rumours_into_numbers() {
        let mut c = CorruptionState {
            corruption: 42.0,
            institutions: 15.0,
            captured: vec![cp_core::Id::new(3, 0)],
            ..CorruptionState::default()
        };
        let blind = standing(&c);
        assert_eq!(
            blind,
            [
                "Institutions: 15",
                "Rumours of bribes are everywhere.",
                "An audit chamber would give the exact numbers."
            ]
        );
        c.present = cp_content::InstitutionKind::Audit.bit();
        assert_eq!(
            standing(&c),
            [
                "Institutions: 15",
                "Corruption: 42",
                "Police stations on the take: 1"
            ]
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
}
