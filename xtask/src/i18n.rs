//! `cargo xtask i18n <check|pseudo|export|import|stats|fonts|list>` (issue
//! #59, ADR 0026) — the text-first localization loop for people, agents
//! and CI. Every command takes `--dir <project>` (the folder holding
//! `assets/`; default the selected project) and `--json`.
//!
//! - `i18n check [--platform p] [--strict]`: the tables (missing and extra
//!   keys, placeholders, plural forms, bad locales), keys nothing uses,
//!   glyphs a font lacks for a language (on each target, through the font
//!   that language draws with), text that overflows a UI box per language
//!   and target, and font atlases past the N64's TMEM. Non-zero exit on an
//!   error (`--strict`: on a warning too).
//! - `i18n pseudo [--from <lang>] [--tag qps-ploc] [--expand 40]` writes a
//!   pseudo-locale (`assets/i18n/qps-ploc.ron`): accented, bracketed, 40%
//!   longer. PC and Web builds bake it; consoles never do.
//! - `i18n export --csv <file>` / `i18n import --csv <file>`: the
//!   translators' round trip (`key` then a column per language).
//! - `i18n export --rust <out.rs> [--platform p]`: every shipped language as
//!   `no_std` static code (`pub const EN_US: StringTable<'static>`), for a
//!   game that keeps its strings in code (Solid Eleven); `include!` it.
//! - `i18n stats`: keys, plurals, words, characters, glyphs per language.
//! - `i18n fonts [--platform p]`: every font atlas a language draws, its
//!   glyphs and its memory (N64 TMEM, 3DS texture memory) per platform.
//! - `i18n list`: the languages, the default first.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::json;
use trino_asset_pipeline::{Manifest, Platform, i18n as pipe};
use trino_core::{Family, FontId};
use trino_scene::i18n::{
    self as tables, I18nIssue, IssueKind, Langs, PSEUDO_TAG, Severity, StringsDef, is_pseudo,
};
use trino_scene::ui::{Flat, ScreenDef, Target, audit};

use crate::project::Project;
use crate::ui_render;

const USAGE: &str = "xtask: usage:\n  \
    cargo xtask i18n check [--platform n64|3ds|pc|web] [--strict] [--dir <project>] [--json]\n  \
    cargo xtask i18n pseudo [--from <lang>] [--tag qps-ploc] [--expand 40] [--dir <project>] [--json]\n  \
    cargo xtask i18n export --csv <file.csv> [--dir <project>] [--json]\n  \
    cargo xtask i18n import --csv <file.csv> [--dir <project>] [--json]\n  \
    cargo xtask i18n export --rust <out.rs> [--platform n64|3ds|pc|web] [--dir <project>] [--json]\n  \
    cargo xtask i18n stats [--dir <project>] [--json]\n  \
    cargo xtask i18n fonts [--platform n64|3ds|pc|web] [--dir <project>] [--json]\n  \
    cargo xtask i18n list [--dir <project>] [--json]";

pub fn run(project: &Project, args: &[&str]) -> ExitCode {
    let mut args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let json = crate::web::take_flag(&mut args, "--json");
    let strict = crate::web::take_flag(&mut args, "--strict");
    let opts = (|| -> Result<Opts, String> {
        let take = |a: &mut Vec<String>, k: &str| crate::web::take_value(a, k);
        let platform = match take(&mut args, "--platform")? {
            Some(p) => Some(
                crate::ui::family(&p)
                    .ok_or(format!("--platform {p}: needs n64, 3ds, pc or web"))?,
            ),
            None => None,
        };
        let expand = match take(&mut args, "--expand")? {
            Some(e) => e
                .parse::<u32>()
                .ok()
                .filter(|v| *v <= 400)
                .ok_or("--expand needs a percentage (0..=400)")?,
            None => 40,
        };
        Ok(Opts {
            dir: take(&mut args, "--dir")?.map(PathBuf::from),
            platform,
            from: take(&mut args, "--from")?,
            tag: take(&mut args, "--tag")?,
            expand,
            csv: take(&mut args, "--csv")?.map(PathBuf::from),
            rust: take(&mut args, "--rust")?.map(PathBuf::from),
        })
    })();
    let opts = match opts {
        Ok(o) => o,
        Err(e) => {
            eprintln!("xtask: {e}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let root = match &opts.dir {
        Some(d) if d.is_absolute() => d.clone(),
        Some(d) => std::env::current_dir().unwrap_or_default().join(d),
        None => project.root.clone(),
    };
    let rest: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match (rest.as_slice(), &opts.csv, &opts.rust) {
        (["check"], None, None) => check(project, &root, &opts, strict, json),
        (["pseudo"], None, None) => pseudo(&root, &opts, json),
        (["export"], Some(csv), None) => export_csv(&root, csv, json),
        (["import"], Some(csv), None) => import_csv(&root, csv, json),
        (["export"], None, Some(out)) => export_rust(&root, out, &opts, json),
        (["stats"], None, None) => stats(&root, json),
        (["fonts"], None, None) => fonts(project, &root, &opts, json),
        (["list"], None, None) => list(&root, json),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            if json {
                println!("{}", json!({ "error": e }));
            } else {
                eprintln!("{e}");
            }
            ExitCode::FAILURE
        }
    }
}

struct Opts {
    dir: Option<PathBuf>,
    platform: Option<Family>,
    from: Option<String>,
    tag: Option<String>,
    expand: u32,
    csv: Option<PathBuf>,
    rust: Option<PathBuf>,
}

fn manifest(root: &Path) -> Result<Manifest, String> {
    Manifest::load(&root.join("assets").join("manifest.toml"))
}

fn load_langs(root: &Path, manifest: &Manifest) -> Result<Langs, String> {
    pipe::langs(&root.join("assets"), manifest.i18n.as_ref())
}

fn i18n_dir(root: &Path) -> PathBuf {
    root.join(tables::I18N_DIR)
}

/// The targets to check: `--platform`, else the project's (the engine
/// itself and examples: all four).
fn families(project: &Project, root: &Path, opts: &Opts) -> Vec<Family> {
    match opts.platform {
        Some(f) => vec![f],
        None if root == project.root => crate::ui::families(project),
        None => vec![Family::N64, Family::N3ds, Family::Pc, Family::Web],
    }
}

// --------------------------------------------------------------- check ----

fn check(
    project: &Project,
    root: &Path,
    opts: &Opts,
    strict: bool,
    json: bool,
) -> Result<bool, String> {
    let manifest = manifest(root)?;
    let langs = load_langs(root, &manifest)?;
    if langs.tables.is_empty() {
        let msg = format!("i18n check: no tables in {}", i18n_dir(root).display());
        if json {
            println!(
                "{}",
                json!({"languages": [], "issues": [], "errors": 0, "warnings": 0})
            );
        } else {
            println!("{msg}");
        }
        return Ok(true);
    }
    let mut issues = tables::check(&langs);
    issues.extend(tables::unused(&langs, &used_keys(root)));
    let families = families(project, root, opts);
    issues.extend(glyph_issues(root, &manifest, &langs, &families));
    issues.extend(overflow_issues(root, &manifest, &langs, &families));
    issues.extend(budget_issues(root, &manifest, &families));
    issues.sort_by(|a, b| {
        (b.severity, &a.lang, &a.key, &a.message).cmp(&(a.severity, &b.lang, &b.key, &b.message))
    });
    issues.dedup();
    let errors = issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .count();
    let warnings = issues
        .iter()
        .filter(|i| i.severity == Severity::Warning)
        .count();
    let infos = issues.len() - errors - warnings;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "languages": langs.ordered().iter().map(|t| &t.lang).collect::<Vec<_>>(),
                "default": langs.default_lang(),
                "targets": families.iter().map(|f| f.key()).collect::<Vec<_>>(),
                "issues": issues,
                "errors": errors,
                "warnings": warnings,
                "infos": infos,
            }))
            .unwrap()
        );
    } else {
        for i in &issues {
            println!("{i}");
        }
        println!(
            "i18n check: {} language(s), {} target(s): {errors} error(s), {warnings} warning(s), \
             {infos} note(s)",
            langs.tables.len(),
            families.len()
        );
    }
    Ok(errors == 0 && (!strict || warnings == 0))
}

/// Every key a UI screen names, every sound's `caption` in the manifest
/// (issue #60), and every string literal of the project's Rust sources (a
/// key the game passes to `StrKey::new`).
fn used_keys(root: &Path) -> BTreeSet<String> {
    let mut used = BTreeSet::new();
    if let Ok(m) = trino_asset_pipeline::Manifest::load(&root.join("assets/manifest.toml")) {
        for c in trino_asset_pipeline::access::captions(&m) {
            used.extend(c.caption);
        }
    }
    // Achievements' names and descriptions, presence keys (issue #61).
    if let Ok(Some(def)) = trino_scene::services::load(root) {
        used.extend(def.keys());
    }
    for path in trino_asset_pipeline::ui::screen_files(root) {
        if let Ok(def) = ScreenDef::load(&path) {
            for (_, n) in def.nodes() {
                if !n.key.is_empty() {
                    used.insert(n.key.clone());
                }
            }
        }
    }
    let mut files = Vec::new();
    rust_files(root, &mut files, 0);
    for f in files {
        if let Ok(text) = std::fs::read_to_string(&f) {
            let mut rest = text.as_str();
            while let Some(a) = rest.find('"') {
                let tail = &rest[a + 1..];
                let Some(b) = tail.find('"') else { break };
                let lit = &tail[..b];
                if !lit.is_empty() && lit.len() < 96 && !lit.contains(char::is_whitespace) {
                    used.insert(lit.to_string());
                }
                rest = &tail[b + 1..];
            }
        }
    }
    used
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 6 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if p.is_dir() {
            if !matches!(
                name.as_ref(),
                "target" | ".git" | "node_modules" | "docker" | "tools"
            ) {
                rust_files(&p, out, depth + 1);
            }
        } else if name.ends_with(".rs") {
            out.push(p);
        }
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

/// Glyphs a language uses that the font it draws a checked font with lacks,
/// per target.
fn glyph_issues(
    root: &Path,
    manifest: &Manifest,
    langs: &Langs,
    families: &[Family],
) -> Vec<I18nIssue> {
    let checked = pipe::checked_fonts(manifest, root);
    let mut out = Vec::new();
    for &f in families {
        let platform = ui_render::pipeline_platform(f);
        let fonts = trino_asset_pipeline::ui::fonts(root, manifest, platform);
        for t in pipe::baked_for(langs, manifest.i18n.as_ref(), platform) {
            let need = tables::shown_glyphs(langs, &t.lang);
            for font in &checked {
                let draws = pipe::font_for(manifest, font, &t.lang, platform);
                let Some(table) = fonts.get(FontId::from_path(&draws)) else {
                    continue;
                };
                let missing: String = need
                    .iter()
                    .filter(|c| **c != ' ' && table.glyph(**c).is_none())
                    .collect();
                if !missing.is_empty() {
                    let sev = if is_pseudo(&t.lang) {
                        Severity::Info
                    } else {
                        Severity::Error
                    };
                    out.push(issue(
                        sev,
                        IssueKind::MissingGlyph,
                        &t.lang,
                        "",
                        format!("{draws} on {}: no glyph for {missing:?}", f.key()),
                    ));
                }
            }
        }
    }
    out
}

/// Text that overflows its box on a target in a language: every UI screen
/// laid out as the runtime will.
fn overflow_issues(
    root: &Path,
    manifest: &Manifest,
    langs: &Langs,
    families: &[Family],
) -> Vec<I18nIssue> {
    let mut out = Vec::new();
    for path in trino_asset_pipeline::ui::screen_files(root) {
        let Ok(def) = ScreenDef::load(&path) else {
            continue;
        };
        for &f in families {
            let platform = ui_render::pipeline_platform(f);
            let fonts = trino_asset_pipeline::ui::fonts(root, manifest, platform);
            let sizes =
                |s: &str| trino_asset_pipeline::ui::sprite_size(root, manifest, platform, s);
            let flat = Flat::new(&def, &sizes);
            let langs = pipe::with_remaps(langs.clone(), manifest, platform);
            for t in pipe::baked_for(&langs, manifest.i18n.as_ref(), platform) {
                let report = audit(
                    &flat,
                    Target {
                        caps: crate::ui::caps_of(f),
                        lang: Some(&t.lang),
                        interface_scale: 1.0,
                        access: trino_core::ui::UiAccess::NONE,
                    },
                    &fonts,
                    &langs,
                );
                let nodes = def.nodes();
                for &i in &report.overflow {
                    let n = nodes[i].1;
                    let sev = if is_pseudo(&t.lang) {
                        Severity::Warning
                    } else {
                        Severity::Error
                    };
                    out.push(issue(
                        sev,
                        IssueKind::Overflow,
                        &t.lang,
                        &n.key,
                        format!(
                            "{} on {}: {} overflows its box",
                            def.name,
                            f.key(),
                            n.label()
                        ),
                    ));
                }
            }
        }
    }
    out
}

/// Share of TMEM at which a checked font atlas gets a warning (GitHub #99:
/// castle64's pt-BR and es-ES atlases sat at 100 %, one glyph from not
/// fitting, and nothing said so).
pub const TMEM_WARN_PERCENT: u64 = 75;

/// Checked font atlases near or past the N64's TMEM, warnings both: from
/// [`TMEM_WARN_PERCENT`] (a few more glyphs and it won't fit), and past it
/// (it draws, but no longer stays resident: every glyph re-uploads, ~50 µs
/// each; big display fonts, Solid Eleven's 22 and 44 px, live with that).
fn budget_issues(root: &Path, manifest: &Manifest, families: &[Family]) -> Vec<I18nIssue> {
    if !families.contains(&Family::N64) {
        return Vec::new();
    }
    let checked = pipe::checked_fonts(manifest, root);
    let Ok(costs) = pipe::font_costs(&root.join("assets"), manifest, &[Platform::N64]) else {
        return Vec::new();
    };
    costs
        .into_iter()
        .filter(|c| {
            checked
                .iter()
                .any(|f| c.font == *f || c.font.starts_with(&format!("{f}@")))
        })
        .filter_map(|c| {
            let budget = c.budget.unwrap_or(4096);
            let percent = c.memory_bytes * 100 / budget;
            let atlas = format!(
                "{} on n64: {}x{} {} atlas takes {} of TMEM's {} bytes ({percent} %)",
                c.font, c.width, c.height, c.format, c.memory_bytes, budget
            );
            let msg = if !c.fits {
                format!(
                    "{atlas}: past TMEM, its glyphs upload per run (fewer glyphs, a smaller \
                     size or per_language shrink it)"
                )
            } else if percent >= TMEM_WARN_PERCENT {
                format!(
                    "{atlas}, {TMEM_WARN_PERCENT} % or more: a few more glyphs and it won't \
                     fit (fewer glyphs, a smaller size or per_language shrink it)"
                )
            } else {
                return None;
            };
            Some(issue(
                Severity::Warning,
                IssueKind::FontBudget,
                &c.lang,
                "",
                msg,
            ))
        })
        .collect()
}

// -------------------------------------------------------------- pseudo ----

fn pseudo(root: &Path, opts: &Opts, json: bool) -> Result<bool, String> {
    let manifest = manifest(root)?;
    let langs = load_langs(root, &manifest)?;
    let from = match &opts.from {
        Some(f) => f.clone(),
        None => langs
            .default_lang()
            .ok_or("i18n pseudo: no string table to start from")?
            .to_string(),
    };
    let base = langs
        .tables
        .get(&from)
        .ok_or_else(|| format!("--from {from}: no assets/i18n/{from}.ron"))?;
    let tag = opts.tag.clone().unwrap_or_else(|| PSEUDO_TAG.into());
    if !is_pseudo(&tag) {
        return Err(format!(
            "--tag {tag}: a pseudo-locale's tag starts with `qps`"
        ));
    }
    let p = tables::pseudo(base, &tag, opts.expand);
    let path = i18n_dir(root).join(format!("{tag}.ron"));
    p.save(&path)?;
    if json {
        println!(
            "{}",
            json!({"file": path.display().to_string(), "lang": tag, "from": from,
                   "entries": p.entries.len(), "expand": opts.expand})
        );
    } else {
        println!(
            "i18n pseudo: wrote {} ({} entries from {from}, +{}%)",
            path.display(),
            p.entries.len(),
            opts.expand
        );
    }
    Ok(true)
}

// ----------------------------------------------------------------- CSV ----

fn export_csv(root: &Path, out: &Path, json: bool) -> Result<bool, String> {
    let manifest = manifest(root)?;
    let langs = load_langs(root, &manifest)?;
    let csv = tables::to_csv(&langs);
    std::fs::write(out, &csv).map_err(|e| format!("{}: {e}", out.display()))?;
    let rows = csv.lines().count().saturating_sub(1);
    if json {
        println!(
            "{}",
            json!({"file": out.display().to_string(), "rows": rows,
                   "languages": langs.ordered().iter().filter(|t| !is_pseudo(&t.lang)).map(|t| &t.lang).collect::<Vec<_>>()})
        );
    } else {
        println!("i18n export: {} ({rows} rows)", out.display());
    }
    Ok(true)
}

fn import_csv(root: &Path, file: &Path, json: bool) -> Result<bool, String> {
    let manifest = manifest(root)?;
    let mut langs = load_langs(root, &manifest)?;
    let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let report = tables::from_csv(&mut langs, &text)?;
    let dir = i18n_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for lang in &report.languages {
        langs.tables[lang].save(&dir.join(format!("{lang}.ron")))?;
    }
    if json {
        println!("{}", serde_json::to_string(&report).unwrap());
    } else {
        println!(
            "i18n import: {} language(s): {} added, {} changed, {} removed",
            report.languages.len(),
            report.added,
            report.changed,
            report.removed
        );
    }
    Ok(true)
}

// ---------------------------------------------------------------- Rust ----

fn export_rust(root: &Path, out: &Path, opts: &Opts, json: bool) -> Result<bool, String> {
    let manifest = manifest(root)?;
    let langs = load_langs(root, &manifest)?;
    let platform = ui_render::pipeline_platform(opts.platform.unwrap_or(Family::N64));
    let shipped = pipe::shipped(&langs, manifest.i18n.as_ref());
    if shipped.is_empty() {
        return Err(format!(
            "i18n export: no tables in {}",
            i18n_dir(root).display()
        ));
    }
    let code = to_rust(&manifest, &shipped, platform)?;
    std::fs::write(out, &code).map_err(|e| format!("{}: {e}", out.display()))?;
    if json {
        println!(
            "{}",
            json!({"file": out.display().to_string(),
                   "languages": shipped.iter().map(|t| &t.lang).collect::<Vec<_>>(),
                   "bytes": code.len()})
        );
    } else {
        println!(
            "i18n export: {} ({} language(s))",
            out.display(),
            shipped.len()
        );
    }
    Ok(true)
}

/// `EN_US` for `en-US`.
pub fn const_name(lang: &str) -> String {
    lang.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// Every table as static Rust: the same bytes the bake writes, split into
/// [`trino_core::i18n::StringTable::from_parts`]' parts, the pool one
/// literal per text so a string change is a one-line diff.
pub fn to_rust(
    manifest: &Manifest,
    shipped: &[&StringsDef],
    platform: Platform,
) -> Result<String, String> {
    let mut s = String::new();
    s.push_str(
        "// Generated by `cargo xtask i18n export --rust` from assets/i18n/*.ron: edit those\n\
         // files and export again (a test fails on a stale export).\n\n",
    );
    let mut names = Vec::new();
    for t in shipped {
        let (fonts, group) = pipe::remaps(manifest, &t.lang, platform);
        let bytes = t.to_tstr_with(&fonts, group)?;
        let table = trino_core::i18n::StringTable::parse(&bytes)
            .map_err(|e| format!("{}: {e:?}", t.lang))?;
        let (header, fonts_b, records, pool) = table.parts();
        let name = const_name(&t.lang);
        let keys: BTreeMap<u32, &str> = t
            .entries
            .keys()
            .map(|k| (trino_core::asset_id(k), k.as_str()))
            .collect();
        let _ = writeln!(
            s,
            "/// `{}`{}.",
            t.lang,
            if t.name.is_empty() {
                String::new()
            } else {
                format!(" ({})", t.name)
            }
        );
        let _ = writeln!(
            s,
            "pub const {name}: trino_core::i18n::StringTable<'static> =\n    \
             trino_core::i18n::StringTable::from_parts("
        );
        let _ = writeln!(s, "        &{},", bytes_lit(header));
        let _ = writeln!(s, "        &{},", bytes_lit(fonts_b));
        s.push_str("        &[\n");
        for r in records.as_chunks::<12>().0 {
            let hash = u32::from_le_bytes([r[0], r[1], r[2], r[3]]);
            let line: Vec<String> = r.iter().map(|b| format!("{b}")).collect();
            let _ = writeln!(
                s,
                "            {}, // {}",
                line.join(", "),
                keys.get(&hash).copied().unwrap_or("?")
            );
        }
        s.push_str("        ],\n        concat!(\n");
        // The pool in the order the bake wrote it: the seven header refs,
        // then each record's text.
        let mut cuts: Vec<(usize, usize)> = Vec::new();
        for r in 0..7 {
            let at = 8 + r * 8;
            let off = u32::from_le_bytes(header[at..at + 4].try_into().unwrap()) as usize;
            let len = u32::from_le_bytes(header[at + 4..at + 8].try_into().unwrap()) as usize;
            cuts.push((off, len));
        }
        for r in records.as_chunks::<12>().0 {
            let off = u32::from_le_bytes([r[4], r[5], r[6], r[7]]) as usize;
            let len = (u32::from_le_bytes([r[8], r[9], r[10], r[11]]) & !(1 << 31)) as usize;
            cuts.push((off, len));
        }
        let mut at = 0;
        for (off, len) in cuts {
            if off != at {
                return Err(format!("{}: unexpected pool layout", t.lang));
            }
            let piece = &pool[off..off + len];
            if !piece.is_empty() {
                let _ = writeln!(s, "            {piece:?},");
            }
            at = off + len;
        }
        if at != pool.len() {
            return Err(format!("{}: unexpected pool tail", t.lang));
        }
        s.push_str("        ),\n    );\n\n");
        names.push(name);
    }
    let _ = writeln!(
        s,
        "/// Every language, the default first.\npub const TABLES: [trino_core::i18n::StringTable<'static>; {}] = [{}];",
        names.len(),
        names.join(", ")
    );
    Ok(s)
}

fn bytes_lit(b: &[u8]) -> String {
    let v: Vec<String> = b.iter().map(|x| x.to_string()).collect();
    format!("[{}]", v.join(", "))
}

// --------------------------------------------------------------- stats ----

fn stats(root: &Path, json: bool) -> Result<bool, String> {
    let manifest = manifest(root)?;
    let langs = load_langs(root, &manifest)?;
    let rows = tables::stats(&langs);
    if json {
        println!("{}", serde_json::to_string_pretty(&rows).unwrap());
    } else {
        println!(
            "{:<10} {:<22} {:>5} {:>7} {:>7} {:>6} {:>6} {:>6}",
            "lang", "name", "keys", "plural", "missing", "words", "chars", "glyphs"
        );
        for r in &rows {
            println!(
                "{:<10} {:<22} {:>5} {:>7} {:>7} {:>6} {:>6} {:>6}",
                r.lang, r.name, r.keys, r.plurals, r.missing, r.words, r.chars, r.glyphs
            );
        }
    }
    Ok(true)
}

// --------------------------------------------------------------- fonts ----

fn fonts(project: &Project, root: &Path, opts: &Opts, json: bool) -> Result<bool, String> {
    let manifest = manifest(root)?;
    let platforms: Vec<Platform> = families(project, root, opts)
        .into_iter()
        .map(ui_render::pipeline_platform)
        .collect();
    let costs = pipe::font_costs(&root.join("assets"), &manifest, &platforms)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&costs).unwrap());
        return Ok(true);
    }
    println!(
        "{:<26} {:<8} {:>6} {:>9} {:<5} {:<9} {:>8} {:<14} {:<10}",
        "font", "lang", "glyphs", "atlas", "plat", "format", "bytes", "memory", "group"
    );
    for c in &costs {
        let budget = match c.budget {
            Some(b) => {
                format!("{}{}", c.memory, if c.fits { "" } else { " OVER" })
                    + &format!(" {:.0}%", c.memory_bytes as f64 * 100.0 / b as f64)
            }
            None => c.memory.clone(),
        };
        println!(
            "{:<26} {:<8} {:>6} {:>9} {:<5} {:<9} {:>8} {:<14} {:<10}",
            c.font,
            c.lang,
            c.glyphs,
            format!("{}x{}", c.width, c.height),
            c.platform,
            c.format,
            c.bytes,
            budget,
            c.group
        );
    }
    Ok(true)
}

// ---------------------------------------------------------------- list ----

fn list(root: &Path, json: bool) -> Result<bool, String> {
    let manifest = manifest(root)?;
    let langs = load_langs(root, &manifest)?;
    let shipped: BTreeSet<String> = pipe::shipped(&langs, manifest.i18n.as_ref())
        .iter()
        .map(|t| t.lang.clone())
        .collect();
    let rows: Vec<serde_json::Value> = langs
        .ordered()
        .iter()
        .map(|t| {
            json!({
                "lang": t.lang,
                "name": t.name,
                "fallback": t.fallback,
                "default": langs.default_lang() == Some(t.lang.as_str()),
                "shipped": shipped.contains(&t.lang),
                "pseudo": is_pseudo(&t.lang),
                "keys": t.entries.len(),
            })
        })
        .collect();
    if json {
        println!("{}", serde_json::to_string_pretty(&rows).unwrap());
    } else {
        for r in &rows {
            println!(
                "{:<10} {:<24} keys {:>4}{}{}{}",
                r["lang"].as_str().unwrap_or(""),
                r["name"].as_str().unwrap_or(""),
                r["keys"],
                if r["default"] == true {
                    "  default"
                } else {
                    ""
                },
                if r["pseudo"] == true {
                    "  pseudo (PC/Web only)"
                } else {
                    ""
                },
                match r["fallback"].as_str() {
                    Some(f) if !f.is_empty() => format!("  falls back to {f}"),
                    _ => String::new(),
                }
            );
        }
    }
    Ok(true)
}
