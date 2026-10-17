//! The heavy traffic ban (T-195): which segments lie in a district where trucks are unwelcome.
//!
//! [`HeavyBan`] is derived (not saved): the district of each segment's midpoint, the districts
//! (or the whole city) with `HeavyTrafficBan` on, and the A* cost multiplier. It is rebuilt by
//! [`crate::update_path_network`] when the network, the district map, the district records or
//! the city policies change. A truck's search multiplies the cost of a lane in a banned district
//! by `mul` unless its start or its goal lies in that district: a cost, not a prohibition.

use bevy_math::Vec2;
use cp_district::{CityPolicies, DistrictMap, Districts, district_at};
use cp_net::{NetGraph, SegmentId};

use cp_content::PolicyKind;

/// Segments in districts with the ban, as a snapshot the path searches read.
#[derive(Clone, Debug, PartialEq)]
pub struct HeavyBan {
    /// Cost multiplier; `1.0` = no ban anywhere.
    mul: f32,
    /// District id (`0` = none) of the midpoint of each segment, by arena index.
    seg_district: Vec<u8>,
    /// Whether trucks avoid district `n` (index 0: the segments outside every district).
    banned: [bool; 256],
}

impl Default for HeavyBan {
    fn default() -> Self {
        Self {
            mul: 1.0,
            seg_district: Vec::new(),
            banned: [false; 256],
        }
    }
}

impl HeavyBan {
    /// The ban in force, or the default (no ban) when nothing is banned.
    pub fn build(
        net: &NetGraph,
        map: &DistrictMap,
        districts: &Districts,
        city: &CityPolicies,
        mul: f32,
    ) -> Self {
        let bit = PolicyKind::HeavyTrafficBan.bit();
        let whole_city = city.mask & bit != 0;
        let mut banned = [false; 256];
        if whole_city {
            banned = [true; 256];
        } else {
            for (id, d) in districts.iter() {
                banned[usize::from(id.0)] = d.policies & bit != 0;
            }
        }
        if mul <= 1.0 || !banned.iter().any(|&b| b) {
            return Self::default();
        }
        let mut seg_district = Vec::new();
        for (id, seg) in net.segments() {
            let m = seg.curve.point(0.5);
            let d = district_at(map, Vec2::new(m.x, m.z)).map_or(0, |d| d.0);
            let i = id.index as usize;
            if seg_district.len() <= i {
                seg_district.resize(i + 1, 0);
            }
            seg_district[i] = d;
        }
        Self {
            mul,
            seg_district,
            banned,
        }
    }

    /// Whether any district bans trucks.
    pub fn is_active(&self) -> bool {
        self.mul > 1.0
    }

    /// The district of a segment's midpoint (`0` = none).
    pub fn district_of(&self, segment: SegmentId) -> u8 {
        self.seg_district
            .get(segment.index as usize)
            .copied()
            .unwrap_or(0)
    }

    /// The cost multiplier of a lane on `segment` for a trip between districts `ends`.
    pub fn factor(&self, segment: SegmentId, ends: (u8, u8)) -> f32 {
        let d = self.district_of(segment);
        if self.banned[usize::from(d)] && d != ends.0 && d != ends.1 {
            self.mul
        } else {
            1.0
        }
    }
}
