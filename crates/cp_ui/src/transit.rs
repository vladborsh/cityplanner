//! The Transport category (T-157): a section per mode of public transport with the tools of
//! that mode — for buses the line tool and the depot — and, in the Transit view, the labels on
//! the map: the riders waiting at each stop and the length and round trip of the line being
//! drawn.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use cp_content::{ContentDb, RoadClass, RoadDefId, ServiceKind};
use cp_locale::{tr, trf};
use cp_render::{Overlay, OverlayState, RtsCamera};
use cp_sim::SimView;
use cp_tools::{ActiveTool, ToolState, route_summary};
use cp_transit::TransitMode;

use crate::buildings::{Card, groups, lock_text, select};
use crate::icons::Icon;
use crate::style::Sizes;
use crate::tile::{TileIcon, tool_tile, tool_tile_enabled};

/// The track tool of a section (metro, T-164): the road type it draws and how deep.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackTool {
    pub road: RoadDefId,
    pub name: String,
    pub depth: f32,
}

/// One section of the panel: a mode, its buildings, whether it has a line tool and its track.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub mode: TransitMode,
    pub cards: Vec<Card>,
    pub line_tool: bool,
    pub track: Option<TrackTool>,
}

impl Section {
    pub fn title(&self) -> &'static str {
        match self.mode {
            TransitMode::Bus => "Bus",
            TransitMode::Metro => "Metro",
            TransitMode::Train => "Train",
            TransitMode::Tram => "Tram",
            TransitMode::Trolley => "Trolleybus",
        }
    }
}

/// The sections to draw. Buses have the line tool and the buildings of the `Transit` service
/// kind without a platform (the depot); metro has its track tool, the line tool and the
/// stations (the buildings that lay a platform of a `Rail` road); train comes with T-166 and
/// stays hidden while the content has no station for it.
pub fn sections(content: &ContentDb) -> Vec<Section> {
    let (stations, depots): (Vec<Card>, Vec<Card>) = groups(content)
        .into_iter()
        .filter(|g| g.kind == ServiceKind::Transit)
        .flat_map(|g| g.cards)
        .partition(|c| content.building(c.id).track.is_some());
    // A station with its platform on the ground belongs to the trains (T-166), one under the
    // ground to the metro.
    let (trains, stations): (Vec<Card>, Vec<Card>) = stations.into_iter().partition(|c| {
        content
            .building(c.id)
            .track
            .as_ref()
            .is_some_and(|t| t.depth <= 0.0)
    });
    // A depot serves the mode its service names (T-302b, T-302c: the tram and the trolleybus
    // depots have their own sections).
    let mode_of = |c: &Card| {
        content
            .building(c.id)
            .service
            .as_deref()
            .and_then(|s| content.service_id(s))
            .map_or(cp_content::TransitKind::Bus, |s| {
                content.service(s).transit_mode
            })
    };
    let (trams, depots): (Vec<Card>, Vec<Card>) = depots
        .into_iter()
        .partition(|c| mode_of(c) == cp_content::TransitKind::Tram);
    let (trolleys, depots): (Vec<Card>, Vec<Card>) = depots
        .into_iter()
        .partition(|c| mode_of(c) == cp_content::TransitKind::Trolley);
    let mut out = Vec::new();
    if !depots.is_empty() || content.roads().next().is_some() {
        out.push(Section {
            mode: TransitMode::Bus,
            cards: depots,
            line_tool: true,
            track: None,
        });
    }
    // The rails and the wires are roads with tram or trolleybus lanes (Roads panel); each
    // section has the depot and the line tool.
    for (mode, cards) in [(TransitMode::Tram, trams), (TransitMode::Trolley, trolleys)] {
        if !cards.is_empty() {
            out.push(Section {
                mode,
                cards,
                line_tool: true,
                track: None,
            });
        }
    }
    // The track of the first station that lays one under ground: its road, at its depth.
    let track_of = |cards: &[Card], tunnel: bool| {
        cards.iter().find_map(|c| {
            let spec = content.building(c.id).track.as_ref()?;
            let road = content.road_id(&spec.road)?;
            let def = content.road(road);
            (def.class == RoadClass::Rail && (!tunnel || def.tunnel.is_some())).then(|| TrackTool {
                road,
                name: def.name.clone(),
                depth: spec.depth,
            })
        })
    };
    let track = track_of(&stations, true);
    let train_track = track_of(&trains, false);
    if let Some(track) = track {
        out.push(Section {
            mode: TransitMode::Metro,
            cards: stations,
            line_tool: true,
            track: Some(track),
        });
    }
    // The line tool of the trains comes with T-167.
    if let Some(track) = train_track {
        out.push(Section {
            mode: TransitMode::Train,
            cards: trains,
            line_tool: false,
            track: Some(track),
        });
    }
    out
}

/// Whether the category has anything to offer.
pub fn has_tools(content: Option<&ContentDb>) -> bool {
    content.is_some_and(|c| !sections(c).is_empty())
}

/// Riders waiting at a stop (T-159).
pub fn waiting_at(view: &SimView, stop: cp_transit::StopId) -> u32 {
    view.transit
        .net
        .riders
        .values()
        .filter(|j| {
            matches!(j.stage, cp_transit::Stage::Waiting { .. })
                && j.ride().is_some_and(|r| r.board == stop)
        })
        .count() as u32
}

/// The icon of a mode's section and tab.
pub fn section_icon(mode: TransitMode) -> Icon {
    match mode {
        TransitMode::Metro => Icon::MetroTrack,
        TransitMode::Train => Icon::RailTrack,
        TransitMode::Tram => Icon::LineTram,
        TransitMode::Trolley => Icon::LineTrolley,
        _ => Icon::LineBus,
    }
}

pub fn transit_panel(
    ui: &mut egui::Ui,
    view: Option<&SimView>,
    tools: &mut ToolState,
    overlay: &mut OverlayState,
    lines_open: &mut bool,
) {
    let Some(content) = view.and_then(|v| v.content.as_deref()) else {
        return;
    };
    for section in sections(content) {
        if !crate::tray::is_active(ui.ctx(), &tr(section.title())) {
            continue;
        }
        if !crate::tray::in_tray(ui.ctx()) {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                section_icon(section.mode).show(ui, Sizes::SECTION_ICON);
                ui.strong(tr(section.title()));
            });
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(Sizes::TILE_GAP, Sizes::TILE_GAP);
            if let Some(track) = &section.track {
                let on = tools.active == ActiveTool::Road
                    && tools.road.road == Some(track.road)
                    && tools.road.tunnel_depth.is_some() == (track.depth > 0.0);
                let (icon, name, hint) = if track.depth > 0.0 {
                    (
                        Icon::MetroTrack,
                        tr("Metro track"),
                        trf!(
                            "{name}: click two points to dig a tunnel {depth} m deep, and on to go on. Esc ends the chain. Join the platforms of the stations.",
                            name = tr(&track.name),
                            depth = format!("{:.0}", track.depth)
                        ),
                    )
                } else {
                    (
                        Icon::RailTrack,
                        tr("Railway"),
                        trf!(
                            "{name}: click two points to lay a track on the ground, and on to go on. Esc ends the chain. Join the railway from the map edge to a station.",
                            name = tr(&track.name)
                        ),
                    )
                };
                if tool_tile(ui, TileIcon::Icon(icon), &name, on)
                    .on_hover_text(hint)
                    .clicked()
                {
                    if on {
                        tools.select(ActiveTool::None);
                    } else {
                        tools.select(ActiveTool::Road);
                        select_track(tools, track);
                    }
                }
            }
            if section.line_tool {
                let metro = section.mode == TransitMode::Metro;
                let on = tools.active == ActiveTool::Line && (tools.line.mode == section.mode);
                let (icon, name, hint) = if metro {
                    (
                        Icon::LineMetro,
                        tr("Metro line"),
                        tr("Click stations to set the stops, click the first station to close the line: trains ride there and back. Backspace takes the last stop away."),
                    )
                } else if section.mode == TransitMode::Tram {
                    (
                        Icon::LineTram,
                        tr("Tram line"),
                        tr("Click roads with tram rails to place stops, click the first stop to close the line. Backspace takes the last stop away."),
                    )
                } else if section.mode == TransitMode::Trolley {
                    (
                        Icon::LineTrolley,
                        tr("Trolleybus line"),
                        tr("Click roads with trolleybus wires to place stops, click the first stop to close the line. Backspace takes the last stop away."),
                    )
                } else {
                    (
                        Icon::LineBus,
                        tr("Bus line"),
                        tr("Click roads to place stops, click the first stop to close the line. Backspace takes the last stop away."),
                    )
                };
                // T-302: station first — the button is grey with the reason until the
                // mode's depot (and track) stand; the sim refuses the line too.
                let ready = view
                    .map(|v| cp_transit::prerequisites(content, &v.buildings, &v.net))
                    .map(|r| r[section.mode.kind() as usize]);
                let enabled = ready.is_none_or(|r| r.ok);
                let hint = match ready {
                    Some(r) if !r.ok => tr(r.missing),
                    _ => hint,
                };
                if tool_tile_enabled(ui, TileIcon::Icon(icon), &name, on, enabled)
                    .on_hover_text(hint)
                    .clicked()
                    && enabled
                {
                    if on {
                        tools.select(ActiveTool::None);
                    } else {
                        tools.select(ActiveTool::Line);
                        tools.line.mode = section.mode;
                        if metro {
                            overlay.close();
                        } else {
                            overlay.open_auto(Overlay::Transit);
                        }
                    }
                }
            }
            if section.mode == TransitMode::Bus
                && tool_tile(ui, TileIcon::Icon(Icon::Lines), &tr("Lines"), *lines_open)
                    .on_hover_text(tr("The Lines window: riders, buses and money of every line."))
                    .clicked()
            {
                *lines_open = !*lines_open;
            }
            for card in &section.cards {
                let on = tools.active == ActiveTool::Place && tools.building.def == Some(card.id);
                let lock = view.and_then(|v| lock_text(v, content, card.id));
                let hover = match &lock {
                    Some(l) => format!("{l}\n{}", crate::buildings::card_text(card)),
                    None => crate::buildings::card_text(card),
                };
                if tool_tile_enabled(
                    ui,
                    TileIcon::Def(&card.icon),
                    &tr(&card.name),
                    on,
                    lock.is_none(),
                )
                .on_hover_text(hover)
                .clicked()
                {
                    select(tools, card.id);
                    if section.mode.on_road() {
                        overlay.open_auto(Overlay::Transit);
                    }
                }
            }
        });
    }
}

/// Puts the road tool on the metro track: its type, in a tunnel at the depth of the platforms.
pub fn select_track(tools: &mut ToolState, track: &TrackTool) {
    tools.road.road = Some(track.road);
    tools.road.tunnel_depth = (track.depth > 0.0).then_some(track.depth);
    tools.road.level = 0;
}

/// The map labels of the Transit view: the waiting count at every stop, and the length and
/// round trip of the line being drawn beside the cursor.
pub fn transit_labels(
    mut contexts: EguiContexts,
    view: Option<Res<SimView>>,
    overlay: Res<OverlayState>,
    tools: Res<ToolState>,
    cameras: Query<(&Camera, &GlobalTransform), With<RtsCamera>>,
) {
    let Some(view) = view else { return };
    if overlay.active != Overlay::Transit {
        return;
    }
    let (Ok(ctx), Ok((camera, transform))) = (contexts.ctx_mut(), cameras.single()) else {
        return;
    };
    let Some(terrain) = view.terrain.as_deref() else {
        return;
    };
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("transit_labels"),
    ));
    let label = |at: Vec2, text: String, size: f32, lift: f32| {
        let Ok(p) =
            camera.world_to_viewport(transform, Vec3::new(at.x, terrain.height(at) + lift, at.y))
        else {
            return;
        };
        let pos = egui::pos2(p.x, p.y);
        let font = egui::FontId::proportional(size);
        painter.text(
            pos + egui::vec2(1.0, 1.0),
            egui::Align2::CENTER_BOTTOM,
            &text,
            font.clone(),
            egui::Color32::BLACK,
        );
        painter.text(
            pos,
            egui::Align2::CENTER_BOTTOM,
            text,
            font,
            egui::Color32::WHITE,
        );
    };
    for (id, stop) in view.transit.net.stops.iter() {
        label(stop.pos, waiting_at(&view, id).to_string(), 14.0, 6.0);
    }
    if tools.active == ActiveTool::Line
        && let Some(g) = &tools.line_ghost
        && let Some((at, _)) = g.hover
        && g.route.len() > 1
    {
        label(at, route_summary(g), 16.0, 14.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content() -> ContentDb {
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap()
    }

    #[test]
    fn the_bus_section_has_the_line_tool_and_the_depot_and_the_metro_one_the_track_and_stations() {
        let c = content();
        let all = sections(&c);
        let modes: Vec<TransitMode> = all.iter().map(|s| s.mode).collect();
        assert_eq!(
            modes,
            [
                TransitMode::Bus,
                TransitMode::Tram,
                TransitMode::Trolley,
                TransitMode::Metro,
                TransitMode::Train
            ]
        );
        // T-302b: the tram depot has its own section with the line tool; the rails are roads.
        let tram = &all[1];
        assert!(tram.line_tool && tram.track.is_none());
        assert!(tram.cards.iter().any(|c| c.name == "Tram Depot"));
        assert!(!all[0].cards.iter().any(|c| c.name == "Tram Depot"));
        // T-302c: so has the trolleybus depot.
        let trolley = &all[2];
        assert!(trolley.line_tool && trolley.track.is_none());
        assert!(trolley.cards.iter().any(|c| c.name == "Trolleybus Depot"));
        assert!(!all[0].cards.iter().any(|c| c.name == "Trolleybus Depot"));
        let s = [&all[0], &all[3], &all[4]];
        assert_eq!(s[0].mode, TransitMode::Bus);
        assert!(s[0].line_tool && s[0].track.is_none());
        assert!(s[0].cards.iter().any(|c| c.name == "Bus Depot"));
        assert!(
            !s[0].cards.iter().any(|c| c.name == "Metro Station"),
            "a station is not a depot"
        );
        assert_eq!(s[1].mode, TransitMode::Metro);
        assert!(s[1].line_tool);
        assert!(s[1].cards.iter().any(|c| c.name == "Metro Station"));
        let track = s[1].track.as_ref().expect("the metro track tool");
        assert_eq!(c.road(track.road).id, "metro_track");
        assert_eq!(track.depth, 12.0);
        assert!(has_tools(Some(&c)) && !has_tools(None));
        // T-166: the trains have the railway and the station, no line tool yet.
        assert_eq!(s[2].mode, TransitMode::Train);
        assert!(!s[2].line_tool);
        assert!(s[2].cards.iter().any(|c| c.name == "Train Station"));
        assert!(!s[1].cards.iter().any(|c| c.name == "Train Station"));
        let rail = s[2].track.as_ref().expect("the railway tool");
        assert_eq!(
            (c.road(rail.road).id.as_str(), rail.depth),
            ("rail_2t", 0.0)
        );
    }

    #[test]
    fn selecting_the_track_puts_the_road_tool_in_a_tunnel() {
        let c = content();
        let track = sections(&c)
            .into_iter()
            .find(|s| s.mode == TransitMode::Metro)
            .and_then(|s| s.track)
            .unwrap();
        let mut tools = ToolState::default();
        tools.select(ActiveTool::Road);
        select_track(&mut tools, &track);
        assert_eq!(
            tools.road.elevation(),
            cp_net::ElevationMode::Tunnel(track.depth)
        );
        assert_eq!(tools.road.road, Some(track.road));
    }
}
