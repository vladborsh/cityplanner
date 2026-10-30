---
name: target-sweep-breaks-cargo
description: deleting old-mtime files from a cargo target dir breaks builds (live rlibs keep old mtimes); fix by clearing the profile dir
metadata:
  node_type: memory
  type: project
  originSessionId: 3bfee97b-8581-4155-bcf7-2d65011bd33d
  modified: 2026-10-23T18:24:46.306Z
---

2026-10-23: the TL freed disk by deleting files older than a day from ../.cargo-target/architect. Cargo keeps old mtimes on live dependency rlibs, so the fingerprints stayed "fresh" while the rlibs were gone, and the gate failed at link time ("clang: no such file ... lib*.rlib"). The TL said they won't sweep my dir again.

**Why:** a link error naming a missing `deps/lib*.rlib` is a swept target dir, not a code bug. Deleting fingerprints by matching hashes is unreliable (fingerprint hash differs from the file hash).

**How to apply:** clear the whole profile dir of your own slot (`rm -rf ../.cargo-target/<slot>/debug`); sccache refills it. Never run the clear and a build as two copies of one command at once: one copy's rm races the other's build ("failed to write .fingerprint/..."). Related: [[disk-target-dirs]], [[sccache-setup]].
