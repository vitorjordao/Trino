//! String tables as RON (issue #59, ADR 0026): `assets/i18n/<lang>.ron`,
//! one file per language, the authored form of
//! [`trino_core::i18n::StringTable`].
//!
//! ```ron
//! Strings(
//!     lang: "pt-BR",
//!     fallback: "en-US",
//!     name: "Português (Brasil)",
//!     entries: {
//!         "menu.play": "Jogar",
//!         "guests.count": Plural(one: "{n} convidado", other: "{n} convidados"),
//!         "money": Format("{amount:currency}"),
//!     },
//! )
//! ```
//!
//! - An entry is a string, a `Plural(zero:, one:, two:, few:, many:,
//!   other:)` (the forms the language's rule uses; `other` always) or a
//!   `Format("…")` (a string that is only placeholders, marked so a
//!   translator leaves it alone). Any string may hold `{name}` /
//!   `{name:kind}` placeholders ([`trino_core::i18n::format`]).
//! - `plural:` overrides the tag's CLDR rule (`OneOther`, `OneUpToOne`,
//!   `Other`, `Slavic`, `Polish`, `Czech`); `locale: (decimal: ",", …)`
//!   overrides any of the tag's number, currency and date defaults.
//! - A tag starting `qps-` is a pseudo-locale (`cargo xtask i18n pseudo`):
//!   a development language, never baked for the consoles and never in a
//!   font's glyph subset.
//!
//! [`check`] finds missing and extra keys, mismatched placeholders and
//! missing plural forms; [`pseudo`] makes the pseudo-locale; [`to_csv`] /
//! [`from_csv`] are the translators' round trip; [`glyphs`] is what a
//! language draws (the fonts' subsets).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};
use trino_core::i18n::table::{EntrySpec, TableSpec, encode_tstr_with};
use trino_core::i18n::{DateOrder, Locale, PluralForm, PluralRule, format};
use trino_core::ui::UiStrings;

/// Where the tables live under a project.
pub const I18N_DIR: &str = "assets/i18n";

/// Whether `tag` is a pseudo-locale (`qps-ploc`): development only.
pub fn is_pseudo(tag: &str) -> bool {
    tag.to_ascii_lowercase().starts_with("qps")
}

/// The blob a language bakes into.
pub fn strings_blob_path(lang: &str) -> String {
    format!("i18n/{lang}")
}

/// The blob that lists the baked languages ([`trino_core::i18n::LANGUAGES_BLOB`]).
pub const LANGUAGES_BLOB_PATH: &str = "i18n/languages";

/// A plural entry's forms (empty: not given).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct PluralDef {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub zero: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub one: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub two: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub few: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub many: String,
    pub other: String,
}

impl PluralDef {
    pub fn get(&self, form: PluralForm) -> &str {
        match form {
            PluralForm::Zero => &self.zero,
            PluralForm::One => &self.one,
            PluralForm::Two => &self.two,
            PluralForm::Few => &self.few,
            PluralForm::Many => &self.many,
            PluralForm::Other => &self.other,
        }
    }

    pub fn get_mut(&mut self, form: PluralForm) -> &mut String {
        match form {
            PluralForm::Zero => &mut self.zero,
            PluralForm::One => &mut self.one,
            PluralForm::Two => &mut self.two,
            PluralForm::Few => &mut self.few,
            PluralForm::Many => &mut self.many,
            PluralForm::Other => &mut self.other,
        }
    }

    fn array(&self) -> [&str; 6] {
        PluralForm::ALL.map(|f| self.get(f))
    }
}

/// One entry of a table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryDef {
    Text(String),
    Plural(PluralDef),
    /// Only placeholders (`{amount:currency}`): translators keep it.
    Format(String),
}

impl EntryDef {
    /// The text (a plural's `other` form).
    pub fn text(&self) -> &str {
        match self {
            EntryDef::Text(t) | EntryDef::Format(t) => t,
            EntryDef::Plural(p) => &p.other,
        }
    }

    /// Every text it holds with its form (`None` for a plain entry).
    pub fn texts(&self) -> Vec<(Option<PluralForm>, &str)> {
        match self {
            EntryDef::Text(t) | EntryDef::Format(t) => vec![(None, t.as_str())],
            EntryDef::Plural(p) => PluralForm::ALL
                .into_iter()
                .filter(|f| !p.get(*f).is_empty())
                .map(|f| (Some(f), p.get(f)))
                .collect(),
        }
    }

    pub fn is_plural(&self) -> bool {
        matches!(self, EntryDef::Plural(_))
    }

    fn spec(&self) -> EntrySpec<'_> {
        match self {
            EntryDef::Text(t) | EntryDef::Format(t) => EntrySpec::Text(t),
            EntryDef::Plural(p) => EntrySpec::Plural(p.array()),
        }
    }
}

impl From<&str> for EntryDef {
    fn from(s: &str) -> Self {
        EntryDef::Text(s.to_string())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum RawEntry {
    Text(String),
    Tagged(Tagged),
}

#[derive(Serialize, Deserialize)]
enum Tagged {
    Plural(PluralDef),
    Format(String),
}

impl Serialize for EntryDef {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            EntryDef::Text(t) => RawEntry::Text(t.clone()),
            EntryDef::Plural(p) => RawEntry::Tagged(Tagged::Plural(p.clone())),
            EntryDef::Format(t) => RawEntry::Tagged(Tagged::Format(t.clone())),
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for EntryDef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match RawEntry::deserialize(d)? {
            RawEntry::Text(t) => EntryDef::Text(t),
            RawEntry::Tagged(Tagged::Plural(p)) => EntryDef::Plural(p),
            RawEntry::Tagged(Tagged::Format(t)) => EntryDef::Format(t),
        })
    }
}

/// Overrides of the tag's default number, currency and date formats.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct LocaleDef {
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "bare")]
    pub decimal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "bare")]
    pub thousands: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "bare")]
    pub currency: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "bare")]
    pub currency_after: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "bare")]
    pub currency_space: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "bare")]
    pub currency_decimals: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "bare")]
    pub min_group: Option<u8>,
    /// `Dmy`, `Mdy` or `Ymd`.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "bare")]
    pub date_order: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "bare")]
    pub date_sep: Option<String>,
}

/// An optional field written as a bare value (`currency: "R$"`, not
/// `Some("R$")`).
fn bare<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

impl LocaleDef {
    fn is_empty(&self) -> bool {
        *self == LocaleDef::default()
    }
}

/// A language's strings: `assets/i18n/<lang>.ron`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename = "Strings", deny_unknown_fields)]
pub struct StringsDef {
    pub lang: String,
    /// The language read for keys this one lacks.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fallback: String,
    /// The language's own name, for a language menu (`Português`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// The plural rule's name, when the tag's default isn't right.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub plural: String,
    #[serde(default, skip_serializing_if = "LocaleDef::is_empty")]
    pub locale: LocaleDef,
    pub entries: BTreeMap<String, EntryDef>,
}

impl StringsDef {
    pub fn new(lang: &str) -> StringsDef {
        StringsDef {
            lang: lang.into(),
            ..StringsDef::default()
        }
    }

    pub fn from_ron(text: &str) -> Result<StringsDef, String> {
        ron::from_str(text).map_err(|e| format!("string table parse error: {e}"))
    }

    pub fn load(path: &Path) -> Result<StringsDef, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        StringsDef::from_ron(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The file as the editor and the CLI write it: one line per entry,
    /// sorted by key (comments in a hand-written file are not kept).
    pub fn to_ron(&self) -> String {
        let q = |s: &str| ron::to_string(s).unwrap_or_else(|_| format!("{s:?}"));
        let mut out = String::from("Strings(\n");
        let _ = writeln!(out, "    lang: {},", q(&self.lang));
        for (field, v) in [("fallback", &self.fallback), ("name", &self.name)] {
            if !v.is_empty() {
                let _ = writeln!(out, "    {field}: {},", q(v));
            }
        }
        if !self.plural.is_empty() {
            let _ = writeln!(out, "    plural: {},", q(&self.plural));
        }
        if !self.locale.is_empty() {
            let l = &self.locale;
            let mut parts = Vec::new();
            for (k, v) in [
                ("decimal", &l.decimal),
                ("thousands", &l.thousands),
                ("currency", &l.currency),
                ("date_sep", &l.date_sep),
            ] {
                if let Some(v) = v {
                    parts.push(format!("{k}: {}", q(v)));
                }
            }
            for (k, v) in [
                ("currency_after", l.currency_after),
                ("currency_space", l.currency_space),
            ] {
                if let Some(v) = v {
                    parts.push(format!("{k}: {v}"));
                }
            }
            for (k, v) in [
                ("currency_decimals", l.currency_decimals),
                ("min_group", l.min_group),
            ] {
                if let Some(v) = v {
                    parts.push(format!("{k}: {v}"));
                }
            }
            if let Some(o) = &l.date_order {
                parts.push(format!("date_order: {}", q(o)));
            }
            let _ = writeln!(out, "    locale: ({}),", parts.join(", "));
        }
        out.push_str("    entries: {\n");
        for (k, e) in &self.entries {
            let v = match e {
                EntryDef::Text(t) => q(t),
                EntryDef::Format(t) => format!("Format({})", q(t)),
                EntryDef::Plural(p) => {
                    let forms: Vec<String> = PluralForm::ALL
                        .into_iter()
                        .filter(|f| !p.get(*f).is_empty() || *f == PluralForm::Other)
                        .map(|f| format!("{}: {}", f.key(), q(p.get(f))))
                        .collect();
                    format!("Plural({})", forms.join(", "))
                }
            };
            let _ = writeln!(out, "        {}: {v},", q(k));
        }
        out.push_str("    },\n)\n");
        out
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        std::fs::write(path, self.to_ron()).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The plural rule: `plural`, else the tag's.
    pub fn plural_rule(&self) -> Result<PluralRule, String> {
        if self.plural.is_empty() {
            Ok(PluralRule::for_tag(&self.lang))
        } else {
            PluralRule::from_name(&self.plural).ok_or_else(|| {
                format!(
                    "{}: unknown plural rule `{}` (one of {})",
                    self.lang,
                    self.plural,
                    PluralRule::ALL.map(|r| r.name()).join(", ")
                )
            })
        }
    }

    /// The locale: the tag's defaults under `locale`'s overrides.
    pub fn locale(&self) -> Result<Locale<'_>, String> {
        let d = Locale::for_tag(&self.lang);
        let l = &self.locale;
        let date_order = match &l.date_order {
            None => d.date_order,
            Some(o) => DateOrder::from_name(o)
                .ok_or_else(|| format!("{}: date_order `{o}` is not Dmy, Mdy or Ymd", self.lang))?,
        };
        Ok(Locale {
            decimal: l.decimal.as_deref().unwrap_or(d.decimal),
            thousands: l.thousands.as_deref().unwrap_or(d.thousands),
            currency: l.currency.as_deref().unwrap_or(d.currency),
            currency_after: l.currency_after.unwrap_or(d.currency_after),
            currency_space: l.currency_space.unwrap_or(d.currency_space),
            currency_decimals: l.currency_decimals.unwrap_or(d.currency_decimals).min(18),
            min_group: l.min_group.unwrap_or(d.min_group),
            date_order,
            date_sep: l.date_sep.as_deref().unwrap_or(d.date_sep),
            plural: self.plural_rule()?,
        })
    }

    /// The `TSTR` blob (`i18n/<lang>`) with no font remaps.
    pub fn to_tstr(&self) -> Result<Vec<u8>, String> {
        self.to_tstr_with(&[], 0)
    }

    /// The `TSTR` blob with the language's font remaps (`(font, the font
    /// this language draws it with)` hashes) and its fonts' asset group.
    pub fn to_tstr_with(&self, fonts: &[(u32, u32)], group: u32) -> Result<Vec<u8>, String> {
        let spec = TableSpec {
            lang: &self.lang,
            fallback: &self.fallback,
            name: &self.name,
            locale: self.locale()?,
            fonts,
            group,
        };
        let mut entries: Vec<(&str, EntrySpec<'_>)> = self
            .entries
            .iter()
            .map(|(k, e)| (k.as_str(), e.spec()))
            .collect();
        let mut out = Vec::new();
        encode_tstr_with(&spec, &mut entries, &mut |b| out.extend_from_slice(b)).map_err(
            |(hash, at)| {
                format!(
                    "{}: key \"{}\" collides with another (hash {hash:#010x})",
                    self.lang, entries[at].0
                )
            },
        )?;
        Ok(out)
    }
}

/// String tables by language (host side; the runtime reads `TSTR`).
#[derive(Clone, Debug, Default)]
pub struct Langs {
    pub tables: BTreeMap<String, StringsDef>,
    /// The game's default language (the manifest's `[i18n] default`).
    /// `None`: [`Langs::default_lang`] guesses.
    pub default: Option<String>,
    /// Per language, the fonts it draws with another (`(font, replacement)`
    /// hashes: per-language atlases, overrides), as its `TSTR` carries them
    /// for one platform (`trino_asset_pipeline::i18n::with_remaps`).
    pub fonts: BTreeMap<String, Vec<(u32, u32)>>,
}

impl Langs {
    /// Every `<dir>/*.ron` file.
    pub fn load_dir(dir: &Path) -> Result<Langs, String> {
        let mut langs = Langs::default();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Ok(langs);
        };
        let mut paths: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "ron"))
            .collect();
        paths.sort();
        for p in paths {
            let table = StringsDef::load(&p)?;
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if stem != table.lang {
                return Err(format!(
                    "{}: the file of `{}` must be named {}.ron",
                    p.display(),
                    table.lang,
                    table.lang
                ));
            }
            langs.tables.insert(table.lang.clone(), table);
        }
        Ok(langs)
    }

    pub fn names(&self) -> Vec<String> {
        self.tables.keys().cloned().collect()
    }

    /// The default language: the manifest's, else the one the others fall
    /// back to most (a table with no fallback first), else the first.
    pub fn default_lang(&self) -> Option<&str> {
        if let Some(d) = &self.default
            && self.tables.contains_key(d)
        {
            return Some(d);
        }
        let mut best: Option<(&str, (usize, bool))> = None;
        for t in self.tables.values().filter(|t| !is_pseudo(&t.lang)) {
            let n = self
                .tables
                .values()
                .filter(|o| o.fallback == t.lang)
                .count();
            let score = (n, t.fallback.is_empty());
            if best.is_none_or(|(_, s)| score > s) {
                best = Some((&t.lang, score));
            }
        }
        best.map(|(l, _)| l)
    }

    /// The default language first, then the others (pseudo-locales last),
    /// as `i18n/languages` lists them.
    pub fn ordered(&self) -> Vec<&StringsDef> {
        let d = self.default_lang();
        let mut v: Vec<&StringsDef> = self.tables.values().collect();
        v.sort_by_key(|t| {
            (
                Some(t.lang.as_str()) != d,
                is_pseudo(&t.lang),
                t.lang.clone(),
            )
        });
        v
    }

    /// `lang`'s strings with its fallback chain (`None` = no table: the
    /// nodes' default texts).
    pub fn strings(&self, lang: &str) -> HostStrings<'_> {
        HostStrings {
            chain: self.chain(lang),
            revision: trino_core::asset_id(lang),
            fonts: self.fonts.get(lang).cloned().unwrap_or_default(),
        }
    }

    /// `lang`'s table and its fallbacks, in lookup order.
    pub fn chain(&self, lang: &str) -> Vec<&StringsDef> {
        let mut chain = Vec::new();
        let mut at = self.tables.get(lang);
        while let Some(t) = at {
            if chain.iter().any(|c: &&StringsDef| c.lang == t.lang) {
                break;
            }
            chain.push(t);
            at = self.tables.get(&t.fallback);
        }
        chain
    }

    /// The entry for `key` in `lang`'s chain.
    pub fn lookup(&self, lang: &str, key: &str) -> Option<&EntryDef> {
        self.chain(lang)
            .into_iter()
            .find_map(|t| t.entries.get(key))
    }

    /// Every key of every table.
    pub fn keys(&self) -> BTreeSet<&str> {
        self.tables
            .values()
            .flat_map(|t| t.entries.keys().map(String::as_str))
            .collect()
    }

    /// The text `lang` shows for `key` in `{{n}}`'s `form` (plural entries
    /// pick it; others are their text), for previews.
    pub fn text_for(&self, lang: &str, key: &str, form: PluralForm) -> Option<&str> {
        self.lookup(lang, key).map(|e| match e {
            EntryDef::Plural(p) if !p.get(form).is_empty() => p.get(form),
            e => e.text(),
        })
    }
}

/// [`UiStrings`] over loaded tables.
pub struct HostStrings<'a> {
    chain: Vec<&'a StringsDef>,
    revision: u32,
    fonts: Vec<(u32, u32)>,
}

impl UiStrings for HostStrings<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.chain
            .iter()
            .find_map(|t| t.entries.get(key).map(EntryDef::text))
    }

    fn revision(&self) -> u32 {
        self.revision
    }

    fn font(&self, font: trino_core::FontId) -> trino_core::FontId {
        match self.fonts.iter().find(|(f, _)| *f == font.0) {
            Some((_, to)) => trino_core::FontId(*to),
            None => font,
        }
    }
}

impl HostStrings<'_> {
    /// The language's own table has the key (no fallback).
    pub fn has_own(&self, key: &str) -> bool {
        self.chain
            .first()
            .is_some_and(|t| t.entries.contains_key(key))
    }
}

// ------------------------------------------------------------- checks ----

/// How bad an [`I18nIssue`] is.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// What an [`I18nIssue`] is about (the `--json` `kind`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueKind {
    MissingKey,
    ExtraKey,
    UnusedKey,
    Placeholder,
    PluralForm,
    BadTable,
    MissingGlyph,
    Overflow,
    FontBudget,
}

/// One finding of [`check`] (and of `cargo xtask i18n check`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct I18nIssue {
    pub severity: Severity,
    pub kind: IssueKind,
    pub lang: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub key: String,
    pub message: String,
}

impl std::fmt::Display for I18nIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let level = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        };
        write!(f, "{level}: [{}]", self.lang)?;
        if !self.key.is_empty() {
            write!(f, " {}", self.key)?;
        }
        write!(f, ": {}", self.message)
    }
}

fn issue(sev: Severity, kind: IssueKind, lang: &str, key: &str, message: String) -> I18nIssue {
    I18nIssue {
        severity: sev,
        kind,
        lang: lang.into(),
        key: key.into(),
        message,
    }
}

/// The placeholder names in `text`, sorted.
pub fn placeholder_names(text: &str) -> BTreeSet<String> {
    let mut s = BTreeSet::new();
    format::placeholders(text, &mut |name, _| {
        s.insert(name.to_string());
    });
    s
}

/// The table-only checks: bad tables (plural rule, locale, a fallback
/// that isn't loaded), keys missing from or extra to each language against
/// the default, placeholders that differ from the default's, plural forms
/// the language's rule needs but the entry lacks.
pub fn check(langs: &Langs) -> Vec<I18nIssue> {
    use IssueKind::*;
    use Severity::*;
    let mut out = Vec::new();
    let Some(default) = langs.default_lang().and_then(|d| langs.tables.get(d)) else {
        return out;
    };
    for t in langs.tables.values() {
        let lang = t.lang.as_str();
        if let Err(e) = t.locale() {
            out.push(issue(Error, BadTable, lang, "", e));
        }
        if !t.fallback.is_empty() && !langs.tables.contains_key(&t.fallback) {
            out.push(issue(
                Error,
                BadTable,
                lang,
                "",
                format!("falls back to `{}`, which has no table", t.fallback),
            ));
        }
        let rule = t.plural_rule().unwrap_or_default();
        for (key, e) in &t.entries {
            if let EntryDef::Plural(p) = e {
                if p.other.is_empty() {
                    out.push(issue(
                        Error,
                        PluralForm,
                        lang,
                        key,
                        "plural without `other`".into(),
                    ));
                }
                let missing: Vec<&str> = rule
                    .forms()
                    .iter()
                    .filter(|f| p.get(**f).is_empty())
                    .map(|f| f.key())
                    .collect();
                if !missing.is_empty() {
                    out.push(issue(
                        Warning,
                        PluralForm,
                        lang,
                        key,
                        format!(
                            "{} uses {}: `{}` missing (falls back to `other`)",
                            rule.name(),
                            rule.forms()
                                .iter()
                                .map(|f| f.key())
                                .collect::<Vec<_>>()
                                .join("/"),
                            missing.join("`, `")
                        ),
                    ));
                }
            }
            for (form, text) in e.texts() {
                if let Err(m) = braces_balanced(text) {
                    let at = form.map_or(String::new(), |f| format!(" ({})", f.key()));
                    out.push(issue(Error, Placeholder, lang, key, format!("{m}{at}")));
                }
            }
        }
        if t.lang == default.lang || is_pseudo(lang) {
            continue;
        }
        for (key, d) in &default.entries {
            match t.entries.get(key) {
                None => {
                    let via = langs
                        .chain(lang)
                        .into_iter()
                        .skip(1)
                        .find(|c| c.entries.contains_key(key));
                    match via {
                        Some(c) => out.push(issue(
                            Warning,
                            MissingKey,
                            lang,
                            key,
                            format!("missing: shows {}'s text", c.lang),
                        )),
                        None => out.push(issue(
                            Error,
                            MissingKey,
                            lang,
                            key,
                            "missing, and no fallback has it: shows nothing".into(),
                        )),
                    }
                }
                Some(e) => {
                    let want = placeholder_names(d.text());
                    for (form, text) in e.texts() {
                        let got = placeholder_names(text);
                        // A plural's one/zero form may leave the count out
                        // ("um convidado").
                        let extra: Vec<&String> = got.difference(&want).collect();
                        let lost: Vec<&String> = want
                            .difference(&got)
                            .filter(|n| {
                                !(e.is_plural()
                                    && form != Some(trino_core::i18n::PluralForm::Other)
                                    && (n.as_str() == "n" || n.as_str() == "count"))
                            })
                            .collect();
                        if !extra.is_empty() || !lost.is_empty() {
                            let at = form.map_or(String::new(), |f| format!(" ({})", f.key()));
                            let mut m = String::from("placeholders differ from the default's");
                            if !lost.is_empty() {
                                let _ = write!(m, "; missing {}", braces(&lost));
                            }
                            if !extra.is_empty() {
                                let _ = write!(m, "; unknown {}", braces(&extra));
                            }
                            out.push(issue(Error, Placeholder, lang, key, format!("{m}{at}")));
                        }
                    }
                    if d.is_plural() && !e.is_plural() && rule != PluralRule::Other {
                        out.push(issue(
                            Warning,
                            PluralForm,
                            lang,
                            key,
                            "a plural in the default language, plain text here".into(),
                        ));
                    }
                }
            }
        }
        for key in t.entries.keys() {
            if !default.entries.contains_key(key) {
                out.push(issue(
                    Warning,
                    ExtraKey,
                    lang,
                    key,
                    format!("not in the default language ({})", default.lang),
                ));
            }
        }
    }
    out
}

fn braces(names: &[&String]) -> String {
    names
        .iter()
        .map(|n| format!("{{{n}}}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn braces_balanced(text: &str) -> Result<(), String> {
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'{' | b'}' if b.get(i + 1) == Some(&b[i]) => i += 2,
            b'{' => match text[i + 1..].find(['}', '{']) {
                Some(j) if b[i + 1 + j] == b'}' => {
                    if j == 0 {
                        return Err("an empty placeholder `{}`".into());
                    }
                    i += j + 2;
                }
                _ => return Err("a `{` without its `}` (write `{{` for a brace)".into()),
            },
            b'}' => return Err("a `}` without its `{` (write `}}` for a brace)".into()),
            _ => i += 1,
        }
    }
    Ok(())
}

/// Keys no screen and no source file uses: `used` holds every key found
/// (UI `key`s, string literals in the game's code).
pub fn unused(langs: &Langs, used: &BTreeSet<String>) -> Vec<I18nIssue> {
    let Some(d) = langs.default_lang().and_then(|d| langs.tables.get(d)) else {
        return Vec::new();
    };
    d.entries
        .keys()
        .filter(|k| !used.contains(*k))
        .map(|k| {
            issue(
                Severity::Info,
                IssueKind::UnusedKey,
                &d.lang,
                k,
                "no UI screen or source file names it".into(),
            )
        })
        .collect()
}

// -------------------------------------------------------------- glyphs ----

/// Every character `table` draws: its texts without their placeholders,
/// its name, and what its placeholders print by kind in its locale:
/// numbers (untyped, `int`, `num`, `raw`) digits, the minus sign and the
/// separators; `currency` those and the symbol; `date` digits and the date
/// separator; `str` nothing. Money goes in `{x:currency}` (an untyped
/// placeholder is counted as a number).
pub fn glyphs(table: &StringsDef) -> BTreeSet<char> {
    let mut set = BTreeSet::new();
    let (mut numbers, mut money, mut dates) = (false, false, false);
    for e in table.entries.values() {
        for (_, text) in e.texts() {
            strip_placeholders(text, &mut |c| {
                set.insert(c);
            });
            format::placeholders(text, &mut |_, kind| match kind {
                "str" => {}
                "currency" => money = true,
                "date" => dates = true,
                _ => numbers = true,
            });
        }
    }
    set.extend(table.name.chars());
    if let Ok(l) = table.locale() {
        if numbers || money || dates {
            set.extend("0123456789".chars());
        }
        if numbers || money {
            set.insert('-');
            set.extend(l.decimal.chars().chain(l.thousands.chars()));
        }
        if money {
            set.extend(l.currency.chars());
        }
        if dates {
            set.extend(l.date_sep.chars());
        }
    }
    set.remove(&'\n');
    set
}

/// What `lang` shows, as one table: its own entries plus every key its
/// fallback chain supplies (drawn with `lang`'s fonts too), its name and
/// locale. `None` when `lang` has no table.
pub fn effective(langs: &Langs, lang: &str) -> Option<StringsDef> {
    let chain = langs.chain(lang);
    let own = chain.first()?;
    let mut t = (*own).clone();
    for other in chain.iter().skip(1) {
        for (k, e) in &other.entries {
            t.entries.entry(k.clone()).or_insert_with(|| e.clone());
        }
    }
    Some(t)
}

/// [`glyphs`] of what `lang` shows ([`effective`]): a language's font must
/// also draw the fallback's texts it borrows.
pub fn shown_glyphs(langs: &Langs, lang: &str) -> BTreeSet<char> {
    effective(langs, lang)
        .map(|t| glyphs(&t))
        .unwrap_or_default()
}

/// Calls `f` with every character of `text` outside its placeholders
/// (`{{` and `}}` as one brace).
pub fn strip_placeholders(text: &str, f: &mut dyn FnMut(char)) {
    let mut chars = text.char_indices().peekable();
    while let Some((_, c)) = chars.next() {
        match c {
            '{' | '}' if chars.peek().map(|p| p.1) == Some(c) => {
                chars.next();
                f(c);
            }
            '{' => {
                for (_, d) in chars.by_ref() {
                    if d == '}' {
                        break;
                    }
                }
            }
            _ => f(c),
        }
    }
}

// -------------------------------------------------------------- pseudo ----

/// The pseudo-locale's default tag.
pub const PSEUDO_TAG: &str = "qps-ploc";

/// A pseudo-localized copy of `base`: every letter accented (Latin-1, which
/// most fonts with pt/es/fr glyphs have), each text in `[ ]` and `expand`
/// percent longer (`~` padding), placeholders kept. Hard-coded strings stay
/// plain on screen, and tight boxes overflow.
pub fn pseudo(base: &StringsDef, tag: &str, expand: u32) -> StringsDef {
    let tr = |s: &str| -> String {
        if s.is_empty() {
            return String::new();
        }
        let mut out = String::from("[");
        let mut letters = 0usize;
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '{' if chars.peek() == Some(&'{') => {
                    out.push_str("{{");
                    chars.next();
                }
                '{' => {
                    out.push('{');
                    for d in chars.by_ref() {
                        out.push(d);
                        if d == '}' {
                            break;
                        }
                    }
                }
                c => {
                    letters += 1;
                    out.push(accent(c));
                }
            }
        }
        let pad = (letters * expand as usize).div_ceil(100);
        if pad > 0 {
            out.push(' ');
            out.extend(std::iter::repeat_n('~', pad.saturating_sub(1).max(1)));
        }
        out.push(']');
        out
    };
    let entries = base
        .entries
        .iter()
        .map(|(k, e)| {
            let e = match e {
                EntryDef::Text(t) => EntryDef::Text(tr(t)),
                EntryDef::Format(t) => EntryDef::Format(t.clone()),
                EntryDef::Plural(p) => {
                    let mut q = PluralDef::default();
                    for f in PluralForm::ALL {
                        *q.get_mut(f) = tr(p.get(f));
                    }
                    EntryDef::Plural(q)
                }
            };
            (k.clone(), e)
        })
        .collect();
    StringsDef {
        lang: tag.into(),
        fallback: base.lang.clone(),
        name: format!("[Pseudo {}]", base.lang),
        // The base's numbers and plural forms: the pseudo text only
        // changes the letters.
        plural: base
            .plural_rule()
            .map(|r| r.name().to_string())
            .unwrap_or_default(),
        locale: base.locale.clone(),
        entries,
    }
}

fn accent(c: char) -> char {
    const FROM: &str = "aceinouyAEINOUYC";
    const TO: &str = "àçéîñöüýÀÉÎÑÖÜÝÇ";
    match FROM.find(c) {
        Some(i) => TO.chars().nth(FROM[..i].chars().count()).unwrap_or(c),
        None => c,
    }
}

// ----------------------------------------------------------------- CSV ----

/// The tables as CSV for translators: `key` then a column per language
/// (the default first); a plural entry is one row per form, `key#one`.
pub fn to_csv(langs: &Langs) -> String {
    let tables: Vec<&StringsDef> = langs
        .ordered()
        .into_iter()
        .filter(|t| !is_pseudo(&t.lang))
        .collect();
    let mut rows: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (col, t) in tables.iter().enumerate() {
        for (key, e) in &t.entries {
            match e {
                EntryDef::Plural(p) => {
                    for f in PluralForm::ALL {
                        if !p.get(f).is_empty() || f == PluralForm::Other {
                            let row = rows
                                .entry(format!("{key}#{}", f.key()))
                                .or_insert_with(|| vec![String::new(); tables.len()]);
                            row[col] = p.get(f).to_string();
                        }
                    }
                }
                e => {
                    let row = rows
                        .entry(key.clone())
                        .or_insert_with(|| vec![String::new(); tables.len()]);
                    row[col] = e.text().to_string();
                }
            }
        }
    }
    let mut out = String::new();
    let header: Vec<String> = std::iter::once("key".to_string())
        .chain(tables.iter().map(|t| t.lang.clone()))
        .collect();
    write_csv_row(&mut out, &header);
    for (key, cells) in rows {
        let row: Vec<String> = std::iter::once(key).chain(cells).collect();
        write_csv_row(&mut out, &row);
    }
    out
}

fn write_csv_row(out: &mut String, cells: &[String]) {
    for (i, c) in cells.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        if c.contains([',', '"', '\n', '\r']) || c.starts_with(' ') || c.ends_with(' ') {
            out.push('"');
            out.push_str(&c.replace('"', "\"\""));
            out.push('"');
        } else {
            out.push_str(c);
        }
    }
    out.push('\n');
}

/// Parses RFC 4180 CSV (quoted fields may hold commas, quotes, newlines).
pub fn parse_csv(text: &str) -> Result<Vec<Vec<String>>, String> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.trim_start_matches('\u{feff}').chars().peekable();
    let mut line = 1;
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    cell.push('"');
                    chars.next();
                }
                '"' => quoted = false,
                c => {
                    if c == '\n' {
                        line += 1;
                    }
                    cell.push(c)
                }
            }
            continue;
        }
        match c {
            '"' if cell.is_empty() => quoted = true,
            ',' => row.push(std::mem::take(&mut cell)),
            '\r' => {}
            '\n' => {
                line += 1;
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            c => cell.push(c),
        }
    }
    if quoted {
        return Err(format!("line {line}: a quoted cell never ends"));
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    Ok(rows)
}

/// What [`from_csv`] changed.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CsvImport {
    /// Languages written (new ones included).
    pub languages: Vec<String>,
    pub added: usize,
    pub changed: usize,
    pub removed: usize,
}

/// Applies a CSV (the [`to_csv`] layout) to `langs`: each language column
/// replaces that language's texts. An empty cell removes the key from that
/// language (it falls back); a language column with no table creates one
/// falling back to the default. Entry kinds (`Format`) and table settings
/// stay as they were.
pub fn from_csv(langs: &mut Langs, text: &str) -> Result<CsvImport, String> {
    let rows = parse_csv(text)?;
    let Some((header, body)) = rows.split_first() else {
        return Err("empty CSV".into());
    };
    if header.first().map(String::as_str) != Some("key") {
        return Err("the first column must be `key`".into());
    }
    let default = langs.default_lang().map(str::to_string);
    let mut report = CsvImport::default();
    for (col, lang) in header.iter().enumerate().skip(1) {
        let lang = lang.trim();
        if lang.is_empty() {
            continue;
        }
        let mut cells: BTreeMap<String, BTreeMap<Option<PluralForm>, String>> = BTreeMap::new();
        for (r, row) in body.iter().enumerate() {
            let Some(k) = row.first().map(|k| k.trim()) else {
                continue;
            };
            if k.is_empty() {
                continue;
            }
            let v = row.get(col).cloned().unwrap_or_default();
            let (key, form) = match k.rsplit_once('#') {
                Some((key, f)) => {
                    let form = PluralForm::ALL
                        .into_iter()
                        .find(|p| p.key() == f)
                        .ok_or_else(|| format!("row {}: unknown plural form `#{f}`", r + 2))?;
                    (key.to_string(), Some(form))
                }
                None => (k.to_string(), None),
            };
            cells.entry(key).or_default().insert(form, v);
        }
        let table = langs
            .tables
            .entry(lang.to_string())
            .or_insert_with(|| StringsDef {
                lang: lang.to_string(),
                fallback: default.clone().filter(|d| d != lang).unwrap_or_default(),
                ..StringsDef::default()
            });
        for (key, forms) in cells {
            let new = if forms.keys().any(Option::is_some) {
                let mut p = PluralDef::default();
                for (f, v) in &forms {
                    if let Some(f) = f {
                        *p.get_mut(*f) = v.clone();
                    }
                }
                (!p.other.is_empty()).then_some(EntryDef::Plural(p))
            } else {
                let v = forms.get(&None).cloned().unwrap_or_default();
                (!v.is_empty()).then(|| match table.entries.get(&key) {
                    Some(EntryDef::Format(_)) => EntryDef::Format(v),
                    _ => EntryDef::Text(v),
                })
            };
            match (table.entries.get(&key), new) {
                (None, Some(n)) => {
                    table.entries.insert(key, n);
                    report.added += 1;
                }
                (Some(old), Some(n)) => {
                    if *old != n {
                        table.entries.insert(key, n);
                        report.changed += 1;
                    }
                }
                (Some(_), None) => {
                    table.entries.remove(&key);
                    report.removed += 1;
                }
                (None, None) => {}
            }
        }
        report.languages.push(lang.to_string());
    }
    Ok(report)
}

// --------------------------------------------------------------- stats ----

/// A language's size.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct LangStats {
    pub lang: String,
    pub name: String,
    pub keys: usize,
    pub plurals: usize,
    /// Keys of the default language this one lacks.
    pub missing: usize,
    pub words: usize,
    pub chars: usize,
    pub glyphs: usize,
}

/// Counts per language (pseudo-locales included, last).
pub fn stats(langs: &Langs) -> Vec<LangStats> {
    let default = langs.default_lang().and_then(|d| langs.tables.get(d));
    langs
        .ordered()
        .into_iter()
        .map(|t| {
            let mut words = 0;
            let mut chars = 0;
            for e in t.entries.values() {
                for (_, text) in e.texts() {
                    let mut plain = String::new();
                    strip_placeholders(text, &mut |c| plain.push(c));
                    words += plain.split_whitespace().count();
                    chars += plain.chars().count();
                }
            }
            LangStats {
                lang: t.lang.clone(),
                name: t.name.clone(),
                keys: t.entries.len(),
                plurals: t.entries.values().filter(|e| e.is_plural()).count(),
                missing: default.map_or(0, |d| {
                    d.entries
                        .keys()
                        .filter(|k| !t.entries.contains_key(*k))
                        .count()
                }),
                words,
                chars,
                glyphs: glyphs(t).len(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(text: &str) -> StringsDef {
        StringsDef::from_ron(text).unwrap()
    }

    #[test]
    fn entries_parse_as_text_plural_and_format_and_round_trip() {
        let s = t(
            r#"Strings(lang: "pt-BR", fallback: "en-US", name: "Português",
            locale: (currency: "R$", date_order: "Dmy"),
            entries: {
                "a": "Jogar",
                "b": Plural(one: "{n} convidado", other: "{n} convidados"),
                "c": Format("{amount:currency}"),
                "d": "aspas \"e\" barra \\ e\nlinha",
            })"#,
        );
        assert_eq!(s.entries["a"], EntryDef::Text("Jogar".into()));
        assert!(s.entries["b"].is_plural());
        assert_eq!(s.entries["c"], EntryDef::Format("{amount:currency}".into()));
        let back = StringsDef::from_ron(&s.to_ron()).unwrap();
        assert_eq!(back, s);
        // One line per entry.
        assert!(s.to_ron().contains(
            "        \"b\": Plural(one: \"{n} convidado\", other: \"{n} convidados\"),\n"
        ));
        assert!(StringsDef::from_ron(r#"Strings(lang: "x", entries: {}, typo: 1)"#).is_err());
        assert!(
            StringsDef::from_ron(r#"Strings(lang: "x", entries: {"a": Plural(one: "x")})"#)
                .is_err(),
            "a plural needs `other`"
        );
    }

    #[test]
    fn the_baked_table_reads_back_its_entries_and_locale() {
        let s = t(
            r#"Strings(lang: "pt-BR", locale: (currency: "BRL"), entries: {
            "b": Plural(one: "um", other: "{n}"),
        })"#,
        );
        let bytes = s.to_tstr().unwrap();
        let table = trino_core::i18n::StringTable::parse(&bytes).unwrap();
        assert_eq!(table.locale().currency, "BRL");
        assert_eq!(table.locale().decimal, ",");
        assert_eq!(table.lookup("b"), Some("{n}"));
    }

    fn langs(files: &[&str]) -> Langs {
        let mut l = Langs::default();
        for f in files {
            let s = t(f);
            l.tables.insert(s.lang.clone(), s);
        }
        l
    }

    #[test]
    fn check_reports_each_kind_of_problem() {
        let l = langs(&[
            r#"Strings(lang: "en-US", entries: {
                "a": "Hi {name}", "b": "Bye", "c": Plural(one: "{n} cat", other: "{n} cats"),
            })"#,
            r#"Strings(lang: "pt-BR", fallback: "en-US", entries: {
                "a": "Oi {nome}", "c": Plural(one: "um gato", other: "{n} gatos"), "z": "extra",
            })"#,
            r#"Strings(lang: "ru", entries: {
                "a": "Привет {name", "c": Plural(one: "{n}", other: "{n}"),
            })"#,
            r#"Strings(lang: "de", fallback: "fr", plural: "Nope", entries: {})"#,
        ]);
        assert_eq!(l.default_lang(), Some("en-US"));
        let issues = check(&l);
        let has = |lang: &str, key: &str, kind: IssueKind, sev: Severity| {
            issues
                .iter()
                .any(|i| i.lang == lang && i.key == key && i.kind == kind && i.severity == sev)
        };
        assert!(
            has("pt-BR", "a", IssueKind::Placeholder, Severity::Error),
            "{issues:#?}"
        );
        assert!(
            has("pt-BR", "b", IssueKind::MissingKey, Severity::Warning),
            "falls back"
        );
        assert!(
            !has("pt-BR", "c", IssueKind::Placeholder, Severity::Error),
            "one may drop {{n}}"
        );
        assert!(has("pt-BR", "z", IssueKind::ExtraKey, Severity::Warning));
        assert!(
            has("ru", "b", IssueKind::MissingKey, Severity::Error),
            "no fallback"
        );
        assert!(
            has("ru", "a", IssueKind::Placeholder, Severity::Error),
            "unbalanced"
        );
        assert!(
            has("ru", "c", IssueKind::PluralForm, Severity::Warning),
            "few/many"
        );
        assert!(
            has("de", "", IssueKind::BadTable, Severity::Error),
            "plural rule, fallback"
        );
        assert_eq!(
            issues
                .iter()
                .filter(|i| i.lang == "de" && i.kind == IssueKind::BadTable)
                .count(),
            2
        );
        let used: BTreeSet<String> = ["a".to_string(), "c".to_string()].into();
        let un = unused(&l, &used);
        assert_eq!(un.len(), 1);
        assert_eq!(un[0].key, "b");
    }

    #[test]
    fn glyphs_skip_placeholders_and_add_the_locales_number_symbols() {
        let s = t(r#"Strings(lang: "pt-BR", name: "Pt", entries: {
            "a": "Olá {{x}}", "m": Format("{v:currency}"),
        })"#);
        let g: String = glyphs(&s).into_iter().collect();
        assert_eq!(g, " $,-.0123456789OPRltx{}á");
        let plain = t(r#"Strings(lang: "en-US", entries: {"a": "{who} hi"})"#);
        let g: String = glyphs(&plain).into_iter().collect();
        assert!(
            g.contains('0'),
            "an untyped placeholder may be a number: {g}"
        );
        let words = t(r#"Strings(lang: "en-US", entries: {"a": "{who:str} hi"})"#);
        let g: String = glyphs(&words).into_iter().collect();
        assert_eq!(g, " hi");
    }

    #[test]
    fn a_language_shows_and_needs_the_glyphs_its_fallback_lends() {
        let l = langs(&[
            r#"Strings(lang: "en-US", entries: {"a": "Fallback", "b": "x"})"#,
            r#"Strings(lang: "pt-BR", fallback: "en-US", entries: {"b": "ç"})"#,
        ]);
        let g = shown_glyphs(&l, "pt-BR");
        assert!(g.contains(&'F') && g.contains(&'ç') && !g.contains(&'x'));
        assert_eq!(effective(&l, "pt-BR").unwrap().entries.len(), 2);
        assert!(effective(&l, "de").is_none());
    }

    #[test]
    fn pseudo_accents_brackets_and_lengthens_but_keeps_placeholders() {
        let s = t(r#"Strings(lang: "en-US", entries: {
            "a": "Play now", "b": Plural(one: "{n} guest", other: "{n} guests"), "f": Format("{x}"),
        })"#);
        let p = pseudo(&s, PSEUDO_TAG, 40);
        assert_eq!(p.lang, "qps-ploc");
        assert_eq!(p.fallback, "en-US");
        let a = p.entries["a"].text();
        assert!(a.starts_with("[Plàý ñöw"), "{a}");
        assert!(a.ends_with(']'));
        assert!(
            a.chars().count() as f32 >= "Play now".len() as f32 * 1.4,
            "{a}"
        );
        assert!(p.entries["b"].text().contains("{n}"));
        assert_eq!(p.entries["f"].text(), "{x}");
        let l = langs(&[&s.to_ron(), &p.to_ron()]);
        assert!(
            check(&l).iter().all(|i| i.lang != PSEUDO_TAG),
            "{:?}",
            check(&l)
        );
        assert!(is_pseudo(PSEUDO_TAG));
        assert_eq!(l.ordered().last().unwrap().lang, PSEUDO_TAG);
    }

    #[test]
    fn csv_round_trips_plurals_quotes_and_new_lines() {
        let mut l = langs(&[
            r#"Strings(lang: "en-US", entries: {
                "a": "Hi, \"you\"", "b": Plural(one: "{n} cat", other: "{n} cats"),
                "m": Format("{v:currency}"), "nl": "two\nlines",
            })"#,
            r#"Strings(lang: "pt-BR", fallback: "en-US", entries: {"a": "Oi"})"#,
        ]);
        let csv = to_csv(&l);
        assert!(csv.starts_with("key,en-US,pt-BR\n"), "{csv}");
        assert!(csv.contains("b#one,{n} cat,\n"));
        let before = l.clone();
        let r = from_csv(&mut l, &csv).unwrap();
        assert_eq!((r.added, r.changed, r.removed), (0, 0, 0));
        assert_eq!(l.tables, before.tables);
        // A translator fills pt-BR, empties a cell and adds es-ES.
        let edited = "key,en-US,pt-BR,es-ES\na,\"Hi, \"\"you\"\"\",,Hola\nb#one,{n} cat,{n} gato,\nb#other,{n} cats,{n} gatos,\nm,{v:currency},{v:currency},\nnl,\"two\nlines\",duas,\n";
        let r = from_csv(&mut l, edited).unwrap();
        assert_eq!(r.languages, ["en-US", "pt-BR", "es-ES"]);
        let pt = &l.tables["pt-BR"];
        assert!(
            !pt.entries.contains_key("a"),
            "an empty cell removes the key"
        );
        assert!(pt.entries["b"].is_plural());
        assert_eq!(pt.entries["m"], EntryDef::Text("{v:currency}".into()));
        assert_eq!(
            l.tables["en-US"].entries["m"],
            EntryDef::Format("{v:currency}".into())
        );
        assert_eq!(l.tables["es-ES"].fallback, "en-US");
        assert_eq!(l.tables["es-ES"].entries["a"].text(), "Hola");
        assert!(parse_csv("a,\"b").is_err());
    }

    #[test]
    fn stats_count_words_and_missing_keys() {
        let l = langs(&[
            r#"Strings(lang: "en-US", entries: {"a": "Play now {n}", "b": "x"})"#,
            r#"Strings(lang: "pt-BR", fallback: "en-US", entries: {"a": "Jogar"})"#,
        ]);
        let s = stats(&l);
        assert_eq!(s[0].lang, "en-US");
        assert_eq!((s[0].words, s[0].missing), (3, 0));
        assert_eq!((s[1].words, s[1].missing, s[1].keys), (1, 1, 1));
    }
}
