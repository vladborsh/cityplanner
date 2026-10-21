//! T-284: every text the UI asks for is in both locale files. The keys are collected from the
//! sources of the client crates (`tr("…")`, `trn("…", …)`, `trf!("…", …)`, `trf!(plural "…", …)`)
//! and from what the UI translates by name (the names of buildings, roads, policies and
//! milestones in the content). A plural key must hold the forms of each language.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use cp_locale::{Lang, Text, locale};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
/// The crates whose sources are read for keys.
const CRATES: [&str; 4] = ["cp_ui", "cp_tools", "cp_app", "cp_render"];

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The value of the Rust string literal starting at `src[0] == '"'`, and its length in bytes.
fn literal(src: &str) -> Option<(String, usize)> {
    let mut out = String::new();
    let mut chars = src.char_indices().skip(1);
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((out, i + 1)),
            '\\' => match chars.next()?.1 {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                '\'' => out.push('\''),
                '\n' => {
                    // A line continuation: the next line's leading spaces are dropped.
                    while let Some((_, c)) = chars.clone().next() {
                        if c.is_whitespace() {
                            chars.next();
                        } else {
                            break;
                        }
                    }
                }
                c => panic!("escape \\{c} in a key"),
            },
            c => out.push(c),
        }
    }
    None
}

/// `(key, plural)` of every call in `src`: `tr(`, `trn(`, `trf!(` and `trf!(plural`, then
/// whitespace (rustfmt may break the line) and the key's literal.
fn keys_in(src: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    for (pat, plural) in [("tr(", false), ("trn(", true), ("trf!(", false)] {
        let mut from = 0;
        while let Some(at) = src[from..].find(pat) {
            let at = from + at;
            from = at + pat.len();
            // `tr(` inside a longer name (`attr(`, `str(`) is not a call.
            if src[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
            {
                continue;
            }
            let mut rest = src[from..].trim_start();
            let mut plural = plural;
            if pat == "trf!("
                && let Some(r) = rest.strip_prefix("plural")
            {
                rest = r.trim_start();
                plural = true;
            }
            if rest.starts_with('"')
                && let Some((key, _)) = literal(rest)
            {
                out.push((key, plural));
            }
        }
    }
    out
}

fn code_keys() -> BTreeSet<(String, bool)> {
    let mut files = Vec::new();
    for c in CRATES {
        rs_files(&Path::new(ROOT).join("..").join(c).join("src"), &mut files);
    }
    files.sort();
    let mut keys = BTreeSet::new();
    for f in files {
        let src = std::fs::read_to_string(&f).unwrap();
        keys.extend(keys_in(&src));
    }
    keys
}

/// The names the UI translates as they come from the content.
fn content_keys() -> BTreeSet<String> {
    let db = cp_content::ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let mut keys = BTreeSet::new();
    keys.extend(db.buildings().map(|(_, d)| d.name.clone()));
    keys.extend(db.roads().map(|(_, d)| d.name.clone()));
    keys.extend(db.policies().iter().map(|d| d.name.clone()));
    keys.extend(db.milestones().iter().map(|d| d.name.clone()));
    // Labels of enums the UI shows through `tr(x.label())`.
    keys.extend(
        cp_render::Overlay::ALL
            .iter()
            .map(|o| o.label().to_string()),
    );
    keys.extend(
        cp_sim::city_services::Sector::ALL
            .iter()
            .map(|s| s.label().to_string()),
    );
    keys.extend(cp_ui::Category::ALL.iter().map(|c| c.label().to_string()));
    keys.extend(
        cp_tools::DistrictToolKind::ALL
            .iter()
            .map(|k| k.label().to_string()),
    );
    {
        use cp_content::ZoneType as Z;
        use cp_tools::ZoneToolKind as K;
        for z in [
            Z::ResLow,
            Z::ResHigh,
            Z::ComLow,
            Z::ComHigh,
            Z::Industrial,
            Z::Office,
        ] {
            keys.insert(cp_tools::zone_name(z).to_string());
        }
        for k in [
            K::FillBlock,
            K::BrushSmall,
            K::BrushLarge,
            K::Rectangle,
            K::Eraser,
        ] {
            keys.insert(k.label().to_string());
        }
    }
    keys
}

fn forms(lang: Lang) -> usize {
    match lang {
        Lang::En => 2,
        Lang::Ru => 3,
    }
}

#[test]
fn every_key_the_ui_uses_is_in_both_locale_files() {
    let code = code_keys();
    assert!(
        code.len() > 50,
        "only {} keys found: the scan is broken",
        code.len()
    );
    let mut problems = Vec::new();
    for lang in Lang::ALL {
        let l = locale(lang);
        for (key, plural) in &code {
            match (l.texts.get(key), plural) {
                (None, _) => problems.push(format!("{}: missing {key:?}", lang.code())),
                (Some(Text::Plural(f)), true) if f.len() == forms(lang) => {}
                (Some(_), true) => problems.push(format!(
                    "{}: {key:?} needs {} plural forms",
                    lang.code(),
                    forms(lang)
                )),
                (Some(Text::Plural(_)), false) => problems.push(format!(
                    "{}: {key:?} is plural but used as tr()",
                    lang.code()
                )),
                (Some(Text::Plain(_)), false) => {}
            }
        }
        for key in content_keys() {
            if !l.texts.contains_key(&key) {
                problems.push(format!("{}: missing name or label {key:?}", lang.code()));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[test]
fn a_translation_keeps_the_placeholders_of_its_key() {
    let mut bad = Vec::new();
    let names = |s: &str| -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut rest = s;
        while let Some(o) = rest.find('{') {
            let after = &rest[o + 1..];
            let Some(c) = after.find('}') else { break };
            out.insert(after[..c].to_string());
            rest = &after[c + 1..];
        }
        out
    };
    for lang in Lang::ALL {
        for (key, text) in &locale(lang).texts {
            let want = names(key);
            let texts: Vec<&String> = match text {
                Text::Plain(t) => vec![t],
                Text::Plural(f) => f.iter().collect(),
            };
            for t in texts {
                // A singular form may say "one" in words instead of `{n}`.
                let mut got = names(t);
                let mut want = want.clone();
                if matches!(text, Text::Plural(_)) {
                    got.remove("n");
                    want.remove("n");
                }
                if got != want {
                    bad.push(format!("{}: {key:?} -> {t:?}", lang.code()));
                }
            }
        }
    }
    assert!(bad.is_empty(), "placeholders differ:\n{}", bad.join("\n"));
}
