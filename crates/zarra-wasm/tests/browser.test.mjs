import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { readFile, readdir, mkdtemp, rm } from "node:fs/promises";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const crate = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const root = path.resolve(crate, "../..");
const fixtureRoot = path.join(root, "testdata/refpy.zarr");
const fixtureFiles = new Map();
for (const relative of ["zarr.json", "0/zarr.json", "0/c/0/0/0"]) {
  fixtureFiles.set(`refpy.zarr/${relative}`, await readFile(path.join(fixtureRoot, relative)));
}

const GOLDEN = {
  native: "530702bf84b0013219d9532b388558f49b6d4472b6925c55efd13360cafe6d86",
  overzoom: "54b40d91191cfdf791aba5c7568e8e6b6e323bc071e050c07c484ed290e00f4e",
  bands: "df00b51cfa6ac5cdc9e903dd3665c44d2e94f6854ba3189e60136c6b048fe188",
};

function listen(server) {
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => resolve(server.address().port));
  });
}

function close(server) {
  return new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
}

async function readBody(request) {
  const chunks = [];
  for await (const chunk of request) chunks.push(chunk);
  return Buffer.concat(chunks).toString("utf8");
}

const requests = [];
const signing = new Map();
const signBodies = [];
let behavior = null;
let ttlSeconds = 60;
let serverGeneration = "g1";
let appOrigin;

const storeServer = http.createServer(async (request, response) => {
  const url = new URL(request.url, "http://store.invalid");
  requests.push({ server: "store", method: request.method, path: url.pathname, range: request.headers.range ?? null, cookie: request.headers.cookie ?? null });
  response.setHeader("Access-Control-Allow-Origin", "*");
  response.setHeader("Access-Control-Allow-Methods", "GET, POST, OPTIONS");
  response.setHeader("Access-Control-Allow-Headers", "Content-Type, Range");
  response.setHeader("Access-Control-Expose-Headers", "Content-Range, Content-Length");
  if (request.method === "OPTIONS") {
    response.writeHead(204).end();
    return;
  }
  const signMatch = /^\/api\/layers\/([^/]+)\/sign$/.exec(url.pathname);
  if (signMatch && request.method === "POST") {
    const body = JSON.parse(await readBody(request));
    signBodies.push(body);
    if (!Array.isArray(body.paths) || body.paths.length !== 1 || Object.keys(body).some((key) => key !== "paths")) {
      response.writeHead(400).end("expected exactly one relative path");
      return;
    }
    const layer = decodeURIComponent(signMatch[1]);
    const relative = body.paths[0];
    const key = `${layer}/${relative}`;
    const generation = serverGeneration;
    const count = (signing.get(`${generation}/${key}`) ?? 0) + 1;
    signing.set(`${generation}/${key}`, count);
    const signed = `http://localhost:${storeServer.address().port}/object/${encodeURIComponent(key)}?generation=${encodeURIComponent(generation)}&token=secret-${count}`;
    response.setHeader("Content-Type", "application/json");
    response.setHeader("Set-Cookie", "store-cookie=must-not-be-sent");
    response.writeHead(200).end(JSON.stringify({
      generation: behavior?.wrongGeneration ? "wrong-generation" : generation,
      expires_at: Math.floor(Date.now() / 1000) + ttlSeconds,
      objects: [{ path: relative, url: signed }],
    }));
    return;
  }
  if (url.pathname === "/redirect") {
    response.writeHead(302, { Location: "/object/refpy.zarr%2Fzarr.json" }).end();
    return;
  }
  if (!url.pathname.startsWith("/object/") || request.method !== "GET") {
    response.writeHead(405).end();
    return;
  }
  const key = decodeURIComponent(url.pathname.slice("/object/".length));
  if (behavior?.httpsOnly) {
    response.writeHead(500).end();
    return;
  }
  if (behavior?.unauthorizedOnce && key.endsWith("/0/c/0/0/0")) {
    behavior = null;
    response.writeHead(401).end();
    return;
  }
  if (behavior?.forbiddenOnce && key.endsWith("/0/c/0/0/0")) {
    behavior = null;
    response.writeHead(403).end();
    return;
  }
  if (behavior?.status500 && key.endsWith("/0/c/0/0/0")) {
    response.writeHead(500, "signed-url-with-token-must-not-leak").end();
    return;
  }
  if (behavior?.missingShard && key.endsWith("/0/c/0/0/0")) {
    response.writeHead(404).end();
    return;
  }
  if (key === "empty.zarr/0/c/0/0/0") {
    response.writeHead(416, { "Content-Range": "bytes */0", "Content-Length": "0" }).end();
    return;
  }
  const bytes = fixtureFiles.get(key);
  if (!bytes) {
    response.writeHead(404).end();
    return;
  }
  const range = request.headers.range;
  if (!range) {
    response.setHeader("Content-Length", String(bytes.length));
    response.writeHead(200).end(bytes);
    return;
  }
  if (range.includes(",")) {
    response.writeHead(400).end("multipart ranges are forbidden");
    return;
  }
  const match = /^bytes=(\d+)-(\d*)$|^bytes=-(\d+)$/.exec(range);
  if (!match) {
    response.writeHead(400).end("bad test range");
    return;
  }
  let start;
  let end;
  if (match[3] !== undefined) {
    start = Math.max(0, bytes.length - Number(match[3]));
    end = bytes.length - 1;
  } else {
    start = Number(match[1]);
    end = match[2] === "" ? bytes.length - 1 : Number(match[2]);
  }
  if (start >= bytes.length || end < start) {
    response.writeHead(416, { "Content-Range": `bytes */${bytes.length}`, "Content-Length": "0" }).end();
    return;
  }
  end = Math.min(end, bytes.length - 1);
  let body = bytes.subarray(start, end + 1);
  let contentRange = `bytes ${start}-${end}/${bytes.length}`;
  let status = 206;
  if (behavior?.partial200) status = 200;
  if (behavior?.wrongRange) contentRange = `bytes 0-0/${bytes.length}`;
  if (behavior?.wrongLength) body = body.subarray(0, Math.max(0, body.length - 1));
  response.setHeader("Content-Range", contentRange);
  response.setHeader("Content-Length", String(body.length));
  response.writeHead(status).end(body);
});

const appServer = http.createServer(async (request, response) => {
  const url = new URL(request.url, "http://app.invalid");
  requests.push({ server: "app", method: request.method, path: url.pathname });
  if (url.pathname === "/") {
    response.setHeader("Content-Type", "text/html");
    response.writeHead(200).end("<!doctype html><title>zarra wasm test</title>");
    return;
  }
  let base;
  if (url.pathname.startsWith("/js/")) base = path.join(crate, "js");
  else if (url.pathname.startsWith("/pkg/")) base = path.join(crate, "pkg");
  else {
    response.writeHead(404).end();
    return;
  }
  const relative = decodeURIComponent(url.pathname.split("/").slice(2).join("/"));
  const filename = path.resolve(base, relative);
  if (!filename.startsWith(`${path.resolve(base)}/`)) {
    response.writeHead(403).end();
    return;
  }
  try {
    const body = await readFile(filename);
    const types = { ".js": "text/javascript", ".wasm": "application/wasm", ".map": "application/json" };
    response.setHeader("Content-Type", types[path.extname(filename)] ?? "application/octet-stream");
    response.writeHead(200).end(body);
  } catch {
    response.writeHead(404).end();
  }
});

function chromeExecutable() {
  if (process.env.CHROME_BIN) return process.env.CHROME_BIN;
  for (const candidate of ["/usr/bin/google-chrome", "/usr/bin/chromium"]) {
    if (existsSync(candidate)) return candidate;
  }
  const cache = path.join(os.homedir(), ".cache/ms-playwright");
  if (existsSync(cache)) {
    const installations = readdir(cache, { withFileTypes: true })
      .then((entries) => entries.filter((entry) => entry.isDirectory() && entry.name.startsWith("chromium-")).map((entry) => entry.name).sort().reverse());
    return installations.then((names) => {
      for (const name of names) {
        for (const folder of ["chrome-linux64", "chrome-linux"]) {
          const candidate = path.join(cache, name, folder, "chrome");
          if (existsSync(candidate)) return candidate;
        }
      }
      throw new Error("No Chromium executable found; set CHROME_BIN");
    });
  }
  throw new Error("No Chromium executable found; set CHROME_BIN");
}

class Cdp {
  constructor(socket) {
    this.socket = socket;
    this.nextId = 1;
    this.pending = new Map();
    socket.addEventListener("message", ({ data }) => {
      const message = JSON.parse(data);
      if (!message.id) return;
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      if (message.error) pending.reject(new Error(message.error.message));
      else pending.resolve(message.result);
    });
  }

  send(method, params = {}) {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }

  async evaluate(expression) {
    const result = await this.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true, userGesture: true });
    if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
    return result.result.value;
  }
}

async function launchBrowser() {
  const profile = await mkdtemp(path.join(os.tmpdir(), "zarra-browser-"));
  const executable = await chromeExecutable();
  const child = spawn(executable, ["--headless=new", "--no-sandbox", "--disable-dev-shm-usage", "--remote-debugging-port=0", `--user-data-dir=${profile}`, "about:blank"], { stdio: "ignore" });
  let port;
  for (let attempt = 0; attempt < 200; attempt++) {
    try {
      port = Number((await readFile(path.join(profile, "DevToolsActivePort"), "utf8")).split("\n")[0]);
      if (port) break;
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  if (!port) {
    child.kill("SIGKILL");
    throw new Error("Chromium did not expose CDP");
  }
  const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  const socket = new WebSocket(targets.find((target) => target.type === "page").webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve, { once: true });
    socket.addEventListener("error", reject, { once: true });
  });
  const cdp = new Cdp(socket);
  await cdp.send("Page.enable");
  await cdp.send("Runtime.enable");
  return {
    cdp,
    async close() {
      await cdp.send("Browser.close").catch(() => {});
      socket.close();
      if (child.exitCode === null) await new Promise((resolve) => { child.once("exit", resolve); setTimeout(() => child.kill("SIGKILL"), 2_000).unref(); });
      await rm(profile, { recursive: true, force: true });
    },
  };
}

let browser;
let storeOrigin;

test("standalone WASM browser integration", { timeout: 120_000 }, async (t) => {
  for (const required of ["js/client.js", "js/object-store.js", "js/worker.js", "pkg/zarra_wasm.js", "pkg/zarra_wasm_bg.wasm"]) {
    assert.ok(existsSync(path.join(crate, required)), `${required} is missing; run wasm-pack build first`);
  }
  const storePort = await listen(storeServer);
  const appPort = await listen(appServer);
  storeOrigin = `http://localhost:${storePort}`;
  appOrigin = `http://127.0.0.1:${appPort}`;
  browser = await launchBrowser();
  t.after(async () => {
    await browser.close();
    await close(appServer);
    await close(storeServer);
  });
  await browser.cdp.send("Page.navigate", { url: appOrigin });
  for (let attempt = 0; attempt < 100; attempt++) {
    if (await browser.cdp.evaluate("document.readyState === 'complete'")) break;
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  await browser.cdp.evaluate("(async () => { window.client = await import('/js/client.js'); window.stores = await import('/js/object-store.js'); window.wasm = await import('/pkg/zarra_wasm.js'); await window.wasm.default('/pkg/zarra_wasm_bg.wasm'); })()");

  const evaluate = (expression) => browser.cdp.evaluate(expression);
  const endpoint = (key = "refpy.zarr") => `${storeOrigin}/api/layers/${key}/sign`;
  const open = (generation = "g1") => {
    serverGeneration = generation;
    return evaluate(`window.client.openRaster({ key: 'refpy.zarr', signEndpoint: '${endpoint()}', generation: '${generation}' }).then((session) => { window.session = session; return true; })`);
  };
  const closeSession = () => evaluate("window.session?.close()");
  const tileHash = async (tile) => {
    const bytes = await evaluate(`window.session.renderTile(${JSON.stringify(tile)}).then((data) => Array.from(new Uint8Array(data)))`);
    return createHash("sha256").update(Buffer.from(bytes)).digest("hex");
  };

  await t.test("exact full, partial, empty and malformed HTTP responses", async () => {
    serverGeneration = "http";
    const result = await evaluate(`(async () => {
      const store = window.stores.createObjectStore({ key: 'refpy.zarr', signEndpoint: '${endpoint()}', generation: 'http' });
      const full = await store.get('zarr.json', null);
      const partial = await store.get('zarr.json', 'bytes=0-9');
      const emptyStore = window.stores.createObjectStore({ key: 'empty.zarr', signEndpoint: '${endpoint("empty.zarr")}', generation: 'http' });
      const empty = await emptyStore.get('0/c/0/0/0', 'bytes=0-0');
      const missing = await store.get('absent', null);
      return { full: full.status, partial: partial.status, partialLength: partial.bytes.byteLength, contentRange: partial.contentRange, empty: empty.status, emptyRange: empty.contentRange, missing: missing.status };
    })()`);
    assert.deepEqual(result, { full: 200, partial: 206, partialLength: 10, contentRange: `bytes 0-9/${fixtureFiles.get("refpy.zarr/zarr.json").length}`, empty: 416, emptyRange: "bytes */0", missing: 404 });
    assert.ok(signBodies.every((body) => Object.keys(body).length === 1 && body.paths.length === 1 && typeof body.paths[0] === "string"));
    for (const mode of ["partial200", "wrongRange", "wrongLength", "status500", "wrongGeneration"]) {
      behavior = { [mode]: true };
      serverGeneration = `bad-${mode}`;
      const message = await evaluate(`window.stores.createObjectStore({ key: 'refpy.zarr', signEndpoint: '${endpoint()}', generation: 'bad-${mode}' }).get('${mode === "status500" ? "0/c/0/0/0" : "zarr.json"}', 'bytes=0-9').then(() => null, (error) => error.message)`);
      assert.ok(message, mode);
      assert.ok(!message.includes("secret-") && !message.includes("/object/"), message);
      behavior = null;
    }
  });

  await t.test("native, overzoom and selected-band PNGs match native bytes", async () => {
    behavior = null;
    await open("render");
    assert.equal(await tileHash({ z: 0, x: 0, y: 0 }), GOLDEN.native);
    assert.equal(await tileHash({ z: 1, x: 1, y: 1 }), GOLDEN.overzoom);
    assert.equal(await tileHash({ z: 0, x: 0, y: 0, bands: [0, 0] }), GOLDEN.bands);
    await closeSession();
    assert.ok(requests.every((entry) => !entry.range || !entry.range.includes(",")));
    assert.ok(requests.some((entry) => entry.range === "bytes=0-0"));
    assert.ok(requests.every((entry) => entry.method !== "HEAD"));
    assert.ok(requests.filter((entry) => entry.server === "store").every((entry) => entry.cookie === null));
  });

  await t.test("404 is sparse but 500 and corrupt responses stay errors", async () => {
    behavior = { missingShard: true };
    await open("sparse");
    const transparent = await evaluate(`(async () => {
      const data = await window.session.renderTile({ z: 0, x: 0, y: 0 });
      const bitmap = await createImageBitmap(new Blob([data], { type: 'image/png' }));
      const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
      const context = canvas.getContext('2d');
      context.drawImage(bitmap, 0, 0);
      const pixels = context.getImageData(0, 0, bitmap.width, bitmap.height).data;
      return { width: bitmap.width, height: bitmap.height, transparent: pixels.every((value, index) => index % 4 !== 3 || value === 0) };
    })()`);
    assert.ok(transparent.width > 0 && transparent.height > 0);
    assert.equal(transparent.transparent, true);
    await closeSession();
    behavior = { status500: true };
    await open("error");
    const message = await evaluate("window.session.renderTile({ z: 0, x: 0, y: 0 }).then(() => null, (error) => error.message)");
    assert.ok(message);
    assert.ok(!message.includes("secret-") && !message.includes("/object/"), message);
    await closeSession();
    behavior = null;
  });

  await t.test("SQL has exact values, nodata NULL, read-only enforcement and caps", async () => {
    await open("sql");
    const point = await evaluate(`window.session.query({ sql: 'SELECT x, y, band, value FROM raster WHERE x = 2 AND y = 2', x: 2, y: 2, width: 1, height: 1 })`);
    assert.deepEqual(point, { columns: ["x", "y", "band", "value"], rows: [[2, 2, 0, 2002]], truncated: false, level: 0, x: 2, y: 2, width: 1, height: 1, source_values: 1 });
    const nodata = await evaluate(`window.session.query({ sql: 'SELECT COUNT(*) AS total, COUNT(value) AS valid FROM raster', x: -1, y: -1, width: 2, height: 2 })`);
    assert.deepEqual(nodata.rows, [[4, 1]]);
    for (const sql of ["DELETE FROM raster", "SELECT 1; SELECT 2", "PRAGMA query_only = OFF"]) {
      const message = await evaluate(`window.session.query({ sql: ${JSON.stringify(sql)}, x: 0, y: 0, width: 1, height: 1 }).then(() => null, (error) => error.message)`);
      assert.ok(message, sql);
    }
    const sourceCap = await evaluate(`window.session.query({ sql: 'SELECT 1', x: 0, y: 0, width: 1025, height: 1024 }).then(() => null, (error) => error.message)`);
    assert.match(sourceCap, /reduce it|source|window/i);
    const rows = await evaluate(`window.session.query({ sql: 'SELECT * FROM raster', x: 0, y: 0, width: 64, height: 64 })`);
    assert.equal(rows.rows.length, 1000);
    assert.equal(rows.truncated, true);
    const bytes = await evaluate(`window.session.query({ sql: "SELECT replace(hex(zeroblob(50000)), '00', 'x') AS payload FROM raster", x: 0, y: 0, width: 64, height: 64 })`);
    assert.equal(bytes.truncated, true);
    assert.ok(bytes.rows.length < 1000);
    assert.ok(JSON.stringify(bytes).length <= 8 * 1024 * 1024 + 1024);
    await closeSession();
  });

  await t.test("SQL uses an injectable monotonic performance clock", async () => {
    serverGeneration = "clock";
    const message = await evaluate(`(async () => {
      const store = window.stores.createObjectStore({ key: 'refpy.zarr', signEndpoint: '${endpoint()}', generation: 'clock' });
      const raster = await window.wasm.BrowserRaster.open(store.get);
      const original = performance.now.bind(performance);
      let calls = 0;
      Object.defineProperty(performance, 'now', { configurable: true, value: () => ++calls * 10001 });
      try {
        return await raster.queryJson(JSON.stringify({ sql: 'WITH RECURSIVE slow(value) AS (VALUES(0) UNION ALL SELECT value + 1 FROM slow WHERE value < 1000000) SELECT SUM(value) FROM slow', x: 0, y: 0, width: 1, height: 1 })).then(() => null, (error) => String(error));
      } finally {
        Object.defineProperty(performance, 'now', { configurable: true, value: original });
        raster.close();
      }
    })()`);
    assert.match(message, /interrupted/i);
  });

  await t.test("URLs refresh proactively, retry 401/403 once, and generation clears cache", async () => {
    ttlSeconds = 1;
    serverGeneration = "expiry";
    const first = await evaluate(`(async () => {
      const store = window.stores.createObjectStore({ key: 'refpy.zarr', signEndpoint: '${endpoint()}', generation: 'expiry' });
      await store.get('zarr.json', null);
      await new Promise((resolve) => setTimeout(resolve, 900));
      await store.get('zarr.json', null);
      return true;
    })()`);
    assert.equal(first, true);
    assert.equal(signing.get("expiry/refpy.zarr/zarr.json"), 2);
    ttlSeconds = 60;
    for (const status of ["unauthorizedOnce", "forbiddenOnce"]) {
      behavior = { [status]: true };
      await open(status);
      assert.equal(await tileHash({ z: 0, x: 0, y: 0 }), GOLDEN.native);
      await closeSession();
      assert.ok((signing.get(`${status}/refpy.zarr/0/c/0/0/0`) ?? 0) >= 2);
      behavior = null;
    }
    for (const generation of ["generation-a", "generation-b", "generation-a"]) {
      serverGeneration = generation;
      await evaluate(`window.stores.createObjectStore({ key: 'refpy.zarr', signEndpoint: '${endpoint()}', generation: '${generation}' }).get('zarr.json', null)`);
    }
    assert.equal(signing.get("generation-a/refpy.zarr/zarr.json"), 2);
  });

  await t.test("MapLibre returns ArrayBuffer PNGs and honors pre-aborted calls", async () => {
    await open("maplibre");
    const result = await evaluate(`(async () => {
      let protocol;
      window.client.registerMapLibreProtocol({ addProtocol: (name, callback) => { if (name !== 'zarra') throw new Error('wrong scheme'); protocol = callback; } }, window.session);
      const response = await protocol({ url: 'zarra:///0/0/0.png' }, new AbortController());
      const controller = new AbortController();
      controller.abort();
      const aborted = await protocol({ url: 'zarra:///1/0/0.png' }, controller).then(() => null, (error) => error.name);
      const malformed = await protocol({ url: 'zarra:///not-a-tile' }, new AbortController()).then(() => null, (error) => error.message);
      return { arrayBuffer: response.data instanceof ArrayBuffer, bytes: Array.from(new Uint8Array(response.data)), aborted, malformed };
    })()`);
    assert.equal(result.arrayBuffer, true);
    assert.equal(createHash("sha256").update(Buffer.from(result.bytes)).digest("hex"), GOLDEN.native);
    assert.equal(result.aborted, "AbortError");
    assert.match(result.malformed, /zarra:\/\/\/\{z\}\/\{x\}\/\{y\}\.png/);
    await closeSession();
  });

  await t.test("non-loopback HTTP and redirects fail without leaking signed URLs", async () => {
    serverGeneration = "insecure";
    const insecure = await evaluate(`(async () => {
      const original = window.fetch;
      window.fetch = (input, init) => String(input).endsWith('/sign') ? Promise.resolve(new Response(JSON.stringify({ generation: 'insecure', expires_at: Math.floor(Date.now() / 1000) + 60, objects: [{ path: 'zarr.json', url: 'http://example.com/object?token=secret-insecure' }] }), { status: 200, headers: { 'Content-Type': 'application/json' } })) : original(input, init);
      try {
        return await window.stores.createObjectStore({ key: 'refpy.zarr', signEndpoint: '${endpoint()}', generation: 'insecure' }).get('zarr.json', null).then(() => null, (error) => error.message);
      } finally { window.fetch = original; }
    })()`);
    assert.ok(insecure);
    assert.ok(!insecure.includes("secret-") && !insecure.includes("/object/"), insecure);
    serverGeneration = "redirect";
    const redirect = await evaluate(`(async () => {
      const original = window.fetch;
      window.fetch = (input, init) => String(input).endsWith('/sign') ? Promise.resolve(new Response(JSON.stringify({ generation: 'redirect', expires_at: Math.floor(Date.now() / 1000) + 60, objects: [{ path: 'zarr.json', url: '${storeOrigin}/redirect?token=secret-redirect' }] }), { status: 200, headers: { 'Content-Type': 'application/json' } })) : original(input, init);
      try {
        return await window.stores.createObjectStore({ key: 'refpy.zarr', signEndpoint: '${endpoint()}', generation: 'redirect' }).get('zarr.json', null).then(() => null, (error) => error.message);
      } finally { window.fetch = original; }
    })()`);
    assert.ok(redirect);
    assert.ok(!redirect.includes("secret-") && !redirect.includes("/redirect"), redirect);
  });

  assert.ok(requests.every((entry) => !entry.path.startsWith("/raster") && !entry.path.startsWith("/api/sql")));
});
