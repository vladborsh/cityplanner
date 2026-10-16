//! The Transport category (T-157): a section per mode of public transport with the tools of
//! that mode — for buses the line tool and the depot — and, in the Transit view, the labels on
//! the map: the riders waiting at each stop and the length and round trip of the line being
//! drawn.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use cp_content::{ContentDb, RoadClass, RoadDefId, ServiceKind};
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
    let mut out = Vec::new();
    if !depots.is_empty() || content.roads().next().is_some() {
        out.push(Section {
            mode: TransitMode::Bus,
            cards: depots,
            line_tool: true,
            track: None,
        });
    }
    // The track of the first station that lays one under ground: its road, at its depth.
    let track = stations.iter().find_map(|c| {
        let spec = content.building(c.id).track.as_ref()?;
        let road = content.road_id(&spec.road)?;
        let def = content.road(road);
        (def.class == RoadClass::Rail && def.tunnel.is_some()).then(|| TrackTool {
            road,
            name: def.name.clone(),
            depth: spec.depth,
        })
    });
    if let Some(track) = track {
        out.push(Section {
            mode: TransitMode::Metro,
            cards: stations,
            line_tool: true,
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
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let icon = if section.mode == TransitMode::Metro {
                Icon::MetroTrack
            } else {
                Icon::LineBus
            };
            icon.show(ui, Sizes::SECTION_ICON);
            ui.strong(section.title());
        });
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(Sizes::TILE_GAP, Sizes::TILE_GAP);
            if let Some(track) = &section.track {
                let on = tools.active == ActiveTool::Road
                    && tools.road.road == Some(track.road)
                    && tools.road.tunnel_depth.is_some();
                if tool_tile(ui, TileIcon::Icon(Icon::MetroTrack), "Metro track", on)
                    .on_hover_text(format!(
                        "{}: click two points to dig a tunnel {:.0} m deep, and on to go on. \
                         Esc ends the chain. Join the platforms of the stations.",
                        track.name, track.depth
                    ))
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
                        "Metro line",
                        "Click stations to set the stops, click the first station to close the line: trains ride there and back. Backspace takes the last stop away.",
                    )
                } else {
                    (
                        Icon::LineBus,
                        "Bus line",
                        "Click roads to place stops, click the first stop to close the line. Backspace takes the last stop away.",
                    )
                };
                if tool_tile(ui, TileIcon::Icon(icon), name, on)
                    .on_hover_text(hint)
                    .clicked()
                {
                    if on {
                        tools.select(ActiveTool::None);
                    } else {
                        tools.select(ActiveTool::Line);
                        tools.line.mode = section.mode;
                        overlay.active = if metro {
                            Overlay::Off
                        } else {
                            Overlay::Transit
                        };
                    }
                }
            }
            if section.mode == TransitMode::Bus
                && tool_tile(ui, TileIcon::Icon(Icon::Lines), "Lines", *lines_open)
                    .on_hover_text("The Lines window: riders, buses and money of every line.")
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
                    &card.name,
                    on,
                    lock.is_none(),
                )
                .on_hover_text(hover)
                .clicked()
                {
                    select(tools, card.id);
                    if section.mode == TransitMode::Bus {
                        overlay.active = Overlay::Transit;
                    }
                }
            }
        });
    }
}

/// Puts the road tool on the metro track: its type, in a tunnel at the depth of the platforms.
pub fn select_track(tools: &mut ToolState, track: &TrackTool) {
    tools.road.road = Some(track.road);
    tools.road.tunnel_depth = Some(track.depth);
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
        let s = sections(&c);
        assert_eq!(s.len(), 2, "bus and metro; train comes with T-166");
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
    }

    #[test]
    fn selecting_the_track_puts_the_road_tool_in_a_tunnel() {
        let c = content();
        let track = sections(&c)[1].track.clone().unwrap();
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
