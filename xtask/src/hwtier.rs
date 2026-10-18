//! The consoles' hardware tiers in the emulator harness (issue #54, ADR
//! 0020): `--expansion-pak on|off|both` boots ares with or without the N64
//! Expansion Pak, `--new3ds on|off|both` Azahar as a New or an old 3DS, and
//! `--speedup on|off` overrides `trino.toml`'s `[3ds] speedup` for one
//! command. Each run checks the ROM's `TRINO_HW` boot line against what
//! the emulator was told, so a detection that lies fails the test.

/// One hardware switch: a single setting, or both in turn (`both`: on,
/// then off — one CI job covers the two tiers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    On,
    Off,
    Both,
}

impl Mode {
    fn parse(flag: &str, v: &str) -> Result<Mode, String> {
        match v {
            "on" | "true" | "1" => Ok(Mode::On),
            "off" | "false" | "0" => Ok(Mode::Off),
            "both" => Ok(Mode::Both),
            _ => Err(format!("xtask: {flag} takes on, off or both (got `{v}`)")),
        }
    }

    /// The settings to boot with, in order.
    pub fn runs(self) -> &'static [bool] {
        match self {
            Mode::On => &[true],
            Mode::Off => &[false],
            Mode::Both => &[true, false],
        }
    }
}

/// The hardware options of a console command. None leaves the emulator's
/// own setting to `run`/`watch`; `test` forces on (ares's and Azahar's
/// default, what every golden and perf ceiling was measured with).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// ares's `Nintendo64/ExpansionPak`.
    pub expansion_pak: Option<Mode>,
    /// Azahar's `is_new_3ds`.
    pub new3ds: Option<Mode>,
    /// `[3ds] speedup` for this command (None: the project's).
    pub speedup: Option<bool>,
}

impl Options {
    /// The Expansion Pak settings `test n64` boots with.
    pub fn pak_tests(&self) -> Mode {
        self.expansion_pak.unwrap_or(Mode::On)
    }

    /// The console mode `test 3ds` boots with.
    pub fn new3ds_tests(&self) -> Mode {
        self.new3ds.unwrap_or(Mode::On)
    }

    /// The Expansion Pak setting `run`/`watch n64` forces, if any.
    pub fn pak_run(&self) -> Option<bool> {
        self.expansion_pak.map(|m| m.runs()[0])
    }

    /// The console mode `run`/`watch 3ds` forces, if any.
    pub fn new3ds_run(&self) -> Option<bool> {
        self.new3ds.map(|m| m.runs()[0])
    }
}

/// Take `--expansion-pak`, `--new3ds` and `--speedup` (with their values)
/// out of `args`.
pub fn parse(args: &mut Vec<String>) -> Result<Options, String> {
    let mut o = Options::default();
    if let Some(v) = take(args, "--expansion-pak")? {
        o.expansion_pak = Some(Mode::parse("--expansion-pak", &v)?);
    }
    if let Some(v) = take(args, "--new3ds")? {
        o.new3ds = Some(Mode::parse("--new3ds", &v)?);
    }
    if let Some(v) = take(args, "--speedup")? {
        o.speedup = match Mode::parse("--speedup", &v)? {
            Mode::On => Some(true),
            Mode::Off => Some(false),
            Mode::Both => return Err("xtask: --speedup takes on or off".into()),
        };
    }
    Ok(o)
}

fn take(args: &mut Vec<String>, flag: &str) -> Result<Option<String>, String> {
    let Some(i) = args.iter().position(|a| a == flag) else {
        return Ok(None);
    };
    if i + 1 >= args.len() {
        return Err(format!("xtask: {flag} needs a value (on, off or both)"));
    }
    let v = args.remove(i + 1);
    args.remove(i);
    Ok(Some(v))
}

/// The commands each flag goes with (`cmd`, `platform` are the first two
/// arguments left).
pub fn check(o: &Options, cmd: Option<&str>, platform: Option<&str>) -> Result<(), String> {
    let n64 = matches!(platform, Some("n64") | Some("all"));
    let n3ds = matches!(platform, Some("3ds" | "n3ds") | Some("all"));
    let emulated = matches!(cmd, Some("run" | "test" | "watch"));
    if o.expansion_pak.is_some() && !(emulated && n64) {
        return Err(
            "xtask: --expansion-pak goes with `run|test|watch n64` (ares's setting)".into(),
        );
    }
    if o.new3ds.is_some() && !(emulated && n3ds) {
        return Err("xtask: --new3ds goes with `run|test|watch 3ds` (Azahar's setting)".into());
    }
    if o.expansion_pak == Some(Mode::Both) && cmd != Some("test") {
        return Err("xtask: only `test n64` boots twice: --expansion-pak on or off".into());
    }
    if o.new3ds == Some(Mode::Both) && cmd != Some("test") {
        return Err("xtask: only `test 3ds` boots twice: --new3ds on or off".into());
    }
    if o.speedup.is_some() && !(matches!(cmd, Some("build" | "run" | "test" | "watch")) && n3ds) {
        return Err("xtask: --speedup goes with `build|run|test|watch 3ds`".into());
    }
    Ok(())
}

/// The `key=value` fields of a `TRINO_HW <console> …` boot line.
fn fields<'a>(line: &'a str, console: &str) -> Option<Vec<(&'a str, &'a str)>> {
    let rest = line.split_once("TRINO_HW ")?.1;
    let mut words = rest.split_whitespace();
    if words.next()? != console {
        return None;
    }
    Some(words.filter_map(|w| w.split_once('=')).collect())
}

fn field<'a>(f: &[(&'a str, &'a str)], key: &str) -> Option<&'a str> {
    f.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// The N64's boot line against ares's setting: `ram=8M tier=expanded` with
/// the Expansion Pak, `ram=4M tier=base` without. Returns the line's
/// summary.
pub fn check_n64(line: Option<&str>, pak: bool) -> Result<String, String> {
    let line = line.ok_or("hw tier: the ROM printed no `TRINO_HW n64` line")?;
    let f = fields(line, "n64").ok_or_else(|| format!("hw tier: bad line `{line}`"))?;
    let (ram, tier) = (field(&f, "ram"), field(&f, "tier"));
    let want = if pak {
        (Some("8M"), Some("expanded"))
    } else {
        (Some("4M"), Some("base"))
    };
    if (ram, tier) != want {
        return Err(format!(
            "hw tier: ares ran {} the Expansion Pak but the ROM says `{}`",
            if pak { "with" } else { "without" },
            line.trim()
        ));
    }
    Ok(format!(
        "ram={} tier={}",
        ram.unwrap_or("?"),
        tier.unwrap_or("?")
    ))
}

/// The 3DS's boot line against Azahar's New 3DS mode and the app's speedup
/// setting: `new=1 speedup=1 tier=expanded` on a New 3DS with the speedup,
/// `new=1 speedup=0 tier=base` without it, `new=0 speedup=0 tier=base` on an
/// old 3DS. `speedup` None (an app built elsewhere, `--no-build`) takes the
/// app's word for its setting and checks the rest.
pub fn check_3ds(
    line: Option<&str>,
    new3ds: bool,
    speedup: Option<bool>,
) -> Result<String, String> {
    let line = line.ok_or("hw tier: the app printed no `TRINO_HW 3ds` line")?;
    let f = fields(line, "3ds").ok_or_else(|| format!("hw tier: bad line `{line}`"))?;
    let speedup = speedup.unwrap_or(field(&f, "speedup") == Some("1"));
    let on = new3ds && speedup;
    let want = (
        Some(if new3ds { "1" } else { "0" }),
        Some(if on { "1" } else { "0" }),
        Some(if on { "expanded" } else { "base" }),
    );
    let got = (field(&f, "new"), field(&f, "speedup"), field(&f, "tier"));
    if got != want {
        return Err(format!(
            "hw tier: Azahar ran as {} 3DS (speedup {}) but the app says `{}`",
            if new3ds { "a New" } else { "an old" },
            if speedup { "on" } else { "off" },
            line.trim()
        ));
    }
    Ok(format!(
        "new={} speedup={} tier={}",
        want.0.unwrap_or("?"),
        want.1.unwrap_or("?"),
        want.2.unwrap_or("?")
    ))
}

/// The engine self-test's `TRINO_STEP:hw tier <tier> … data=<tier> …`
/// line against the boot line's tier: the console loaded its own tier's
/// scene, particles and models (the `base` / `expanded` index lines).
pub fn check_data(hw_line: Option<&str>, step: Option<&str>) -> Result<String, String> {
    let tier = hw_line
        .and_then(|l| {
            l.split_once("TRINO_HW ")?
                .1
                .split_whitespace()
                .find_map(|w| w.strip_prefix("tier="))
        })
        .ok_or("hw tier: no tier in the `TRINO_HW` line")?;
    let step = step.ok_or("hw tier: the self-test printed no `TRINO_STEP:hw tier` line")?;
    let rest = step
        .split_once("TRINO_STEP:hw tier ")
        .map(|(_, r)| r.trim())
        .ok_or_else(|| format!("hw tier: bad line `{step}`"))?;
    let mut words = rest.split_whitespace();
    let stepped = words.next();
    let data = words.find_map(|w| w.strip_prefix("data="));
    if stepped != Some(tier) || data != Some(tier) {
        return Err(format!(
            "hw tier: booted on the {tier} tier but the self-test says `{rest}`"
        ));
    }
    Ok(rest.to_string())
}

/// The self-test's `TRINO_STEP:quality <preset> fixed …` line (issue #55):
/// a console's preset is fixed, scales nothing (LOD bias 1) and is the one
/// its tier gets.
pub fn check_quality(step: Option<&str>, want: &str) -> Result<String, String> {
    let step = step.ok_or("quality: the self-test printed no `TRINO_STEP:quality` line")?;
    let rest = step
        .split_once("TRINO_STEP:quality ")
        .map(|(_, r)| r.trim())
        .ok_or_else(|| format!("quality: bad line `{step}`"))?;
    let mut words = rest.split_whitespace();
    let (preset, kind) = (words.next(), words.next());
    let lod = words.find_map(|w| w.strip_prefix("lod="));
    if preset != Some(want) || kind != Some("fixed") || lod != Some("100") {
        return Err(format!(
            "quality: a console must report its fixed `{want}` preset at LOD bias 1, \
             the self-test says `{rest}`"
        ));
    }
    Ok(rest.to_string())
}

/// The 3DS self-test's `TRINO_STEP:worker core=N …` line (issue #54 Phase
/// 3): a New 3DS runs the jobs on its core 2, an old one on core 1.
pub fn check_worker(step: Option<&str>, new3ds: bool) -> Result<String, String> {
    let step = step.ok_or("worker: the self-test printed no `TRINO_STEP:worker core=` line")?;
    let rest = step
        .split_once("TRINO_STEP:worker ")
        .map(|(_, r)| r.trim())
        .ok_or_else(|| format!("worker: bad line `{step}`"))?;
    let core = rest
        .split_whitespace()
        .find_map(|w| w.strip_prefix("core="));
    let want = if new3ds { "2" } else { "1" };
    if core != Some(want) {
        return Err(format!(
            "worker: {} 3DS should run the jobs on core {want}, the self-test says `{rest}`",
            if new3ds { "a New" } else { "an old" }
        ));
    }
    Ok(rest.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn flags_parse_and_leave_the_rest() {
        let mut a = args(&["test", "n64", "--expansion-pak", "off", "--bless"]);
        let o = parse(&mut a).unwrap();
        assert_eq!(a, args(&["test", "n64", "--bless"]));
        assert_eq!(o.expansion_pak, Some(Mode::Off));
        assert_eq!((o.pak_tests(), o.pak_run()), (Mode::Off, Some(false)));
        assert_eq!((o.new3ds, o.speedup), (None, None));
        check(&o, Some("test"), Some("n64")).unwrap();

        let mut a = args(&["test", "3ds", "--new3ds", "both", "--speedup", "off"]);
        let o = parse(&mut a).unwrap();
        assert_eq!((o.new3ds, o.speedup), (Some(Mode::Both), Some(false)));
        assert_eq!(o.new3ds_tests().runs(), &[true, false]);
        check(&o, Some("test"), Some("3ds")).unwrap();

        // Unset: `test` boots on, `run` leaves the emulator's setting.
        let o = parse(&mut args(&[])).unwrap();
        assert_eq!(o, Options::default());
        assert_eq!((o.pak_tests(), o.new3ds_tests()), (Mode::On, Mode::On));
        assert_eq!((o.pak_run(), o.new3ds_run()), (None, None));
        for bad in [
            &["test", "n64", "--expansion-pak"][..],
            &["test", "n64", "--expansion-pak", "maybe"],
            &["build", "3ds", "--speedup", "both"],
        ] {
            assert!(parse(&mut args(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn each_flag_goes_with_its_console() {
        let pak = Options {
            expansion_pak: Some(Mode::Off),
            ..Options::default()
        };
        assert!(check(&pak, Some("test"), Some("3ds")).is_err());
        assert!(check(&pak, Some("build"), Some("n64")).is_err());
        check(&pak, Some("run"), Some("n64")).unwrap();
        check(&pak, Some("test"), Some("all")).unwrap();
        let both = Options {
            expansion_pak: Some(Mode::Both),
            ..Options::default()
        };
        assert!(check(&both, Some("run"), Some("n64")).is_err());
        check(&both, Some("test"), Some("n64")).unwrap();
        let new = Options {
            new3ds: Some(Mode::Off),
            ..Options::default()
        };
        assert!(check(&new, Some("test"), Some("n64")).is_err());
        check(&new, Some("watch"), Some("3ds")).unwrap();
        let speed = Options {
            speedup: Some(false),
            ..Options::default()
        };
        check(&speed, Some("build"), Some("3ds")).unwrap();
        assert!(check(&speed, Some("build"), Some("n64")).is_err());
        check(&Options::default(), Some("scene"), Some("check")).unwrap();
    }

    #[test]
    fn the_n64_line_must_match_the_expansion_pak() {
        let x = "[ares] TRINO_HW n64 ram=8M tier=expanded";
        let b = "TRINO_HW n64 ram=4M tier=base\n";
        assert_eq!(check_n64(Some(x), true).unwrap(), "ram=8M tier=expanded");
        assert_eq!(check_n64(Some(b), false).unwrap(), "ram=4M tier=base");
        assert!(check_n64(Some(x), false).is_err());
        assert!(check_n64(Some(b), true).is_err());
        assert!(check_n64(None, true).is_err());
        assert!(check_n64(Some("TRINO_HW 3ds new=1"), true).is_err());
    }

    #[test]
    fn the_3ds_line_must_match_the_console_and_the_speedup() {
        let new_on = "TRINO_HW 3ds new=1 speedup=1 app_ram=64M tier=expanded";
        let new_off = "TRINO_HW 3ds new=1 speedup=0 app_ram=64M tier=base";
        let old = "TRINO_HW 3ds new=0 speedup=0 app_ram=64M tier=base";
        check_3ds(Some(new_on), true, Some(true)).unwrap();
        check_3ds(Some(new_off), true, Some(false)).unwrap();
        check_3ds(Some(old), false, Some(true)).unwrap();
        check_3ds(Some(old), false, Some(false)).unwrap();
        assert!(check_3ds(Some(new_on), false, Some(true)).is_err());
        assert!(check_3ds(Some(new_on), true, Some(false)).is_err());
        assert!(check_3ds(Some(old), true, Some(true)).is_err());
        assert!(check_3ds(None, true, Some(true)).is_err());
        // An app built elsewhere: its own speedup, the rest still checked.
        check_3ds(Some(new_on), true, None).unwrap();
        check_3ds(Some(new_off), true, None).unwrap();
        check_3ds(Some(old), false, None).unwrap();
        assert!(check_3ds(Some(old), true, None).is_err());
        let lying = "TRINO_HW 3ds new=1 speedup=0 app_ram=64M tier=expanded";
        assert!(check_3ds(Some(lying), true, None).is_err());
    }

    #[test]
    fn the_self_test_data_must_be_the_booted_tiers() {
        let hw = Some("TRINO_HW n64 ram=8M tier=expanded");
        let step = "TRINO_STEP:hw tier expanded ram=8M cores=1 data=expanded sparks=6";
        assert_eq!(
            check_data(hw, Some(step)).unwrap(),
            "expanded ram=8M cores=1 data=expanded sparks=6"
        );
        let base = Some("TRINO_HW 3ds new=1 speedup=0 app_ram=64M tier=base");
        let step_base = "[azahar] TRINO_STEP:hw tier base ram=64M cores=1 data=base sparks=4";
        check_data(base, Some(step_base)).unwrap();
        // The base tier's data on expanded hardware, and the reverse.
        let stale = "TRINO_STEP:hw tier expanded ram=8M cores=1 data=base sparks=4";
        assert!(check_data(hw, Some(stale)).is_err());
        assert!(check_data(base, Some(step)).is_err());
        assert!(check_data(hw, None).is_err());
        assert!(check_data(None, Some(step)).is_err());
    }

    #[test]
    fn the_worker_runs_on_the_consoles_second_core() {
        let new = "[azahar] TRINO_STEP:worker core=2 queued=8 us=900";
        let old = "TRINO_STEP:worker core=1 queued=8 us=2900";
        assert_eq!(
            check_worker(Some(new), true).unwrap(),
            "core=2 queued=8 us=900"
        );
        check_worker(Some(old), false).unwrap();
        assert!(check_worker(Some(old), true).is_err());
        assert!(check_worker(Some("TRINO_STEP:worker core=-1 queued=0 us=1"), false).is_err());
        assert!(check_worker(None, true).is_err());
    }

    #[test]
    fn a_console_reports_its_fixed_quality() {
        let n64 = "[ares] TRINO_STEP:quality low fixed lod=100 particles=4";
        assert_eq!(
            check_quality(Some(n64), "low").unwrap(),
            "low fixed lod=100 particles=4"
        );
        assert!(check_quality(Some(n64), "medium").is_err());
        let hosted = "TRINO_STEP:quality high hosted lod=100 particles=4";
        assert!(check_quality(Some(hosted), "high").is_err());
        let biased = "TRINO_STEP:quality medium fixed lod=80 particles=4";
        assert!(check_quality(Some(biased), "medium").is_err());
        assert!(check_quality(None, "low").is_err());
    }
}
