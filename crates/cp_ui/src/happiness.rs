//! What the UI says and does about contentment (T-318c, `architecture/system-design.md` §19.5):
//! the text of each complaint, the action a click on it runs, the mood's colour and its smiley.
//! The inspector, the district panel, the metrics window, the top bar and the advisor share it.

use bevy_egui::egui;
use cp_building::Complaint;
use cp_render::Overlay;

use crate::advisor::Action;
use crate::palette::Category;
use crate::panels::UiWindow;

/// What a click on a complaint opens: the overlay, window or panel where the player can see or
/// fix its cause.
pub fn action(c: Complaint) -> Option<Action> {
    Some(match c {
        Complaint::None => return None,
        Complaint::NoHealth | Complaint::Sick => Action::Overlay(Overlay::Health),
        Complaint::NoPolice | Complaint::Crime => Action::Overlay(Overlay::Police),
        Complaint::NoFire => Action::Overlay(Overlay::Fire),
        Complaint::NoSchool => Action::Overlay(Overlay::Education),
        Complaint::Pollution | Complaint::BadWater => Action::Overlay(Overlay::Pollution),
        Complaint::Noise => Action::Trees,
        Complaint::NoRoad | Complaint::LongCommute => Action::Overlay(Overlay::Traffic),
        Complaint::NoWater => Action::Overlay(Overlay::Water),
        Complaint::NoPower => Action::Overlay(Overlay::Power),
        Complaint::Garbage => Action::Overlay(Overlay::Garbage),
        Complaint::Dead => Action::Overlay(Overlay::Deathcare),
        Complaint::Jobless => Action::Window(UiWindow::Jobs),
        Complaint::NoPark => Action::Panel(Category::Parks),
        Complaint::HighTax => Action::Window(UiWindow::Budget),
        // T-328: trust is on the corruption axis.
        Complaint::Distrust => Action::Window(UiWindow::Power),
    })
}

/// The hint of a complaint's action (English key; the UI translates it).
pub fn action_hint(c: Complaint) -> &'static str {
    match c {
        Complaint::None => "",
        Complaint::NoHealth | Complaint::Sick => "Show health coverage",
        Complaint::NoPolice | Complaint::Crime => "Show police coverage",
        Complaint::NoFire => "Show fire coverage",
        Complaint::NoSchool => "Show education coverage",
        Complaint::Pollution | Complaint::BadWater => "Show pollution",
        Complaint::Noise => "Show noise; plant trees along the road",
        Complaint::NoRoad | Complaint::LongCommute => "Show traffic",
        Complaint::NoWater => "Show water",
        Complaint::NoPower => "Show power",
        Complaint::Garbage => "Show garbage coverage",
        Complaint::Dead => "Show deathcare coverage",
        Complaint::Jobless => "Open Jobs & education",
        Complaint::NoPark => "Open Parks",
        Complaint::HighTax => "Open the budget",
        Complaint::Distrust => "Open the corruption axis",
    }
}

fn request_key() -> egui::Id {
    egui::Id::new("complaint_action")
}

/// Asks for `action` from a panel that cannot run it itself; `draw_ui` runs it ([`take`]).
pub fn request(ctx: &egui::Context, action: Action) {
    ctx.data_mut(|d| d.insert_temp(request_key(), action));
}

/// The action asked for this frame, if any.
pub fn take(ctx: &egui::Context) -> Option<Action> {
    ctx.data_mut(|d| {
        let a = d.get_temp::<Action>(request_key());
        d.remove::<Action>(request_key());
        a
    })
}

/// A complaint as a link with the hint of its action; a click asks for the action.
pub fn complaint_link(ui: &mut egui::Ui, c: Complaint, text: String) -> egui::Response {
    let r = ui.link(text).on_hover_text(cp_locale::tr(action_hint(c)));
    if r.clicked()
        && let Some(a) = action(c)
    {
        request(ui.ctx(), a);
    }
    r
}

/// The top `n` complaints of a histogram as links, "text (homes)".
pub fn complaint_links(ui: &mut egui::Ui, complaints: &[(Complaint, u32)], n: usize) {
    for &(c, homes) in complaints.iter().take(n) {
        complaint_link(
            ui,
            c,
            cp_locale::trf!(
                plural "{complaint}: {n} homes",
                n = homes,
                complaint = cp_locale::tr(c.label())
            ),
        );
    }
}

/// The three moods of §19.5.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mood {
    Content,
    Unhappy,
    Miserable,
}

impl Mood {
    pub fn of(index: u8) -> Mood {
        match cp_building::mood_word(index) {
            "content" => Mood::Content,
            "unhappy" => Mood::Unhappy,
            _ => Mood::Miserable,
        }
    }

    pub fn colour(self) -> egui::Color32 {
        match self {
            Mood::Content => egui::Color32::from_rgb(90, 190, 90),
            Mood::Unhappy => egui::Color32::from_rgb(230, 180, 50),
            Mood::Miserable => egui::Color32::from_rgb(220, 70, 60),
        }
    }
}

/// A smiley of `index`'s mood, `size` points square: a disc, two eyes and a mouth that smiles,
/// stays flat or frowns.
pub fn smiley(ui: &mut egui::Ui, index: u8, size: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    paint_smiley(ui.painter(), rect, index);
    response
}

pub fn paint_smiley(p: &egui::Painter, rect: egui::Rect, index: u8) {
    let mood = Mood::of(index);
    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.5;
    p.circle_filled(c, r, mood.colour());
    let ink = egui::Color32::from_rgb(40, 30, 20);
    for dx in [-0.35, 0.35] {
        p.circle_filled(c + egui::vec2(dx * r, -0.25 * r), 0.12 * r, ink);
    }
    let bend = match mood {
        Mood::Content => 0.25,
        Mood::Unhappy => 0.0,
        Mood::Miserable => -0.25,
    };
    let stroke = egui::Stroke::new((0.12 * r).max(1.0), ink);
    let at = |t: f32| {
        let x = -0.45 + 0.9 * t;
        // A parabola through the mouth's corners, `bend` deep in the middle.
        let y = 0.35 + bend * (1.0 - (2.0 * t - 1.0).powi(2));
        c + egui::vec2(x * r, y * r)
    };
    let points: Vec<egui::Pos2> = (0..=8).map(|i| at(i as f32 / 8.0)).collect();
    p.add(egui::Shape::line(points, stroke));
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_locale::{Lang, locale};

    /// §19.7 item 7: every complaint has a text and an action, and both are in every language.
    #[test]
    fn every_complaint_has_an_action_and_its_texts_in_every_language() {
        for c in Complaint::ALL {
            assert!(action(c).is_some(), "{c:?}: no action");
            for key in [c.label(), action_hint(c)] {
                assert!(!key.is_empty(), "{c:?}: empty text");
                for l in Lang::ALL {
                    assert!(
                        locale(l).texts.contains_key(key),
                        "{c:?}: {key:?} missing in {}",
                        l.code()
                    );
                }
            }
        }
        assert_eq!(action(Complaint::None), None);
    }

    #[test]
    fn the_mood_follows_the_bands() {
        assert_eq!(Mood::of(100), Mood::Content);
        assert_eq!(Mood::of(60), Mood::Content);
        assert_eq!(Mood::of(59), Mood::Unhappy);
        assert_eq!(Mood::of(35), Mood::Unhappy);
        assert_eq!(Mood::of(34), Mood::Miserable);
        assert_eq!(Mood::of(0), Mood::Miserable);
    }
}
