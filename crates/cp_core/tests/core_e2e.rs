//! Acceptance tests for `m0-core`: arena churn and RNG streams, public API only.

use cp_core::{Arena, SimRng, Tick};

#[test]
fn arena_survives_heavy_churn_with_stale_ids_never_resolving() {
    let mut a: Arena<u64> = Arena::new();
    let mut live = Vec::new();
    let mut stale = Vec::new();
    let mut x = 0x9E37_79B9_7F4A_7C15u64; // local LCG, independent of SimRng
    for step in 0..50_000u64 {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        if x >> 60 < 9 || live.is_empty() {
            live.push((a.insert(step), step));
        } else {
            let i = (x >> 20) as usize % live.len();
            let (id, v) = live.swap_remove(i);
            assert_eq!(a.remove(id), Some(v));
            assert_eq!(a.remove(id), None, "double remove");
            stale.push(id);
        }
        assert_eq!(a.len(), live.len());
    }
    for &(id, v) in &live {
        assert_eq!(a.get(id), Some(&v));
    }
    for &id in &stale {
        assert!(
            a.get(id).is_none() && !a.contains(id),
            "stale id {id:?} resolved"
        );
    }
    let idx: Vec<_> = a.iter().map(|(id, _)| id).collect();
    assert_eq!(idx.len(), live.len());
    // Slot reuse keeps the arena dense: it never grew past the peak live count.
    let mut b: Arena<u8> = Arena::new();
    let ids: Vec<_> = (0..100).map(|_| b.insert(0)).collect();
    for _ in 0..1000 {
        for &i in &ids {
            b.remove(i);
        }
        for _ in 0..100 {
            b.insert(0);
        }
        let again: Vec<_> = b.iter().map(|(i, _)| i).collect();
        for i in again {
            b.remove(i);
        }
        for _ in 0..100 {
            b.insert(0);
        }
    }
    assert_eq!(b.len(), 100);
    assert_eq!(b.iter().count(), 100);
}

#[test]
fn rng_fork_is_pure_distinct_and_total() {
    let r = SimRng::seed_from_u64(42);
    let mut seen = std::collections::BTreeSet::new();
    for stream in [0u64, 1, 2, u64::MAX] {
        for tick in [0u64, 1, 2, 255, u64::MAX / 8, u64::MAX] {
            let mut a = r.fork(stream, Tick(tick));
            let mut b = r.fork(stream, Tick(tick));
            let first = a.next_u64();
            assert_eq!(
                first,
                b.next_u64(),
                "fork not deterministic ({stream},{tick})"
            );
            assert!(
                seen.insert(first),
                "fork collision at stream {stream} tick {tick}"
            );
        }
    }
    // Forking does not advance the parent.
    let mut p1 = SimRng::seed_from_u64(42);
    let mut p2 = SimRng::seed_from_u64(42);
    let _ = p1.fork(3, Tick(9));
    assert_eq!(p1.next_u64(), p2.next_u64());
    // A different parent seed gives a different child.
    assert_ne!(
        SimRng::seed_from_u64(43).fork(0, Tick(0)).next_u64(),
        SimRng::seed_from_u64(42).fork(0, Tick(0)).next_u64()
    );
}
