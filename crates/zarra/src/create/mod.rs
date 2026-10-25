//! Raster creation through the GDAL command-line tools.

mod gdal;
pub mod webmercator;

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use tempfile::Builder;

use self::gdal::Gdal;
use self::webmercator::{native_maxzoom, pick_dst_nodata, tile_range_for};
use crate::{
    Channels, DType, Error, LevelMeta, LevelOffset, Nodata, RasterManifest, Result, TileEncoding,
};

/// Summary of one fully validated, newly published raster store.
#[derive(Debug)]
pub struct BuildReport {
    /// Canonical path of the new output directory.
    pub output: PathBuf,
    /// Validated root manifest written to the store.
    pub manifest: RasterManifest,
    /// Number of published zoom levels.
    pub levels: usize,
    /// Number of regular files in the published store.
    pub files: u64,
}

/// Converts a local GeoTIFF/COG into a canonical multiscale Zarr v3 store.
///
/// GDAL is invoked directly with argument arrays, never through a shell. The
/// output must not exist; no overwrite or remote-publication mode is provided.
pub fn convert(input: &Path, output: &Path) -> Result<BuildReport> {
    let input = fs::canonicalize(input)?;
    let input_metadata = fs::metadata(&input)?;
    if !input_metadata.is_file() || input_metadata.len() == 0 {
        return Err(Error::InvalidInput(
            "input must be a nonempty regular file".into(),
        ));
    }

    let output_name = output
        .file_name()
        .ok_or_else(|| Error::InvalidInput("output must name a new directory".into()))?
        .to_os_string();
    let requested_parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = fs::canonicalize(requested_parent)?;
    let output = parent.join(output_name);
    require_absent(&output)?;

    let temporary = Builder::new().prefix(".zarra-build-").tempdir_in(&parent)?;
    let work = temporary.path().join("work");
    let store = temporary.path().join("store");
    fs::create_dir(&work)?;

    let gdal = Gdal::from_env();
    let source = gdal.probe_source(&input, &work)?;
    let nodata = match source.nodata {
        Some(nodata) => Some(nodata),
        None => pick_dst_nodata(source.dtype, &source.per_band_range)?,
    };
    let maxzoom = native_maxzoom(source.px)?;
    let mut levels = Vec::with_capacity(usize::from(maxzoom) + 1);
    for z in (0..=maxzoom).rev() {
        let range = tile_range_for(z, source.minx, source.miny, source.maxx, source.maxy)?;
        gdal.emit_level(&input, &work, z, &source, range, nodata)?;
        levels.push(LevelOffset {
            z,
            xoff: range.tx0,
            yoff: range.ty0,
        });
    }

    let first_range = source
        .per_band_range
        .first()
        .copied()
        .ok_or_else(|| Error::Format("source has no band zero range".into()))?;
    let manifest = RasterManifest {
        profile_version: 1,
        crs: 3857,
        tile_size: 256,
        dtype: source.dtype,
        bands: source.bands,
        nodata,
        minzoom: 0,
        maxzoom,
        levels,
        tile_encoding: TileEncoding {
            channels: if source.bands == 1 {
                Channels::Rg16
            } else {
                Channels::Rgba8
            },
            encoded_min: first_range[0],
            encoded_max: first_range[1],
        },
        per_band_range: source.per_band_range,
        extent: source.extent,
    };
    manifest.validate()?;

    fs::create_dir(&store)?;
    for level in &manifest.levels {
        let z = level.z.to_string();
        let data = work.join(&z).join("data");
        if !fs::metadata(&data)?.is_dir() {
            return Err(Error::Format(format!(
                "GDAL level {z} has no pixel data array"
            )));
        }
        fs::rename(data, store.join(&z))?;
    }

    // Each gdal_translate call writes a group at work/<L>, not at work/. Use
    // level zero's retained group metadata as the new root group's template.
    let mut root = read_json(&work.join("0").join("zarr.json"))?;
    if root.get("zarr_format").and_then(Value::as_u64) != Some(3)
        || root.get("node_type").and_then(Value::as_str) != Some("group")
    {
        return Err(Error::Format(
            "GDAL root template must be a Zarr v3 group".into(),
        ));
    }
    let root_object = root
        .as_object_mut()
        .ok_or_else(|| Error::Format("GDAL root group must be an object".into()))?;
    let attributes = root_object
        .entry("attributes")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| Error::Format("GDAL root attributes must be an object".into()))?;
    attributes.insert("zarra".into(), manifest.to_value()?);
    let mut root_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(store.join("zarr.json"))?;
    serde_json::to_writer_pretty(&mut root_file, &root)
        .map_err(|error| Error::Format(format!("cannot write root group metadata: {error}")))?;
    root_file.write_all(b"\n")?;

    for level in &manifest.levels {
        let path = store.join(level.z.to_string()).join("zarr.json");
        let mut metadata = read_json(&path)?;
        LevelMeta::from_json(&metadata, level.z, &manifest)?;
        if metadata.get("fill_value").is_none_or(Value::is_null) {
            let fill_value = array_fill_value(&manifest)?;
            let object = metadata
                .as_object_mut()
                .ok_or_else(|| Error::Format("level metadata must be an object".into()))?;
            object.insert("fill_value".into(), fill_value);
            let encoded = serde_json::to_vec_pretty(&metadata).map_err(|error| {
                Error::Format(format!("cannot write level fill value: {error}"))
            })?;
            fs::write(&path, encoded)?;
        }
    }
    let files = count_files(&store)?;
    require_absent(&output)?;
    fs::rename(&store, &output)?;

    Ok(BuildReport {
        output,
        levels: manifest.levels.len(),
        files,
        manifest,
    })
}

fn require_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(Error::InvalidInput(format!(
            "output already exists: {}",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::Io(error)),
    }
}

fn read_json(path: &Path) -> Result<Value> {
    let bytes = fs::read(path)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| Error::Format(format!("invalid JSON at {}: {error}", path.display())))
}

fn array_fill_value(manifest: &RasterManifest) -> Result<Value> {
    let value = match manifest.nodata {
        Some(Nodata::NaN) => return Ok(Value::String("NaN".into())),
        Some(Nodata::Number(value)) => value,
        None => 0.0,
    };
    let (minimum, maximum, signed) = match manifest.dtype {
        DType::Float32 | DType::Float64 => {
            return serde_json::Number::from_f64(value)
                .map(Value::Number)
                .ok_or_else(|| Error::Format("floating array fill value must be finite".into()));
        }
        DType::Uint8 => (0.0, 255.0, false),
        DType::Int8 => (-128.0, 127.0, true),
        DType::Uint16 => (0.0, 65_535.0, false),
        DType::Int16 => (-32_768.0, 32_767.0, true),
        DType::Uint32 => (0.0, 4_294_967_295.0, false),
        DType::Int32 => (-2_147_483_648.0, 2_147_483_647.0, true),
    };
    if value.fract() != 0.0 || value < minimum || value > maximum {
        return Err(Error::Format(format!(
            "array fill value {value} is not representable as {}",
            manifest.dtype.as_str()
        )));
    }
    if signed {
        Ok(Value::from(value as i64))
    } else {
        Ok(Value::from(value as u64))
    }
}

fn count_files(directory: &Path) -> Result<u64> {
    let mut count = 0u64;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            count = count
                .checked_add(count_files(&entry.path())?)
                .ok_or_else(|| Error::InvalidInput("published file count overflows u64".into()))?;
        } else if file_type.is_file() {
            count = count
                .checked_add(1)
                .ok_or_else(|| Error::InvalidInput("published file count overflows u64".into()))?;
        }
    }
    Ok(count)
}
