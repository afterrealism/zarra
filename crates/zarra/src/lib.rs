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

pub mod error;
pub mod manifest;

pub use error::{Error, Result};
pub use manifest::{Channels, DType, LevelOffset, Nodata, RasterManifest, TileEncoding};
