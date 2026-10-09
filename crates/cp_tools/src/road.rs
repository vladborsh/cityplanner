//! The road tool: modes, click state machine, preview.

use bevy_math::{Vec2, Vec3};
use cp_content::RoadDefId;
use cp_core::Cost;
use cp_core::curve::CubicBezier3;
use cp_net::{
    ElevationMode, RoadBuildError, RoadBuildPlan, RoadPath, RoadPathRequest, SnapOptions,
    validate_road_path,
};
use cp_sim::SimView;

/// Metres of road height per PgUp/PgDn step.
pub const ELEVATION_STEP: f32 = 5.0;
/// Highest elevation level (`level * ELEVATION_STEP` metres).
pub const MAX_ELEVATION_LEVEL: u8 = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RoadMode {
    #[default]
    Straight,
    /// Three clicks: start, control point, end.
    Curve,
    /// A polyline; Enter (or a click on the last point) finishes it.
    Free,
}

/// State of the road tool.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RoadTool {
    pub mode: RoadMode,
    pub road: Option<RoadDefId>,
    /// 0 = ground, otherwise raised by `level * ELEVATION_STEP`.
    pub level: u8,
    /// Points clicked so far in the current drawing.
    pub points: Vec<Vec2>,
}

impl RoadTool {
    pub fn elevation(&self) -> ElevationMode {
        if self.level == 0 {
            ElevationMode::Ground
        } else {
            ElevationMode::Elevated(f32::from(self.level) * ELEVATION_STEP)
        }
    }

    pub fn raise(&mut self) {
        self.level = (self.level + 1).min(MAX_ELEVATION_LEVEL);
    }

    pub fn lower(&mut self) {
        self.level = self.level.saturating_sub(1);
    }

    pub fn set_mode(&mut self, mode: RoadMode) {
        if self.mode != mode {
            self.mode = mode;
            self.points.clear();
        }
    }

    pub fn cancel(&mut self) {
        self.points.clear();
    }

    /// Clicks needed before the path is complete (`None` for free mode, which ends on demand).
    fn clicks_needed(&self) -> Option<usize> {
        match self.mode {
            RoadMode::Straight => Some(2),
            RoadMode::Curve => Some(3),
            RoadMode::Free => None,
        }
    }

    /// The path as it would be if the cursor were at `hover` (what the ghost shows). With
    /// nothing clicked yet there is nothing to preview.
    pub fn path_with_hover(&self, hover: Vec2) -> Option<RoadPath> {
        let first = *self.points.first()?;
        Some(match (self.mode, self.points.len()) {
            (RoadMode::Straight, _) | (RoadMode::Curve, 1) => RoadPath::Straight {
                from: first,
                to: hover,
            },
            (RoadMode::Curve, _) => RoadPath::Curve {
                from: first,
                control: self.points[1],
                to: hover,
            },
            (RoadMode::Free, _) => {
                let mut pts = self.points.clone();
                pts.push(hover);
                RoadPath::Free(pts)
            }
        })
    }

    /// The request for `path` with the tool's road type, elevation and the default snaps (the
    /// ones the `BuildRoad` command uses, so the preview and the command agree).
    pub fn request(&self, path: RoadPath) -> Option<RoadPathRequest> {
        Some(RoadPathRequest {
            road: self.road?,
            path,
            elevation: self.elevation(),
            snap: SnapOptions::default(),
        })
    }

    /// A click at `p`. Returns the finished request when this click completed the drawing.
    pub fn click(&mut self, p: Vec2) -> Option<RoadPathRequest> {
        self.road?;
        if self.mode == RoadMode::Free && self.points.last().is_some_and(|l| l.distance(p) < 1.0) {
            return self.finish_free();
        }
        self.points.push(p);
        if self.clicks_needed() == Some(self.points.len()) {
            let pts = std::mem::take(&mut self.points);
            let path = match self.mode {
                RoadMode::Straight => RoadPath::Straight {
                    from: pts[0],
                    to: pts[1],
                },
                RoadMode::Curve => RoadPath::Curve {
                    from: pts[0],
                    control: pts[1],
                    to: pts[2],
                },
                RoadMode::Free => unreachable!("free mode has no fixed length"),
            };
            return self.request(path);
        }
        None
    }

    /// Ends a free-mode drawing (needs at least two points).
    pub fn finish_free(&mut self) -> Option<RoadPathRequest> {
        if self.mode != RoadMode::Free || self.points.len() < 2 {
            return None;
        }
        let pts = std::mem::take(&mut self.points);
        self.request(RoadPath::Free(pts))
    }
}

/// Validates `req` against the snapshot with the same pure function `BuildRoad` uses.
/// `None` when the view has no terrain or content (nothing to validate against).
pub fn preview(
    view: &SimView,
    req: &RoadPathRequest,
) -> Option<Result<RoadBuildPlan, Vec<RoadBuildError>>> {
    let terrain = view.terrain.as_deref()?;
    let content = view.content.as_deref()?;
    Some(validate_road_path(
        &view.net,
        terrain,
        &view.tiles,
        content,
        req,
    ))
}

/// Cost as a money string: cents → `$1,234.56`.
pub fn format_money(cost: Cost) -> String {
    let cents = cost.0;
    let sign = if cents < 0 { "-" } else { "" };
    let cents = cents.unsigned_abs();
    let dollars = (cents / 100).to_string();
    let mut grouped = String::new();
    for (i, ch) in dollars.chars().enumerate() {
        if i > 0 && (dollars.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!("{sign}${grouped}.{:02}", cents % 100)
}

/// Points along a curve, about every `step` metres.
pub fn sample_curve(c: &CubicBezier3, step: f32) -> Vec<Vec3> {
    let len = c.arc_table().length().max(step);
    let n = (len / step).ceil() as usize;
    (0..=n).map(|i| c.point(i as f32 / n as f32)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(mode: RoadMode) -> RoadTool {
        RoadTool {
            mode,
            road: Some(RoadDefId(0)),
            ..Default::default()
        }
    }

    #[test]
    fn money_is_formatted_from_cents() {
        assert_eq!(format_money(Cost(0)), "$0.00");
        assert_eq!(format_money(Cost(5)), "$0.05");
        assert_eq!(format_money(Cost(123_456)), "$1,234.56");
        assert_eq!(format_money(Cost(100_000_000)), "$1,000,000.00");
        assert_eq!(format_money(Cost(-250)), "-$2.50");
    }

    #[test]
    fn elevation_levels_step_and_clamp() {
        let mut t = tool(RoadMode::Straight);
        assert_eq!(t.elevation(), ElevationMode::Ground);
        t.lower();
        assert_eq!(t.level, 0);
        t.raise();
        assert_eq!(t.elevation(), ElevationMode::Elevated(ELEVATION_STEP));
        for _ in 0..10 {
            t.raise();
        }
        assert_eq!(t.level, MAX_ELEVATION_LEVEL);
        t.lower();
        assert_eq!(t.level, MAX_ELEVATION_LEVEL - 1);
    }

    #[test]
    fn straight_takes_two_clicks() {
        let mut t = tool(RoadMode::Straight);
        assert!(t.path_with_hover(Vec2::ONE).is_none());
        assert!(t.click(Vec2::new(1.0, 2.0)).is_none());
        assert_eq!(
            t.path_with_hover(Vec2::new(5.0, 2.0)),
            Some(RoadPath::Straight {
                from: Vec2::new(1.0, 2.0),
                to: Vec2::new(5.0, 2.0)
            })
        );
        let req = t.click(Vec2::new(9.0, 2.0)).unwrap();
        assert_eq!(
            req.path,
            RoadPath::Straight {
                from: Vec2::new(1.0, 2.0),
                to: Vec2::new(9.0, 2.0)
            }
        );
        assert_eq!(req.snap, SnapOptions::default());
        assert!(t.points.is_empty(), "the tool is ready for the next road");
    }

    #[test]
    fn curve_takes_three_clicks_start_control_end() {
        let mut t = tool(RoadMode::Curve);
        assert!(t.click(Vec2::new(0.0, 0.0)).is_none());
        // After the first click the ghost is a straight line; after the second a curve.
        assert!(matches!(
            t.path_with_hover(Vec2::new(10.0, 0.0)),
            Some(RoadPath::Straight { .. })
        ));
        assert!(t.click(Vec2::new(50.0, 0.0)).is_none());
        assert_eq!(
            t.path_with_hover(Vec2::new(50.0, 50.0)),
            Some(RoadPath::Curve {
                from: Vec2::new(0.0, 0.0),
                control: Vec2::new(50.0, 0.0),
                to: Vec2::new(50.0, 50.0)
            })
        );
        let req = t.click(Vec2::new(50.0, 50.0)).unwrap();
        assert!(matches!(req.path, RoadPath::Curve { .. }));
    }

    #[test]
    fn free_mode_collects_points_until_finished() {
        let mut t = tool(RoadMode::Free);
        for p in [(0.0, 0.0), (30.0, 0.0), (30.0, 30.0)] {
            assert!(t.click(Vec2::new(p.0, p.1)).is_none());
        }
        assert_eq!(
            t.path_with_hover(Vec2::new(60.0, 30.0)),
            Some(RoadPath::Free(vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(30.0, 0.0),
                Vec2::new(30.0, 30.0),
                Vec2::new(60.0, 30.0)
            ]))
        );
        // Clicking on the last point again finishes.
        let req = t.click(Vec2::new(30.0, 30.0)).unwrap();
        assert_eq!(
            req.path,
            RoadPath::Free(vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(30.0, 0.0),
                Vec2::new(30.0, 30.0)
            ])
        );
        // Enter with fewer than two points does nothing.
        t.click(Vec2::ZERO);
        assert!(t.finish_free().is_none());
        t.click(Vec2::new(20.0, 0.0));
        assert!(t.finish_free().is_some());
    }

    #[test]
    fn changing_mode_or_cancelling_drops_the_drawing() {
        let mut t = tool(RoadMode::Curve);
        t.click(Vec2::ZERO);
        t.set_mode(RoadMode::Free);
        assert!(t.points.is_empty());
        t.click(Vec2::ZERO);
        t.cancel();
        assert!(t.points.is_empty());
    }

    #[test]
    fn nothing_is_built_without_a_road_type() {
        let mut t = RoadTool::default();
        assert!(t.click(Vec2::ZERO).is_none());
        assert!(t.points.is_empty());
    }
}
