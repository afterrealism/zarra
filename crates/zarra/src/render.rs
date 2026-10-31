//! XYZ raster tile rendering with RG16/RGB8 packing and PNG encoding.

#[cfg(feature = "async")]
use crate::AsyncRasterStore;
#[cfg(any(feature = "local", feature = "http"))]
use crate::RasterStore;
use crate::{Chunk, Error, Nodata, RasterManifest, Result};

/// One validated XYZ tile coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileCoord {
    /// Zoom level, from 0 through 22.
    pub z: u8,
    /// Horizontal tile coordinate in `0..2^z`.
    pub x: u32,
    /// Vertical tile coordinate in `0..2^z`.
    pub y: u32,
}

impl TileCoord {
    fn validate(self) -> Result<()> {
        if self.z > 22 {
            return Err(Error::InvalidInput(format!(
                "tile zoom {} is outside 0..=22",
                self.z
            )));
        }
        let grid = 1u32 << self.z;
        if self.x >= grid || self.y >= grid {
            return Err(Error::InvalidInput(format!(
                "tile coordinate {}/{}/{} is outside the XYZ grid",
                self.z, self.x, self.y
            )));
        }
        Ok(())
    }
}

/// A rendered tile, or an empty result when no pixel is painted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tile {
    /// A color-type-6, 8-bit RGBA PNG.
    Png(Vec<u8>),
    /// A tile with no opaque pixels or no source chunk.
    Empty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ancestor {
    level: u8,
    ox: u32,
    oy: u32,
    winx: u32,
    winy: u32,
    factor: u32,
}

#[derive(Debug)]
struct RenderSelection {
    bands: Vec<usize>,
    ranges: Vec<[f64; 2]>,
    nodata: Option<Nodata>,
}

impl RenderSelection {
    fn new(manifest: &RasterManifest, bands: &[usize]) -> Result<Self> {
        let bands = if bands.is_empty() {
            (0..manifest.bands.min(3)).collect::<Vec<_>>()
        } else {
            bands.to_vec()
        };
        if bands.is_empty() || bands.len() > 3 {
            return Err(Error::InvalidInput(
                "tile band selection must contain one to three indices".into(),
            ));
        }
        let mut ranges = Vec::with_capacity(bands.len());
        for band in bands.iter().copied() {
            if band >= manifest.bands {
                return Err(Error::InvalidInput(format!(
                    "band {band} is outside 0..{}",
                    manifest.bands
                )));
            }
            let range = manifest.per_band_range.get(band).copied().ok_or_else(|| {
                Error::Format(format!("manifest has no display range for band {band}"))
            })?;
            ranges.push(range);
        }
        Ok(Self {
            bands,
            ranges,
            nodata: manifest.nodata,
        })
    }
}

fn ancestor(tile: TileCoord, maxzoom: u8) -> Ancestor {
    let k = tile.z.saturating_sub(maxzoom);
    let factor = 1u32 << k;
    Ancestor {
        level: tile.z - k,
        ox: tile.x >> k,
        oy: tile.y >> k,
        winx: tile.x & (factor - 1),
        winy: tile.y & (factor - 1),
        factor,
    }
}

struct TileRead {
    selection: RenderSelection,
    ancestor: Ancestor,
    col: u64,
    row: u64,
}

fn prepare_tile(
    manifest: &RasterManifest,
    tile: TileCoord,
    bands: &[usize],
) -> Result<Option<TileRead>> {
    tile.validate()?;
    let selection = RenderSelection::new(manifest, bands)?;
    let ancestor = ancestor(tile, manifest.maxzoom);
    let offset = manifest
        .level(ancestor.level)
        .ok_or_else(|| Error::Format(format!("manifest has no level {}", ancestor.level)))?;
    let Some(col) = ancestor.ox.checked_sub(offset.xoff) else {
        return Ok(None);
    };
    let Some(row) = ancestor.oy.checked_sub(offset.yoff) else {
        return Ok(None);
    };
    Ok(Some(TileRead {
        selection,
        ancestor,
        col: u64::from(col),
        row: u64::from(row),
    }))
}

fn render_chunk(chunk: &Chunk, read: TileRead) -> Result<Tile> {
    if chunk.width == 0 || chunk.width != chunk.height {
        return Err(Error::Format(
            "rendered chunk must have a positive square shape".into(),
        ));
    }
    let Some(rgba) = pack_data_tile(chunk, &read.selection, read.ancestor)? else {
        return Ok(Tile::Empty);
    };
    Ok(Tile::Png(encode_rgba(chunk.width, chunk.height, &rgba)?))
}

/// Renders one XYZ tile from a canonical raster store.
///
/// An empty band slice selects the manifest default. One selected band uses
/// RG16; two or three selected bands use independently normalized RGB8.
#[cfg(any(feature = "local", feature = "http"))]
pub fn render_tile(store: &mut RasterStore, tile: TileCoord, bands: &[usize]) -> Result<Tile> {
    let Some(read) = prepare_tile(store.manifest(), tile, bands)? else {
        return Ok(Tile::Empty);
    };
    let Some(chunk) = store.read_chunk(read.ancestor.level, read.col, read.row)? else {
        return Ok(Tile::Empty);
    };
    render_chunk(&chunk, read)
}

/// Renders one XYZ tile through the same geometry and packing as the native renderer.
#[cfg(feature = "async")]
pub async fn render_async_tile(
    store: &mut AsyncRasterStore,
    tile: TileCoord,
    bands: &[usize],
) -> Result<Tile> {
    let Some(read) = prepare_tile(store.manifest(), tile, bands)? else {
        return Ok(Tile::Empty);
    };
    let Some(chunk) = store
        .read_chunk(read.ancestor.level, read.col, read.row)
        .await?
    else {
        return Ok(Tile::Empty);
    };
    render_chunk(&chunk, read)
}

fn pack_data_tile(
    chunk: &Chunk,
    selection: &RenderSelection,
    ancestor: Ancestor,
) -> Result<Option<Vec<u8>>> {
    let size = chunk.width;
    let size_usize = usize::try_from(size)
        .map_err(|_| Error::InvalidInput("tile edge does not fit usize".into()))?;
    let output_len = size_usize
        .checked_mul(size_usize)
        .and_then(|count| count.checked_mul(4))
        .ok_or_else(|| Error::InvalidInput("RGBA tile length overflows usize".into()))?;
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(output_len)
        .map_err(|error| Error::InvalidInput(format!("cannot allocate RGBA tile: {error}")))?;
    rgba.resize(output_len, 0);

    let first_band = selection
        .bands
        .first()
        .copied()
        .ok_or_else(|| Error::InvalidInput("tile band selection must not be empty".into()))?;
    let rg16 = selection.bands.len() == 1;
    let mut painted = 0usize;
    for row in 0..size {
        let source_row = sample_axis(row, ancestor.winy, ancestor.factor, size);
        for col in 0..size {
            let source_col = sample_axis(col, ancestor.winx, ancestor.factor, size);
            let first = chunk
                .value(first_band, source_row, source_col)
                .ok_or_else(|| {
                    Error::Format("selected first band is absent from the decoded chunk".into())
                })?;
            if transparent(first, selection.nodata) {
                continue;
            }
            let output = (usize::try_from(row)
                .map_err(|_| Error::InvalidInput("tile row does not fit usize".into()))?
                * size_usize
                + usize::try_from(col)
                    .map_err(|_| Error::InvalidInput("tile column does not fit usize".into()))?)
            .checked_mul(4)
            .ok_or_else(|| Error::InvalidInput("RGBA pixel index overflows usize".into()))?;
            if rg16 {
                let range = selection.ranges.first().copied().ok_or_else(|| {
                    Error::Format("selected first band has no display range".into())
                })?;
                let encoded = normalize16(first, range);
                rgba[output] = (encoded >> 8) as u8;
                rgba[output + 1] = (encoded & 0xff) as u8;
            } else {
                for channel in 0..3usize {
                    let selected = if channel < selection.bands.len() {
                        channel
                    } else {
                        0
                    };
                    let band = selection.bands.get(selected).copied().ok_or_else(|| {
                        Error::Format("selected band index is inconsistent".into())
                    })?;
                    let range = selection.ranges.get(selected).copied().ok_or_else(|| {
                        Error::Format("selected band range is inconsistent".into())
                    })?;
                    let value = if channel == 0 {
                        first
                    } else {
                        chunk.value(band, source_row, source_col).ok_or_else(|| {
                            Error::Format("selected band is absent from the decoded chunk".into())
                        })?
                    };
                    rgba[output + channel] = normalize8(value, range);
                }
            }
            rgba[output + 3] = 255;
            painted += 1;
        }
    }
    if painted == 0 {
        Ok(None)
    } else {
        Ok(Some(rgba))
    }
}

fn sample_axis(output: u32, window: u32, factor: u32, size: u32) -> u32 {
    let origin = f64::from(window) / f64::from(factor);
    let span = 1.0 / f64::from(factor);
    let coordinate =
        (origin + ((f64::from(output) + 0.5) / f64::from(size)) * span) * f64::from(size);
    (coordinate as u32).min(size - 1)
}

fn transparent(value: f64, nodata: Option<Nodata>) -> bool {
    value.is_nan()
        || matches!(
            nodata,
            Some(Nodata::Number(declared)) if (value - declared).abs() <= 1e-9
        )
}

fn normalized(value: f64, range: [f64; 2]) -> f64 {
    let [minimum, maximum] = range;
    ((value - minimum) / (maximum - minimum)).clamp(0.0, 1.0)
}

fn normalize8(value: f64, range: [f64; 2]) -> u8 {
    (normalized(value, range) * 255.0 + 0.5) as u8
}

fn normalize16(value: f64, range: [f64; 2]) -> u16 {
    (normalized(value, range) * 65535.0 + 0.5) as u16
}

fn encode_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|count| count.checked_mul(4))
        .ok_or_else(|| Error::InvalidInput("PNG dimensions overflow".into()))?;
    if rgba.len() != expected {
        return Err(Error::InvalidInput("RGBA buffer length mismatch".into()));
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder
            .write_header()
            .map_err(|error| Error::Png(error.to_string()))?;
        writer
            .write_image_data(rgba)
            .map_err(|error| Error::Png(error.to_string()))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{ancestor, encode_rgba, pack_data_tile, Ancestor, RenderSelection, TileCoord};
    use crate::{Channels, Chunk, DType, LevelOffset, Nodata, RasterManifest, TileEncoding};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn manifest(bands: usize, nodata: Option<Nodata>) -> RasterManifest {
        RasterManifest {
            profile_version: 1,
            crs: 3857,
            tile_size: 4,
            dtype: DType::Float64,
            bands,
            nodata,
            minzoom: 0,
            maxzoom: 0,
            levels: vec![LevelOffset {
                z: 0,
                xoff: 0,
                yoff: 0,
            }],
            tile_encoding: TileEncoding {
                channels: if bands == 1 {
                    Channels::Rg16
                } else {
                    Channels::Rgba8
                },
                encoded_min: 0.0,
                encoded_max: 100.0,
            },
            per_band_range: (0..bands)
                .map(|band| {
                    let minimum = band as f64 * 100.0;
                    [minimum, minimum + 100.0]
                })
                .collect(),
            extent: None,
        }
    }

    fn chunk(bands: usize, size: u32, values: Vec<f64>) -> Chunk {
        Chunk {
            bands,
            width: size,
            height: size,
            values,
        }
    }

    fn geometry(factor: u32, winx: u32, winy: u32) -> Ancestor {
        Ancestor {
            level: 0,
            ox: 0,
            oy: 0,
            winx,
            winy,
            factor,
        }
    }

    #[test]
    fn resolves_defaults_and_selected_band_ranges() -> TestResult {
        let manifest = manifest(4, None);
        let defaults = RenderSelection::new(&manifest, &[])?;
        assert_eq!(defaults.bands, [0, 1, 2]);
        assert_eq!(
            defaults.ranges,
            [[0.0, 100.0], [100.0, 200.0], [200.0, 300.0]]
        );

        let one = RenderSelection::new(&manifest, &[2])?;
        assert_eq!(one.bands, [2]);
        assert_eq!(one.ranges, [[200.0, 300.0]]);
        let repeated = RenderSelection::new(&manifest, &[1, 1])?;
        assert_eq!(repeated.bands, [1, 1]);
        Ok(())
    }

    #[test]
    fn overzoom_ancestor_never_selects_an_intermediate_level() {
        for delta in [1, 12, 13, 22] {
            let resolved = ancestor(
                TileCoord {
                    z: delta,
                    x: 0,
                    y: 0,
                },
                0,
            );
            assert_eq!(resolved.level, 0);
            assert_eq!(resolved.factor, 1u32 << delta);
        }
    }

    #[test]
    fn packs_exact_rg16_low_mid_high_and_clamped_values() -> TestResult {
        let source = chunk(
            1,
            3,
            vec![
                f64::NEG_INFINITY,
                0.0,
                50.0,
                100.0,
                101.0,
                f64::INFINITY,
                25.0,
                75.0,
                -1.0,
            ],
        );
        let selection = RenderSelection::new(&manifest(1, None), &[0])?;
        let rgba = pack_data_tile(&source, &selection, geometry(1, 0, 0))?
            .ok_or("valid pixels must be painted")?;
        assert_eq!(
            rgba,
            vec![
                0, 0, 0, 255, // -infinity clamps to 0
                0, 0, 0, 255, // low endpoint
                128, 0, 0, 255, // midpoint = 0x8000
                255, 255, 0, 255, // high endpoint
                255, 255, 0, 255, // above high clamps
                255, 255, 0, 255, // +infinity clamps
                64, 0, 0, 255, // 25% = 0x4000
                191, 255, 0, 255, // 75% = 0xbfff
                0, 0, 0, 255, // below low clamps
            ]
        );
        Ok(())
    }

    #[test]
    fn packs_rgb8_independent_ranges_and_repeated_bands() -> TestResult {
        let source = chunk(3, 1, vec![25.0, 175.0, 250.0]);
        let selection = RenderSelection::new(&manifest(3, None), &[0, 1, 2])?;
        let rgba = pack_data_tile(&source, &selection, geometry(1, 0, 0))?
            .ok_or("valid RGB pixel must be painted")?;
        assert_eq!(rgba, [64, 191, 128, 255]);

        let repeated = RenderSelection::new(&manifest(3, None), &[1, 1])?;
        let rgba = pack_data_tile(&source, &repeated, geometry(1, 0, 0))?
            .ok_or("repeated-band pixel must be painted")?;
        assert_eq!(rgba, [191, 191, 191, 255]);

        let one = RenderSelection::new(&manifest(3, None), &[1])?;
        let rgba = pack_data_tile(&source, &one, geometry(1, 0, 0))?
            .ok_or("single selected band must be painted")?;
        assert_eq!(rgba, [191, 255, 0, 255]);
        Ok(())
    }

    #[test]
    fn first_band_controls_alpha_and_later_nan_only_zeroes_its_channel() -> TestResult {
        let source = chunk(
            2,
            2,
            vec![10.0, f64::NAN, 20.0, 30.0, f64::NAN, 999.0, 10.0, f64::NAN],
        );
        let mut manifest = manifest(2, Some(Nodata::Number(10.0)));
        manifest.per_band_range = vec![[0.0, 100.0], [0.0, 100.0]];
        let selection = RenderSelection::new(&manifest, &[0, 1])?;
        let rgba = pack_data_tile(&source, &selection, geometry(1, 0, 0))?
            .ok_or("two valid first-band pixels must be painted")?;
        assert_eq!(
            rgba,
            [
                0, 0, 0, 0, // first-band declared nodata
                0, 0, 0, 0, // first-band NaN
                51, 26, 51, 255, // later declared nodata does not clear alpha
                77, 0, 77, 255, // later NaN is zero, alpha remains opaque
            ]
        );
        Ok(())
    }

    #[test]
    fn all_transparent_pixels_produce_an_empty_tile() -> TestResult {
        for value in [f64::NAN, 10.0] {
            let source = chunk(1, 1, vec![value]);
            let selection = RenderSelection::new(&manifest(1, Some(Nodata::Number(10.0))), &[0])?;
            assert_eq!(
                pack_data_tile(&source, &selection, geometry(1, 0, 0))?,
                None
            );
        }
        Ok(())
    }

    #[test]
    fn factor_two_samples_the_requested_subwindow_with_pixel_centers() -> TestResult {
        let source = chunk(1, 4, (0..16).map(f64::from).collect());
        let mut manifest = manifest(1, None);
        manifest.per_band_range = vec![[0.0, 15.0]];
        manifest.tile_encoding.encoded_max = 15.0;
        let selection = RenderSelection::new(&manifest, &[0, 0])?;
        let rgba = pack_data_tile(&source, &selection, geometry(2, 1, 0))?
            .ok_or("source pixels must be painted")?;
        let rows = [
            [34, 34, 51, 51],
            [34, 34, 51, 51],
            [102, 102, 119, 119],
            [102, 102, 119, 119],
        ];
        let mut expected = Vec::new();
        for row in rows {
            for value in row {
                expected.extend_from_slice(&[value, value, value, 255]);
            }
        }
        assert_eq!(rgba, expected);
        Ok(())
    }

    #[test]
    fn encodes_exact_golden_png_and_checks_rgba_length() -> TestResult {
        let encoded = encode_rgba(1, 1, &[1, 2, 3, 4])?;
        assert_eq!(
            encoded,
            vec![
                137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0,
                1, 8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 16, 73, 68, 65, 84, 120, 1, 1, 5, 0,
                250, 255, 0, 1, 2, 3, 4, 0, 25, 0, 11, 185, 176, 227, 235, 0, 0, 0, 0, 73, 69, 78,
                68, 174, 66, 96, 130,
            ]
        );
        assert!(encode_rgba(1, 1, &[0; 3]).is_err());
        Ok(())
    }
}
