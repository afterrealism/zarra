//! Exact chunk, window, neighborhood, and band reads from sharded arrays.

use zarrs::array::{ArrayShardedReadableExt, CodecOptions};

use crate::{DType, Error, RasterStore, Result};

const MAX_QUERY_VALUES: u64 = 16_777_216;

/// An array-relative inner-chunk coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkRef {
    /// Chunk column.
    pub col: u64,
    /// Chunk row.
    pub row: u64,
}

/// A full decoded inner chunk with band-planar `f64` samples.
/// Edge chunks retain this padded inner shape; array-bounded reads clip separately.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    /// Number of bands.
    pub bands: usize,
    /// Chunk width in pixels.
    pub width: u32,
    /// Chunk height in pixels.
    pub height: u32,
    /// Samples indexed as `[band, row, col]`.
    pub values: Vec<f64>,
}

impl Chunk {
    /// Returns one sample, or `None` when any coordinate is outside the chunk.
    pub fn value(&self, band: usize, row: u32, col: u32) -> Option<f64> {
        if band >= self.bands || row >= self.height || col >= self.width {
            return None;
        }
        let width = usize::try_from(self.width).ok()?;
        let height = usize::try_from(self.height).ok()?;
        let row = usize::try_from(row).ok()?;
        let col = usize::try_from(col).ok()?;
        let plane = width.checked_mul(height)?;
        let index = band
            .checked_mul(plane)?
            .checked_add(row.checked_mul(width)?)?
            .checked_add(col)?;
        self.values.get(index).copied()
    }
}

/// A decoded window with row-major, band-interleaved `f64` samples.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    /// Window width in pixels.
    pub width: u32,
    /// Window height in pixels.
    pub height: u32,
    /// Ordered source band indices, including any repeats.
    pub bands: Vec<usize>,
    /// Samples indexed as `[(row * width + col) * bands.len() + channel]`.
    pub values: Vec<f64>,
}

impl RasterStore {
    /// Enumerates every inner-chunk coordinate in row-major order.
    pub fn enumerate_chunks(&mut self, level: u8) -> Result<Vec<ChunkRef>> {
        let meta = self.level_meta(level)?;
        let columns = grid_axis(meta.width, meta.inner);
        let rows = grid_axis(meta.height, meta.inner);
        let count = columns
            .checked_mul(rows)
            .ok_or_else(|| Error::InvalidInput("chunk enumeration count overflows u64".into()))?;
        if count > MAX_QUERY_VALUES {
            return Err(Error::InvalidInput(
                "chunk enumeration exceeds 16777216-coordinate allocation cap".into(),
            ));
        }
        let capacity = usize::try_from(count)
            .map_err(|_| Error::InvalidInput("chunk count does not fit usize".into()))?;
        let mut references = Vec::new();
        references.try_reserve_exact(capacity).map_err(|error| {
            Error::InvalidInput(format!("cannot allocate chunk coordinates: {error}"))
        })?;
        for row in 0..rows {
            for col in 0..columns {
                references.push(ChunkRef { col, row });
            }
        }
        Ok(references)
    }

    /// Reads one inner chunk, returning `None` only for out-of-grid or sparse data.
    pub fn read_chunk(&mut self, level: u8, col: u64, row: u64) -> Result<Option<Chunk>> {
        let meta = self.level_meta(level)?;
        if col >= grid_axis(meta.width, meta.inner) || row >= grid_axis(meta.height, meta.inner) {
            return Ok(None);
        }
        self.reset_caches_if_needed()?;
        let state = self.level_mut(level)?;
        let indices = if state.array.shape().len() == 2 {
            vec![row, col]
        } else {
            vec![0, row, col]
        };
        if state
            .array
            .subchunk_byte_range(&state.cache, &indices)
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
                    .retrieve_subchunk_opt(&state.cache, &indices, &CodecOptions::default())
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
        let edge = usize::try_from(meta.inner)
            .map_err(|_| Error::InvalidInput("chunk edge does not fit usize".into()))?;
        let expected = meta
            .bands
            .checked_mul(edge)
            .and_then(|count| count.checked_mul(edge))
            .ok_or_else(|| Error::InvalidInput("decoded chunk length overflows usize".into()))?;
        if values.len() != expected {
            return Err(Error::Format(format!(
                "decoded chunk has {} values, expected {expected}",
                values.len()
            )));
        }
        Ok(Some(Chunk {
            bands: meta.bands,
            width: meta.inner,
            height: meta.inner,
            values,
        }))
    }

    /// Reads all bands as row-major, band-interleaved samples with NaN padding.
    pub fn read_window(
        &mut self,
        level: u8,
        x0: i64,
        y0: i64,
        width: u32,
        height: u32,
    ) -> Result<Window> {
        let bands = (0..self.manifest().bands).collect::<Vec<_>>();
        self.read_bands(level, x0, y0, width, height, &bands)
    }

    /// Reads selected bands in the requested order; repeated indices are allowed.
    pub fn read_bands(
        &mut self,
        level: u8,
        x0: i64,
        y0: i64,
        width: u32,
        height: u32,
        bands: &[usize],
    ) -> Result<Window> {
        if bands.is_empty() {
            return Err(Error::InvalidInput(
                "band selection must not be empty".into(),
            ));
        }
        let output_len = query_values_len(width, height, bands.len())?;
        let meta = self.level_meta(level)?;
        if let Some(band) = bands.iter().find(|band| **band >= meta.bands) {
            return Err(Error::InvalidInput(format!(
                "band {band} is outside 0..{}",
                meta.bands
            )));
        }
        let mut values = Vec::new();
        values.try_reserve_exact(output_len).map_err(|error| {
            Error::InvalidInput(format!("cannot allocate query window: {error}"))
        })?;
        values.resize(output_len, f64::NAN);

        let x0_signed = i128::from(x0);
        let y0_signed = i128::from(y0);
        let x1 = x0_signed + i128::from(width);
        let y1 = y0_signed + i128::from(height);
        let valid_x0 = x0_signed.max(0);
        let valid_y0 = y0_signed.max(0);
        let valid_x1 = x1.min(i128::from(meta.width));
        let valid_y1 = y1.min(i128::from(meta.height));
        if valid_x0 >= valid_x1 || valid_y0 >= valid_y1 {
            return Ok(Window {
                width,
                height,
                bands: bands.to_vec(),
                values,
            });
        }

        let inner = i128::from(meta.inner);
        let first_col = valid_x0 / inner;
        let first_row = valid_y0 / inner;
        let last_col = (valid_x1 - 1) / inner;
        let last_row = (valid_y1 - 1) / inner;
        let output_width = usize::try_from(width)
            .map_err(|_| Error::InvalidInput("window width does not fit usize".into()))?;
        for chunk_row in first_row..=last_row {
            for chunk_col in first_col..=last_col {
                let col = u64::try_from(chunk_col)
                    .map_err(|_| Error::InvalidInput("chunk column exceeds u64".into()))?;
                let row = u64::try_from(chunk_row)
                    .map_err(|_| Error::InvalidInput("chunk row exceeds u64".into()))?;
                let Some(chunk) = self.read_chunk(level, col, row)? else {
                    continue;
                };
                let chunk_x0 = chunk_col * inner;
                let chunk_y0 = chunk_row * inner;
                let overlap_x0 = valid_x0.max(chunk_x0);
                let overlap_y0 = valid_y0.max(chunk_y0);
                let overlap_x1 = valid_x1.min(chunk_x0 + inner);
                let overlap_y1 = valid_y1.min(chunk_y0 + inner);
                for y in overlap_y0..overlap_y1 {
                    let output_row = usize::try_from(y - y0_signed)
                        .map_err(|_| Error::InvalidInput("window row does not fit usize".into()))?;
                    let chunk_y = u32::try_from(y - chunk_y0)
                        .map_err(|_| Error::InvalidInput("chunk row does not fit u32".into()))?;
                    for x in overlap_x0..overlap_x1 {
                        let output_col = usize::try_from(x - x0_signed).map_err(|_| {
                            Error::InvalidInput("window column does not fit usize".into())
                        })?;
                        let chunk_x = u32::try_from(x - chunk_x0).map_err(|_| {
                            Error::InvalidInput("chunk column does not fit u32".into())
                        })?;
                        let output_pixel = output_row
                            .checked_mul(output_width)
                            .and_then(|index| index.checked_add(output_col))
                            .ok_or_else(|| {
                                Error::InvalidInput("window output index overflows usize".into())
                            })?;
                        for (channel, source_band) in bands.iter().copied().enumerate() {
                            let value =
                                chunk.value(source_band, chunk_y, chunk_x).ok_or_else(|| {
                                    Error::Format("decoded chunk coordinate is inconsistent".into())
                                })?;
                            let output_index = output_pixel
                                .checked_mul(bands.len())
                                .and_then(|index| index.checked_add(channel))
                                .ok_or_else(|| {
                                    Error::InvalidInput("window band index overflows usize".into())
                                })?;
                            let output = values.get_mut(output_index).ok_or_else(|| {
                                Error::Format("window output index is inconsistent".into())
                            })?;
                            *output = value;
                        }
                    }
                }
            }
        }
        Ok(Window {
            width,
            height,
            bands: bands.to_vec(),
            values,
        })
    }

    /// Reads a checked `(2r+1)` square centered on one pixel with NaN padding.
    pub fn read_neighborhood(
        &mut self,
        level: u8,
        cx: i64,
        cy: i64,
        radius: u32,
    ) -> Result<Window> {
        let side = radius
            .checked_mul(2)
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| Error::InvalidInput("neighborhood diameter overflows u32".into()))?;
        let radius = i64::from(radius);
        let x0 = cx
            .checked_sub(radius)
            .ok_or_else(|| Error::InvalidInput("neighborhood x origin overflows i64".into()))?;
        let y0 = cy
            .checked_sub(radius)
            .ok_or_else(|| Error::InvalidInput("neighborhood y origin overflows i64".into()))?;
        self.read_window(level, x0, y0, side, side)
    }
}

/// Parses comma-separated nonnegative band indices with optional `b`/`B` prefixes.
pub fn parse_band_list(text: &str) -> Result<Vec<usize>> {
    if text.is_empty() {
        return Err(Error::InvalidInput("band list must not be empty".into()));
    }
    text.split(',')
        .map(|token| {
            let digits = token
                .strip_prefix('b')
                .or_else(|| token.strip_prefix('B'))
                .unwrap_or(token);
            if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(Error::InvalidInput(format!(
                    "invalid band list token {token:?}"
                )));
            }
            digits.parse::<usize>().map_err(|error| {
                Error::InvalidInput(format!("invalid band index {token:?}: {error}"))
            })
        })
        .collect()
}

fn grid_axis(size: u64, inner: u32) -> u64 {
    size.div_ceil(u64::from(inner))
}

fn query_values_len(width: u32, height: u32, channels: usize) -> Result<usize> {
    if width == 0 || height == 0 {
        return Err(Error::InvalidInput(
            "window dimensions must be positive".into(),
        ));
    }
    let channels = u64::try_from(channels)
        .map_err(|_| Error::InvalidInput("channel count does not fit u64".into()))?;
    let count = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|count| count.checked_mul(channels))
        .ok_or_else(|| Error::InvalidInput("query value count overflows u64".into()))?;
    if count > MAX_QUERY_VALUES {
        return Err(Error::InvalidInput(
            "query exceeds 16777216-value allocation cap".into(),
        ));
    }
    usize::try_from(count)
        .map_err(|_| Error::InvalidInput("query length does not fit usize".into()))
}
