//! The integration tests of `cp_save` in one test binary (T-334: one link instead of one per
//! file). Each module is a former `tests/<name>.rs`.

mod bikes_resume;
mod bridges_e2e;
mod budgets_resume;
mod build_splits_e2e;
mod contentment_resume;
mod corruption_resume;
mod debug_traffic_e2e;
mod emissions_resume;
mod free_standing_e2e;
mod highway_junction_e2e;
mod hotels_save_e2e;
mod instant_services_e2e;
mod interchange_fixture;
mod junction_control_e2e;
mod m1_fixtures;
mod market_resume;
mod net_commands_e2e;
mod park_visitors_save;
mod parking_resume;
mod rail_e2e;
mod remove_part_e2e;
mod save_e2e;
mod save_integrity;
mod service_coverage_e2e;
mod station_e2e;
