# Attribution and provenance

Zarra uses the Zarr upstream format and was informed by the following behavior references, package structures, fixtures, and dependencies. This file records provenance; the respective upstream licenses and notices remain with their works.

## Tilify Everything

Tilify Everything supplied the raster behavior and independent test-fixture reference used during implementation:

- `server/include/etl/raster_source.h`: GDAL probing, WebMercatorQuad planning, multiscale creation, band/nodata rules, and RG16/RGB8 source behavior.
- `server/include/etl/raster_pipeline.h`: flattening each level's pixel array out of GDAL's group output.
- `server/include/raster_array.h`: level, chunk, window, neighborhood, and band-read behavior.
- `server/routes/raster.h`: XYZ rendering, offsets, band selection, nodata alpha, and the source deep-overzoom behavior against which the corrected ancestor regression is defined.
- `server/include/zarr_client.h`: historical shard-read behavior only; Zarra deliberately uses maintained `zarrs` decoding instead of porting this custom codec stack.
- `web/index.html`: the ordinary MapLibre raster source/layer at opacity 0.92.
- `server/tests/fixtures/refpy.zarr`: the independent `zarr-python 3.2.1` reference array, whose canonical array bytes and hashes are retained.
- `server/tests/fixtures/sample.tif`: the georeferenced 512×512 COG conversion fixture, with its canonical hash retained.

The Tilify Everything source checkout was the behavioral reference. Zarra does not include that service's upload, account, registry, or production-service machinery.

## FGBA

FGBA supplied the core/CLI/example package pattern and the structural reference for a bounded loopback range server, especially its workspace manifests, `crates/fgba-cli/src/main.rs`, `crates/fgba-cli/src/serve.rs`, HTTP tests, per-crate fixture copies, and dependency-free Node example tests.

Reference repository: `https://github.com/afterrealism/fgba`. No FGB/A5 code is in Zarra's runtime data path; the storage model here is a Zarr directory/prefix, not FGBA's single-file layout.

## Zarr and maintained Rust decoding

- [Zarr v3 core specification](https://zarr-specs.readthedocs.io/en/latest/v3/core/index.html) defines the upstream array/group format.
- [ZEP0002: sharding-indexed storage](https://zarr.dev/zeps/accepted/ZEP0002.html) defines the sharding representation used by the canonical profile.
- [`zarrs`](https://github.com/zarrs/zarrs) provides Zarr metadata, storage, sharded reads, and codecs. This checkout uses the `0.23.15` backport at full commit [`9e55886ecfc4e8f282632d6fb7682869ca69db4b`](https://github.com/zarrs/zarrs/commit/9e55886ecfc4e8f282632d6fb7682869ca69db4b), with default features off and only `filesystem`, `sharding`, `crc32c`, and `zstd` enabled. The workspace pins `zarrs_storage` to the same full SHA for trait-source compatibility.
- [`zarrs_http` 0.3.1](https://crates.io/crates/zarrs_http/0.3.1) provides the optional blocking keyed-HTTP store.

`zarrs` and `zarrs_http` are licensed under MIT OR Apache-2.0. The full-SHA source selection is recorded for reproducibility and is not a claim that `zarrs 0.23.15` is available on crates.io.

## Raster and PNG tools

- [GDAL](https://gdal.org/) supplies `gdalinfo`, `gdalwarp`, and `gdal_translate` for probing, reprojection/resampling, and Zarr emission. Zarra requires GDAL 3.13+ and uses the GDAL 3.13.1 container for compatibility tests. GDAL is distributed under its [MIT-style license](https://github.com/OSGeo/gdal/blob/v3.13.1/LICENSE.TXT).
- [`png` 0.18.1](https://crates.io/crates/png/0.18.1) encodes the renderer's RGBA8 output and decodes golden-test PNGs. The crate is licensed under MIT OR Apache-2.0.

## MapLibre and bundled notices

The CLI embeds [MapLibre GL JS 4.7.1](https://github.com/maplibre/maplibre-gl-js/releases/tag/v4.7.1) JavaScript/CSS under BSD-3-Clause. Its vendored license is retained at `crates/zarra-cli/assets/vendor/maplibre-LICENSE.txt` and is served at `/vendor/maplibre-LICENSE.txt`.

That notice file also preserves the upstream notices for Mapbox GL JS v1.13 and earlier, glfx.js (Evan Wallace), and the d3-color portion (Mike Bostock). These assets are local and pinned; no CDN script or separate viewer dependency is substituted.
