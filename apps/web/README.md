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

The egui UI works in the browser thanks to the vendored `bevy_egui` patch
(`vendor/bevy_egui/PATCH.md`).

Notes
- Tooling is plain `cargo` + `wasm-bindgen-cli` (no trunk). CI builds the site and uploads
  `dist/web` as the `web-demo` artifact.
- Terrain uses `cp_render::VertexColorMaterial`, which binds no textures. With
  `StandardMaterial` the 257 terrain draws cost >1 s per frame in Chrome (wgpu's GLES
  backend rebinds ~6 textures per draw).
- WebGL2 supports one shadow cascade; Bevy logs a warning for the 4 cascades configured
  for the desktop (it clamps to 1).
