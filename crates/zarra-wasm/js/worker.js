import init, { BrowserRaster } from "../pkg/zarra_wasm.js";
import { createObjectStore } from "./object-store.js";

let raster;
let queue = Promise.resolve();
const cancelled = new Set();

function abortError() {
  return new DOMException("The operation was aborted", "AbortError");
}

async function dispatch(message) {
  if (cancelled.has(message.id)) throw abortError();
  if (message.type === "open") {
    await init(new URL("../pkg/zarra_wasm_bg.wasm", import.meta.url));
    const store = createObjectStore(message.options);
    raster = await BrowserRaster.open(store.get);
    return raster.manifestJson();
  }
  if (!raster) throw new Error("raster is closed");
  if (message.type === "render") {
    const { z, x, y, bands } = message.request;
    const bytes = await raster.renderTile(z, x, y, bands ?? []);
    return bytes.buffer;
  }
  if (message.type === "query") {
    return raster.queryJson(JSON.stringify(message.request));
  }
  if (message.type === "manifest") return raster.manifestJson();
  if (message.type === "close") {
    raster.close();
    raster = undefined;
    return null;
  }
  throw new Error("unknown worker request");
}

self.onmessage = (event) => {
  const message = event.data;
  if (message.type === "cancel") {
    cancelled.add(message.id);
    return;
  }
  queue = queue.then(async () => {
    try {
      const result = await dispatch(message);
      if (cancelled.delete(message.id)) return;
      self.postMessage({ id: message.id, ok: true, result }, result instanceof ArrayBuffer ? [result] : []);
    } catch (error) {
      if (cancelled.delete(message.id)) return;
      self.postMessage({ id: message.id, ok: false, error: error instanceof Error ? error.message : String(error) });
    }
  });
};
