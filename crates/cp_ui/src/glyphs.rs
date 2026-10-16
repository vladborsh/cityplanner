//! BUG-039: every non-ASCII character a UI string uses must have a glyph in the loaded fonts,
//! or egui draws a square ("tofu"). The test reads the sources of the UI crates, takes the
//! characters outside ASCII from everything that is not a `//` comment, and asks egui's fonts.

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use bevy_egui::egui;

    /// Non-ASCII characters of the code (not comments) of `dir`'s `.rs` files, with one place
    /// each.
    fn characters(dir: &Path, found: &mut BTreeMap<char, String>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                characters(&path, found);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                for (n, line) in text.lines().enumerate() {
                    let code = line.split("//").next().unwrap_or("");
                    for c in code.chars().filter(|c| !c.is_ascii()) {
                        found.entry(c).or_insert_with(|| {
                            format!("{}:{}", path.file_name().unwrap().to_string_lossy(), n + 1)
                        });
                    }
                }
            }
        }
    }

    #[test]
    fn every_non_ascii_character_in_the_ui_has_a_glyph() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut found = BTreeMap::new();
        for krate in ["cp_ui", "cp_tools", "cp_app"] {
            characters(&root.join(krate).join("src"), &mut found);
        }
        assert!(found.len() > 3, "the scan found nothing: {found:?}");

        let ctx = egui::Context::default();
        // One pass so the default fonts are loaded.
        crate::testutil::frame(
            &ctx,
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                ..Default::default()
            },
            |_| {},
        );
        let font = egui::FontId::proportional(13.0);
        let missing: Vec<String> = found
            .iter()
            .filter(|(c, _)| !ctx.fonts_mut(|f| f.has_glyph(&font, **c)))
            .map(|(c, at)| format!("{c} U+{:04X} ({at})", *c as u32))
            .collect();
        assert!(missing.is_empty(), "no glyph for: {missing:#?}");
    }
}
