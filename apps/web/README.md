# apps/web — browser build

The same game app as `apps/desktop` (`cp_app::GamePlugin`) on WebGL2: single-threaded,
`SimLimits::wasm()`, maps fetched over HTTP.

```sh
cargo install wasm-bindgen-cli --version 0.2.129 --locked   # must equal Cargo.lock
scripts/build-web.sh                                         # -> dist/web
python3 -m http.server -d dist/web 8080                      # open http://localhost:8080
```

URL parameters: `?map=flatlands|river_valley`, `?hour=0..24`, `?cam=overview|ground|valley`.
Controls (RTS camera): WASD / middle-drag pan, Q/E or right-drag rotate, wheel zoom.

Known gap: **no egui UI in the browser yet.** `bevy_egui` 0.43.0-rc.1's `egui.wesl` starts
with an unconditional `enable wgpu_binding_array;`, which WebGL2 cannot compile, so
`GamePlugin { ui: false }` is used here. Fix options: upstream `@if(BINDLESS)` on that
directive, a bevy_egui bump once released, or a vendored patch via `[patch.crates-io]`.

Notes
- Tooling is plain `cargo` + `wasm-bindgen-cli` (no trunk). CI builds the site and uploads
  `dist/web` as the `web-demo` artifact.
- Terrain uses `cp_render::VertexColorMaterial`, which binds no textures. With
  `StandardMaterial` the 257 terrain draws cost >1 s per frame in Chrome (wgpu's GLES
  backend rebinds ~6 textures per draw).
- WebGL2 supports one shadow cascade; Bevy logs a warning for the 4 cascades configured
  for the desktop (it clamps to 1).
