import { RASTER_OPACITY, rasterSource, tileCoords } from "./geo.js";

const WORLD_BOUNDS = [[-180, -85], [180, 85]];
const byId = (id) => document.getElementById(id);
const status = byId("status");
const fitButton = byId("fit");

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
  const source = rasterSource(manifest, window.location.origin);
  if (!window.maplibregl) throw new Error("MapLibre did not load.");

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

start().catch((error) => {
  fitButton.disabled = true;
  setStatus(`Unable to display this store: ${error.message}`, true);
});
