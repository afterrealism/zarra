export const RASTER_OPACITY = 0.92;

export function tileCoords(lng, lat, z) {
  const x = Math.floor(((lng + 180) / 360) * 2 ** z);
  const rad = (lat * Math.PI) / 180;
  const y = Math.floor(((1 - Math.log(Math.tan(rad) + 1 / Math.cos(rad)) / Math.PI) / 2) * 2 ** z);
  return { x, y };
}

export function rasterSource(manifest, origin = "") {
  if (!manifest || typeof manifest.tiles !== "string" || manifest.tiles.length === 0) {
    throw new Error("Store manifest is missing its tile template.");
  }
  if (!Number.isInteger(manifest.tile_size) || manifest.tile_size <= 0) {
    throw new Error("Store manifest has an invalid tile_size.");
  }
  if (!Number.isInteger(manifest.minzoom) || manifest.minzoom < 0) {
    throw new Error("Store manifest has an invalid minzoom.");
  }
  if (!Number.isInteger(manifest.maxzoom) || manifest.maxzoom < manifest.minzoom || manifest.maxzoom > 22) {
    throw new Error("Store manifest has an invalid maxzoom.");
  }
  const template = origin === "" ? manifest.tiles : new URL(manifest.tiles, origin).toString()
    .replaceAll("%7B", "{")
    .replaceAll("%7D", "}");
  return {
    type: "raster",
    tiles: [template],
    tileSize: manifest.tile_size,
    minzoom: manifest.minzoom,
    maxzoom: manifest.maxzoom,
  };
}
