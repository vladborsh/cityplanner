//! Determinism: a dummy system using `SimRng::fork` and `par_iter_mut` over thousands of
//! entities must give identical checksums in two runs, and a different one for another seed.

use bevy_ecs::component::Component;
use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Query, Res, ResMut};
use bevy_ecs::world::World;
use cp_core::{SimRng, SimSpeed, Tick};
use cp_sim::{Fnv1a, SimBuilder, SimCommand, SimPhase, SimPlugin, SimRunner, run_headless_with};

const ENTITIES: u32 = 3000;
const TICKS: u64 = 300; // crosses one periodic checksum (tick 255)

#[derive(Component)]
struct Index(u32);
#[derive(Component)]
struct Roll(u64);
#[derive(Resource, Default)]
struct Digest(u64);

/// Per-entity independent stream: `fork(entity index, tick)`; written in parallel.
fn roll(rng: Res<SimRng>, tick: Res<Tick>, mut q: Query<(&Index, &mut Roll)>) {
    q.par_iter_mut().for_each(|(i, mut r)| {
        let mut child = rng.fork(u64::from(i.0), *tick);
        r.0 = r.0.wrapping_add(child.next_u64());
    });
}

/// Folds results in sorted (index) order into a resource the checksum reads.
fn fold(q: Query<(&Index, &Roll)>, mut d: ResMut<Digest>) {
    let mut all: Vec<(u32, u64)> = q.iter().map(|(i, r)| (i.0, r.0)).collect();
    all.sort_unstable();
    let mut h = Fnv1a::new();
    for (i, r) in all {
        h.write_u64(u64::from(i));
        h.write_u64(r);
    }
    d.0 = h.finish();
}

struct Dummy;
impl SimPlugin for Dummy {
    fn build(&self, sim: &mut SimBuilder) {
        let world: &mut World = sim.world_mut();
        world.init_resource::<Digest>();
        world.spawn_batch((0..ENTITIES).map(|i| (Index(i), Roll(0))));
        sim.add_systems(SimPhase::Agents, roll);
        sim.add_systems(SimPhase::Stats, fold);
        sim.register_checksum("dummy", |w, h| h.write_u64(w.resource::<Digest>().0));
    }
}

fn run(seed: u64) -> u64 {
    let cmds = [(
        Tick(7),
        SimCommand::SetSpeed {
            speed: SimSpeed::X3,
        },
    )];
    run_headless_with(seed, &cmds, TICKS, |sim: &mut SimRunner| {
        sim.add_plugin(&Dummy);
    })
}

#[test]
fn same_seed_same_checksum() {
    let a = run(42);
    for _ in 0..3 {
        assert_eq!(run(42), a);
    }
}

#[test]
fn different_seed_different_checksum() {
    assert_ne!(run(42), run(43));
}

#[test]
fn commands_change_the_checksum() {
    let base = cp_sim::run_headless(1, &[], 50);
    let with = cp_sim::run_headless(
        1,
        &[(
            Tick(3),
            SimCommand::SetSpeed {
                speed: SimSpeed::X2,
            },
        )],
        50,
    );
    assert_ne!(base, with);
    assert_eq!(base, cp_sim::run_headless(1, &[], 50));
}
