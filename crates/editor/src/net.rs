//! The Net tab's state (issue #67): the project's `[players]` and `[net]`
//! (`trino.toml`, the keys `cargo xtask net config set` writes), a
//! simulated network, and two local instances of the game playing online
//! (`cargo xtask net local`, run in the background, its `TRINO_NET` lines
//! read back) or a lobby (`cargo xtask net lobby`).

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use trino_net::{NetConfig, NetMode};

/// A background `cargo xtask net …` and the lines it printed.
pub struct Job {
    child: Child,
    lines: Arc<Mutex<Vec<String>>>,
}

impl Job {
    fn spawn(mut cmd: Command, what: &'static str) -> Result<Job, String> {
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("{what}: {e}"))?;
        let lines = Arc::new(Mutex::new(Vec::new()));
        for pipe in [
            child
                .stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
            child
                .stderr
                .take()
                .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let lines = lines.clone();
            std::thread::spawn(move || {
                for l in BufReader::new(pipe).lines().map_while(Result::ok) {
                    if let Ok(mut v) = lines.lock() {
                        v.push(l);
                    }
                }
            });
        }
        Ok(Job { child, lines })
    }

    /// Still running?
    pub fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn lines(&self) -> Vec<String> {
        self.lines.lock().map(|v| v.clone()).unwrap_or_default()
    }

    pub fn stop(&mut self) {
        let _ = self.child.kill();
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.stop();
    }
}

pub struct NetPanel {
    pub max_players: u8,
    pub cfg: NetConfig,
    /// The simulated network (one way).
    pub latency_ms: u32,
    pub jitter_ms: u32,
    pub loss_percent: f32,
    /// What the two instances play (the engine: `solid-eleven`).
    pub game: String,
    /// Each instance plays its seat of the two-player recording and both
    /// print their hash on the same frame (a check, not play).
    pub scripted: bool,
    pub local: Option<Job>,
    pub lobby: Option<Job>,
    /// Whether the project has a `trino.toml` to save to.
    pub has_toml: bool,
    pub status: Option<String>,
}

impl NetPanel {
    pub fn new(root: &Path) -> NetPanel {
        let mut p = NetPanel {
            max_players: 1,
            cfg: NetConfig::DEFAULT,
            latency_ms: 40,
            jitter_ms: 10,
            loss_percent: 3.0,
            game: "solid-eleven".into(),
            scripted: false,
            local: None,
            lobby: None,
            has_toml: false,
            status: None,
        };
        p.reload(root);
        p
    }

    /// Read `trino.toml`'s sections again (an edit on disk, another tool).
    pub fn reload(&mut self, root: &Path) {
        self.has_toml = root.join("trino.toml").is_file();
        if let Ok(n) = trino_asset_pipeline::net::ProjectNet::load(root) {
            self.max_players = n.max_players;
            self.cfg = NetConfig::from_baked(Some(&n.bake())).0;
        }
    }

    /// Write the settings to `trino.toml` (what `net config set` writes).
    pub fn save(&self, root: &Path) -> Result<(), String> {
        let set = |k: &str, v: String| trino_asset_pipeline::net::set_in(root, k, &v);
        set("players.max", self.max_players.to_string())?;
        set("net.mode", self.cfg.mode.key().to_string())?;
        set("net.max_rollback", self.cfg.max_rollback.to_string())?;
        set("net.input_delay", self.cfg.input_delay.to_string())
    }

    pub fn sim(&self) -> String {
        format!(
            "latency={},jitter={},loss={}",
            self.latency_ms, self.jitter_ms, self.loss_percent
        )
    }

    /// `cargo xtask net local …` as an agent would type it.
    pub fn local_args(&self, root: &Path) -> Vec<String> {
        let mut a = vec![
            "net".to_string(),
            "local".into(),
            "--game".into(),
            self.game.clone(),
            "--sim".into(),
            self.sim(),
            "--mode".into(),
            self.cfg.mode.key().into(),
            "--rollback".into(),
            self.cfg.max_rollback.to_string(),
            "--delay".into(),
            self.cfg.input_delay.to_string(),
        ];
        if self.scripted {
            let rec = engine().join("examples/solid-eleven/assets/shared/replays/two_players.trec");
            a.extend([
                "--script".into(),
                rec.display().to_string(),
                "--frames".into(),
                "900".into(),
            ]);
        }
        if root.join("trino.toml").is_file() {
            a.extend(["--project".into(), root.display().to_string()]);
        }
        a
    }

    pub fn start_local(&mut self, root: &Path) -> Result<(), String> {
        self.local = Some(Job::spawn(xtask(&self.local_args(root)), "net local")?);
        Ok(())
    }

    pub fn start_lobby(&mut self) -> Result<(), String> {
        self.lobby = Some(Job::spawn(
            xtask(&["net".into(), "lobby".into()]),
            "net lobby",
        )?);
        Ok(())
    }

    /// The instances' `TRINO_NET` lines (host first), newest last.
    pub fn net_lines(&self) -> Vec<String> {
        self.local
            .as_ref()
            .map(|j| {
                j.lines()
                    .into_iter()
                    .filter(|l| l.contains("TRINO_NET") || l.starts_with("[net]"))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn mode_is_delay(&self) -> bool {
        self.cfg.mode == NetMode::Delay
    }
}

/// The engine checkout (the editor's own).
fn engine() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `cargo run -p xtask -- <args>` from the engine.
fn xtask(args: &[String]) -> Command {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut c = Command::new(cargo);
    c.current_dir(engine())
        .args(["run", "--quiet", "-p", "xtask", "--"])
        .args(args);
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_runs_what_the_cli_runs() {
        let root = engine();
        let mut p = NetPanel::new(&root);
        p.latency_ms = 120;
        p.loss_percent = 5.0;
        p.cfg.mode = NetMode::Delay;
        let a = p.local_args(&root);
        let line = a.join(" ");
        assert!(
            line.starts_with("net local --game solid-eleven --sim latency=120,jitter=10,loss=5"),
            "{line}"
        );
        assert!(line.contains("--mode delay"), "{line}");
        assert!(!line.contains("--project"), "the engine has no trino.toml");
        p.scripted = true;
        assert!(
            p.local_args(&root)
                .join(" ")
                .contains("two_players.trec --frames 900")
        );
    }
}
