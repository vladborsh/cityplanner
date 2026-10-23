//! The Parks category (T-226, M6): the area brush, "New area" with the choice of its kind, the
//! list of the park areas, and a section per kind with its gate and attractions (closed until
//! an area of the kind has the level), the props and the footpath tile. The Parks overlay shows
//! automatically while a tool of the category is on.

use bevy_egui::egui;
use cp_content::{BuildingDefId, ContentDb, ParkKind, ParkRole, ServiceKind};
use cp_core::Cost;
use cp_district::{AreaId, AreaKind, DistrictCommand, NAME_MAX};
use cp_locale::{tr, trf, trn};
use cp_render::{Overlay, OverlayState, park_color};
use cp_sim::{SimCommand, SimView};
use cp_tools::{ActiveTool, CommandOutbox, DistrictToolKind, ToolState, format_money};

use crate::buildings::{Card, UniqueState, badge, card_text, groups, select};
use crate::icons::Icon;
use crate::industry::next_id;
use crate::panels::section;
use crate::style::Sizes;
use crate::tile::{TileIcon, tool_tile, tool_tile_enabled};

pub fn kind_name(kind: ParkKind) -> &'static str {
    cp_tools::area::park_kind_name(kind)
}

pub fn kind_icon(kind: ParkKind) -> Icon {
    match kind {
        ParkKind::CityPark => Icon::ParkCityPark,
        ParkKind::Nature => Icon::ParkNature,
        ParkKind::Zoo => Icon::ParkZoo,
        ParkKind::Amusement => Icon::ParkAmusement,
    }
}

/// The colour of a park kind for swatches.
pub fn swatch(kind: ParkKind) -> egui::Color32 {
    let [r, g, b] = park_color(kind);
    egui::Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

/// "Zoo 3" with `3` the id the new area gets.
pub fn new_name(kind: ParkKind, id: AreaId) -> String {
    cp_tools::area::new_area_name(AreaKind::Park(kind), id)
}

fn kind_tool_icon(kind: DistrictToolKind) -> Icon {
    match kind {
        DistrictToolKind::BrushSmall => Icon::DistrictBrushSmall,
        DistrictToolKind::BrushLarge => Icon::DistrictBrushLarge,
        DistrictToolKind::Erase => Icon::DistrictErase,
    }
}

fn send(outbox: &mut CommandOutbox, cmd: DistrictCommand) {
    outbox.push(SimCommand::District(cmd));
}

fn show_overlay(overlay: &mut OverlayState) {
    overlay.open_auto(Overlay::Parks);
}

/// The cards of a section: the gate and the attractions of `kind` by level, or (`None`) the
/// props.
pub fn section_cards(db: &ContentDb, kind: Option<ParkKind>) -> Vec<Card> {
    let mut cards: Vec<(u8, u8, Card)> = groups(db)
        .into_iter()
        .filter(|g| g.kind == ServiceKind::Park)
        .flat_map(|g| g.cards)
        .filter_map(|c| {
            let spec = db.building(c.id).park.as_ref()?;
            let mine = match kind {
                Some(k) => spec.park == Some(k) && spec.role != ParkRole::Prop,
                None => spec.role == ParkRole::Prop,
            };
            mine.then_some((spec.role as u8, spec.min_level, c))
        })
        .collect();
    cards.sort_by_key(|(role, level, c)| (*role, *level, c.id));
    cards.into_iter().map(|(_, _, c)| c).collect()
}

/// Why a park building cannot be placed yet, from the view alone: no area of its kind (or any
/// park area, for a prop), or none high enough. `None` when it is open (or not a park-area
/// building). The same rules as `cp_building::validate_park_site`, read from the snapshot.
pub fn park_lock(view: &SimView, db: &ContentDb, def: BuildingDefId) -> Option<String> {
    let spec = db.building(def).park.as_ref()?;
    let best = |want: Option<ParkKind>| -> Option<u8> {
        view.areas
            .iter()
            .filter_map(|(_, a)| match a.kind {
                AreaKind::Park(k) if want.is_none_or(|w| w == k) => Some(a.level),
                _ => None,
            })
            .max()
    };
    match (spec.role, spec.park) {
        // A city park of the Buildings panel stands anywhere.
        (ParkRole::Attraction, None) => None,
        (ParkRole::Prop, _) => best(None).is_none().then(|| tr("needs a park area")),
        (role, Some(kind)) => match best(Some(kind)) {
            None => Some(trf!("needs a {kind} area", kind = tr(kind_name(kind)))),
            Some(level) if role == ParkRole::Attraction && level < spec.min_level => {
                Some(trf!("area level {n}", n = spec.min_level))
            }
            Some(_) => None,
        },
        (ParkRole::Gate, None) => None,
    }
}

/// One tile row of cards, closed ones grey with a padlock.
fn card_row(
    ui: &mut egui::Ui,
    view: &SimView,
    db: &ContentDb,
    cards: &[Card],
    tools: &mut ToolState,
    overlay: &mut OverlayState,
) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(Sizes::TILE_GAP, Sizes::TILE_GAP);
        for card in cards {
            let lock = park_lock(view, db, card.id);
            let on = tools.active == ActiveTool::Place && tools.building.def == Some(card.id);
            let hover = match &lock {
                Some(l) => format!(
                    "{}\n{}",
                    trf!("Locked: {reason}", reason = l),
                    card_text(card)
                ),
                None => card_text(card),
            };
            let r = tool_tile_enabled(
                ui,
                TileIcon::Def(&card.icon),
                &tr(&card.name),
                on,
                lock.is_none(),
            )
            .on_hover_text(hover);
            if lock.is_some() {
                badge(ui, r.rect, UniqueState::Locked);
            }
            if r.clicked() {
                select(tools, card.id);
                show_overlay(overlay);
            }
        }
    });
}

/// The label of every park area: "Name · L2" at the middle of its cells.
pub fn area_labels(view: &SimView) -> Vec<(String, bevy::math::Vec2)> {
    view.areas
        .iter()
        .filter(|(_, a)| matches!(a.kind, AreaKind::Park(_)))
        .filter_map(|(id, a)| {
            let (mut sum, mut n) = (bevy::math::Vec2::ZERO, 0.0f32);
            for (c, r) in view.area_map.cells_of(id) {
                sum += bevy::math::Vec2::new(
                    (f32::from(c) + 0.5) * cp_district::CELL_M,
                    (f32::from(r) + 0.5) * cp_district::CELL_M,
                );
                n += 1.0;
            }
            (n > 0.0).then(|| {
                (
                    format!("{} · {}", a.name, trf!("L{n}", n = a.level)),
                    sum / n,
                )
            })
        })
        .collect()
}

/// Draws the names and levels of the park areas on the map while the Parks overlay is on or a
/// brush paints a park (T-226).
pub fn draw_labels(
    mut contexts: bevy_egui::EguiContexts,
    view: Option<bevy::prelude::Res<SimView>>,
    overlay: bevy::prelude::Res<OverlayState>,
    cameras: bevy::prelude::Query<(&bevy::prelude::Camera, &bevy::prelude::GlobalTransform)>,
) {
    let Some(view) = view else { return };
    if overlay.active != Overlay::Parks {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let Ok((camera, transform)) = cameras.single() else {
        return;
    };
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("park_labels"),
    ));
    for (text, at) in area_labels(&view) {
        let y = view.terrain.as_deref().map_or(0.0, |t| t.height(at));
        let Ok(p) = camera.world_to_viewport(transform, bevy::math::Vec3::new(at.x, y, at.y))
        else {
            continue;
        };
        let pos = egui::pos2(p.x, p.y);
        for (off, color) in [
            (egui::vec2(1.0, 1.0), egui::Color32::BLACK),
            (egui::Vec2::ZERO, egui::Color32::WHITE),
        ] {
            painter.text(
                pos + off,
                egui::Align2::CENTER_CENTER,
                &text,
                egui::FontId::proportional(16.0),
                color,
            );
        }
    }
}

/// The Parks panel.
pub fn parks_panel(
    ui: &mut egui::Ui,
    view: Option<&SimView>,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
    outbox: &mut CommandOutbox,
) {
    let of_kind = |view: &SimView, kind: ParkKind, id: AreaId| {
        view.areas
            .get(id)
            .is_some_and(|a| a.kind == AreaKind::Park(kind))
    };
    section(ui, &tr("Tool"), |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(Sizes::TILE_GAP, Sizes::TILE_GAP);
            for kind in DistrictToolKind::ALL {
                let selected = tools.active == ActiveTool::Area && tools.area.kind == kind;
                if tool_tile(
                    ui,
                    TileIcon::Icon(kind_tool_icon(kind)),
                    &tr(kind.label()),
                    selected,
                )
                .clicked()
                {
                    if tools.active != ActiveTool::Area {
                        tools.select(ActiveTool::Area);
                    }
                    tools.area.set_kind(kind);
                    // A brush with no park area under it paints nothing: take the first.
                    if let Some(view) = view
                        && tools.area.selected.is_none_or(|a| {
                            !matches!(view.areas.get(a).map(|a| a.kind), Some(AreaKind::Park(_)))
                        })
                    {
                        tools.area.selected = view
                            .areas
                            .iter()
                            .find(|(_, a)| matches!(a.kind, AreaKind::Park(_)))
                            .map(|(id, _)| id);
                    }
                    show_overlay(overlay);
                }
            }
            if tool_tile_enabled(
                ui,
                TileIcon::Icon(Icon::Remove),
                &tr("Off"),
                false,
                tools.active == ActiveTool::Area,
            )
            .clicked()
            {
                tools.select(ActiveTool::None);
            }
        });
    });
    let Some(view) = view else { return };
    let Some(db) = view.content.as_deref() else {
        return;
    };
    section(ui, &tr("New area"), |ui| {
        ui.weak(tr("Choose the kind of park, then press New area."));
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(Sizes::TILE_GAP, Sizes::TILE_GAP);
            for kind in ParkKind::ALL {
                let on = tools.area.new_park == Some(kind);
                if tool_tile(
                    ui,
                    TileIcon::Icon(kind_icon(kind)),
                    &tr(kind_name(kind)),
                    on,
                )
                .on_hover_text(trf!(
                    "{kind}: the colour of its area and the overlay",
                    kind = tr(kind_name(kind))
                ))
                .clicked()
                {
                    tools.area.new_park = Some(kind);
                    // The selected area of that kind, else the first, else a new one.
                    let existing = tools
                        .area
                        .selected
                        .filter(|&a| of_kind(view, kind, a))
                        .or_else(|| {
                            view.areas
                                .iter()
                                .map(|(id, _)| id)
                                .find(|&id| of_kind(view, kind, id))
                        });
                    let chosen = match existing {
                        Some(id) => Some(id),
                        None => next_id(view).inspect(|&id| {
                            send(
                                outbox,
                                DistrictCommand::CreateArea {
                                    kind: AreaKind::Park(kind),
                                    name: new_name(kind, id),
                                },
                            );
                        }),
                    };
                    if chosen.is_some() {
                        tools.area.selected = chosen;
                        if tools.active != ActiveTool::Area {
                            tools.select(ActiveTool::Area);
                        }
                        show_overlay(overlay);
                    }
                }
            }
            let can = next_id(view).zip(tools.area.new_park);
            if tool_tile_enabled(
                ui,
                TileIcon::Icon(Icon::DistrictNew),
                &tr("New area"),
                false,
                can.is_some(),
            )
            .clicked()
                && let Some((id, kind)) = can
            {
                send(
                    outbox,
                    DistrictCommand::CreateArea {
                        kind: AreaKind::Park(kind),
                        name: new_name(kind, id),
                    },
                );
                tools.area.selected = Some(id);
                if tools.active != ActiveTool::Area {
                    tools.select(ActiveTool::Area);
                }
                show_overlay(overlay);
            }
        });
    });
    if tools.active == ActiveTool::Area
        && crate::tray::is_active(ui.ctx(), &tr("New area"))
        && let Some(id) = tools.area.selected
        && let Some(a) = view.areas.get(id)
    {
        ui.label(trf!("Painting: {name}", name = a.name));
    }
    section(ui, &tr("Areas"), |ui| {
        if !view
            .areas
            .iter()
            .any(|(_, a)| matches!(a.kind, AreaKind::Park(_)))
        {
            ui.weak(tr(
                "No park areas yet: pick a kind, press New area, then paint with a brush.",
            ));
        }
        for (id, a) in view.areas.iter() {
            let AreaKind::Park(kind) = a.kind else {
                continue;
            };
            let selected = tools.area.selected == Some(id);
            ui.horizontal(|ui| {
                let (rect, resp) =
                    ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
                ui.painter().rect_filled(rect, 3.0, swatch(kind));
                if selected {
                    ui.painter().rect_stroke(
                        rect,
                        3.0,
                        egui::Stroke::new(2.0, ui.visuals().selection.stroke.color),
                        egui::StrokeKind::Outside,
                    );
                }
                if resp.on_hover_text(tr("Choose this area")).clicked() {
                    tools.area.selected = Some(id);
                    if tools.active != ActiveTool::Area {
                        tools.select(ActiveTool::Area);
                    }
                    show_overlay(overlay);
                }
                let key = egui::Id::new(("park_area_name", id.0));
                let mut buf: String = ui
                    .data(|m| m.get_temp(key))
                    .unwrap_or_else(|| a.name.clone());
                if !ui.memory(|m| m.has_focus(key)) {
                    buf = a.name.clone();
                }
                let r = ui.add(
                    egui::TextEdit::singleline(&mut buf)
                        .id(key)
                        .desired_width(130.0)
                        .char_limit(NAME_MAX),
                );
                if r.lost_focus() && buf.trim() != a.name && !buf.trim().is_empty() {
                    send(
                        outbox,
                        DistrictCommand::RenameArea {
                            area: id,
                            name: buf.trim().to_string(),
                        },
                    );
                }
                ui.data_mut(|m| m.insert_temp(key, buf));
                ui.label(trf!("L{n}", n = a.level));
                if ui.small_button(tr("Delete")).clicked() {
                    send(outbox, DistrictCommand::DeleteArea { area: id });
                    if selected {
                        tools.area.selected = None;
                    }
                }
            });
            ui.weak(format!(
                "    {} · {}",
                tr(kind_name(kind)),
                trn("{n} cells", view.area_map.cell_count(id) as i64)
            ));
        }
    });
    for kind in ParkKind::ALL {
        let cards = section_cards(db, Some(kind));
        if cards.is_empty() {
            continue;
        }
        if !crate::tray::is_active(ui.ctx(), &tr(kind_name(kind))) {
            continue;
        }
        if !crate::tray::in_tray(ui.ctx()) {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                kind_icon(kind).show(ui, Sizes::SECTION_ICON);
                ui.strong(tr(kind_name(kind)));
            });
        }
        card_row(ui, view, db, &cards, tools, overlay);
    }
    let props = section_cards(db, None);
    if !props.is_empty() && crate::tray::is_active(ui.ctx(), &tr("Props")) {
        if !crate::tray::in_tray(ui.ctx()) {
            ui.add_space(4.0);
            ui.strong(tr("Props"));
        }
        card_row(ui, view, db, &props, tools, overlay);
    }
    if crate::tray::is_active(ui.ctx(), &tr("Path")) {
        if !crate::tray::in_tray(ui.ctx()) {
            ui.add_space(4.0);
            ui.strong(tr("Path"));
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(Sizes::TILE_GAP, Sizes::TILE_GAP);
            crate::roads::path_tile(ui, db, tools);
        });
    }
}

/// The numbers of the park panel (T-229), read from the view.
#[derive(Clone, Debug, PartialEq)]
pub struct ParkSummary {
    pub name: String,
    pub kind: ParkKind,
    pub level: u8,
    pub xp: u32,
    /// The xp of the next level, `None` at the top.
    pub next_xp: Option<u32>,
    pub inside: u32,
    pub capacity: u32,
    pub visitors_day: u32,
    pub visitors_week: u32,
    pub attract: f32,
    pub fee: f32,
    pub fee_max: f32,
    /// Gates and other park buildings of the area.
    pub gates: u32,
    /// Where the first gate stands, for "Jump".
    pub gate_at: Option<bevy::math::Vec2>,
    pub buildings: u32,
    /// Fees the visits of the week come to so far (visitors x fee).
    pub income_week: Cost,
    /// The weekly upkeep of the area's buildings at the budget now.
    pub upkeep_week: Cost,
}

/// The summary of park area `id`; `None` for another kind of area or without content.
pub fn park_summary(view: &SimView, id: AreaId) -> Option<ParkSummary> {
    let db = view.content.as_deref()?;
    let area = view.areas.get(id)?;
    let AreaKind::Park(kind) = area.kind else {
        return None;
    };
    let st = view.parks.get(id).copied().unwrap_or_default();
    let mut out = ParkSummary {
        name: area.name.clone(),
        kind,
        level: area.level,
        xp: area.xp,
        next_xp: db
            .balance()
            .park_levels
            .get(usize::from(area.level.saturating_sub(1)))
            .copied(),
        inside: st.inside,
        capacity: st.capacity,
        visitors_day: st.visitors_day,
        visitors_week: st.visitors_week,
        attract: st.attract,
        fee: st.fee,
        fee_max: db.balance().park_fee_max,
        gates: 0,
        gate_at: None,
        buildings: 0,
        income_week: Cost((f64::from(st.visitors_week) * f64::from(st.fee) * 100.0).round() as i64),
        upkeep_week: Cost::ZERO,
    };
    for (_, b) in view.buildings.iter() {
        let def = db.building(b.def);
        let Some(spec) = &def.park else { continue };
        let in_area = match spec.role {
            ParkRole::Gate => {
                cp_park::gate_area(&view.areas, &view.area_map, kind, b.pos) == Some(id)
            }
            _ => cp_district::area_at(&view.area_map, b.pos) == Some(id),
        };
        if !in_area {
            continue;
        }
        if spec.role == ParkRole::Gate {
            out.gates += 1;
            out.gate_at.get_or_insert(b.pos);
        }
        out.buildings += 1;
        if let Some(service) = def.service.as_deref().and_then(|s| db.service_id(s)) {
            let pct = view.economy.budgets.get(db.service(service).kind);
            out.upkeep_week += Cost::scaled(def.upkeep_week, f64::from(pct) / 100.0);
        }
    }
    Some(out)
}

/// The area a clicked park building belongs to: a gate's area, or the area its cell is in.
pub fn area_of_building(view: &SimView, id: cp_building::BuildingId) -> Option<AreaId> {
    let db = view.content.as_deref()?;
    let b = view.buildings.get(id)?;
    let spec = db.building(b.def).park.as_ref()?;
    match spec.role {
        ParkRole::Gate => cp_park::gate_area(&view.areas, &view.area_map, spec.park?, b.pos),
        _ => cp_district::area_at(&view.area_map, b.pos).filter(|&a| {
            view.areas
                .get(a)
                .is_some_and(|a| matches!(a.kind, AreaKind::Park(_)))
        }),
    }
}

/// The park panel (T-229): kind, level and its xp bar, the visitors, the draw, the fee slider
/// and the money of the week. `rects` gets the slider's rectangle for tests.
pub fn park_panel(
    ui: &mut egui::Ui,
    view: &SimView,
    id: AreaId,
    outbox: &mut CommandOutbox,
    slider: &mut Option<egui::Rect>,
) {
    let Some(s) = park_summary(view, id) else {
        return;
    };
    ui.horizontal(|ui| {
        kind_icon(s.kind).show(ui, Sizes::SECTION_ICON);
        ui.strong(tr(kind_name(s.kind)));
    });
    ui.label(trf!("Level {n}", n = s.level));
    let (frac, text) = match s.next_xp {
        Some(next) => (
            (s.xp as f32 / next.max(1) as f32).min(1.0),
            trf!("{xp} / {next} xp", xp = s.xp, next = next),
        ),
        None => (1.0, trf!("{xp} xp (top level)", xp = s.xp)),
    };
    ui.add(egui::ProgressBar::new(frac).text(text));
    egui::Grid::new("park_rows").num_columns(2).show(ui, |ui| {
        let mut row = |l: &str, v: String| {
            ui.weak(tr(l));
            ui.label(v);
            ui.end_row();
        };
        row("Inside now", format!("{} / {}", s.inside, s.capacity));
        row(
            "Visitors",
            trf!(
                "{today} today · {week} this week",
                today = s.visitors_day,
                week = s.visitors_week
            ),
        );
        row("Attraction", format!("{:.0}", s.attract));
        row("Gates", s.gates.to_string());
        row("Fees this week", format_money(s.income_week));
        row("Upkeep (week)", format_money(s.upkeep_week));
        row(
            "Fees last week (all parks)",
            format_money(view.park_ledger.last_fees),
        );
    });
    if s.gates == 0 {
        ui.colored_label(
            egui::Color32::from_rgb(230, 160, 40),
            tr("No gate: nobody can come in. Put a gate on a road next to the area."),
        );
    }
    ui.horizontal(|ui| {
        ui.label(tr("Entrance fee"));
        let mut fee = s.fee;
        let r = ui.add(
            egui::Slider::new(&mut fee, 0.0..=s.fee_max)
                .step_by(0.5)
                .prefix("$"),
        );
        *slider = Some(r.rect);
        if r.changed() && (fee - s.fee).abs() > f32::EPSILON {
            outbox.push(SimCommand::Park(cp_park::ParkCommand::SetFee {
                area: id,
                fee,
            }));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{click, frame, input};
    use cp_sim::SimRunner;
    use std::sync::Arc;

    fn sim() -> SimRunner {
        let db = Arc::new(
            ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
                .unwrap(),
        );
        let mut sim = SimRunner::new(1);
        sim.set_content(db);
        sim
    }

    /// A world with a Zoo area (id 1) at `level`.
    fn zoo_world(level: u8) -> (SimRunner, Arc<ContentDb>) {
        let mut sim = sim();
        let db = sim.view().content.clone().unwrap();
        let cell =
            cp_district::DistrictMap::cell_of(bevy::math::Vec2::new(1000.0, 1000.0)).unwrap();
        for c in [
            DistrictCommand::CreateArea {
                kind: AreaKind::Park(ParkKind::Zoo),
                name: "Zoo 1".into(),
            },
            DistrictCommand::PaintArea {
                cells: vec![cell],
                area: AreaId(1),
            },
        ] {
            sim.push_command(SimCommand::District(c), sim.tick());
        }
        sim.tick_once();
        sim.world_mut()
            .resource_mut::<cp_district::Areas>()
            .get_mut(AreaId(1))
            .unwrap()
            .level = level;
        (sim, db)
    }

    fn draw(
        ctx: &egui::Context,
        view: &SimView,
        tools: &mut ToolState,
        overlay: &mut OverlayState,
        outbox: &mut CommandOutbox,
        events: Vec<egui::Event>,
    ) -> Vec<egui::Rect> {
        crate::tile::take_rects(ctx);
        // A tall screen: the whole panel is on it.
        let mut raw = input(0.0, events);
        raw.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 3000.0),
        ));
        frame(ctx, raw, |ui| {
            parks_panel(ui, Some(view), tools, overlay, outbox)
        });
        crate::tile::take_rects(ctx)
            .0
            .into_iter()
            .map(|(t, _)| t)
            .collect()
    }

    fn press(
        ctx: &egui::Context,
        view: &SimView,
        tools: &mut ToolState,
        overlay: &mut OverlayState,
        outbox: &mut CommandOutbox,
        at: egui::Pos2,
    ) {
        let [down, up] = click(at);
        draw(ctx, view, tools, overlay, outbox, down);
        draw(ctx, view, tools, overlay, outbox, up);
    }

    /// Tiles: Brush S, Brush L, Erase, Off | four kinds, New area | the cards of the four kinds
    /// (a gate and four attractions each) | six props | the path.
    #[test]
    fn the_zoo_tile_makes_a_zoo_area_and_selects_it_and_shows_the_parks_overlay() {
        let sim = sim();
        let view = sim.view();
        let ctx = egui::Context::default();
        let (mut tools, mut overlay, mut outbox) = (
            ToolState::default(),
            OverlayState::default(),
            CommandOutbox::default(),
        );
        let tiles = draw(&ctx, &view, &mut tools, &mut overlay, &mut outbox, vec![]);
        assert_eq!(tiles.len(), 4 + 4 + 1 + 20 + 6 + 1, "{}", tiles.len());
        press(
            &ctx,
            &view,
            &mut tools,
            &mut overlay,
            &mut outbox,
            tiles[6].center(),
        );
        assert_eq!(tools.area.new_park, Some(ParkKind::Zoo));
        assert_eq!(
            outbox.iter().cloned().collect::<Vec<_>>(),
            [SimCommand::District(DistrictCommand::CreateArea {
                kind: AreaKind::Park(ParkKind::Zoo),
                name: "Zoo 1".into(),
            })]
        );
        assert_eq!(tools.area.selected, Some(AreaId(1)));
        assert_eq!(tools.active, ActiveTool::Area);
        assert_eq!(overlay.active, Overlay::Parks);
        // The path tile picks the road tool with the footpath (the panel grew: read it again).
        let tiles = draw(&ctx, &view, &mut tools, &mut overlay, &mut outbox, vec![]);
        press(
            &ctx,
            &view,
            &mut tools,
            &mut overlay,
            &mut outbox,
            tiles.last().unwrap().center(),
        );
        assert_eq!(tools.active, ActiveTool::Road);
        let path = view
            .content
            .as_deref()
            .unwrap()
            .road_id("park_path")
            .unwrap();
        assert_eq!(tools.road.road, Some(path));
    }

    /// T-226: a park building is closed without an area of its kind and below its level, and
    /// opens with both; a prop wants any park area; a city park is never closed.
    #[test]
    fn a_zoo_attraction_is_closed_in_a_level_one_area_and_opens_at_its_level() {
        let id = |db: &ContentDb, n: &str| db.building_id(n).unwrap();
        let (sim, db) = zoo_world(1);
        let lock = |sim: &SimRunner, n: &str| park_lock(&sim.view(), &db, id(&db, n));
        assert_eq!(lock(&sim, "zoo_gate"), None);
        assert_eq!(lock(&sim, "zoo_attr_1"), None);
        assert_eq!(lock(&sim, "zoo_attr_2").as_deref(), Some("area level 2"));
        assert_eq!(
            lock(&sim, "nature_gate").as_deref(),
            Some("needs a Nature reserve area")
        );
        assert_eq!(lock(&sim, "prop_bench"), None);
        assert_eq!(lock(&sim, "small_park"), None);
        let (sim, db) = zoo_world(2);
        assert_eq!(park_lock(&sim.view(), &db, id(&db, "zoo_attr_2")), None);
        assert_eq!(
            park_lock(&sim.view(), &db, id(&db, "zoo_attr_3")).as_deref(),
            Some("area level 3")
        );
        // No area at all: even the props wait.
        let empty = self::sim();
        assert_eq!(
            park_lock(&empty.view(), &db, id(&db, "prop_bench")).as_deref(),
            Some("needs a park area")
        );
    }

    #[test]
    fn a_park_area_is_labelled_with_its_name_and_level_at_the_middle_of_its_cells() {
        let (sim, _) = zoo_world(3);
        let labels = area_labels(&sim.view());
        assert_eq!(labels.len(), 1);
        assert_eq!(labels[0].0, "Zoo 1 · L3");
        // One cell of 32 m: its middle.
        let cell =
            cp_district::DistrictMap::cell_of(bevy::math::Vec2::new(1000.0, 1000.0)).unwrap();
        assert_eq!(
            labels[0].1,
            bevy::math::Vec2::new(
                (f32::from(cell.0) + 0.5) * 32.0,
                (f32::from(cell.1) + 0.5) * 32.0
            )
        );
    }

    #[test]
    fn the_sections_hold_a_gate_and_four_attractions_per_kind_and_six_props() {
        let sim = sim();
        let view = sim.view();
        let db = view.content.as_deref().unwrap();
        for kind in ParkKind::ALL {
            let names: Vec<_> = section_cards(db, Some(kind))
                .iter()
                .map(|c| db.building(c.id).id.clone())
                .collect();
            let stem = match kind {
                ParkKind::CityPark => "city_park",
                ParkKind::Nature => "nature",
                ParkKind::Zoo => "zoo",
                ParkKind::Amusement => "amusement",
            };
            assert_eq!(
                names,
                [
                    format!("{stem}_gate"),
                    format!("{stem}_attr_1"),
                    format!("{stem}_attr_2"),
                    format!("{stem}_attr_3"),
                    format!("{stem}_attr_4"),
                ]
            );
        }
        assert_eq!(section_cards(db, None).len(), 6);
    }

    fn park_frame(
        ctx: &egui::Context,
        view: &SimView,
        outbox: &mut CommandOutbox,
        events: Vec<egui::Event>,
    ) -> Option<egui::Rect> {
        let mut slider = None;
        let mut raw = input(0.0, events);
        raw.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 3000.0),
        ));
        frame(ctx, raw, |ui| {
            park_panel(ui, view, AreaId(1), outbox, &mut slider)
        });
        slider
    }

    /// T-229: the fee slider sends `SetFee` for its area, inside `0..=park_fee_max`, and sends
    /// nothing while it is left alone.
    #[test]
    fn the_fee_slider_sends_set_fee_for_the_area() {
        let (sim, _) = zoo_world(1);
        let view = sim.view();
        let ctx = egui::Context::default();
        let mut outbox = CommandOutbox::default();
        let rect = park_frame(&ctx, &view, &mut outbox, vec![]).expect("the slider is drawn");
        park_frame(&ctx, &view, &mut outbox, vec![]);
        assert_eq!(outbox.iter().count(), 0);
        // The slider rail is the left part of the widget; press a third of the way along it.
        let at = egui::pos2(rect.left() + rect.width() * 0.25, rect.center().y);
        let [down, up] = click(at);
        park_frame(&ctx, &view, &mut outbox, down);
        park_frame(&ctx, &view, &mut outbox, up);
        let sent: Vec<_> = outbox.iter().cloned().collect();
        let max = view.content.as_deref().unwrap().balance().park_fee_max;
        assert!(!sent.is_empty(), "no command");
        for c in sent {
            let SimCommand::Park(cp_park::ParkCommand::SetFee { area, fee }) = c else {
                panic!("{c:?}");
            };
            assert_eq!(area, AreaId(1));
            assert!(fee > 0.0 && fee <= max, "{fee}");
        }
    }

    /// T-229: the summary adds up the area's own gate, and a gate or a prop click maps to the
    /// area; a city park (no area) maps to none.
    #[test]
    fn the_summary_counts_the_gate_and_a_gate_maps_to_its_area() {
        use crate::industry::tests::put;
        let (mut sim, db) = zoo_world(2);
        let at = bevy::math::Vec2::new(1000.0, 1000.0);
        let s = park_summary(&sim.view(), AreaId(1)).unwrap();
        assert_eq!((s.gates, s.buildings, s.level), (0, 0, 2));
        assert_eq!(s.next_xp, Some(db.balance().park_levels[1]));
        let gate = put(&mut sim, &db, "zoo_gate", at);
        let s = park_summary(&sim.view(), AreaId(1)).unwrap();
        assert_eq!((s.gates, s.buildings), (1, 1));
        assert_eq!(s.gate_at, Some(at));
        assert_eq!(area_of_building(&sim.view(), gate), Some(AreaId(1)));
        let city = put(
            &mut sim,
            &db,
            "small_park",
            bevy::math::Vec2::new(3000.0, 3000.0),
        );
        assert_eq!(area_of_building(&sim.view(), city), None);
    }

    /// T-229: the advice for the parks. A gateless area warns and a gate takes it away; a rise
    /// of the level comes once; a town of 2000 with no park hears about it, and one small park
    /// is enough.
    #[test]
    fn park_advice_warns_of_a_gateless_area_and_a_leisureless_town() {
        use crate::advisor::{collect, level_advice};
        use crate::industry::tests::put;
        let (mut sim, db) = zoo_world(1);
        let ids = |sim: &SimRunner| -> Vec<&'static str> {
            let mut v = sim.view();
            std::sync::Arc::make_mut(&mut v.city).population = 2_500;
            collect(&v, &mut None).iter().map(|a| a.id).collect()
        };
        let got = ids(&sim);
        assert!(got.contains(&"park_no_gate"), "{got:?}");
        assert!(got.contains(&"leisure_none"), "{got:?}");
        let at = bevy::math::Vec2::new(1000.0, 1000.0);
        put(&mut sim, &db, "zoo_gate", at);
        let got = ids(&sim);
        assert!(!got.contains(&"park_no_gate"), "{got:?}");
        assert!(!got.contains(&"leisure_none"), "{got:?}");
        // A small town does not hear it; one small park is enough for a big one.
        let (mut sim2, db2) = zoo_world(1);
        let mut v = sim2.view();
        std::sync::Arc::make_mut(&mut v.city).population = 1_999;
        assert!(
            !collect(&v, &mut None)
                .iter()
                .any(|a| a.id == "leisure_none")
        );
        put(
            &mut sim2,
            &db2,
            "small_park",
            bevy::math::Vec2::new(3000.0, 3000.0),
        );
        assert!(!ids(&sim2).contains(&"leisure_none"));

        let mut seen = None;
        assert!(level_advice(&sim.view(), &mut seen).is_empty());
        sim.world_mut()
            .resource_mut::<cp_district::Areas>()
            .get_mut(AreaId(1))
            .unwrap()
            .level = 2;
        let got = level_advice(&sim.view(), &mut seen);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "park_level_up");
        assert!(level_advice(&sim.view(), &mut seen).is_empty());
    }

    /// T-229: a full park gives `park_full` that jumps to its gate.
    #[test]
    fn a_full_park_gives_advice_that_jumps_to_the_gate() {
        use crate::advisor::{Action, collect};
        use crate::industry::tests::put;
        let (mut sim, db) = zoo_world(1);
        let at = bevy::math::Vec2::new(1000.0, 1000.0);
        put(&mut sim, &db, "zoo_gate", at);
        sim.world_mut()
            .resource_mut::<cp_park::ParkState>()
            .areas
            .insert(
                1,
                cp_park::ParkAreaState {
                    capacity: 40,
                    inside: 40,
                    ..Default::default()
                },
            );
        let got = collect(&sim.view(), &mut None);
        let full = got.iter().find(|a| a.id == "park_full").expect("park_full");
        assert!(
            matches!(full.action, Some(Action::Jump(p)) if p == at),
            "{:?}",
            full.action
        );
    }

    /// T-229: the Parks metrics section counts the areas, the gateless ones and the visitors.
    #[test]
    fn the_parks_metrics_section_counts_areas_and_visitors() {
        let (sim, _) = zoo_world(3);
        let secs = crate::metrics::sections(&sim.view());
        let sec = secs.iter().find(|s| s.title == "Parks").expect("Parks");
        let v = |l: &str| sec.rows.iter().find(|(k, _)| k == l).unwrap().1.clone();
        assert_eq!(v("Areas"), "1");
        assert_eq!(v("Area levels"), "3");
        assert_eq!(v("Areas without a gate"), "1");
    }
}
