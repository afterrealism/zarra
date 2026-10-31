import { RASTER_OPACITY, rasterSource, tileCoords } from "./geo.js";

const WORLD_BOUNDS = [[-180, -85], [180, 85]];
const byId = (id) => document.getElementById(id);
const status = byId("status");
const fitButton = byId("fit");
const state = {
  manifest: null,
  uploading: false,
  queryRunning: false,
  outputNameTouched: false,
};

function setFormStatus(id, message, kind = "") {
  const node = byId(id);
  node.textContent = message;
  node.dataset.state = kind;
}

function updateButtons() {
  byId("upload-open").disabled = state.uploading || state.queryRunning || !state.manifest?.upload_enabled;
  byId("sql-open").disabled = state.uploading || state.queryRunning || !state.manifest;
  byId("upload-submit").disabled = state.uploading || !byId("geotiff-file").files?.length;
  for (const id of ["upload-close", "upload-cancel"]) byId(id).disabled = state.uploading;
  for (const id of ["geotiff-file", "output-name", "max-zoom", "resampling", "zstd-level"]) {
    byId(id).disabled = state.uploading;
  }
  for (const id of ["sql-close", "sql-clear", "sql-run"]) byId(id).disabled = state.queryRunning;
  for (const id of ["sql-input", "sql-level", "sql-x", "sql-y", "sql-width", "sql-height"]) {
    byId(id).disabled = state.queryRunning;
  }
}

async function requestJson(url, options = {}) {
  const response = await fetch(url, options);
  const body = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(body.error || `The local server answered ${response.status}.`);
  return body;
}

function openDialog(dialog) {
  if (typeof dialog.showModal === "function") dialog.showModal();
  else dialog.setAttribute("open", "");
}

function closeDialog(dialog) {
  if (typeof dialog.close === "function" && dialog.open) dialog.close();
  else dialog.removeAttribute("open");
}

function clearSqlResults() {
  const empty = document.createElement("p");
  empty.className = "results-empty";
  empty.textContent = "Run a query to inspect its results.";
  byId("sql-results").replaceChildren(empty);
}

function setStatus(message, isError = false) {
  status.textContent = message;
  status.dataset.state = isError ? "error" : "ready";
}

function setText(id, value) {
  byId(id).textContent = value;
}

function boundsFor(extent) {
  if (!Array.isArray(extent) || extent.length !== 4 || !extent.every(Number.isFinite)) {
    return WORLD_BOUNDS;
  }
  const [west, south, east, north] = extent;
  return [[west, south], [east, north]];
}

async function start() {
  setStatus("Loading store metadata…");
  const response = await fetch("/api/store", { cache: "no-store" });
  if (!response.ok) throw new Error(`Store metadata request failed (${response.status}).`);
  const manifest = await response.json();
  state.manifest = manifest;
  updateButtons();
  const source = rasterSource(manifest, window.location.origin);
  if (!window.maplibregl) throw new Error("MapLibre did not load.");

  setText("store-name", manifest.store_name || "Active store");
  setText("dtype", String(manifest.dtype));
  setText("bands", String(manifest.bands));
  setText("levels", `${manifest.minzoom}–${manifest.maxzoom}`);
  setText("tile-size", `${manifest.tile_size} px`);
  setText("encoding", manifest.tile_encoding?.channels ?? "unknown");

  const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
  const bounds = boundsFor(manifest.extent);
  const map = new window.maplibregl.Map({
    container: "map",
    style: {
      version: 8,
      sources: { zarra: source },
      layers: [
        { id: "background", type: "background", paint: { "background-color": "#0d1513" } },
        {
          id: "zarra-raster",
          type: "raster",
          source: "zarra",
          paint: { "raster-opacity": RASTER_OPACITY },
        },
      ],
    },
    center: [0, 0],
    zoom: 0,
    minZoom: 0,
    maxZoom: 22,
    attributionControl: false,
    fadeDuration: reducedMotion.matches ? 0 : 300,
  });
  map.addControl(new window.maplibregl.NavigationControl({ showCompass: false }), "top-right");

  const fit = () => {
    map.fitBounds(bounds, {
      padding: 32,
      duration: reducedMotion.matches ? 0 : 350,
    });
  };
  fitButton.addEventListener("click", fit);

  let pointerPoint = null;
  function updateTelemetry(next = pointerPoint ? map.unproject(pointerPoint) : map.getCenter()) {
    const wrapped = next.wrap();
    const z = Math.max(0, Math.floor(map.getZoom()));
    const { x, y } = tileCoords(wrapped.lng, wrapped.lat, z);
    setText("zoom", `z${z}`);
    setText("tile", `z${z}/x${x}/y${y}`);
    setText("coordinates", `${wrapped.lat.toFixed(4)}°, ${wrapped.lng.toFixed(4)}°`);
  }

  map.on("mousemove", (event) => {
    pointerPoint = event.point;
    updateTelemetry(event.lngLat);
  });
  map.on("move", () => updateTelemetry());
  map.on("load", () => {
    fitButton.disabled = false;
    fit();
    updateTelemetry();
    setStatus("Loading raster tiles…");
  });
  map.on("idle", () => setStatus("Raster ready."));
  map.on("error", (event) => {
    setStatus(event.error?.message || "The map reported an error.", true);
  });
}

function defaultOutputName(filename) {
  const base = filename.replace(/\.tiff?$/i, "");
  return `${base || "raster"}.zarr`;
}

async function uploadRaster() {
  const file = byId("geotiff-file").files?.[0];
  if (!file) {
    setFormStatus("upload-status", "Choose a GeoTIFF file first.", "error");
    return;
  }
  const output = byId("output-name").value.trim();
  if (!output.toLowerCase().endsWith(".zarr") || output.length <= 5) {
    setFormStatus("upload-status", "Give the new store a basename ending in .zarr.", "error");
    return;
  }
  const maxzoomText = byId("max-zoom").value.trim();
  const maxzoom = maxzoomText ? Number(maxzoomText) : null;
  const zstd = Number(byId("zstd-level").value);
  if (maxzoom !== null && (!Number.isInteger(maxzoom) || maxzoom < 0 || maxzoom > 22)) {
    setFormStatus("upload-status", "Maximum zoom must be an integer from 0 to 22.", "error");
    return;
  }
  if (!Number.isInteger(zstd) || zstd < 1 || zstd > 22) {
    setFormStatus("upload-status", "ZSTD level must be an integer from 1 to 22.", "error");
    return;
  }

  const params = new URLSearchParams({
    filename: file.name,
    output,
    resampling: byId("resampling").value,
    zstd_level: String(zstd),
  });
  if (maxzoom !== null) params.set("maxzoom", String(maxzoom));
  state.uploading = true;
  updateButtons();
  setFormStatus("upload-status", "Uploading and converting with GDAL. Large rasters can take a while…");
  try {
    const result = await requestJson(`/api/uploads?${params}`, {
      method: "POST",
      headers: { "Content-Type": file.type || "image/tiff" },
      body: file,
    });
    setFormStatus(
      "upload-status",
      `Created ${result.output}: ${result.levels} levels across ${result.files} files. Opening the new store…`,
      "success",
    );
    window.location.reload();
  } catch (error) {
    setFormStatus("upload-status", error.message || String(error), "error");
  } finally {
    state.uploading = false;
    updateButtons();
  }
}

function sqlNumber(id, label, options = {}) {
  const text = byId(id).value.trim();
  if (!text && options.optional) return null;
  const value = Number(text);
  const minimum = options.minimum ?? Number.MIN_SAFE_INTEGER;
  const maximum = options.maximum ?? Number.MAX_SAFE_INTEGER;
  if (!Number.isSafeInteger(value) || value < minimum || value > maximum) {
    throw new Error(`${label} must be an integer from ${minimum} to ${maximum}.`);
  }
  return value;
}

function formatCell(value) {
  if (value === null) return "NULL";
  if (typeof value === "string") return value;
  return JSON.stringify(value);
}

function renderSqlResults(result) {
  const table = document.createElement("table");
  const caption = document.createElement("caption");
  caption.className = "sr-only";
  caption.textContent = `SQL results for ${state.manifest?.store_name || "the active raster"}`;
  const thead = document.createElement("thead");
  const headerRow = document.createElement("tr");
  for (const name of result.columns) {
    const cell = document.createElement("th");
    cell.scope = "col";
    cell.textContent = name;
    headerRow.append(cell);
  }
  thead.append(headerRow);
  const tbody = document.createElement("tbody");
  if (!result.rows.length) {
    const row = document.createElement("tr");
    const cell = document.createElement("td");
    cell.colSpan = result.columns.length;
    cell.textContent = "No rows returned.";
    row.append(cell);
    tbody.append(row);
  } else {
    for (const values of result.rows) {
      const row = document.createElement("tr");
      for (const value of values) {
        const cell = document.createElement("td");
        const text = formatCell(value);
        cell.textContent = text;
        cell.title = text;
        row.append(cell);
      }
      tbody.append(row);
    }
  }
  table.append(caption, thead, tbody);
  byId("sql-results").replaceChildren(table);
  const capped = result.truncated ? " · result capped" : "";
  setFormStatus(
    "sql-status",
    `${result.rows.length.toLocaleString("en-US")} rows · ${result.columns.length} columns${capped} · level ${result.level} · ${result.source_values.toLocaleString("en-US")} source values`,
    "success",
  );
}

async function runSql() {
  if (!state.manifest) return;
  try {
    const level = sqlNumber("sql-level", "Level", { minimum: 0, maximum: state.manifest.maxzoom });
    const x = sqlNumber("sql-x", "Origin X");
    const y = sqlNumber("sql-y", "Origin Y");
    const width = sqlNumber("sql-width", "Width", { optional: true, minimum: 1 });
    const height = sqlNumber("sql-height", "Height", { optional: true, minimum: 1 });
    if ((width === null) !== (height === null)) {
      throw new Error("Set both width and height, or leave both blank for the whole level.");
    }
    const sql = byId("sql-input").value.trim();
    if (!sql) throw new Error("Enter a SQL query.");
    state.queryRunning = true;
    clearSqlResults();
    updateButtons();
    setFormStatus("sql-status", "Reading the source window and running SQL…");
    try {
      const result = await requestJson("/api/sql", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ sql, level, x, y, width, height }),
      });
      renderSqlResults(result);
    } catch (error) {
      clearSqlResults();
      setFormStatus("sql-status", error.message || String(error), "error");
    } finally {
      state.queryRunning = false;
      updateButtons();
    }
  } catch (error) {
    setFormStatus("sql-status", error.message || String(error), "error");
  }
}

byId("geotiff-file").addEventListener("change", () => {
  const file = byId("geotiff-file").files?.[0];
  if (file && !state.outputNameTouched) byId("output-name").value = defaultOutputName(file.name);
  setFormStatus("upload-status", file ? `${file.name} · ${file.size.toLocaleString("en-US")} bytes` : "");
  updateButtons();
});
byId("output-name").addEventListener("input", () => { state.outputNameTouched = true; });
byId("upload-open").addEventListener("click", () => {
  setFormStatus("upload-status", "");
  updateButtons();
  openDialog(byId("upload-dialog"));
});
byId("upload-form").addEventListener("submit", (event) => {
  event.preventDefault();
  uploadRaster();
});
for (const id of ["upload-close", "upload-cancel"]) {
  byId(id).addEventListener("click", () => closeDialog(byId("upload-dialog")));
}
byId("upload-dialog").addEventListener("cancel", (event) => {
  if (state.uploading) event.preventDefault();
});
byId("sql-open").addEventListener("click", () => {
  if (!state.manifest) return;
  byId("sql-store").textContent = state.manifest.store_name || "Active raster store";
  byId("sql-level").max = String(state.manifest.maxzoom);
  byId("sql-level").value = String(state.manifest.maxzoom);
  setFormStatus("sql-status", "");
  updateButtons();
  openDialog(byId("sql-dialog"));
});
byId("sql-close").addEventListener("click", () => closeDialog(byId("sql-dialog")));
byId("sql-dialog").addEventListener("cancel", (event) => {
  if (state.queryRunning) event.preventDefault();
});
byId("sql-form").addEventListener("submit", (event) => {
  event.preventDefault();
  runSql();
});
byId("sql-clear").addEventListener("click", () => {
  clearSqlResults();
  setFormStatus("sql-status", "Results cleared.");
});
document.querySelectorAll(".query-example").forEach((button) => {
  button.addEventListener("click", () => {
    byId("sql-input").value = button.dataset.query;
    setFormStatus("sql-status", "Example loaded. Run it when you are ready.");
    byId("sql-input").focus();
  });
});
updateButtons();

start().catch((error) => {
  state.manifest = null;
  updateButtons();
  fitButton.disabled = true;
  setStatus(`Unable to display this store: ${error.message}`, true);
});
