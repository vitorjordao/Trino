//! What a project's assets take of a console's RAM (issue #56, ADR 0022):
//! per asset and per group, the numbers strict mode, `cargo xtask assets
//! report` and the editor's Memory tab compare with `Caps::ram_bytes`.
//!
//! Each asset counts what its platform holds once it is loaded:
//! - sprites and font atlases: the texture as that platform stores it
//!   (`texture::report`'s memory bytes: N64 texels and palette, 3DS linear
//!   memory, PC VRAM, the Web's transcoded size);
//! - sounds and music: their resident bytes (`audio::Footprint`'s rule: an
//!   N64 wav64 streams from the cartridge, a 3DS stream keeps no track);
//! - models, scenes, particle effects, blobs, font tables and the mix: their
//!   baked bytes (portable formats: the PC's file is the console's).
//!
//! The rest of the RAM is not the assets': the engine's code, framebuffers,
//! audio buffers and stack (the platform's `reserve`) and the game's own
//! state (`game`). Both come from `[memory]` in the manifest, with defaults
//! measured in ares and Azahar (the `TRINO_MEM` boot line):
//!
//! ```toml
//! [memory]
//! reserve = { n64 = "2.2M" }   # per platform, or one value
//! game = "256K"
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use trino_core::HwTier;
use trino_core::bundle::MemoryCategory;

use crate::manifest::{Manifest, PerPlatform, Platform};

/// `[memory]` in the manifest.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryDecl {
    /// RAM the engine holds on each platform before any asset loads.
    #[serde(default)]
    pub reserve: Option<PerPlatform<Size>>,
    /// RAM the game's own state needs (its heap use besides the assets).
    #[serde(default)]
    pub game: Option<PerPlatform<Size>>,
}

/// A byte count: `1048576`, `"512K"`, `"1.5M"`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Size {
    Bytes(u64),
    Text(String),
}

impl Size {
    pub fn bytes(&self) -> Result<u64, String> {
        match self {
            Size::Bytes(b) => Ok(*b),
            Size::Text(t) => parse_size(t),
        }
    }
}

/// `"1.5M"`, `"512K"`, `"300000"` as bytes (K and M are binary).
pub fn parse_size(text: &str) -> Result<u64, String> {
    let t = text.trim();
    let (num, scale) = match t.as_bytes().last() {
        Some(b'K' | b'k') => (&t[..t.len() - 1], 1024.0),
        Some(b'M' | b'm') => (&t[..t.len() - 1], 1024.0 * 1024.0),
        _ => (t, 1.0),
    };
    match num.trim().parse::<f64>() {
        Ok(n) if n >= 0.0 && n.is_finite() => Ok((n * scale).round() as u64),
        _ => Err(format!(
            "`{text}`: a size in bytes, or with K or M (`512K`, `1.5M`)"
        )),
    }
}

/// `4.6 MB`, `320 KB`, `900 B` (reports and messages).
pub fn show_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{} KB", bytes.div_ceil(1024))
    } else {
        format!("{bytes} B")
    }
}

/// The engine's RAM on a console before the first asset loads: code and
/// data, framebuffers, the display list and audio buffers, the stack. Read
/// from castle64's `TRINO_MEM` boot line (`ram - heap_total +
/// before_assets`) in ares and Azahar, plus a twentieth: the N64's is
/// 2 134 KB with or without the Expansion Pak (the same code, the same
/// framebuffers); the 3DS's 5 988 KB on either model (5 184 KB of malloc
/// and linear heap before the assets, 804 KB of code and stack beside the
/// heaps).
pub fn default_reserve(platform: Platform, tier: HwTier) -> u64 {
    const K: u64 = 1024;
    match (platform, tier) {
        (Platform::N64, _) => 2240 * K,
        (Platform::N3ds, _) => 6400 * K,
        _ => 0,
    }
}

/// `N64`, `3DS`, `PC`, `Web` (messages).
pub fn platform_label(platform: Platform) -> &'static str {
    match platform {
        Platform::N64 => "N64",
        Platform::N3ds => "3DS",
        Platform::Pc => "PC",
        Platform::Web => "Web",
    }
}

/// The RAM table of `platform` on `tier` hardware.
pub fn caps(platform: Platform, tier: HwTier) -> trino_core::Caps {
    match platform {
        Platform::N64 => trino_core::Caps::N64.with_tier(tier),
        Platform::N3ds => trino_core::Caps::N3DS.with_tier(tier),
        Platform::Pc => trino_core::Caps::PC,
        Platform::Web => trino_core::Caps::WEB,
    }
}

/// One asset's RAM on the plan's platform.
#[derive(Clone, Debug, PartialEq)]
pub struct AssetMemory {
    /// `models/boar`; a font is two entries (`fonts/x` atlas and table).
    pub logical: String,
    /// `sprite`, `font`, `model`, `sound`, `music`, `blob`, `mixer`.
    pub kind: &'static str,
    pub category: MemoryCategory,
    /// Its groups (`["boot"]` when it names none).
    pub groups: Vec<String>,
    pub bytes: u64,
}

impl AssetMemory {
    /// Resident with `loaded` groups (and boot) in memory.
    pub fn resident_with(&self, loaded: &[&str]) -> bool {
        self.groups
            .iter()
            .any(|g| g == crate::groups::BOOT || loaded.contains(&g.as_str()))
    }
}

/// Every asset's RAM on one platform and hardware tier, and the RAM they
/// share.
#[derive(Clone, Debug)]
pub struct MemoryPlan {
    pub platform: Platform,
    pub tier: HwTier,
    /// `Caps::ram_bytes` (`u32::MAX`: the PC and the Web have no ceiling).
    pub ram: u64,
    pub reserve: u64,
    pub game: u64,
    /// `boot` first, then the declared groups.
    pub groups: Vec<String>,
    pub assets: Vec<AssetMemory>,
}

impl MemoryPlan {
    /// Whether the platform has a RAM ceiling (the consoles).
    pub fn limited(&self) -> bool {
        self.ram < u32::MAX as u64
    }

    /// RAM left for assets: `ram - reserve - game`.
    pub fn budget(&self) -> u64 {
        self.ram.saturating_sub(self.reserve + self.game)
    }

    /// The bytes of `group`'s assets (an asset in two groups counts in
    /// both).
    pub fn group_bytes(&self, group: &str) -> u64 {
        self.assets
            .iter()
            .filter(|a| a.groups.iter().any(|g| g == group))
            .map(|a| a.bytes)
            .sum()
    }

    /// Resident bytes with `loaded` groups and boot in memory (each asset
    /// once).
    pub fn resident(&self, loaded: &[&str]) -> u64 {
        self.resident_assets(loaded).map(|a| a.bytes).sum()
    }

    /// [`MemoryPlan::resident`] by category (meshes, textures, audio, data).
    pub fn resident_by_category(&self, loaded: &[&str]) -> [u64; 4] {
        let mut out = [0; 4];
        for a in self.resident_assets(loaded) {
            out[a.category as usize] += a.bytes;
        }
        out
    }

    fn resident_assets<'a>(&'a self, loaded: &'a [&str]) -> impl Iterator<Item = &'a AssetMemory> {
        self.assets.iter().filter(move |a| a.resident_with(loaded))
    }

    /// Strict mode's check: `loaded` and boot fit the RAM left for assets,
    /// or a message naming the groups, the overrun and the biggest assets.
    pub fn check(&self, loaded: &[&str]) -> Result<(), String> {
        if !self.limited() {
            return Ok(());
        }
        let resident = self.resident(loaded);
        let budget = self.budget();
        if resident <= budget {
            return Ok(());
        }
        let mut names: Vec<&str> = loaded
            .iter()
            .copied()
            .filter(|g| *g != crate::groups::BOOT)
            .collect();
        names.push(crate::groups::BOOT);
        let groups = names
            .iter()
            .map(|g| format!("`{g}`"))
            .collect::<Vec<_>>()
            .join(" + ");
        let mut biggest: Vec<&AssetMemory> = self.resident_assets(loaded).collect();
        biggest.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.logical.cmp(&b.logical)));
        let top = biggest
            .iter()
            .take(3)
            .map(|a| {
                format!(
                    "`{}` {} ({})",
                    a.logical,
                    show_bytes(a.bytes),
                    a.groups.join(", ")
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        Err(format!(
            "{groups} = {} > {} {} for assets ({} of RAM on the {} tier, less {} engine \
             reserve and {} game state; `[memory]` in the manifest): biggest {top}. Move \
             one to a PC-only variant (`platforms`), use a smaller texture format or \
             codec, or split the group.",
            show_bytes(resident),
            show_bytes(budget),
            platform_label(self.platform),
            show_bytes(self.ram),
            self.tier.key(),
            show_bytes(self.reserve),
            show_bytes(self.game),
        ))
    }

    /// The scenarios a report shows: boot alone, then boot plus each
    /// group, then `extra` sets (a scene's required groups), with their
    /// resident bytes.
    pub fn scenarios(&self, extra: &[(String, Vec<String>)]) -> Vec<(String, u64)> {
        let mut out = vec![(crate::groups::BOOT.to_string(), self.resident(&[]))];
        for g in self.groups.iter().skip(1) {
            out.push((format!("boot + {g}"), self.resident(&[g.as_str()])));
        }
        for (name, groups) in extra {
            let refs: Vec<&str> = groups.iter().map(String::as_str).collect();
            out.push((name.clone(), self.resident(&refs)));
        }
        out
    }
}

impl Manifest {
    /// The engine reserve and game RAM on `platform` (`[memory]`, else the
    /// measured defaults).
    pub fn memory_reserve(&self, platform: Platform, tier: HwTier) -> Result<(u64, u64), String> {
        let decl = self.memory.as_ref();
        let get = |v: Option<&PerPlatform<Size>>, what: &str| {
            v.and_then(|v| v.get(platform))
                .map(|s| s.bytes().map_err(|e| format!("[memory] {what}: {e}")))
                .transpose()
        };
        let reserve = get(decl.and_then(|d| d.reserve.as_ref()), "reserve")?
            .unwrap_or_else(|| default_reserve(platform, tier));
        let game = get(decl.and_then(|d| d.game.as_ref()), "game")?.unwrap_or(0);
        Ok((reserve, game))
    }

    pub(crate) fn check_memory(&self) -> Result<(), String> {
        let Some(decl) = &self.memory else {
            return Ok(());
        };
        for (what, v) in [("reserve", &decl.reserve), ("game", &decl.game)] {
            if let Some(k) = v.as_ref().and_then(|v| v.unknown_keys().pop()) {
                return Err(format!("[memory] {what}: unknown platform `{k}`"));
            }
        }
        for p in Platform::ALL {
            self.memory_reserve(p, HwTier::Base)?;
        }
        Ok(())
    }
}

/// The memory plan of the project at `assets_root` on `platform`'s `tier`
/// hardware: every asset the build holds there, sized as it loads.
pub fn plan(assets_root: &Path, platform: Platform, tier: HwTier) -> Result<MemoryPlan, String> {
    let manifest = Manifest::load(&assets_root.join("manifest.toml"))?;
    let (reserve, game) = manifest.memory_reserve(platform, tier)?;
    let mut assets = Vec::new();
    let mut add = |logical: &str, kind: &'static str, category, bytes: u64| {
        assets.push(AssetMemory {
            logical: logical.to_string(),
            kind,
            category,
            groups: manifest.groups_of(logical),
            bytes,
        })
    };
    let texture = |logical: &str| -> Result<u64, String> {
        let (settings, m) = crate::texture::master(assets_root, &manifest, logical, platform)?;
        let choice = crate::texture::choose(
            platform, logical, m.source, &settings, m.width, m.height, &m.rgba,
        )?;
        Ok(crate::texture::report(platform, choice, m.width, m.height).memory_bytes)
    };
    for name in manifest.sprites.keys() {
        let logical = format!("sprites/{name}");
        add(
            &logical,
            "sprite",
            MemoryCategory::Textures,
            texture(&logical)?,
        );
    }
    for (name, decl) in &manifest.fonts {
        let logical = format!("fonts/{name}");
        add(
            &logical,
            "sprite",
            MemoryCategory::Textures,
            texture(&logical)?,
        );
        let source = crate::resolve_source(assets_root, platform, &decl.file)?;
        let font = crate::bake_font(&source, decl).map_err(|e| format!("`{logical}`: {e}"))?;
        add(
            &logical,
            "font",
            MemoryCategory::Data,
            font.table.len() as u64,
        );
    }
    for (prefix, kind, music, section) in [
        ("sounds", "sound", false, &manifest.sounds),
        ("music", "music", true, &manifest.music),
    ] {
        for (name, decl) in section {
            let logical = format!("{prefix}/{name}");
            let settings = decl
                .audio_settings(music, platform)
                .map_err(|e| format!("`{logical}`: {e}"))?;
            let source = crate::resolve_source(assets_root, platform, &decl.file)?;
            let bytes = sound_resident(platform, &settings, &source)
                .map_err(|e| format!("`{logical}`: {e}"))?;
            add(&logical, kind, MemoryCategory::Audio, bytes);
        }
    }
    let mut bounds = BTreeMap::new();
    for (name, decl) in &manifest.models {
        let logical = format!("models/{name}");
        // Not in this build, or only on the other tier.
        match crate::tier::model_tier(decl, platform) {
            None => continue,
            Some(Some(t)) if t != tier => continue,
            Some(_) => {}
        }
        let source = crate::resolve_source(assets_root, platform, &decl.file)?;
        let blob =
            crate::bake_model(&source, &decl.options()).map_err(|e| format!("`{logical}`: {e}"))?;
        if let Some(b) = crate::model::tmdl_bounds(&blob) {
            bounds.insert(logical.clone(), b);
        }
        add(&logical, "model", MemoryCategory::Meshes, blob.len() as u64);
    }
    for (name, decl) in &manifest.blobs {
        let logical = format!("blobs/{name}");
        let source = crate::resolve_source(assets_root, platform, &decl.file)?;
        let len = std::fs::metadata(&source)
            .map_err(|e| format!("`{logical}`: {}: {e}", source.display()))?
            .len();
        add(&logical, "blob", MemoryCategory::Data, len);
    }
    let target = platform.target_on(tier);
    for (name, decl) in &manifest.particles {
        let logical = format!("particles/{name}");
        let source = crate::resolve_source(assets_root, platform, &decl.file)?;
        let blob = crate::particles::bake(&source, target)?;
        add(&logical, "blob", MemoryCategory::Data, blob.len() as u64);
    }
    for (name, decl) in &manifest.scenes {
        let logical = format!("scenes/{name}");
        let blob =
            crate::model::bake_scene_with_bounds(assets_root, &decl.file, bounds.clone(), target)?;
        add(&logical, "blob", MemoryCategory::Data, blob.len() as u64);
    }
    let mix = assets_root.join(crate::audio::MixFile::FILE);
    if mix.exists() {
        let text = std::fs::read_to_string(&mix).map_err(|e| format!("{}: {e}", mix.display()))?;
        let blob = crate::audio::MixFile::parse(&text)
            .and_then(|m| m.bake())
            .map_err(|e| format!("{}: {e}", mix.display()))?;
        add(
            trino_core::MixerConfig::LOGICAL,
            "mixer",
            MemoryCategory::Data,
            blob.len() as u64,
        );
    }
    // UI screens and string tables (issue #58), resident like the mix.
    let root = assets_root.parent().unwrap_or(assets_root);
    for b in crate::ui::bake(root, &manifest, platform)? {
        add(
            &b.logical,
            "blob",
            MemoryCategory::Data,
            b.bytes.len() as u64,
        );
    }
    Ok(MemoryPlan {
        platform,
        tier,
        ram: caps(platform, tier).ram_bytes as u64,
        reserve,
        game,
        groups: manifest.group_names(),
        assets,
    })
}

/// A sound's RAM once loaded on `platform` (`audio::Encoded::footprint`'s
/// `resident`), without encoding it: an N64 wav64 and a 3DS stream hold
/// their state only; the rest hold their samples.
fn sound_resident(
    platform: Platform,
    settings: &crate::audio::AudioSettings,
    source: &Path,
) -> Result<u64, String> {
    use crate::audio::Codec;
    match platform {
        Platform::N64 => return Ok(crate::audio::N64_OPEN_BYTES as u64),
        Platform::N3ds if settings.stream => return Ok(0),
        _ => {}
    }
    let master = crate::decode_master(source)?;
    let rate = settings.sample_rate.unwrap_or(master.rate).max(1);
    let frames = (master.samples.len() as u64 * rate as u64).div_ceil(master.rate.max(1) as u64);
    Ok(match platform {
        Platform::N3ds if settings.codec == Codec::DspAdpcm => {
            crate::audio::dspadpcm::data_bytes(frames as usize) as u64
        }
        Platform::N3ds => frames * 2,
        // A PC or Web stream keeps its encoded file; estimate it as 16-bit.
        _ if settings.stream => frames * 2,
        _ => frames * crate::audio::OUTPUT_RATE as u64 / rate as u64 * 4,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_parse() {
        assert_eq!(parse_size("512K"), Ok(512 * 1024));
        assert_eq!(parse_size("1.5M"), Ok(1536 * 1024));
        assert_eq!(parse_size(" 300 "), Ok(300));
        assert!(parse_size("lots").is_err());
        assert!(parse_size("-1K").is_err());
        assert_eq!(show_bytes(900), "900 B");
        assert_eq!(show_bytes(1500), "2 KB");
        assert_eq!(show_bytes(4_823_449), "4.6 MB");
    }

    fn asset(logical: &str, groups: &[&str], bytes: u64) -> AssetMemory {
        AssetMemory {
            logical: logical.into(),
            kind: "model",
            category: MemoryCategory::Meshes,
            groups: groups.iter().map(|g| g.to_string()).collect(),
            bytes,
        }
    }

    fn fixture() -> MemoryPlan {
        MemoryPlan {
            platform: Platform::N64,
            tier: HwTier::Base,
            ram: 4 << 20,
            reserve: 1 << 20,
            game: 0,
            groups: vec!["boot".into(), "level1".into(), "level2".into()],
            assets: vec![
                asset("models/player", &["boot"], 500 << 10),
                asset("models/boar", &["level1", "level2"], 300 << 10),
                asset("models/castle", &["level1"], 1 << 20),
                asset("models/stadium_hd", &["level2"], 2400 << 10),
            ],
        }
    }

    #[test]
    fn residency_counts_shared_assets_once() {
        let p = fixture();
        assert_eq!(p.resident(&[]), 500 << 10);
        assert_eq!(p.resident(&["level1"]), (800 << 10) + (1 << 20));
        assert_eq!(p.resident(&["level1", "level2"]), (3200 << 10) + (1 << 20));
        assert_eq!(p.group_bytes("level2"), 2700 << 10);
        assert_eq!(
            p.resident_by_category(&["level1"])[0],
            p.resident(&["level1"])
        );
        let s = p.scenarios(&[("scenes/arena".into(), vec!["level2".into()])]);
        assert_eq!(s[0], ("boot".into(), 500 << 10));
        assert_eq!(s.len(), 4);
        assert_eq!(s[3].1, p.resident(&["level2"]));
    }

    #[test]
    fn check_names_the_groups_and_the_biggest_asset() {
        let p = fixture();
        assert!(p.check(&[]).is_ok());
        assert!(p.check(&["level1"]).is_ok());
        let e = p.check(&["level2"]).unwrap_err();
        assert!(
            e.starts_with("`level2` + `boot` = 3.1 MB > 3.0 MB N64"),
            "{e}"
        );
        assert!(e.contains("`models/stadium_hd` 2.3 MB (level2)"), "{e}");
        let pc = MemoryPlan {
            platform: Platform::Pc,
            ram: u32::MAX as u64,
            ..fixture()
        };
        assert!(pc.check(&["level1", "level2"]).is_ok());
    }
}

#[cfg(test)]
mod repo_plan {
    use super::*;

    /// The repo's own assets (castle64 and the self-test) boot on every
    /// console tier: what runs in ares and Azahar fits the plan.
    #[test]
    fn repo_assets_fit_every_console_tier() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        for p in [Platform::N64, Platform::N3ds] {
            for &t in p.tiers() {
                let plan = plan(&root, p, t).unwrap();
                assert!(plan.limited());
                assert!(!plan.assets.is_empty());
                let groups: Vec<&str> = plan.groups.iter().skip(1).map(String::as_str).collect();
                plan.check(&groups).unwrap();
            }
        }
        let pc = plan(&root, Platform::Pc, HwTier::Base).unwrap();
        assert!(!pc.limited() && pc.check(&[]).is_ok());
    }
}
