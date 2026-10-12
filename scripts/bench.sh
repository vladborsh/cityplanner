#!/usr/bin/env bash
# Local bench runner (GitHub CI is unavailable): `scripts/bench.sh [10k|100k] [--update-baseline]`.
#
# Loads the grid_city fixture, runs N timed ticks RUNS times (default 5), aggregates the runs
# (per-phase median of the run medians), prints ms per phase, writes the JSON result to
# $BENCH_OUT/bench-<size>.json (default <target dir>/bench) and compares it with
# bench/baseline-<size>.json (exit 1 on a phase > max(baseline, 0.05 ms) x 1.20).
#
# fixtures/saves/bench-10k.sav is committed. bench-100k.sav (9 MB) is gitignored and built on
# demand with `cp_bench fixture` (about 80 s in release); its state checksum must equal the
# one recorded in bench/baseline.json ("fixtures"), otherwise the build is not the same city.
# `--update-baseline` stores the result as the new bench/baseline-<size>.json: do it
# deliberately, from a quiet machine, and say which machine in the commit.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

size="${1:-10k}"
update=0
[[ "${2:-}" == "--update-baseline" ]] && update=1
case "$size" in
  10k)  pop=10000;  ticks=600; warmup=100 ;;
  100k) pop=100000; ticks=200; warmup=50 ;;
  *) echo "usage: scripts/bench.sh [10k|100k] [--update-baseline]" >&2; exit 2 ;;
esac
runs="${RUNS:-5}"
target="${CARGO_TARGET_DIR:-target}"
out="${BENCH_OUT:-$target/bench}"
fixture="fixtures/saves/bench-$size.sav"
bin="$target/release/cp_bench"
mkdir -p "$out"

cargo build --release -p cp_bench

if [[ ! -f "$fixture" ]]; then
  echo "building $fixture (grid_city pop $pop, seed 1)"
  line="$("$bin" fixture --pop "$pop" --seed 1 --map flatlands --out "$fixture")"
  echo "$line"
  want="$(sed -n "s/.*\"bench-$size\"[^}]*\"checksum\": *\"\([0-9a-f]*\)\".*/\1/p" bench/baseline.json | head -1)"
  got="$(sed -n 's/.*checksum \([0-9a-f]*\)$/\1/p' <<<"$line")"
  if [[ -z "$want" || "$want" != "$got" ]]; then
    echo "FAIL: $fixture has checksum '$got', bench/baseline.json fixtures say '$want'" >&2
    rm -f "$fixture"
    exit 1
  fi
fi

for i in $(seq 1 "$runs"); do
  "$bin" --map flatlands --save "$fixture" --ticks "$ticks" --warmup "$warmup" \
    --out "$out/run-$size-$i.json" > /dev/null
done
runfiles=()
for i in $(seq 1 "$runs"); do runfiles+=("$out/run-$size-$i.json"); done
"$bin" aggregate "$out/bench-$size.json" "${runfiles[@]}"
echo "result: $out/bench-$size.json"

baseline="bench/baseline-$size.json"
if [[ "$update" == 1 ]]; then
  cp "$out/bench-$size.json" "$baseline"
  echo "baseline updated: $baseline"
elif [[ -f "$baseline" ]]; then
  "$bin" compare "$baseline" "$out/bench-$size.json"
else
  echo "no $baseline yet: run with --update-baseline to record one"
fi
