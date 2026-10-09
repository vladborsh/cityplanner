//! Acceptance tests for `m0-core`: runner stepping, command ordering and the periodic
//! checksum, through the public API only.

use bevy_ecs::message::Messages;
use cp_core::{CommandLog, CommandOutcome, CommandQueue, SimSpeed, Tick};
use cp_sim::{MAX_TICKS_PER_STEP, SimChecksum, SimCommand, SimRunner, run_headless};
use std::time::Duration;

fn speed(s: SimSpeed) -> SimCommand {
    SimCommand::SetSpeed { speed: s }
}

fn log(sim: &SimRunner) -> Vec<(Tick, u64, SimCommand)> {
    sim.world()
        .resource::<CommandLog<SimCommand>>()
        .entries()
        .iter()
        .map(|e| (e.tick, e.id.0, e.command.clone()))
        .collect()
}

#[test]
fn same_tick_commands_apply_in_push_order_and_that_order_is_the_replay_order() {
    let mut a = SimRunner::new(1);
    a.push_command(speed(SimSpeed::X3), Tick(3));
    a.push_command(speed(SimSpeed::X2), Tick(3));
    a.push_command(speed(SimSpeed::Pause), Tick(1));
    for _ in 0..6 {
        a.tick_once();
    }
    // Last writer within tick 3 wins; tick 1 ran first.
    assert_eq!(a.speed(), SimSpeed::X2);
    let order: Vec<_> = log(&a).iter().map(|e| (e.0, e.1)).collect();
    assert_eq!(order, vec![(Tick(1), 2), (Tick(3), 0), (Tick(3), 1)]);

    // Pushed in the opposite order -> different ids -> the other command wins.
    let mut b = SimRunner::new(1);
    b.push_command(speed(SimSpeed::X2), Tick(3));
    b.push_command(speed(SimSpeed::X3), Tick(3));
    for _ in 0..6 {
        b.tick_once();
    }
    assert_eq!(b.speed(), SimSpeed::X3);

    // Replaying the log into a fresh sim reproduces the checksum, provided commands are
    // re-pushed in CommandId order (ids are part of the logged/hashed state).
    let mut replay = SimRunner::new(1);
    let mut entries = log(&a);
    entries.sort_by_key(|e| e.1);
    for (t, _, c) in entries {
        replay.push_command(c, t);
    }
    for _ in 0..6 {
        replay.tick_once();
    }
    assert_eq!(replay.checksum(), a.checksum());
}

#[test]
fn a_command_scheduled_in_the_past_runs_on_the_next_tick_and_is_logged_then() {
    let mut s = SimRunner::new(1);
    for _ in 0..10 {
        s.tick_once();
    }
    let id = s.push_command(speed(SimSpeed::X2), Tick(3));
    assert_eq!(s.world().resource::<CommandQueue<SimCommand>>().len(), 1);
    s.tick_once();
    assert_eq!(s.speed(), SimSpeed::X2);
    assert_eq!(log(&s), vec![(Tick(10), id.0, speed(SimSpeed::X2))]);
    let out: Vec<_> = s
        .world()
        .resource::<Messages<CommandOutcome>>()
        .iter_current_update_messages()
        .collect();
    assert_eq!(out.len(), 1);
    assert!(out[0].result.is_ok());
}

#[test]
fn step_boundaries() {
    let ms = Duration::from_millis;
    let mut s = SimRunner::new(1);
    assert_eq!(s.step(Duration::ZERO), 0);
    // 49 x 1 ms owes nothing; the 50th crosses exactly one tick.
    for _ in 0..49 {
        assert_eq!(s.step(ms(1)), 0);
    }
    assert_eq!(s.step(ms(1)), 1);
    assert_eq!(s.tick(), Tick(1));
    // Sub-tick remainders carry over exactly across many frames: 1000 x 7 ms = 140 ticks.
    let mut s = SimRunner::new(1);
    let total: u32 = (0..1000).map(|_| s.step(ms(7))).sum();
    assert_eq!(total, 140);
    // Absurd frame times are capped, never panic or overflow.
    for sp in [SimSpeed::X1, SimSpeed::X3] {
        let mut s = SimRunner::new(1);
        s.world_mut().insert_resource(sp);
        assert_eq!(s.step(Duration::MAX), MAX_TICKS_PER_STEP);
        assert_eq!(s.step(Duration::ZERO), 0, "surplus dropped");
    }
    // Pause owes nothing and keeps nothing: resuming starts from zero debt.
    let mut s = SimRunner::new(1);
    s.world_mut().insert_resource(SimSpeed::Pause);
    assert_eq!(s.step(Duration::from_secs(60)), 0);
    s.world_mut().insert_resource(SimSpeed::X1);
    assert_eq!(s.step(Duration::ZERO), 0);
    assert_eq!(s.step(ms(50)), 1);
}

#[test]
fn periodic_checksum_lands_on_ticks_255_511_and_matches_on_demand_value() {
    let mut s = SimRunner::new(8);
    s.push_command(speed(SimSpeed::X2), Tick(10));
    let mut seen = Vec::new();
    for _ in 0..600 {
        s.tick_once();
        let last = s.world().resource::<SimChecksum>().last;
        if last.map(|l| l.0) != seen.last().map(|l: &(Tick, u64)| l.0)
            && let Some(l) = last
        {
            seen.push(l);
        }
    }
    assert_eq!(
        seen.iter().map(|l| l.0).collect::<Vec<_>>(),
        vec![Tick(255), Tick(511)]
    );
    // The periodic value equals what a fresh identical run reports at that tick.
    let mut r = SimRunner::new(8);
    r.push_command(speed(SimSpeed::X2), Tick(10));
    for _ in 0..256 {
        r.tick_once();
    }
    assert_eq!(
        r.world().resource::<SimChecksum>().last.unwrap().1,
        seen[0].1
    );
    // Computing on demand never changes later results.
    let mut a = SimRunner::new(8);
    let mut b = SimRunner::new(8);
    for i in 0..300 {
        a.tick_once();
        b.tick_once();
        if i % 7 == 0 {
            let _ = a.checksum();
        }
    }
    assert_eq!(a.checksum(), b.checksum());
}

#[test]
fn headless_runs_are_reproducible_and_seed_and_commands_matter() {
    let cmds = [
        (Tick(3), speed(SimSpeed::X3)),
        (Tick(900), speed(SimSpeed::Pause)),
    ];
    let base = run_headless(5, &cmds, 1200);
    for _ in 0..3 {
        assert_eq!(base, run_headless(5, &cmds, 1200));
    }
    assert_ne!(base, run_headless(6, &cmds, 1200));
    assert_ne!(base, run_headless(5, &cmds[..1], 1200));
    assert_ne!(base, run_headless(5, &cmds, 1199));
}
