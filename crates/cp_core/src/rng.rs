//! The single sim RNG (ChaCha8). Parallel systems take `rng.fork(stream_id, tick)` once per
//! tick and never share one stream.

use bevy_ecs::resource::Resource;
use rand_chacha::ChaCha8Rng;
use rand_core::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};

use crate::time::Tick;

/// Sim RNG. Wraps ChaCha8 and remembers its seed and position so it can be saved.
///
/// Parallel systems take `rng.fork(stream_id, tick)` once per tick and never share one
/// stream. [`fork`](Self::fork) is a **pure function of `(self.seed, stream_id, tick)`**:
/// it ignores how many numbers the parent has drawn, so the result doesn't depend on call
/// order, and it doesn't advance the parent. Forks of forks work the same way (a fork's
/// seed is its own derived seed). There is deliberately no tick-less fork: it would hand
/// a system the identical sequence every tick.
#[derive(Resource, Clone, Debug)]
pub struct SimRng {
    seed: [u8; 32],
    rng: ChaCha8Rng,
}

impl SimRng {
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self {
            seed,
            rng: ChaCha8Rng::from_seed(seed),
        }
    }

    /// Expands a 64-bit seed (little-endian, zero padded) into a full seed.
    pub fn seed_from_u64(seed: u64) -> Self {
        let mut full = [0u8; 32];
        full[..8].copy_from_slice(&seed.to_le_bytes());
        Self::from_seed(full)
    }

    pub fn seed(&self) -> [u8; 32] {
        self.seed
    }

    /// Derives an independent per-tick stream. The child seed is the 32 keystream bytes
    /// (8 words) of ChaCha8 keyed with this RNG's seed, on ChaCha stream `stream_id`, at
    /// word position `tick * 8`. Distinct ticks therefore read disjoint keystream
    /// windows of the same stream, and distinct stream ids use distinct streams.
    pub fn fork(&self, stream_id: u64, tick: Tick) -> SimRng {
        let mut keyed = ChaCha8Rng::from_seed(self.seed);
        keyed.set_stream(stream_id);
        keyed.set_word_pos(u128::from(tick.0) * 8);
        let mut child = [0u8; 32];
        keyed.fill_bytes(&mut child);
        SimRng::from_seed(child)
    }

    pub fn next_u32(&mut self) -> u32 {
        self.rng.next_u32()
    }

    pub fn next_u64(&mut self) -> u64 {
        self.rng.next_u64()
    }

    /// Uniform in `[0, 1)`.
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Uniform in `0..n` (`n > 0`), by widening multiply (negligible bias for sim use).
    pub fn below(&mut self, n: u32) -> u32 {
        assert!(n > 0, "below(0)");
        ((u64::from(self.next_u32()) * u64::from(n)) >> 32) as u32
    }
}

#[derive(Serialize, Deserialize)]
struct SimRngRepr {
    seed: [u8; 32],
    word_pos: u128,
}

impl Serialize for SimRng {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        SimRngRepr {
            seed: self.seed,
            word_pos: self.rng.get_word_pos(),
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for SimRng {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let repr = SimRngRepr::deserialize(d)?;
        let mut rng = Self::from_seed(repr.seed);
        rng.rng.set_word_pos(repr.word_pos);
        Ok(rng)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(rng: &mut SimRng, n: usize) -> Vec<u64> {
        (0..n).map(|_| rng.next_u64()).collect()
    }

    #[test]
    fn same_seed_same_stream() {
        let mut a = SimRng::seed_from_u64(42);
        let mut b = SimRng::seed_from_u64(42);
        assert_eq!(draw(&mut a, 32), draw(&mut b, 32));
        let mut c = SimRng::seed_from_u64(43);
        assert_ne!(draw(&mut SimRng::seed_from_u64(42), 8), draw(&mut c, 8));
    }

    const T: Tick = Tick(10);

    #[test]
    fn fork_is_deterministic_and_distinct() {
        let a = SimRng::seed_from_u64(7);
        let b = SimRng::seed_from_u64(7);
        assert_eq!(draw(&mut a.fork(3, T), 16), draw(&mut b.fork(3, T), 16));
        // Different streams, same tick.
        assert_ne!(draw(&mut a.fork(3, T), 16), draw(&mut a.fork(4, T), 16));
        assert_ne!(draw(&mut a.fork(3, T), 16), draw(&mut a.clone(), 16));
        // Different parents.
        let other = SimRng::seed_from_u64(8);
        assert_ne!(draw(&mut a.fork(3, T), 16), draw(&mut other.fork(3, T), 16));
    }

    #[test]
    fn consecutive_ticks_differ_on_same_stream() {
        let r = SimRng::seed_from_u64(7);
        let seqs: Vec<_> = (0..64).map(|t| draw(&mut r.fork(3, Tick(t)), 4)).collect();
        for i in 0..seqs.len() {
            for j in i + 1..seqs.len() {
                assert_ne!(seqs[i], seqs[j], "ticks {i} and {j} collided");
            }
        }
        // Large ticks still work.
        assert_ne!(
            draw(&mut r.fork(3, Tick(u64::MAX)), 4),
            draw(&mut r.fork(3, Tick(u64::MAX - 1)), 4)
        );
    }

    #[test]
    fn fork_ignores_parent_consumption() {
        let fresh = SimRng::seed_from_u64(9);
        let mut used = SimRng::seed_from_u64(9);
        draw(&mut used, 100);
        assert_eq!(
            draw(&mut fresh.fork(1, T), 8),
            draw(&mut used.fork(1, T), 8)
        );
        // Forking does not advance the parent.
        let mut p1 = SimRng::seed_from_u64(9);
        let _ = p1.fork(1, T);
        assert_eq!(draw(&mut p1, 8), draw(&mut SimRng::seed_from_u64(9), 8));
    }

    #[test]
    fn nested_forks_deterministic() {
        let r = SimRng::seed_from_u64(1);
        assert_eq!(
            draw(&mut r.fork(1, T).fork(2, T), 8),
            draw(&mut r.fork(1, T).fork(2, T), 8)
        );
        assert_ne!(
            draw(&mut r.fork(1, T).fork(2, T), 8),
            draw(&mut r.fork(2, T).fork(1, T), 8)
        );
        assert_ne!(
            draw(&mut r.fork(1, T).fork(2, Tick(1)), 8),
            draw(&mut r.fork(1, T).fork(2, Tick(2)), 8)
        );
    }

    #[test]
    fn serde_round_trip_resumes_stream() {
        let mut a = SimRng::seed_from_u64(5);
        draw(&mut a, 37);
        let bytes = postcard::to_stdvec(&a).unwrap();
        let mut b: SimRng = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(draw(&mut a, 64), draw(&mut b, 64));
        assert_eq!(draw(&mut a.fork(2, T), 8), draw(&mut b.fork(2, T), 8));
    }

    #[test]
    fn helpers_in_range() {
        let mut r = SimRng::seed_from_u64(0);
        for _ in 0..1000 {
            assert!(r.below(10) < 10);
            let f = r.next_f32();
            assert!((0.0..1.0).contains(&f));
        }
    }
}
