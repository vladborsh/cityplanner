# Vendored `bevy_egui` 0.43.0-rc.1 (patched)

- **Upstream:** `bevy_egui` 0.43.0-rc.1 from crates.io (https://github.com/vladbat00/bevy_egui),
  the release that supports Bevy 0.20. Pinned `=0.43.0-rc.1` in the root `Cargo.toml`.
- **Licence:** MIT (see `LICENSE`, unchanged).
- **Wired by:** `[patch.crates-io] bevy_egui = { path = "vendor/bevy_egui" }` in the root
  `Cargo.toml`; `vendor/` is excluded from the workspace (no lints, no tests of its own).
- **Why:** `src/render/egui.wesl` began with an unconditional `enable wgpu_binding_array;`.
  WebGL2 (wgpu's GLES backend) has no binding arrays, so the shader module failed validation
  and no egui was drawn in the browser. The directive is only needed in the `BINDLESS`
  variant.
- **Everything else is the unmodified crates.io source** (minus the `.cargo-ok` marker).

## The diff

```diff
--- a/src/render/egui.wesl
+++ b/src/render/egui.wesl
@@ -1,3 +1,4 @@
+@if(BINDLESS)
 enable wgpu_binding_array;
 
 struct Transform {
```

## Removal condition

Delete `vendor/bevy_egui`, the `[patch.crates-io]` entry and the `exclude` line when a
`bevy_egui` release for Bevy 0.20 contains the fix (or an equivalent one), and bump the pin.
No upstream issue has been opened (pending the Tech Lead's go-ahead).
