use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use super::webmercator::{level_shapes, TileRange};
use super::ConvertOptions;
use crate::{DType, Error, Nodata, Result};

const STDERR_LIMIT: usize = 8 * 1024;

#[derive(Debug)]
pub(crate) struct SourceInfo {
    pub(crate) bands: usize,
    pub(crate) dtype: DType,
    pub(crate) nodata: Option<Nodata>,
    pub(crate) categorical: bool,
    pub(crate) minx: f64,
    pub(crate) miny: f64,
    pub(crate) maxx: f64,
    pub(crate) maxy: f64,
    pub(crate) px: f64,
    pub(crate) extent: Option<[f64; 4]>,
    pub(crate) per_band_range: Vec<[f64; 2]>,
}

impl SourceInfo {
    pub(crate) fn from_gdal_json(original: &Value, warped_vrt: &Value) -> Result<Self> {
        let georeferenced = ["coordinateSystem", "gcps"]
            .into_iter()
            .any(|field| original.get(field).is_some_and(|value| !value.is_null()));
        if !georeferenced {
            return Err(Error::Format(
                "source has no CRS or GCP georeference".into(),
            ));
        }

        let bands = original
            .get("bands")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Format("gdalinfo bands must be an array".into()))?;
        if bands.is_empty() {
            return Err(Error::Format("source has no raster bands".into()));
        }
        let first_type = required_string(&bands[0], "type")?;
        let dtype = DType::from_gdal(first_type).ok_or_else(|| {
            Error::Unsupported(format!("unsupported band data type {first_type}"))
        })?;
        let mut nodata = None;
        let mut nodata_declarations = 0usize;
        let mut categorical = false;
        let mut per_band_range = Vec::with_capacity(bands.len());

        for band in bands {
            let band_type = required_string(band, "type")?;
            let band_dtype = DType::from_gdal(band_type).ok_or_else(|| {
                Error::Unsupported(format!("unsupported band data type {band_type}"))
            })?;
            if band_dtype != dtype {
                return Err(Error::Format(
                    "all raster bands must have the same dtype".into(),
                ));
            }
            if let Some(value) = band.get("noDataValue") {
                let parsed = parse_nodata(value, dtype)?;
                if nodata.is_some_and(|existing| existing != parsed) {
                    return Err(Error::Format(
                        "all raster bands must declare the same nodata value".into(),
                    ));
                }
                nodata = Some(parsed);
                nodata_declarations += 1;
            }
            if band.get("colorInterpretation").and_then(Value::as_str) == Some("Palette") {
                categorical = true;
            }

            let minimum = optional_finite_number(band, "minimum")?.unwrap_or(0.0);
            let mut maximum = optional_finite_number(band, "maximum")?.unwrap_or(1.0);
            if maximum <= minimum {
                maximum = minimum + 1.0;
            }
            if !maximum.is_finite() || maximum <= minimum {
                return Err(Error::Format(
                    "band has no finite usable display range".into(),
                ));
            }
            per_band_range.push([minimum, maximum]);
            optional_finite_number(band, "mean")?;
            optional_finite_number(band, "stdDev")?;
        }
        if nodata_declarations != 0 && nodata_declarations != bands.len() {
            return Err(Error::Format(
                "bands mix present and absent nodata declarations".into(),
            ));
        }

        let extent = parse_wgs84_extent(original)?;
        let transform = finite_array(warped_vrt.get("geoTransform"), 6, "3857 VRT geoTransform")?;
        let px = transform[1].abs();
        if px == 0.0 {
            return Err(Error::Format(
                "3857 VRT native pixel size must be positive".into(),
            ));
        }
        let corners = warped_vrt
            .get("cornerCoordinates")
            .and_then(Value::as_object)
            .ok_or_else(|| Error::Format("3857 VRT missing cornerCoordinates".into()))?;
        let upper_left = finite_array(corners.get("upperLeft"), 2, "3857 VRT upperLeft corner")?;
        let lower_right = finite_array(corners.get("lowerRight"), 2, "3857 VRT lowerRight corner")?;
        let minx = upper_left[0].min(lower_right[0]);
        let maxx = upper_left[0].max(lower_right[0]);
        let miny = upper_left[1].min(lower_right[1]);
        let maxy = upper_left[1].max(lower_right[1]);
        if minx >= maxx || miny >= maxy {
            return Err(Error::Format("3857 VRT footprint is empty".into()));
        }

        Ok(Self {
            bands: bands.len(),
            dtype,
            nodata,
            categorical,
            minx,
            miny,
            maxx,
            maxy,
            px,
            extent,
            per_band_range,
        })
    }
}

pub(crate) struct LevelRequest<'a> {
    pub(crate) source: &'a SourceInfo,
    pub(crate) range: TileRange,
    pub(crate) nodata: Option<Nodata>,
    pub(crate) options: ConvertOptions,
}

pub(crate) struct Gdal {
    bin_dir: Option<PathBuf>,
}

impl Gdal {
    pub(crate) fn from_env() -> Self {
        let bin_dir = env::var_os("GDAL_BIN_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        Self { bin_dir }
    }

    fn tool(&self, name: &str) -> OsString {
        self.bin_dir.as_ref().map_or_else(
            || OsString::from(name),
            |directory| directory.join(name).into_os_string(),
        )
    }

    fn run(&self, tool: &str, args: &[OsString]) -> Result<Vec<u8>> {
        let output = Command::new(self.tool(tool))
            .args(args)
            .env("GDAL_PAM_ENABLED", "NO")
            .output()
            .map_err(|error| Error::Gdal(format!("could not run {tool}: {error}")))?;
        if !output.status.success() {
            return Err(Error::Gdal(format!(
                "{tool} failed with status {}: {}",
                output.status,
                bounded_stderr(&output.stderr)
            )));
        }
        Ok(output.stdout)
    }

    fn json(&self, tool: &str, args: &[OsString]) -> Result<Value> {
        let stdout = self.run(tool, args)?;
        serde_json::from_slice(&stdout)
            .map_err(|error| Error::Gdal(format!("{tool} returned invalid JSON output: {error}")))
    }

    pub(crate) fn probe_source(&self, input: &Path, work: &Path) -> Result<SourceInfo> {
        let original = self.json(
            "gdalinfo",
            &[
                OsString::from("-json"),
                OsString::from("-approx_stats"),
                input.as_os_str().to_os_string(),
            ],
        )?;
        let probe = work.join("probe.vrt");
        self.run(
            "gdalwarp",
            &[
                OsString::from("-t_srs"),
                OsString::from("EPSG:3857"),
                OsString::from("-of"),
                OsString::from("VRT"),
                OsString::from("-overwrite"),
                input.as_os_str().to_os_string(),
                probe.as_os_str().to_os_string(),
            ],
        )?;
        let warped = self.json(
            "gdalinfo",
            &[OsString::from("-json"), probe.as_os_str().to_os_string()],
        )?;
        SourceInfo::from_gdal_json(&original, &warped)
    }

    pub(crate) fn emit_level(
        &self,
        input: &Path,
        work: &Path,
        z: u8,
        request: LevelRequest<'_>,
    ) -> Result<()> {
        let LevelRequest {
            source,
            range,
            nodata,
            options,
        } = request;
        let resolution = super::webmercator::webmerc_res(z)?;
        let level_tif = work.join(format!("_warp_{z}.tif"));
        let mut warp = vec![
            OsString::from("-t_srs"),
            OsString::from("EPSG:3857"),
            OsString::from("-r"),
            OsString::from(options.resampling.gdal(source.categorical)),
            OsString::from("-tr"),
            gdal_number(resolution),
            gdal_number(resolution),
            OsString::from("-te"),
            gdal_number(range.te_minx),
            gdal_number(range.te_miny),
            gdal_number(range.te_maxx),
            gdal_number(range.te_maxy),
            OsString::from("-te_srs"),
            OsString::from("EPSG:3857"),
        ];
        if let Some(nodata) = nodata {
            warp.push(OsString::from("-dstnodata"));
            warp.push(match nodata {
                Nodata::Number(value) => gdal_number(value),
                Nodata::NaN => OsString::from("nan"),
            });
        }
        warp.extend([
            OsString::from("-overwrite"),
            input.as_os_str().to_os_string(),
            level_tif.as_os_str().to_os_string(),
        ]);
        self.run("gdalwarp", &warp)?;

        let (blocksize, shard) = level_shapes(source.bands, range.ncols(), range.nrows())?;
        let level_output = work.join(z.to_string());
        self.run(
            "gdal_translate",
            &[
                OsString::from("-of"),
                OsString::from("ZARR"),
                OsString::from("-co"),
                OsString::from("FORMAT=ZARR_V3"),
                OsString::from("-co"),
                OsString::from("ARRAY_NAME=data"),
                OsString::from("-co"),
                OsString::from(format!("BLOCKSIZE={blocksize}")),
                OsString::from("-co"),
                OsString::from(format!("SHARD_CHUNK_SHAPE={shard}")),
                OsString::from("-co"),
                OsString::from("COMPRESS=ZSTD"),
                OsString::from("-co"),
                OsString::from(format!("ZSTD_LEVEL={}", options.zstd_level)),
                level_tif.as_os_str().to_os_string(),
                level_output.as_os_str().to_os_string(),
            ],
        )?;
        fs::remove_file(level_tif)?;
        Ok(())
    }
}

fn required_string<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Format(format!("gdalinfo band {field} must be a string")))
}

fn optional_finite_number(value: &Value, field: &str) -> Result<Option<f64>> {
    value.get(field).map_or(Ok(None), |value| {
        let number = value
            .as_f64()
            .ok_or_else(|| Error::Format(format!("gdalinfo {field} must be a finite number")))?;
        if number.is_finite() {
            Ok(Some(number))
        } else {
            Err(Error::Format(format!(
                "gdalinfo {field} must be a finite number"
            )))
        }
    })
}

fn parse_nodata(value: &Value, dtype: DType) -> Result<Nodata> {
    if let Some(number) = value.as_f64() {
        if number.is_finite() {
            return Ok(Nodata::Number(number));
        }
        return Err(Error::Format("nodata must be finite or NaN".into()));
    }
    if let Some(text) = value.as_str() {
        if text.eq_ignore_ascii_case("nan") {
            if matches!(dtype, DType::Float32 | DType::Float64) {
                return Ok(Nodata::NaN);
            }
            return Err(Error::Format(
                "NaN nodata requires floating-point bands".into(),
            ));
        }
    }
    Err(Error::Format(
        "nodata must be one finite number or NaN".into(),
    ))
}

fn finite_array(value: Option<&Value>, length: usize, description: &str) -> Result<Vec<f64>> {
    let values = value
        .and_then(Value::as_array)
        .filter(|values| values.len() == length)
        .ok_or_else(|| Error::Format(format!("{description} must have {length} numbers")))?;
    values
        .iter()
        .map(|value| {
            value
                .as_f64()
                .filter(|number| number.is_finite())
                .ok_or_else(|| {
                    Error::Format(format!("{description} values must be finite numbers"))
                })
        })
        .collect()
}

fn parse_wgs84_extent(original: &Value) -> Result<Option<[f64; 4]>> {
    fn walk(value: &Value, bounds: &mut Option<[f64; 4]>) -> Result<()> {
        let Some(values) = value.as_array() else {
            return Ok(());
        };
        if values.len() == 2 && values[0].is_number() && values[1].is_number() {
            let longitude = values[0]
                .as_f64()
                .filter(|value| value.is_finite())
                .ok_or_else(|| Error::Format("WGS84 longitude must be finite".into()))?;
            let latitude = values[1]
                .as_f64()
                .filter(|value| value.is_finite())
                .ok_or_else(|| Error::Format("WGS84 latitude must be finite".into()))?;
            *bounds = Some(match *bounds {
                Some([west, south, east, north]) => [
                    west.min(longitude),
                    south.min(latitude),
                    east.max(longitude),
                    north.max(latitude),
                ],
                None => [longitude, latitude, longitude, latitude],
            });
        } else {
            for child in values {
                walk(child, bounds)?;
            }
        }
        Ok(())
    }

    let Some(coordinates) = original
        .get("wgs84Extent")
        .and_then(|extent| extent.get("coordinates"))
    else {
        return Ok(None);
    };
    let mut bounds = None;
    walk(coordinates, &mut bounds)?;
    Ok(bounds)
}

fn gdal_number(value: f64) -> OsString {
    OsString::from(format!("{value:.10}"))
}

fn bounded_stderr(stderr: &[u8]) -> String {
    let tail = &stderr[stderr.len().saturating_sub(STDERR_LIMIT)..];
    let message = String::from_utf8_lossy(tail);
    let message = message.trim();
    if message.is_empty() {
        "<no stderr>".into()
    } else {
        message.into()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::SourceInfo;
    use crate::{DType, Nodata};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn band(dtype: &str) -> Value {
        json!({
            "type": dtype,
            "minimum": 10.0,
            "maximum": 20.0,
            "mean": 15.0,
            "stdDev": 2.0
        })
    }

    fn original(bands: Vec<Value>) -> Value {
        json!({
            "coordinateSystem": {"wkt": "PROJCS[test]"},
            "bands": bands,
            "wgs84Extent": {
                "coordinates": [[[[-10.0, -5.0], [30.0, 40.0]]]]
            }
        })
    }

    fn warped() -> Value {
        json!({
            "geoTransform": [1000.0, 2.5, 0.0, 2000.0, 0.0, -2.5],
            "cornerCoordinates": {
                "upperLeft": [1000.0, 2000.0],
                "lowerRight": [900.0, 1800.0]
            }
        })
    }

    #[test]
    fn parses_valid_single_band_metadata() -> TestResult {
        let mut source_band = band("Byte");
        source_band["noDataValue"] = json!(255);
        let source = SourceInfo::from_gdal_json(&original(vec![source_band]), &warped())?;
        assert_eq!(source.bands, 1);
        assert_eq!(source.dtype, DType::Uint8);
        assert_eq!(source.nodata, Some(Nodata::Number(255.0)));
        assert!(!source.categorical);
        assert_eq!(source.per_band_range, vec![[10.0, 20.0]]);
        assert_eq!(source.extent, Some([-10.0, -5.0, 30.0, 40.0]));
        assert_eq!((source.minx, source.maxx), (900.0, 1000.0));
        assert_eq!((source.miny, source.maxy), (1800.0, 2000.0));
        assert_eq!(source.px, 2.5);
        Ok(())
    }

    #[test]
    fn parses_valid_three_band_metadata_and_palette() -> TestResult {
        let mut bands = vec![band("Float32"), band("Float32"), band("Float32")];
        for band in &mut bands {
            band["noDataValue"] = json!("NaN");
        }
        bands[1]["colorInterpretation"] = json!("Palette");
        let source = SourceInfo::from_gdal_json(&original(bands), &warped())?;
        assert_eq!(source.bands, 3);
        assert_eq!(source.dtype, DType::Float32);
        assert_eq!(source.nodata, Some(Nodata::NaN));
        assert!(source.categorical);
        assert_eq!(source.per_band_range.len(), 3);
        Ok(())
    }

    #[test]
    fn rejects_mixed_and_unsupported_band_dtypes() {
        for bands in [
            vec![band("Byte"), band("UInt16")],
            vec![band("UInt64")],
            vec![band("CFloat32")],
        ] {
            assert!(SourceInfo::from_gdal_json(&original(bands), &warped()).is_err());
        }
    }

    #[test]
    fn rejects_conflicting_and_mixed_presence_nodata() {
        let mut conflict = vec![band("Byte"), band("Byte")];
        conflict[0]["noDataValue"] = json!(0);
        conflict[1]["noDataValue"] = json!(1);
        assert!(SourceInfo::from_gdal_json(&original(conflict), &warped()).is_err());

        let mut mixed = vec![band("Byte"), band("Byte")];
        mixed[0]["noDataValue"] = json!(0);
        assert!(SourceInfo::from_gdal_json(&original(mixed), &warped()).is_err());
    }

    #[test]
    fn nan_declarations_agree_only_for_float_bands() -> TestResult {
        let bands = vec![band("Float64"), band("Float64")];
        let mut source = original(bands);
        for band in source["bands"].as_array_mut().ok_or("missing bands")? {
            band["noDataValue"] = json!("nan");
        }
        let parsed = SourceInfo::from_gdal_json(&source, &warped())?;
        assert_eq!(parsed.nodata, Some(Nodata::NaN));

        let mut integer = original(vec![band("Byte")]);
        integer["bands"][0]["noDataValue"] = json!("NaN");
        assert!(SourceInfo::from_gdal_json(&integer, &warped()).is_err());

        for invalid in [json!("Infinity"), json!("-Infinity"), Value::Null] {
            let mut source = original(vec![band("Float32")]);
            source["bands"][0]["noDataValue"] = invalid;
            assert!(SourceInfo::from_gdal_json(&source, &warped()).is_err());
        }
        Ok(())
    }

    #[test]
    fn requires_a_crs_or_gcps_and_nonempty_bands() -> TestResult {
        let mut missing = original(vec![band("Byte")]);
        missing
            .as_object_mut()
            .ok_or("missing object")?
            .remove("coordinateSystem");
        assert!(SourceInfo::from_gdal_json(&missing, &warped()).is_err());

        missing["gcps"] = json!([{"pixel": 0, "line": 0}]);
        assert!(SourceInfo::from_gdal_json(&missing, &warped()).is_ok());

        assert!(SourceInfo::from_gdal_json(&original(vec![]), &warped()).is_err());
        let mut absent = original(vec![band("Byte")]);
        absent
            .as_object_mut()
            .ok_or("missing object")?
            .remove("bands");
        assert!(SourceInfo::from_gdal_json(&absent, &warped()).is_err());
        Ok(())
    }

    #[test]
    fn uses_fallback_ranges_and_rejects_nonfinite_statistics() -> TestResult {
        let mut fallback = band("Byte");
        let object = fallback.as_object_mut().ok_or("missing band")?;
        object.remove("minimum");
        object.remove("maximum");
        let parsed = SourceInfo::from_gdal_json(&original(vec![fallback]), &warped())?;
        assert_eq!(parsed.per_band_range, vec![[0.0, 1.0]]);

        let mut constant = band("Byte");
        constant["minimum"] = json!(5.0);
        constant["maximum"] = json!(5.0);
        let parsed = SourceInfo::from_gdal_json(&original(vec![constant]), &warped())?;
        assert_eq!(parsed.per_band_range, vec![[5.0, 6.0]]);

        for field in ["minimum", "maximum", "mean", "stdDev"] {
            let mut invalid = original(vec![band("Byte")]);
            invalid["bands"][0][field] = json!("NaN");
            assert!(
                SourceInfo::from_gdal_json(&invalid, &warped()).is_err(),
                "{field}"
            );
        }
        Ok(())
    }

    #[test]
    fn rejects_invalid_geotransform_and_corner_coordinates() -> TestResult {
        for (field, value) in [
            ("geoTransform", json!([0.0, 1.0])),
            ("geoTransform", json!([0.0, 1.0, 0.0, 0.0, 0.0, "NaN"])),
            ("cornerCoordinates", json!({})),
            (
                "cornerCoordinates",
                json!({"upperLeft": [0.0], "lowerRight": [1.0, 1.0]}),
            ),
            (
                "cornerCoordinates",
                json!({"upperLeft": [1.0, 1.0], "lowerRight": [1.0, 1.0]}),
            ),
        ] {
            let mut invalid = warped();
            invalid[field] = value;
            assert!(SourceInfo::from_gdal_json(&original(vec![band("Byte")]), &invalid).is_err());
        }

        let mut absent = warped();
        absent
            .as_object_mut()
            .ok_or("missing object")?
            .remove("geoTransform");
        assert!(SourceInfo::from_gdal_json(&original(vec![band("Byte")]), &absent).is_err());
        Ok(())
    }

    #[test]
    fn rejects_zero_or_nonfinite_native_pixel_size() {
        for pixel_size in [json!(0.0), json!("NaN"), json!("Infinity"), Value::Null] {
            let mut invalid = warped();
            invalid["geoTransform"][1] = pixel_size;
            assert!(SourceInfo::from_gdal_json(&original(vec![band("Byte")]), &invalid).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn emit_level_passes_selected_resampling_and_zstd_to_gdal() -> TestResult {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        use super::super::webmercator::TileRange;
        use super::{Gdal, LevelRequest};
        use crate::{ConvertOptions, Resampling};

        let temporary = tempfile::tempdir()?;
        let bin = temporary.path().join("bin");
        fs::create_dir(&bin)?;
        for (name, script) in [
            (
                "gdalwarp",
                "#!/bin/sh\ndir=\"$(dirname \"$0\")\"\nprintf '%s\\n' \"$@\" > \"$dir/warp.args\"\nfor last; do :; done\n: > \"$last\"\n",
            ),
            (
                "gdal_translate",
                "#!/bin/sh\ndir=\"$(dirname \"$0\")\"\nprintf '%s\\n' \"$@\" > \"$dir/translate.args\"\n",
            ),
        ] {
            let path = bin.join(name);
            fs::write(&path, script)?;
            let mut permissions = fs::metadata(&path)?.permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions)?;
        }
        let input = temporary.path().join("input.tif");
        fs::write(&input, b"fake")?;
        let source = SourceInfo::from_gdal_json(&original(vec![band("Byte")]), &warped())?;
        let range = TileRange {
            tx0: 0,
            tx1: 0,
            ty0: 0,
            ty1: 0,
            te_minx: 0.0,
            te_miny: 0.0,
            te_maxx: 1.0,
            te_maxy: 1.0,
        };
        for (resampling, expected) in [
            (Resampling::Nearest, "near"),
            (Resampling::Average, "average"),
        ] {
            let work = temporary.path().join(expected);
            fs::create_dir(&work)?;
            let gdal = Gdal {
                bin_dir: Some(bin.clone()),
            };
            gdal.emit_level(
                &input,
                &work,
                0,
                LevelRequest {
                    source: &source,
                    range,
                    nodata: None,
                    options: ConvertOptions {
                        maxzoom: Some(0),
                        resampling,
                        zstd_level: 1,
                    },
                },
            )?;
            let warp_args = fs::read_to_string(bin.join("warp.args"))?;
            assert!(
                warp_args.contains(&format!("-r\n{expected}\n")),
                "{warp_args}"
            );
            let translate_args = fs::read_to_string(bin.join("translate.args"))?;
            assert!(
                translate_args.contains("ZSTD_LEVEL=1\n"),
                "{translate_args}"
            );
        }
        Ok(())
    }
}
