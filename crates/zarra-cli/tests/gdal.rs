use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use zarra::RasterStore;

const SAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/sample.tif");
const FIXTURE_DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");
type TestResult = Result<(), Box<dyn std::error::Error>>;

fn read_json(path: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn gdal_tool(tool: &str) -> OsString {
    std::env::var_os("GDAL_BIN_DIR")
        .filter(|directory| !directory.is_empty())
        .map(|directory| PathBuf::from(directory).join(tool).into_os_string())
        .unwrap_or_else(|| OsString::from(tool))
}

fn convert(input: &Path, output: &Path) -> Result<Output, Box<dyn std::error::Error>> {
    Ok(Command::new(env!("CARGO_BIN_EXE_zarra"))
        .arg("convert")
        .arg(input)
        .arg("-o")
        .arg(output)
        .output()?)
}

fn tile(input: &Path, z: u8, x: u32, y: u32, output: &Path) -> TestResult {
    let result = Command::new(env!("CARGO_BIN_EXE_zarra"))
        .arg("tile")
        .arg(input)
        .arg("--z")
        .arg(z.to_string())
        .arg("--x")
        .arg(x.to_string())
        .arg("--y")
        .arg(y.to_string())
        .arg("-o")
        .arg(output)
        .output()?;
    assert!(
        result.status.success(),
        "tile failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8(result.stdout)?,
        format!("{}\n", output.display())
    );
    Ok(())
}

fn decode_png(path: &Path) -> Result<(u32, u32, Vec<u8>), Box<dyn std::error::Error>> {
    let mut reader = png::Decoder::new(Cursor::new(fs::read(path)?)).read_info()?;
    let mut pixels = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or("missing PNG buffer size")?
    ];
    let info = reader.next_frame(&mut pixels)?;
    assert_eq!(info.color_type, png::ColorType::Rgba);
    assert_eq!(info.bit_depth, png::BitDepth::Eight);
    pixels.truncate(info.buffer_size());
    Ok((info.width, info.height, pixels))
}

fn assert_conversion_success(output: &Output, bands: usize) {
    assert!(
        output.status.success(),
        "conversion failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    for expected in ["output:", "levels:", "files:", "dtype:", "maxzoom:"] {
        assert!(stdout.contains(expected), "missing {expected}: {stdout}");
    }
    assert!(stdout.contains(&format!("bands: {bands}")), "{stdout}");
}

fn validate_store(output: &Path, bands: usize) -> TestResult {
    let root = read_json(&output.join("zarr.json"))?;
    assert_eq!(root["zarr_format"], 3);
    assert_eq!(root["node_type"], "group");
    let manifest = &root["attributes"]["zarra"];
    assert_eq!(manifest["profile_version"], 1);
    assert_eq!(manifest["crs"], 3857);
    assert_eq!(manifest["tile_size"], 256);
    assert_eq!(manifest["bands"], bands);
    assert_eq!(
        manifest["per_band_range"],
        serde_json::json!(vec![[0.0, 1.0]; bands])
    );
    assert_eq!(manifest["nodata"], serde_json::json!(2.0));
    assert_eq!(
        manifest["tile_encoding"]["channels"],
        if bands == 1 { "rg16" } else { "rgba8" }
    );
    let maxzoom = manifest["maxzoom"].as_u64().ok_or("missing maxzoom")?;
    assert_eq!(
        manifest["levels"].as_array().ok_or("missing levels")?.len(),
        maxzoom as usize + 1
    );

    let level = read_json(&output.join(maxzoom.to_string()).join("zarr.json"))?;
    assert_eq!(level["zarr_format"], 3);
    assert_eq!(level["node_type"], "array");
    let shape = level["shape"].as_array().ok_or("missing shape")?;
    let outer = level["chunk_grid"]["configuration"]["chunk_shape"]
        .as_array()
        .ok_or("missing outer shape")?;
    let inner = level["codecs"][0]["configuration"]["chunk_shape"]
        .as_array()
        .ok_or("missing inner shape")?;
    assert_eq!(level["codecs"][0]["name"], "sharding_indexed");
    if bands == 1 {
        assert_eq!(shape.len(), 2, "one-band output must be 2D: {shape:?}");
        assert_eq!(outer.len(), 2);
        assert_eq!(
            inner,
            serde_json::json!([256, 256])
                .as_array()
                .ok_or("invalid test shape")?
        );
    } else {
        assert_eq!(shape.len(), 3, "multiband output must be 3D: {shape:?}");
        assert_eq!(shape[0], bands);
        assert_eq!(outer.len(), 3);
        assert_eq!(outer[0], bands);
        assert_eq!(
            inner,
            serde_json::json!([bands, 256, 256])
                .as_array()
                .ok_or("invalid test shape")?
        );
    }

    let mut store = RasterStore::open(output)?;
    {
        let opened = store.manifest();
        assert_eq!(opened.bands, bands);
        assert_eq!(opened.tile_size, 256);
        assert_eq!(opened.maxzoom, u8::try_from(maxzoom)?);
    }
    for z in 0..=u8::try_from(maxzoom)? {
        let meta = store.level_meta(z)?;
        assert_eq!(meta.z, z);
        assert_eq!(meta.bands, bands);
        assert_eq!(meta.inner, 256);
        assert!(meta.width > 0 && meta.height > 0);
    }
    let meta = store.level_meta(u8::try_from(maxzoom)?)?;
    let level = u8::try_from(maxzoom)?;
    let window = store.read_window(level, 0, 0, 1, 1)?;
    assert_eq!(window.bands, (0..bands).collect::<Vec<_>>());
    assert_eq!(window.values, vec![2.0; bands]);
    let valid = store.read_window(level, 255, 255, 1, 1)?;
    assert_eq!(valid.values, vec![0.0; bands]);

    let selected = if bands == 1 {
        vec![0, 0]
    } else {
        vec![2, 0, 2]
    };
    let selected_window = store.read_bands(level, 0, 0, 1, 1, &selected)?;
    assert_eq!(selected_window.bands, selected);
    assert_eq!(
        selected_window.values,
        vec![2.0; selected_window.bands.len()]
    );

    let sparse_col = meta.width / 2 / u64::from(meta.inner);
    let sparse_row = meta.height / 2 / u64::from(meta.inner);
    assert!(store.read_chunk(level, sparse_col, sparse_row)?.is_none());
    let sparse = store.read_window(
        level,
        i64::try_from(meta.width / 2)?,
        i64::try_from(meta.height / 2)?,
        1,
        1,
    )?;
    assert!(sparse.values.iter().all(|value| value.is_nan()));

    let offset = store
        .manifest()
        .level(level)
        .ok_or("missing native level offset")?;
    let overzoom = level.checked_add(1).ok_or("overzoom exceeds u8")?;
    let over_x = offset
        .xoff
        .checked_mul(2)
        .and_then(|x| x.checked_add(1))
        .ok_or("overzoom x overflows")?;
    let over_y = offset
        .yoff
        .checked_mul(2)
        .and_then(|y| y.checked_add(1))
        .ok_or("overzoom y overflows")?;
    let parent = output.parent().ok_or("converted store has no parent")?;
    let dimensions = if bands == 1 { "2d" } else { "3d" };
    for (name, z, x, y) in [
        ("native", level, offset.xoff, offset.yoff),
        ("overzoom", overzoom, over_x, over_y),
    ] {
        let path = parent.join(format!("{dimensions}-{name}.png"));
        tile(output, z, x, y, &path)?;
        let (width, height, pixels) = decode_png(&path)?;
        assert_eq!((width, height), (256, 256));
        assert_eq!(pixels.len(), 256 * 256 * 4);
        // Border value 2 is declared nodata; valid value 0 maps to the low endpoint.
        assert_eq!(&pixels[..4], [0, 0, 0, 0]);
        assert_eq!(&pixels[pixels.len() - 4..], [0, 0, 0, 255]);
    }
    Ok(())
}

fn snapshot(root: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>, Box<dyn std::error::Error>> {
    fn visit(root: &Path, directory: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) -> TestResult {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            assert!(
                !name.starts_with("_warp_"),
                "warp artifact published: {path:?}"
            );
            assert!(
                name != "data" && name != "X" && name != "Y",
                "GDAL sidecar published: {path:?}"
            );
            assert!(
                !name.starts_with(".zarra-build-"),
                "temporary path published: {path:?}"
            );
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                visit(root, &path, files)?;
            } else if file_type.is_file() {
                files.insert(path.strip_prefix(root)?.to_path_buf(), fs::read(&path)?);
            }
        }
        Ok(())
    }

    let mut files = BTreeMap::new();
    visit(root, root, &mut files)?;
    assert!(!files.is_empty());
    Ok(files)
}

fn assert_no_build_directories(parent: &Path) -> TestResult {
    for entry in fs::read_dir(parent)? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        assert!(
            !name.starts_with(".zarra-build-"),
            "build directory leaked: {name}"
        );
    }
    Ok(())
}

#[test]
#[ignore = "requires GDAL >= 3.13"]
fn converts_2d_and_3d_stores_and_never_overwrites() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let one = temporary.path().join("one.zarr");
    let three = temporary.path().join("three.zarr");
    let rgb = temporary.path().join("rgb.tif");
    let sample = Path::new(SAMPLE);
    let fixture_before = snapshot(Path::new(FIXTURE_DATA))?;

    assert_conversion_success(&convert(sample, &one)?, 1);
    let translated = Command::new(gdal_tool("gdal_translate"))
        .env("GDAL_PAM_ENABLED", "NO")
        .arg("-of")
        .arg("COG")
        .arg("-b")
        .arg("1")
        .arg("-b")
        .arg("1")
        .arg("-b")
        .arg("1")
        .arg(sample)
        .arg(&rgb)
        .output()?;
    assert!(
        translated.status.success(),
        "three-band COG creation failed: {}",
        String::from_utf8_lossy(&translated.stderr)
    );
    assert_conversion_success(&convert(&rgb, &three)?, 3);

    validate_store(&one, 1)?;
    validate_store(&three, 3)?;
    snapshot(&one)?;
    snapshot(&three)?;
    assert_no_build_directories(temporary.path())?;

    let one_before = snapshot(&one)?;
    let three_before = snapshot(&three)?;
    let repeated_one = convert(sample, &one)?;
    assert!(!repeated_one.status.success());
    assert!(String::from_utf8_lossy(&repeated_one.stderr).contains("error:"));
    let repeated_three = convert(&rgb, &three)?;
    assert!(!repeated_three.status.success());
    assert_eq!(snapshot(&one)?, one_before);
    assert_eq!(snapshot(&three)?, three_before);
    assert_no_build_directories(temporary.path())?;
    assert_eq!(snapshot(Path::new(FIXTURE_DATA))?, fixture_before);
    Ok(())
}

#[test]
#[ignore = "requires GDAL >= 3.13"]
fn convert_cli_applies_zoom_resampling_and_compression_options() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let output = temporary.path().join("custom.zarr");
    let converted = Command::new(env!("CARGO_BIN_EXE_zarra"))
        .arg("convert")
        .arg(SAMPLE)
        .arg("-o")
        .arg(&output)
        .args([
            "--maxzoom",
            "0",
            "--resampling",
            "nearest",
            "--zstd-level",
            "1",
        ])
        .output()?;
    assert_conversion_success(&converted, 1);
    let root = read_json(&output.join("zarr.json"))?;
    assert_eq!(root["attributes"]["zarra"]["maxzoom"], 0);
    assert_eq!(
        root["attributes"]["zarra"]["levels"]
            .as_array()
            .ok_or("missing levels")?
            .len(),
        1
    );
    let level = read_json(&output.join("0/zarr.json"))?;
    assert_eq!(
        level["codecs"][0]["configuration"]["codecs"][1]["configuration"]["level"],
        1
    );
    Ok(())
}
