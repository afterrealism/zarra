import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import {
  copyFile,
  mkdir,
  mkdtemp,
  readFile,
  realpath,
  rm,
  symlink,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import test, { after, before } from "node:test";
import { fileURLToPath } from "node:url";

import { RASTER_OPACITY, rasterSource, tileCoords } from "../crates/zarra-cli/assets/geo.js";

const REPO_ROOT = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const BINARY = path.join(REPO_ROOT, "target", "debug", process.platform === "win32" ? "zarra.exe" : "zarra");
const FIXTURE = path.join(REPO_ROOT, "testdata", "refpy.zarr");
const ASSETS = path.join(REPO_ROOT, "crates", "zarra-cli", "assets");
const TILE_CACHE = "no-cache";

let canonicalServer;
let sparseServer;
let temporaryRoot;
const servers = [];

async function stopProcess(child) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  const exited = once(child, "exit");
  child.kill();
  await exited;
}

async function startServer(store) {
  const child = spawn(BINARY, ["serve", store, "--port", "0"], {
    stdio: ["ignore", "pipe", "ignore"],
  });
  try {
    const origin = await new Promise((resolve, reject) => {
      let stdout = "";
      let settled = false;
      const timer = setTimeout(() => finish(new Error("timed out waiting for the server URL")), 15_000);
      function finish(error) {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        child.stdout.off("data", onData);
        if (error) reject(error);
      }
      function onData(chunk) {
        stdout += chunk.toString();
        const newline = stdout.indexOf("\n");
        if (newline < 0) return;
        const line = stdout.slice(0, newline).trim();
        if (!/^http:\/\/127\.0\.0\.1:[1-9][0-9]*$/.test(line)) {
          finish(new Error(`invalid server URL: ${JSON.stringify(line)}`));
          return;
        }
        settled = true;
        clearTimeout(timer);
        child.stdout.off("data", onData);
        resolve(line);
      }
      child.stdout.on("data", onData);
      child.on("error", (error) => finish(error));
      child.on("exit", (code, signal) => {
        if (!settled) finish(new Error(`server exited before startup: code=${code} signal=${signal}`));
      });
    });
    return { child, origin };
  } catch (error) {
    await stopProcess(child);
    throw error;
  }
}

before(async () => {
  temporaryRoot = await mkdtemp(path.join(tmpdir(), "zarra-viewer-"));
  const sparse = path.join(temporaryRoot, "sparse.zarr");
  await mkdir(path.join(sparse, "0"), { recursive: true });
  const canonicalFixture = await realpath(FIXTURE);
  await copyFile(path.join(canonicalFixture, "zarr.json"), path.join(sparse, "zarr.json"));
  await copyFile(path.join(canonicalFixture, "0", "zarr.json"), path.join(sparse, "0", "zarr.json"));
  await writeFile(path.join(sparse, "empty.bin"), Buffer.alloc(0));
  const outside = path.join(temporaryRoot, "outside.txt");
  await writeFile(outside, "outside");
  if (process.platform !== "win32") await symlink(outside, path.join(sparse, "escape"));

  canonicalServer = await startServer(canonicalFixture);
  servers.push(canonicalServer);
  sparseServer = await startServer(await realpath(sparse));
  servers.push(sparseServer);
});

after(async () => {
  await Promise.all(servers.map(({ child }) => stopProcess(child)));
  if (temporaryRoot) await rm(temporaryRoot, { recursive: true, force: true });
});

async function request(route, options = {}, server = canonicalServer) {
  assert.ok(server, "test server must be running");
  const response = await fetch(new URL(route, server.origin), options);
  assert.equal(response.headers.get("access-control-allow-origin"), "*");
  assert.equal(response.headers.get("x-content-type-options"), "nosniff");
  assert.equal(response.headers.get("referrer-policy"), "no-referrer");
  return response;
}

async function body(response) {
  return Buffer.from(await response.arrayBuffer());
}

function assertPng16(bytes) {
  assert.ok(bytes.length >= 33);
  assert.deepEqual(bytes.subarray(0, 8), Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]));
  assert.equal(bytes.subarray(12, 16).toString(), "IHDR");
  assert.equal(bytes.readUInt32BE(16), 16);
  assert.equal(bytes.readUInt32BE(20), 16);
  assert.equal(bytes[24], 8);
  assert.equal(bytes[25], 6);
}

test("geo.js exports the exact source and tile math used by the browser", () => {
  const manifest = { tiles: "/raster/{z}/{x}/{y}.png", tile_size: 16, minzoom: 0, maxzoom: 0 };
  assert.deepEqual(rasterSource(manifest, "http://127.0.0.1:8090"), {
    type: "raster",
    tiles: ["http://127.0.0.1:8090/raster/{z}/{x}/{y}.png"],
    tileSize: 16,
    minzoom: 0,
    maxzoom: 0,
  });
  assert.deepEqual(rasterSource(manifest).tiles, ["/raster/{z}/{x}/{y}.png"]);
  assert.equal(RASTER_OPACITY, 0.92);
  assert.deepEqual(tileCoords(0, 0, 0), { x: 0, y: 0 });
  assert.deepEqual(tileCoords(-90, 0, 2), { x: 1, y: 2 });
  assert.deepEqual(tileCoords(90, 0, 2), { x: 3, y: 2 });
  assert.deepEqual(tileCoords(0, 66.51326044311185, 2), { x: 2, y: 1 });
});

test("the API returns the manifest and real tile template", async () => {
  const response = await request("/api/store");
  assert.equal(response.status, 200);
  assert.equal(response.headers.get("cache-control"), "no-store");
  assert.match(response.headers.get("content-type"), /^application\/json/);
  const manifest = await response.json();
  assert.equal(manifest.tiles, "/raster/{z}/{x}/{y}.png");
  assert.equal(manifest.tile_size, 16);
  assert.equal(manifest.maxzoom, 0);
  assert.equal(manifest.store_name, "refpy.zarr");
  assert.equal(manifest.upload_enabled, false);
});

test("the SQL API queries actual pixels and rejects writes", async () => {
  const response = await request("/api/sql", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ sql: "SELECT x, y, band, value FROM raster", x: 2, y: 2, width: 1, height: 1 }),
  });
  assert.equal(response.status, 200);
  const result = await response.json();
  assert.deepEqual(result.columns, ["x", "y", "band", "value"]);
  assert.deepEqual(result.rows, [[2, 2, 0, 2002]]);
  assert.equal(result.source_values, 1);

  const rejected = await request("/api/sql", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ sql: "DELETE FROM raster" }),
  });
  assert.equal(rejected.status, 400);
  assert.match((await rejected.json()).error, /read.only|readonly|SQL/i);
});

test("native, deep-overzoom, repeated, and b-prefixed band tiles are real PNGs", async () => {
  for (const route of [
    "/raster/0/0/0.png",
    "/raster/13/0/0.png",
    "/raster/22/0/0.png",
    "/raster/0/0/0.png?bands=0,0",
    "/raster/0/0/0.png?bands=b0",
    "/raster/0/0/0.png?bands=B0,B0",
  ]) {
    const response = await request(route);
    assert.equal(response.status, 200, route);
    assert.equal(response.headers.get("cache-control"), TILE_CACHE, route);
    assert.equal(response.headers.get("content-type"), "image/png", route);
    const bytes = await body(response);
    assertPng16(bytes);
    assert.equal(Number(response.headers.get("content-length")), bytes.length, route);
  }
  const rendered = await request("/raster/0/0/0.png");
  const length = Number(rendered.headers.get("content-length"));
  await rendered.arrayBuffer();
  const head = await request("/raster/0/0/0.png", { method: "HEAD" });
  assert.equal(head.status, 200);
  assert.equal(Number(head.headers.get("content-length")), length);
  assert.equal((await body(head)).length, 0);
});

test("invalid XYZ and bands are 400 while a metadata-only store is truly 204", async () => {
  for (const route of [
    "/raster/0/1/0.png",
    "/raster/0/0/1.png",
    "/raster/23/0/0.png",
    "/raster/0/0/0.png?bands=",
    "/raster/0/0/0.png?bands=0,,0",
    "/raster/0/0/0.png?bands=b",
    "/raster/0/0/0.png?bands=1",
    "/raster/0/0/0.png?bands=0,0,0,0",
  ]) {
    assert.equal((await request(route)).status, 400, route);
  }
  for (const method of ["GET", "HEAD"]) {
    const response = await request("/raster/0/0/0.png", { method }, sparseServer);
    assert.equal(response.status, 204);
    assert.equal(response.headers.get("cache-control"), TILE_CACHE);
    assert.equal(response.headers.get("content-length"), null);
    assert.equal((await body(response)).length, 0);
  }
});

test("nested objects stream exact full, HEAD, suffix, open, and bounded ranges", async () => {
  const route = "/store/0/c/0/0/0";
  const expected = await readFile(path.join(FIXTURE, "0", "c", "0", "0", "0"));
  const full = await request(route);
  assert.equal(full.status, 200);
  assert.equal(full.headers.get("accept-ranges"), "bytes");
  assert.equal(full.headers.get("content-type"), "application/octet-stream");
  assert.deepEqual(await body(full), expected);
  assert.equal(Number(full.headers.get("content-length")), expected.length);

  const head = await request(route, { method: "HEAD" });
  assert.equal(head.status, 200);
  assert.equal(head.headers.get("accept-ranges"), "bytes");
  assert.equal(Number(head.headers.get("content-length")), expected.length);
  assert.equal((await body(head)).length, 0);

  const cases = [
    ["bytes=-17", expected.length - 17, expected.length - 1],
    ["bytes=3-19", 3, 19],
    ["bytes=25-", 25, expected.length - 1],
  ];
  for (const [range, start, end] of cases) {
    const response = await request(route, { headers: { Range: range } });
    assert.equal(response.status, 206, range);
    assert.equal(response.headers.get("content-range"), `bytes ${start}-${end}/${expected.length}`, range);
    assert.equal(response.headers.get("accept-ranges"), "bytes", range);
    assert.deepEqual(await body(response), expected.subarray(start, end + 1), range);
    assert.equal(Number(response.headers.get("content-length")), end - start + 1, range);
  }

  for (const malformed of ["bytes=word", "bytes=0-1,4-5"]) {
    const response = await request(route, { headers: { Range: malformed } });
    assert.equal(response.status, 200, malformed);
    assert.deepEqual(await body(response), expected, malformed);
  }
  const unsatisfiable = await request(route, { headers: { Range: `bytes=${expected.length}-` } });
  assert.equal(unsatisfiable.status, 416);
  assert.equal(unsatisfiable.headers.get("content-range"), `bytes */${expected.length}`);
  assert.equal(Number(unsatisfiable.headers.get("content-length")), 0);
  assert.equal((await body(unsatisfiable)).length, 0);
});

test("empty objects, traversal, symlink escape, and non-files are handled safely", async () => {
  const empty = await request("/store/empty.bin", {}, sparseServer);
  assert.equal(empty.status, 200);
  assert.equal(Number(empty.headers.get("content-length")), 0);
  assert.equal((await body(empty)).length, 0);
  const emptyRange = await request("/store/empty.bin", { headers: { Range: "bytes=0-" } }, sparseServer);
  assert.equal(emptyRange.status, 416);
  assert.equal(emptyRange.headers.get("content-range"), "bytes */0");

  for (const route of [
    "/store/%2e%2e%2Foutside.txt",
    "/store/.%2Fzarr.json",
    "/store/%2Fetc/passwd",
    "/store/zarr%00.json",
    "/store/zarr%5C.json",
  ]) {
    assert.equal((await request(route, {}, sparseServer)).status, 403, route);
  }
  assert.equal((await request("/store/missing", {}, sparseServer)).status, 404);
  assert.equal((await request("/store/0", {}, sparseServer)).status, 404);
  if (process.platform !== "win32") {
    assert.equal((await request("/store/escape", {}, sparseServer)).status, 403);
  }
});

test("OPTIONS, CORS, mutation rejection, and unknown routes are explicit", async () => {
  for (const route of ["/api/store", "/store/zarr.json", "/raster/0/0/0.png", "/geo.js"]) {
    const response = await request(route, { method: "OPTIONS" });
    assert.equal(response.status, 204, route);
    assert.equal(response.headers.get("content-length"), null, route);
    assert.match(response.headers.get("access-control-allow-methods"), /GET/);
    assert.match(response.headers.get("access-control-allow-methods"), /HEAD/);
    assert.match(response.headers.get("access-control-allow-methods"), /POST/);
    assert.equal(response.headers.get("access-control-allow-headers"), "Range, Content-Type");
    const exposed = response.headers.get("access-control-expose-headers");
    for (const name of ["Accept-Ranges", "Content-Length", "Content-Range"]) assert.match(exposed, new RegExp(name));
  }
  for (const method of ["POST", "PUT", "DELETE"]) {
    const response = await request("/store/zarr.json", { method });
    assert.equal(response.status, 405);
    assert.match(response.headers.get("allow"), /GET/);
    assert.equal((await body(response)).length, 0);
  }
  assert.equal((await request("/not-a-route")).status, 404);
  assert.equal((await request("/not-a-route", { method: "OPTIONS" })).status, 404);
});

test("every allowlisted viewer asset is local, exact, and no-cache", async () => {
  const assets = [
    ["/", "index.html", "text/html"],
    ["/index.html", "index.html", "text/html"],
    ["/app.js", "app.js", "text/javascript"],
    ["/geo.js", "geo.js", "text/javascript"],
    ["/styles.css", "styles.css", "text/css"],
    ["/favicon.svg", "favicon.svg", "image/svg+xml"],
    ["/vendor/maplibre-gl.js", "vendor/maplibre-gl.js", "text/javascript"],
    ["/vendor/maplibre-gl.css", "vendor/maplibre-gl.css", "text/css"],
    ["/vendor/maplibre-LICENSE.txt", "vendor/maplibre-LICENSE.txt", "text/plain"],
  ];
  for (const [route, file, mime] of assets) {
    const response = await request(route);
    assert.equal(response.status, 200, route);
    assert.ok(response.headers.get("content-type").startsWith(mime), route);
    assert.equal(response.headers.get("cache-control"), "no-cache", route);
    const [actual, expected] = await Promise.all([body(response), readFile(path.join(ASSETS, file))]);
    assert.deepEqual(actual, expected, route);
    assert.equal(Number(response.headers.get("content-length")), expected.length, route);
  }
  assert.equal((await request("/geo.js.map")).status, 404);
});
