//! Frozen definition-name tables: the content as it was when saves of each version were written.
//!
//! A save before v18 carries no names, only dense indices into the sorted definition tables of
//! the content that wrote it. Those tables must not follow the content on disk (new road types
//! sort anywhere), so the v17 → v18 step takes them from here. Content history (git, `assets/content`):
//!
//! * roads (`basic_2l`, `highway_1w_3l`, `large_6l`, `medium_4l`), vehicles (`car_basic`) and
//!   utilities (`power_line`, `sewage_pipe`, `water_pipe`) never changed up to v17;
//! * buildings: 45 definitions from T-028 (save v6) to v11; T-038 added `coal_power_plant`,
//!   `sewage_outflow_station` and `wind_turbine` (49) while v12 was current, so every v12-and-later
//!   file is read with the 49-name table (a v12 file written between T-035 and T-038 would be
//!   mis-read; no such file is committed). Before v6 a save holds no building ids;
//! * services: `water_pump` alone to v11, plus `coal_plant`, `sewage_outflow` and `wind_turbine`
//!   from T-038 (v12).
//!
//! Do not edit a table: content changes after v17 are covered by the names the save then carries.

use crate::DefNames;

/// Road types, v0..=v17.
const ROADS_V17: &[&str] = &["basic_2l", "highway_1w_3l", "large_6l", "medium_4l"];

/// Building types, v12..=v17.
const BUILDINGS_V12_TO_V17: &[&str] = &[
    "apt_high_1",
    "apt_high_1b",
    "apt_high_2",
    "apt_high_2b",
    "apt_high_3",
    "apt_high_3b",
    "apt_high_4",
    "apt_high_4b",
    "apt_high_5",
    "apt_high_5b",
    "coal_power_plant",
    "com_high_1",
    "com_high_1b",
    "com_high_2",
    "com_high_2b",
    "com_high_3",
    "com_high_3b",
    "com_low_1",
    "com_low_1b",
    "com_low_2",
    "com_low_2b",
    "com_low_3",
    "com_low_3b",
    "house_low_1",
    "house_low_1b",
    "house_low_2",
    "house_low_2b",
    "house_low_3",
    "house_low_3b",
    "house_low_4",
    "house_low_4b",
    "house_low_5",
    "house_low_5b",
    "ind_1",
    "ind_1b",
    "ind_2",
    "ind_2b",
    "ind_3",
    "ind_3b",
    "office_1",
    "office_1b",
    "office_2",
    "office_2b",
    "office_3",
    "office_3b",
    "sewage_outflow_station",
    "water_pump_station",
    "wind_turbine",
];

/// Building types, v0..=v11.
const BUILDINGS_UP_TO_V11: &[&str] = &[
    "apt_high_1",
    "apt_high_1b",
    "apt_high_2",
    "apt_high_2b",
    "apt_high_3",
    "apt_high_3b",
    "apt_high_4",
    "apt_high_4b",
    "apt_high_5",
    "apt_high_5b",
    "com_high_1",
    "com_high_1b",
    "com_high_2",
    "com_high_2b",
    "com_high_3",
    "com_high_3b",
    "com_low_1",
    "com_low_1b",
    "com_low_2",
    "com_low_2b",
    "com_low_3",
    "com_low_3b",
    "house_low_1",
    "house_low_1b",
    "house_low_2",
    "house_low_2b",
    "house_low_3",
    "house_low_3b",
    "house_low_4",
    "house_low_4b",
    "house_low_5",
    "house_low_5b",
    "ind_1",
    "ind_1b",
    "ind_2",
    "ind_2b",
    "ind_3",
    "ind_3b",
    "office_1",
    "office_1b",
    "office_2",
    "office_2b",
    "office_3",
    "office_3b",
    "water_pump_station",
];

const SERVICES_V12_TO_V17: &[&str] =
    &["coal_plant", "sewage_outflow", "water_pump", "wind_turbine"];
const SERVICES_UP_TO_V11: &[&str] = &["water_pump"];
const VEHICLES_V17: &[&str] = &["car_basic"];
const UTILITIES_V17: &[&str] = &["power_line", "sewage_pipe", "water_pipe"];

fn owned(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| (*n).to_owned()).collect()
}

/// The definition names of the content that wrote a save of `version` (≤ 17).
pub fn def_names_before_v18(version: u32) -> DefNames {
    let late = version >= 12;
    DefNames {
        roads: owned(ROADS_V17),
        buildings: owned(if late {
            BUILDINGS_V12_TO_V17
        } else {
            BUILDINGS_UP_TO_V11
        }),
        services: owned(if late {
            SERVICES_V12_TO_V17
        } else {
            SERVICES_UP_TO_V11
        }),
        vehicles: owned(VEHICLES_V17),
        utilities: owned(UTILITIES_V17),
    }
}
