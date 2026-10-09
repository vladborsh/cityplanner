# Agent rules for this repository

Every coding agent working here reads this file. Codex reads it directly; Claude Code
loads it through `CLAUDE.md`. Keep it short: only rules an agent would otherwise get
wrong. Project context lives in `PROJECT.md`; system designs live in `architecture/`.

## Secrets

- Never read, print, log or commit any `.env` file. `.env` files are gitignored; keep them
  that way.

## Build and test

- Green means **both** `cargo test --workspace` and
  `cargo clippy --workspace --all-targets -- -D warnings` pass, plus `cargo fmt --check`.
  Report the test count.
- Never change the Bevy version pinned in the root `Cargo.toml` as a side effect of a
  task. A version bump is its own task.
- Do not add a dependency to a `cp_*` sim crate without saying so in the review request.

## Simulation crates are headless

Sim crates are every crate that `cp_sim` pulls in: `cp_core`, `cp_content`, `cp_terrain`,
`cp_net`, `cp_zone`, `cp_building`, `cp_citizen`, `cp_traffic`, `cp_utility`,
`cp_service`, `cp_economy`, `cp_transit`, `cp_district`, `cp_industry`, `cp_park`,
`cp_save` and `cp_sim` itself.

- **No rendering, windowing or input dependencies.** A sim crate may use `bevy_ecs`,
  `bevy_app`, `bevy_math`, `bevy_reflect` and `bevy_utils`. It must never depend on
  `bevy_render`, `bevy_pbr`, `bevy_asset` loaders, `winit`, `bevy_egui` or the `bevy`
  umbrella crate. `cargo tree -p <crate>` is the check.
- **Player input enters the sim only as commands** (the `cp_core` command queue).
  Render and UI crates read sim state; they never mutate it.
- **Every sim feature ships with a headless test**: build a small world in code or load a
  fixture save, run N ticks, assert on state. A feature whose only proof is "I looked at
  it in the window" is not done.

## Determinism

The sim must replay bit-identically from `(save, seed, command log)` on the **same binary
and platform**. We do **not** promise cross-platform or cross-compiler determinism (no
lockstep multiplayer), so `f32` math is fine. Within that scope:

- **No iteration over `HashMap` or `HashSet` in sim code.** Use `BTreeMap`, `IndexMap`,
  dense `Vec`s keyed by id, or collect and sort before iterating. Lookups without iteration
  are fine.
- **No wall clock in sim code.** No `Instant`, `SystemTime` or Bevy's `Time<Real>`/`Time<Virtual>`.
  Sim time is the tick counter from `cp_core`.
- **RNG only from the sim RNG resource** in `cp_core`. Never `rand::thread_rng()` and never
  an RNG seeded from time. Derive sub-streams from the sim RNG; don't share one stream
  across parallel systems.
- **Parallel systems must not race on order-dependent results.** If two systems write the
  same data, give them an explicit order. If a parallel query accumulates into a shared
  result, sort or reduce it deterministically.
- A determinism test (run the reference save twice, compare state hashes) lives in CI.
  Never mark it ignored to get green.

## Content and saves

- Building, road and vehicle types are data in `assets/content/*.ron`. Don't hardcode a
  type in Rust that belongs in content.
- Any change to a serialized sim type bumps the save version and adds a migration plus a
  test that loads the previous version's fixture. Never edit a committed fixture save to
  make a test pass.

## Working in a worktree

Each slot has its own target dir (so parallel builds don't fight over one lock) and its
own save/config dir (so one session's saves never show up in another's).

| Slot | `CARGO_TARGET_DIR` | `CITYPLANNER_DATA_DIR` |
|---|---|---|
| Coordinator / Tech Lead checkout | `target/` (default) | `.data/coord` |
| dev-1 | `../.cargo-target/dev-1` | `.data/dev-1` |
| dev-2 | `../.cargo-target/dev-2` | `.data/dev-2` |
| dev-3 | `../.cargo-target/dev-3` | `.data/dev-3` |
| Tester | `../.cargo-target/tester` | `.data/tester` |

If you are not one of the slots above, take a new `../.cargo-target/<name>` and
`.data/<name>`, and say which one you took. `.data/` is gitignored.

## Assets

- Only CC0 or self-made assets. Record every imported pack (name, URL, licence) in
  `assets/LICENSES.md` in the same commit that adds it.
