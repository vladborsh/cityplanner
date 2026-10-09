//! Deterministic state checksum.
//!
//! The hash is FNV-1a (64 bit), implemented here — never `DefaultHasher`. It covers the
//! tick, the full `SimRng` state, the command log, the pending `CommandQueue` (incl.
//! `next_id`), `SimSpeed`, `TileOwnership` (all three hashed via postcard, the same
//! serialized form as in saves), and every *part* that domains register. Parts are hashed in registration order (plugin order is fixed), each
//! preceded by its name.
//!
//! **Extending it:** a domain registers a function `fn(&World, &mut Fnv1a)` from its
//! [`SimPlugin`](crate::SimPlugin) via `SimBuilder::register_checksum`. The function
//! must feed its state in a deterministic order (arenas iterate by index; sort anything
//! else). [`Fnv1a::write_serialized`] hashes any `Serialize` value (e.g. a whole
//! `Arena<T>`) through postcard.
//!
//! The checksum is computed in the `Stats` phase when a 256-tick period completes and
//! stored in [`SimChecksum::last`]; [`SimChecksum::compute`] gives it on demand.

use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use cp_core::{CommandLog, CommandQueue, SimRng, SimSpeed, Tick};
use cp_terrain::TileOwnership;
use serde::Serialize;

use crate::command::SimCommand;

/// Ticks between periodic checksums.
pub const CHECKSUM_PERIOD: u64 = 256;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a 64-bit streaming hasher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fnv1a(u64);

impl Default for Fnv1a {
    fn default() -> Self {
        Self::new()
    }
}

impl Fnv1a {
    pub const fn new() -> Self {
        Self(FNV_OFFSET)
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(FNV_PRIME);
        }
    }

    pub fn write_u64(&mut self, v: u64) {
        self.write_bytes(&v.to_le_bytes());
    }

    pub fn write_u128(&mut self, v: u128) {
        self.write_bytes(&v.to_le_bytes());
    }

    /// Hashes the postcard encoding of `value` (length-prefixed).
    pub fn write_serialized<T: Serialize>(&mut self, value: &T) {
        let bytes = postcard::to_stdvec(value).expect("checksum serialization failed");
        self.write_u64(bytes.len() as u64);
        self.write_bytes(&bytes);
    }

    pub fn finish(&self) -> u64 {
        self.0
    }
}

/// A domain's contribution to the checksum.
pub type ChecksumFn = fn(&World, &mut Fnv1a);

/// Checksum part of the pathfinding requests in flight (`cp_traffic::PathService`).
pub(crate) fn paths_part(world: &World, h: &mut Fnv1a) {
    h.write_serialized(&world.resource::<cp_traffic::PathService>().export());
}

/// Checksum part of the zone blocks (`cp_zone::ZoneGrid`: blocks, painted types, validity).
pub(crate) fn zone_part(world: &World, h: &mut Fnv1a) {
    h.write_serialized(world.resource::<cp_zone::ZoneGrid>());
}

/// Checksum part of the buildings (`cp_building::BuildingGrid`).
pub(crate) fn building_part(world: &World, h: &mut Fnv1a) {
    h.write_serialized(world.resource::<cp_building::BuildingGrid>());
}

/// Checksum part of citizens and households (`cp_citizen::CitizenGrid`).
pub(crate) fn citizen_part(world: &World, h: &mut Fnv1a) {
    h.write_serialized(world.resource::<cp_citizen::CitizenGrid>());
}

/// Checksum part of the road network (`cp_net::NetGraph`: arenas and dirty set).
/// Registered by `SimRunner::new` — `cp_net` cannot depend on `cp_sim`, so built-in
/// domains register here rather than through a `SimPlugin`.
pub(crate) fn net_part(world: &World, h: &mut Fnv1a) {
    h.write_serialized(world.resource::<cp_net::NetGraph>());
}

/// Checksum registry and the latest periodic value.
#[derive(Resource, Default)]
pub struct SimChecksum {
    parts: Vec<(&'static str, ChecksumFn)>,
    /// Command-log entries already folded into `log_hash`.
    log_hashed: usize,
    log_hash: Fnv1a,
    /// `(tick, value)` of the most recent periodic checksum. `tick` is the tick whose
    /// `Stats` phase computed it.
    pub last: Option<(Tick, u64)>,
}

impl SimChecksum {
    /// Drops the incremental command-log hash. Call whenever the `CommandLog` resource
    /// is replaced wholesale (save restore): the cache is only valid for a log that
    /// has grown append-only since it was folded. `compute` also self-heals if the log
    /// became shorter than what was folded, but an equal-or-longer replacement can only
    /// be caught by this explicit reset.
    pub fn reset_log_cache(&mut self) {
        self.log_hashed = 0;
        self.log_hash = Fnv1a::new();
        self.last = None;
    }

    pub fn register(&mut self, name: &'static str, part: ChecksumFn) {
        self.parts.push((name, part));
    }

    /// Checksum of the world as it is now. Folds only log entries added since the last
    /// call into a running hash, so cost stays proportional to new commands.
    pub fn compute(&mut self, world: &World) -> u64 {
        let log = world.resource::<CommandLog<SimCommand>>();
        if log.len() < self.log_hashed {
            self.reset_log_cache();
        }
        for e in &log.entries()[self.log_hashed..] {
            self.log_hash.write_serialized(e);
        }
        self.log_hashed = log.len();

        let mut h = Fnv1a::new();
        h.write_u64(world.resource::<Tick>().0);
        let rng = world.resource::<SimRng>();
        h.write_bytes(&rng.seed());
        h.write_u128(rng.word_pos());
        h.write_u64(log.len() as u64);
        h.write_u64(self.log_hash.finish());
        h.write_serialized(world.resource::<CommandQueue<SimCommand>>());
        h.write_serialized(world.resource::<SimSpeed>());
        h.write_serialized(world.resource::<TileOwnership>());
        for (name, part) in &self.parts {
            h.write_bytes(name.as_bytes());
            part(world, &mut h);
        }
        h.finish()
    }
}

/// `Stats`-phase system: records a checksum each time a 256-tick period completes.
pub(crate) fn update_checksum(world: &mut World) {
    let tick = *world.resource::<Tick>();
    if !(tick.0 + 1).is_multiple_of(CHECKSUM_PERIOD) {
        return;
    }
    world.resource_scope(
        |world, mut cs: bevy_ecs::change_detection::Mut<SimChecksum>| {
            let value = cs.compute(world);
            cs.last = Some((tick, value));
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_known_vectors() {
        let mut h = Fnv1a::new();
        assert_eq!(h.finish(), 0xcbf2_9ce4_8422_2325);
        h.write_bytes(b"a");
        assert_eq!(h.finish(), 0xaf63_dc4c_8601_ec8c);
        let mut h = Fnv1a::new();
        h.write_bytes(b"foobar");
        assert_eq!(h.finish(), 0x8594_4171_f739_67e8);
    }

    use crate::command::SimCommand;
    use crate::runner::SimRunner;

    #[test]
    fn covers_tile_ownership() {
        let mut a = SimRunner::new(3);
        let c1 = a.checksum();
        a.world_mut().insert_resource(TileOwnership::only((1, 1)));
        assert_ne!(c1, a.checksum());
    }

    #[test]
    fn covers_sim_speed() {
        let mut a = SimRunner::new(3);
        let c1 = a.checksum();
        a.world_mut().insert_resource(SimSpeed::X3);
        assert_ne!(c1, a.checksum());
    }

    #[test]
    fn covers_pending_commands() {
        let mut a = SimRunner::new(3);
        let mut b = SimRunner::new(3);
        assert_eq!(a.checksum(), b.checksum());
        let c0 = a.checksum();
        a.push_command(
            SimCommand::SetSpeed {
                speed: SimSpeed::X2,
            },
            Tick(5),
        );
        let c1 = a.checksum();
        assert_ne!(c0, c1);
        b.push_command(
            SimCommand::SetSpeed {
                speed: SimSpeed::X2,
            },
            Tick(6),
        );
        assert_ne!(c1, b.checksum());
    }
}
