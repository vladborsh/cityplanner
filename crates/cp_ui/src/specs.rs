//! District specialisations (T-197): the Specialisation tab of a district. One row per group of
//! zone types a specialisation can take (Commercial: low and high shops together; Office;
//! Residential low), each a tile "None" and a tile per specialisation of the group, the one in
//! force lit. A click sends one `SetSpec` for every zone type of the group.

use bevy_egui::egui;
use cp_content::{ContentDb, SpecDef, SpecKind, ZoneType};
use cp_district::{DistrictCommand, DistrictId, zone_slot};
use cp_locale::{tr, trf};
use cp_sim::{SimCommand, SimView};
use cp_tools::CommandOutbox;

use crate::icons::Icon;
use crate::style::Sizes;
use crate::tile::{TileIcon, tool_tile};

/// What a group of zone types is called.
pub fn group_name(zones: &[ZoneType]) -> &'static str {
    match zones {
        [ZoneType::ComLow, ZoneType::ComHigh] => "Commercial",
        [ZoneType::Office] => "Office",
        [ZoneType::ResLow] => "Residential low",
        [ZoneType::Industrial] => "Industrial",
        _ => "Other",
    }
}

/// The specialisation groups of the content: the zone types and the specialisations of each, in
/// kind order; groups in the order their first specialisation appears.
pub fn groups(db: &ContentDb) -> Vec<(Vec<ZoneType>, Vec<&SpecDef>)> {
    let mut out: Vec<(Vec<ZoneType>, Vec<&SpecDef>)> = Vec::new();
    for kind in SpecKind::ALL {
        let Some(def) = db.spec(kind) else { continue };
        match out.iter_mut().find(|(z, _)| *z == def.zones) {
            Some((_, list)) => list.push(def),
            None => out.push((def.zones.clone(), vec![def])),
        }
    }
    out
}

/// The specialisation in force for a group in `district` (read from its first zone type).
pub fn current(view: &SimView, district: DistrictId, zones: &[ZoneType]) -> Option<SpecKind> {
    let slot = zone_slot(*zones.first()?)?;
    view.districts.get(district)?.spec[slot]
}

fn pct(x: f32) -> i32 {
    ((x - 1.0) * 100.0).round() as i32
}

/// What a specialisation does, in words, one effect after the other.
pub fn effect_text(d: &SpecDef) -> String {
    let mut parts = Vec::new();
    if pct(d.tax_mul) != 0 {
        parts.push(trf!("{v}% tax", v = format!("{:+}", pct(d.tax_mul))));
    }
    if pct(d.tourist_weight) != 0 {
        parts.push(trf!(
            "{v}% tourists",
            v = format!("{:+}", pct(d.tourist_weight))
        ));
    }
    if d.noise_add > 0.0 {
        parts.push(trf!("noisier (+{v})", v = format!("{:.0}", d.noise_add)));
    }
    if d.land_value_add > 0.0 {
        parts.push(trf!(
            "land value +{v}",
            v = format!("{:.0}", d.land_value_add)
        ));
    }
    if pct(d.goods_mul) != 0 {
        parts.push(trf!(
            "{v}% goods used",
            v = format!("{:+}", pct(d.goods_mul))
        ));
    }
    if parts.is_empty() {
        tr("Its own look")
    } else {
        parts.join(", ")
    }
}

/// The tab body for `district`.
pub fn specs_tab(ui: &mut egui::Ui, view: &SimView, outbox: &mut CommandOutbox, id: DistrictId) {
    let Some(db) = view.content.as_deref() else {
        return;
    };
    for (zones, specs) in groups(db) {
        ui.weak(tr(group_name(&zones)));
        let now = current(view, id, &zones);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(Sizes::TILE_GAP, Sizes::TILE_GAP);
            let mut pick: Option<Option<SpecKind>> = None;
            if tool_tile(
                ui,
                TileIcon::Icon(Icon::SpecNone),
                &tr("None"),
                now.is_none(),
            )
            .on_hover_text(tr("Ordinary buildings"))
            .clicked()
            {
                pick = Some(None);
            }
            for def in &specs {
                if tool_tile(
                    ui,
                    TileIcon::Def(&def.icon),
                    &tr(&def.name),
                    now == Some(def.kind),
                )
                .on_hover_text(format!("{}\n{}", tr(&def.name), effect_text(def)))
                .clicked()
                {
                    pick = Some(Some(def.kind));
                }
            }
            if let Some(spec) = pick {
                for zone in &zones {
                    outbox.push(SimCommand::District(DistrictCommand::SetSpec {
                        district: id,
                        zone: *zone,
                        spec,
                    }));
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{click, frame, input};
    use cp_sim::SimRunner;
    use std::sync::Arc;

    fn view_with(spec: Option<(ZoneType, SpecKind)>) -> SimView {
        let db = Arc::new(
            ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
                .unwrap(),
        );
        let mut sim = SimRunner::new(1);
        sim.set_content(db);
        let cell = cp_district::DistrictMap::cell_of(bevy::math::Vec2::new(500.0, 500.0)).unwrap();
        let mut cmds = vec![
            DistrictCommand::Create {
                name: "Hill".into(),
            },
            DistrictCommand::Paint {
                cells: vec![cell],
                district: DistrictId(1),
            },
        ];
        if let Some((zone, kind)) = spec {
            cmds.push(DistrictCommand::SetSpec {
                district: DistrictId(1),
                zone,
                spec: Some(kind),
            });
        }
        for c in cmds {
            sim.push_command(SimCommand::District(c), sim.tick());
        }
        sim.tick_once();
        sim.view()
    }

    fn draw(
        ctx: &egui::Context,
        view: &SimView,
        outbox: &mut CommandOutbox,
        events: Vec<egui::Event>,
    ) -> Vec<egui::Rect> {
        crate::tile::take_rects(ctx);
        frame(ctx, input(0.0, events), |ui| {
            specs_tab(ui, view, outbox, DistrictId(1))
        });
        crate::tile::take_rects(ctx)
            .0
            .into_iter()
            .map(|(t, _)| t)
            .collect()
    }

    #[test]
    fn three_groups_commercial_office_and_residential_low_with_none_and_the_specs() {
        let view = view_with(None);
        let g = groups(view.content.as_deref().unwrap());
        let names: Vec<_> = g.iter().map(|(z, _)| group_name(z)).collect();
        assert_eq!(names, ["Commercial", "Office", "Residential low"]);
        let sizes: Vec<_> = g.iter().map(|(_, s)| s.len()).collect();
        assert_eq!(sizes, [3, 1, 1]);
        // None + 3, None + 1, None + 1.
        let tiles = draw(
            &egui::Context::default(),
            &view,
            &mut CommandOutbox::default(),
            vec![],
        );
        assert_eq!(tiles.len(), 4 + 2 + 2);
    }

    #[test]
    fn a_commercial_click_sends_a_set_spec_for_both_commercial_zone_types_and_none_clears() {
        let view = view_with(None);
        let ctx = egui::Context::default();
        let mut outbox = CommandOutbox::default();
        let tiles = draw(&ctx, &view, &mut outbox, vec![]);
        // Tiles of the first row: None, Leisure, Tourism, Organic.
        let [down, up] = click(tiles[2].center());
        draw(&ctx, &view, &mut outbox, down);
        draw(&ctx, &view, &mut outbox, up);
        let set = |zone, spec| {
            SimCommand::District(DistrictCommand::SetSpec {
                district: DistrictId(1),
                zone,
                spec,
            })
        };
        let sent: Vec<SimCommand> = outbox.iter().cloned().collect();
        assert_eq!(
            sent,
            [
                set(ZoneType::ComLow, Some(SpecKind::Tourism)),
                set(ZoneType::ComHigh, Some(SpecKind::Tourism))
            ]
        );
        // None on the office row clears the one zone type.
        let mut outbox = CommandOutbox::default();
        let [down, up] = click(tiles[4].center());
        draw(&ctx, &view, &mut outbox, down);
        draw(&ctx, &view, &mut outbox, up);
        let sent: Vec<SimCommand> = outbox.iter().cloned().collect();
        assert_eq!(sent, [set(ZoneType::Office, None)]);
    }

    #[test]
    fn the_specialisation_in_force_is_read_from_the_view_and_effects_are_in_words() {
        let view = view_with(Some((ZoneType::ComLow, SpecKind::Leisure)));
        let zones = [ZoneType::ComLow, ZoneType::ComHigh];
        assert_eq!(
            current(&view, DistrictId(1), &zones),
            Some(SpecKind::Leisure)
        );
        assert_eq!(current(&view, DistrictId(1), &[ZoneType::Office]), None);
        let db = view.content.as_deref().unwrap();
        let text = effect_text(db.spec(SpecKind::Leisure).unwrap());
        assert!(
            text.contains("noisier") && text.contains("+50% tourists"),
            "{text}"
        );
        let text = effect_text(db.spec(SpecKind::ItCluster).unwrap());
        assert!(text.contains("+30% tax"), "{text}");
        let text = effect_text(db.spec(SpecKind::Organic).unwrap());
        assert!(
            text.contains("-30% goods used") && text.contains("land value +5"),
            "{text}"
        );
    }
}
