//! Asynchronous Zarr storage with the same validation and raster semantics as native stores.

use std::collections::HashMap;

use zarrs::array::{
    Array, AsyncArrayShardedReadableExt, AsyncArrayShardedReadableExtCache, CodecOptions,
};
use zarrs::group::{Group, GroupMetadata};
use zarrs::storage::{AsyncReadableStorage, AsyncReadableStorageTraits};

use crate::query::{grid_axis, WindowPlan};
use crate::{Chunk, DType, Error, LevelMeta, RasterManifest, Result, Window, MAX_CACHED_SHARDS};

type AsyncDynArray = Array<dyn AsyncReadableStorageTraits>;

struct AsyncLevel {
    array: AsyncDynArray,
    meta: LevelMeta,
    cache: AsyncArrayShardedReadableExtCache,
}

/// An asynchronous canonical raster store with lazily opened levels and shard-index caches.
pub struct AsyncRasterStore {
    storage: AsyncReadableStorage,
    manifest: RasterManifest,
    levels: HashMap<u8, AsyncLevel>,
}

impl AsyncRasterStore {
    /// Opens a Zarr v3 raster from an asynchronous readable store.
    pub async fn open(storage: AsyncReadableStorage) -> Result<Self> {
        let group = Group::async_open(storage.clone(), "/")
            .await
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
        store.level_meta(maxzoom).await?;
        Ok(store)
    }

    /// Returns the validated root manifest.
    pub fn manifest(&self) -> &RasterManifest {
        &self.manifest
    }

    /// Opens and validates one level on first use, returning its metadata.
    pub async fn level_meta(&mut self, level: u8) -> Result<LevelMeta> {
        Ok(self.level_mut(level).await?.meta)
    }

    async fn level_mut(&mut self, level: u8) -> Result<&mut AsyncLevel> {
        if self.manifest.level(level).is_none() {
            return Err(Error::InvalidInput(format!(
                "manifest has no level {level}"
            )));
        }
        if !self.levels.contains_key(&level) {
            let array = AsyncDynArray::async_open(self.storage.clone(), &format!("/{level}"))
                .await
                .map_err(|error| Error::Zarr(format!("cannot open level {level}: {error}")))?;
            let meta = LevelMeta::from_array(&array, level, &self.manifest)?;
            let cache = AsyncArrayShardedReadableExtCache::new(&array);
            self.levels.insert(level, AsyncLevel { array, meta, cache });
        }
        self.levels
            .get_mut(&level)
            .ok_or_else(|| Error::Format(format!("level {level} cache is inconsistent")))
    }

    async fn reset_caches_if_needed(&mut self) -> Result<()> {
        let mut cached = 0usize;
        for level in self.levels.values() {
            cached = cached
                .checked_add(level.cache.len().await)
                .ok_or_else(|| Error::InvalidInput("cached shard count overflows usize".into()))?;
        }
        if cached >= MAX_CACHED_SHARDS {
            for level in self.levels.values() {
                level.cache.clear().await;
            }
        }
        Ok(())
    }

    /// Reads one inner chunk, returning `None` only for out-of-grid or sparse data.
    pub async fn read_chunk(&mut self, level: u8, col: u64, row: u64) -> Result<Option<Chunk>> {
        let meta = self.level_meta(level).await?;
        if col >= grid_axis(meta.width, meta.inner) || row >= grid_axis(meta.height, meta.inner) {
            return Ok(None);
        }
        self.reset_caches_if_needed().await?;
        let state = self.level_mut(level).await?;
        let indices = if state.array.shape().len() == 2 {
            vec![row, col]
        } else {
            vec![0, row, col]
        };
        if state
            .array
            .async_subchunk_byte_range(&state.cache, &indices)
            .await
            .map_err(|error| {
                Error::Zarr(format!("cannot read level {level} shard index: {error}"))
            })?
            .is_none()
        {
            return Ok(None);
        }

        macro_rules! decode {
            ($type:ty) => {{
                let decoded: Vec<$type> = state
                    .array
                    .async_retrieve_subchunk_opt::<Vec<$type>>(
                        &state.cache,
                        &indices,
                        &CodecOptions::default(),
                    )
                    .await
                    .map_err(|error| {
                        Error::Zarr(format!("cannot decode level {level} chunk: {error}"))
                    })?;
                decoded.into_iter().map(f64::from).collect::<Vec<_>>()
            }};
        }
        let values = match meta.dtype {
            DType::Uint8 => decode!(u8),
            DType::Int8 => decode!(i8),
            DType::Uint16 => decode!(u16),
            DType::Int16 => decode!(i16),
            DType::Uint32 => decode!(u32),
            DType::Int32 => decode!(i32),
            DType::Float32 => decode!(f32),
            DType::Float64 => decode!(f64),
        };
        Chunk::from_decoded(meta, values).map(Some)
    }

    /// Reads all bands as row-major, band-interleaved samples with NaN padding.
    pub async fn read_window(
        &mut self,
        level: u8,
        x0: i64,
        y0: i64,
        width: u32,
        height: u32,
    ) -> Result<Window> {
        let bands = (0..self.manifest().bands).collect::<Vec<_>>();
        self.read_bands(level, x0, y0, width, height, &bands).await
    }

    /// Reads selected bands in the requested order; repeated indices are allowed.
    pub async fn read_bands(
        &mut self,
        level: u8,
        x0: i64,
        y0: i64,
        width: u32,
        height: u32,
        bands: &[usize],
    ) -> Result<Window> {
        let meta = self.level_meta(level).await?;
        let mut plan = WindowPlan::new(meta, x0, y0, width, height, bands)?;
        while let Some(reference) = plan.next_chunk() {
            if let Some(chunk) = self.read_chunk(level, reference.col, reference.row).await? {
                plan.accumulate(reference, &chunk)?;
            }
        }
        Ok(plan.finish())
    }
}
