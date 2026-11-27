// Browser smoke test for the web app: starts a `trunk build` in headless Chrome, offline, and
// works it from the keyboard as a user would. Add Data from Command Search, a command, Undo and
// Redo, Save (a download), then a fresh page that opens the saved file and saves it again. It
// fails on any page error, console error or panic, and on a saved project that doesn't hold what
// was done. CI compiles the browser build; this runs it, where a clock or blocking call that only
// fails at run time shows (AGENTS.md › Never break wasm).
//
//   cd apps/rustgis-web && trunk build --release && cargo xtask web-pwa ../../dist/web
//   node smoke/run.mjs [--dist ../../dist/web] [--chrome /path/to/chrome] [--limit ms] [--keep]
//
// (CI packages its dev build instead; see the `wasm` job in .github/workflows/ci.yml.)
//
// No packages: Node's standard library, and Chrome driven through its DevTools protocol on a pipe.
// The app's interface is drawn on a canvas, so the test uses only what doesn't move when the
// interface is rearranged: shortcuts, Command Search by name, the browser's file chooser and
// downloads. Nothing waits a fixed time; each step waits for the event it causes, and the project
// state is read from saved copies.

import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, existsSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, normalize, extname, dirname, sep } from "node:path";
import { fileURLToPath } from "node:url";

const args = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  if (i < 0) return fallback;
  if (!args[i + 1] || args[i + 1].startsWith("--")) throw new Error(`--${name} needs a value`);
  return args[i + 1];
};
const here = dirname(fileURLToPath(import.meta.url));
const dist = resolve(opt("dist", join(here, "../../../dist/web")));
const chrome = opt("chrome", process.env.CHROME || "google-chrome");
// Each wait gives up after this long; the whole run takes a few seconds on a laptop.
const LIMIT_MS = Number(opt("limit", "120000"));
if (!existsSync(join(dist, "index.html"))) throw new Error(`${dist}: no index.html; build the app with trunk first`);

const work = mkdtempSync(join(tmpdir(), "rustgis-smoke-"));
const downloads = join(work, "downloads");
mkdirSync(downloads);
const failures = [];
let crashed = false;
const external = new Set();
const step = (s) => console.log(`smoke: ${s}`);

// The job's data (the W1 workflow's, crates/engine/tests/integration/workflows.rs): parcel ids
// with leading zeros in a layer and in a table, which must stay text in the browser too.
const WEST = -90.05, SOUTH = 35.1, STEP = 0.005;
const rect = (w, s, e, n) => ({ type: "Polygon", coordinates: [[[w, s], [e, s], [e, n], [w, n], [w, s]]] });
const id = (k) => String(101 + k).padStart(5, "0");
const parcels = [...Array(12).keys()].map((k) => {
  const [w, s] = [WEST + (k % 4) * STEP, SOUTH + Math.floor(k / 4) * STEP];
  return { type: "Feature", properties: { parcel_id: id(k) }, geometry: rect(w, s, w + STEP, s + STEP) };
});
const zoning = [
  { type: "Feature", properties: { zone: "R1" }, geometry: rect(WEST, SOUTH, WEST + 2.5 * STEP, SOUTH + 3 * STEP) },
  { type: "Feature", properties: { zone: "C2" }, geometry: rect(WEST + 2.5 * STEP, SOUTH, WEST + 4 * STEP, SOUTH + 2 * STEP) },
];
const fc = (features) => JSON.stringify({ type: "FeatureCollection", features });
const inputs = {
  "parcels.geojson": fc(parcels),
  "zoning.geojson": fc(zoning),
  "assessments.csv": "parcel_id,value_usd\n" + [0, 1, 3].map((k) => `${id(k)},${100000 + k * 1000}\n`).join(""),
};
for (const [name, text] of Object.entries(inputs)) writeFileSync(join(work, name), text);

// The build, served as a web server would: on the loopback, so the page is a secure context and
// its service worker installs.
const TYPES = { ".html": "text/html", ".js": "text/javascript", ".wasm": "application/wasm", ".json": "application/json",
  ".webmanifest": "application/manifest+json", ".txt": "text/plain", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" };
const server = createServer((req, res) => {
  let path;
  try {
    path = normalize(decodeURIComponent(new URL(req.url, "http://x").pathname)).replace(/^\/+/, "");
  } catch {
    res.writeHead(400).end();
    return;
  }
  const file = join(dist, path || "index.html");
  if (!file.startsWith(dist + sep) || !existsSync(file) || !statSync(file).isFile()) {
    res.writeHead(404).end();
    return;
  }
  res.writeHead(200, { "content-type": TYPES[extname(file)] ?? "application/octet-stream", "cache-control": "no-store" });
  res.end(readFileSync(file));
});
await new Promise((ok) => server.listen(0, "127.0.0.1", ok));
const origin = `http://127.0.0.1:${server.address().port}`;

// Chrome, headless, offline: every host but the loopback fails to resolve. WebGL2 through
// SwiftShader, as on a machine without a GPU; WebGPU finds no adapter and the app falls back.
const browser = spawn(chrome, [
  "--headless", "--remote-debugging-pipe", `--user-data-dir=${join(work, "profile")}`, "--no-first-run",
  "--no-default-browser-check", "--no-sandbox", "--window-size=1400,900", "--use-angle=swiftshader",
  "--enable-unsafe-swiftshader", "--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1", "about:blank",
], { stdio: ["ignore", "ignore", "ignore", "pipe", "pipe"] });
browser.on("error", (e) => {
  console.error(`smoke: FAILED: could not start ${chrome}: ${e.message}`);
  process.exit(1);
});
const exited = new Promise((ok) => browser.on("exit", ok));

// The DevTools protocol: JSON messages, each ended by a NUL byte, on file descriptors 3 and 4.
let next = 0, buffer = "";
const calls = new Map();
const listeners = new Set();
browser.stdio[4].on("data", (chunk) => {
  buffer += chunk.toString();
  let end;
  while ((end = buffer.indexOf("\0")) >= 0) {
    const msg = JSON.parse(buffer.slice(0, end));
    buffer = buffer.slice(end + 1);
    if (msg.id !== undefined) {
      const call = calls.get(msg.id);
      calls.delete(msg.id);
      if (msg.error) call?.fail(new Error(`${call.method}: ${msg.error.message}`));
      else call?.ok(msg.result);
    } else for (const l of [...listeners]) l(msg);
  }
});
const send = (method, params = {}, sessionId) =>
  new Promise((ok, fail) => {
    const id = ++next;
    calls.set(id, { ok, fail, method });
    browser.stdio[3].write(JSON.stringify({ id, method, params, sessionId }) + "\0");
  });
// The next event `method` for which `test` holds, or an error naming `what` after the limit, or
// as soon as the page throws (a panic ends the app, so nothing more will come). Its error is kept
// until the caller awaits it, which may be after the steps that cause the event.
const event = (method, what, test = () => true) => {
  const waiting = new Promise((ok, fail) => {
    if (crashed) return fail(new Error(`no ${what}: the page stopped with an error`));
    const done = () => {
      clearTimeout(timer);
      listeners.delete(l);
    };
    const timer = setTimeout(() => {
      done();
      fail(new Error(`no ${what} within ${LIMIT_MS / 1000} s`));
    }, LIMIT_MS);
    const l = (msg) => {
      if (msg.method === method && test(msg.params)) {
        done();
        ok(msg.params);
      } else if (msg.method === "Runtime.exceptionThrown") {
        done();
        fail(new Error(`no ${what}: the page stopped with an error`));
      }
    };
    listeners.add(l);
  });
  waiting.catch(() => {});
  return waiting;
};

const local = (url) => !url || url.startsWith(origin) || /^(data|blob|about|chrome-error):/.test(url);
listeners.add(({ method, params }) => {
  if (method === "Runtime.exceptionThrown") {
    crashed = true;
    const d = params.exceptionDetails;
    failures.push(`page error: ${d.exception?.description ?? d.text}`);
  } else if (method === "Runtime.consoleAPICalled" && ["error", "assert"].includes(params.type)) {
    failures.push(`console.${params.type}: ${params.args.map((a) => a.value ?? a.description ?? "").join(" ")}`);
  } else if (method === "Log.entryAdded" && params.entry.level === "error") {
    // The page's images from the asset host can't load offline; that's expected, and listed.
    if (params.entry.source === "network" && !local(params.entry.url)) external.add(params.entry.url);
    else failures.push(`browser error: ${params.entry.text} ${params.entry.url ?? ""}`.trim());
  }
});

let session;
const js = async (expression) => {
  const r = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true }, session);
  if (r.exceptionDetails) throw new Error(`${expression}: ${r.exceptionDetails.text}`);
  return r.result.value;
};
// Two animation frames: the app has handled the input before and drawn the result.
const frames = () => js("new Promise((ok) => requestAnimationFrame(() => requestAnimationFrame(() => ok(true))))");
const KEYS = { Enter: 13, Escape: 27, k: 75, o: 79, s: 83, y: 89, z: 90 };
const press = async (key, ctrl = false) => {
  const code = key.length === 1 ? `Key${key.toUpperCase()}` : key;
  const common = { key, code, windowsVirtualKeyCode: KEYS[key], modifiers: ctrl ? 2 : 0 };
  await send("Input.dispatchKeyEvent", { type: "rawKeyDown", ...common, ...(key === "Enter" ? { text: "\r" } : {}) }, session);
  await send("Input.dispatchKeyEvent", { type: "keyUp", ...common }, session);
  await frames();
};
const type = async (text) => {
  for (const ch of text) {
    const code = /[a-z]/i.test(ch) ? `Key${ch.toUpperCase()}` : ch === " " ? "Space" : "";
    await send("Input.dispatchKeyEvent", { type: "keyDown", key: ch, code, text: ch }, session);
    await send("Input.dispatchKeyEvent", { type: "keyUp", key: ch, code }, session);
  }
  await frames();
};
// Runs a command from Command Search (Ctrl+K), by the name it shows there.
const search = async (name) => {
  await press("k", true);
  await type(name);
  await press("Enter");
};
const choose = async (opened, files) => {
  const chooser = await opened;
  await send("DOM.setFileInputFiles", { files, backendNodeId: chooser.backendNodeId }, session);
};

// Opens the app on a fresh page load and waits until it has started.
const start = async () => {
  const loaded = event("Page.loadEventFired", "page load");
  await send("Page.navigate", { url: `${origin}/?blank` }, session);
  await loaded;
  const deadline = Date.now() + LIMIT_MS;
  for (;;) {
    const state = await js(`(() => { const l = document.getElementById("rustgis_loading"); return l ? l.textContent : null })()`);
    if (state === null) break;
    if (state.includes("failed to start")) throw new Error(`the app didn't start: ${state.trim()}`);
    if (Date.now() > deadline) throw new Error("the app didn't start: still loading");
    await frames();
  }
  await js(`document.getElementById("rustgis_canvas").focus(), true`);
  await frames();
};

// Downloads that have finished, by id: a small one can finish before its start is seen.
const finished = new Set();
listeners.add(({ method, params }) => {
  if (method === "Browser.downloadProgress" && params.state === "completed") finished.add(params.guid);
});
// Saves the project (Ctrl+S, then Save in the Save As dialog) and returns the downloaded file.
let saves = 0;
const save = async () => {
  const begun = event("Browser.downloadWillBegin", "download after Save");
  await press("s", true);
  await press("Enter");
  const { guid, suggestedFilename } = await begun;
  if (!finished.has(guid)) await event("Browser.downloadProgress", "finished download", (p) => p.guid === guid && p.state === "completed");
  const file = join(downloads, `save-${++saves}-${suggestedFilename.replace(/[^\w.-]/g, "_")}`);
  writeFileSync(file, readFileSync(join(downloads, guid)));
  return { file, project: JSON.parse(readFileSync(file, "utf8")) };
};
// Saves until the project passes `ready`: the app reads files and runs work between frames, so a
// save shows when it has landed. Fails after the limit, saying what the last save held.
const saveUntil = async (what, ready) => {
  const deadline = Date.now() + LIMIT_MS;
  for (;;) {
    const saved = await save();
    if (ready(saved.project)) return saved;
    if (Date.now() > deadline) throw new Error(`${what}: the saved project has layers ${layerNames(saved.project)}`);
    await frames();
  }
};
const layerNames = (p) => JSON.stringify((p.maps?.[0]?.layers ?? []).map((l) => l.name));
const value = (cell) => (cell && typeof cell === "object" ? Object.values(cell)[0] : cell);

try {
  await send("Browser.setDownloadBehavior", { behavior: "allowAndName", downloadPath: downloads, eventsEnabled: true });
  const { targetId } = await send("Target.createTarget", { url: "about:blank" });
  ({ sessionId: session } = await send("Target.attachToTarget", { targetId, flatten: true }));
  for (const domain of ["Page", "Runtime", "Log"]) await send(`${domain}.enable`, {}, session);
  await send("Page.setInterceptFileChooserDialog", { enabled: true }, session);
  // Leaving a page with unsaved work asks first; the test says yes.
  listeners.add(({ method, sessionId }) => {
    if (method === "Page.javascriptDialogOpening") send("Page.handleJavaScriptDialog", { accept: true }, sessionId);
  });

  step("starting the app");
  await start();

  step("Add Data: two layers and a table through the file chooser");
  const picked = event("Page.fileChooserOpened", "file chooser from Add Data");
  await search("Add Data");
  await choose(picked, Object.keys(inputs).map((f) => join(work, f)));
  const added = await saveUntil("Add Data", (p) => p.maps?.[0]?.layers?.length === 2 && p.tables?.length === 1);
  const layers = layerNames(added.project);
  const table = added.project.tables[0];
  const ids = table.features.map((f) => value(f.attrs[0]));
  if (JSON.stringify(ids) !== JSON.stringify(["00101", "00102", "00104"])) failures.push(`the table's parcel ids lost their zeros: ${ids}`);
  const parcelLayer = added.project.maps[0].layers.find((l) => l.name === "parcels");
  if (parcelLayer?.dataset?.features?.length !== 12) failures.push(`parcels has ${parcelLayer?.dataset?.features?.length} features, not 12`);

  // Move Down moves the active layer: the last one added, zoning, on top (the shell reads picked
  // files in order). These commands run in the frame that takes the key, so one save shows each.
  step("Move Down from Command Search, then Undo and Redo");
  const order = (p) => layerNames(p);
  const want = JSON.stringify(["parcels", "zoning"]);
  if (layers !== JSON.stringify(["zoning", "parcels"])) throw new Error(`Add Data left the layers ${layers}, not zoning on top`);
  const after = async (what, expected) => {
    const saved = await save();
    if (order(saved.project) !== expected) throw new Error(`${what}: the layers are ${order(saved.project)}, not ${expected}`);
    return saved;
  };
  await search("Move Down");
  await after("Move Down", want);
  await press("z", true);
  await after("Undo", layers);
  await press("y", true);
  const redone = await after("Redo", want);

  step("a fresh page opens the saved project, which saves the same");
  await start();
  const opened = event("Page.fileChooserOpened", "file chooser from Open (Ctrl+O)");
  await press("o", true);
  await choose(opened, [redone.file]);
  const again = await saveUntil("Open", (p) => order(p) === order(redone.project));
  const data = (p) => JSON.stringify([p.maps.map((m) => m.layers.map((l) => [l.name, l.dataset])), p.tables]);
  if (data(again.project) !== data(redone.project)) failures.push("the reopened project's layers or tables differ from the saved ones");
  step(`${saves} saves; layers ${order(again.project)}`);
} catch (e) {
  failures.push(e.message);
} finally {
  browser.kill();
  await exited;
  server.close();
  if (!args.includes("--keep")) rmSync(work, { recursive: true, force: true });
  else step(`kept ${work}`);
}

if (external.size) step(`left offline: ${[...external].join(", ")}`);
if (failures.length) {
  for (const f of failures) console.error(`smoke: FAILED: ${f}`);
  process.exit(1);
}
step("passed");
