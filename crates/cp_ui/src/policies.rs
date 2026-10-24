//! Policies (T-193): a tile per policy in the Policies tab of a district and in the City
//! policies window. A tile is lit while the policy is on; its tooltip says the effect in words
//! and the price a week for the buildings it covers now; a click sends `SetPolicy`.
//! Policies whose scope does not allow the place (a city one in a district, or the other way)
//! are not shown there.

use bevy_egui::egui;
use cp_building::BuildingState;
use cp_content::{ContentDb, PolicyDef, PolicyKind};
use cp_core::Cost;
use cp_district::{DistrictCommand, DistrictId};
use cp_locale::{tr, trf};
use cp_sim::{SimCommand, SimView};
use cp_tools::{CommandOutbox, format_money};

use crate::icons::Icon;
use crate::style::Sizes;
use crate::tile::{TileIcon, tool_tile_enabled};

/// What a policy does, in words (`value` is the one number of its definition).
pub fn effect_text(kind: PolicyKind, value: f32) -> String {
    let pct = (value * 100.0).round() as i32;
    match kind {
        PolicyKind::Recycling => trf!("Buildings make {pct}% less garbage", pct = pct),
        PolicyKind::SmokeDetectors => trf!("{pct}% less fire risk", pct = pct),
        PolicyKind::NeighborhoodWatch => trf!("{pct}% less crime", pct = pct),
        PolicyKind::PowerSaving => trf!("{pct}% less electricity used", pct = pct),
        PolicyKind::HighRiseBan => trf!(
            "Tall buildings stop at level {level}",
            level = format!("{value:.0}")
        ),
        PolicyKind::HeavyTrafficBan => tr("Trucks drive around instead of through"),
        PolicyKind::FreeTransit => trf!(
            "Free tickets; transit looks {pct}% better when choosing how to travel",
            pct = pct
        ),
        PolicyKind::TourismPromotion => trf!("{pct}% more tourists", pct = pct),
        PolicyKind::TransparentTenders => trf!(
            "Building costs {pct}% more; contract deals stop and bring no corruption",
            pct = pct
        ),
    }
}

/// The policies that may be on for `district` (`None`: the whole city), in bit order.
pub fn shown(db: &ContentDb, district: Option<DistrictId>) -> Vec<&PolicyDef> {
    PolicyKind::ALL
        .into_iter()
        .filter_map(|k| db.policy(k))
        .filter(|p| {
            if district.is_some() {
                p.scope.allows_district()
            } else {
                p.scope.allows_city()
            }
        })
        .collect()
}

/// Whether `kind` is on for `district` (`None`: the whole city).
pub fn is_on(view: &SimView, district: Option<DistrictId>, kind: PolicyKind) -> bool {
    match district {
        Some(id) => view
            .districts
            .get(id)
            .is_some_and(|d| d.policies & kind.bit() != 0),
        None => view.city_policies.has(kind),
    }
}

/// The active buildings a policy at `district` covers now.
pub fn covered(view: &SimView, district: Option<DistrictId>) -> u32 {
    view.buildings
        .iter()
        .filter(|(_, b)| b.state == BuildingState::Active)
        .filter(|(_, b)| {
            district
                .is_none_or(|id| cp_district::district_at(&view.district_map, b.pos) == Some(id))
        })
        .count() as u32
}

/// What the policy costs a week for the buildings it covers now.
pub fn week_cost(def: &PolicyDef, buildings: u32) -> Cost {
    Cost::from_units(def.cost_per_building_week * buildings as f32)
}

/// The tooltip of a policy tile.
pub fn hover(view: &SimView, def: &PolicyDef, district: Option<DistrictId>) -> String {
    let n = covered(view, district);
    format!(
        "{}\n{}\n{}",
        tr(&def.name),
        effect_text(def.kind, def.value),
        trf!(
            plural "{money}/wk for {n} buildings",
            n = n,
            money = format_money(week_cost(def, n))
        )
    )
}

/// Why `kind` cannot be switched on yet: the milestone it waits for; `None` when it is open.
pub fn lock_text(view: &SimView, db: &ContentDb, kind: PolicyKind) -> Option<String> {
    let m = view.progress.policy_locked_by(db, kind)?;
    let ms = db.milestones().get(m)?;
    Some(crate::buildings::unlocks_at(ms.population, &ms.name))
}

/// The tiles for `district` (`None`: the city); a click switches the policy.
pub fn policy_tiles(
    ui: &mut egui::Ui,
    view: &SimView,
    outbox: &mut CommandOutbox,
    district: Option<DistrictId>,
) {
    let Some(db) = view.content.as_deref() else {
        return;
    };
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(Sizes::TILE_GAP, Sizes::TILE_GAP);
        for def in shown(db, district) {
            let on = is_on(view, district, def.kind);
            // A policy that waits for a milestone stays in the list, greyed (T-283).
            let lock = lock_text(view, db, def.kind);
            let text = match &lock {
                Some(l) => format!("{l}\n{}", hover(view, def, district)),
                None => hover(view, def, district),
            };
            if tool_tile_enabled(
                ui,
                TileIcon::Def(&def.icon),
                &tr(&def.name),
                on,
                lock.is_none(),
            )
            .on_hover_text(text)
            .clicked()
            {
                outbox.push(SimCommand::District(DistrictCommand::SetPolicy {
                    district,
                    policy: def.kind,
                    on: !on,
                }));
            }
        }
    });
}

/// The City policies window.
pub fn city_policies_window(
    ctx: &egui::Context,
    open: &mut bool,
    view: &SimView,
    outbox: &mut CommandOutbox,
) {
    crate::frame::window(
        "city_policies",
        Icon::DistrictTabPolicies,
        &tr("City policies"),
        open,
    )
    .show(ctx, |ui| {
        if view.content.is_none() {
            return;
        }
        ui.weak(tr(
            "They apply everywhere in the city and cost money every week.",
        ));
        policy_tiles(ui, view, outbox, None);
        let n = covered(view, None);
        let total: Cost = view
            .content
            .as_deref()
            .map(|db| {
                shown(db, None)
                    .into_iter()
                    .filter(|p| is_on(view, None, p.kind))
                    .map(|p| week_cost(p, n))
                    .fold(Cost::ZERO, |a, c| a + c)
            })
            .unwrap_or(Cost::ZERO);
        ui.label(trf!(
            "City policies in force cost {money}/wk",
            money = format_money(total)
        ));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{click, frame, input};
    use cp_sim::SimRunner;
    use std::sync::Arc;

    fn view_with_district() -> SimView {
        let db = Arc::new(
            ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
                .unwrap(),
        );
        let mut sim = SimRunner::new(1);
        sim.set_content(db);
        let cell = cp_district::DistrictMap::cell_of(bevy::math::Vec2::new(500.0, 500.0)).unwrap();
        for c in [
            DistrictCommand::Create {
                name: "Hill".into(),
            },
            DistrictCommand::Paint {
                cells: vec![cell],
                district: DistrictId(1),
            },
        ] {
            sim.push_command(SimCommand::District(c), sim.tick());
        }
        sim.tick_once();
        sim.view()
    }

    /// T-283: a policy that waits for a milestone says when it unlocks; Hamlet's is open.
    #[test]
    fn a_locked_policy_says_where_it_unlocks() {
        let mut view = view_with_district();
        view.progress = cp_sim::Progress::START;
        let db = view.content.clone().unwrap();
        assert_eq!(
            lock_text(&view, &db, PolicyKind::NeighborhoodWatch).as_deref(),
            Some("Unlocks at 700 (Large Village)")
        );
        assert_eq!(lock_text(&view, &db, PolicyKind::PowerSaving), None);
        view.progress = cp_sim::Progress::at(2);
        assert_eq!(lock_text(&view, &db, PolicyKind::NeighborhoodWatch), None);
    }

    /// Draws the tiles for `district` and returns the tile rectangles.
    fn draw(
        ctx: &egui::Context,
        view: &SimView,
        outbox: &mut CommandOutbox,
        district: Option<DistrictId>,
        events: Vec<egui::Event>,
    ) -> Vec<egui::Rect> {
        crate::tile::take_rects(ctx);
        frame(ctx, input(0.0, events), |ui| {
            policy_tiles(ui, view, outbox, district)
        });
        crate::tile::take_rects(ctx)
            .0
            .into_iter()
            .map(|(t, _)| t)
            .collect()
    }

    fn sent(outbox: &CommandOutbox) -> Vec<SimCommand> {
        outbox.iter().cloned().collect()
    }

    #[test]
    fn a_click_sends_set_policy_with_the_district_or_none_and_scope_hides_the_wrong_ones() {
        let view = view_with_district();
        let ctx = egui::Context::default();
        let mut outbox = CommandOutbox::default();
        // A district shows the seven that allow districts, no tourism promotion (city only).
        let tiles = draw(&ctx, &view, &mut outbox, Some(DistrictId(1)), vec![]);
        assert_eq!(tiles.len(), 7);
        let db = view.content.as_deref().unwrap();
        assert!(
            shown(db, Some(DistrictId(1)))
                .iter()
                .all(|p| p.kind != PolicyKind::TourismPromotion)
        );
        // The first tile is Recycling: a click switches it on for the district.
        let [down, up] = click(tiles[0].center());
        draw(&ctx, &view, &mut outbox, Some(DistrictId(1)), down);
        draw(&ctx, &view, &mut outbox, Some(DistrictId(1)), up);
        assert_eq!(
            sent(&outbox),
            [SimCommand::District(DistrictCommand::SetPolicy {
                district: Some(DistrictId(1)),
                policy: PolicyKind::Recycling,
                on: true
            })]
        );
        // The city shows all nine (T-327: Transparent tenders), and a click on tourism sends None.
        let mut outbox = CommandOutbox::default();
        let tiles = draw(&ctx, &view, &mut outbox, None, vec![]);
        assert_eq!(tiles.len(), 9);
        let [down, up] = click(tiles[7].center());
        draw(&ctx, &view, &mut outbox, None, down);
        draw(&ctx, &view, &mut outbox, None, up);
        assert_eq!(
            sent(&outbox),
            [SimCommand::District(DistrictCommand::SetPolicy {
                district: None,
                policy: PolicyKind::TourismPromotion,
                on: true
            })]
        );
    }

    #[test]
    fn a_policy_that_is_on_is_lit_and_a_click_switches_it_off_and_the_tooltip_has_effect_and_price()
    {
        let mut view = view_with_district();
        let mut city = view.city_policies;
        city.mask = PolicyKind::Recycling.bit();
        view.city_policies = city;
        assert!(is_on(&view, None, PolicyKind::Recycling));
        assert!(!is_on(&view, None, PolicyKind::PowerSaving));
        let ctx = egui::Context::default();
        let mut outbox = CommandOutbox::default();
        let tiles = draw(&ctx, &view, &mut outbox, None, vec![]);
        let [down, up] = click(tiles[0].center());
        draw(&ctx, &view, &mut outbox, None, down);
        draw(&ctx, &view, &mut outbox, None, up);
        assert!(matches!(
            sent(&outbox)[..],
            [SimCommand::District(DistrictCommand::SetPolicy {
                district: None,
                policy: PolicyKind::Recycling,
                on: false
            })]
        ));
        let db = view.content.as_deref().unwrap();
        let def = db.policy(PolicyKind::Recycling).unwrap();
        let text = hover(&view, def, None);
        assert!(text.contains("30% less garbage"), "{text}");
        assert!(text.contains("/wk for 0 buildings"), "{text}");
        assert_eq!(week_cost(def, 10), Cost::from_units(20.0));
    }
}
