//! The Jobs & education window (`architecture/system-design.md` §13.6a, T-307): per education
//! level the residents, how the adults split into working, looking and studying or retired, and
//! the job slots that need the level; one verdict line; the 30-day unemployment series. All read
//! from [`cp_sim::CityStats`]; [`verdict`] is the pure part.

use bevy_egui::egui;
use cp_locale::{tr, trf};
use cp_sim::SimView;
use cp_sim::stats::CityStats;

use crate::frame;
use crate::icons::Icon;
use crate::metrics::sparkline;

/// A group of residents or slots smaller than this is noise, not a verdict.
pub const VERDICT_MIN: u32 = 5;
/// Width of the longest bar.
const BAR_W: f32 = 200.0;
const BAR_H: f32 = 14.0;

const EMPLOYED: egui::Color32 = egui::Color32::from_rgb(92, 184, 92);
const LOOKING: egui::Color32 = egui::Color32::from_rgb(220, 90, 70);
const OTHER: egui::Color32 = egui::Color32::from_rgb(110, 130, 160);
const JOBS: egui::Color32 = egui::Color32::from_rgb(90, 150, 220);

/// What the verdict line says; the first branch that fits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Residents of `level` look for work and the slots they may take are too few.
    NoJobs {
        level: usize,
        looking: u32,
        free: u32,
    },
    /// More free slots than jobless residents of any kind.
    NoWorkers,
    /// Free slots for the educated while the jobless are the uneducated.
    NoEducated,
    Balanced,
}

/// Free slots a resident of `level` may take: those that need `level` or less.
fn available(s: &CityStats, level: usize) -> u32 {
    s.jobs[..=level].iter().map(|j| j.free).sum()
}

/// The first branch that fits `stats` (a pure function of the summary).
pub fn verdict(s: &CityStats) -> Verdict {
    for level in 0..4 {
        let looking = s.education[level].looking;
        let free = available(s, level);
        if looking > free + VERDICT_MIN {
            return Verdict::NoJobs {
                level,
                looking,
                free,
            };
        }
    }
    let looking: u32 = s.education.iter().map(|e| e.looking).sum();
    let free: u32 = s.jobs.iter().map(|j| j.free).sum();
    if free > looking + VERDICT_MIN && looking <= VERDICT_MIN.max(s.adults / 30) {
        return Verdict::NoWorkers;
    }
    let low_looking: u32 = s.education[..2].iter().map(|e| e.looking).sum();
    let high_free: u32 = s.jobs[2..].iter().map(|j| j.free).sum();
    if high_free > VERDICT_MIN && low_looking > VERDICT_MIN {
        return Verdict::NoEducated;
    }
    Verdict::Balanced
}

/// The text of a verdict.
pub fn verdict_text(v: Verdict) -> String {
    match v {
        Verdict::NoJobs {
            level,
            looking,
            free,
        } => {
            let what = if level >= 2 {
                tr("offices")
            } else {
                tr("commerce or industry")
            };
            trf!(
                "{n} residents have no work (education: {level}), {m} jobs are free — zone {what}",
                n = looking,
                level = level_name(level),
                m = free,
                what = what
            )
        }
        Verdict::NoWorkers => tr("Jobs are free but nobody is looking — build more homes"),
        Verdict::NoEducated => {
            tr("Skilled jobs are free, the jobless are uneducated — build schools and a university")
        }
        Verdict::Balanced => tr("Work and education are in balance"),
    }
}

fn level_name(level: usize) -> String {
    match level {
        0 => tr("none"),
        1 => tr("primary"),
        2 => tr("secondary"),
        _ => tr("higher"),
    }
}

fn level_title(level: usize) -> String {
    match level {
        0 => tr("No education"),
        1 => tr("Primary"),
        2 => tr("Secondary"),
        _ => tr("Higher"),
    }
}

/// Unemployment (`looking` over the adults) of a daily sample, in percent.
fn percent(looking: u32, adults: u32) -> f32 {
    if adults == 0 {
        0.0
    } else {
        looking as f32 * 100.0 / adults as f32
    }
}

/// Paints `parts` as one bar left to right: `(width in points, colour)`.
fn stacked(ui: &mut egui::Ui, parts: &[(f32, egui::Color32)]) -> egui::Rect {
    let width: f32 = parts.iter().map(|p| p.0).sum();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(BAR_W, BAR_H), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, egui::Color32::from_gray(28));
    let mut x = rect.left();
    for &(w, colour) in parts {
        let part = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(w, BAR_H));
        painter.rect_filled(part, 0.0, colour);
        x += w;
    }
    debug_assert!(width <= BAR_W + 0.5);
    rect
}

/// The window contents (no scroll area: tests draw it into a tall `Ui`).
pub fn jobs_contents(ui: &mut egui::Ui, view: &SimView) {
    let s = &*view.city;
    let max_adults = s
        .education
        .iter()
        .map(|e| e.adults)
        .max()
        .unwrap_or(0)
        .max(1) as f32;
    let max_jobs = s
        .jobs
        .iter()
        .map(|j| j.total + j.building)
        .max()
        .unwrap_or(0)
        .max(1) as f32;
    egui::Grid::new("jobs_rows")
        .num_columns(4)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            for h in [
                tr("Education"),
                tr("Residents"),
                tr("Adults"),
                tr("Jobs needing it"),
            ] {
                ui.weak(h);
            }
            ui.end_row();
            for level in 0..4 {
                let e = &s.education[level];
                let j = &s.jobs[level];
                ui.label(level_title(level));
                ui.label(e.residents.to_string());
                let scale = BAR_W * e.adults as f32 / max_adults / e.adults.max(1) as f32;
                let rest = e.adults.saturating_sub(e.employed + e.looking);
                let rect = stacked(
                    ui,
                    &[
                        (e.employed as f32 * scale, EMPLOYED),
                        (e.looking as f32 * scale, LOOKING),
                        (rest as f32 * scale, OTHER),
                    ],
                );
                ui.allocate_rect(rect, egui::Sense::hover()).on_hover_text(trf!(
                    "{employed} working, {looking} looking for work, {rest} studying or retired",
                    employed = e.employed,
                    looking = e.looking,
                    rest = rest
                ));
                let scale = BAR_W / max_jobs;
                let filled = j.total.saturating_sub(j.free);
                let rect = stacked(
                    ui,
                    &[
                        (filled as f32 * scale, JOBS),
                        (j.free as f32 * scale, JOBS.gamma_multiply(0.4)),
                    ],
                );
                // Slots of buildings under construction: an outline after the finished ones.
                let at = rect.left() + j.total as f32 * scale;
                ui.painter().rect_stroke(
                    egui::Rect::from_min_size(
                        egui::pos2(at, rect.top()),
                        egui::vec2(j.building as f32 * scale, BAR_H),
                    ),
                    0.0,
                    egui::Stroke::new(1.0, JOBS),
                    egui::StrokeKind::Inside,
                );
                ui.allocate_rect(rect, egui::Sense::hover())
                    .on_hover_text(trf!(
                        "{filled} filled, {free} free, {building} being built",
                        filled = filled,
                        free = j.free,
                        building = j.building
                    ));
                ui.end_row();
            }
        });
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(EMPLOYED, tr("working"));
        ui.colored_label(LOOKING, tr("looking for work"));
        ui.colored_label(OTHER, tr("studying or retired"));
        ui.colored_label(JOBS, tr("jobs filled"));
        ui.colored_label(JOBS.gamma_multiply(0.4), tr("free"));
        ui.colored_label(JOBS, tr("outline: being built"));
    });
    ui.add_space(6.0);
    let v = verdict(s);
    let colour = if v == Verdict::Balanced {
        EMPLOYED
    } else {
        egui::Color32::from_rgb(230, 170, 50)
    };
    ui.colored_label(colour, verdict_text(v));
    ui.add_space(6.0);
    ui.weak(tr("Unemployment, last 30 days"));
    let series: Vec<f32> = s
        .daily
        .iter()
        .map(|d| percent(d.looking, d.adults))
        .collect();
    sparkline(ui, &series, egui::vec2(240.0, 36.0));
    let now = percent(s.education.iter().map(|e| e.looking).sum(), s.adults);
    ui.label(trf!("Now: {percent} %", percent = format!("{now:.1}")));
}

/// The window itself, shown while `*open`.
pub fn jobs_window(ctx: &egui::Context, open: &mut bool, view: &SimView) {
    frame::window("jobs", Icon::Jobs, &tr("Jobs & education"), open).show(ctx, |ui| {
        jobs_contents(ui, view);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_sim::stats::{EducationStat, JobSlots};

    fn stats(looking: [u32; 4], free: [u32; 4]) -> CityStats {
        let mut s = CityStats::default();
        for l in 0..4 {
            s.education[l] = EducationStat {
                residents: 100,
                adults: 100,
                looking: looking[l],
                ..Default::default()
            };
            s.jobs[l] = JobSlots {
                total: free[l] + 10,
                free: free[l],
                building: 0,
            };
        }
        s.adults = 400;
        s
    }

    #[test]
    fn the_verdict_follows_the_four_branches_in_order() {
        // Branch 1: educated jobless and no slots they may take.
        let s = stats([0, 0, 0, 30], [0, 0, 0, 0]);
        assert_eq!(
            verdict(&s),
            Verdict::NoJobs {
                level: 3,
                looking: 30,
                free: 0
            }
        );
        // Free slots of lower levels are available to the educated: no complaint.
        let s = stats([0, 0, 0, 30], [40, 0, 0, 0]);
        assert_eq!(verdict(&s), Verdict::Balanced);
        // The uneducated cannot take higher slots.
        let s = stats([30, 0, 0, 0], [0, 0, 0, 60]);
        assert_eq!(
            verdict(&s),
            Verdict::NoJobs {
                level: 0,
                looking: 30,
                free: 0
            }
        );
        // Branch 2: free slots and nobody looking.
        let s = stats([0, 0, 0, 0], [20, 20, 0, 0]);
        assert_eq!(verdict(&s), Verdict::NoWorkers);
        // Branch 3: free skilled slots while the jobless are the uneducated, who still have
        // some slots of their own.
        let s = stats([8, 8, 0, 0], [20, 20, 30, 30]);
        assert_eq!(verdict(&s), Verdict::NoEducated);
        // Branch 4: the rest.
        let s = stats([2, 0, 0, 0], [1, 1, 1, 1]);
        assert_eq!(verdict(&s), Verdict::Balanced);
        assert_eq!(verdict(&CityStats::default()), Verdict::Balanced);
    }

    #[test]
    fn the_window_draws_the_rows_the_verdict_and_the_percent() {
        use crate::testutil::{frame_texts, input};
        let mut view = cp_sim::SimRunner::new(1).view();
        let mut city = (*view.city).clone();
        city.adults = 100;
        city.education[1].adults = 100;
        city.education[1].looking = 12;
        city.jobs[1].free = 0;
        view.city = std::sync::Arc::new(city);
        let ctx = egui::Context::default();
        let mut all = Vec::new();
        for t in [0.0, 0.1] {
            let mut tall = input(t, vec![]);
            tall.screen_rect = Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 4000.0),
            ));
            all = frame_texts(&ctx, tall, |ui| jobs_contents(ui, &view))
                .into_iter()
                .map(|(t, _)| t)
                .collect();
        }
        for want in [
            "No education",
            "Primary",
            "Secondary",
            "Higher",
            "Now: 12.0 %",
        ] {
            assert!(all.iter().any(|t| t == want), "{want}: {all:?}");
        }
        assert!(
            all.iter()
                .any(|t| t.starts_with("12 residents have no work (education: primary)")),
            "{all:?}"
        );
    }
}
