//! The quality settings of a PC or Web host (issue #55, ADR 0022): the
//! player's saved settings (the PC's `<save_name>.quality` file, the Web's
//! `localStorage`), what the first launch starts with, Steam Deck
//! detection, and the platform side of a game's
//! [`trino_core::Platform::set_quality`] / `detect_quality`
//! ([`HostedQuality`], what `PcPlatform` and `WebPlatform` hold).
//!
//! What a run starts with, first match wins:
//!
//! 1. Forced for this run, never saved: `TRINO_QUALITY=<preset>|auto`
//!    (`cargo xtask run pc --quality low`), the Web's `?quality=`.
//! 2. The saved settings: the player's last choice.
//! 3. The machine: a Steam Deck gets [`QualitySettings::STEAM_DECK`] over
//!    the project's Medium, a phone or a tablet's browser the project's
//!    Low.
//! 4. The project's `[pc.quality] default` (`trino.toml`): a preset name,
//!    or `auto` (the default) for the calibration bench, whose pick is then
//!    saved.

use trino_core::{
    Caps, FrameLimit, PresetTable, QualityInfo, QualityPreset, QualitySettings, QualitySource,
};

/// What a game asked of the host this frame.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum QualityRequest {
    /// New settings ([`trino_core::Platform::set_quality`]).
    Set(QualitySettings),
    /// Calibrate again ([`trino_core::Platform::detect_quality`]).
    Detect,
}

/// What a run starts with ([`start`]).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Start {
    pub settings: QualitySettings,
    pub source: QualitySource,
    /// Run the calibration bench before the first frame and take its pick
    /// (`settings` is the High placeholder until then).
    pub bench: bool,
    /// Write the settings to the player's file once known (not for a
    /// forced run).
    pub save: bool,
}

/// The settings as the `.quality` file holds them (`key=value` lines,
/// [`QualitySettings::write_text`]).
pub fn to_text(q: &QualitySettings) -> String {
    let mut s = String::from("# Trino quality settings (issue #55): edit or delete freely.\n");
    let _ = q.write_text(&mut s);
    s
}

/// Parse [`to_text`]: its `preset=` line picks the base values from
/// `table` (High without one), the other lines override them; broken
/// lines are ignored and every value is clamped into its range.
pub fn from_text(text: &str, table: &PresetTable) -> QualitySettings {
    let preset = text
        .lines()
        .filter_map(|l| l.split_once('='))
        .find(|(k, _)| k.trim() == "preset")
        .and_then(|(_, v)| QualityPreset::parse(v))
        .unwrap_or(QualityPreset::High);
    QualitySettings::parse_text(table.get(preset), text)
}

/// The baked `quality/presets` blob (`trino-asset-pipeline`'s
/// `quality::baked_text`: a `default=` line, then the table's sections):
/// the project's presets and `[pc.quality] default`. Without one (no
/// `assets/quality.ron`, the default `auto`): the built-in presets, `auto`.
pub fn baked(blob: Option<&[u8]>) -> (PresetTable, String) {
    let Some(text) = blob.and_then(|b| std::str::from_utf8(b).ok()) else {
        return (PresetTable::BUILT_IN, "auto".into());
    };
    let default = text
        .lines()
        .take_while(|l| !l.trim_start().starts_with('['))
        .filter_map(|l| l.split_once('='))
        .find(|(k, _)| k.trim() == "default")
        .map_or_else(|| "auto".into(), |(_, v)| v.trim().to_string());
    (PresetTable::parse(text), default)
}

/// A preset name, or `auto` (`None`: the bench); anything else is not a
/// quality.
pub fn parse_choice(s: &str) -> Option<Option<QualityPreset>> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("auto") {
        return Some(None);
    }
    QualityPreset::parse(s)
        .filter(|p| *p != QualityPreset::Custom)
        .map(Some)
}

/// The Steam Deck's settings over the project's table: its Medium, capped
/// at 60 fps.
pub fn steam_deck(table: &PresetTable) -> QualitySettings {
    QualitySettings {
        frame_limit: QualitySettings::STEAM_DECK.frame_limit,
        ..table.get(QualityPreset::Medium)
    }
}

/// The frame limits a Steam Deck's options page offers on battery, after
/// its 60.
pub const STEAM_DECK_BATTERY: [FrameLimit; 2] = [FrameLimit::Fps(40), FrameLimit::Fps(30)];

/// The machine a host runs on, as far as its first quality goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Machine {
    #[default]
    Desktop,
    /// A Steam Deck ([`is_steam_deck`]).
    SteamDeck,
    /// A phone or a tablet (the Web's `navigator`).
    Mobile,
}

/// What a run starts with (see the module doc): `forced` is
/// `TRINO_QUALITY` (`?quality=`), `saved` the saved text, `default` the
/// project's `[pc.quality] default`.
pub fn start(
    forced: Option<&str>,
    saved: Option<&str>,
    machine: Machine,
    default: &str,
    table: &PresetTable,
) -> Start {
    let bench = |source, save| Start {
        settings: table.get(QualityPreset::High),
        source,
        bench: true,
        save,
    };
    if let Some(choice) = forced.and_then(parse_choice) {
        return match choice {
            None => bench(QualitySource::Forced, false),
            Some(p) => Start {
                settings: table.get(p),
                source: QualitySource::Forced,
                bench: false,
                save: false,
            },
        };
    }
    if let Some(text) = saved.filter(|t| t.contains('=')) {
        return Start {
            settings: from_text(text, table),
            source: QualitySource::Saved,
            bench: false,
            save: false,
        };
    }
    match machine {
        Machine::SteamDeck => {
            return Start {
                settings: steam_deck(table),
                source: QualitySource::SteamDeck,
                bench: false,
                save: true,
            };
        }
        // Not saved: a phone keeps following the project's Low.
        Machine::Mobile => {
            return Start {
                settings: table.get(QualityPreset::Low),
                source: QualitySource::Mobile,
                bench: false,
                save: false,
            };
        }
        Machine::Desktop => {}
    }
    match parse_choice(default).unwrap_or(None) {
        None => bench(QualitySource::Detected, true),
        Some(p) => Start {
            settings: table.get(p),
            source: QualitySource::Default,
            bench: false,
            save: false,
        },
    }
}

/// The platform side of a game's quality calls on a PC or Web host: what
/// [`trino_core::Platform::quality`] and its siblings answer, and the
/// request the host applies after the frame. Until the host publishes
/// ([`publish`](Self::publish)) the quality is fixed and requests are
/// ignored (a console profile, a test platform).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct HostedQuality {
    settings: QualitySettings,
    info: Option<QualityInfo>,
    table: PresetTable,
    request: Option<QualityRequest>,
}

impl HostedQuality {
    /// Fixed at what `caps` says: High on the native profiles (no clock
    /// budget), a console's preset on its profile.
    pub fn new(caps: &Caps) -> HostedQuality {
        HostedQuality {
            settings: if caps.cpu_mhz == 0 {
                QualitySettings::HIGH
            } else {
                QualitySettings::console(caps)
            },
            info: None,
            table: PresetTable::BUILT_IN,
            request: None,
        }
    }

    /// The host publishes the quality in effect and how it got there
    /// (`None`: fixed again).
    pub fn publish(&mut self, info: Option<QualityInfo>) {
        if let Some(i) = &info {
            self.settings = i.settings;
        }
        self.info = info;
    }

    /// The project's presets (`assets/quality.ron`, baked to
    /// `quality/presets`).
    pub fn set_presets(&mut self, table: PresetTable) {
        self.table = table;
    }

    /// The game's last request, for the host to apply after the frame.
    pub fn take_request(&mut self) -> Option<QualityRequest> {
        self.request.take()
    }

    /// [`trino_core::Platform::quality`].
    pub fn settings(&self) -> QualitySettings {
        self.settings
    }

    /// [`trino_core::Platform::set_quality`]: clamped into range.
    pub fn request(&mut self, settings: QualitySettings) {
        if self.info.is_some() {
            self.request = Some(QualityRequest::Set(settings.clamped()));
        }
    }

    /// [`trino_core::Platform::quality_preset`].
    pub fn preset(&self, preset: QualityPreset) -> Option<QualitySettings> {
        (self.info.is_some() && preset != QualityPreset::Custom).then(|| self.table.get(preset))
    }

    /// [`trino_core::Platform::quality_info`].
    pub fn info(&self) -> Option<QualityInfo> {
        self.info
    }

    /// [`trino_core::Platform::detect_quality`].
    pub fn detect(&mut self) {
        if self.info.is_some() {
            self.request = Some(QualityRequest::Detect);
        }
    }
}

/// This machine is a Steam Deck: `SteamDeck=1` (what Steam sets in game
/// mode) or the board's DMI name (`Jupiter`, the LCD model; `Galileo`, the
/// OLED one).
pub fn is_steam_deck() -> bool {
    let env = std::env::var("SteamDeck").ok();
    let product = std::fs::read_to_string("/sys/devices/virtual/dmi/id/product_name").ok();
    steam_deck_from(env.as_deref(), product.as_deref())
}

/// [`is_steam_deck`] from its two signals.
pub fn steam_deck_from(env: Option<&str>, product_name: Option<&str>) -> bool {
    env.is_some_and(|v| v.trim() == "1")
        || product_name.is_some_and(|n| matches!(n.trim(), "Jupiter" | "Galileo"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use trino_core::{AoSetting, DynRes};

    #[test]
    fn the_file_round_trips_and_survives_hand_edits() {
        let t = PresetTable::BUILT_IN;
        for p in QualityPreset::NAMED {
            let q = t.get(p);
            assert_eq!(from_text(&to_text(&q), &t), q, "{p:?}");
        }
        let custom = QualitySettings {
            preset: QualityPreset::Custom,
            render_scale: 0.8,
            dynamic_resolution: Some(DynRes {
                target_us: 8_000,
                min_scale: 0.5,
                max_scale: 1.25,
            }),
            ao: AoSetting::High,
            frame_limit: FrameLimit::Fps(40),
            ..QualitySettings::MEDIUM
        };
        assert_eq!(from_text(&to_text(&custom), &t), custom);
        // A hand-edited file: unknown keys, broken values, out of range.
        let q = from_text(
            "preset=low\nmsaa=banana\nrender_scale=9\nwhat=ever\nao=high\n",
            &t,
        );
        assert_eq!(q.preset, QualityPreset::Low);
        assert_eq!(q.msaa, QualitySettings::LOW.msaa);
        assert_eq!(q.render_scale, 2.0);
        assert_eq!(q.ao, AoSetting::High);
        // A project's table changes what a bare preset means.
        let table = PresetTable::parse("[low]\nrender_scale=0.6\n");
        assert_eq!(from_text("preset=low\n", &table).render_scale, 0.6);
    }

    #[test]
    fn the_start_follows_forced_saved_the_machine_then_the_project() {
        let t = PresetTable::BUILT_IN;
        let saved = to_text(&QualitySettings::ULTRA);
        // Forced beats everything and is never saved.
        let s = start(Some("low"), Some(&saved), Machine::SteamDeck, "auto", &t);
        assert_eq!(
            (s.settings, s.source, s.bench, s.save),
            (QualitySettings::LOW, QualitySource::Forced, false, false)
        );
        let s = start(Some("auto"), Some(&saved), Machine::Desktop, "high", &t);
        assert!(s.bench && !s.save);
        // A forced value that isn't a quality is ignored.
        let s = start(Some("fast"), Some(&saved), Machine::Mobile, "auto", &t);
        assert_eq!(
            (s.settings, s.source),
            (QualitySettings::ULTRA, QualitySource::Saved)
        );
        // The Steam Deck when nothing was saved.
        let s = start(None, None, Machine::SteamDeck, "auto", &t);
        assert_eq!(s.source, QualitySource::SteamDeck);
        assert_eq!(s.settings, QualitySettings::STEAM_DECK);
        assert!(s.save && !s.bench);
        // A phone's browser: the project's Low, not saved.
        let table = PresetTable::parse("[low]\nrender_scale=0.6\n");
        let s = start(None, None, Machine::Mobile, "auto", &table);
        assert_eq!(
            (s.settings, s.source, s.bench, s.save),
            (
                table.get(QualityPreset::Low),
                QualitySource::Mobile,
                false,
                false
            )
        );
        // The project: auto calibrates and saves; a preset is the default.
        let s = start(None, Some(""), Machine::Desktop, "auto", &t);
        assert_eq!(
            (s.source, s.bench, s.save),
            (QualitySource::Detected, true, true)
        );
        let s = start(None, None, Machine::Desktop, "medium", &t);
        assert_eq!(
            (s.settings, s.source, s.bench),
            (QualitySettings::MEDIUM, QualitySource::Default, false)
        );
        // Garbage in trino.toml calibrates.
        assert!(start(None, None, Machine::Desktop, "nope", &t).bench);
    }

    #[test]
    fn the_baked_blob_gives_the_table_and_the_default() {
        assert_eq!(baked(None), (PresetTable::BUILT_IN, "auto".to_string()));
        let (t, d) = baked(Some(
            b"default=medium\n[low]\nrender_scale=0.6\ndefault=x\n",
        ));
        assert_eq!(d, "medium");
        assert_eq!(t.get(QualityPreset::Low).render_scale, 0.6);
        assert_eq!(baked(Some(b"[low]\ndefault=high\n")).1, "auto");
    }

    #[test]
    fn the_steam_deck_is_recognized_by_steam_or_its_board() {
        assert!(steam_deck_from(Some("1"), None));
        assert!(steam_deck_from(None, Some("Jupiter\n")));
        assert!(steam_deck_from(None, Some("Galileo")));
        assert!(!steam_deck_from(Some("0"), Some("20XWCTO1WW")));
        assert!(!steam_deck_from(None, None));
        assert_eq!(
            steam_deck(&PresetTable::BUILT_IN).frame_limit,
            FrameLimit::Fps(60)
        );
    }
}
