//! Asset groups in the bake (issue #56, ADR 0022): which group each asset is
//! in, how each group is compressed per platform, and the `TBND` bundle
//! builder every platform's bake uses.
//!
//! ```toml
//! [groups.boot]                    # optional: boot is implicit
//! compression = { n64 = 1, 3ds = "lz4" }
//! [groups.level1]                  # loaded on demand
//! compression = { n64 = 2 }        # the others take the default
//!
//! [models.boar]
//! file = "models/boar.glb"
//! groups = ["level1", "level2"]    # default: ["boot"]
//! ```
//!
//! - `boot` stays what every earlier build was: loaded at boot, listed in
//!   the per-file index (`index.tsv` / `index.toml`), uncompressed unless
//!   `[groups.boot]` asks. A project without groups bakes byte for byte as
//!   before.
//! - Any other group becomes `groups/<name>.tbnd` (`trino_core::bundle`).
//!   An asset also in `boot` is resident from boot and stays out of the
//!   other bundles.
//! - Compression: the N64 takes libdragon's `mkasset` levels (0 none, 1 LZ4,
//!   2 aPLib, 3 Shrinkler; the pinned libdragon's `asset_load` decodes all
//!   three, `asset_fopen` streams 1 and 2); the 3DS, the PC and the Web
//!   `"lz4"` (`crate::lz4`) or `"none"`. Groups other than `boot` default to
//!   N64 level 1 and LZ4 elsewhere; `boot` defaults to none. `boot` is
//!   compressed on the consoles only (the N64 in place, the 3DS packed into
//!   `groups/boot.tbnd`): the PC's and the Web's boot files stay as they
//!   were, with no ROM to fit.

use std::collections::BTreeMap;

use serde::Deserialize;
use trino_core::bundle::{self, Codec, Entry, EntryKind, FLAG_EXTERNAL};

use crate::manifest::{Manifest, PerPlatform, Platform};
use trino_scene::{Library, Scene, Target, resolve_for};

pub const BOOT: &str = "boot";

/// `[groups.<name>]` in the manifest.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupDecl {
    /// Per platform (`{ n64 = 2, 3ds = "lz4" }`) or one value for all.
    #[serde(default)]
    pub compression: Option<PerPlatform<CompressionValue>>,
}

/// A manifest compression value: an N64 level or a codec name.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum CompressionValue {
    Level(i64),
    Name(String),
}

/// How one group's bytes are stored on one platform.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Compression {
    None,
    /// The 3DS, the PC and the Web: LZ4 blocks decoded by `trino_core::lz4`.
    Lz4,
    /// The N64: libdragon's `mkasset -c <level>` (1..=3).
    Libdragon(u8),
}

impl Compression {
    /// `none`, `lz4`, `n64 level 2` (reports).
    pub fn label(self) -> String {
        match self {
            Compression::None => "none".into(),
            Compression::Lz4 => "lz4".into(),
            Compression::Libdragon(l) => format!("libdragon level {l}"),
        }
    }

    /// The `TBND` codec of an inline entry compressed this way.
    pub fn codec(self) -> Codec {
        match self {
            Compression::None => Codec::Stored,
            Compression::Lz4 => Codec::Lz4,
            Compression::Libdragon(_) => Codec::Libdragon,
        }
    }

    /// `mkasset`'s level (0 = leave the file alone).
    pub fn n64_level(self) -> u8 {
        match self {
            Compression::Libdragon(l) => l,
            _ => 0,
        }
    }

    fn parse(value: &CompressionValue, platform: Platform) -> Result<Compression, String> {
        match (platform, value) {
            (Platform::N64, CompressionValue::Level(0)) => Ok(Compression::None),
            (Platform::N64, CompressionValue::Level(l @ 1..=3)) => {
                Ok(Compression::Libdragon(*l as u8))
            }
            (Platform::N64, CompressionValue::Name(n)) if n == "none" => Ok(Compression::None),
            (Platform::N64, v) => Err(format!(
                "n64 compression `{}`: a libdragon level 0..=3 (1 LZ4, 2 aPLib, 3 Shrinkler) or \"none\"",
                show(v)
            )),
            (_, CompressionValue::Name(n)) if n == "none" => Ok(Compression::None),
            (_, CompressionValue::Name(n)) if n == "lz4" => Ok(Compression::Lz4),
            (p, v) => Err(format!(
                "{} compression `{}`: \"lz4\" or \"none\"",
                p.key(),
                show(v)
            )),
        }
    }
}

fn show(v: &CompressionValue) -> String {
    match v {
        CompressionValue::Level(l) => l.to_string(),
        CompressionValue::Name(n) => n.clone(),
    }
}

impl Manifest {
    /// The groups `logical` is in (`["boot"]` when it names none).
    pub fn groups_of(&self, logical: &str) -> Vec<String> {
        let list = self.declared_groups(logical);
        if list.is_empty() {
            vec![BOOT.to_string()]
        } else {
            list.to_vec()
        }
    }

    /// Resident from boot: in no group, or in `boot` among others.
    pub fn in_boot(&self, logical: &str) -> bool {
        let list = self.declared_groups(logical);
        list.is_empty() || list.iter().any(|g| g == BOOT)
    }

    /// The non-boot groups whose bundle holds `logical` (empty for a boot
    /// asset: it is resident already).
    pub fn bundles_of(&self, logical: &str) -> Vec<String> {
        if self.in_boot(logical) {
            return Vec::new();
        }
        self.declared_groups(logical).to_vec()
    }

    fn declared_groups(&self, logical: &str) -> &[String] {
        let Some((section, name)) = logical.split_once('/') else {
            return &[];
        };
        let list = match section {
            "sprites" => self.sprites.get(name).map(|d| &d.groups),
            "sounds" => self.sounds.get(name).map(|d| &d.groups),
            "music" => self.music.get(name).map(|d| &d.groups),
            "models" => self.models.get(name).map(|d| &d.groups),
            "fonts" => self.fonts.get(name).map(|d| &d.groups),
            "blobs" => self.blobs.get(name).map(|d| &d.groups),
            "scenes" => self.scenes.get(name).map(|d| &d.groups),
            "particles" => self.particles.get(name).map(|d| &d.groups),
            "luts" => self.luts.get(name).map(|d| &d.groups),
            _ => None,
        };
        list.map_or(&[], |v| v.as_slice())
    }

    /// Every group the build has: `boot` first, then the declared ones (also
    /// those only named by assets, which [`Manifest::load`] rejects).
    pub fn group_names(&self) -> Vec<String> {
        let mut out = vec![BOOT.to_string()];
        for name in self.groups.keys() {
            if name != BOOT {
                out.push(name.clone());
            }
        }
        out
    }

    /// The logical paths of every asset of every section, in bake order.
    pub fn logicals(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut add = |prefix: &str, names: Vec<&String>| {
            out.extend(names.into_iter().map(|n| format!("{prefix}/{n}")))
        };
        add("sprites", self.sprites.keys().collect());
        add("sounds", self.sounds.keys().collect());
        add("music", self.music.keys().collect());
        add("models", self.models.keys().collect());
        add("fonts", self.fonts.keys().collect());
        add("blobs", self.blobs.keys().collect());
        add("particles", self.particles.keys().collect());
        add("scenes", self.scenes.keys().collect());
        add("luts", self.luts.keys().collect());
        out
    }

    /// How `group` is compressed on `platform` (`boot`: only on the
    /// consoles, see the module docs).
    pub fn compression(&self, group: &str, platform: Platform) -> Result<Compression, String> {
        let declared = self
            .groups
            .get(group)
            .and_then(|g| g.compression.as_ref())
            .and_then(|c| c.get(platform));
        match declared {
            Some(v) => {
                let c = Compression::parse(&v, platform)
                    .map_err(|e| format!("group `{group}`: {e}"))?;
                let console = matches!(platform, Platform::N64 | Platform::N3ds);
                Ok(if group == BOOT && !console {
                    Compression::None
                } else {
                    c
                })
            }
            None if group == BOOT => Ok(Compression::None),
            None if platform == Platform::N64 => Ok(Compression::Libdragon(1)),
            None => Ok(Compression::Lz4),
        }
    }

    /// The group checks of [`Manifest::load`]: names, every group an asset
    /// names declared, every compression value valid.
    pub(crate) fn check_groups(&self) -> Result<(), String> {
        for (name, decl) in &self.groups {
            if !crate::manifest::edit::valid_name(name) {
                return Err(format!(
                    "group `{name}`: names are lowercase letters, digits, `_` and `-`"
                ));
            }
            if let Some(k) = decl
                .compression
                .as_ref()
                .and_then(|c| c.unknown_keys().pop())
            {
                return Err(format!(
                    "group `{name}`: compression: unknown platform `{k}`"
                ));
            }
            for p in Platform::ALL {
                self.compression(name, p)?;
            }
        }
        for logical in self.logicals() {
            for g in self.declared_groups(&logical) {
                if g != BOOT && !self.groups.contains_key(g) {
                    return Err(format!(
                        "`{logical}`: group `{g}` is not declared (add `[groups.{g}]`)"
                    ));
                }
            }
        }
        if !self
            .declared_groups(trino_core::MixerConfig::LOGICAL)
            .is_empty()
        {
            return Err("audio/mixer is always in boot".into());
        }
        Ok(())
    }
}

/// Every group a scene can hold at once: `requires` and its cells'.
pub fn scene_groups(scene: &Scene) -> Vec<String> {
    let mut out: Vec<String> = scene.requires.clone();
    for (_, groups) in scene.cell_groups() {
        for g in groups {
            if !out.contains(&g) {
                out.push(g);
            }
        }
    }
    out
}

/// The assets `scene` draws on some platform, each with the cell it sits
/// in (`NO_CELL` outside every cell): models, LOD levels and sprites as
/// every target resolves them (prefabs and variants included), and the
/// scene's sounds.
pub fn scene_assets(scene: &Scene, lib: &Library) -> Vec<(String, u8)> {
    let mut out: Vec<(String, u8)> = Vec::new();
    let mut add = |path: &str, cell: u8| {
        if !out.iter().any(|(p, c)| p == path && *c == cell) {
            out.push((path.to_string(), cell));
        }
    };
    for target in Target::EVERY {
        for item in resolve_for(scene, lib, 0.0, target).items {
            if let Some((_, p)) = &item.model {
                add(p, item.cell);
            }
            for l in &item.lod {
                if let trino_scene::LodDraw::Model(_, p) = &l.draw {
                    add(p, item.cell);
                }
            }
            if let Some(s) = &item.sprite {
                add(&s.path, item.cell);
            }
        }
    }
    // Sounds by path from the file (a resolved source keeps the id only),
    // with the cell their entity's resolve put them in.
    let items = resolve_for(scene, lib, 0.0, Target::Pc).items;
    for path in scene.asset_refs() {
        if path.starts_with("sounds/") {
            let id = trino_core::SoundId::from_path(&path);
            let cells: Vec<u8> = items
                .iter()
                .filter(|i| i.sound.is_some_and(|s| s.sound == id))
                .map(|i| i.cell)
                .collect();
            for c in if cells.is_empty() {
                vec![trino_core::NO_CELL]
            } else {
                cells
            } {
                add(&path, c);
            }
        }
    }
    out
}

/// What `cargo xtask assets check` and the editor's scene settings say
/// about `scene`'s groups: a group in `requires` or a cell that the
/// manifest does not declare, and an asset the scene draws (on any
/// platform, prefabs and variants included) that is in no group the scene
/// loads where it sits — `requires`, or its cell's groups — so it would
/// not draw. The RAM those groups take is [`crate::memory::MemoryPlan::check`]'s.
pub fn scene_problems(scene: &Scene, lib: &Library, manifest: &Manifest) -> Vec<String> {
    let mut out = Vec::new();
    let declared = manifest.group_names();
    let cells = resolve_for(scene, lib, 0.0, Target::Pc).cell_groups;
    let mut named: Vec<(String, String)> = scene
        .requires
        .iter()
        .map(|g| (g.clone(), "`requires`".to_string()))
        .collect();
    for (cell, groups) in scene.cell_groups() {
        named.extend(groups.into_iter().map(|g| (g, format!("cell `{cell}`"))));
    }
    for (g, by) in &named {
        if !declared.contains(g) {
            out.push(format!(
                "{by} names group `{g}`, which the manifest does not declare \
                 (add `[groups.{g}]`, or `cargo xtask assets group <asset> {g}`)"
            ));
        }
    }
    for (path, cell) in scene_assets(scene, lib) {
        // Not a manifest asset (an impostor, a missing file): other checks'.
        if !manifest.logicals().contains(&path) || manifest.in_boot(&path) {
            continue;
        }
        let groups = manifest.groups_of(&path);
        let cell_groups = cells.get(cell as usize).map_or(&[][..], Vec::as_slice);
        if groups
            .iter()
            .any(|g| scene.requires.contains(g) || cell_groups.contains(g))
        {
            continue;
        }
        let place = if cell_groups.is_empty() {
            String::new()
        } else {
            format!(" or cell {}'s {:?}", cell, cell_groups)
        };
        out.push(format!(
            "draws `{path}` (groups {groups:?}), loaded by none of the scene's \
             groups ({:?}{place}): it would not draw. Add one of its groups to `requires`, \
             or put it in one (`cargo xtask assets group {path} {}`)",
            scene.requires,
            scene.requires.last().map_or("boot", String::as_str),
        ));
    }
    out
}

/// The index tier column as a `TBND` entry's tier byte.
pub fn tier_byte(tier: Option<trino_core::HwTier>) -> u8 {
    match tier {
        None => 0,
        Some(trino_core::HwTier::Base) => 1,
        Some(trino_core::HwTier::Expanded) => 2,
    }
}

/// One group's bundle being baked.
pub struct BundleBuilder {
    group: String,
    compression: Compression,
    entries: Vec<Entry>,
    strings: Vec<u8>,
    payload: Vec<u8>,
    /// (hash, raw length) → (codec, offset, stored): one payload per
    /// distinct bytes.
    dedup: BTreeMap<(u32, u32), (Codec, u32, u32)>,
    raw_total: u64,
}

impl BundleBuilder {
    pub fn new(group: &str, compression: Compression) -> Self {
        BundleBuilder {
            group: group.to_string(),
            compression,
            entries: Vec::new(),
            strings: Vec::new(),
            payload: Vec::new(),
            dedup: BTreeMap::new(),
            raw_total: 0,
        }
    }

    pub fn group(&self) -> &str {
        &self.group
    }

    pub fn compression(&self) -> Compression {
        self.compression
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// An entry whose bytes go in the bundle, compressed as the group says
    /// (stored when LZ4 wouldn't shrink them, or on the N64, whose bundles
    /// hold files).
    pub fn add_inline(
        &mut self,
        kind: EntryKind,
        id: u32,
        tier: Option<trino_core::HwTier>,
        raw: &[u8],
    ) {
        let hash = bundle::hash(raw);
        let key = (hash, raw.len() as u32);
        let (codec, offset, stored) = match self.dedup.get(&key) {
            Some(&v) => v,
            None => {
                let packed = (self.compression == Compression::Lz4)
                    .then(|| crate::lz4::compress(raw))
                    .filter(|p| p.len() < raw.len());
                let (codec, bytes) = match packed {
                    Some(p) => (Codec::Lz4, p),
                    None => (Codec::Stored, raw.to_vec()),
                };
                let v = (codec, self.payload.len() as u32, bytes.len() as u32);
                self.payload.extend_from_slice(&bytes);
                // Keep payloads 4-byte aligned for the readers.
                while !self.payload.len().is_multiple_of(4) {
                    self.payload.push(0);
                }
                self.dedup.insert(key, v);
                v
            }
        };
        self.push(
            kind,
            codec,
            0,
            tier,
            id,
            offset,
            stored,
            raw.len() as u32,
            hash,
        );
    }

    /// An entry that is a file beside the bundle (`path` from the bundle's
    /// root: the DFS or the RomFS): `raw` bytes once loaded, `hash` of them
    /// (0: not checked; the console's own loader reads the file), the file
    /// stored as `codec` says (`Libdragon`: `mkasset` compressed it).
    #[allow(clippy::too_many_arguments)]
    pub fn add_external(
        &mut self,
        kind: EntryKind,
        id: u32,
        tier: Option<trino_core::HwTier>,
        path: &str,
        raw: u32,
        hash: u32,
        codec: Codec,
    ) {
        let offset = self.strings.len() as u32;
        self.strings.extend_from_slice(path.as_bytes());
        self.push(
            kind,
            codec,
            FLAG_EXTERNAL,
            tier,
            id,
            offset,
            path.len() as u32,
            raw,
            hash,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        kind: EntryKind,
        codec: Codec,
        flags: u8,
        tier: Option<trino_core::HwTier>,
        id: u32,
        offset: u32,
        stored: u32,
        raw: u32,
        hash: u32,
    ) {
        self.raw_total += raw as u64;
        self.entries.push(Entry {
            kind,
            codec,
            flags,
            tier: tier_byte(tier),
            id,
            offset,
            stored,
            raw,
            hash,
        });
    }

    /// Bytes the entries take once loaded (every tier's, summed).
    pub fn raw_total(&self) -> u64 {
        self.raw_total
    }

    /// The bundle file.
    pub fn finish(&self) -> Vec<u8> {
        let mut table =
            vec![
                0u8;
                bundle::HEADER_LEN + self.entries.len() * bundle::ENTRY_LEN + self.strings.len()
            ];
        let n = bundle::write_table(
            bundle::group_id(&self.group),
            &self.entries,
            &self.strings,
            self.payload.len() as u32,
            &mut table,
        )
        .expect("sized above");
        table.truncate(n);
        table.extend_from_slice(&self.payload);
        table
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(text: &str) -> Result<Manifest, String> {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("manifest.toml");
        std::fs::write(&p, text).unwrap();
        Manifest::load(&p)
    }

    #[test]
    fn membership_and_defaults() {
        let m = manifest(
            "version = 1\n[groups.level1]\n[groups.level2]\ncompression = { n64 = 2, 3ds = \"none\" }\n\
             [models.a]\nfile = \"a.glb\"\n\
             [models.b]\nfile = \"b.glb\"\ngroups = [\"level1\", \"level2\"]\n\
             [models.c]\nfile = \"c.glb\"\ngroups = [\"boot\", \"level1\"]\n",
        )
        .unwrap();
        assert_eq!(m.groups_of("models/a"), ["boot"]);
        assert!(m.in_boot("models/a") && m.in_boot("models/c") && !m.in_boot("models/b"));
        assert_eq!(m.bundles_of("models/b"), ["level1", "level2"]);
        assert!(m.bundles_of("models/c").is_empty());
        assert_eq!(m.group_names(), ["boot", "level1", "level2"]);
        use Compression::*;
        assert_eq!(m.compression("boot", Platform::N64), Ok(None));
        assert_eq!(m.compression("level1", Platform::N64), Ok(Libdragon(1)));
        assert_eq!(m.compression("level1", Platform::Pc), Ok(Lz4));
        assert_eq!(m.compression("level2", Platform::N64), Ok(Libdragon(2)));
        assert_eq!(m.compression("level2", Platform::N3ds), Ok(None));
        assert_eq!(m.compression("level2", Platform::Web), Ok(Lz4));

        // Boot compresses on the consoles only.
        let m = manifest(
            "version = 1\n[groups.boot]\ncompression = { n64 = 2, 3ds = \"lz4\", pc = \"lz4\" }\n",
        )
        .unwrap();
        assert_eq!(m.compression("boot", Platform::N64), Ok(Libdragon(2)));
        assert_eq!(m.compression("boot", Platform::N3ds), Ok(Lz4));
        assert_eq!(m.compression("boot", Platform::Pc), Ok(None));
        assert_eq!(m.compression("boot", Platform::Web), Ok(None));
    }

    #[test]
    fn bad_groups_are_load_errors() {
        let undeclared =
            manifest("version = 1\n[models.a]\nfile = \"a.glb\"\ngroups = [\"level9\"]\n");
        assert!(undeclared.unwrap_err().contains("`level9` is not declared"));
        let bad = manifest("version = 1\n[groups.l]\ncompression = { n64 = 7 }\n");
        assert!(bad.unwrap_err().contains("libdragon level"));
        let bad = manifest("version = 1\n[groups.l]\ncompression = { 3ds = 2 }\n");
        assert!(bad.unwrap_err().contains("\"lz4\" or \"none\""));
        let bad = manifest("version = 1\n[groups.L]\n");
        assert!(bad.unwrap_err().contains("lowercase"));
        let bad = manifest("version = 1\n[groups.l]\ncompression = { gc = \"lz4\" }\n");
        assert!(bad.unwrap_err().contains("unknown platform `gc`"));
    }

    #[test]
    fn bundles_round_trip_and_dedup() {
        let mut b = BundleBuilder::new("level1", Compression::Lz4);
        let model = b"model bytes model bytes model bytes model bytes".to_vec();
        b.add_inline(EntryKind::Model, 1, None, &model);
        b.add_inline(EntryKind::Model, 2, None, &model);
        b.add_inline(
            EntryKind::Blob,
            3,
            Some(trino_core::HwTier::Expanded),
            b"xy",
        );
        b.add_external(
            EntryKind::Sound,
            4,
            None,
            "00000004.t3au",
            999,
            7,
            Codec::Stored,
        );
        let bytes = b.finish();
        let parsed = trino_core::bundle::Bundle::parse(&bytes).unwrap();
        assert_eq!(parsed.group(), trino_core::bundle::group_id("level1"));
        let es: Vec<_> = parsed.entries().collect();
        assert_eq!(es[0].offset, es[1].offset, "same bytes, one payload");
        assert_eq!(es[0].codec, Codec::Lz4);
        assert_eq!(es[2].codec, Codec::Stored, "too small to shrink");
        assert_eq!(es[2].tier, 2);
        for e in &es[..3] {
            let mut out = vec![0u8; e.raw as usize];
            trino_core::bundle::decode(e, parsed.stored(e).unwrap(), &mut out).unwrap();
            assert_eq!(trino_core::bundle::hash(&out), e.hash);
        }
        assert_eq!(parsed.path(&es[3]), Some("00000004.t3au"));
        assert_eq!(b.raw_total(), 2 * model.len() as u64 + 2 + 999);
    }
}
