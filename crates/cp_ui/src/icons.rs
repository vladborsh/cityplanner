//! The UI icons (T-082): self-made SVGs in `assets/ui/icons/src`, rendered to 128×128 PNGs in
//! `assets/ui/icons` by `tools/assets/icons.mjs` and compiled in with `include_image!`, so a
//! desktop and a WASM build carry the same bytes and nothing is loaded at run time.
//! [`road_tile`] is the procedural icon of a road type.

use bevy_egui::egui;
use cp_content::{LaneKind, RoadClass, RoadDef};

macro_rules! icons {
    ($($variant:ident => $file:literal),+ $(,)?) => {
        /// One icon of the UI.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Icon {
            $($variant),+
        }

        impl Icon {
            pub const ALL: &[Icon] = &[$(Icon::$variant),+];

            /// File stem under `assets/ui/icons` (`<stem>.png`, source `src/<stem>.svg`).
            pub fn stem(self) -> &'static str {
                match self {
                    $(Icon::$variant => $file),+
                }
            }

            /// The compiled-in PNG.
            pub fn image(self) -> egui::ImageSource<'static> {
                match self {
                    $(Icon::$variant => egui::include_image!(
                        concat!("../../../assets/ui/icons/", $file, ".png")
                    )),+
                }
            }
        }
    };
}

icons! {
    // Palette categories.
    Roads => "roads",
    Zones => "zones",
    Water => "water",
    Power => "power",
    Buildings => "buildings",
    Bulldozer => "bulldozer",
    // Tools.
    Road => "road",
    Curve => "curve",
    FreeRoad => "free",
    ZoneFill => "zone_fill",
    ZoneBrushSmall => "zone_brush_s",
    ZoneBrushLarge => "zone_brush_l",
    ZoneRect => "zone_rect",
    ZoneEraser => "zone_eraser",
    PipeWater => "pipe_water",
    PipeSewage => "pipe_sewage",
    PowerLine => "power_line",
    Remove => "remove",
    Building => "building",
    // Zone types (T-099): one silhouette each.
    ZoneResLow => "zone_res_low",
    ZoneResHigh => "zone_res_high",
    ZoneComLow => "zone_com_low",
    ZoneComHigh => "zone_com_high",
    ZoneIndustrial => "zone_industrial",
    ZoneOffice => "zone_office",
    // Windows.
    Budget => "budget",
    Demand => "demand",
    Metrics => "metrics",
    Saves => "saves",
    Debug => "debug",
    Info => "info",
}

impl Icon {
    /// An icon of `size` points as a widget with a fixed footprint: the space is taken even
    /// while the image is still loading (or cannot load), so layouts do not jump.
    pub fn show(self, ui: &mut egui::Ui, size: f32) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
        if ui.is_rect_visible(rect) {
            egui::Image::new(self.image()).paint_at(ui, rect);
        }
        response
    }

    /// An icon button of exactly `size` + `pad` on each side (palette, top bar).
    pub fn button(self, ui: &mut egui::Ui, size: f32, pad: f32, selected: bool) -> egui::Response {
        let side = size + 2.0 * pad;
        let (rect, response) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::click());
        if ui.is_rect_visible(rect) {
            let visuals = ui.style().interact_selectable(&response, selected);
            ui.painter().rect(
                rect,
                visuals.corner_radius,
                visuals.weak_bg_fill,
                visuals.bg_stroke,
                egui::StrokeKind::Inside,
            );
            egui::Image::new(self.image()).paint_at(ui, rect.shrink(pad));
        }
        response
    }

    /// An icon image of `size` points.
    pub fn sized(self, size: f32) -> egui::Image<'static> {
        egui::Image::new(self.image()).fit_to_exact_size(egui::vec2(size, size))
    }
}

/// The icon of a content def by name (`BuildingDef.icon`, `UtilityDef.icon`, T-105). Until the
/// per-def icons exist (T-105) every name maps to the generic building icon.
pub fn def_icon(_name: &str) -> egui::ImageSource<'static> {
    Icon::Building.image()
}

/// The icon of a zone type (the six that can be painted); `None` has none.
pub fn zone_icon(zone: cp_content::ZoneType) -> Icon {
    use cp_content::ZoneType as Z;
    match zone {
        Z::ResLow => Icon::ZoneResLow,
        Z::ResHigh => Icon::ZoneResHigh,
        Z::ComLow => Icon::ZoneComLow,
        Z::ComHigh => Icon::ZoneComHigh,
        Z::Industrial => Icon::ZoneIndustrial,
        Z::Office | Z::None => Icon::ZoneOffice,
    }
}

/// Colour of the class corner of a road tile.
pub fn class_colour(class: RoadClass) -> egui::Color32 {
    match class {
        RoadClass::Local => egui::Color32::from_rgb(160, 160, 175),
        RoadClass::Collector => egui::Color32::from_rgb(90, 150, 225),
        RoadClass::Arterial => egui::Color32::from_rgb(235, 175, 60),
        RoadClass::Highway => egui::Color32::from_rgb(215, 80, 70),
    }
}

/// Share of the square's width the widest road of the content takes.
const ROAD_TILE_FILL: f32 = 0.8;
const GRASS: egui::Color32 = egui::Color32::from_rgb(76, 120, 66);
const ASPHALT: egui::Color32 = egui::Color32::from_gray(62);
const MARK_YELLOW: egui::Color32 = egui::Color32::from_rgb(235, 190, 50);

/// The icon of a road type (T-098): a straight piece of the road seen **from straight above**,
/// running vertically through the whole square on grass, as wide as the road is (the widest
/// road of the content takes 80 % of the square: `widest` metres). Lanes keep their colours,
/// a yellow centre line divides opposing directions, a white dash lines separate lanes of one
/// direction, one-way roads carry arrows, a coloured corner names the class and a crossed
/// square marks a road that cannot be zoned.
pub fn road_tile_scaled(painter: &egui::Painter, def: &RoadDef, rect: egui::Rect, widest: f32) {
    let px = rect.width() * ROAD_TILE_FILL / widest.max(0.1);
    let x_of = |lateral: f32| rect.center().x - lateral * px;
    let band = |lo: f32, hi: f32| {
        egui::Rect::from_min_max(
            egui::pos2(x_of(hi), rect.top()),
            egui::pos2(x_of(lo), rect.bottom()),
        )
    };
    painter.rect_filled(rect, 0.0, GRASS);
    let half = def.width * 0.5;
    painter.rect_filled(band(-half, half), 0.0, ASPHALT);
    if def.median_width > 0.0 {
        painter.rect_filled(
            band(-def.median_width * 0.5, def.median_width * 0.5),
            0.0,
            egui::Color32::from_rgb(70, 110, 60),
        );
    }
    for lane in &def.lanes {
        let colour = match lane.kind {
            LaneKind::Car => ASPHALT,
            LaneKind::Bus => egui::Color32::from_rgb(150, 70, 60),
            LaneKind::Tram => egui::Color32::from_rgb(120, 120, 130),
            LaneKind::Bike => egui::Color32::from_rgb(60, 140, 85),
            LaneKind::Pedestrian => egui::Color32::from_rgb(190, 182, 165),
        };
        let (lo, hi) = (
            lane.offset - lane.width * 0.5,
            lane.offset + lane.width * 0.5,
        );
        painter.rect_filled(band(lo, hi), 0.0, colour);
    }
    // Markings between neighbouring vehicle lanes.
    let mut cars: Vec<&cp_content::LaneDef> = def
        .lanes
        .iter()
        .filter(|l| matches!(l.kind, LaneKind::Car | LaneKind::Bus))
        .collect();
    cars.sort_by(|a, b| a.offset.total_cmp(&b.offset));
    for pair in cars.windows(2) {
        let x = x_of((pair[0].offset + pair[1].offset) * 0.5);
        if pair[0].dir != pair[1].dir {
            let w = (px * 0.2).max(1.5);
            painter.rect_filled(
                egui::Rect::from_x_y_ranges(x - w..=x + w, rect.y_range()),
                0.0,
                MARK_YELLOW,
            );
        } else {
            let dash = rect.height() / 10.0;
            let mut y = rect.top() + dash * 0.5;
            while y < rect.bottom() {
                painter.rect_filled(
                    egui::Rect::from_x_y_ranges(
                        x - 0.7..=x + 0.7,
                        y..=(y + dash * 0.5).min(rect.bottom()),
                    ),
                    0.0,
                    egui::Color32::from_gray(225),
                );
                y += dash;
            }
        }
    }
    // Direction arrows (travel runs up the tile) on the lanes of a one-way road.
    if def.is_one_way() {
        for lane in cars.iter().filter(|l| l.kind == LaneKind::Car) {
            let w = (lane.width * px * 0.55).clamp(4.0, 12.0);
            for f in [0.3, 0.72] {
                let (cx, top) = (x_of(lane.offset), rect.top() + rect.height() * f);
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        egui::pos2(cx, top),
                        egui::pos2(cx + w * 0.5, top + w),
                        egui::pos2(cx - w * 0.5, top + w),
                    ],
                    egui::Color32::from_gray(235),
                    egui::Stroke::NONE,
                ));
            }
        }
    }
    // The class, as a coloured corner bar.
    painter.rect_filled(
        egui::Rect::from_min_size(rect.min + egui::vec2(3.0, 3.0), egui::vec2(14.0, 5.0)),
        2.0,
        class_colour(def.class),
    );
    if !def.allows_zoning {
        let r = egui::Rect::from_min_size(
            egui::pos2(rect.right() - 15.0, rect.top() + 3.0),
            egui::vec2(12.0, 12.0),
        );
        painter.rect_filled(r, 2.0, egui::Color32::from_rgb(170, 60, 50));
        painter.line_segment(
            [r.left_bottom(), r.right_top()],
            egui::Stroke::new(1.5, egui::Color32::WHITE),
        );
    }
}

/// [`road_tile_scaled`] with `def` itself as the widest road.
pub fn road_tile(painter: &egui::Painter, def: &RoadDef, rect: egui::Rect) {
    road_tile_scaled(painter, def, rect, def.width);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn icons_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/ui/icons")
    }

    #[test]
    fn every_icon_has_a_128_by_128_png_and_an_svg_source() {
        for icon in Icon::ALL {
            let png = icons_dir().join(format!("{}.png", icon.stem()));
            let (w, h) =
                image::image_dimensions(&png).unwrap_or_else(|e| panic!("{}: {e}", png.display()));
            assert_eq!((w, h), (128, 128), "{}", png.display());
            let svg = icons_dir().join("src").join(format!("{}.svg", icon.stem()));
            assert!(svg.is_file(), "missing source {}", svg.display());
        }
    }

    #[test]
    fn the_compiled_in_pngs_decode_through_the_egui_image_loader() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        for icon in Icon::ALL {
            let image = icon.sized(32.0);
            let mut done = false;
            for _ in 0..500 {
                match image.load_for_size(&ctx, egui::vec2(32.0, 32.0)) {
                    Ok(egui::load::TexturePoll::Ready { texture }) => {
                        assert!(texture.size.x > 0.0, "{icon:?}");
                        done = true;
                        break;
                    }
                    Ok(egui::load::TexturePoll::Pending { .. }) => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(e) => panic!("{icon:?}: {e}"),
                }
            }
            assert!(done, "{icon:?} did not finish loading");
        }
    }

    #[test]
    fn no_png_without_an_icon_variant() {
        let stems: Vec<_> = Icon::ALL.iter().map(|i| i.stem()).collect();
        for entry in std::fs::read_dir(icons_dir()).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "png") {
                let stem = path.file_stem().unwrap().to_str().unwrap().to_string();
                assert!(stems.contains(&stem.as_str()), "{stem}.png has no Icon");
            }
        }
    }

    #[test]
    fn the_six_zone_icons_are_pairwise_different_files() {
        use cp_content::ZoneType as Z;
        let zones = [
            Z::ResLow,
            Z::ResHigh,
            Z::ComLow,
            Z::ComHigh,
            Z::Industrial,
            Z::Office,
        ];
        let bytes: Vec<Vec<u8>> = zones
            .iter()
            .map(|&z| {
                std::fs::read(icons_dir().join(format!("{}.png", zone_icon(z).stem()))).unwrap()
            })
            .collect();
        for i in 0..bytes.len() {
            for j in i + 1..bytes.len() {
                assert_ne!(
                    bytes[i], bytes[j],
                    "{:?} and {:?} are the same picture",
                    zones[i], zones[j]
                );
            }
        }
    }

    #[test]
    fn stems_are_unique() {
        let mut stems: Vec<_> = Icon::ALL.iter().map(|i| i.stem()).collect();
        stems.sort_unstable();
        let n = stems.len();
        stems.dedup();
        assert_eq!(stems.len(), n);
    }
}
