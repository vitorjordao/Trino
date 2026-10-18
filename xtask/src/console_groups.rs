//! Asset groups in the console bakes (issue #56, ADR 0022): the N64's and the
//! 3DS's `index.tsv` keep the `boot` group, line for line what they always
//! were; every other group's lines become its bundle,
//! `groups/<name>.tbnd` (`trino_core::bundle`), named in the index by a
//! `<group id> \t group \t groups/<name>.tbnd` line the runtime registers.
//!
//! - **N64**: the bundle lists files (external entries: the DFS file and,
//!   for sounds, the codec and bus columns). libdragon's `asset_load` and
//!   `sprite_load` decompress what `mkasset` compressed, so the files are
//!   compressed in place at their group's level (a `wav64` streams and stays
//!   as `audioconv64` made it). `[groups.boot] compression` compresses the
//!   boot files the same way.
//! - **3DS**: textures, meshes, fonts and blobs go into the bundle, LZ4
//!   blocks decoded on the worker core; sounds (`T3AU`, some streamed) stay
//!   files beside it. `[groups.boot] compression = { 3ds = "lz4" }` packs
//!   the boot files the same way, into `groups/boot.tbnd`, which the index
//!   names on a `<id> \t boot \t groups/boot.tbnd` line the runtime decodes
//!   at boot (sounds stay loose files on their own lines).
//!
//! A project without groups writes the same index as before.

use std::collections::BTreeMap;
use std::path::Path;

use trino_asset_pipeline::{BundleBuilder, Compression, Manifest, Platform};
use trino_core::bundle::{Codec, EntryKind};

/// One index line of a non-boot asset.
#[derive(Clone, Debug)]
struct Line {
    kind: EntryKind,
    id: u32,
    tier: Option<trino_core::HwTier>,
    file: String,
    /// The columns after the tier (sounds: codec and bus), tab-joined.
    extra: String,
}

impl Line {
    fn parse(line: &str) -> Option<Line> {
        let mut cols = line.trim_end_matches('\n').split('\t');
        let id = u32::from_str_radix(cols.next()?, 16).ok()?;
        let kind = EntryKind::parse(cols.next()?)?;
        let file = cols.next()?.to_string();
        let tier = match cols.next().unwrap_or("") {
            "" => None,
            "base" => Some(trino_core::HwTier::Base),
            "expanded" => Some(trino_core::HwTier::Expanded),
            _ => return None,
        };
        let extra = cols.collect::<Vec<_>>().join("\t");
        Some(Line {
            kind,
            id,
            tier,
            file,
            extra,
        })
    }

    /// The external entry's locator: the file, then its extra columns.
    fn locator(&self) -> String {
        if self.extra.is_empty() {
            self.file.clone()
        } else {
            format!("{}\t{}", self.file, self.extra)
        }
    }
}

/// The groups of one console bake (see the module docs).
pub struct GroupIndex {
    platform: Platform,
    /// Group → its lines, in bake order.
    groups: BTreeMap<String, Vec<Line>>,
    /// Every file and the first group that holds it (its compression).
    file_group: BTreeMap<String, String>,
    boot_files: Vec<String>,
    /// 3DS, `[groups.boot]` LZ4: the boot lines packed into `boot.tbnd`.
    boot_bundle: Vec<Line>,
}

impl GroupIndex {
    pub fn new(platform: Platform) -> Self {
        GroupIndex {
            platform,
            groups: BTreeMap::new(),
            file_group: BTreeMap::new(),
            boot_files: Vec::new(),
            boot_bundle: Vec::new(),
        }
    }

    /// Whether boot's files go into `boot.tbnd` (the 3DS asked for LZ4;
    /// the N64 compresses them in place, `n64_compress_script`).
    fn packs_boot(&self, manifest: &Manifest) -> Result<bool, String> {
        Ok(self.platform == Platform::N3ds
            && manifest.compression(trino_asset_pipeline::groups::BOOT, self.platform)?
                == Compression::Lz4)
    }

    /// Send `logical`'s index `lines` to boot (`index`) or to its groups'
    /// bundles.
    pub fn route(
        &mut self,
        manifest: &Manifest,
        logical: &str,
        lines: &str,
        index: &mut String,
    ) -> Result<(), String> {
        let groups = manifest.bundles_of(logical);
        for line in lines.lines() {
            let parsed = Line::parse(line)
                .ok_or_else(|| format!("`{logical}`: unreadable index line `{line}`"))?;
            if groups.is_empty() {
                let sound = matches!(parsed.kind, EntryKind::Sound | EntryKind::Music);
                if !sound && self.packs_boot(manifest)? {
                    self.boot_bundle.push(parsed);
                    continue;
                }
                index.push_str(line);
                index.push('\n');
                if !sound {
                    self.boot_files.push(parsed.file);
                }
                continue;
            }
            self.file_group
                .entry(parsed.file.clone())
                .or_insert_with(|| groups[0].clone());
            for g in &groups {
                self.groups
                    .entry(g.clone())
                    .or_default()
                    .push(parsed.clone());
            }
        }
        Ok(())
    }

    /// The index lines naming the bundles (`<id> \t group \t groups/<g>.tbnd`),
    /// after the packed boot bundle's (`<id> \t boot \t groups/boot.tbnd`).
    pub fn index_lines(&self) -> String {
        let boot = trino_asset_pipeline::groups::BOOT;
        let packed = (!self.boot_bundle.is_empty()).then(|| {
            format!(
                "{:08x}\tboot\tgroups/{boot}.tbnd\n",
                trino_core::bundle::group_id(boot)
            )
        });
        packed
            .into_iter()
            .chain(self.groups.keys().map(|g| {
                format!(
                    "{:08x}\tgroup\tgroups/{g}.tbnd\n",
                    trino_core::bundle::group_id(g)
                )
            }))
            .collect()
    }

    /// N64: the `mkasset` commands that compress the boot files (when
    /// `[groups.boot]` asks) and every grouped file at its group's level, in
    /// place in `dir` (the DFS, relative to the container's project root).
    /// The biggest window: everything is read whole by `asset_load`.
    pub fn n64_compress_script(&self, manifest: &Manifest, dir: &str) -> Result<String, String> {
        let mut script = String::new();
        for (level, files) in self.n64_levels(manifest)? {
            if level == 0 {
                continue;
            }
            // mksprite compressed its sprites itself (`n64_sprite_level`).
            let files: Vec<&str> = files
                .into_iter()
                .filter(|f| !f.ends_with(".sprite"))
                .collect();
            for chunk in files.chunks(64) {
                let list = chunk
                    .iter()
                    .map(|f| format!("{dir}/{f}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                script.push_str(&format!("mkasset -c {level} -w 256 -o {dir} {list}\n"));
            }
        }
        Ok(script)
    }

    /// N64: the index lines that enable libdragon's level 2 and 3
    /// decompressors (`asset_init_compression`), to go before every asset.
    pub fn n64_compression_lines(&self, manifest: &Manifest) -> Result<String, String> {
        Ok(self
            .n64_levels(manifest)?
            .keys()
            .filter(|&&l| l >= 2)
            .map(|l| format!("{l:08x}\tcompression\tlibdragon\n"))
            .collect())
    }

    /// N64: every file by the level it is compressed at (0 = left alone):
    /// `mkasset` does all but the sprites, which mksprite does.
    fn n64_levels(&self, manifest: &Manifest) -> Result<BTreeMap<u8, Vec<&str>>, String> {
        let mut by_level: BTreeMap<u8, Vec<&str>> = BTreeMap::new();
        let boot = manifest.compression(trino_asset_pipeline::groups::BOOT, Platform::N64)?;
        for f in &self.boot_files {
            by_level.entry(boot.n64_level()).or_default().push(f);
        }
        for (file, group) in &self.file_group {
            if file.ends_with(".wav64") {
                continue;
            }
            let level = manifest.compression(group, Platform::N64)?.n64_level();
            by_level.entry(level).or_default().push(file);
        }
        Ok(by_level)
    }

    /// Write `groups/<g>.tbnd` into `fsdir` (the DFS or the RomFS, already
    /// holding every file). N64: external entries only, their RAM size
    /// read from `mkasset`'s header. 3DS: everything but sounds inline (LZ4,
    /// the loose file removed), sounds external; the packed boot bundle
    /// first, as `groups/boot.tbnd`.
    pub fn write_bundles(
        &self,
        manifest: &Manifest,
        fsdir: &Path,
    ) -> Result<Vec<BundleSize>, String> {
        let out = fsdir.join("groups");
        std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        let mut sizes = Vec::new();
        let mut inlined = Vec::new();
        let boot = (!self.boot_bundle.is_empty())
            .then_some((trino_asset_pipeline::groups::BOOT, &self.boot_bundle));
        let groups = self.groups.iter().map(|(g, l)| (g.as_str(), l));
        for (group, lines) in boot.into_iter().chain(groups) {
            let compression = manifest.compression(group, self.platform)?;
            let mut b = BundleBuilder::new(group, compression);
            let mut files = 0u64;
            for l in lines {
                let path = fsdir.join(&l.file);
                let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                let external = self.platform == Platform::N64
                    || matches!(l.kind, EntryKind::Sound | EntryKind::Music);
                if external {
                    let raw = libdragon_raw_len(&bytes);
                    let codec = if bytes.starts_with(b"DCA3") {
                        Codec::Libdragon
                    } else {
                        Codec::Stored
                    };
                    b.add_external(l.kind, l.id, l.tier, &l.locator(), raw, 0, codec);
                    files += bytes.len() as u64;
                } else {
                    b.add_inline(l.kind, l.id, l.tier, &bytes);
                    inlined.push(path);
                }
            }
            let raw = b.raw_total();
            let bytes = b.finish();
            let file = out.join(format!("{group}.tbnd"));
            std::fs::write(&file, &bytes).map_err(|e| format!("{}: {e}", file.display()))?;
            sizes.push(BundleSize {
                group: group.to_string(),
                compression,
                entries: lines.len(),
                raw,
                stored: files + bytes.len() as u64,
            });
        }
        inlined.sort();
        inlined.dedup();
        for f in inlined {
            std::fs::remove_file(&f).map_err(|e| format!("{}: {e}", f.display()))?;
        }
        Ok(sizes)
    }
}

/// N64: the level `mksprite --compress` writes a sprite (or a font's atlas)
/// at: its first group's. mksprite compresses its own output, so `mkasset`
/// leaves `.sprite` files alone (twice compressed, `sprite_load` would get
/// a compressed buffer). `None` for a boot sprite with no `[groups.boot]`
/// level: mksprite's default (level 1), as before groups.
pub fn n64_sprite_level(manifest: &Manifest, logical: &str) -> Result<Option<u8>, String> {
    let groups = manifest.bundles_of(logical);
    let group = groups
        .first()
        .map_or(trino_asset_pipeline::groups::BOOT, String::as_str);
    let level = manifest.compression(group, Platform::N64)?;
    Ok(match (group == trino_asset_pipeline::groups::BOOT, level) {
        (true, Compression::None) => None,
        (_, l) => Some(l.n64_level()),
    })
}

/// `mksprite`'s flag for [`n64_sprite_level`] (with its trailing space).
pub fn mksprite_compress(level: Option<u8>) -> String {
    level.map_or_else(String::new, |l| format!("--compress {l} "))
}

/// A written bundle, for the bake's report line.
#[derive(Clone, Debug)]
pub struct BundleSize {
    pub group: String,
    pub compression: Compression,
    pub entries: usize,
    /// Bytes once loaded, and on the medium (bundle plus its files).
    pub raw: u64,
    pub stored: u64,
}

impl BundleSize {
    pub fn line(&self, platform: &str) -> String {
        format!(
            "[{platform}] group {}: {} entries, {} KB raw, {} KB stored ({})",
            self.group,
            self.entries,
            self.raw.div_ceil(1024),
            self.stored.div_ceil(1024),
            self.compression.label()
        )
    }
}

/// The size a file has once `asset_load` read it: `mkasset`'s header
/// (`DCA3`, big-endian original size at byte 12) or the file itself.
pub fn libdragon_raw_len(bytes: &[u8]) -> u32 {
    if bytes.len() >= 16 && bytes.starts_with(b"DCA3") {
        u32::from_be_bytes([bytes[12], bytes[13], bytes[14], bytes[15]])
    } else {
        bytes.len() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_lines_parse_with_tiers_and_columns() {
        let l = Line::parse("0000abcd\tsound\t0000abcd.wav64\t\topus\tmusic").unwrap();
        assert_eq!(l.kind, EntryKind::Sound);
        assert_eq!(l.tier, None);
        assert_eq!(l.locator(), "0000abcd.wav64\topus\tmusic");
        let t = Line::parse("00000001\tblob\t00000001.expanded.blob\texpanded\n").unwrap();
        assert_eq!(t.tier, Some(trino_core::HwTier::Expanded));
        assert_eq!(t.locator(), "00000001.expanded.blob");
        assert!(Line::parse("zz\tsprite\tf").is_none());
        assert!(Line::parse("00000001\tdeluxe\tf").is_none());
    }

    #[test]
    fn mkasset_headers_give_the_loaded_size() {
        let mut h =
            b"DCA3\x00\x01\x01\x02\x00\x00\x0a\x88\x00\x00\x13\x88\x00\x00\x00\x2a".to_vec();
        assert_eq!(libdragon_raw_len(&h), 5000);
        h[0] = b'X';
        assert_eq!(libdragon_raw_len(&h), h.len() as u32);
    }

    #[test]
    fn a_compressed_boot_is_packed_on_the_3ds_and_compressed_in_place_on_the_n64() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.toml");
        std::fs::write(
            &path,
            "version = 1\n[groups.boot]\ncompression = { n64 = 1, 3ds = \"lz4\" }\n\
             [models.a]\nfile = \"a.glb\"\n[sounds.s]\nfile = \"s.wav\"\n",
        )
        .unwrap();
        let m = Manifest::load(&path).unwrap();
        let model = "0000000a\tmodel\t0000000a.tmdl\n";
        let sound = "0000000b\tsound\t0000000b.t3au\n";
        let fs = dir.path().join("romfs");
        std::fs::create_dir_all(&fs).unwrap();
        std::fs::write(fs.join("0000000a.tmdl"), vec![9u8; 4096]).unwrap();
        std::fs::write(fs.join("0000000b.t3au"), b"T3AU").unwrap();

        // The 3DS: the model goes into boot.tbnd, the sound stays a line.
        let mut g = GroupIndex::new(Platform::N3ds);
        let mut index = String::new();
        g.route(&m, "models/a", model, &mut index).unwrap();
        g.route(&m, "sounds/s", sound, &mut index).unwrap();
        assert_eq!(index, sound);
        assert_eq!(
            g.index_lines(),
            format!(
                "{:08x}\tboot\tgroups/boot.tbnd\n",
                trino_core::bundle::group_id("boot")
            )
        );
        let sizes = g.write_bundles(&m, &fs).unwrap();
        assert_eq!(sizes.len(), 1);
        assert_eq!(sizes[0].group, "boot");
        assert!(sizes[0].stored < 4096, "{}", sizes[0].stored);
        assert!(!fs.join("0000000a.tmdl").exists());
        assert!(fs.join("0000000b.t3au").exists());
        let bytes = std::fs::read(fs.join("groups/boot.tbnd")).unwrap();
        let b = trino_core::bundle::Bundle::parse(&bytes).unwrap();
        assert_eq!(b.len(), 1);
        let e = b.entry(0);
        assert_eq!((e.kind, e.id, e.raw), (EntryKind::Model, 0xa, 4096));
        assert_eq!(e.codec, Codec::Lz4);
        let mut out = vec![0; 4096];
        trino_core::bundle::decode(&e, b.stored(&e).unwrap(), &mut out).unwrap();
        assert_eq!(out, vec![9u8; 4096]);

        // The N64 keeps the index and compresses the file where it is.
        let mut g = GroupIndex::new(Platform::N64);
        let mut index = String::new();
        g.route(&m, "models/a", model, &mut index).unwrap();
        assert_eq!(index, model);
        assert!(g.index_lines().is_empty());
        let script = g.n64_compress_script(&m, "fs").unwrap();
        assert!(
            script.contains("mkasset -c 1 -w 256 -o fs fs/0000000a.tmdl"),
            "{script}"
        );
    }
}
