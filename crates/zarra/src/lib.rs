//! Core types for the Zarra canonical multiscale Zarr v3 raster profile.

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

#[cfg(feature = "async")]
pub mod async_store;
#[cfg(feature = "local")]
pub mod create;
pub mod error;
pub mod level;
pub mod manifest;
pub mod query;
pub mod render;
#[cfg(feature = "sql")]
pub mod sql;
#[cfg(any(feature = "local", feature = "http"))]
pub mod store;

pub(crate) const MAX_CACHED_SHARDS: usize = 1024;

#[cfg(feature = "async")]
pub use async_store::AsyncRasterStore;
#[cfg(feature = "local")]
pub use create::{convert, convert_with_options, BuildReport, ConvertOptions, Resampling};
pub use error::{Error, Result};
pub use level::LevelMeta;
pub use manifest::{Channels, DType, LevelOffset, Nodata, RasterManifest, TileEncoding};
pub use query::{parse_band_list, Chunk, ChunkRef, Window};
#[cfg(feature = "async")]
pub use render::render_async_tile;
#[cfg(any(feature = "local", feature = "http"))]
pub use render::render_tile;
pub use render::{Tile, TileCoord};
#[cfg(any(feature = "local", feature = "http"))]
pub use store::RasterStore;
