//! Saves as the engine reads them (issue #68), for tools: `cargo xtask save
//! inspect|corrupt` and the editor's Saves tab.
//!
//! A save is one of:
//! - **slot files** (PC, Web export, 3DS SD): `<save>.a` and `<save>.b` beside
//!   the path given, plus `<save>` itself when it is the save from before
//!   slot files (`trino_core::save::read_slot_files`);
//! - **one slot file** (`…/x.sav.a`);
//! - **a raw medium** (an N64 EEPROM image, 512 B or 2 KB, or `medium`):
//!   the A/B slots at 0 and half the medium.

use std::path::{Path, PathBuf};

use trino_core::save::{self, Medium, SlotStore};
use trino_core::{SaveError, SaveFault, SaveStatus};

/// `<save>.a` / `<save>.b` and the legacy `<save>`, as the PC lays them out.
pub struct FileSlots<'a>(pub &'a Path);

impl FileSlots<'_> {
    pub fn slot_path(&self, slot: u8) -> PathBuf {
        let mut p = self.0.as_os_str().to_owned();
        p.push(if slot == 0 { ".a" } else { ".b" });
        PathBuf::from(p)
    }
    pub fn exists(path: &Path) -> bool {
        let s = FileSlots(path);
        s.slot_path(0).exists() || s.slot_path(1).exists()
    }
}

fn read_into(p: &Path, buf: &mut [u8]) -> Result<Option<usize>, SaveError> {
    match std::fs::read(p) {
        Ok(b) => {
            let n = b.len().min(buf.len());
            buf[..n].copy_from_slice(&b[..n]);
            Ok(Some(b.len()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(SaveError::Io),
    }
}

impl SlotStore for FileSlots<'_> {
    fn read_slot(&mut self, slot: u8, buf: &mut [u8]) -> Result<Option<usize>, SaveError> {
        read_into(&self.slot_path(slot), buf)
    }
    fn write_slot(&mut self, slot: u8, bytes: &[u8]) -> Result<(), SaveError> {
        std::fs::write(self.slot_path(slot), bytes).map_err(|_| SaveError::Io)
    }
    fn read_legacy(&mut self, buf: &mut [u8]) -> Result<Option<usize>, SaveError> {
        read_into(self.0, buf)
    }
}

/// An EEPROM image in memory.
struct Image(Vec<u8>);

impl Medium for Image {
    fn size(&self) -> usize {
        self.0.len()
    }
    fn read(&mut self, at: usize, buf: &mut [u8]) {
        buf.copy_from_slice(&self.0[at..at + buf.len()]);
    }
    fn write(&mut self, at: usize, data: &[u8]) {
        self.0[at..at + data.len()].copy_from_slice(data);
    }
}

/// Largest save inspected (the PC's slot files).
const MAX: usize = 1 << 20;

#[derive(Debug, Default)]
pub struct SlotLine {
    pub name: String,
    /// `missing`, `blank`, `damaged`, `valid`.
    pub state: &'static str,
    pub bytes: usize,
    pub seq: Option<u16>,
    pub stored: usize,
    pub raw: usize,
    pub compressed: bool,
}

#[derive(Debug)]
pub struct Report {
    pub file: String,
    /// `slots`, `slot`, `medium`.
    pub kind: &'static str,
    pub status: SaveStatus,
    pub newest: Option<String>,
    pub len: usize,
    pub version: Option<u16>,
    pub preview: String,
    pub slots: Vec<SlotLine>,
}

impl Report {
    pub fn json(&self) -> String {
        let slots: Vec<String> = self
            .slots
            .iter()
            .map(|s| {
                format!(
                    "{{\"name\":{:?},\"state\":\"{}\",\"bytes\":{},\"seq\":{},\"stored\":{},\"raw\":{},\"compressed\":{}}}",
                    s.name,
                    s.state,
                    s.bytes,
                    s.seq.map_or("null".into(), |v| v.to_string()),
                    s.stored,
                    s.raw,
                    s.compressed
                )
            })
            .collect();
        format!(
            "{{\"file\":{:?},\"kind\":\"{}\",\"status\":\"{}\",\"newest\":{},\"len\":{},\"version\":{},\"preview\":{:?},\"slots\":[{}]}}",
            self.file,
            self.kind,
            self.status.label(),
            self.newest
                .as_ref()
                .map_or("null".into(), |n| format!("{n:?}")),
            self.len,
            self.version.map_or("null".into(), |v| v.to_string()),
            self.preview,
            slots.join(",")
        )
    }

    pub fn text(&self) -> String {
        let mut o = format!(
            "{} ({})\n  status: {}\n",
            self.file,
            self.kind,
            self.status.label()
        );
        match &self.newest {
            Some(n) => o += &format!("  reads: {} bytes from {n}\n", self.len),
            None => o += "  reads: nothing\n",
        }
        if let Some(v) = self.version {
            o += &format!("  version tag: {v}\n");
        }
        if !self.preview.is_empty() {
            o += &format!("  data: {}\n", self.preview);
        }
        for s in &self.slots {
            o += &format!("  {:<12} {:<8} {:>6} B", s.name, s.state, s.bytes);
            if let Some(q) = s.seq {
                o += &format!("  seq {q}");
            }
            if s.state == "valid" {
                o += &format!(
                    "  stored {} raw {}{}",
                    s.stored,
                    s.raw,
                    if s.compressed { " (lz)" } else { "" }
                );
            }
            o += "\n";
        }
        o
    }
}

fn preview(data: &[u8]) -> String {
    let d = &data[..data.len().min(32)];
    if d.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        String::from_utf8_lossy(d).into_owned()
    } else {
        d.iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn data_meta(data: &[u8]) -> (Option<u16>, String) {
    let (v, rest) = save::split_version(data);
    ((rest.len() != data.len()).then_some(v), preview(data))
}

fn slot_file_line(name: String, bytes: Option<&[u8]>) -> SlotLine {
    let Some(b) = bytes else {
        return SlotLine {
            name,
            state: "missing",
            ..SlotLine::default()
        };
    };
    let mut l = SlotLine {
        name,
        bytes: b.len(),
        ..SlotLine::default()
    };
    match save::parse_slot(b) {
        Ok(v) => {
            l.state = "valid";
            l.seq = Some(v.seq);
            l.stored = v.data.len();
            l.raw = v.data.len();
        }
        Err(save::SlotError::Damaged(seq)) => {
            l.state = "damaged";
            l.seq = Some(seq);
        }
        Err(save::SlotError::Foreign) => l.state = if b.is_empty() { "blank" } else { "damaged" },
    }
    l
}

pub fn inspect(path: &Path, medium: bool) -> Result<Report, String> {
    let file = path.display().to_string();
    if !medium && FileSlots::exists(path) {
        let mut store = FileSlots(path);
        let mut out = vec![0u8; MAX];
        let mut scratch = vec![0u8; MAX];
        let r = save::read_slot_files(&mut store, &mut out, &mut scratch)
            .map_err(|e| format!("{e:?}"))?;
        let slots = (0..2u8)
            .map(|s| {
                let p = store.slot_path(s);
                let b = std::fs::read(&p).ok();
                slot_file_line(
                    p.file_name().unwrap().to_string_lossy().into(),
                    b.as_deref(),
                )
            })
            .chain(std::fs::read(path).ok().map(|b| SlotLine {
                name: path.file_name().unwrap().to_string_lossy().into(),
                state: "legacy",
                bytes: b.len(),
                ..SlotLine::default()
            }))
            .collect();
        let data = &out[..r.len.min(out.len())];
        let (version, preview) = data_meta(data);
        let newest = match r.slot {
            Some(s) => Some(
                store
                    .slot_path(s)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into(),
            ),
            None if r.len > 0 => Some(path.file_name().unwrap().to_string_lossy().into()),
            None => None,
        };
        return Ok(Report {
            file,
            kind: "slots",
            status: r.status,
            newest,
            len: r.len,
            version,
            preview,
            slots,
        });
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let looks_medium = medium || eeprom_sized(&bytes) || bytes.starts_with(b"TS");
    if !looks_medium {
        let line = slot_file_line(
            path.file_name().unwrap().to_string_lossy().into(),
            Some(&bytes),
        );
        let ok = save::parse_slot(&bytes).ok();
        let data = ok.map(|v| v.data).unwrap_or(&[]);
        let (version, preview) = data_meta(data);
        let status = match (ok, line.state) {
            (Some(_), _) => SaveStatus::Ok,
            (None, "blank") => SaveStatus::Empty,
            _ => SaveStatus::Corrupted,
        };
        return Ok(Report {
            file,
            kind: "slot",
            status,
            newest: ok.map(|_| line.name.clone()),
            len: data.len(),
            version,
            preview,
            slots: vec![line],
        });
    }
    let size = bytes.len();
    let mut m = Image(bytes);
    let mut scratch = vec![0u8; size];
    let mut out = vec![0u8; size];
    let (n, status) = save::read_newest_status(&mut m, &mut out, &mut scratch);
    let (found, _) = save::find_newest_status(&mut m, &mut scratch);
    let mut slots = Vec::new();
    for (name, off) in [("A @0", 0usize), ("B @half", size / 2)] {
        let mut l = SlotLine {
            name: name.into(),
            bytes: size / 2,
            ..SlotLine::default()
        };
        match save::slot_state(&mut m, off, &mut scratch) {
            Ok(f) => {
                l.state = "valid";
                l.seq = (!f.legacy).then_some(f.seq);
                l.stored = f.stored;
                l.raw = f.raw;
                l.compressed = f.compressed;
                if f.span > size / 2 {
                    l.name = format!("{name} (wide)");
                }
            }
            Err(Some(seq)) => {
                l.state = "damaged";
                l.seq = seq;
            }
            Err(None) => l.state = "blank",
        }
        slots.push(l);
    }
    let data = &out[..n.min(out.len())];
    let (version, preview) = data_meta(data);
    let newest = found.map(|f| {
        if f.offset == 0 {
            "A @0".to_string()
        } else {
            "B @half".to_string()
        }
    });
    Ok(Report {
        file,
        kind: "medium",
        status,
        newest,
        len: n,
        version,
        preview,
        slots,
    })
}

pub fn corrupt(
    path: &Path,
    medium: bool,
    mode: &str,
    slot: &str,
    at: Option<usize>,
) -> Result<String, String> {
    let fault = match (mode, slot) {
        ("flip", "all") => SaveFault::FlipAll,
        ("flip", "newest") => SaveFault::FlipNewest,
        ("truncate", "newest") => SaveFault::TearNewest,
        ("flip" | "truncate", "a" | "b") => SaveFault::FlipNewest, // handled below
        _ => {
            return Err(format!(
                "--mode flip|truncate with --slot newest|a|b|all (not {mode} {slot})"
            ));
        }
    };
    if !medium && FileSlots::exists(path) {
        let mut store = FileSlots(path);
        if slot == "a" || slot == "b" {
            let p = store.slot_path(if slot == "a" { 0 } else { 1 });
            return raw(&p, mode, at);
        }
        let mut scratch = vec![0u8; MAX];
        let hit = save::slot_files_fault(&mut store, fault, &mut scratch)
            .map_err(|e| format!("{e:?}"))?;
        return if hit {
            Ok(format!("{mode} {slot}: done ({})", path.display()))
        } else {
            Err("no intact slot to damage".into())
        };
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let is_medium = medium || eeprom_sized(&bytes);
    if !is_medium || at.is_some() {
        return raw(path, mode, at);
    }
    let size = bytes.len();
    let mut m = Image(bytes);
    let mut scratch = vec![0u8; size];
    if !save::medium_fault(&mut m, fault, &mut scratch) {
        return Err("no valid save on the medium to damage".into());
    }
    std::fs::write(path, &m.0).map_err(|e| e.to_string())?;
    Ok(format!(
        "{mode} {slot}: done ({}, EEPROM image)",
        path.display()
    ))
}

/// An EEPROM's size (512 B or 2 KB) and not one whole slot file that size.
fn eeprom_sized(bytes: &[u8]) -> bool {
    (bytes.len() == 512 || bytes.len() == 2048) && save::parse_slot(bytes).is_err()
}

/// Flip a bit at `at` (default: the middle) or cut the file at `at`
/// (default: half).
fn raw(p: &Path, mode: &str, at: Option<usize>) -> Result<String, String> {
    let mut b = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    if b.is_empty() {
        return Err(format!("{} is empty", p.display()));
    }
    match mode {
        "flip" => {
            let i = at.unwrap_or(b.len() / 2).min(b.len() - 1);
            b[i] ^= 0x10;
            std::fs::write(p, &b).map_err(|e| e.to_string())?;
            Ok(format!("flipped bit 4 of byte {i} in {}", p.display()))
        }
        _ => {
            let n = at.unwrap_or(b.len() / 2).min(b.len());
            b.truncate(n);
            std::fs::write(p, &b).map_err(|e| e.to_string())?;
            Ok(format!("cut {} to {n} bytes", p.display()))
        }
    }
}
