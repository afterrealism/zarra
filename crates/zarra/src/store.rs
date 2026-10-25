//! Local and HTTP storage opening with shared root/level validation.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use zarrs::array::ArrayShardedReadableExtCache;
use zarrs::filesystem::FilesystemStore;
use zarrs::group::{Group, GroupMetadata};
use zarrs::storage::ReadableStorage;

use crate::level::DynArray;
use crate::{Error, LevelMeta, RasterManifest, Result};

const MAX_CACHED_SHARDS: usize = 1024;

pub(crate) struct Level {
    pub(crate) array: DynArray,
    pub(crate) meta: LevelMeta,
    pub(crate) cache: ArrayShardedReadableExtCache,
}

/// An owned canonical raster store with lazily opened levels and shard-index caches.
pub struct RasterStore {
    storage: ReadableStorage,
    manifest: RasterManifest,
    levels: HashMap<u8, Level>,
}

impl RasterStore {
    /// Opens a local Zarr v3 raster store directory.
    pub fn open(path: &Path) -> Result<Self> {
        let root = fs::canonicalize(path)?;
        if !fs::metadata(&root)?.is_dir() {
            return Err(Error::InvalidInput(
                "raster store must be a directory".into(),
            ));
        }
        let filesystem = FilesystemStore::new(&root)
            .map_err(|error| Error::Zarr(format!("cannot open filesystem store: {error}")))?;
        let storage: ReadableStorage = Arc::new(filesystem);
        Self::open_with_storage(storage)
    }

    /// Opens an HTTPS or loopback-HTTP Zarr v3 raster store.
    ///
    /// Userinfo, query strings, fragments, unsupported schemes, and plain HTTP
    /// to non-loopback hosts are rejected before constructing the HTTP store.
    #[cfg(feature = "http")]
    pub fn open_url(base_url: &str) -> Result<Self> {
        use zarrs_http::HTTPStore;

        let url = url::Url::parse(base_url)
            .map_err(|error| Error::Http(format!("invalid store URL: {error}")))?;
        validate_url(&url)?;
        let base = url.as_str().trim_end_matches('/');
        let mut http = HTTPStore::new(base)
            .map_err(|error| Error::Http(format!("cannot create HTTP store: {error}")))?;
        http.set_batch_range_requests(false);
        let storage: ReadableStorage = Arc::new(http);
        Self::open_with_storage(storage)
    }

    fn open_with_storage(storage: ReadableStorage) -> Result<Self> {
        let group = Group::open(storage.clone(), "/")
            .map_err(|error| Error::Zarr(format!("cannot open root group: {error}")))?;
        if !matches!(group.metadata(), GroupMetadata::V3(_)) {
            return Err(Error::Format("root group must use Zarr v3".into()));
        }
        let manifest = RasterManifest::from_attributes(group.attributes())?;
        let maxzoom = manifest.maxzoom;
        let mut store = Self {
            storage,
            manifest,
            levels: HashMap::new(),
        };
        store.level_meta(maxzoom)?;
        Ok(store)
    }

    /// Returns the validated root manifest.
    pub fn manifest(&self) -> &RasterManifest {
        &self.manifest
    }

    /// Opens and validates one level on first use, returning its metadata.
    pub fn level_meta(&mut self, level: u8) -> Result<LevelMeta> {
        Ok(self.level_mut(level)?.meta)
    }

    pub(crate) fn level_mut(&mut self, level: u8) -> Result<&mut Level> {
        use std::collections::hash_map::Entry;

        if self.manifest.level(level).is_none() {
            return Err(Error::InvalidInput(format!(
                "manifest has no level {level}"
            )));
        }
        match self.levels.entry(level) {
            Entry::Occupied(entry) => Ok(entry.into_mut()),
            Entry::Vacant(entry) => {
                let array = DynArray::open(self.storage.clone(), &format!("/{level}"))
                    .map_err(|error| Error::Zarr(format!("cannot open level {level}: {error}")))?;
                let meta = LevelMeta::from_array(&array, level, &self.manifest)?;
                let cache = ArrayShardedReadableExtCache::new(&array);
                Ok(entry.insert(Level { array, meta, cache }))
            }
        }
    }

    pub(crate) fn reset_caches_if_needed(&mut self) -> Result<()> {
        let cached = self.levels.values().try_fold(0usize, |total, level| {
            total
                .checked_add(level.cache.len())
                .ok_or_else(|| Error::InvalidInput("cached shard count overflows usize".into()))
        })?;
        if cached >= MAX_CACHED_SHARDS {
            for level in self.levels.values() {
                level.cache.clear();
            }
        }
        Ok(())
    }
}

#[cfg(feature = "http")]
fn validate_url(url: &url::Url) -> Result<()> {
    let authority_has_userinfo = url
        .as_str()
        .get(url.scheme().len() + 3..)
        .and_then(|remainder| remainder.split(['/', '?', '#']).next())
        .is_some_and(|authority| authority.contains('@'));
    if !url.username().is_empty() || url.password().is_some() || authority_has_userinfo {
        return Err(Error::Http("store URL must not contain userinfo".into()));
    }
    if url.query().is_some() {
        return Err(Error::Http("store URL must not contain a query".into()));
    }
    if url.fragment().is_some() {
        return Err(Error::Http("store URL must not contain a fragment".into()));
    }
    let host = url
        .host()
        .ok_or_else(|| Error::Http("store URL must include a host".into()))?;
    match url.scheme() {
        "https" => Ok(()),
        "http" if is_loopback(host) => Ok(()),
        "http" => Err(Error::Http(
            "plain HTTP is allowed only for loopback hosts".into(),
        )),
        scheme => Err(Error::Http(format!("unsupported URL scheme {scheme}"))),
    }
}

#[cfg(feature = "http")]
fn is_loopback(host: url::Host<&str>) -> bool {
    match host {
        url::Host::Domain("localhost") => true,
        url::Host::Ipv4(address) => address.octets()[0] == 127,
        url::Host::Ipv6(address) => address.is_loopback(),
        _ => false,
    }
}
