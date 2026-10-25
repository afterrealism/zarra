//! Pure WebMercatorQuad tile and no-data planning.

use crate::{DType, Error, Nodata, Result};

/// Half of the EPSG:3857 world's horizontal or vertical span, in meters.
pub const WORLD: f64 = 20_037_508.342_789_244;
/// Full EPSG:3857 world span, in meters.
pub const SPAN: f64 = 2.0 * WORLD;
const TILE_SIZE: u32 = 256;
const MAX_ZOOM: u8 = 22;

fn zoom_scale(z: u8) -> Result<f64> {
    if z > MAX_ZOOM {
        return Err(Error::InvalidInput("zoom must be between 0 and 22".into()));
    }
    Ok(f64::from(1u32 << z))
}

/// Returns the pixel resolution of a WebMercatorQuad level in meters.
pub fn webmerc_res(z: u8) -> Result<f64> {
    Ok(SPAN / (f64::from(TILE_SIZE) * zoom_scale(z)?))
}

/// Returns the width and height of one tile at a WebMercatorQuad level.
pub fn webmerc_tilespan(z: u8) -> Result<f64> {
    Ok(SPAN / zoom_scale(z)?)
}

/// Selects the native overview zoom for a positive 3857 pixel size in meters.
pub fn native_maxzoom(px_meters: f64) -> Result<u8> {
    if !px_meters.is_finite() || px_meters <= 0.0 {
        return Err(Error::InvalidInput(
            "native pixel size must be finite and positive".into(),
        ));
    }
    let zoom = (SPAN / (f64::from(TILE_SIZE) * px_meters)).log2().ceil();
    if zoom <= 0.0 {
        Ok(0)
    } else if zoom >= f64::from(MAX_ZOOM) {
        Ok(MAX_ZOOM)
    } else {
        Ok(zoom as u8)
    }
}

/// Inclusive tile indices and the corresponding aligned 3857 warp extent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileRange {
    /// First touched tile column.
    pub tx0: u32,
    /// Last touched tile column.
    pub tx1: u32,
    /// First touched tile row.
    pub ty0: u32,
    /// Last touched tile row.
    pub ty1: u32,
    /// Aligned minimum X coordinate.
    pub te_minx: f64,
    /// Aligned minimum Y coordinate.
    pub te_miny: f64,
    /// Aligned maximum X coordinate.
    pub te_maxx: f64,
    /// Aligned maximum Y coordinate.
    pub te_maxy: f64,
}

impl TileRange {
    /// Returns the number of touched tile columns.
    pub fn ncols(&self) -> u32 {
        self.tx1.saturating_sub(self.tx0).saturating_add(1)
    }

    /// Returns the number of touched tile rows.
    pub fn nrows(&self) -> u32 {
        self.ty1.saturating_sub(self.ty0).saturating_add(1)
    }
}

/// Snaps an EPSG:3857 area of interest to the level's inclusive tile range.
pub fn tile_range_for(z: u8, minx: f64, miny: f64, maxx: f64, maxy: f64) -> Result<TileRange> {
    let span = webmerc_tilespan(z)?;
    if ![minx, miny, maxx, maxy].into_iter().all(f64::is_finite) {
        return Err(Error::InvalidInput("AOI coordinates must be finite".into()));
    }
    if minx > maxx || miny > maxy {
        return Err(Error::InvalidInput("AOI corners are not ordered".into()));
    }

    let max_index = (1u32 << z) - 1;
    let clamp = |value: f64| {
        if value <= 0.0 {
            0
        } else if value >= f64::from(max_index) {
            max_index
        } else {
            value as u32
        }
    };
    let origin_x = -WORLD;
    let origin_y = WORLD;
    let tx0 = clamp(((minx - origin_x) / span).floor());
    let mut tx1 = clamp(((maxx - origin_x) / span - 1e-9).floor());
    let ty0 = clamp(((origin_y - maxy) / span).floor());
    let mut ty1 = clamp(((origin_y - miny) / span - 1e-9).floor());
    if tx1 < tx0 {
        tx1 = tx0;
    }
    if ty1 < ty0 {
        ty1 = ty0;
    }

    Ok(TileRange {
        tx0,
        tx1,
        ty0,
        ty1,
        te_minx: origin_x + f64::from(tx0) * span,
        te_maxx: origin_x + f64::from(tx1 + 1) * span,
        te_maxy: origin_y - f64::from(ty0) * span,
        te_miny: origin_y - f64::from(ty1 + 1) * span,
    })
}

/// Chooses a shared destination no-data value when the source declares none.
pub fn pick_dst_nodata(dtype: DType, per_band_range: &[[f64; 2]]) -> Result<Option<Nodata>> {
    for [minimum, maximum] in per_band_range {
        if !minimum.is_finite() || !maximum.is_finite() || maximum <= minimum {
            return Err(Error::InvalidInput(
                "band ranges must be finite and strictly increasing".into(),
            ));
        }
    }
    if matches!(dtype, DType::Float32 | DType::Float64) {
        return Ok(Some(Nodata::NaN));
    }

    let (dtype_min, dtype_max) = match dtype {
        DType::Uint8 => (0.0, 255.0),
        DType::Int8 => (-128.0, 127.0),
        DType::Uint16 => (0.0, 65535.0),
        DType::Int16 => (-32768.0, 32767.0),
        DType::Uint32 => (0.0, 4_294_967_295.0),
        DType::Int32 => (-2_147_483_648.0, 2_147_483_647.0),
        DType::Float32 | DType::Float64 => return Ok(Some(Nodata::NaN)),
    };
    if per_band_range.is_empty() {
        return Ok(Some(Nodata::Number(0.0)));
    }
    let collides = |candidate: f64| {
        per_band_range
            .iter()
            .any(|[minimum, maximum]| candidate >= *minimum && candidate <= *maximum)
    };
    if !collides(0.0) {
        return Ok(Some(Nodata::Number(0.0)));
    }

    let global_min = per_band_range
        .iter()
        .map(|range| range[0])
        .fold(dtype_max, f64::min);
    let global_max = per_band_range
        .iter()
        .map(|range| range[1])
        .fold(dtype_min, f64::max);
    let below = global_min.floor() - 1.0;
    if below >= dtype_min && !collides(below) {
        return Ok(Some(Nodata::Number(below)));
    }
    let above = global_max.ceil() + 1.0;
    if above <= dtype_max && !collides(above) {
        return Ok(Some(Nodata::Number(above)));
    }
    Ok(None)
}

/// Builds GDAL BLOCKSIZE and SHARD_CHUNK_SHAPE values for one level.
pub fn level_shapes(bands: usize, ncols: u32, nrows: u32) -> Result<(String, String)> {
    if bands == 0 {
        return Err(Error::InvalidInput("bands must be at least 1".into()));
    }
    if ncols == 0 || nrows == 0 {
        return Err(Error::InvalidInput(
            "touched tile dimensions must be positive".into(),
        ));
    }
    let width = ncols
        .checked_mul(TILE_SIZE)
        .ok_or_else(|| Error::InvalidInput("tile-column shape overflows u32".into()))?
        .min(2048);
    let height = nrows
        .checked_mul(TILE_SIZE)
        .ok_or_else(|| Error::InvalidInput("tile-row shape overflows u32".into()))?
        .min(2048);
    if bands == 1 {
        Ok((format!("{height},{width}"), "256,256".into()))
    } else {
        Ok((
            format!("{bands},{height},{width}"),
            format!("{bands},256,256"),
        ))
    }
}
