// Two pages playing online (issue #67): `cargo xtask net web` serves the
// build, runs a lobby and opens the same game twice in one browser, each
// page in its own context (no shared storage), as two players would:
//
//   node net.mjs --url http://127.0.0.1:PORT/ --lobby ws://127.0.0.1:WS \
//       --out target/web-net [--browser chromium] [--query "&net_script=..."]
//
// Each page meets the other through the lobby, opens a WebRTC data
// channel and plays under rollback; with `net_hash_at` both log
// `TRINO_NET_HASH:<frame>:<hash>`. Writes <out>/<seat>.log, <seat>.png and
// results.json; exit 1 when a page failed or the hashes differ.

import { chromium, firefox } from "playwright";
import fs from "node:fs";
import path from "node:path";

const args = process.argv.slice(2);
const opt = (name, dflt) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : dflt;
};
const base = opt("--url", "http://127.0.0.1:8080/");
const lobby = opt("--lobby", "ws://127.0.0.1:7401");
const out = opt("--out", "target/web-net");
const engine = { chromium, firefox }[opt("--browser", "chromium")];
const extra = opt("--query", "");
const room = `R${Date.now() % 100000}`;
const timeoutMs = Number(process.env.TRINO_WEB_TIMEOUT || 600) * 1000;
fs.mkdirSync(out, { recursive: true });

// Host candidates on this machine: no mDNS-hidden addresses.
const browser = await engine.launch({
  args: engine === chromium ? ["--disable-features=WebRtcHideLocalIpsWithMdns"] : [],
  firefoxUserPrefs: engine === firefox ? { "media.peerconnection.ice.obfuscate_host_addresses": false, "webgl.force-enabled": true } : undefined,
});
const pages = [];
for (const name of ["first", "second"]) {
  const context = await browser.newContext({ viewport: { width: 640, height: 480 }, deviceScaleFactor: 1 });
  const page = await context.newPage();
  const log = [];
  page.on("console", (m) => log.push(m.text()));
  page.on("pageerror", (e) => log.push(`[pageerror] ${e.message}`));
  const url = `${base}?game=solid-eleven&net=${room}&lobby=${encodeURIComponent(lobby)}&backend=webgl2&size=640x480${extra}`;
  await page.goto(url);
  pages.push({ name, page, log });
  // The second page a moment later, as a second player would come.
  await page.waitForTimeout(500);
}
const results = { room, pages: [] };
for (const p of pages) {
  let error = null;
  try {
    await p.page.waitForFunction(() => globalThis.__trino && globalThis.__trino.done, null, {
      timeout: timeoutMs,
      polling: 250,
    });
  } catch (e) {
    error = String(e.message || e).split("\n")[0];
  }
  const state = await p.page.evaluate(() => globalThis.__trino || {});
  await p.page.locator("canvas").screenshot({ path: path.join(out, `${p.name}.png`) }).catch(() => {});
  fs.writeFileSync(path.join(out, `${p.name}.log`), p.log.join("\n") + "\n");
  const hash = p.log.map((l) => l.match(/TRINO_NET_HASH:\d+:(0x[0-9a-f]+)/)).find(Boolean);
  const seat = p.log.map((l) => l.match(/TRINO_NET: player (\d) of/)).find(Boolean);
  results.pages.push({
    name: p.name,
    pass: !error && !!state.pass,
    error: error || state.error || null,
    seat: seat ? Number(seat[1]) : null,
    hash: hash ? hash[1] : null,
    net: (state.info || {}).net || null,
    lines: p.log.filter((l) => l.includes("TRINO_NET")),
  });
}
await browser.close();
const [a, b] = results.pages;
results.same = !!a.hash && a.hash === b.hash;
fs.writeFileSync(path.join(out, "results.json"), JSON.stringify(results, null, 2));
for (const p of results.pages) {
  console.error(`[web-net] ${p.name}: player ${p.seat}, ${p.pass ? "PASS" : "FAIL"}${p.error ? ` · ${p.error}` : ""} · hash ${p.hash}`);
  for (const l of p.lines) console.error(`[web-net] ${p.name}: ${l}`);
}
console.error(`[web-net] ${results.same ? "both pages played the same game" : "the pages differ"}`);
process.exit(a.pass && b.pass && (results.same || !extra.includes("net_hash_at")) ? 0 : 1);
