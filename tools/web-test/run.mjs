// The Web target's browser harness (`cargo xtask test web`, issue #47).
//
// One process, every engine: for each browser it opens the build xtask
// serves, runs the self-test page (`?selftest`) and waits for its verdict
// in `globalThis.__trino` (written by trino-platform-web's glue.js), then
// checks what only a browser can show: a save in localStorage survives a
// reload, a click unlocks audio, keys and the mouse reach the game. The
// self-test's `audio codecs` step needs the audio context running: the
// harness clicks the page once it is up, as a player's first click would
// (Chromium still runs with a gesture required).
//
//   node run.mjs --url http://127.0.0.1:PORT/ --out target/web-test \
//       [--browsers chromium,firefox,webkit] [--headed]
//
// Writes <out>/results.json, <out>/<run>/console.log and the golden frames
// <out>/<run>/<name>.png; xtask compares those with tests/golden/web/.
// Exit code 1 when a run fails.

import { chromium, firefox, webkit } from "playwright";
import fs from "node:fs";
import path from "node:path";

const args = process.argv.slice(2);
const opt = (name, dflt) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : dflt;
};
const base = opt("--url", "http://127.0.0.1:8080/");
const out = opt("--out", "target/web-test");
const browsers = opt("--browsers", "chromium,firefox,webkit").split(",");
const headed = args.includes("--headed");
// --smoke: a project without the engine's self-test (`?frames=N` runs its
// game and stops): no save or golden checks, the rest the same.
const smoke = args.includes("--smoke");
const timeoutMs = Number(process.env.TRINO_WEB_TIMEOUT || 900) * 1000;

// Chromium runs twice: WebGPU (its primary API; SwiftShader's Vulkan on a
// GPU-less runner) and WebGL2. Firefox and WebKit run WebGL2, the API
// every version of them has.
const plan = [];
for (const b of browsers) {
  if (b === "chromium") {
    plan.push({ name: "chromium-webgl2", browser: "chromium", backend: "webgl2", args: [] });
    // The full Chromium (not the headless shell, which has no WebGPU
    // adapter): the machine's GPU, or SwiftShader's Vulkan on a GPU-less
    // Linux runner.
    plan.push({
      name: "chromium-webgpu",
      browser: "chromium",
      backend: "webgpu",
      channel: "chromium",
      args: [
        "--enable-unsafe-webgpu",
        ...(process.platform === "linux"
          ? // ANGLE on SwiftShader too: with `--disable-vulkan-surface` the canvas
            // swap chain found no shared-image backing, and Chromium destroyed the
            // device after a few hundred frames.
            ["--enable-features=Vulkan", "--use-angle=swiftshader", "--use-vulkan=swiftshader", "--use-webgpu-adapter=swiftshader"]
          : []),
      ],
    });
  } else if (b === "firefox") {
    plan.push({ name: "firefox-webgl2", browser: "firefox", backend: "webgl2" });
  } else if (b === "webkit") {
    plan.push({ name: "webkit-webgl2", browser: "webkit", backend: "webgl2" });
  } else {
    console.error(`unknown browser ${b}`);
    process.exit(2);
  }
}

const engines = { chromium, firefox, webkit };

fs.mkdirSync(out, { recursive: true });
const results = { url: base, runs: [] };

for (const run of plan) {
  const dir = path.join(out, run.name);
  fs.mkdirSync(dir, { recursive: true });
  const started = Date.now();
  const r = { name: run.name, browser: run.browser, backend: run.backend, pass: false };
  const log = [];
  let browser;
  try {
    browser = await engines[run.browser].launch({
      // Headless Firefox on a GPU-less Linux has no WebGL at all ("Exhausted
      // GL driver options"); headed under Xvfb it gets Mesa's llvmpipe. CI
      // sets TRINO_WEB_FIREFOX_HEADED=1 and runs the harness in xvfb-run.
      headless: !headed && !(run.browser === "firefox" && process.env.TRINO_WEB_FIREFOX_HEADED === "1"),
      // Chromium: audio waits for a gesture, as on a real page.
      args: run.browser === "chromium" ? [...run.args, "--autoplay-policy=user-gesture-required"] : [],
      channel: run.channel,
      firefoxUserPrefs:
        run.browser === "firefox"
          ? { "webgl.force-enabled": true, "media.autoplay.default": 1 }
          : undefined,
    });
    r.version = browser.version();
    const context = await browser.newContext({ viewport: { width: 800, height: 600 }, deviceScaleFactor: 1 });
    const page = await context.newPage();
    page.on("console", (m) => log.push(`[${m.type()}] ${m.text()}`));
    page.on("pageerror", (e) => log.push(`[pageerror] ${e.message}`));

    // 1) The self-test.
    const first = smoke ? "?frames=120" : "?selftest";
    await page.goto(`${base}${first}&backend=${run.backend}&size=640x480`);
    if (!smoke) {
      // The gesture that lets audio run (`audio codecs` waits for it).
      await page
        .waitForFunction(() => globalThis.__trino && globalThis.__trino.info && globalThis.__trino.info.audio, null, {
          timeout: 120000,
        })
        .then(() => page.mouse.click(4, 4))
        .catch(() => {});
    }
    await page.waitForFunction(() => globalThis.__trino && globalThis.__trino.done, null, {
      timeout: timeoutMs,
      polling: 500,
    });
    const state = await page.evaluate(() => globalThis.__trino);
    r.pass = state.pass;
    r.error = state.error;
    r.steps = state.steps;
    r.info = state.info;
    // `audio codecs` (#53): its verdict and the backend's counters.
    r.audio_codecs = (state.steps || []).find((l) => l.startsWith("audio codecs ok") || l.startsWith("audio codecs skipped"));
    r.audio_lines = (state.log || []).filter((l) => l.startsWith("TRINO_AUDIO:"));
    for (const [name, url] of Object.entries(state.frames || {})) {
      const png = Buffer.from(url.split(",")[1], "base64");
      fs.writeFileSync(path.join(dir, `${name}.png`), png);
    }
    r.frames = Object.keys(state.frames || {});
    // Only the renderer's own "no adapter" error: a panic inside wgpu's WebGPU
    // backend (its path names "webgpu") is a failure, not a skip.
    const noAdapter = /no compatible GPU adapter/.test(state.error || "");
    if (!r.pass && run.backend === "webgpu" && noAdapter) {
      // A browser without a WebGPU adapter here (no GPU and no software
      // Vulkan): reported, not failed. The WebGL2 run covers the browser.
      r.skipped = `no WebGPU adapter: ${state.error}`;
    }

    if (r.pass) {
      // 2) The save the self-test wrote is still there after a reload,
      // and the game reads it (castle64 boots with it).
      const saved = await page.evaluate(() => localStorage.getItem("trino-save:selftest"));
      const game = smoke ? "" : "game=castle64&";
      await page.goto(`${base}?${game}backend=${run.backend}&size=640x480`);
      await page.waitForFunction(() => globalThis.__trino && globalThis.__trino.info.frames > 5, null, {
        timeout: 120000,
      });
      const after = await page.evaluate(() => localStorage.getItem("trino-save:selftest"));
      r.save_persisted = smoke || (saved !== null && saved === after);

      // 3) Audio: suspended until a gesture, running after a click.
      r.audio_before = await page.evaluate(() => globalThis.__trino.info.audio);
      await page.click("canvas", { position: { x: 320, y: 240 } });
      await page.waitForFunction(() => globalThis.__trino.info.audio === "running", null, { timeout: 10000 }).catch(() => {});
      r.audio_after = await page.evaluate(() => globalThis.__trino.info.audio);

      // 4) Input reaches the game: a held key, the stick, the pointer.
      await page.mouse.move(100, 120);
      await page.keyboard.down("KeyZ");
      await page.keyboard.down("KeyD");
      // Wait for every part: a frame between the two key presses shows
      // A with the stick still at rest.
      const inputOk = (i) => /A/.test(i || "") && /stick=1,0/.test(i || "") && /pointer=/.test(i || "");
      await page
        .waitForFunction(`(${inputOk})(globalThis.__trino.info.input)`, null, { timeout: 10000 })
        .catch(() => {});
      r.input_keys = await page.evaluate(() => globalThis.__trino.info.input);
      await page.keyboard.up("KeyZ");
      await page.keyboard.up("KeyD");
      r.input_ok = inputOk(r.input_keys);

      const extra = [];
      if (!r.save_persisted) extra.push("the save did not survive a reload");
      if (r.audio_before === "none") {
        // A browser build without Web Audio (Playwright's WebKit on
        // Windows): the backend runs silent, which the self-test covered.
        r.audio_note = "no Web Audio in this browser build: silent backend";
      } else if (r.audio_after !== "running") {
        extra.push(`audio did not unlock on a click (${r.audio_before} -> ${r.audio_after})`);
      }
      if (!r.input_ok) extra.push(`keys/pointer did not reach the game (${r.input_keys})`);
      if (extra.length) {
        r.pass = false;
        r.error = extra.join("; ");
      }
    }
    await context.close();
  } catch (e) {
    r.pass = false;
    r.error = String(e && e.message ? e.message : e).split("\n")[0];
    // The full Chromium missing (it has WebGPU, the headless shell not).
    if (run.backend === "webgpu" && !browser) r.skipped = `WebGPU run did not start: ${r.error}`;
  } finally {
    if (browser) await browser.close();
  }
  r.ms = Date.now() - started;
  fs.writeFileSync(path.join(dir, "console.log"), log.join("\n") + "\n");
  const verdict = r.pass ? "PASS" : r.skipped ? "SKIP" : "FAIL";
  console.log(`[web-test] ${run.name} (${r.version || "?"}): ${verdict} in ${(r.ms / 1000).toFixed(1)} s` +
    (r.info ? ` · ${r.info.adapter || ""} · textures ${r.info.texture_target || "?"}` : "") +
    (r.error ? ` · ${r.error}` : ""));
  if (r.audio_codecs) console.log(`[web-test] ${run.name}: ${r.audio_codecs}`);
  for (const l of r.audio_lines || []) console.log(`[web-test] ${run.name}: ${l}`);
  results.runs.push(r);
}

// A skipped WebGPU run is not a failure, and it carries no frames.
results.runs = results.runs.map((r) => (r.skipped ? { ...r, pass: true, frames: [], skippedRun: true } : r));
fs.writeFileSync(path.join(out, "results.json"), JSON.stringify(results, null, 2));
const failed = results.runs.filter((r) => !r.pass);
process.exit(failed.length ? 1 : 0);
