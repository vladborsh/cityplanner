//! The save / load window (T-046). The UI never touches the sim or the disk: it fills
//! [`FileMenu::request`], and the app (`cp_app::saves`) carries it out, refreshes the file
//! list and writes the outcome to [`FileMenu::status`].

use bevy::prelude::*;
use bevy_egui::egui;

use crate::icons::Icon;

/// One save file as the list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveEntry {
    pub name: String,
    pub bytes: u64,
    /// Sim tick the game was saved at (from the file header).
    pub tick: u64,
}

/// What the player asked for; the app takes it with `Option::take`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileRequest {
    Save(String),
    Load(String),
    /// Re-read the list of files.
    Refresh,
}

#[derive(Resource, Default, Debug)]
pub struct FileMenu {
    pub open: bool,
    /// The name typed for a new save.
    pub name: String,
    pub files: Vec<SaveEntry>,
    pub request: Option<FileRequest>,
    /// The outcome of the last request, shown under the list.
    pub status: String,
    /// Where the files live, shown in the window.
    pub dir: String,
}

/// Where the widgets of the last frame were (tests click them).
#[derive(Clone, Debug, Default)]
pub struct FileRects {
    pub save: Option<egui::Rect>,
    pub loads: Vec<(String, egui::Rect)>,
}

/// What the top bar's saves icon does: opens or closes the window and, on opening, asks for
/// a fresh list.
pub fn toggle_saves(menu: &mut FileMenu) {
    menu.open = !menu.open;
    if menu.open {
        menu.request = Some(FileRequest::Refresh);
    }
}

/// The save / load window (opened by the saves icon in the top bar).
pub fn file_window(ctx: &egui::Context, menu: &mut FileMenu) -> FileRects {
    let mut rects = FileRects::default();
    let mut open = menu.open;
    crate::frame::window("file_menu", Icon::Saves, "Save / Load", &mut open)
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-8.0, 40.0))
        .show(ctx, |ui| {
            ui.weak(&menu.dir);
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut menu.name);
                let r = ui.add_enabled(!menu.name.trim().is_empty(), egui::Button::new("Save"));
                rects.save = Some(r.rect);
                if r.clicked() {
                    menu.request = Some(FileRequest::Save(menu.name.trim().to_string()));
                }
            });
            ui.separator();
            if menu.files.is_empty() {
                ui.weak("No saves yet.");
            }
            for f in &menu.files {
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "{}  ({} kB, tick {})",
                        f.name,
                        f.bytes / 1024,
                        f.tick
                    ));
                    let r = ui.button("Load");
                    rects.loads.push((f.name.clone(), r.rect));
                    if r.clicked() {
                        menu.request = Some(FileRequest::Load(f.name.clone()));
                    }
                });
            }
            if !menu.status.is_empty() {
                ui.separator();
                ui.label(&menu.status);
            }
        });
    menu.open = open;
    rects
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(t: f64, events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            time: Some(t),
            events,
            ..Default::default()
        }
    }

    /// One frame of the window with `events`; returns where its widgets are.
    fn frame(
        ctx: &egui::Context,
        menu: &mut FileMenu,
        t: f64,
        events: Vec<egui::Event>,
    ) -> FileRects {
        let mut rects = FileRects::default();
        crate::testutil::frame(ctx, input(t, events), |ui| {
            rects = file_window(ui.ctx(), menu);
        });
        rects
    }

    fn click(ctx: &egui::Context, menu: &mut FileMenu, pos: egui::Pos2, t: f64) {
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(
            ctx,
            menu,
            t + 0.1,
            vec![egui::Event::PointerMoved(pos), button(true)],
        );
        frame(ctx, menu, t + 0.2, vec![button(false)]);
    }

    /// Two frames so the layout has settled; returns the second one's rectangles.
    fn layout(ctx: &egui::Context, menu: &mut FileMenu, t: f64) -> FileRects {
        frame(ctx, menu, t, vec![]);
        frame(ctx, menu, t + 0.05, vec![])
    }

    #[test]
    fn toggling_opens_the_window_and_asks_for_the_list() {
        let ctx = egui::Context::default();
        let mut menu = FileMenu::default();
        let r = layout(&ctx, &mut menu, 0.0);
        assert!(!menu.open && r.save.is_none());
        toggle_saves(&mut menu);
        assert!(menu.open);
        assert_eq!(menu.request.take(), Some(FileRequest::Refresh));
        assert!(layout(&ctx, &mut menu, 1.0).save.is_some());
        toggle_saves(&mut menu);
        assert!(!menu.open);
        assert_eq!(menu.request, None);
    }

    #[test]
    fn save_and_load_buttons_make_requests() {
        let ctx = egui::Context::default();
        let mut menu = FileMenu {
            open: true,
            name: "  harbour ".into(),
            files: vec![
                SaveEntry {
                    name: "a".into(),
                    bytes: 4096,
                    tick: 10,
                },
                SaveEntry {
                    name: "b".into(),
                    bytes: 8192,
                    tick: 20,
                },
            ],
            ..Default::default()
        };
        let r = layout(&ctx, &mut menu, 0.0);
        click(&ctx, &mut menu, r.save.unwrap().center(), 1.0);
        assert_eq!(
            menu.request.take(),
            Some(FileRequest::Save("harbour".into()))
        );
        let r = layout(&ctx, &mut menu, 2.0);
        let (_, rect) = r.loads.iter().find(|(n, _)| n == "b").unwrap();
        click(&ctx, &mut menu, rect.center(), 3.0);
        assert_eq!(menu.request.take(), Some(FileRequest::Load("b".into())));
    }

    #[test]
    fn an_empty_name_cannot_be_saved() {
        let ctx = egui::Context::default();
        let mut menu = FileMenu {
            open: true,
            name: "   ".into(),
            ..Default::default()
        };
        let r = layout(&ctx, &mut menu, 0.0);
        click(&ctx, &mut menu, r.save.unwrap().center(), 1.0);
        assert_eq!(menu.request, None);
    }
}
