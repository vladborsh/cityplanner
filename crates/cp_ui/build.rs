//! Builds the table of the icons of content defs (T-105): every `assets/ui/icons/*.png`, by
//! file stem, compiled in with `include_bytes!` so a desktop and a WASM build carry the same
//! bytes and a def with a new icon needs no Rust change.

use std::fmt::Write as _;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let dir = manifest.join("../../assets/ui/icons");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "png"))
        .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(str::to_string))
        .collect();
    names.sort();
    let mut out = String::from("pub const DEF_ICONS: &[(&str, &[u8])] = &[\n");
    for name in &names {
        let path = dir.join(format!("{name}.png"));
        let path = path.canonicalize().expect("icon path");
        writeln!(
            out,
            "    ({name:?}, include_bytes!({:?})),",
            path.to_string_lossy()
        )
        .expect("write");
    }
    out.push_str("];\n");
    let target = PathBuf::from(std::env::var("OUT_DIR").expect("out dir")).join("def_icons.rs");
    std::fs::write(target, out).expect("write table");
}
