# apps/desktop

Для игрока (сборка, запуск, управление, первые шаги): [`docs/PLAYING.md`](../../docs/PLAYING.md).
Ниже флаги для агентов и скриптов.

```sh
cargo run --release -p cp_desktop -- [--map flatlands|river_valley] [--cam overview|ground|street|valley]
  [--hour 0..24] [--scene grid:N] [--screenshot out.png] [--bench seconds] [--no-vsync] [--hidden]
cargo run --release -p cp_desktop -- --gallery [--screenshot out.png] [--hidden]
cargo run --release -p cp_desktop -- --map flatlands --headless 512      # no rendering at all
```

## Agents and scripts: always `--hidden`, always under an outer timeout

Windowed modes must never pop a window up on the user's screen, and must never run
unattended without a kill switch:

```sh
perl -e 'alarm 120; exec @ARGV' -- target/release/cp_desktop --hidden --map flatlands --screenshot out.png
```

(macOS has no `timeout`; `perl alarm` kills the process after 120 s.)

* **`--hidden` creates no window at all.** The cameras render into an off-screen image that
  is captured directly. This is the only thing that works on macOS: a window with
  `visible: false`, or an unfocused borderless one placed off-screen, renders nothing there
  (every capture was blank), so those were ruled out by measurement.
* Hidden runs have **no egui UI** (it needs a window), so screenshots show the 3D scene only.
  With `--bench` the event loop runs continuously, so the numbers are uncapped, like
  `--no-vsync`; a screenshot run is paced at 60 Hz (T-245: uncapped, a paused city seen
  obliquely queued frames faster than the GPU drew them and Metal lost the device).
* `--scene night:N --hour H --cam night-wide|night-street|night-dark` (T-245): the city of
  `city:N`, held at hour `H`, with one district without power (lit windows and an outage).
* `--scene post --cam post|post-office|post-sorting|post-van` (T-248): the post office and the
  sorting facility by a street, a mail van on its way between them; held, shown as powered
  (try `--hour 20` for the lit windows and signs).
* `--scene services --cam services --overlay police --place-demo police_station,5470,5132` (T-254):
  a 12 × 12 block grid with a police and a fire station, paused after a coverage recompute, and
  the placement ghost held at (x, z) with its road coverage preview (blue: new, white: already
  covered). `--place-demo <building>,x,z` works with any scene.
* `--scene services-paused --cam services --overlay police` (T-254 step 2): the same grid with a
  third police station placed after the pause; the overlay shows its coverage at once (live).
* `--scene grown --cam place-over --place-demo clinic,5140,5078` (T-252): the placement ghost of a
  clinic held over grown houses: the lot in green drawn over the roofs, the zoned buildings it would
  demolish tinted red whole (T-252b) (`--place-demo def,x,z` works with any scene).
* `--scene grid-shots --cam grid-shot1|grid-shot2|grid-shot3|grid-shot4` (T-255): the four grid
  layouts of the user's BUG-073 shots, each built through the road tool's own snap and tick code
  (`cp_app::grid_shots`), held paused with the ghost of its last street (the zone strip, the ticks
  and guides): 1 the L closed from the left street's tick, 2 a street extended from a rounded bend,
  3 two parallels closed on the right, 4 a block closed from the top street's tick. Screenshots
  `docs/screenshots/t255-grid-shotN-before.png` (e7fa39c6, before the BUG-073 fix) and
  `t255-grid-shotN-after.png` (after it; the ticks are the streets' centreline stations).
* `--scene joins --cam joins|joins-N` (T-261): the junction gallery of the `junction_sweep` test
  (`cp_sim::road_cases::join_cases`: bends of 5-135 degrees for every road, curved arms, T, X and Y
  junctions, width changes, a street into a highway, automatic cuts), one case per 260 m cell;
  `joins-N` looks closely at case `N`. `--scene place --cam place|place-N`: the service sites of
  `place_sweep` with `basic_2l` and the largest service lot placed at every spot (refusals are
  logged). Before/after proof for the road redesign (§6.8, T-262…T-268).
* `--screenshot` never saves a blank frame (BUG-012): blank captures are discarded and
  retried; if no real frame arrives within 20 s it exits with code **3**; a write error exits
  **4**.
* A **watchdog thread** ends the process with code **3** and a message after 90 s (plus the
  `--bench` time) even if the app stops producing frames altogether, so a hung run can never
  leave a window open for ever. `CP_CAPTURE_DEADLINE_SECS=<n>` shortens the limit (used to
  prove it: `CP_CAPTURE_DEADLINE_SECS=2 ... --hidden --screenshot x.png` exits 3 after 2 s).
