const cache = new Map();
let activeGeneration;

function assertNetworkUrl(value, label) {
  let url;
  try {
    url = new URL(value);
  } catch {
    throw new Error(`${label} is not a valid URL`);
  }
  const loopback = ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname);
  if (url.protocol !== "https:" && !(url.protocol === "http:" && loopback)) {
    throw new Error(`${label} must use HTTPS`);
  }
  return url;
}

function joinKey(base, relative) {
  if (typeof relative !== "string" || !relative || relative.startsWith("/") || relative.split("/").includes("..")) {
    throw new Error("object key must be a relative path inside the raster");
  }
  return base ? `${base}/${relative}` : relative;
}

async function sign(path, generation, signEndpoint) {
  let response;
  try {
    response = await fetch(signEndpoint, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ paths: [path] }),
      credentials: "omit",
      redirect: "error",
      cache: "no-store",
    });
  } catch {
    throw new Error("object URL signing failed");
  }
  if (response.status !== 200) throw new Error(`object URL signing returned ${response.status}`);
  let body;
  try {
    body = await response.json();
  } catch {
    throw new Error("object URL signing returned invalid JSON");
  }
  if (body?.generation !== generation) throw new Error("object URL signing changed generation");
  if (!Array.isArray(body?.objects) || body.objects.length !== 1 || body.objects[0]?.path !== path) {
    throw new Error("object URL signing returned the wrong object");
  }
  const url = assertNetworkUrl(body.objects[0]?.url, "signed object URL").href;
  const now = Date.now();
  const expiresAt = body?.expires_at * 1000;
  if (!Number.isSafeInteger(body?.expires_at) || !Number.isSafeInteger(expiresAt) || expiresAt <= now) {
    throw new Error("signed object URL has an invalid expiry");
  }
  const ttl = expiresAt - now;
  return { url, refreshAt: expiresAt - Math.min(60_000, ttl * 0.2) };
}

async function cachedUrl(key, path, generation, signEndpoint, force) {
  const cacheKey = `${generation}\0${key}`;
  let entry = cache.get(cacheKey);
  if (!force && entry?.url && Date.now() < entry.refreshAt) return entry.url;
  if (!force && entry?.signing) return entry.signing;
  const signing = sign(path, generation, signEndpoint);
  entry = { signing };
  cache.set(cacheKey, entry);
  try {
    const signed = await signing;
    if (activeGeneration !== generation) throw new Error("object store generation changed");
    cache.set(cacheKey, signed);
    return signed.url;
  } catch (error) {
    if (cache.get(cacheKey) === entry) cache.delete(cacheKey);
    throw error;
  }
}

function parseContentRange(value) {
  const match = /^bytes (\d+)-(\d+)\/(\d+)$/.exec(value ?? "");
  if (!match) throw new Error("partial object response has invalid Content-Range");
  const [, startText, endText, totalText] = match;
  const start = Number(startText);
  const end = Number(endText);
  const total = Number(totalText);
  if (![start, end, total].every(Number.isSafeInteger) || start > end || end >= total) {
    throw new Error("partial object response has invalid Content-Range bounds");
  }
  return { start, end, total };
}

function validateRange(rangeHeader, contentRange) {
  const actual = parseContentRange(contentRange);
  const bounded = /^bytes=(\d+)-(\d*)$/.exec(rangeHeader);
  if (bounded) {
    const start = Number(bounded[1]);
    const expectedEnd = bounded[2] === "" ? actual.total - 1 : Number(bounded[2]);
    if (actual.start !== start || actual.end !== expectedEnd) {
      throw new Error("partial object response does not match the requested range");
    }
    return actual;
  }
  const suffix = /^bytes=-(\d+)$/.exec(rangeHeader);
  if (!suffix) throw new Error("object range header is invalid");
  const length = Number(suffix[1]);
  if (actual.end !== actual.total - 1 || actual.start !== Math.max(0, actual.total - length)) {
    throw new Error("partial object response does not match the requested suffix");
  }
  return actual;
}

function responseError(key, status) {
  return new Error(`object request for ${key} returned ${status}`);
}

async function fetchObject(key, path, generation, signEndpoint, rangeHeader, force = false) {
  const url = await cachedUrl(key, path, generation, signEndpoint, force);
  let response;
  try {
    response = await fetch(url, {
      method: "GET",
      headers: rangeHeader ? { Range: rangeHeader } : undefined,
      credentials: "omit",
      redirect: "error",
      cache: "no-store",
    });
  } catch {
    throw new Error(`object request failed for ${key}`);
  }
  if ((response.status === 401 || response.status === 403) && !force) {
    return fetchObject(key, path, generation, signEndpoint, rangeHeader, true);
  }
  if (response.status === 404) {
    return { status: 404, bytes: new Uint8Array(), contentRange: null, contentLength: 0 };
  }
  const contentRange = response.headers.get("Content-Range");
  if (response.status === 416) {
    if (rangeHeader === "bytes=0-0" && contentRange === "bytes */0") {
      return { status: 416, bytes: new Uint8Array(), contentRange, contentLength: 0 };
    }
    throw responseError(key, 416);
  }
  if (!rangeHeader && response.status !== 200) throw responseError(key, response.status);
  if (rangeHeader && response.status === 200) {
    throw new Error(`partial object request for ${key} returned 200 instead of 206`);
  }
  if (rangeHeader && response.status !== 206) throw responseError(key, response.status);
  if (rangeHeader) validateRange(rangeHeader, contentRange);
  let bytes;
  try {
    bytes = new Uint8Array(await response.arrayBuffer());
  } catch {
    throw new Error(`object response decoding failed for ${key}`);
  }
  const declared = response.headers.get("Content-Length");
  if (declared !== null && (!/^\d+$/.test(declared) || Number(declared) !== bytes.byteLength)) {
    throw new Error(`object response length mismatch for ${key}`);
  }
  if (rangeHeader) {
    const expected = parseContentRange(contentRange);
    if (bytes.byteLength !== expected.end - expected.start + 1) {
      throw new Error(`partial object response length mismatch for ${key}`);
    }
  }
  return { status: response.status, bytes, contentRange: rangeHeader ? contentRange : null, contentLength: bytes.byteLength };
}

export function createObjectStore({ key, signEndpoint, generation }) {
  if (typeof key !== "string" || !key || key.startsWith("/") || key.endsWith("/")) {
    throw new Error("raster key must be a non-empty relative prefix");
  }
  if (typeof generation !== "string" && typeof generation !== "number") {
    throw new Error("raster generation must be a string or number");
  }
  const normalizedGeneration = String(generation);
  const endpoint = assertNetworkUrl(signEndpoint, "sign endpoint").href;
  if (activeGeneration !== normalizedGeneration) {
    cache.clear();
    activeGeneration = normalizedGeneration;
  }
  return {
    get(relative, rangeHeader = null) {
      if (rangeHeader !== null && typeof rangeHeader !== "string") {
        return Promise.reject(new Error("range header must be a string or null"));
      }
      let fullKey;
      try {
        fullKey = joinKey(key, relative);
      } catch (error) {
        return Promise.reject(error);
      }
      return fetchObject(fullKey, relative, normalizedGeneration, endpoint, rangeHeader);
    },
  };
}
