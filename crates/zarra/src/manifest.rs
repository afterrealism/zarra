//! Root-group manifest types and validation for the Zarra raster profile.

use std::fmt;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::error::{Error, Result};

/// A raster element type supported by the Zarra profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DType {
    /// Unsigned 8-bit integer.
    Uint8,
    /// Signed 8-bit integer.
    Int8,
    /// Unsigned 16-bit integer.
    Uint16,
    /// Signed 16-bit integer.
    Int16,
    /// Unsigned 32-bit integer.
    Uint32,
    /// Signed 32-bit integer.
    Int32,
    /// 32-bit floating point.
    Float32,
    /// 64-bit floating point.
    Float64,
}

impl DType {
    /// Maps a GDAL band type name to a supported profile type.
    pub fn from_gdal(name: &str) -> Option<Self> {
        match name {
            "Byte" => Some(Self::Uint8),
            "Int8" => Some(Self::Int8),
            "UInt16" => Some(Self::Uint16),
            "Int16" => Some(Self::Int16),
            "UInt32" => Some(Self::Uint32),
            "Int32" => Some(Self::Int32),
            "Float32" => Some(Self::Float32),
            "Float64" => Some(Self::Float64),
            _ => None,
        }
    }

    /// Returns the lowercase Zarr/JSON name for this type.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Uint8 => "uint8",
            Self::Int8 => "int8",
            Self::Uint16 => "uint16",
            Self::Int16 => "int16",
            Self::Uint32 => "uint32",
            Self::Int32 => "int32",
            Self::Float32 => "float32",
            Self::Float64 => "float64",
        }
    }

    fn is_float(self) -> bool {
        matches!(self, Self::Float32 | Self::Float64)
    }
}

/// Default channel packing used when a raster is rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channels {
    /// One selected band encoded into red and green bytes.
    Rg16,
    /// Up to three selected bands encoded into red, green, and blue bytes.
    Rgba8,
}

/// A shared no-data declaration for every raster band.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Nodata {
    /// A finite numeric no-data value.
    Number(f64),
    /// A floating-point NaN no-data value, represented in JSON as `"NaN"`.
    NaN,
}

impl Serialize for Nodata {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Number(value) => serializer.serialize_f64(*value),
            Self::NaN => serializer.serialize_str("NaN"),
        }
    }
}

struct NodataVisitor;

impl Visitor<'_> for NodataVisitor {
    type Value = Nodata;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a number or the string \"NaN\"")
    }

    fn visit_f64<E>(self, value: f64) -> std::result::Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Nodata::Number(value))
    }

    fn visit_i64<E>(self, value: i64) -> std::result::Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Nodata::Number(value as f64))
    }

    fn visit_u64<E>(self, value: u64) -> std::result::Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Nodata::Number(value as f64))
    }

    fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
    where
        E: de::Error,
    {
        if value == "NaN" {
            Ok(Nodata::NaN)
        } else {
            Err(E::custom("nodata string must be \"NaN\""))
        }
    }
}

impl<'de> Deserialize<'de> for Nodata {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(NodataVisitor)
    }
}

/// The world-tile offset represented by array chunk `(0, 0)` at one zoom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LevelOffset {
    /// XYZ zoom level.
    pub z: u8,
    /// Horizontal world-tile offset.
    pub xoff: u32,
    /// Vertical world-tile offset.
    pub yoff: u32,
}

/// Default display scaling and channel packing for a raster.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TileEncoding {
    /// Default rendered channel layout.
    pub channels: Channels,
    /// Lower display-scaling bound, equal to band zero's range minimum.
    pub encoded_min: f64,
    /// Upper display-scaling bound, equal to band zero's range maximum.
    pub encoded_max: f64,
}

/// The `attributes.zarra` manifest stored on a canonical root group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RasterManifest {
    /// Zarra profile version; currently exactly 1.
    pub profile_version: u8,
    /// EPSG coordinate reference system code; currently exactly 3857.
    pub crs: u32,
    /// Square spatial inner-chunk edge in pixels.
    pub tile_size: u32,
    /// Shared element type for every band and level.
    pub dtype: DType,
    /// Number of raster bands.
    pub bands: usize,
    /// Optional shared no-data declaration.
    pub nodata: Option<Nodata>,
    /// Lowest published zoom; currently exactly 0.
    pub minzoom: u8,
    /// Highest published zoom, from 0 through 22.
    pub maxzoom: u8,
    /// Exactly one array offset for every zoom from 0 through `maxzoom`.
    pub levels: Vec<LevelOffset>,
    /// Default tile display encoding.
    pub tile_encoding: TileEncoding,
    /// One finite, strictly increasing display range per band.
    pub per_band_range: Vec<[f64; 2]>,
    /// Optional WGS84 `[west, south, east, north]` display extent.
    pub extent: Option<[f64; 4]>,
}

impl RasterManifest {
    /// Deserializes and validates the own `zarra` member of root attributes.
    pub fn from_attributes(attributes: &Map<String, Value>) -> Result<Self> {
        let value = attributes
            .get("zarra")
            .ok_or_else(|| format_error("root attributes have no own zarra member"))?;
        let manifest: Self = serde_json::from_value(value.clone())
            .map_err(|error| format_error(format!("invalid zarra manifest: {error}")))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Serializes a validated manifest object, without the outer `zarra` wrapper.
    pub fn to_value(&self) -> Result<Value> {
        self.validate()?;
        serde_json::to_value(self)
            .map_err(|error| format_error(format!("cannot serialize zarra manifest: {error}")))
    }

    /// Checks every root-manifest invariant in the Zarra profile.
    pub fn validate(&self) -> Result<()> {
        if self.profile_version != 1 {
            return Err(format_error("profile_version must be 1"));
        }
        if self.crs != 3857 {
            return Err(format_error("crs must be 3857"));
        }
        if self.tile_size == 0 {
            return Err(format_error("tile_size must be positive"));
        }
        if self.bands == 0 {
            return Err(format_error("bands must be at least 1"));
        }
        if self.minzoom != 0 {
            return Err(format_error("minzoom must be 0"));
        }
        if self.maxzoom > 22 {
            return Err(format_error("maxzoom must be between 0 and 22"));
        }

        match self.nodata {
            Some(Nodata::Number(value)) if !value.is_finite() => {
                return Err(format_error("numeric nodata must be finite"));
            }
            Some(Nodata::NaN) if !self.dtype.is_float() => {
                return Err(format_error("NaN nodata requires a floating-point dtype"));
            }
            _ => {}
        }

        let expected_levels = usize::from(self.maxzoom) + 1;
        if self.levels.len() != expected_levels {
            return Err(format_error(format!(
                "levels must contain exactly {expected_levels} entries"
            )));
        }
        for level in &self.levels {
            if level.z > self.maxzoom {
                return Err(format_error(format!(
                    "level {} is outside 0..={}",
                    level.z, self.maxzoom
                )));
            }
            let grid_size = 1u32 << level.z;
            if level.xoff >= grid_size || level.yoff >= grid_size {
                return Err(format_error(format!(
                    "level {} offset must be inside its zoom grid",
                    level.z
                )));
            }
        }
        for z in 0..=self.maxzoom {
            let count = self.levels.iter().filter(|level| level.z == z).count();
            if count != 1 {
                return Err(format_error(format!(
                    "levels must contain exactly one entry for zoom {z}"
                )));
            }
        }

        let channels_valid = if self.bands == 1 {
            self.tile_encoding.channels == Channels::Rg16
        } else {
            self.tile_encoding.channels == Channels::Rgba8
        };
        if !channels_valid {
            return Err(format_error(
                "channels must be rg16 for one band or rgba8 for multiple bands",
            ));
        }

        if self.per_band_range.len() != self.bands {
            return Err(format_error(
                "per_band_range must contain one range per band",
            ));
        }
        for (band, range) in self.per_band_range.iter().enumerate() {
            let [minimum, maximum] = *range;
            if !minimum.is_finite() || !maximum.is_finite() {
                return Err(format_error(format!(
                    "per_band_range[{band}] must be finite"
                )));
            }
            if maximum <= minimum {
                return Err(format_error(format!(
                    "per_band_range[{band}] must have max greater than min"
                )));
            }
        }

        let encoding = self.tile_encoding;
        if !encoding.encoded_min.is_finite() || !encoding.encoded_max.is_finite() {
            return Err(format_error("encoded range must be finite"));
        }
        if encoding.encoded_max <= encoding.encoded_min {
            return Err(format_error("encoded_max must be greater than encoded_min"));
        }
        let first_range = self
            .per_band_range
            .first()
            .ok_or_else(|| format_error("per_band_range has no band zero"))?;
        if encoding.encoded_min != first_range[0] || encoding.encoded_max != first_range[1] {
            return Err(format_error("encoded range must equal the band zero range"));
        }

        if let Some(extent) = self.extent {
            let [west, south, east, north] = extent;
            if !extent.into_iter().all(f64::is_finite) {
                return Err(format_error("extent coordinates must be finite"));
            }
            if !(-180.0..=180.0).contains(&west)
                || !(-180.0..=180.0).contains(&east)
                || !(-90.0..=90.0).contains(&south)
                || !(-90.0..=90.0).contains(&north)
            {
                return Err(format_error("extent is outside WGS84 bounds"));
            }
            if west > east || south > north {
                return Err(format_error("extent corners are not ordered"));
            }
        }

        Ok(())
    }

    /// Returns the offset for `z` by value, or `None` when it is absent.
    pub fn level(&self, z: u8) -> Option<LevelOffset> {
        self.levels.iter().find(|level| level.z == z).copied()
    }
}

fn format_error(message: impl Into<String>) -> Error {
    Error::Format(message.into())
}
