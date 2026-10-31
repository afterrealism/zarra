function abortError() {
  return new DOMException("The operation was aborted", "AbortError");
}

export async function openRaster(options) {
  const worker = new Worker(new URL("./worker.js", import.meta.url), { type: "module" });
  const pending = new Map();
  let nextId = 1;
  let closed = false;

  worker.onmessage = ({ data }) => {
    const request = pending.get(data.id);
    if (!request) return;
    pending.delete(data.id);
    request.cleanup();
    if (data.ok) request.resolve(data.result);
    else request.reject(new Error(data.error));
  };
  worker.onerror = () => {
    for (const request of pending.values()) {
      request.cleanup();
      request.reject(new Error("raster worker failed"));
    }
    pending.clear();
  };

  function call(type, payload = {}, signal) {
    if (closed) return Promise.reject(new Error("raster is closed"));
    if (signal?.aborted) return Promise.reject(abortError());
    const id = nextId++;
    return new Promise((resolve, reject) => {
      const onAbort = () => {
        if (!pending.delete(id)) return;
        cleanup();
        worker.postMessage({ type: "cancel", id });
        reject(abortError());
      };
      const cleanup = () => signal?.removeEventListener("abort", onAbort);
      pending.set(id, { resolve, reject, cleanup });
      signal?.addEventListener("abort", onAbort, { once: true });
      worker.postMessage({ id, type, ...payload });
    });
  }

  try {
    const manifest = await call("open", { options });
    return {
      manifestJson: () => call("manifest"),
      manifest: JSON.parse(manifest),
      renderTile(request, signal) {
        return call("render", { request }, signal);
      },
      query(request) {
        return call("query", { request }).then(JSON.parse);
      },
      async close() {
        if (closed) return;
        try {
          await call("close");
        } finally {
          closed = true;
          worker.terminate();
          for (const request of pending.values()) {
            request.cleanup();
            request.reject(new Error("raster is closed"));
          }
          pending.clear();
        }
      },
    };
  } catch (error) {
    closed = true;
    worker.terminate();
    throw error;
  }
}

export function registerMapLibreProtocol(maplibregl, session) {
  maplibregl.addProtocol("zarra", async ({ z, x, y }, abortController) => {
    const signal = abortController?.signal ?? abortController;
    const data = await session.renderTile({ z, x, y }, signal);
    return { data };
  });
}
