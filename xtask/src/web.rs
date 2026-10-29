//! The Web target (issue #47): `cargo xtask build|run|test|assets web`.
//!
//! - **`build web [--release] [--out <dir>]`**: bakes the assets for the
//!   Web (KTX2 textures, ADR 0017 §6) into asset packs, compiles the web
//!   host crate for `wasm32-unknown-unknown`, runs `wasm-bindgen` and
//!   writes a ready-to-serve folder — `target/web/{index.html, trino.js,
//!   trino.wasm, snippets/, basis/, assets/*.pak}` by default. `--out`
//!   (the editor's "Export Web Package") writes it elsewhere. `--selftest`
//!   is accepted for symmetry with the consoles: the self-test is a URL
//!   (`?selftest`), every build carries it.
//! - **`run web [--port N]`**: build, serve the folder on 127.0.0.1 with
//!   the right MIME types and COOP/COEP headers, and open the browser
//!   (`TRINO_NO_BROWSER=1` only serves).
//! - **`test web [--headed] [--browsers chromium,firefox,webkit]
//!   [--no-build] [--bless]`**: serve the build and drive the self-test in
//!   each browser with Playwright (`tools/web-test/run.mjs`), in one run:
//!   `TRINO_TEST_PASS`, the KTX2 transcode step, a save that survives a
//!   reload, and the golden frames against `tests/golden/web/` within a
//!   tolerance (each browser rasterizes on its own GPU path).
//!
//! The toolchain is checked before anything runs: the wasm32 target (added
//! with rustup when missing) and the `wasm-bindgen` CLI at the exact
//! version of the `wasm-bindgen` crate in `Cargo.lock` (installed into
//! `target/tools` when missing, or taken from `$TRINO_WASM_BINDGEN`).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use trino_asset_pipeline::Platform;
use trino_platform_web::pack::{self, Kind};

use crate::project::Project;

const TARGET: &str = "wasm32-unknown-unknown";

/// `build web` options.
#[derive(Clone, Debug, Default)]
pub struct BuildOptions {
    pub release: bool,
    /// Where the page goes (default `<project>/target/web`).
    pub out: Option<PathBuf>,
}

/// Parse `--release`, `--out <dir>`, `--port N`, `--headed`, `--browsers
/// a,b` out of `args` (removing them).
pub fn take_flag(args: &mut Vec<String>, flag: &str) -> bool {
    let found = args.iter().any(|a| a == flag);
    args.retain(|a| a != flag);
    found
}

pub fn take_value(args: &mut Vec<String>, flag: &str) -> Result<Option<String>, String> {
    let Some(i) = args.iter().position(|a| a == flag) else {
        return Ok(None);
    };
    if i + 1 >= args.len() {
        return Err(format!("xtask: {flag} needs a value"));
    }
    let v = args.remove(i + 1);
    args.remove(i);
    Ok(Some(v))
}

fn cargo_bin() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

/// The wasm32 target: present, or added with rustup.
fn ensure_target() -> Result<(), String> {
    let list = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output();
    if let Ok(out) = &list
        && String::from_utf8_lossy(&out.stdout)
            .lines()
            .any(|l| l.trim() == TARGET)
    {
        return Ok(());
    }
    println!("[web] adding the {TARGET} target (rustup target add {TARGET})");
    let ok = Command::new("rustup")
        .args(["target", "add", TARGET])
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        Ok(())
    } else {
        Err(format!(
            "the Web target needs Rust's {TARGET} target: run `rustup target add {TARGET}` \
             (rustup not found or the download failed)"
        ))
    }
}

/// The `wasm-bindgen` crate's version in the engine's `Cargo.lock`: the
/// CLI must match it exactly.
pub fn bindgen_version(engine: &Path) -> Result<String, String> {
    let lock = std::fs::read_to_string(engine.join("Cargo.lock"))
        .map_err(|e| format!("Cargo.lock: {e}"))?;
    let mut lines = lock.lines();
    while let Some(l) = lines.next() {
        if l.trim() == "name = \"wasm-bindgen\""
            && let Some(v) = lines.next()
            && let Some(v) = v.trim().strip_prefix("version = \"")
        {
            return Ok(v.trim_end_matches('"').to_string());
        }
    }
    Err("Cargo.lock has no wasm-bindgen: the Web crates are missing".into())
}

/// The `wasm-bindgen` CLI at the version the project's `Cargo.lock` pins (the
/// engine's for the engine): `$TRINO_WASM_BINDGEN`, the one on PATH when its
/// version matches, the engine's `target/tools`, else installed there.
fn ensure_bindgen(project: &Project) -> Result<PathBuf, String> {
    let engine = &project.engine;
    let version = bindgen_version(&project.root).or_else(|_| bindgen_version(engine))?;
    let matches = |bin: &Path| {
        Command::new(bin)
            .arg("--version")
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains(&version))
    };
    if let Ok(p) = std::env::var("TRINO_WASM_BINDGEN") {
        let p = PathBuf::from(p);
        return if matches(&p) {
            Ok(p)
        } else {
            Err(format!(
                "$TRINO_WASM_BINDGEN ({}) is not wasm-bindgen {version}",
                p.display()
            ))
        };
    }
    let exe = if cfg!(windows) {
        "wasm-bindgen.exe"
    } else {
        "wasm-bindgen"
    };
    let on_path = PathBuf::from(exe);
    if matches(&on_path) {
        return Ok(on_path);
    }
    let tools = engine.join("target/tools");
    let local = tools.join("bin").join(exe);
    if matches(&local) {
        return Ok(local);
    }
    println!(
        "[web] installing wasm-bindgen-cli {version} into {} (once; about a minute)",
        tools.display()
    );
    let ok = Command::new(cargo_bin())
        .args(["install", "wasm-bindgen-cli", "--locked", "--version"])
        .arg(&version)
        .arg("--root")
        .arg(&tools)
        .status()
        .is_ok_and(|s| s.success());
    if ok && matches(&local) {
        Ok(local)
    } else {
        Err(format!(
            "the Web target needs wasm-bindgen-cli {version}: `cargo install wasm-bindgen-cli \
             --locked --version {version}` (or set TRINO_WASM_BINDGEN)"
        ))
    }
}

/// Fixtures the browser self-test transcodes (`ktx2 transcode` step): a
/// test card in both Basis modes and what Binomial's transcoder decodes
/// each to here, as blobs (`web/ktx2_<mode>`, `web/ktx2_<mode>.rgba`).
pub fn ktx2_fixtures() -> Result<Vec<(Kind, u32, Vec<u8>)>, String> {
    use trino_asset_pipeline::texture::{Ktx2Mode, ktx2};
    let (w, h) = (32u32, 32u32);
    let card: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let (x, y) = (i % w, i / w);
            let a = if (x / 8 + y / 8) % 2 == 0 { 255 } else { 128 };
            [(x * 8) as u8, (y * 8) as u8, ((x + y) * 4) as u8, a]
        })
        .collect();
    let mut out = Vec::new();
    for mode in Ktx2Mode::ALL {
        let key = match mode {
            Ktx2Mode::Etc1s => "etc1s",
            Ktx2Mode::Uastc => "uastc",
        };
        let e = ktx2::encode(mode, w, h, &card)?;
        let id = |p: &str| trino_core::asset_id(p);
        out.push((Kind::Blob, id(&format!("web/ktx2_{key}")), e.ktx2));
        out.push((Kind::Blob, id(&format!("web/ktx2_{key}.rgba")), e.decoded));
    }
    Ok(out)
}

/// Bake `assets` for the Web into `baked` and pack it into `pak` (with the
/// KTX2 fixtures when `fixtures`), each asset group's bundle beside it
/// (`pack::bundle_url`, issue #56). Returns the pack's size.
pub fn pack_assets(assets: &Path, baked: &Path, pak: &Path, fixtures: bool) -> Result<u64, String> {
    trino_asset_pipeline::bake_all_with(assets, Platform::Web, baked, crate::threads::build())
        .map_err(|e| e.to_string())?;
    let entries = pack_entries(baked)?;
    let mut entries = entries;
    if fixtures {
        entries.extend(ktx2_fixtures()?);
    }
    let bytes = pack::write(&entries);
    if let Some(dir) = pak.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(pak, &bytes).map_err(|e| format!("{}: {e}", pak.display()))?;
    for (kind, _, name) in &entries {
        if *kind != Kind::Group {
            continue;
        }
        let name = String::from_utf8_lossy(name);
        let from = baked.join("groups").join(format!("{name}.tbnd"));
        let to = PathBuf::from(pack::bundle_url(&pak.to_string_lossy(), &name));
        std::fs::copy(&from, &to).map_err(|e| format!("{}: {e}", from.display()))?;
        let size = std::fs::metadata(&to).map_or(0, |m| m.len());
        println!("[web] {}: {size} bytes", to.display());
    }
    Ok(bytes.len() as u64)
}

/// A Web bake's `index.toml` as pack entries: sprites as their KTX2 file
/// when they have one (else the master), sounds as their `TSN2` blob (Ogg
/// Vorbis or PCM16, checked), the project's mix as `TMIX`, the rest as
/// baked. Assets of other groups than `boot` stay in their bundles: the
/// pack names those groups ([`Kind::Group`]).
pub fn pack_entries(baked: &Path) -> Result<Vec<(Kind, u32, Vec<u8>)>, String> {
    #[derive(serde::Deserialize)]
    struct Index {
        #[serde(default, rename = "asset")]
        assets: Vec<Entry>,
    }
    #[derive(serde::Deserialize)]
    struct Entry {
        id: u32,
        kind: String,
        file: String,
        #[serde(default)]
        ktx2: Option<String>,
        #[serde(default)]
        groups: Vec<String>,
    }
    let path = baked.join("index.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let index: Index = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let read = |file: &str| {
        let p = baked.join(file);
        std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))
    };
    let mut out = Vec::new();
    let mut groups = std::collections::BTreeSet::new();
    for e in index.assets {
        if !e.groups.is_empty() {
            groups.extend(e.groups);
            continue;
        }
        let entry = match (e.kind.as_str(), &e.ktx2) {
            ("sprite", Some(k)) => (Kind::SpriteKtx2, read(k)?),
            ("sprite", None) => (Kind::Sprite, read(&e.file)?),
            ("sound" | "music", _) => {
                let kind = if e.kind == "music" {
                    Kind::Music
                } else {
                    Kind::Sound
                };
                let bytes = read(&e.file)?;
                pack::sound_blob(kind, &bytes).map_err(|err| format!("{}: {err}", e.file))?;
                (kind, bytes)
            }
            ("mixer", _) => {
                let bytes = read(&e.file)?;
                trino_core::MixerConfig::parse(&bytes)
                    .map_err(|err| format!("{}: {err}", e.file))?;
                (Kind::Mixer, bytes)
            }
            ("model", _) => (Kind::Model, read(&e.file)?),
            ("font", _) => (Kind::Font, read(&e.file)?),
            ("blob", _) => (Kind::Blob, read(&e.file)?),
            (other, _) => return Err(format!("{}: unknown asset kind `{other}`", path.display())),
        };
        out.push((entry.0, e.id, entry.1));
    }
    for g in groups {
        out.push((
            Kind::Group,
            trino_core::bundle::group_id(&g),
            g.into_bytes(),
        ));
    }
    Ok(out)
}

/// `build web`.
pub fn build(project: &Project, opts: &BuildOptions) -> ExitCode {
    match build_inner(project, opts) {
        Ok(out) => {
            println!("[web] ready: {}", out.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("[web] {e}");
            ExitCode::FAILURE
        }
    }
}

pub fn build_inner(project: &Project, opts: &BuildOptions) -> Result<PathBuf, String> {
    ensure_target()?;
    let bindgen = ensure_bindgen(project)?;
    let out = opts
        .out
        .clone()
        .unwrap_or_else(|| project.root.join("target/web"));
    std::fs::create_dir_all(out.join("assets")).map_err(|e| format!("{}: {e}", out.display()))?;

    // Assets: the project's, plus Solid Eleven's own for the engine.
    let bytes = pack_assets(
        &project.root.join("assets"),
        &project.root.join("target/assets/web"),
        &out.join("assets/trino.pak"),
        project.is_engine,
    )?;
    println!("[web] assets/trino.pak: {bytes} bytes");
    if project.is_engine {
        let se = project.root.join("examples/solid-eleven");
        let bytes = pack_assets(
            &se.join("assets"),
            &project.root.join("target/solid-eleven/assets/web"),
            &out.join("assets/solid-eleven.pak"),
            false,
        )?;
        println!("[web] assets/solid-eleven.pak: {bytes} bytes");
    }

    // The wasm module.
    let profile = if opts.release { "release" } else { "debug" };
    let manifest = project.root.join("Cargo.toml");
    let mut cmd = Command::new(cargo_bin());
    cmd.args(["build", "--target", TARGET, "-p", &project.web_package])
        .current_dir(&project.root);
    if !project.is_engine {
        cmd.arg("--manifest-path").arg(&manifest);
    }
    if opts.release {
        cmd.arg("--release");
    }
    cmd.args(project.trace_features());
    let ok = cmd.status().is_ok_and(|s| s.success());
    if !ok {
        return Err(format!(
            "cargo build -p {} --target {TARGET} failed",
            project.web_package
        ));
    }
    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| project.root.join("target"));
    let wasm = target_dir
        .join(TARGET)
        .join(profile)
        .join(format!("{}.wasm", project.web_package.replace('-', "_")));
    let ok = Command::new(&bindgen)
        .args([
            "--target",
            "web",
            "--no-typescript",
            "--out-name",
            "trino",
            "--out-dir",
        ])
        .arg(&out)
        .arg(&wasm)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        return Err(format!("wasm-bindgen failed on {}", wasm.display()));
    }
    // The issue's layout: trino.js + trino.wasm (the page passes the
    // module's path to `init`).
    let bg = out.join("trino_bg.wasm");
    let dest = out.join("trino.wasm");
    std::fs::rename(&bg, &dest).map_err(|e| format!("{}: {e}", bg.display()))?;

    // The page and the KTX2 transcoder.
    let template = std::fs::read_to_string(project.engine.join("apps/web/index.html"))
        .map_err(|e| format!("apps/web/index.html: {e}"))?;
    let page = template
        .replace("{{TITLE}}", &html_escape(&project.web_title))
        .replace("{{CANVAS_ID}}", &project.web_canvas_id);
    std::fs::write(out.join("index.html"), page).map_err(|e| e.to_string())?;
    let basis = out.join("basis");
    std::fs::create_dir_all(&basis).map_err(|e| e.to_string())?;
    for f in ["basis_transcoder.js", "basis_transcoder.wasm"] {
        std::fs::copy(
            project.engine.join("apps/web/vendor/basis").join(f),
            basis.join(f),
        )
        .map_err(|e| format!("vendor/basis/{f}: {e}"))?;
    }
    let size = std::fs::metadata(&dest).map_or(0, |m| m.len());
    println!("[web] trino.wasm: {size} bytes ({profile})");
    Ok(out)
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A file's MIME type by extension (`application/wasm` matters: browsers
/// refuse to stream-compile anything else).
pub fn mime(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "wasm" => "application/wasm",
        "json" => "application/json",
        "css" => "text/css",
        "png" => "image/png",
        "ktx2" => "image/ktx2",
        // Asset group bundles and packs: bytes.
        _ => "application/octet-stream",
    }
}

/// Serve `dir` on 127.0.0.1:`port` (0 = any free port) on a background
/// thread. Returns the port. GET and HEAD, no directory listings, nothing
/// outside `dir`; every response carries COOP/COEP (a cross-origin
/// isolated page, what threads and precise timers would need) and
/// `no-cache` (a rebuild shows on reload).
pub fn serve(dir: PathBuf, port: u16) -> Result<u16, String> {
    let listener = (port..=port.saturating_add(if port == 0 { 0 } else { 20 }))
        .find_map(|p| TcpListener::bind(("127.0.0.1", p)).ok())
        .ok_or_else(|| format!("no free port from {port}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let dir = dir.clone();
            std::thread::spawn(move || {
                let _ = handle(stream, &dir);
            });
        }
    });
    Ok(port)
}

fn handle(mut stream: TcpStream, dir: &Path) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    // Drain the headers.
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" || line == "\n" {
            break;
        }
    }
    let mut parts = request.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");
    let path = target.split(['?', '#']).next().unwrap_or("/");
    let rel = path.trim_start_matches('/');
    let rel = if rel.is_empty() || rel.ends_with('/') {
        format!("{rel}index.html")
    } else {
        rel.to_string()
    };
    let file = dir.join(&rel);
    let inside = !rel.split('/').any(|c| c == ".." || c.contains('\\'));
    let (status, body, kind) = match (method, inside, std::fs::read(&file)) {
        ("GET" | "HEAD", true, Ok(bytes)) => ("200 OK", bytes, mime(&file)),
        ("GET" | "HEAD", _, _) => ("404 Not Found", b"not found".to_vec(), "text/plain"),
        _ => ("405 Method Not Allowed", Vec::new(), "text/plain"),
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\
         Cross-Origin-Opener-Policy: same-origin\r\nCross-Origin-Embedder-Policy: require-corp\r\n\
         Cache-Control: no-cache\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    if method != "HEAD" {
        stream.write_all(&body)?;
    }
    stream.flush()?;
    // Let the client close first (no RST on unread request bodies).
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut sink = [0u8; 256];
    let _ = stream.read(&mut sink);
    Ok(())
}

/// Open `url` in the default browser.
fn open_browser(url: &str) {
    let status = if cfg!(windows) {
        Command::new("cmd").args(["/C", "start", "", url]).status()
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg(url).status()
    } else {
        Command::new("xdg-open").arg(url).status()
    };
    if !status.is_ok_and(|s| s.success()) {
        println!("[web] open {url} in a browser");
    }
}

/// `run web`.
pub fn run(project: &Project, opts: &BuildOptions, port: u16, query: &str) -> ExitCode {
    let out = match build_inner(project, opts) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[web] {e}");
            return ExitCode::FAILURE;
        }
    };
    let port = match serve(out.clone(), port) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[web] {e}");
            return ExitCode::FAILURE;
        }
    };
    let url = format!("http://127.0.0.1:{port}/{query}");
    println!("[web] serving {} at {url} (Ctrl+C stops)", out.display());
    if std::env::var_os("TRINO_NO_BROWSER").is_none() {
        open_browser(&url);
    }
    loop {
        std::thread::park();
    }
}

/// `test web` options.
#[derive(Clone, Debug)]
pub struct TestOptions {
    pub build: bool,
    pub bless: bool,
    pub headed: bool,
    pub browsers: Vec<String>,
}

/// The golden frames each browser must match, within a tolerance: mean
/// per-pixel difference and share of far pixels (`fbgolden::distance`).
/// Browsers rasterize, filter and resolve on their own GPU paths
/// (SwiftShader, llvmpipe, WebKit's), like Azahar on different hosts.
pub const GOLDENS: [&str; 3] = ["showcase", "castle64_hub", "texture_formats"];
pub const MAX_MEAN: f32 = 3.0;
pub const MAX_FAR: f32 = 0.02;

/// `test web`.
pub fn test(project: &Project, opts: &TestOptions) -> ExitCode {
    match test_inner(project, opts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("[web] {e}");
            ExitCode::FAILURE
        }
    }
}

fn test_inner(project: &Project, opts: &TestOptions) -> Result<(), String> {
    let web = project.root.join("target/web");
    if opts.build {
        build_inner(
            project,
            &BuildOptions {
                release: true,
                out: Some(web.clone()),
            },
        )?;
    } else if !web.join("trino.wasm").is_file() {
        return Err(format!("--no-build, but {} has no build", web.display()));
    }
    let harness = project.engine.join("tools/web-test");
    ensure_harness(&harness, &opts.browsers)?;
    let port = serve(web, 0)?;
    let out = project.root.join("target/web-test");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let mut cmd = harness_command(&project.root, &harness.join("run.mjs"));
    cmd.arg("--url")
        .arg(format!("http://127.0.0.1:{port}/"))
        .arg("--out")
        .arg(&out)
        .arg("--browsers")
        .arg(opts.browsers.join(","));
    if opts.headed {
        cmd.arg("--headed");
    }
    // A project has no engine self-test: its game runs N frames instead,
    // and the engine's goldens don't apply.
    if !project.is_engine {
        cmd.arg("--smoke");
    }
    let harness_ok = cmd.status().is_ok_and(|s| s.success());

    // The goldens, per browser run that produced frames.
    let results = std::fs::read_to_string(out.join("results.json"))
        .map_err(|e| format!("the harness wrote no results.json: {e}"))?;
    let results: serde_json::Value =
        serde_json::from_str(&results).map_err(|e| format!("results.json: {e}"))?;
    let mut failures = Vec::new();
    let runs = results["runs"].as_array().cloned().unwrap_or_default();
    // Bless from the first run that passed (Chromium on WebGL2 in CI's
    // order: the reference rasterizer is the one every browser is held to).
    let mut blessed = false;
    for run in &runs {
        let name = run["name"].as_str().unwrap_or("?").to_string();
        if run["skippedRun"].as_bool() == Some(true) {
            println!(
                "[web] {name}: skipped ({})",
                run["skipped"].as_str().unwrap_or("no WebGPU here")
            );
            continue;
        }
        if !run["pass"].as_bool().unwrap_or(false) {
            failures.push(format!(
                "{name}: {}",
                run["error"].as_str().unwrap_or("self-test failed")
            ));
            continue;
        }
        let goldens: &[&str] = if project.is_engine { &GOLDENS } else { &[] };
        for golden in goldens {
            let png = out.join(&name).join(format!("{golden}.png"));
            let img = match crate::fbgolden::read_png(&png) {
                Ok(i) => i,
                Err(e) => {
                    failures.push(format!("{name}: {golden}: {e}"));
                    continue;
                }
            };
            let bless = opts.bless && !blessed;
            match crate::fbgolden::check_close(
                &project.root,
                "web",
                golden,
                img,
                bless,
                MAX_MEAN,
                MAX_FAR,
            ) {
                Ok(msg) => println!("[web] {name}: {msg}"),
                Err(e) => failures.push(format!("{name}: {e}")),
            }
        }
        if opts.bless {
            blessed = true;
        }
        // The run's trace (issue #62): the self-test's `trace` step, and
        // with --trace the engine's zones, from the page's console.
        let console =
            std::fs::read_to_string(out.join(&name).join("console.log")).unwrap_or_default();
        let mut traces = crate::trace::Capture::default();
        let mut rerecord = crate::replay::Rerecord::default();
        for line in console.lines() {
            if !rerecord.feed(line) {
                traces.feed(line);
            }
        }
        // The replay step's re-recording against the PC's file (issue #67).
        if project.is_engine {
            let file = out.join(&name).join("replay.trec");
            match rerecord.check(&project.engine, &file) {
                Ok(msg) => println!("[web] {name}: re-recording: {msg}"),
                Err(e) => failures.push(format!("{name}: {e}")),
            }
        }
        let file = format!("web-{name}{}", if project.trace { "-trace" } else { "" });
        match traces.finish(&project.engine, &file, project.is_engine) {
            Ok(msg) => println!("[web] {name}: trace: {msg}"),
            Err(e) => failures.push(format!("{name}: {e}")),
        }
    }
    if runs.is_empty() {
        failures.push("no browser ran".into());
    }
    if !harness_ok && failures.is_empty() {
        failures.push("the harness failed".into());
    }
    if failures.is_empty() {
        println!("[web] every browser passed ({} runs)", runs.len());
        Ok(())
    } else {
        Err(format!("failures:\n  {}", failures.join("\n  ")))
    }
}

/// A `node` command for one of the Playwright scripts in `tools/web-test`
/// (`run.mjs`, `net.mjs`), with its temporary files under `root`'s
/// `target/web-tmp` (GitHub #107). Playwright launches Chromium with
/// `--disable-dev-shm-usage`, so the browser's shared memory and its profile
/// go to `TMPDIR`. Where `/tmp` is a tmpfs with under ~2 GB free, Chromium's
/// headless shell crashed its renderer ("Target crashed", an `int3` in the
/// Compositor thread) at the WebGL2 run's `asset groups` step; the build
/// tree is on a real disk.
pub fn harness_command(root: &Path, script: &Path) -> Command {
    let tmp = harness_tmp(root);
    let _ = std::fs::create_dir_all(&tmp);
    let mut cmd = Command::new("node");
    cmd.arg(script).env("TMPDIR", &tmp);
    cmd
}

/// Where [`harness_command`] points the browsers' `TMPDIR`. Not under
/// `target/web-test`: the test run wipes that folder first.
pub fn harness_tmp(root: &Path) -> PathBuf {
    root.join("target/web-tmp")
}

/// `tools/web-test` has its node modules and the browsers Playwright drives
/// (`npm ci`, `npx playwright install`).
pub fn ensure_harness(dir: &Path, browsers: &[String]) -> Result<(), String> {
    let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
    let npx = if cfg!(windows) { "npx.cmd" } else { "npx" };
    if Command::new("node").arg("--version").output().is_err() {
        return Err("`test web` needs Node.js (node, npm) for Playwright".into());
    }
    if !dir.join("node_modules/playwright").is_dir() {
        println!(
            "[web] installing the Playwright harness (npm ci in {})",
            dir.display()
        );
        let ok = Command::new(npm)
            .arg("ci")
            .current_dir(dir)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            return Err(format!("npm ci failed in {}", dir.display()));
        }
    }
    if std::env::var_os("TRINO_SKIP_BROWSER_INSTALL").is_none() {
        let ok = Command::new(npx)
            .args(["playwright", "install"])
            .args(browsers)
            .current_dir(dir)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            return Err("npx playwright install failed".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cli_version_comes_from_the_lockfile() {
        let v = bindgen_version(&crate::project::engine_root()).unwrap();
        assert!(v.starts_with("0.2."), "{v}");
    }

    /// GitHub #107: the browsers' temporary files (Chromium's shared memory
    /// under Playwright's `--disable-dev-shm-usage`, the profiles) go to the
    /// build tree, not to a tmpfs `/tmp` that may be nearly full.
    #[test]
    fn the_harness_keeps_the_browsers_temp_files_in_the_build_tree() {
        let root = tempfile::tempdir().unwrap();
        let cmd = harness_command(root.path(), Path::new("run.mjs"));
        let tmp = harness_tmp(root.path());
        assert_eq!(tmp, root.path().join("target/web-tmp"));
        assert!(tmp.is_dir(), "the folder exists before node starts");
        let env: Vec<_> = cmd.get_envs().collect();
        assert!(
            env.contains(&(std::ffi::OsStr::new("TMPDIR"), Some(tmp.as_os_str()))),
            "{env:?}"
        );
        assert_eq!(cmd.get_program(), "node");
    }

    #[test]
    fn wasm_and_js_get_the_types_browsers_require() {
        assert_eq!(mime(Path::new("a/trino.wasm")), "application/wasm");
        assert!(mime(Path::new("trino.js")).starts_with("text/javascript"));
        assert!(mime(Path::new("index.html")).starts_with("text/html"));
        assert_eq!(
            mime(Path::new("assets/trino.pak")),
            "application/octet-stream"
        );
    }

    #[test]
    fn the_server_serves_files_with_isolation_headers_and_nothing_outside() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "<p>hi</p>").unwrap();
        std::fs::write(dir.path().join("trino.wasm"), [0u8, 97, 115, 109]).unwrap();
        let port = serve(dir.path().to_path_buf(), 0).unwrap();
        let get = |path: &str| {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
            let mut out = Vec::new();
            s.read_to_end(&mut out).unwrap();
            String::from_utf8_lossy(&out).into_owned()
        };
        let index = get("/?game=castle64");
        assert!(index.starts_with("HTTP/1.1 200"), "{index}");
        assert!(index.contains("Cross-Origin-Embedder-Policy: require-corp"));
        assert!(index.ends_with("<p>hi</p>"));
        let wasm = get("/trino.wasm");
        assert!(wasm.contains("Content-Type: application/wasm"));
        assert!(get("/../Cargo.toml").starts_with("HTTP/1.1 404"));
        assert!(get("/missing.js").starts_with("HTTP/1.1 404"));
    }

    #[test]
    fn packs_hold_ktx2_textures_vorbis_sounds_and_the_mix() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("assets/shared")).unwrap();
        // A 16x16 opaque gradient, a sound in each codec and the mix.
        let png = root.join("assets/shared/g.png");
        let file = std::fs::File::create(&png).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), 16, 16);
        enc.set_color(png::ColorType::Rgba);
        let px: Vec<u8> = (0..256u32)
            .flat_map(|i| [(i % 16 * 16) as u8, (i / 16 * 16) as u8, 90, 255])
            .collect();
        enc.write_header().unwrap().write_image_data(&px).unwrap();
        let wav = root.join("assets/shared/tone.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 22_050,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&wav, spec).unwrap();
        for i in 0..22_050 {
            let t = i as f32 / 22_050.0;
            let s = (t * 440.0 * std::f32::consts::TAU).sin() * 0.5;
            w.write_sample((s * 32767.0) as i16).unwrap();
        }
        w.finalize().unwrap();
        std::fs::write(
            root.join("assets/manifest.toml"),
            "version = 1\n[sprites.g]\nfile = \"g.png\"\nformats = { web = \"KTX2-UASTC\" }\n\
             [sprites.raw]\nfile = \"g.png\"\nformats = { web = \"RGBA8\" }\n\
             [sounds.tone]\nfile = \"tone.wav\"\n\
             [sounds.tone_pcm]\nfile = \"tone.wav\"\ncodec = { web = \"pcm\" }\n\
             [music.song]\nfile = \"tone.wav\"\n\
             [groups.level]\n\
             [sprites.level_g]\nfile = \"g.png\"\ngroups = [\"level\"]\n",
        )
        .unwrap();
        std::fs::write(
            root.join("assets/audio.toml"),
            "version = 1\n[buses]\nmusic = 0.5\n",
        )
        .unwrap();
        let pak = root.join("out/trino.pak");
        pack_assets(&root.join("assets"), &root.join("baked"), &pak, true).unwrap();
        let bytes = std::fs::read(&pak).unwrap();
        let entries = pack::parse(&bytes).unwrap();
        let g = trino_core::asset_id("sprites/g");
        let raw = trino_core::asset_id("sprites/raw");
        let find = |id| entries.iter().find(|e| e.id == id).unwrap();
        assert_eq!(find(g).kind, Kind::SpriteKtx2);
        assert!(
            find(g)
                .bytes
                .starts_with(&trino_asset_pipeline::texture::ktx2::KTX2_MAGIC)
        );
        assert_eq!(find(raw).kind, Kind::Sprite);
        // Sounds: Vorbis by default (a second of PCM16 is 44 100 bytes),
        // PCM where asked; music streams; the mix rides along.
        let sound = |p: &str| {
            let e = find(trino_core::asset_id(p));
            let b = trino_core::SoundBlob::parse(e.bytes).unwrap();
            (e.kind, b.codec, b.stream, b.frames, e.bytes.len())
        };
        let (kind, codec, stream, frames, len) = sound("sounds/tone");
        assert_eq!(
            (kind, codec, stream, frames),
            (Kind::Sound, trino_core::SoundCodec::Vorbis, false, 22_050)
        );
        assert!(len < 44_100 / 4, "Vorbis: {len} bytes");
        let (_, codec, _, _, len) = sound("sounds/tone_pcm");
        assert_eq!((codec, len), (trino_core::SoundCodec::Pcm16, 20 + 44_100));
        let (kind, codec, stream, ..) = sound("music/song");
        assert_eq!(
            (kind, codec, stream),
            (Kind::Music, trino_core::SoundCodec::Vorbis, true)
        );
        let mix = find(trino_core::asset_id(trino_core::MixerConfig::LOGICAL));
        assert_eq!(mix.kind, Kind::Mixer);
        let cfg = trino_core::MixerConfig::parse(mix.bytes).unwrap();
        assert_eq!(cfg.volumes[trino_core::Bus::Music.index()], 0.5);
        // The fixtures: each mode's file and its expected decode.
        let fixture = find(trino_core::asset_id("web/ktx2_uastc"));
        let expected = find(trino_core::asset_id("web/ktx2_uastc.rgba"));
        assert_eq!(
            trino_asset_pipeline::texture::ktx2::decode(fixture.bytes).unwrap(),
            expected.bytes
        );
        // A group's assets stay out of the pack, which names the group;
        // its bundle sits beside the pack.
        let level = trino_core::asset_id("sprites/level_g");
        assert!(entries.iter().all(|e| e.id != level));
        let group = find(trino_core::bundle::group_id("level"));
        assert_eq!((group.kind, group.bytes), (Kind::Group, &b"level"[..]));
        let bundle = std::fs::read(root.join("out/trino.level.tbnd")).unwrap();
        let b = trino_core::bundle::Bundle::parse(&bundle).unwrap();
        assert_eq!(b.group(), trino_core::bundle::group_id("level"));
        assert!(b.entries().any(|e| e.id == level));
    }
}
