//! Format 20 (T-129): the economy state carries the active loans and the weekly report its
//! loan payments. The v19 fixture (made by the real v19 writer) migrates with no loans and
//! round-trips on the new version, and a city saved in the middle of a loan's term resumes
//! exactly like the uninterrupted run.

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec3;
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_WEEK;
use cp_core::{Cost, Treasury};
use cp_economy::{EconomyCommand, EconomyState};
use cp_net::{Elevation, NetGraph};
use cp_save::*;
use cp_sim::{SimCommand, SimRunner};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn content() -> Arc<ContentDb> {
    Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap())
}

#[test]
fn the_v19_fixture_migrates_without_loans_and_round_trips() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v19-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 19, "made by the v19 writer");
    let content = content();
    let l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    assert!(l.data.economy.loans.is_empty());
    assert_eq!(l.data.economy.last_week.loan_payments, Cost::ZERO);
    assert_eq!(l.data.economy.taxes, EconomyState::default().taxes);
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
}

#[test]
fn a_loan_saved_mid_term_resumes_like_the_continuous_run() {
    let content = content();
    let hash = content.content_hash();
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    let mut a = SimRunner::new(5);
    a.set_content(Arc::clone(&content));
    a.world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            &def,
            road,
            CubicBezier3::line(
                Vec3::new(3000.0, 0.0, 3000.0),
                Vec3::new(3090.0, 0.0, 3000.0),
            ),
            Elevation::Ground,
        )
        .unwrap();
    a.push_command(
        SimCommand::Economy(EconomyCommand::TakeLoan { offer: 0 }),
        a.tick(),
    );
    while a.tick().0 <= 3 * TICKS_PER_WEEK {
        a.tick_once();
    }
    let loan = *a.world().resource::<EconomyState>().loan(0).unwrap();
    assert_eq!(loan.weeks_left, 23);
    assert!(
        a.world()
            .resource::<EconomyState>()
            .last_week
            .loan_payments
            .0
            > 0
    );

    let bytes = save_runner(&a, hash, "none").unwrap();
    let mut b = SimRunner::new(5);
    b.set_content(Arc::clone(&content));
    load_into_runner(&mut b, &bytes, hash).unwrap();
    assert_eq!(b.world().resource::<EconomyState>().loan(0), Some(&loan));
    assert_eq!(a.checksum(), b.checksum(), "identical right after the load");
    for _ in 0..(2 * TICKS_PER_WEEK) {
        a.tick_once();
        b.tick_once();
    }
    assert_eq!(
        a.world()
            .resource::<EconomyState>()
            .loan(0)
            .unwrap()
            .weeks_left,
        21
    );
    assert_eq!(a.checksum(), b.checksum(), "same state two weeks later");
    assert_eq!(
        a.world().resource::<Treasury>(),
        b.world().resource::<Treasury>()
    );
}
