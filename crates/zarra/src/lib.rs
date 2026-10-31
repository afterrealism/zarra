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

pub mod create;
pub mod error;
pub mod level;
pub mod manifest;
pub mod query;
pub mod render;
pub mod store;

pub use create::{convert, convert_with_options, BuildReport, ConvertOptions, Resampling};
pub use error::{Error, Result};
pub use level::LevelMeta;
pub use manifest::{Channels, DType, LevelOffset, Nodata, RasterManifest, TileEncoding};
pub use query::{parse_band_list, Chunk, ChunkRef, Window};
pub use render::{render_tile, Tile, TileCoord};
pub use store::RasterStore;
