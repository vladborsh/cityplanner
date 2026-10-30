---
name: github-ci-unavailable
description: Repo public since 2026-10-27 → GitHub Actions free again; CI runs scripts/ci-gate.sh (sim tests only) and deploys the web build to Pages; local gate is still the merge gate
metadata:
  type: project
---

2026-10-09 to 2026-10-27, Actions failed every job in ~3 s (account billing). On 2026-10-27 the user made vladborsh/cityplanner PUBLIC (MIT OR Apache-2.0) so Actions run free.

CI (`.github/workflows/ci.yml`): `scripts/ci-gate.sh` on push/PR. It runs fmt, clippy for the whole workspace, sim-deps, isolated-clippy, and nextest + doctests for the sim crates only. User rule: only required tests, no GPU/window tests. On master, `web` (`scripts/build-web.sh`) → `deploy` to https://vladborsh.github.io/cityplanner/.

**Why:** the user wants every green master commit playable in the browser.
**How to apply:** the merge gate is still the local `scripts/gate.sh`/quick-gate; CI is a second check plus deploy. The repo is public: never commit secrets, personal paths or non-CC0 assets. Check `gh run list --branch master` after a push. Right after the visibility change, push returned 403 "repository is disabled" for a few seconds, then worked. Related: [[cityplanner-delivery-setup]]
