// The JavaScript side of trino-platform-web (wasm-bindgen copies it into
// the build's `snippets/`): the KTX2 transcoder, the page's log and the
// test harness's view of a run (`globalThis.__trino`, read by
// `tools/web-test/run.mjs`).

const state = (globalThis.__trino = globalThis.__trino || {
  log: [],
  steps: [],
  done: false,
  pass: false,
  error: null,
  frames: {},
  info: {},
});

let basis = null;

// Load Binomial's transcoder (a classic script that defines `BASIS`) and
// its WebAssembly. Resolves to false when the files are missing: the
// engine then has no KTX2 and says so (`trino_basis_ready`).
export async function trino_init_basis(jsUrl, wasmUrl) {
  if (basis) return true;
  try {
    if (typeof globalThis.BASIS !== "function") {
      await new Promise((resolve, reject) => {
        const s = document.createElement("script");
        s.src = jsUrl;
        s.onload = resolve;
        s.onerror = () => reject(new Error("cannot load " + jsUrl));
        document.head.appendChild(s);
      });
    }
    const module = await globalThis.BASIS({ locateFile: () => wasmUrl });
    module.initializeBasis();
    basis = module;
    return true;
  } catch (e) {
    console.error("[trino] KTX2 transcoder unavailable:", e);
    return false;
  }
}

export function trino_basis_ready() {
  return basis !== null;
}

// Transcode one KTX2 file (`bytes`, a Uint8Array) to Basis transcoder
// format `format` (0 ETC1, 1 ETC2 RGBA, 2 BC1, 3 BC3, 6 BC7, 10 ASTC 4x4,
// 13 RGBA32). Returns [width, height, hasAlpha, data] or throws.
export function trino_transcode_ktx2(bytes, format) {
  if (!basis) throw new Error("KTX2 transcoder not loaded");
  const file = new basis.KTX2File(bytes);
  try {
    if (!file.isValid()) throw new Error("invalid KTX2 file");
    const w = file.getWidth();
    const h = file.getHeight();
    const alpha = !!file.getHasAlpha();
    if (!file.startTranscoding()) throw new Error("startTranscoding failed");
    const size = file.getImageTranscodedSizeInBytes(0, 0, 0, format);
    const out = new Uint8Array(size);
    if (!file.transcodeImage(out, 0, 0, 0, format, 0, -1, -1)) {
      throw new Error("transcodeImage failed for format " + format);
    }
    return [w, h, alpha, out];
  } finally {
    file.close();
    file.delete();
  }
}

// One log line: the browser console (styled like the terminal's tags), the
// harness's list and the page's log panel.
export function trino_log(line, level) {
  state.log.push(line);
  if (line.startsWith("TRINO_STEP:")) state.steps.push(line.slice(11));
  const tag = line.startsWith("TRINO_TEST_FAIL") || level === 2
    ? "color:#fff;background:#c0392b;padding:0 4px;border-radius:2px"
    : line.startsWith("TRINO_")
      ? "color:#111;background:#9b59b6;padding:0 4px;border-radius:2px"
      : "color:#9b59b6";
  if (level === 2) console.error("%ctrino%c " + line, tag, "");
  else if (level === 1) console.warn("%ctrino%c " + line, tag, "");
  else console.log("%ctrino%c " + line, tag, "");
  const panel = document.getElementById("trino-log");
  if (panel) {
    panel.textContent += line + "\n";
    panel.scrollTop = panel.scrollHeight;
  }
}

// The run's end (self-test pass/fail, a panic).
export function trino_finish(pass, error) {
  state.pass = pass;
  if (error) state.error = error;
  state.done = true;
  document.documentElement.dataset.trino = pass ? "pass" : "fail";
}

// Facts the harness reports (backend, adapter, texture formats...).
export function trino_info(key, value) {
  state.info[key] = value;
}

// A frame read back from the GPU (RGBA8, `width`×`height`), kept as a PNG
// data URL under `name` for the harness's golden comparison.
export function trino_frame(name, width, height, rgba) {
  const c = document.createElement("canvas");
  c.width = width;
  c.height = height;
  const ctx = c.getContext("2d");
  const img = new ImageData(new Uint8ClampedArray(rgba), width, height);
  ctx.putImageData(img, 0, 0);
  state.frames[name] = c.toDataURL("image/png");
}

// The canvas's unsafe borders, CSS pixels `[left, top, right, bottom]`: a
// phone's notch and home bar (`env(safe-area-inset-*)`, read through a
// hidden probe; the page sets `viewport-fit=cover`), less the part outside
// the canvas.
let safeProbe = null;
export function trino_safe_area(canvas) {
  if (!safeProbe) {
    safeProbe = document.createElement("div");
    safeProbe.style.cssText =
      "position:fixed;left:0;top:0;visibility:hidden;pointer-events:none;" +
      "padding:env(safe-area-inset-top) env(safe-area-inset-right) " +
      "env(safe-area-inset-bottom) env(safe-area-inset-left)";
    document.body.appendChild(safeProbe);
  }
  const s = getComputedStyle(safeProbe);
  const px = (v) => parseFloat(v) || 0;
  const r = canvas.getBoundingClientRect();
  return [
    Math.max(0, px(s.paddingLeft) - r.left),
    Math.max(0, px(s.paddingTop) - r.top),
    Math.max(0, px(s.paddingRight) - (window.innerWidth - r.right)),
    Math.max(0, px(s.paddingBottom) - (window.innerHeight - r.bottom)),
  ];
}

// Show or hide the "click to enable sound" banner.
export function trino_audio_banner(show) {
  const el = document.getElementById("trino-audio");
  if (el) el.hidden = !show;
}

// A fatal error over the canvas.
export function trino_error_overlay(text) {
  const el = document.getElementById("trino-error");
  if (el) {
    el.textContent = text;
    el.hidden = false;
  }
}
