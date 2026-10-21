//! The words of the UI in the player's language (T-284). Every player-facing string goes through
//! [`tr`], [`trn`] or [`trf!`]; the texts live in `assets/locale/<lang>.ron`.
//!
//! **Keys are the English texts.** `tr("Pick a tool.")` looks the key up in the current
//! language's file; `en.ron` lists every key too (its value is what English shows, so wording is
//! changed there, not in code), and a test checks both files hold every key the code uses. A key
//! missing from a file shows the key itself.
//!
//! * Placeholders are named, `{n}`, `{name}`: [`trf!`] fills them after the lookup, so a
//!   translation may put them in another order. Numbers are formatted by the caller.
//! * Plurals: [`trn`] picks the form for `n` by the language's rule (English one / other,
//!   Russian one / few / many: 1 житель, 3 жителя, 5 жителей) and fills `{n}`. The key is the
//!   English plural (`"{n} residents"`).
//! * Content names (buildings, roads, policies, zones, milestones) are keys as well: `tr(&def.name)`.
//!
//! The language is process-wide ([`set_lang`]); the UI is redrawn every frame, so a switch shows
//! at once.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::{Arc, LazyLock, RwLock};

use serde::Deserialize;

/// A language of the UI.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Lang {
    #[default]
    En,
    Ru,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Ru];

    /// The code used in settings and on the command line (`--lang ru`).
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Ru => "ru",
        }
    }

    pub fn from_code(code: &str) -> Option<Lang> {
        Lang::ALL.into_iter().find(|l| l.code() == code)
    }

    /// The language's own name, for the language menu.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Ru => "Русский",
        }
    }

    /// The text of the language's file.
    pub fn source(self) -> &'static str {
        match self {
            Lang::En => include_str!("../../../assets/locale/en.ron"),
            Lang::Ru => include_str!("../../../assets/locale/ru.ron"),
        }
    }
}

/// One entry of a locale file: a text, or the plural forms of one.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Text {
    Plain(String),
    /// The forms in the language's order: English `[one, other]`, Russian `[one, few, many]`.
    Plural(Vec<String>),
}

/// A locale file: `{ "key": "text", "key": ["form", ...] }`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(transparent)]
pub struct Locale {
    pub texts: HashMap<String, Text>,
}

impl Locale {
    /// Parses a locale file.
    pub fn parse(src: &str) -> Result<Locale, String> {
        ron::from_str(src).map_err(|e| e.to_string())
    }

    /// The text of `key`, or the key itself. A plural entry gives its last form.
    pub fn get<'a>(&'a self, key: &'a str) -> &'a str {
        match self.texts.get(key) {
            Some(Text::Plain(t)) => t,
            Some(Text::Plural(f)) => f.last().map_or(key, String::as_str),
            None => key,
        }
    }

    /// The form of `key` for `n` in `lang`, `{n}` not yet filled; the key without an entry.
    pub fn plural<'a>(&'a self, lang: Lang, key: &'a str, n: i64) -> &'a str {
        match self.texts.get(key) {
            Some(Text::Plural(f)) => f
                .get(plural_index(lang, n))
                .or(f.last())
                .map_or(key, String::as_str),
            Some(Text::Plain(t)) => t,
            None => key,
        }
    }
}

/// The index of the plural form of `n` in `lang`'s order.
pub fn plural_index(lang: Lang, n: i64) -> usize {
    let n = n.unsigned_abs();
    match lang {
        Lang::En => usize::from(n != 1),
        Lang::Ru => {
            let (d, h) = (n % 10, n % 100);
            if d == 1 && h != 11 {
                0
            } else if (2..=4).contains(&d) && !(12..=14).contains(&h) {
                1
            } else {
                2
            }
        }
    }
}

struct Current {
    lang: Lang,
    locale: Arc<Locale>,
}

static LOCALES: LazyLock<HashMap<Lang, Arc<Locale>>> = LazyLock::new(|| {
    Lang::ALL
        .into_iter()
        .map(|l| {
            let locale = Locale::parse(l.source())
                .unwrap_or_else(|e| panic!("assets/locale/{}.ron: {e}", l.code()));
            (l, Arc::new(locale))
        })
        .collect()
});

static CURRENT: LazyLock<RwLock<Current>> = LazyLock::new(|| {
    RwLock::new(Current {
        lang: Lang::En,
        locale: LOCALES[&Lang::En].clone(),
    })
});

/// The language of the UI from now on.
pub fn set_lang(lang: Lang) {
    let mut c = CURRENT.write().unwrap_or_else(|e| e.into_inner());
    c.lang = lang;
    c.locale = LOCALES[&lang].clone();
}

/// The language of the UI.
pub fn lang() -> Lang {
    CURRENT.read().unwrap_or_else(|e| e.into_inner()).lang
}

/// The parsed file of `lang`.
pub fn locale(lang: Lang) -> Arc<Locale> {
    LOCALES[&lang].clone()
}

fn current() -> (Lang, Arc<Locale>) {
    let c = CURRENT.read().unwrap_or_else(|e| e.into_inner());
    (c.lang, c.locale.clone())
}

/// `key` in the current language (the key itself if the file has no entry).
pub fn tr(key: &str) -> String {
    current().1.get(key).to_string()
}

/// The plural form of `key` for `n` in the current language, with `{n}` filled.
pub fn trn(key: &str, n: i64) -> String {
    let (lang, l) = current();
    l.plural(lang, key, n).replace("{n}", &n.to_string())
}

/// `template` with each `{name}` replaced by its value; unknown placeholders stay as written.
pub fn fill(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                let name = &after[..close];
                match args.iter().find(|(n, _)| *n == name) {
                    Some((_, v)) => out.push_str(&v.to_string()),
                    None => {
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            None => {
                out.push_str(&rest[open..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// `trf!("{n} cells under the cursor", n = count)`: [`tr`] of the key, its placeholders filled
/// by name. With `n = ...` first and `plural`, the form is picked by `n` as [`trn`] does:
/// `trf!(plural "{n} residents in {name}", n = 5, name = town)`.
#[macro_export]
macro_rules! trf {
    (plural $key:literal, n = $n:expr $(, $name:ident = $val:expr)* $(,)?) => {{
        let n: i64 = ($n) as i64;
        $crate::fill(
            &$crate::trn_raw($key, n),
            &[("n", &n as &dyn ::std::fmt::Display) $(, (stringify!($name), &$val as &dyn ::std::fmt::Display))*],
        )
    }};
    ($key:literal $(, $name:ident = $val:expr)* $(,)?) => {
        $crate::fill(
            &$crate::tr($key),
            &[$((stringify!($name), &$val as &dyn ::std::fmt::Display)),*],
        )
    };
}

/// The plural form of `key` for `n` in the current language, placeholders not filled (for
/// [`trf!`]).
#[doc(hidden)]
pub fn trn_raw(key: &str, n: i64) -> String {
    let (lang, l) = current();
    l.plural(lang, key, n).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn russian_plurals_take_one_few_and_many() {
        let forms = |n| plural_index(Lang::Ru, n);
        for n in [1, 21, 101, 1001] {
            assert_eq!(forms(n), 0, "{n}");
        }
        for n in [2, 3, 4, 22, 34, 102] {
            assert_eq!(forms(n), 1, "{n}");
        }
        for n in [0, 5, 11, 12, 14, 19, 20, 25, 111, 112, 1000] {
            assert_eq!(forms(n), 2, "{n}");
        }
        assert_eq!(plural_index(Lang::En, 1), 0);
        assert_eq!(plural_index(Lang::En, 0), 1);
        assert_eq!(plural_index(Lang::En, 2), 1);
        let l = Locale::parse(r#"{"{n} residents": ["{n} житель", "{n} жителя", "{n} жителей"]}"#)
            .unwrap();
        let say = |n: i64| {
            l.plural(Lang::Ru, "{n} residents", n)
                .replace("{n}", &n.to_string())
        };
        assert_eq!(say(1), "1 житель");
        assert_eq!(say(3), "3 жителя");
        assert_eq!(say(5), "5 жителей");
        assert_eq!(say(21), "21 житель");
        assert_eq!(say(11), "11 жителей");
    }

    #[test]
    fn placeholders_are_filled_by_name_in_any_order_and_unknown_ones_stay() {
        assert_eq!(
            fill("{b} then {a}, {c}", &[("a", &1), ("b", &"two")]),
            "two then 1, {c}"
        );
        assert_eq!(fill("no braces", &[]), "no braces");
        assert_eq!(fill("open { brace", &[]), "open { brace");
    }

    #[test]
    fn both_files_parse_and_a_missing_key_shows_itself() {
        for l in Lang::ALL {
            Locale::parse(l.source()).unwrap_or_else(|e| panic!("{}: {e}", l.code()));
        }
        let l = Locale::default();
        assert_eq!(l.get("Not in the file"), "Not in the file");
    }
}
