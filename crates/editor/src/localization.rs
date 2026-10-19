//! The Localization tab's state (issue #59): the project's string tables
//! (`assets/i18n/<lang>.ron`) as one table of keys × languages, edited in
//! place and written back as the same RON files `cargo xtask i18n` and an
//! agent write; the issues `i18n check` reports; the editor-wide preview
//! language (the UI tab's, and `TRINO_LANG` for Play); and, per checked
//! font and language, the glyphs the language draws that the font lacks
//! and the N64 TMEM its atlas takes (measured on a worker: fonts bake).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use trino_asset_pipeline as pipeline;
use trino_core::i18n::{LangTag, PluralForm};
use trino_scene::i18n::{
    EntryDef, I18nIssue, Langs, PSEUDO_TAG, PluralDef, Severity, StringsDef, check, from_csv,
    is_pseudo, pseudo, shown_glyphs, to_csv,
};

/// Where Export / Import CSV read and write, under the project.
pub const CSV_PATH: &str = "assets/i18n/strings.csv";

/// How a cell stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cell {
    /// The language's own text.
    Own,
    /// Missing here; this language's text shows (a fallback).
    Fallback(String),
    /// Missing here and in every fallback: nothing shows.
    Missing,
}

/// One row of the table: a key, or one plural form of a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub key: String,
    /// `Some` for a plural entry's form.
    pub form: Option<PluralForm>,
}

/// One font × language line of the fonts section.
#[derive(Clone, Debug, PartialEq)]
pub struct FontRow {
    /// The font the screens name.
    pub font: String,
    pub lang: String,
    /// The font the language draws it with on the N64 (its atlas or an
    /// override).
    pub draws_with: String,
    /// Glyphs the language uses, and how many of them the font has.
    pub used: usize,
    pub covered: usize,
    /// The ones it lacks.
    pub missing: String,
    /// The atlas's N64 bytes against TMEM (`None`: not measured).
    pub n64_bytes: Option<u64>,
    pub tmem: u64,
}

/// What the fonts were measured from: the tables' revision and the
/// project's bake generation.
type FontKey = (u64, u64);

/// A fonts measurement in flight: what it is for, where it arrives.
type FontJob = (FontKey, mpsc::Receiver<Result<Vec<FontRow>, String>>);

pub struct Localization {
    pub langs: Langs,
    pub error: Option<String>,
    /// Bumped on every change (edit or reload): the issues follow.
    pub rev: u64,
    pub filter: String,
    /// The selected key (an issue click, a row click).
    pub selected: Option<String>,
    /// Scroll the table to the selected key on the next frame.
    pub scroll_to_selected: bool,
    /// The language the editor previews in: the UI tab's language and
    /// `TRINO_LANG` for Play (`None`: the default texts / the system's).
    pub preview: Option<String>,
    pub issues: Vec<I18nIssue>,
    issues_rev: u64,
    /// The "New key" / "New language" fields (`Some`: open).
    pub new_key: Option<String>,
    pub new_lang: Option<String>,
    /// The first language column shown (the table pages sideways when the
    /// languages don't fit the tab).
    pub first_column: usize,
    /// Languages edited and not written yet.
    dirty: BTreeSet<String>,
    /// The fonts section's lines (measured on a worker).
    pub fonts: Vec<FontRow>,
    fonts_key: Option<FontKey>,
    fonts_job: Option<FontJob>,
    pub fonts_error: Option<String>,
}

impl Localization {
    /// The tables under `root` (and the manifest's default language).
    pub fn load(root: &Path) -> Localization {
        let mut s = Localization {
            langs: Langs::default(),
            error: None,
            rev: 0,
            filter: String::new(),
            selected: None,
            scroll_to_selected: false,
            preview: None,
            issues: Vec::new(),
            issues_rev: u64::MAX,
            new_key: None,
            new_lang: None,
            first_column: 0,
            dirty: BTreeSet::new(),
            fonts: Vec::new(),
            fonts_key: None,
            fonts_job: None,
            fonts_error: None,
        };
        s.reload(root);
        s
    }

    /// [`load`](Self::load), then `TRINO_EDITOR_LANG=<tag>` (the preview
    /// language).
    pub fn from_env(root: &Path) -> Localization {
        let mut s = Localization::load(root);
        if let Ok(l) = std::env::var("TRINO_EDITOR_LANG") {
            let l = l.trim();
            s.preview = (!l.is_empty() && l != "default").then(|| l.to_string());
        }
        s
    }

    /// Read the tables again (an outside edit). Unwritten edits of a
    /// language whose file changed are dropped: the file wins.
    pub fn reload(&mut self, root: &Path) {
        let assets = root.join("assets");
        let decl = pipeline::Manifest::load(&assets.join("manifest.toml"))
            .ok()
            .and_then(|m| m.i18n);
        match pipeline::i18n::langs(&assets, decl.as_ref()) {
            Ok(l) => {
                self.langs = l;
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
        self.dirty.clear();
        self.rev += 1;
    }

    /// The languages as columns: the default first, pseudo-locales last.
    pub fn columns(&self) -> Vec<String> {
        self.langs
            .ordered()
            .iter()
            .map(|t| t.lang.clone())
            .collect()
    }

    pub fn default_lang(&self) -> Option<String> {
        self.langs.default_lang().map(str::to_string)
    }

    /// The rows to show: every key matching the filter (key or any text),
    /// sorted; a plural key gives one row per form some language uses or
    /// its rule needs.
    pub fn rows(&self) -> Vec<Row> {
        let f = self.filter.trim().to_lowercase();
        let mut out = Vec::new();
        for key in self.langs.keys() {
            let entries: Vec<(&StringsDef, &EntryDef)> = self
                .langs
                .tables
                .values()
                .filter_map(|t| t.entries.get(key).map(|e| (t, e)))
                .collect();
            if !f.is_empty()
                && !key.to_lowercase().contains(&f)
                && !entries
                    .iter()
                    .any(|(_, e)| e.texts().iter().any(|(_, t)| t.to_lowercase().contains(&f)))
            {
                continue;
            }
            if entries.iter().any(|(_, e)| e.is_plural()) {
                let mut forms: BTreeSet<PluralForm> = BTreeSet::new();
                forms.insert(PluralForm::Other);
                for (_, e) in &entries {
                    if let EntryDef::Plural(p) = e {
                        forms.extend(
                            PluralForm::ALL
                                .into_iter()
                                .filter(|f| !p.get(*f).is_empty()),
                        );
                    }
                }
                // Every column shows the key: each language's rule's forms.
                for t in self.langs.tables.values() {
                    forms.extend(t.plural_rule().unwrap_or_default().forms().iter().copied());
                }
                out.extend(forms.into_iter().map(|form| Row {
                    key: key.to_string(),
                    form: Some(form),
                }));
            } else {
                out.push(Row {
                    key: key.to_string(),
                    form: None,
                });
            }
        }
        out
    }

    /// The text a cell edits (its own; empty when it has none).
    pub fn text(&self, lang: &str, row: &Row) -> String {
        let Some(e) = self
            .langs
            .tables
            .get(lang)
            .and_then(|t| t.entries.get(&row.key))
        else {
            return String::new();
        };
        match (e, row.form) {
            (EntryDef::Plural(p), Some(f)) => p.get(f).to_string(),
            (EntryDef::Plural(p), None) => p.other.clone(),
            (e, Some(PluralForm::Other) | None) => e.text().to_string(),
            (_, Some(_)) => String::new(),
        }
    }

    /// Whether `lang` has `key`, else which language's text shows.
    pub fn cell(&self, lang: &str, key: &str) -> Cell {
        if self
            .langs
            .tables
            .get(lang)
            .is_some_and(|t| t.entries.contains_key(key))
        {
            return Cell::Own;
        }
        match self
            .langs
            .chain(lang)
            .into_iter()
            .skip(1)
            .find(|t| t.entries.contains_key(key))
        {
            Some(t) => Cell::Fallback(t.lang.clone()),
            None => Cell::Missing,
        }
    }

    /// The text a missing cell shows (its fallback's), for the hint.
    pub fn shown(&self, lang: &str, row: &Row) -> Option<String> {
        let form = row.form.unwrap_or(PluralForm::Other);
        self.langs
            .text_for(lang, &row.key, form)
            .map(str::to_string)
    }

    /// The issues `i18n check` reports for a cell.
    pub fn cell_issues(&self, lang: &str, key: &str) -> Vec<&I18nIssue> {
        self.issues
            .iter()
            .filter(|i| i.lang == lang && i.key == key)
            .collect()
    }

    /// Set a cell. An empty text removes the language's entry (it falls
    /// back again); a form of a plain entry makes it a plural (its text as
    /// `other`). True when something changed.
    pub fn set_text(&mut self, lang: &str, row: &Row, text: &str) -> bool {
        let Some(table) = self.langs.tables.get_mut(lang) else {
            return false;
        };
        let old = table.entries.get(&row.key).cloned();
        let new = match row.form {
            None => (!text.is_empty()).then(|| match &old {
                Some(EntryDef::Format(_)) => EntryDef::Format(text.to_string()),
                _ => EntryDef::Text(text.to_string()),
            }),
            Some(form) => {
                let mut p = match &old {
                    Some(EntryDef::Plural(p)) => p.clone(),
                    Some(e) => PluralDef {
                        other: e.text().to_string(),
                        ..PluralDef::default()
                    },
                    None => PluralDef::default(),
                };
                *p.get_mut(form) = text.to_string();
                let empty = PluralForm::ALL.iter().all(|f| p.get(*f).is_empty());
                (!empty).then_some(EntryDef::Plural(p))
            }
        };
        if new == old {
            return false;
        }
        match new {
            Some(e) => {
                table.entries.insert(row.key.clone(), e);
            }
            None => {
                table.entries.remove(&row.key);
            }
        }
        self.touch(lang);
        true
    }

    fn touch(&mut self, lang: &str) {
        self.dirty.insert(lang.to_string());
        self.rev += 1;
    }

    /// A new key in the default language (its text the key itself, for a
    /// translator to replace). `Err` says why not.
    pub fn add_key(&mut self, key: &str) -> Result<(), String> {
        let key = key.trim();
        if key.is_empty() || key.contains(char::is_whitespace) {
            return Err("a key is one word, like menu.play".into());
        }
        let Some(d) = self.default_lang() else {
            return Err("no language yet: add one first".into());
        };
        if self.langs.keys().contains(key) {
            return Err(format!("`{key}` is already a key"));
        }
        self.langs
            .tables
            .get_mut(&d)
            .unwrap()
            .entries
            .insert(key.to_string(), EntryDef::Text(key.to_string()));
        self.touch(&d);
        self.selected = Some(key.to_string());
        self.scroll_to_selected = true;
        Ok(())
    }

    /// Remove `key` from every language.
    pub fn remove_key(&mut self, key: &str) {
        let langs: Vec<String> = self
            .langs
            .tables
            .values()
            .filter(|t| t.entries.contains_key(key))
            .map(|t| t.lang.clone())
            .collect();
        for l in langs {
            self.langs.tables.get_mut(&l).unwrap().entries.remove(key);
            self.touch(&l);
        }
        if self.selected.as_deref() == Some(key) {
            self.selected = None;
        }
    }

    /// A new, empty language falling back to the default.
    pub fn add_language(&mut self, tag: &str) -> Result<String, String> {
        let tag = LangTag::parse(tag)
            .ok_or_else(|| format!("`{tag}` is not a language tag (like pt-BR)"))?;
        let tag = tag.as_str().to_string();
        if self.langs.tables.contains_key(&tag) {
            return Err(format!("{tag} already has a table"));
        }
        let fallback = self.default_lang().unwrap_or_default();
        self.langs.tables.insert(
            tag.clone(),
            StringsDef {
                fallback,
                ..StringsDef::new(&tag)
            },
        );
        self.touch(&tag);
        Ok(tag)
    }

    /// The pseudo-locale (`qps-ploc`) made from the default language,
    /// replacing an older one.
    pub fn make_pseudo(&mut self) -> Result<(), String> {
        let d = self
            .default_lang()
            .ok_or("no default language to make it from")?;
        let p = pseudo(&self.langs.tables[&d], PSEUDO_TAG, 40);
        self.langs.tables.insert(PSEUDO_TAG.into(), p);
        self.touch(PSEUDO_TAG);
        Ok(())
    }

    /// The tables as CSV ([`CSV_PATH`]).
    pub fn csv(&self) -> String {
        to_csv(&self.langs)
    }

    /// Apply a CSV: every language it names is rewritten.
    pub fn import_csv(&mut self, text: &str) -> Result<String, String> {
        let r = from_csv(&mut self.langs, text)?;
        for l in &r.languages {
            self.touch(l);
        }
        Ok(format!(
            "{} language(s): {} added, {} changed, {} removed",
            r.languages.len(),
            r.added,
            r.changed,
            r.removed
        ))
    }

    /// The files edited since the last call, as `(path, text)` to write.
    pub fn take_writes(&mut self, root: &Path) -> Vec<(PathBuf, String)> {
        std::mem::take(&mut self.dirty)
            .into_iter()
            .filter_map(|l| {
                let t = self.langs.tables.get(&l)?;
                Some((
                    root.join(trino_scene::i18n::I18N_DIR)
                        .join(format!("{l}.ron")),
                    t.to_ron(),
                ))
            })
            .collect()
    }

    pub fn has_writes(&self) -> bool {
        !self.dirty.is_empty()
    }

    /// `i18n check`'s table issues, refreshed when the tables changed.
    pub fn refresh_issues(&mut self) {
        if self.issues_rev == self.rev {
            return;
        }
        self.issues_rev = self.rev;
        let mut v = check(&self.langs);
        v.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.lang.cmp(&b.lang)));
        self.issues = v;
    }

    pub fn errors(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity == Severity::Error)
            .count()
    }

    /// Start measuring the fonts when the tables or the bake changed
    /// (once; the result arrives on a later frame), and pick up a result.
    pub fn poll_fonts(&mut self, root: &Path, generation: u64) {
        let key = (self.rev, generation);
        if let Some((k, rx)) = &self.fonts_job {
            match rx.try_recv() {
                Ok(r) => {
                    let k = *k;
                    self.fonts_job = None;
                    self.fonts_key = Some(k);
                    match r {
                        Ok(rows) => {
                            self.fonts = rows;
                            self.fonts_error = None;
                        }
                        Err(e) => self.fonts_error = Some(e),
                    }
                }
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => self.fonts_job = None,
            }
        }
        if self.fonts_key == Some(key) || self.fonts_job.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let langs = self.langs.clone();
        let root = root.to_path_buf();
        std::thread::spawn(move || {
            let _ = tx.send(measure_fonts(&root, &langs));
        });
        self.fonts_job = Some((key, rx));
    }

    pub fn fonts_measuring(&self) -> bool {
        self.fonts_job.is_some()
    }
}

/// Per checked font and shipped language: the glyphs the language uses
/// that the font it draws with lacks, and that atlas's N64 bytes.
pub fn measure_fonts(root: &Path, langs: &Langs) -> Result<Vec<FontRow>, String> {
    use pipeline::Platform;
    let assets = root.join("assets");
    let m = pipeline::Manifest::load(&assets.join("manifest.toml"))?;
    let fonts = pipeline::i18n::checked_fonts(&m, root);
    let costs = pipeline::i18n::font_costs(&assets, &m, &[Platform::N64]).unwrap_or_default();
    let tmem = trino_core::Caps::N64.texture_memory_bytes as u64;
    let mut baked: BTreeMap<String, Option<BTreeSet<char>>> = BTreeMap::new();
    let mut have = |logical: &str| -> Option<BTreeSet<char>> {
        baked
            .entry(logical.to_string())
            .or_insert_with(|| {
                let decl = m.fonts.get(logical.strip_prefix("fonts/")?)?;
                let src = pipeline::resolve_source(&assets, Platform::N64, &decl.file).ok()?;
                let b = pipeline::bake_font(&src, decl).ok()?;
                Some(
                    b.glyphs
                        .iter()
                        .filter_map(|g| char::from_u32(g.codepoint))
                        .collect(),
                )
            })
            .clone()
    };
    let mut out = Vec::new();
    for font in &fonts {
        for t in langs.ordered() {
            if is_pseudo(&t.lang) {
                continue;
            }
            let draws_with = pipeline::i18n::font_for(&m, font, &t.lang, Platform::N64);
            let used = shown_glyphs(langs, &t.lang);
            let set = have(&draws_with).unwrap_or_default();
            let missing: String = used
                .iter()
                .filter(|c| **c != ' ' && !set.contains(c))
                .collect();
            let n64_bytes = costs
                .iter()
                .find(|c| c.font == draws_with && c.platform == "n64")
                .map(|c| c.bytes);
            out.push(FontRow {
                font: font.clone(),
                lang: t.lang.clone(),
                draws_with,
                used: used.len(),
                covered: used.len() - missing.chars().count(),
                missing,
                n64_bytes,
                tmem,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Localization {
        let mut l = Localization {
            langs: Langs::default(),
            error: None,
            rev: 0,
            filter: String::new(),
            selected: None,
            scroll_to_selected: false,
            preview: None,
            issues: Vec::new(),
            issues_rev: u64::MAX,
            new_key: None,
            new_lang: None,
            first_column: 0,
            dirty: BTreeSet::new(),
            fonts: Vec::new(),
            fonts_key: None,
            fonts_job: None,
            fonts_error: None,
        };
        for src in [
            r#"Strings(lang: "en-US", entries: {
                "a": "Play", "b": "Quit {x}", "c": Plural(one: "{n} cat", other: "{n} cats"),
            })"#,
            r#"Strings(lang: "pt-BR", fallback: "en-US", entries: {"a": "Jogar", "b": "Sair {y}"})"#,
            r#"Strings(lang: "ru", entries: {"a": "Играть"})"#,
        ] {
            let t = StringsDef::from_ron(src).unwrap();
            l.langs.tables.insert(t.lang.clone(), t);
        }
        l
    }

    fn row(k: &str, f: Option<PluralForm>) -> Row {
        Row {
            key: k.into(),
            form: f,
        }
    }

    #[test]
    fn rows_cells_and_issues() {
        let mut l = fixture();
        assert_eq!(l.columns()[0], "en-US", "the default first");
        let rows = l.rows();
        // c is a plural: one, few, many (ru's rule), other.
        let c: Vec<_> = rows
            .iter()
            .filter(|r| r.key == "c")
            .map(|r| r.form)
            .collect();
        assert_eq!(
            c,
            [
                Some(PluralForm::One),
                Some(PluralForm::Few),
                Some(PluralForm::Many),
                Some(PluralForm::Other)
            ]
        );
        assert_eq!(l.cell("pt-BR", "c"), Cell::Fallback("en-US".into()));
        assert_eq!(l.cell("ru", "b"), Cell::Missing);
        assert_eq!(l.cell("pt-BR", "a"), Cell::Own);
        assert_eq!(
            l.shown("pt-BR", &row("c", Some(PluralForm::One))).unwrap(),
            "{n} cat"
        );
        l.refresh_issues();
        assert!(
            !l.cell_issues("pt-BR", "b").is_empty(),
            "placeholder mismatch"
        );
        assert!(l.errors() > 0);
        l.filter = "jog".into();
        assert_eq!(l.rows(), [row("a", None)]);
    }

    #[test]
    fn edits_mark_their_language_and_write_ron() {
        let mut l = fixture();
        assert!(l.set_text("pt-BR", &row("b", None), "Sair {x}"));
        assert!(
            !l.set_text("pt-BR", &row("b", None), "Sair {x}"),
            "no change"
        );
        // A form on a missing entry makes a plural.
        assert!(l.set_text("pt-BR", &row("c", Some(PluralForm::One)), "um gato"));
        assert!(l.set_text("pt-BR", &row("c", Some(PluralForm::Other)), "{n} gatos"));
        // Emptying a cell removes the key there.
        assert!(l.set_text("pt-BR", &row("a", None), ""));
        assert_eq!(l.cell("pt-BR", "a"), Cell::Fallback("en-US".into()));
        let w = l.take_writes(Path::new("/p"));
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].0, Path::new("/p/assets/i18n/pt-BR.ron"));
        let back = StringsDef::from_ron(&w[0].1).unwrap();
        assert!(back.entries["c"].is_plural());
        assert!(!back.entries.contains_key("a"));
        assert!(!l.has_writes());
    }

    #[test]
    fn keys_languages_pseudo_and_csv() {
        let mut l = fixture();
        assert!(l.add_key("menu new").is_err());
        assert!(l.add_key("a").is_err());
        l.add_key("menu.new").unwrap();
        assert_eq!(l.text("en-US", &row("menu.new", None)), "menu.new");
        assert_eq!(l.add_language("es_ES").unwrap(), "es-ES");
        assert_eq!(l.langs.tables["es-ES"].fallback, "en-US");
        assert!(l.add_language("es-ES").is_err());
        assert!(l.add_language("not a tag").is_err());
        l.make_pseudo().unwrap();
        assert_eq!(l.columns().last().unwrap(), PSEUDO_TAG);
        l.remove_key("a");
        assert!(
            l.langs
                .tables
                .values()
                .all(|t| !t.entries.contains_key("a"))
        );
        let csv = l.csv();
        let msg = l.import_csv(&csv.replace("Sair {y}", "Sair {x}")).unwrap();
        assert!(msg.contains("1 changed"), "{msg}");
        let langs: BTreeSet<String> = l
            .take_writes(Path::new(""))
            .into_iter()
            .map(|(p, _)| p.file_stem().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(langs.contains("es-ES") && langs.contains(PSEUDO_TAG) && langs.contains("pt-BR"));
    }
}
