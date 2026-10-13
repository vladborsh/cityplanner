//! The UI icons (T-082): self-made SVGs in `assets/ui/icons/src`, rendered to 64×64 PNGs in
//! `assets/ui/icons` by `tools/assets/icons.mjs` and compiled in with `include_image!`, so a
//! desktop and a WASM build carry the same bytes and nothing is loaded at run time.
//! [`road_profile`] is the one procedural icon: the cross-section of a road type.

use bevy_egui::egui;
use cp_content::{LaneKind, RoadDef};

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
    // Windows.
    Budget => "budget",
    Demand => "demand",
    Metrics => "metrics",
    Saves => "saves",
    Debug => "debug",
    Info => "info",
}

impl Icon {
    /// An icon image of `size` points.
    pub fn sized(self, size: f32) -> egui::Image<'static> {
        egui::Image::new(self.image()).fit_to_exact_size(egui::vec2(size, size))
    }
}

/// Colour of a lane in the profile icon.
fn lane_colour(kind: LaneKind) -> egui::Color32 {
    match kind {
        LaneKind::Car => egui::Color32::from_gray(70),
        LaneKind::Bus => egui::Color32::from_rgb(150, 70, 60),
        LaneKind::Tram => egui::Color32::from_rgb(120, 120, 130),
        LaneKind::Bike => egui::Color32::from_rgb(60, 130, 80),
        LaneKind::Pedestrian => egui::Color32::from_rgb(170, 160, 140),
    }
}

/// The cross-section of `def`, lanes side by side scaled to `size` (the profile's width spans
/// the icon's width). Lane `offset` is the lateral centre from the axis, positive to the left.
pub fn road_profile(ui: &mut egui::Ui, def: &RoadDef, size: egui::Vec2) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, egui::Color32::from_gray(30));
    let span = def.width.max(0.1);
    let x_of = |lateral: f32| rect.center().x - lateral / span * rect.width();
    let lane_h = rect.height() * 0.7;
    let lane_rect = |centre: f32, width: f32| {
        egui::Rect::from_center_size(
            egui::pos2(x_of(centre), rect.center().y),
            egui::vec2(width / span * rect.width(), lane_h),
        )
    };
    if def.median_width > 0.0 {
        painter.rect_filled(
            lane_rect(0.0, def.median_width),
            0.0,
            egui::Color32::from_rgb(70, 110, 60),
        );
    }
    for lane in &def.lanes {
        let r = lane_rect(lane.offset, lane.width);
        painter.rect_filled(r.shrink2(egui::vec2(0.5, 0.0)), 0.0, lane_colour(lane.kind));
    }
    response.on_hover_text(format!("{} — {:.0} m wide", def.name, def.width))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn icons_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/ui/icons")
    }

    #[test]
    fn every_icon_has_a_64_by_64_png_and_an_svg_source() {
        for icon in Icon::ALL {
            let png = icons_dir().join(format!("{}.png", icon.stem()));
            let (w, h) =
                image::image_dimensions(&png).unwrap_or_else(|e| panic!("{}: {e}", png.display()));
            assert_eq!((w, h), (64, 64), "{}", png.display());
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
    fn stems_are_unique() {
        let mut stems: Vec<_> = Icon::ALL.iter().map(|i| i.stem()).collect();
        stems.sort_unstable();
        let n = stems.len();
        stems.dedup();
        assert_eq!(stems.len(), n);
    }
}
