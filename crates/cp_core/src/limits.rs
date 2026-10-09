//! Arena and agent-pool capacities (`architecture/system-design.md` §3).

use bevy_ecs::resource::Resource;
use serde::{Deserialize, Serialize};

#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimLimits {
    pub nodes: u32,
    pub segments: u32,
    pub lanes: u32,
    pub buildings: u32,
    pub citizens: u32,
    pub households: u32,
    /// Vehicle agent pool.
    pub vehicles: u32,
    /// Pedestrian agent pool.
    pub pedestrians: u32,
    pub zone_blocks: u32,
}

impl SimLimits {
    pub const fn desktop() -> Self {
        Self {
            nodes: 32_768,
            segments: 36_864,
            lanes: 262_144,
            buildings: 49_152,
            citizens: 1_048_576,
            households: 262_144,
            vehicles: 16_384,
            pedestrians: 65_536,
            zone_blocks: 49_152,
        }
    }

    pub const fn wasm() -> Self {
        Self {
            nodes: 8_192,
            segments: 9_216,
            lanes: 65_536,
            buildings: 12_288,
            citizens: 131_072,
            households: 32_768,
            vehicles: 2_048,
            pedestrians: 4_096,
            zone_blocks: 12_288,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_match_design_table() {
        let d = SimLimits::desktop();
        assert_eq!(
            [
                d.nodes,
                d.segments,
                d.lanes,
                d.buildings,
                d.citizens,
                d.households,
                d.vehicles,
                d.pedestrians,
                d.zone_blocks
            ],
            [
                32_768, 36_864, 262_144, 49_152, 1_048_576, 262_144, 16_384, 65_536, 49_152
            ]
        );
        let w = SimLimits::wasm();
        assert_eq!(
            [
                w.nodes,
                w.segments,
                w.lanes,
                w.buildings,
                w.citizens,
                w.households,
                w.vehicles,
                w.pedestrians,
                w.zone_blocks
            ],
            [
                8_192, 9_216, 65_536, 12_288, 131_072, 32_768, 2_048, 4_096, 12_288
            ]
        );
    }

    #[test]
    fn serde_round_trip() {
        for l in [SimLimits::desktop(), SimLimits::wasm()] {
            let bytes = postcard::to_stdvec(&l).unwrap();
            assert_eq!(postcard::from_bytes::<SimLimits>(&bytes).unwrap(), l);
        }
    }
}
