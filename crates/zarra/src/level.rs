//! Canonical Zarr v3 array metadata validation.

use serde_json::{Map, Value};
use zarrs::array::Array;
use zarrs::storage::ReadableStorageTraits;

use crate::{DType, Error, RasterManifest, Result};

const MAX_CHUNK_VALUES: u64 = 4_194_304;

pub(crate) type DynArray = Array<dyn ReadableStorageTraits>;

/// Validated geometry and element metadata for one raster level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelMeta {
    /// XYZ zoom level.
    pub z: u8,
    /// Number of bands in the array.
    pub bands: usize,
    /// Array width in pixels.
    pub width: u64,
    /// Array height in pixels.
    pub height: u64,
    /// Square spatial inner-chunk edge in pixels.
    pub inner: u32,
    /// Element type shared with the root manifest.
    pub dtype: DType,
}

impl LevelMeta {
    /// Validates an opened array through the same canonical JSON validator.
    pub(crate) fn from_array(array: &DynArray, z: u8, manifest: &RasterManifest) -> Result<Self> {
        let metadata = serde_json::to_value(array.metadata()).map_err(|error| {
            format_error(format!("cannot serialize level {z} metadata: {error}"))
        })?;
        Self::from_json(&metadata, z, manifest)
    }

    /// Validates canonical array JSON and derives its public level metadata.
    ///
    /// GDAL writes one band as `[H,W]`; an explicit `[bands,H,W]` axis is also
    /// accepted so the independent one-band reference fixture remains readable.
    pub fn from_json(value: &Value, z: u8, manifest: &RasterManifest) -> Result<Self> {
        manifest.validate()?;
        if manifest.level(z).is_none() {
            return Err(format_error(format!("manifest has no level {z}")));
        }
        if value.get("zarr_format").and_then(Value::as_u64) != Some(3) {
            return Err(format_error("level zarr_format must be 3"));
        }
        if value.get("node_type").and_then(Value::as_str) != Some("array") {
            return Err(format_error("level node_type must be array"));
        }
        match value.get("storage_transformers") {
            None => {}
            Some(Value::Array(transformers)) if transformers.is_empty() => {}
            _ => return Err(format_error("storage transformers are not supported")),
        }

        let shape = parse_shape(value.get("shape"), "shape")?;
        let dimensions = match shape.len() {
            2 if manifest.bands == 1 => 2,
            3 => 3,
            2 => return Err(format_error("multiband levels must have three dimensions")),
            _ => return Err(format_error("levels must have two or three dimensions")),
        };
        if dimensions == 3 && shape[0] != manifest.bands as u64 {
            return Err(format_error("array band shape does not match the manifest"));
        }

        let grid = required(value, "chunk_grid")?;
        require_name(grid, "regular", "chunk grid")?;
        let outer = parse_shape(configuration(grid)?.get("chunk_shape"), "outer chunk shape")?;
        if outer.len() != dimensions {
            return Err(format_error("outer chunk dimensionality mismatch"));
        }

        let key_encoding = required(value, "chunk_key_encoding")?;
        require_name(key_encoding, "default", "chunk-key encoding")?;
        if configuration(key_encoding)?
            .get("separator")
            .and_then(Value::as_str)
            != Some("/")
        {
            return Err(format_error("chunk-key separator must be /"));
        }

        let dtype_name = required(value, "data_type")?
            .as_str()
            .ok_or_else(|| format_error("data_type must be a string"))?;
        let dtype = parse_dtype(dtype_name)
            .ok_or_else(|| format_error(format!("unsupported level data_type {dtype_name}")))?;
        if dtype != manifest.dtype {
            return Err(format_error("level dtype does not match the manifest"));
        }

        let codecs = required(value, "codecs")?
            .as_array()
            .ok_or_else(|| format_error("codecs must be an array"))?;
        if codecs.len() != 1 {
            return Err(format_error("exactly one outer codec is required"));
        }
        let sharding = &codecs[0];
        require_name(sharding, "sharding_indexed", "outer codec")?;
        let sharding_configuration = configuration(sharding)?;
        let inner = parse_shape(
            sharding_configuration.get("chunk_shape"),
            "inner chunk shape",
        )?;
        if inner.len() != dimensions {
            return Err(format_error("inner chunk dimensionality mismatch"));
        }

        let inner_codecs = required_map(sharding_configuration, "codecs")?
            .as_array()
            .ok_or_else(|| format_error("inner codecs must be an array"))?;
        if !(inner_codecs.len() == 1 || inner_codecs.len() == 2) {
            return Err(format_error(
                "inner codecs must be bytes plus optional zstd",
            ));
        }
        validate_bytes_codec(&inner_codecs[0], "inner")?;
        if inner_codecs.len() == 2 {
            require_name(&inner_codecs[1], "zstd", "inner codec")?;
        }

        let index_codecs = required_map(sharding_configuration, "index_codecs")?
            .as_array()
            .ok_or_else(|| format_error("shard index codecs must be an array"))?;
        if index_codecs.len() != 2 {
            return Err(format_error("shard index codecs must be bytes and crc32c"));
        }
        validate_bytes_codec(&index_codecs[0], "shard index")?;
        require_name(&index_codecs[1], "crc32c", "shard index codec")?;
        if !matches!(
            sharding_configuration
                .get("index_location")
                .and_then(Value::as_str),
            Some("end" | "start")
        ) {
            return Err(format_error("shard index location must be end or start"));
        }

        if dimensions == 3 {
            let bands = manifest.bands as u64;
            if outer[0] != bands || inner[0] != bands {
                return Err(format_error("chunk band axes must equal the band count"));
            }
        }
        let inner_y = inner[dimensions - 2];
        let inner_x = inner[dimensions - 1];
        if inner_y != inner_x || inner_y != u64::from(manifest.tile_size) {
            return Err(format_error(
                "inner spatial chunks must be square and equal tile_size",
            ));
        }
        if outer[dimensions - 2] % inner_y != 0 || outer[dimensions - 1] % inner_x != 0 {
            return Err(format_error(
                "outer spatial chunks must be exact inner-chunk multiples",
            ));
        }
        let chunk_values = inner.iter().try_fold(1u64, |product, size| {
            product.checked_mul(*size).ok_or_else(|| {
                format_error("decoded chunk value count overflows checked arithmetic")
            })
        })?;
        if chunk_values > MAX_CHUNK_VALUES {
            return Err(format_error(
                "decoded chunk exceeds 4194304-value allocation cap",
            ));
        }

        Ok(Self {
            z,
            bands: manifest.bands,
            width: shape[dimensions - 1],
            height: shape[dimensions - 2],
            inner: manifest.tile_size,
            dtype,
        })
    }
}

fn required<'a>(value: &'a Value, field: &str) -> Result<&'a Value> {
    value
        .get(field)
        .ok_or_else(|| format_error(format!("missing {field}")))
}

fn configuration(value: &Value) -> Result<&Map<String, Value>> {
    required(value, "configuration")?
        .as_object()
        .ok_or_else(|| format_error("codec configuration must be an object"))
}

fn required_map<'a>(object: &'a Map<String, Value>, field: &str) -> Result<&'a Value> {
    object
        .get(field)
        .ok_or_else(|| format_error(format!("missing {field}")))
}

fn require_name(value: &Value, expected: &str, description: &str) -> Result<()> {
    if required(value, "name")?.as_str() == Some(expected) {
        Ok(())
    } else {
        Err(format_error(format!("{description} must be {expected}")))
    }
}

fn validate_bytes_codec(value: &Value, description: &str) -> Result<()> {
    require_name(value, "bytes", description)?;
    if configuration(value)?.get("endian").and_then(Value::as_str) == Some("little") {
        Ok(())
    } else {
        Err(format_error(format!(
            "{description} bytes codec must use little endian"
        )))
    }
}

fn parse_shape(value: Option<&Value>, description: &str) -> Result<Vec<u64>> {
    let values = value
        .and_then(Value::as_array)
        .ok_or_else(|| format_error(format!("{description} must be an array")))?;
    values
        .iter()
        .map(|value| {
            value.as_u64().filter(|size| *size > 0).ok_or_else(|| {
                format_error(format!("{description} sizes must be positive integers"))
            })
        })
        .collect()
}

fn parse_dtype(name: &str) -> Option<DType> {
    match name {
        "uint8" => Some(DType::Uint8),
        "int8" => Some(DType::Int8),
        "uint16" => Some(DType::Uint16),
        "int16" => Some(DType::Int16),
        "uint32" => Some(DType::Uint32),
        "int32" => Some(DType::Int32),
        "float32" => Some(DType::Float32),
        "float64" => Some(DType::Float64),
        _ => None,
    }
}

fn format_error(message: impl Into<String>) -> Error {
    Error::Format(message.into())
}
