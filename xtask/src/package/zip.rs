//! A zip writer for packages (deflate, UTF-8 names, Unix permissions in
//! the external attributes so an executable stays executable when
//! extracted on Linux or macOS). Fixed timestamps (1980-01-01): the same
//! inputs give the same bytes.

use std::io::Write;
use std::path::Path;

struct Entry {
    name: String,
    crc: u32,
    compressed: u32,
    size: u32,
    method: u16,
    mode: u32,
    offset: u32,
}

/// Zip every file under `dir` into `out`, with `prefix/` in front of the
/// names (a folder inside the archive; empty for none). Returns the file
/// count.
pub fn zip_dir(dir: &Path, prefix: &str, out: &Path) -> Result<usize, String> {
    let mut files = Vec::new();
    walk(dir, dir, &mut files)?;
    files.sort();
    let mut body: Vec<u8> = Vec::new();
    let mut entries = Vec::new();
    for rel in &files {
        let path = dir.join(rel);
        let data = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let name = if prefix.is_empty() {
            rel.clone()
        } else {
            format!("{prefix}/{rel}")
        };
        entries.push(add(&mut body, &name, &data, mode_of(&path))?);
    }
    let mut out_bytes = body;
    let cd_at = out_bytes.len() as u32;
    for e in &entries {
        central(&mut out_bytes, e);
    }
    let cd_len = out_bytes.len() as u32 - cd_at;
    out_bytes.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out_bytes.extend_from_slice(&[0, 0, 0, 0]);
    out_bytes.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out_bytes.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out_bytes.extend_from_slice(&cd_len.to_le_bytes());
    out_bytes.extend_from_slice(&cd_at.to_le_bytes());
    out_bytes.extend_from_slice(&0u16.to_le_bytes());
    std::fs::write(out, out_bytes).map_err(|e| format!("{}: {e}", out.display()))?;
    Ok(entries.len())
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<(), String> {
    for e in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let p = e.map_err(|e| e.to_string())?.path();
        if p.is_dir() {
            walk(root, &p, out)?;
        } else {
            let rel = p.strip_prefix(root).unwrap();
            let rel: Vec<String> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            out.push(rel.join("/"));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn mode_of(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map_or(0o644, |m| m.permissions().mode() & 0o777)
}

/// Windows has no execute bit: executables (and the scripts a package
/// starts) are marked executable by name.
#[cfg(not(unix))]
fn mode_of(p: &Path) -> u32 {
    let exec = p.extension().is_none()
        || p.extension()
            .is_some_and(|e| e == "sh" || e == "exe" || e == "AppImage");
    if exec { 0o755 } else { 0o644 }
}

fn add(body: &mut Vec<u8>, name: &str, data: &[u8], mode: u32) -> Result<Entry, String> {
    let mut crc = flate2::Crc::new();
    crc.update(data);
    let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(data).map_err(|e| e.to_string())?;
    let deflated = enc.finish().map_err(|e| e.to_string())?;
    let (method, stored) = if deflated.len() < data.len() {
        (8u16, deflated)
    } else {
        (0u16, data.to_vec())
    };
    if body.len() > u32::MAX as usize || stored.len() > u32::MAX as usize {
        return Err(format!("{name}: past 4 GB (zip64 is not written)"));
    }
    let e = Entry {
        name: name.to_string(),
        crc: crc.sum(),
        compressed: stored.len() as u32,
        size: data.len() as u32,
        method,
        mode,
        offset: body.len() as u32,
    };
    body.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
    body.extend_from_slice(&20u16.to_le_bytes());
    body.extend_from_slice(&0x0800u16.to_le_bytes());
    body.extend_from_slice(&e.method.to_le_bytes());
    body.extend_from_slice(&0u16.to_le_bytes()); // time
    body.extend_from_slice(&0x0021u16.to_le_bytes()); // 1980-01-01
    body.extend_from_slice(&e.crc.to_le_bytes());
    body.extend_from_slice(&e.compressed.to_le_bytes());
    body.extend_from_slice(&e.size.to_le_bytes());
    body.extend_from_slice(&(name.len() as u16).to_le_bytes());
    body.extend_from_slice(&0u16.to_le_bytes());
    body.extend_from_slice(name.as_bytes());
    body.extend_from_slice(&stored);
    Ok(e)
}

fn central(out: &mut Vec<u8>, e: &Entry) {
    out.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
    out.extend_from_slice(&((3u16 << 8) | 20).to_le_bytes()); // made by Unix
    out.extend_from_slice(&20u16.to_le_bytes());
    out.extend_from_slice(&0x0800u16.to_le_bytes());
    out.extend_from_slice(&e.method.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0x0021u16.to_le_bytes());
    out.extend_from_slice(&e.crc.to_le_bytes());
    out.extend_from_slice(&e.compressed.to_le_bytes());
    out.extend_from_slice(&e.size.to_le_bytes());
    out.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]); // extra, comment, disk, internal
    out.extend_from_slice(&((0o100000 | e.mode) << 16).to_le_bytes());
    out.extend_from_slice(&e.offset.to_le_bytes());
    out.extend_from_slice(e.name.as_bytes());
}

/// The names, sizes and Unix modes in a zip this module wrote, with each
/// file's bytes (tests read archives back with it).
#[cfg(test)]
pub fn read(zip: &[u8]) -> Result<Vec<(String, u32, Vec<u8>)>, String> {
    use std::io::Read;
    let rd16 = |i: usize| u16::from_le_bytes([zip[i], zip[i + 1]]) as usize;
    let rd32 = |i: usize| u32::from_le_bytes(zip[i..i + 4].try_into().unwrap()) as usize;
    let eocd = zip.len().checked_sub(22).ok_or("too short")?;
    if rd32(eocd) != 0x0605_4b50 {
        return Err("no end record".into());
    }
    let (n, mut at) = (rd16(eocd + 10), rd32(eocd + 16));
    let mut out = Vec::new();
    for _ in 0..n {
        if rd32(at) != 0x0201_4b50 {
            return Err("bad central entry".into());
        }
        let method = rd16(at + 10);
        let crc = rd32(at + 16) as u32;
        let csize = rd32(at + 20);
        let name_len = rd16(at + 28);
        let mode = (rd32(at + 38) >> 16) as u32 & 0o777;
        let local = rd32(at + 42);
        let name = String::from_utf8_lossy(&zip[at + 46..at + 46 + name_len]).into_owned();
        let data_at = local + 30 + rd16(local + 26) + rd16(local + 28);
        let raw = &zip[data_at..data_at + csize];
        let data = if method == 8 {
            let mut d = Vec::new();
            flate2::read::DeflateDecoder::new(raw)
                .read_to_end(&mut d)
                .map_err(|e| e.to_string())?;
            d
        } else {
            raw.to_vec()
        };
        let mut c = flate2::Crc::new();
        c.update(&data);
        if c.sum() != crc {
            return Err(format!("{name}: crc mismatch"));
        }
        out.push((name, mode, data));
        at += 46 + name_len;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_names_modes_and_bytes() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../target/test-projects/zip-writer");
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src");
        std::fs::create_dir_all(src.join("assets/pc")).unwrap();
        std::fs::write(src.join("game"), b"#!/bin/sh\necho hi\n").unwrap();
        let big = vec![7u8; 10_000];
        std::fs::write(src.join("assets/pc/blob.bin"), &big).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(src.join("game"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        let out = dir.join("x.zip");
        assert_eq!(zip_dir(&src, "Game", &out).unwrap(), 2);
        let bytes = std::fs::read(&out).unwrap();
        let back = read(&bytes).unwrap();
        assert_eq!(back[0].0, "Game/assets/pc/blob.bin");
        assert_eq!(back[0].2, big);
        assert!(bytes.len() < 1000, "deflated");
        assert_eq!(back[1].0, "Game/game");
        assert_eq!(back[1].1, 0o755, "executable stays executable");
        // Same inputs, same bytes.
        zip_dir(&src, "Game", &dir.join("y.zip")).unwrap();
        assert_eq!(bytes, std::fs::read(dir.join("y.zip")).unwrap());
    }
}
