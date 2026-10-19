//! The road and placement cases of the T-261 acceptance sweeps (`architecture/system-design.md`
//! §6.8.5): the junction gallery (`junction_sweep` in `cp_render`, `--scene joins`) and the
//! service sites (`place_sweep`, `--scene place`). Each case is roads in a local frame around
//! the origin; [`build_roads`] lays them at a world point through `BuildRoad` commands, as the
//! player would.

use bevy_ecs::message::Messages;
use bevy_math::Vec2;
use cp_core::CommandOutcome;
use cp_net::{ElevationMode, NetCommand, RoadPath};

use crate::{SimCommand, SimRunner};

/// Length of the arms of a junction case, metres.
pub const ARM: f32 = 90.0;

/// One road of a case: its type id and its path in the local frame.
pub type CaseRoad = (&'static str, RoadPath);

/// A junction case: the roads that meet at the local origin.
#[derive(Clone, Debug)]
pub struct JoinCase {
    pub name: String,
    pub roads: Vec<CaseRoad>,
}

fn dir(deg: f32) -> Vec2 {
    Vec2::from_angle(deg.to_radians())
}

fn line(road: &'static str, a: Vec2, b: Vec2) -> CaseRoad {
    (road, RoadPath::Straight { from: a, to: b })
}

/// Roads with sidewalks, narrow to wide, and a one-way street.
pub const STREETS: [&str; 4] = ["basic_2l", "medium_4l", "large_6l", "basic_1w_2l"];
/// Every road of the bends: the streets and a highway.
pub const BEND_ROADS: [&str; 5] = [
    "basic_2l",
    "medium_4l",
    "large_6l",
    "basic_1w_2l",
    "highway_1w_2l",
];
/// Turns of the degree-2 bends, degrees: the small ones are the chords of BUG-080.
pub const TURNS: [f32; 7] = [5.0, 10.0, 20.0, 30.0, 45.0, 90.0, 135.0];

/// The junction gallery: bends of every turn and road, curved bends, T, X and Y junctions,
/// curved arms into a T, a change of width at a degree-2 node (also street to highway, the
/// entry road's merge), a street into a highway, and an automatic cut on a long street.
pub fn join_cases() -> Vec<JoinCase> {
    let o = Vec2::ZERO;
    let mut out = Vec::new();
    let mut add = |name: String, roads: Vec<CaseRoad>| out.push(JoinCase { name, roads });
    for road in BEND_ROADS {
        for turn in TURNS {
            add(
                format!("bend/{road}/{turn}"),
                vec![
                    line(road, -Vec2::X * ARM, o),
                    line(road, o, dir(turn) * ARM),
                ],
            );
        }
        // A curved arm leaving a 30-degree kink.
        add(
            format!("bend_curved/{road}"),
            vec![
                line(road, -Vec2::X * ARM, o),
                (
                    road,
                    RoadPath::Curve {
                        from: o,
                        control: dir(30.0) * 40.0,
                        to: Vec2::new(70.0, 60.0),
                    },
                ),
            ],
        );
    }
    for road in STREETS {
        for deg in [90.0, 60.0, 45.0] {
            add(
                format!("t/{road}/{deg}"),
                vec![
                    line(road, -Vec2::X * ARM, Vec2::X * ARM),
                    line(road, o, dir(deg) * ARM),
                ],
            );
        }
        for deg in [90.0, 60.0] {
            add(
                format!("x/{road}/{deg}"),
                vec![
                    line(road, -Vec2::X * ARM, Vec2::X * ARM),
                    line(road, -dir(deg) * ARM, dir(deg) * ARM),
                ],
            );
        }
        add(
            format!("y/{road}"),
            vec![
                line(road, -Vec2::X * ARM, o),
                line(road, o, dir(20.0) * ARM),
                line(road, o, dir(-20.0) * ARM),
            ],
        );
        add(
            format!("curved_t/{road}"),
            vec![
                line(road, -Vec2::X * ARM, Vec2::X * ARM),
                (
                    road,
                    RoadPath::Curve {
                        from: o,
                        control: Vec2::Y * 40.0,
                        to: Vec2::new(40.0, 70.0),
                    },
                ),
            ],
        );
    }
    for turn in [0.0, 20.0] {
        add(
            format!("width/basic_2l-medium_4l/{turn}"),
            vec![
                line("basic_2l", -Vec2::X * ARM, o),
                line("medium_4l", o, dir(turn) * ARM),
            ],
        );
        add(
            format!("width/highway_1w_2l-basic_2l/{turn}"),
            vec![
                line("highway_1w_2l", -Vec2::X * ARM, o),
                line("basic_2l", o, dir(turn) * ARM),
            ],
        );
    }
    for deg in [90.0, 45.0] {
        add(
            format!("highway_t/{deg}"),
            vec![
                line("highway_1w_2l", -Vec2::X * ARM, Vec2::X * ARM),
                line("basic_2l", o, dir(deg) * ARM),
            ],
        );
    }
    for road in ["basic_2l", "medium_4l"] {
        add(
            format!("cut/{road}"),
            vec![line(road, -Vec2::X * 110.0, Vec2::X * 110.0)],
        );
    }
    out
}

/// A service site: the roads, and the corners or edges the lot is pushed into.
#[derive(Clone, Debug)]
pub struct PlaceSite {
    pub name: String,
    pub roads: Vec<CaseRoad>,
    /// Where the lot goes: a point on a road's edge line (or the corner of two edge lines),
    /// with the unit directions along the road and away from it into the block. A lot of
    /// `w x d` metres is put with its corner there: its centre is `at + along * w/2 + into * d/2`.
    pub spots: Vec<PlaceSpot>,
}

#[derive(Clone, Copy, Debug)]
pub struct PlaceSpot {
    pub name: &'static str,
    pub at: Vec2,
    pub along: Vec2,
    pub into: Vec2,
}

impl PlaceSpot {
    /// The centre of a `w x d` lot (metres) pushed into this spot, plus `gap` metres off the
    /// road edge and off a corner's cross street.
    pub fn centre(&self, w: f32, d: f32, gap: f32) -> Vec2 {
        self.at + self.along * (w / 2.0 + gap) + self.into * (d / 2.0 + gap)
    }
}

/// The service sites, all with `road` (half width `hw`): mid-street on both sides, the four
/// corners of a crossing, the two corners beside a T and the one across it, the inside of a
/// rounded corner, both sides of a gentle curve and across the automatic cut of a long street.
pub fn place_sites(road: &'static str, hw: f32) -> Vec<PlaceSite> {
    let mut out = Vec::new();
    let x = Vec2::X;
    let y = Vec2::Y;
    out.push(PlaceSite {
        name: format!("straight/{road}"),
        roads: vec![line(road, -x * 150.0, x * 150.0)],
        spots: vec![
            PlaceSpot {
                name: "left",
                at: Vec2::new(-20.0, hw),
                along: x,
                into: y,
            },
            PlaceSpot {
                name: "right",
                at: Vec2::new(20.0, -hw),
                along: -x,
                into: -y,
            },
        ],
    });
    out.push(PlaceSite {
        name: format!("x/{road}"),
        roads: vec![
            line(road, -x * 120.0, x * 120.0),
            line(road, -y * 120.0, y * 120.0),
        ],
        spots: [
            (1.0, 1.0, "ne"),
            (-1.0, 1.0, "nw"),
            (-1.0, -1.0, "sw"),
            (1.0, -1.0, "se"),
        ]
        .into_iter()
        .map(|(sx, sy, name)| PlaceSpot {
            name,
            at: Vec2::new(sx * hw, sy * hw),
            along: x * sx,
            into: y * sy,
        })
        .collect(),
    });
    out.push(PlaceSite {
        name: format!("t/{road}"),
        roads: vec![
            line(road, -x * 120.0, x * 120.0),
            line(road, Vec2::ZERO, y * 120.0),
        ],
        spots: vec![
            PlaceSpot {
                name: "corner_e",
                at: Vec2::new(hw, hw),
                along: x,
                into: y,
            },
            PlaceSpot {
                name: "corner_w",
                at: Vec2::new(-hw, hw),
                along: -x,
                into: y,
            },
            PlaceSpot {
                name: "across",
                at: Vec2::new(-10.0, -hw),
                along: x,
                into: -y,
            },
        ],
    });
    // An L whose corner is a quadratic of radius 24 m about (24, 24): the inside of the bend.
    let r = 24.0;
    out.push(PlaceSite {
        name: format!("rounded/{road}"),
        roads: vec![
            line(road, Vec2::new(120.0, 0.0), Vec2::new(r, 0.0)),
            (
                road,
                RoadPath::Curve {
                    from: Vec2::new(r, 0.0),
                    control: Vec2::ZERO,
                    to: Vec2::new(0.0, r),
                },
            ),
            line(road, Vec2::new(0.0, r), Vec2::new(0.0, 120.0)),
        ],
        spots: vec![PlaceSpot {
            name: "inside",
            at: Vec2::new(hw, hw),
            along: x,
            into: y,
        }],
    });
    // A gentle arc: a quadratic from (-80, 0) through the control (0, 40) to (80, 0).
    out.push(PlaceSite {
        name: format!("curve/{road}"),
        roads: vec![(
            road,
            RoadPath::Curve {
                from: Vec2::new(-80.0, 0.0),
                control: Vec2::new(0.0, 40.0),
                to: Vec2::new(80.0, 0.0),
            },
        )],
        spots: vec![
            PlaceSpot {
                name: "outside",
                at: Vec2::new(-8.0, 20.0 + hw),
                along: x,
                into: y,
            },
            PlaceSpot {
                name: "inside",
                at: Vec2::new(8.0, 20.0 - hw),
                along: -x,
                into: -y,
            },
        ],
    });
    // A 220 m street is cut in three: the lot stands across the first cut.
    out.push(PlaceSite {
        name: format!("cut/{road}"),
        roads: vec![line(road, -x * 110.0, x * 110.0)],
        spots: vec![PlaceSpot {
            name: "across_cut",
            at: Vec2::new(-110.0 + 220.0 / 3.0 - 12.0, hw),
            along: x,
            into: y,
        }],
    });
    out
}

/// Lays `roads` with their origin at `at` (world XZ), each as one `BuildRoad`, and runs a tick
/// after each. Returns the refusals, as `(road index, message)`.
pub fn build_roads(sim: &mut SimRunner, at: Vec2, roads: &[CaseRoad]) -> Vec<(usize, String)> {
    let mut refused = Vec::new();
    let Some(db) = sim.view().content.clone() else {
        return vec![(0, "no content".into())];
    };
    for (i, (road, path)) in roads.iter().enumerate() {
        let Some(id) = db.road_id(road) else {
            refused.push((i, format!("no road type {road}")));
            continue;
        };
        let path = match path {
            RoadPath::Straight { from, to } => RoadPath::Straight {
                from: *from + at,
                to: *to + at,
            },
            RoadPath::Curve { from, control, to } => RoadPath::Curve {
                from: *from + at,
                control: *control + at,
                to: *to + at,
            },
            RoadPath::Free(v) => RoadPath::Free(v.iter().map(|p| *p + at).collect()),
        };
        let tick = sim.tick();
        sim.push_command(
            SimCommand::Net(NetCommand::BuildRoad {
                road: id,
                path,
                elevation: ElevationMode::Ground,
            }),
            tick,
        );
        sim.tick_once();
        let out: Vec<CommandOutcome> = sim
            .world_mut()
            .resource_mut::<Messages<CommandOutcome>>()
            .drain()
            .collect();
        if let Some(Err(e)) = out.last().map(|o| &o.result) {
            refused.push((i, format!("{e:?}")));
        }
    }
    sim.tick_once();
    refused
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_names_are_unique() {
        fn unique(mut names: Vec<String>) {
            let n = names.len();
            names.sort();
            names.dedup();
            assert_eq!(names.len(), n);
        }
        unique(join_cases().into_iter().map(|c| c.name).collect());
        unique(
            place_sites("basic_2l", 6.0)
                .into_iter()
                .map(|s| s.name)
                .collect(),
        );
    }
}
