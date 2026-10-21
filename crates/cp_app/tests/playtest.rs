//! T-293: a scripted new game played to Busy Town (5000 residents) through the real commands
//! and tools (`cp_app::playtest`), one line per game day. Re-run after balance changes:
//! `cargo test --release -p cp_app --test playtest -- --ignored --nocapture`
//! (`PLAYTEST_DAYS`, default 80; `PLAYTEST_POP`, default 5000).

#[test]
#[ignore = "T-293 playtest: prints a report"]
fn a_new_game_played_to_busy_town() {
    let days = std::env::var("PLAYTEST_DAYS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(80);
    let pop = std::env::var("PLAYTEST_POP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5000);
    eprintln!(
        "day | money | +/-day | pop | milestone | bldg | noPow noWat noSew noGoods | crime fire garb sick dead | aband | unemp% | load ovl stuck | demand R C I move-in | free R C I"
    );
    let report = cp_app::playtest::play(pop, days, |d| {
        eprintln!(
            "{:3} | {:9.0} | {:+8.0} | {:5} | {:13} | {:4} | {:3} {:3} {:3} {:3} | {:3} {:3} {:3} {:3} {:3} | {:3} | {:5.1} | {:4.2} {:3} {:4} | {:6.1} {:6.1} {:6.1} {:6.1} | {} {} {}",
            d.day,
            d.money,
            d.balance,
            d.population,
            d.milestone,
            d.buildings,
            d.no_power,
            d.no_water,
            d.no_sewage,
            d.no_goods,
            d.crime,
            d.fire,
            d.garbage,
            d.sick,
            d.dead,
            d.abandoned,
            d.unemployment,
            d.avg_load,
            d.overloaded,
            d.stuck,
            d.demand[0],
            d.demand[1],
            d.demand[2],
            d.move_in,
            d.free[0],
            d.free[1],
            d.free[2]
        );
        if std::env::var("PLAYTEST_ACC").is_ok() {
            let a = d.acc;
            eprintln!(
                "      acc (seen {} buckets): citizens {} unemployed {} (65+ now: {}) (manual {} educated {}) vacant homes {} free jobs (weighted) {:.0} shop jobs {} industry jobs {} free office jobs {}",
                a.seen,
                a.citizens,
                a.unemployed,
                d.unemployed_old,
                a.unemployed_manual,
                a.unemployed_educated,
                a.vacant_homes,
                a.free_jobs_weighted,
                a.commercial_jobs,
                a.industrial_jobs,
                a.free_office_jobs
            );
        }
        if d.stuck > 0 {
            let w = d.stuck_why;
            eprintln!(
                "      stuck: waiting {} in-junction {} lane-change {} conflict-stands {} full-beyond {} front-other {} hard-cap {} hard-cap-head {}",
                w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7]
            );
        }
        if !d.actions.is_empty() {
            eprintln!("      did: {}", d.actions.join("; "));
        }
        if !d.refused.is_empty() {
            let mut r = d.refused.clone();
            r.sort();
            r.dedup();
            eprintln!("      refused: {}", r.join("; "));
        }
    });
    assert!(!report.is_empty());
}
