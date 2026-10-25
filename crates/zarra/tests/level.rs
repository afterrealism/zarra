use serde_json::Value;
use zarra::{Channels, DType, Error, LevelMeta, RasterManifest};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn level_json() -> Result<Value, Box<dyn std::error::Error>> {
    Ok(serde_json::from_str(include_str!(
        "data/refpy.zarr/0/zarr.json"
    ))?)
}

fn manifest() -> Result<RasterManifest, Box<dyn std::error::Error>> {
    let root: Value = serde_json::from_str(include_str!("data/refpy.zarr/zarr.json"))?;
    let attributes = root["attributes"].as_object().cloned().unwrap_or_default();
    Ok(RasterManifest::from_attributes(&attributes)?)
}

fn as_2d(mut level: Value) -> Result<Value, Box<dyn std::error::Error>> {
    for pointer in [
        "/shape",
        "/chunk_grid/configuration/chunk_shape",
        "/codecs/0/configuration/chunk_shape",
    ] {
        level
            .pointer_mut(pointer)
            .and_then(Value::as_array_mut)
            .ok_or("missing test shape")?
            .remove(0);
    }
    Ok(level)
}

#[test]
fn accepts_reference_end_index_zstd_and_start_index_and_bytes_only() -> TestResult {
    let manifest = manifest()?;
    let level = level_json()?;
    let meta = LevelMeta::from_json(&level, 0, &manifest)?;
    assert_eq!(meta.z, 0);
    assert_eq!(meta.bands, 1);
    assert_eq!((meta.width, meta.height, meta.inner), (64, 64, 16));
    assert_eq!(meta.dtype, DType::Uint16);

    let mut start = level.clone();
    start["codecs"][0]["configuration"]["index_location"] = Value::from("start");
    LevelMeta::from_json(&start, 0, &manifest)?;

    let mut bytes_only = level;
    bytes_only["codecs"][0]["configuration"]["codecs"]
        .as_array_mut()
        .ok_or("missing inner codecs")?
        .remove(1);
    LevelMeta::from_json(&bytes_only, 0, &manifest)?;
    Ok(())
}

#[test]
fn accepts_gdal_style_2d_single_band_metadata() -> TestResult {
    let level = as_2d(level_json()?)?;
    let meta = LevelMeta::from_json(&level, 0, &manifest()?)?;
    assert_eq!((meta.bands, meta.width, meta.height), (1, 64, 64));
    Ok(())
}

#[test]
fn rejects_wrong_endian_in_inner_or_index_bytes() -> TestResult {
    for pointer in [
        "/codecs/0/configuration/codecs/0/configuration/endian",
        "/codecs/0/configuration/index_codecs/0/configuration/endian",
    ] {
        let mut level = level_json()?;
        *level.pointer_mut(pointer).ok_or("missing endian")? = Value::from("big");
        assert!(
            LevelMeta::from_json(&level, 0, &manifest()?).is_err(),
            "{pointer}"
        );
    }
    Ok(())
}

#[test]
fn rejects_non_default_chunk_keys() -> TestResult {
    let mut name = level_json()?;
    name["chunk_key_encoding"]["name"] = Value::from("v2");
    assert!(LevelMeta::from_json(&name, 0, &manifest()?).is_err());

    let mut separator = level_json()?;
    separator["chunk_key_encoding"]["configuration"]["separator"] = Value::from(".");
    assert!(LevelMeta::from_json(&separator, 0, &manifest()?).is_err());
    Ok(())
}

#[test]
fn rejects_blosc_other_inner_codecs_and_non_sharding_outer_codecs() -> TestResult {
    for codec in ["blosc", "gzip", "transpose"] {
        let mut level = level_json()?;
        level["codecs"][0]["configuration"]["codecs"][1]["name"] = Value::from(codec);
        assert!(
            LevelMeta::from_json(&level, 0, &manifest()?).is_err(),
            "{codec}"
        );
    }

    let mut outer = level_json()?;
    outer["codecs"][0]["name"] = Value::from("bytes");
    assert!(LevelMeta::from_json(&outer, 0, &manifest()?).is_err());
    Ok(())
}

#[test]
fn rejects_non_square_and_non_multiple_spatial_chunks() -> TestResult {
    let mut non_square = level_json()?;
    non_square["codecs"][0]["configuration"]["chunk_shape"] = serde_json::json!([1, 16, 32]);
    assert!(LevelMeta::from_json(&non_square, 0, &manifest()?).is_err());

    let mut non_multiple = level_json()?;
    non_multiple["chunk_grid"]["configuration"]["chunk_shape"] = serde_json::json!([1, 40, 64]);
    assert!(LevelMeta::from_json(&non_multiple, 0, &manifest()?).is_err());
    Ok(())
}

#[test]
fn rejects_band_shape_and_dimension_mismatches() -> TestResult {
    for pointer in [
        "/shape/0",
        "/chunk_grid/configuration/chunk_shape/0",
        "/codecs/0/configuration/chunk_shape/0",
    ] {
        let mut level = level_json()?;
        *level.pointer_mut(pointer).ok_or("missing band shape")? = Value::from(2);
        assert!(
            LevelMeta::from_json(&level, 0, &manifest()?).is_err(),
            "{pointer}"
        );
    }

    let mut multiband = manifest()?;
    multiband.bands = 2;
    multiband.per_band_range.push([0.0, 63063.0]);
    multiband.tile_encoding.channels = Channels::Rgba8;
    assert!(LevelMeta::from_json(&as_2d(level_json()?)?, 0, &multiband).is_err());

    let mut one_dimensional = as_2d(level_json()?)?;
    one_dimensional["shape"] = serde_json::json!([64]);
    assert!(LevelMeta::from_json(&one_dimensional, 0, &manifest()?).is_err());
    Ok(())
}

#[test]
fn rejects_unsupported_and_mismatched_dtypes() -> TestResult {
    for dtype in ["uint64", "int64", "complex64", "uint8"] {
        let mut level = level_json()?;
        level["data_type"] = Value::from(dtype);
        assert!(
            LevelMeta::from_json(&level, 0, &manifest()?).is_err(),
            "{dtype}"
        );
    }
    Ok(())
}

#[test]
fn rejects_storage_transformers_and_noncanonical_container_metadata() -> TestResult {
    let mut transformed = level_json()?;
    transformed["storage_transformers"] = serde_json::json!([{"name": "example"}]);
    assert!(LevelMeta::from_json(&transformed, 0, &manifest()?).is_err());

    for (field, value) in [
        ("zarr_format", Value::from(2)),
        ("node_type", Value::from("group")),
    ] {
        let mut level = level_json()?;
        level[field] = value;
        assert!(LevelMeta::from_json(&level, 0, &manifest()?).is_err());
    }

    let mut grid = level_json()?;
    grid["chunk_grid"]["name"] = Value::from("irregular");
    assert!(LevelMeta::from_json(&grid, 0, &manifest()?).is_err());

    assert!(LevelMeta::from_json(&level_json()?, 1, &manifest()?).is_err());
    Ok(())
}

#[test]
fn rejects_wrong_index_layout_and_codec_chain() -> TestResult {
    let mut location = level_json()?;
    location["codecs"][0]["configuration"]["index_location"] = Value::from("middle");
    assert!(LevelMeta::from_json(&location, 0, &manifest()?).is_err());

    let mut index_codecs = level_json()?;
    index_codecs["codecs"][0]["configuration"]["index_codecs"][1]["name"] = Value::from("zstd");
    assert!(LevelMeta::from_json(&index_codecs, 0, &manifest()?).is_err());
    Ok(())
}

#[test]
fn enforces_the_decoded_chunk_value_cap_with_a_specific_error() -> TestResult {
    let mut manifest = manifest()?;
    manifest.tile_size = 2048;
    let mut level = as_2d(level_json()?)?;
    level["shape"] = serde_json::json!([2048, 2048]);
    level["chunk_grid"]["configuration"]["chunk_shape"] = serde_json::json!([2048, 2048]);
    level["codecs"][0]["configuration"]["chunk_shape"] = serde_json::json!([2048, 2048]);
    LevelMeta::from_json(&level, 0, &manifest)?;

    manifest.tile_size = 4096;
    level["shape"] = serde_json::json!([4096, 4096]);
    level["chunk_grid"]["configuration"]["chunk_shape"] = serde_json::json!([4096, 4096]);
    level["codecs"][0]["configuration"]["chunk_shape"] = serde_json::json!([4096, 4096]);
    let error = match LevelMeta::from_json(&level, 0, &manifest) {
        Ok(_) => return Err("oversized metadata was accepted".into()),
        Err(error) => error,
    };
    match error {
        Error::Format(message) => assert!(
            message.contains("4194304"),
            "unexpected allocation-cap message: {message}"
        ),
        other => return Err(format!("wrong allocation-cap error category: {other}").into()),
    }
    Ok(())
}
