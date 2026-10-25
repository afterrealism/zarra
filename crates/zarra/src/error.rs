//! Errors shared by the Zarra library.

use thiserror::Error;

/// An error produced while validating or processing raster data.
#[derive(Debug, Error)]
pub enum Error {
    /// An underlying filesystem or stream operation failed.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// A GDAL command failed or returned invalid output.
    #[error("GDAL error: {0}")]
    Gdal(String),
    /// Input metadata or bytes do not match the expected format.
    #[error("format error: {0}")]
    Format(String),
    /// The input requests a format or capability outside the supported profile.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// Zarr metadata, storage, or decoding failed.
    #[error("zarr error: {0}")]
    Zarr(String),
    /// HTTP validation or transport failed.
    #[error("http error: {0}")]
    Http(String),
    /// PNG encoding or decoding failed.
    #[error("png error: {0}")]
    Png(String),
    /// A caller-supplied argument is invalid.
    #[error("invalid input: {0}")]
    InvalidInput(String),
}

/// A result using the Zarra library error type.
pub type Result<T> = std::result::Result<T, Error>;
