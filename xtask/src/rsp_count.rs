//! `cargo xtask rsp count [--json] [--bless]` (issue #79, ADR 0038): static
//! instruction counts of each RSP overlay path, and IMEM/DMEM use, from the
//! assembled overlays (`target/n64/rsp/<overlay>.elf`, built by `build
//! n64`'s shim step).
//!
//! Counting reads the ELF's symbol table (`mips64-elf-nm -n`): every label
//! of `.text` starts a block that runs to the next label, so a block's size
//! in words is its instruction count (delay slots included). A *path* is a
//! named span of labels (`PATHS`): the command's setup, a segment's setup,
//! the vertex loop, the triangle loop; the water overlay's surface vertex,
//! ripple pass and ring vertex (#69 counted these by reading the asm: ares
//! charges ~4–5 cycles per RSP instruction, dependent or not, so the count
//! is the model of an overlay's speed).
//!
//! `tests/perf/rsp_counts.toml` pins them: `cargo xtask test n64` (CI's n64
//! job) fails when a count or a memory size moves without `--bless`, so a
//! change to `rsp_trino.S` shows its cost in the PR's diff.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::process::ExitCode;

use crate::project::Project;

pub const COUNTS_FILE: &str = "tests/perf/rsp_counts.toml";

/// The overlays `compile_shim` assembles.
pub const OVERLAYS: [&str; 4] = [
    "rsp_trino",
    "rsp_trino_color",
    "rsp_trino_tex",
    "rsp_trino_water",
];

/// IMEM and DMEM are 4 KB each.
pub const IMEM_BYTES: u64 = 4096;
pub const DMEM_BYTES: u64 = 4096;

/// Named paths: a name and its label spans `[from, to)`, summed. A path
/// whose labels an overlay lacks is left out for that overlay. Counts are
/// static (every instruction of the span, branches not followed): the
/// vertex paths are per pass of the loop (two vertices on the colored,
/// textured and water overlays).
pub const PATHS: &[(&str, &[(&str, &str)])] = &[
    ("command", &[("TrinoCmd_Draw", "draw_segment")]),
    // A palette draw's own entry and its table's copy, before the command.
    ("palette entry", &[("TrinoCmd_DrawPal", "TrinoCmd_Draw")]),
    ("segment", &[("draw_segment", "transform_vertex")]),
    ("vertex pass", &[("transform_vertex", "vertices_done")]),
    ("triangle", &[("draw_triangle", "segment_done")]),
    (
        "rdpq triangle",
        &[("RDPQ_Triangle_Send_Async", "RDPQ_Triangle_Send_End")],
    ),
    // The water overlay's three kinds of pass (`rsp_trino.S`'s labels
    // `water_ripple_pass` and `water_lit` mark them; issue #69).
    (
        "water surface pass",
        &[
            ("transform_vertex", "water_ripple_pass"),
            ("water_ripples_done", "vertices_done"),
        ],
    ),
    (
        "water ripples",
        &[("water_ripple_pass", "water_ripples_done")],
    ),
    (
        "water ring pass",
        &[
            ("transform_vertex", "water_ripple_pass"),
            ("water_ripples_done", "water_lit"),
            ("water_fog", "vertices_done"),
        ],
    ),
];

/// One overlay's counts.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OverlayCounts {
    pub imem_bytes: u64,
    pub dmem_bytes: u64,
    /// Path → instructions.
    pub paths: BTreeMap<String, u64>,
    /// Every label's block → instructions (the detail behind the paths).
    pub blocks: BTreeMap<String, u64>,
}

/// Count one overlay from its `nm -n` and `size -A` text.
pub fn count(nm: &str, size: &str) -> OverlayCounts {
    let sec = |name: &str| -> Option<(u64, u64)> {
        size.lines().find_map(|l| {
            let mut p = l.split_whitespace();
            (p.next()? == name).then_some(())?;
            Some((p.next()?.parse().ok()?, p.next()?.parse().ok()?))
        })
    };
    // `size -A` prints the sign-extended 64-bit address; nm the 32 bits.
    let (text_size, text_addr) = sec(".text").map_or((0, 0), |(s, a)| (s, a & 0xFFFF_FFFF));
    let data = sec(".data").map_or(0, |s| s.0);
    let bss = sec(".bss").map_or(0, |s| s.0);
    let text_end = text_addr + text_size;
    // Labels inside .text, by address (several names at one address: the
    // last one listed owns the block, the others are empty).
    let mut labels: Vec<(u64, String)> = nm
        .lines()
        .filter_map(|l| {
            let mut p = l.split_whitespace();
            let addr = u64::from_str_radix(p.next()?, 16).ok()?;
            let kind = p.next()?;
            let name = p.next()?;
            (matches!(kind, "t" | "T") && addr >= text_addr && addr < text_end)
                .then(|| (addr, name.to_string()))
        })
        .collect();
    labels.sort_by_key(|l| l.0);
    let mut blocks = BTreeMap::new();
    let mut at = BTreeMap::new();
    for (i, (addr, name)) in labels.iter().enumerate() {
        let next = labels.get(i + 1).map_or(text_end, |l| l.0);
        blocks.insert(name.clone(), (next - addr) / 4);
        at.entry(name.clone()).or_insert(*addr);
    }
    let mut paths = BTreeMap::new();
    'path: for (name, spans) in PATHS {
        let mut n = 0;
        for (from, to) in *spans {
            match (at.get(*from), at.get(*to)) {
                (Some(a), Some(b)) if b > a => n += (b - a) / 4,
                _ => continue 'path,
            }
        }
        paths.insert(name.to_string(), n);
    }
    OverlayCounts {
        imem_bytes: text_size,
        dmem_bytes: data + bss,
        paths,
        blocks,
    }
}

/// The pinned file: per overlay, its memory and paths (blocks are printed,
/// not pinned: a label renamed would fail the gate for nothing).
pub fn render(counts: &BTreeMap<String, OverlayCounts>) -> String {
    let mut s = String::from(
        "# Static RSP instruction counts per overlay path and IMEM/DMEM use\n\
         # (`cargo xtask rsp count`, issue #79). `cargo xtask test n64` fails when\n\
         # one moves; `cargo xtask rsp count --bless` rewrites this file — review\n\
         # the diff: ares charges ~4-5 cycles per RSP instruction.\n",
    );
    for (ovl, c) in counts {
        let _ = writeln!(s, "\n[{ovl}]");
        let _ = writeln!(s, "imem_bytes = {}", c.imem_bytes);
        let _ = writeln!(s, "dmem_bytes = {}", c.dmem_bytes);
        for (p, n) in &c.paths {
            let _ = writeln!(s, "\"{p}\" = {n}");
        }
    }
    s
}

/// Compare a measurement with the pinned file: the differences, if any.
pub fn compare(pinned: &str, counts: &BTreeMap<String, OverlayCounts>) -> Result<(), String> {
    let pinned: toml::Table = toml::from_str(pinned).map_err(|e| format!("{COUNTS_FILE}: {e}"))?;
    let mut diffs = Vec::new();
    let now: toml::Table = toml::from_str(&render(counts)).expect("own output");
    for (ovl, t) in &now {
        let old = pinned.get(ovl).and_then(|v| v.as_table());
        for (k, v) in t.as_table().into_iter().flatten() {
            let was = old.and_then(|o| o.get(k)).and_then(|v| v.as_integer());
            if was != v.as_integer() {
                diffs.push(format!(
                    "{ovl} {k}: {} -> {}",
                    was.map_or("(none)".into(), |w| w.to_string()),
                    v
                ));
            }
        }
    }
    for (ovl, t) in &pinned {
        for k in t.as_table().into_iter().flatten().map(|(k, _)| k) {
            if now.get(ovl).and_then(|n| n.get(k)).is_none() {
                diffs.push(format!("{ovl} {k}: gone"));
            }
        }
    }
    if diffs.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "RSP overlay counts changed ({COUNTS_FILE}; `cargo xtask rsp count --bless` after \
             reviewing):\n  {}",
            diffs.join("\n  ")
        ))
    }
}

/// The table `rsp count` prints.
pub fn table(counts: &BTreeMap<String, OverlayCounts>, detail: bool) -> String {
    let mut s = String::new();
    for (ovl, c) in counts {
        let _ = writeln!(
            s,
            "{ovl}: IMEM {} / {IMEM_BYTES} bytes ({} free), DMEM {} / {DMEM_BYTES} bytes",
            c.imem_bytes,
            IMEM_BYTES.saturating_sub(c.imem_bytes),
            c.dmem_bytes
        );
        for (p, n) in &c.paths {
            let _ = writeln!(s, "  {p:<20} {n:>5} instructions");
        }
        if detail {
            for (b, n) in &c.blocks {
                let _ = writeln!(s, "    {b:<32} {n:>5}");
            }
        }
    }
    s
}

pub fn to_json(counts: &BTreeMap<String, OverlayCounts>) -> serde_json::Value {
    serde_json::Value::Object(
        counts
            .iter()
            .map(|(o, c)| {
                (
                    o.clone(),
                    serde_json::json!({
                        "imem_bytes": c.imem_bytes,
                        "imem_free": IMEM_BYTES.saturating_sub(c.imem_bytes),
                        "dmem_bytes": c.dmem_bytes,
                        "paths": c.paths,
                        "blocks": c.blocks,
                    }),
                )
            })
            .collect(),
    )
}

/// Assemble the overlays if needed and count them (the toolchain image).
pub fn measure(p: &Project) -> Result<BTreeMap<String, OverlayCounts>, String> {
    crate::n64::ensure_image(&p.engine)?;
    crate::n64::compile_shim(p)?;
    let script = "set -e\ncd target/n64/rsp\n\
                  for u in rsp_trino rsp_trino_color rsp_trino_tex rsp_trino_water; do \
                  mips64-elf-nm -n $u.elf > $u.nm; mips64-elf-size -A -d $u.elf > $u.size; done\n";
    let status = crate::n64::docker_engine_sh(p, script).map_err(|e| format!("docker: {e}"))?;
    if !status.success() {
        return Err("reading the RSP overlays' symbols failed".into());
    }
    let dir = p.engine.join("target/n64/rsp");
    let mut out = BTreeMap::new();
    for ovl in OVERLAYS {
        let read = |ext: &str| {
            let f = dir.join(format!("{ovl}.{ext}"));
            std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))
        };
        out.insert(ovl.to_string(), count(&read("nm")?, &read("size")?));
    }
    Ok(out)
}

/// `test n64`'s gate: the counts against the pinned file (`--bless`
/// rewrites it).
pub fn gate(p: &Project, bless: bool) -> Result<String, String> {
    let counts = measure(p)?;
    let file = p.engine.join(COUNTS_FILE);
    if bless || !file.exists() {
        std::fs::write(&file, render(&counts)).map_err(|e| format!("{}: {e}", file.display()))?;
        return Ok(format!("RSP counts written to {COUNTS_FILE}"));
    }
    let pinned = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
    compare(&pinned, &counts)?;
    let water = counts.get("rsp_trino_water").map_or(0, |c| c.imem_bytes);
    Ok(format!(
        "RSP counts match {COUNTS_FILE} (water overlay IMEM {water}/{IMEM_BYTES})"
    ))
}

pub fn run(p: &Project, args: &[&str], bless: bool) -> ExitCode {
    let json = args.contains(&"--json");
    let detail = args.contains(&"--blocks");
    let result = match args.first() {
        Some(&"count") => measure(p).and_then(|counts| {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&to_json(&counts)).unwrap()
                );
            } else {
                print!("{}", table(&counts, detail));
            }
            let file = p.engine.join(COUNTS_FILE);
            if bless {
                std::fs::write(&file, render(&counts))
                    .map_err(|e| format!("{}: {e}", file.display()))?;
                eprintln!("[rsp] wrote {COUNTS_FILE}");
                Ok(())
            } else {
                match std::fs::read_to_string(&file) {
                    Ok(pinned) => compare(&pinned, &counts).map(|()| {
                        eprintln!("[rsp] counts match {COUNTS_FILE}");
                    }),
                    Err(_) => Err(format!("{COUNTS_FILE} missing: run with --bless")),
                }
            }
        }),
        _ => Err("usage: cargo xtask rsp count [--json] [--blocks] [--bless]".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask rsp: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Whether `root` has the pinned file (external projects don't).
pub fn pinned(root: &Path) -> bool {
    root.join(COUNTS_FILE).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NM: &str = "\
04001000 T _start
04001100 t TrinoCmd_Draw
04001140 t draw_segment
04001180 t transform_vertex
04001200 t vertices_done
04001210 t draw_triangle
04001250 t segment_done
04001260 t RDPQ_Triangle_Send_Async
040012a0 t RDPQ_Triangle_Send_End
04000000 d DRAW
";
    const SIZE: &str = "\
rsp_trino.elf  :
section           size        addr
.text              704    18446744069481697280
.data              256    67108864
.bss              1024    67109120
Total             1984
";

    #[test]
    fn labels_become_blocks_and_paths_with_memory_from_the_sections() {
        let c = count(NM, SIZE);
        assert_eq!((c.imem_bytes, c.dmem_bytes), (704, 1280));
        assert_eq!(c.paths["command"], 0x40 / 4);
        assert_eq!(c.paths["segment"], 0x40 / 4);
        assert_eq!(c.paths["vertex pass"], 0x80 / 4);
        assert_eq!(c.paths["triangle"], 0x40 / 4);
        assert_eq!(c.paths["rdpq triangle"], 0x40 / 4);
        assert!(!c.paths.contains_key("water ring pass"), "no water labels");
        // The last label runs to the end of .text (0x04001000 + 704).
        assert_eq!(
            c.blocks["RDPQ_Triangle_Send_End"],
            (0x040012c0 - 0x040012a0) / 4
        );
        assert!(!c.blocks.contains_key("DRAW"), "data symbols are not code");
    }

    #[test]
    fn the_pinned_file_round_trips_and_a_moved_count_fails() {
        let mut counts = BTreeMap::new();
        counts.insert("rsp_trino".to_string(), count(NM, SIZE));
        let pinned = render(&counts);
        compare(&pinned, &counts).unwrap();
        let mut more = counts.clone();
        *more
            .get_mut("rsp_trino")
            .unwrap()
            .paths
            .get_mut("vertex pass")
            .unwrap() += 3;
        let e = compare(&pinned, &more).unwrap_err();
        assert!(e.contains("rsp_trino vertex pass: 32 -> 35"), "{e}");
        more.get_mut("rsp_trino").unwrap().paths.remove("triangle");
        assert!(
            compare(&pinned, &more)
                .unwrap_err()
                .contains("triangle: gone")
        );
        let t = table(&counts, true);
        assert!(t.contains("IMEM 704 / 4096 bytes (3392 free)"), "{t}");
    }
}
