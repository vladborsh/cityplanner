---
name: sccache-setup
description: sccache shared compile cache enabled 2026-10-21 — global rustc-wrapper; cross-slot hits need every slot's target dir listed in basedirs
metadata:
  type: reference
---

Enabled 2026-10-21 (user: "запускай sccache"): brew sccache 0.18; `~/.cargo/config.toml` [build] rustc-wrapper = /opt/homebrew/bin/sccache; config `~/Library/Application Support/Mozilla.sccache/config` with `[cache.disk] size = 50 GiB` and `basedirs = [...]` listing EVERY slot target dir (.cargo-target/dev-1..4, architect, reviewer, tester, tl-land, coord target). Cache in ~/Library/Caches/Mozilla.sccache.
Measured: without basedirs, a different CARGO_TARGET_DIR missed 100 % (paths are in the key); with per-slot basedirs, 64/64 hits across slots. Workspace crates (incremental) are non-cacheable; deps are cached. Cold full workspace build was 10.5 min.
**How to apply:** a NEW slot/target dir must be added to basedirs (then `sccache --stop-server; sccache --start-server`), else it gets no hits. Related: [[disk-target-dirs]]
