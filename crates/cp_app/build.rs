//! Lists every `assets/content/**/*.ron` so the app can embed the content (the browser build
//! has no file system, and scenes need building and road definitions).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn collect(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        let rel_name = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        if path.is_dir() {
            collect(&path, &rel_name, out);
        } else if name.ends_with(".ron") {
            out.push((rel_name, path));
        }
    }
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    collect(&root, "", &mut files);
    let mut src = String::from("pub const FILES: &[(&str, &str)] = &[\n");
    for (rel, path) in files {
        let abs = path.canonicalize().expect("content file");
        println!("cargo:rerun-if-changed={}", abs.display());
        writeln!(
            src,
            "    ({rel:?}, include_str!({:?})),",
            abs.to_string_lossy()
        )
        .unwrap();
    }
    src.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("content_files.rs");
    std::fs::write(out, src).expect("write content_files.rs");
}
