use std::path::PathBuf;

use clap::Args as ClapArgs;
use zarra::{Error, RasterStore, Result};

/// Exactly one local store path or remote store URL.
#[derive(Debug, ClapArgs)]
pub(crate) struct Source {
    /// Local Zarr store directory.
    #[arg(
        value_name = "STORE",
        required_unless_present = "url",
        conflicts_with = "url"
    )]
    path: Option<PathBuf>,
    /// HTTPS or loopback-HTTP Zarr store URL.
    #[arg(long, value_name = "URL", conflicts_with = "path")]
    url: Option<String>,
}

impl Source {
    pub(crate) fn open(&self) -> Result<RasterStore> {
        match (&self.path, &self.url) {
            (Some(path), None) => RasterStore::open(path),
            (None, Some(url)) => RasterStore::open_url(url),
            _ => Err(Error::InvalidInput(
                "exactly one positional store or --url is required".into(),
            )),
        }
    }

    pub(crate) fn label(&self) -> Result<String> {
        match (&self.path, &self.url) {
            (Some(path), None) => Ok(path.display().to_string()),
            (None, Some(url)) => Ok(url.clone()),
            _ => Err(Error::InvalidInput(
                "exactly one positional store or --url is required".into(),
            )),
        }
    }
}
