use zarra::{Channels, DType, Nodata, RasterManifest};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn attributes() -> serde_json::Map<String, serde_json::Value> {
    let root: serde_json::Value = serde_json::from_str(include_str!("data/refpy.zarr/zarr.json"))
        .unwrap_or(serde_json::Value::Null);
    root["attributes"].as_object().cloned().unwrap_or_default()
}

fn valid_manifest() -> Result<RasterManifest, Box<dyn std::error::Error>> {
    Ok(RasterManifest::from_attributes(&attributes())?)
}

#[test]
fn manifest_roundtrip_and_lookup() -> TestResult {
    let manifest = RasterManifest::from_attributes(&attributes())?;
    assert_eq!(manifest.dtype, DType::Uint16);
    assert_eq!(manifest.tile_size, 16);
    assert_eq!(manifest.level(0).map(|l| (l.xoff, l.yoff)), Some((0, 0)));
    assert!(manifest.level(1).is_none());

    let encoded = manifest.to_value()?;
    assert!(encoded.get("zarra").is_none());
    assert_eq!(encoded["dtype"], serde_json::json!("uint16"));
    assert_eq!(
        encoded["tile_encoding"]["channels"],
        serde_json::json!("rg16")
    );

    let reparsed: RasterManifest = serde_json::from_value(encoded)?;
    reparsed.validate()?;
    assert_eq!(reparsed, manifest);
    Ok(())
}

#[test]
fn manifest_rejects_missing_level_and_u64() -> TestResult {
    let mut attrs = attributes();
    attrs["zarra"]["maxzoom"] = serde_json::json!(1);
    assert!(RasterManifest::from_attributes(&attrs).is_err());
    let mut attrs = attributes();
    attrs["zarra"]["dtype"] = serde_json::json!("uint64");
    assert!(RasterManifest::from_attributes(&attrs).is_err());
    Ok(())
}

#[test]
fn dtype_gdal_names_and_strings_cover_exactly_the_profile_types() {
    let cases = [
        ("Byte", DType::Uint8, "uint8"),
        ("Int8", DType::Int8, "int8"),
        ("UInt16", DType::Uint16, "uint16"),
        ("Int16", DType::Int16, "int16"),
        ("UInt32", DType::Uint32, "uint32"),
        ("Int32", DType::Int32, "int32"),
        ("Float32", DType::Float32, "float32"),
        ("Float64", DType::Float64, "float64"),
    ];
    for (gdal, dtype, profile) in cases {
        assert_eq!(DType::from_gdal(gdal), Some(dtype));
        assert_eq!(dtype.as_str(), profile);
    }
    for unsupported in ["UInt64", "Int64", "CInt16", "CFloat32", "unknown"] {
        assert_eq!(DType::from_gdal(unsupported), None);
    }
}

#[test]
fn manifest_requires_an_own_zarra_object() {
    let mut missing = attributes();
    missing.remove("zarra");
    assert!(RasterManifest::from_attributes(&missing).is_err());

    let mut null = attributes();
    null["zarra"] = serde_json::Value::Null;
    assert!(RasterManifest::from_attributes(&null).is_err());
}

#[test]
fn manifest_rejects_invalid_profile_zoom_and_tile_size() {
    for (field, value) in [
        ("profile_version", 2),
        ("crs", 4326),
        ("minzoom", 1),
        ("maxzoom", 23),
        ("tile_size", 0),
    ] {
        let mut attrs = attributes();
        attrs["zarra"][field] = serde_json::json!(value);
        assert!(RasterManifest::from_attributes(&attrs).is_err(), "{field}");
    }
}

#[test]
fn manifest_rejects_incomplete_and_duplicate_level_sequences() {
    let mut attrs = attributes();
    attrs["zarra"]["maxzoom"] = serde_json::json!(2);
    attrs["zarra"]["levels"] = serde_json::json!([
        { "z": 0, "xoff": 0, "yoff": 0 },
        { "z": 0, "xoff": 0, "yoff": 0 },
        { "z": 2, "xoff": 0, "yoff": 0 }
    ]);
    assert!(RasterManifest::from_attributes(&attrs).is_err());
}

#[test]
fn manifest_rejects_offsets_outside_the_zoom_grid() {
    for field in ["xoff", "yoff"] {
        let mut attrs = attributes();
        attrs["zarra"]["maxzoom"] = serde_json::json!(1);
        attrs["zarra"]["levels"] = serde_json::json!([
            { "z": 0, "xoff": 0, "yoff": 0 },
            { "z": 1, "xoff": 0, "yoff": 0 }
        ]);
        attrs["zarra"]["levels"][1][field] = serde_json::json!(2);
        assert!(RasterManifest::from_attributes(&attrs).is_err(), "{field}");
    }
}

#[test]
fn manifest_rejects_invalid_band_counts_and_channels() -> TestResult {
    let mut zero_bands = valid_manifest()?;
    zero_bands.bands = 0;
    assert!(zero_bands.validate().is_err());

    let mut wrong_single_channel = valid_manifest()?;
    wrong_single_channel.tile_encoding.channels = Channels::Rgba8;
    assert!(wrong_single_channel.validate().is_err());

    let mut missing_band_range = valid_manifest()?;
    missing_band_range.bands = 2;
    missing_band_range.tile_encoding.channels = Channels::Rgba8;
    assert!(missing_band_range.validate().is_err());

    let mut wrong_multi_channel = valid_manifest()?;
    wrong_multi_channel.bands = 2;
    wrong_multi_channel.per_band_range.push([0.0, 63063.0]);
    assert_eq!(wrong_multi_channel.tile_encoding.channels, Channels::Rg16);
    assert!(wrong_multi_channel.validate().is_err());
    Ok(())
}

#[test]
fn manifest_rejects_nonfinite_and_unordered_ranges() -> TestResult {
    let original = valid_manifest()?;
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut encoded = original.clone();
        encoded.tile_encoding.encoded_min = value;
        assert!(encoded.validate().is_err());

        let mut per_band = original.clone();
        per_band.per_band_range[0][1] = value;
        assert!(per_band.validate().is_err());
    }

    let mut encoded = original.clone();
    encoded.tile_encoding.encoded_max = encoded.tile_encoding.encoded_min;
    assert!(encoded.validate().is_err());

    let mut per_band = original;
    per_band.per_band_range[0] = [2.0, 1.0];
    assert!(per_band.validate().is_err());
    Ok(())
}

#[test]
fn manifest_requires_encoded_range_to_equal_band_zero() -> TestResult {
    let mut manifest = valid_manifest()?;
    manifest.tile_encoding.encoded_max += 1.0;
    assert!(manifest.validate().is_err());
    Ok(())
}

#[test]
fn nodata_json_roundtrip_and_type_rules() -> TestResult {
    let mut numeric = attributes();
    numeric["zarra"]["nodata"] = serde_json::json!(65535);
    let numeric = RasterManifest::from_attributes(&numeric)?;
    assert_eq!(numeric.nodata, Some(Nodata::Number(65535.0)));
    assert_eq!(numeric.to_value()?["nodata"], serde_json::json!(65535.0));

    let mut integer_nan = attributes();
    integer_nan["zarra"]["nodata"] = serde_json::json!("NaN");
    assert!(RasterManifest::from_attributes(&integer_nan).is_err());

    let mut float_nan = attributes();
    float_nan["zarra"]["dtype"] = serde_json::json!("float32");
    float_nan["zarra"]["nodata"] = serde_json::json!("NaN");
    let float_nan = RasterManifest::from_attributes(&float_nan)?;
    assert_eq!(float_nan.nodata, Some(Nodata::NaN));
    assert_eq!(float_nan.to_value()?["nodata"], serde_json::json!("NaN"));

    let mut nonfinite = valid_manifest()?;
    nonfinite.nodata = Some(Nodata::Number(f64::INFINITY));
    assert!(nonfinite.validate().is_err());
    Ok(())
}

#[test]
fn manifest_rejects_invalid_or_nonfinite_extents() -> TestResult {
    let original = valid_manifest()?;
    for extent in [
        [-181.0, 0.0, 0.0, 1.0],
        [0.0, 0.0, 181.0, 1.0],
        [0.0, -91.0, 1.0, 1.0],
        [0.0, 0.0, 1.0, 91.0],
        [1.0, 0.0, -1.0, 1.0],
        [0.0, 1.0, 1.0, -1.0],
        [f64::NAN, 0.0, 1.0, 1.0],
        [0.0, 0.0, f64::INFINITY, 1.0],
    ] {
        let mut manifest = original.clone();
        manifest.extent = Some(extent);
        assert!(manifest.validate().is_err());
    }

    let mut optional = original;
    optional.extent = None;
    optional.validate()?;
    Ok(())
}
