//! Browser bindings that reuse Zarra's asynchronous Zarr, PNG, and SQL engines.

#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
#![warn(missing_docs)]

mod js_store;

#[cfg(target_arch = "wasm32")]
use std::sync::Arc;

#[cfg(target_arch = "wasm32")]
use js_sys::{Function, Reflect, Uint8Array};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;
#[cfg(target_arch = "wasm32")]
use zarra::sql::{execute_window_with_clock, resolve_window, SqlRequest};
#[cfg(target_arch = "wasm32")]
use zarra::{render_async_tile, AsyncRasterStore, Tile, TileCoord};
#[cfg(target_arch = "wasm32")]
use zarrs::storage::AsyncReadableStorage;

#[cfg(target_arch = "wasm32")]
use crate::js_store::JsStore;

/// One open browser raster. Mutable operations are serialized by the supplied worker client.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub struct BrowserRaster {
    store: Option<AsyncRasterStore>,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
impl BrowserRaster {
    /// Opens a Zarr hierarchy through `get(key, rangeHeader)` JavaScript calls.
    #[wasm_bindgen(js_name = open)]
    pub async fn open(get: Function) -> Result<Self, JsValue> {
        let storage: AsyncReadableStorage = Arc::new(JsStore::new(get));
        let store = AsyncRasterStore::open(storage).await.map_err(js_error)?;
        Ok(Self { store: Some(store) })
    }

    /// Returns the validated root manifest as JSON.
    #[wasm_bindgen(js_name = manifestJson)]
    pub fn manifest_json(&self) -> Result<String, JsValue> {
        serde_json::to_string(self.store()?.manifest()).map_err(|error| {
            JsError::new(&format!("cannot encode raster manifest: {error}")).into()
        })
    }

    /// Renders one XYZ tile, returning a valid transparent PNG for an empty tile.
    #[wasm_bindgen(js_name = renderTile)]
    pub async fn render_tile(
        &mut self,
        z: u8,
        x: u32,
        y: u32,
        bands: Vec<u32>,
    ) -> Result<Uint8Array, JsValue> {
        let bands = bands
            .into_iter()
            .map(|band| {
                usize::try_from(band)
                    .map_err(|_| JsError::new(&format!("band {band} does not fit usize")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let store = self.store_mut()?;
        let tile_size = store.manifest().tile_size;
        match render_async_tile(store, TileCoord { z, x, y }, &bands)
            .await
            .map_err(js_error)?
        {
            Tile::Png(bytes) => Ok(Uint8Array::from(bytes.as_slice())),
            Tile::Empty => {
                let bytes = transparent_png(tile_size)?;
                Ok(Uint8Array::from(bytes.as_slice()))
            }
        }
    }

    /// Reads a bounded source window and executes one read-only SQLite statement.
    #[wasm_bindgen(js_name = queryJson)]
    pub async fn query_json(&mut self, request_json: String) -> Result<String, JsValue> {
        let request: SqlRequest = serde_json::from_str(&request_json)
            .map_err(|error| JsError::new(&format!("invalid SQL request: {error}")))?;
        let store = self.store_mut()?;
        let level = request.level.unwrap_or(store.manifest().maxzoom);
        let meta = store.level_meta(level).await.map_err(js_error)?;
        let resolved = resolve_window(&request, meta).map_err(js_error)?;
        let window = store
            .read_window(
                resolved.level,
                resolved.x,
                resolved.y,
                resolved.width,
                resolved.height,
            )
            .await
            .map_err(js_error)?;
        let response = execute_window_with_clock(
            &resolved,
            &request.sql,
            store.manifest().nodata,
            window,
            performance_now,
        )
        .map_err(js_error)?;
        serde_json::to_string(&response)
            .map_err(|error| JsError::new(&format!("cannot encode SQL response: {error}")).into())
    }

    /// Releases the raster and its shard-index caches. Further operations fail.
    pub fn close(&mut self) {
        self.store = None;
    }
}

#[cfg(target_arch = "wasm32")]
impl BrowserRaster {
    fn store(&self) -> Result<&AsyncRasterStore, JsError> {
        self.store
            .as_ref()
            .ok_or_else(|| JsError::new("raster is closed"))
    }

    fn store_mut(&mut self) -> Result<&mut AsyncRasterStore, JsError> {
        self.store
            .as_mut()
            .ok_or_else(|| JsError::new("raster is closed"))
    }
}

#[cfg(target_arch = "wasm32")]
fn js_error(error: zarra::Error) -> JsValue {
    JsError::new(&error.to_string()).into()
}

#[cfg(target_arch = "wasm32")]
fn transparent_png(size: u32) -> Result<Vec<u8>, JsValue> {
    let length = (size as usize)
        .checked_mul(size as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| JsError::new("transparent PNG dimensions overflow"))?;
    let rgba = vec![0; length];
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, size, size);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder
            .write_header()
            .map_err(|error| JsError::new(&format!("cannot encode transparent PNG: {error}")))?;
        writer
            .write_image_data(&rgba)
            .map_err(|error| JsError::new(&format!("cannot encode transparent PNG: {error}")))?;
    }
    Ok(bytes)
}

#[cfg(target_arch = "wasm32")]
fn performance_now() -> f64 {
    let global = js_sys::global();
    let performance = Reflect::get(&global, &JsValue::from_str("performance")).ok();
    let now = performance
        .as_ref()
        .and_then(|value| Reflect::get(value, &JsValue::from_str("now")).ok())
        .and_then(|value| value.dyn_into::<Function>().ok());
    now.and_then(|function| {
        function
            .call0(&performance.unwrap_or(JsValue::UNDEFINED))
            .ok()
    })
    .and_then(|value| value.as_f64())
    .filter(|value| value.is_finite())
    .unwrap_or(0.0)
}
