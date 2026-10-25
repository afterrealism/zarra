use std::fs;
use std::path::Path;

use serde_json::Value;
use tempfile::TempDir;
use zarra::{parse_band_list, ChunkRef, DType, RasterStore};

const STORE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/refpy.zarr");
type TestResult = Result<(), Box<dyn std::error::Error>>;

fn expected(y: u64, x: u64) -> f64 {
    ((y * 1000 + x) & 0xffff) as f64
}

fn copy_store(with_shard: bool) -> Result<TempDir, Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    fs::create_dir(temporary.path().join("0"))?;
    fs::copy(
        Path::new(STORE).join("zarr.json"),
        temporary.path().join("zarr.json"),
    )?;
    fs::copy(
        Path::new(STORE).join("0/zarr.json"),
        temporary.path().join("0/zarr.json"),
    )?;
    if with_shard {
        fs::create_dir_all(temporary.path().join("0/c/0/0"))?;
        fs::copy(
            Path::new(STORE).join("0/c/0/0/0"),
            temporary.path().join("0/c/0/0/0"),
        )?;
    }
    Ok(temporary)
}

fn assert_window(store: &mut RasterStore, x0: i64, y0: i64, width: u32, height: u32) -> TestResult {
    let window = store.read_window(0, x0, y0, width, height)?;
    assert_eq!(window.bands, [0]);
    assert_eq!(window.values.len(), (width * height) as usize);
    for row in 0..height {
        for col in 0..width {
            let index = (row * width + col) as usize;
            assert_eq!(
                window.values[index],
                expected((y0 + i64::from(row)) as u64, (x0 + i64::from(col)) as u64)
            );
        }
    }
    Ok(())
}

#[test]
fn opens_metadata_enumerates_and_reads_every_reference_chunk() -> TestResult {
    let mut store = RasterStore::open(Path::new(STORE))?;
    assert_eq!(store.manifest().bands, 1);
    assert_eq!(store.manifest().dtype, DType::Uint16);
    let meta = store.level_meta(0)?;
    assert_eq!((meta.bands, meta.width, meta.height), (1, 64, 64));
    assert_eq!((meta.inner, meta.dtype), (16, DType::Uint16));

    let refs = store.enumerate_chunks(0)?;
    assert_eq!(refs.len(), 16);
    for (index, reference) in refs.iter().enumerate() {
        assert_eq!(
            reference,
            &ChunkRef {
                col: (index % 4) as u64,
                row: (index / 4) as u64,
            }
        );
    }

    for row in 0..4u64 {
        for col in 0..4u64 {
            let chunk = store
                .read_chunk(0, col, row)?
                .ok_or("reference chunk is missing")?;
            assert_eq!((chunk.bands, chunk.width, chunk.height), (1, 16, 16));
            assert_eq!(chunk.values.len(), 16 * 16);
            for inner_row in 0..16u32 {
                for inner_col in 0..16u32 {
                    assert_eq!(
                        chunk.value(0, inner_row, inner_col),
                        Some(expected(
                            row * 16 + u64::from(inner_row),
                            col * 16 + u64::from(inner_col)
                        ))
                    );
                }
            }
            assert_eq!(chunk.value(1, 0, 0), None);
            assert_eq!(chunk.value(0, 16, 0), None);
            assert_eq!(chunk.value(0, 0, 16), None);
        }
    }
    assert!(store.read_chunk(0, 9, 9)?.is_none());
    assert!(store.level_meta(1).is_err());
    Ok(())
}

#[test]
fn edge_chunks_keep_full_inner_shape() -> TestResult {
    let temporary = copy_store(true)?;
    let path = temporary.path().join("0/zarr.json");
    let mut level: Value = serde_json::from_slice(&fs::read(&path)?)?;
    level["shape"] = serde_json::json!([1, 20, 20]);
    fs::write(&path, serde_json::to_vec_pretty(&level)?)?;

    let mut store = RasterStore::open(temporary.path())?;
    let chunk = store
        .read_chunk(0, 1, 1)?
        .ok_or("missing padded edge chunk")?;
    assert_eq!((chunk.width, chunk.height), (16, 16));
    assert_eq!(chunk.values.len(), 16 * 16);
    assert!(chunk.value(0, 15, 15).is_some());

    let window = store.read_window(0, 16, 16, 8, 8)?;
    for row in 0..8u64 {
        for col in 0..8u64 {
            let index = (row * 8 + col) as usize;
            if row < 4 && col < 4 {
                assert_eq!(window.values[index], expected(16 + row, 16 + col));
            } else {
                assert!(window.values[index].is_nan(), "index {index}");
            }
        }
    }
    Ok(())
}

#[test]
fn reads_gdal_style_one_band_2d_arrays() -> TestResult {
    let temporary = copy_store(true)?;
    let path = temporary.path().join("0/zarr.json");
    let mut level: Value = serde_json::from_slice(&fs::read(&path)?)?;
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
    fs::write(&path, serde_json::to_vec_pretty(&level)?)?;
    fs::remove_dir_all(temporary.path().join("0/c"))?;
    fs::create_dir_all(temporary.path().join("0/c/0"))?;
    fs::copy(
        Path::new(STORE).join("0/c/0/0/0"),
        temporary.path().join("0/c/0/0"),
    )?;

    let mut store = RasterStore::open(temporary.path())?;
    assert_eq!(store.level_meta(0)?.bands, 1);
    assert_eq!(
        store.read_chunk(0, 0, 0)?.ok_or("missing 2D chunk")?.values[2 * 16 + 9],
        2009.0
    );
    assert_window(&mut store, 12, 12, 8, 8)
}

#[test]
fn reads_exact_windows_neighborhoods_and_repeated_bands() -> TestResult {
    let mut store = RasterStore::open(Path::new(STORE))?;
    assert_window(&mut store, 4, 4, 8, 8)?;
    assert_window(&mut store, 12, 12, 8, 8)?;

    let neighborhood = store.read_neighborhood(0, 0, 0, 1)?;
    assert_eq!((neighborhood.width, neighborhood.height), (3, 3));
    assert_eq!(neighborhood.values.len(), 9);
    for index in [0, 1, 2, 3, 6] {
        assert!(neighborhood.values[index].is_nan(), "index {index}");
    }
    for (index, value) in [(4, 0.0), (5, 1.0), (7, 1000.0), (8, 1001.0)] {
        assert_eq!(neighborhood.values[index], value);
    }

    let bands = store.read_bands(0, 2, 2, 4, 4, &[0, 0])?;
    assert_eq!(bands.bands, [0, 0]);
    assert_eq!(bands.values.len(), 32);
    for row in 0..4u64 {
        for col in 0..4u64 {
            let index = ((row * 4 + col) * 2) as usize;
            let expected = expected(row + 2, col + 2);
            assert_eq!(bands.values[index], expected);
            assert_eq!(bands.values[index + 1], expected);
        }
    }
    Ok(())
}

#[test]
fn missing_shards_are_sparse_and_padded_with_nan() -> TestResult {
    let temporary = copy_store(false)?;
    let mut store = RasterStore::open(temporary.path())?;
    assert!(store.read_chunk(0, 0, 0)?.is_none());
    let window = store.read_window(0, -1, -1, 3, 3)?;
    assert!(window.values.iter().all(|value| value.is_nan()));
    let bands = store.read_bands(0, 0, 0, 2, 2, &[0, 0])?;
    assert!(bands.values.iter().all(|value| value.is_nan()));

    let outside = store.read_window(0, i64::MAX, i64::MIN, 2, 2)?;
    assert!(outside.values.iter().all(|value| value.is_nan()));
    Ok(())
}

#[test]
fn rejects_invalid_queries_dimensions_radius_and_band_syntax() -> TestResult {
    let mut store = RasterStore::open(Path::new(STORE))?;
    assert!(store.read_window(0, 0, 0, 0, 1).is_err());
    assert!(store.read_window(0, 0, 0, 1, 0).is_err());
    assert!(store.read_window(0, 0, 0, 4097, 4096).is_err());
    assert!(store.read_bands(0, 0, 0, 1, 1, &[]).is_err());
    assert!(store.read_bands(0, 0, 0, 1, 1, &[1]).is_err());
    assert!(store.read_bands(0, 0, 0, 3000, 3000, &[0, 0]).is_err());
    assert!(store.read_neighborhood(0, 0, 0, u32::MAX).is_err());
    assert!(store.read_neighborhood(0, 0, 0, 4096).is_err());
    assert!(store.read_neighborhood(0, i64::MIN, 0, 1).is_err());

    assert_eq!(parse_band_list("0,B2,b1,0")?, [0, 2, 1, 0]);
    let overflow = (usize::MAX as u128 + 1).to_string();
    for invalid in [
        "",
        ",",
        "0,",
        ",0",
        "0,,1",
        "-1",
        "+1",
        "b",
        "B",
        "bb1",
        "1.0",
        " 1",
        "1 ",
        "１２",
        overflow.as_str(),
    ] {
        assert!(parse_band_list(invalid).is_err(), "accepted {invalid:?}");
    }
    Ok(())
}

#[test]
fn rejects_malformed_levels_and_v2_root_groups() -> TestResult {
    let malformed = copy_store(false)?;
    let level_path = malformed.path().join("0/zarr.json");
    let mut level: Value = serde_json::from_slice(&fs::read(&level_path)?)?;
    level["shape"] = serde_json::json!([2, 64, 64]);
    fs::write(&level_path, serde_json::to_vec(&level)?)?;
    assert!(RasterStore::open(malformed.path()).is_err());

    let v2 = copy_store(false)?;
    fs::remove_file(v2.path().join("zarr.json"))?;
    let root: Value = serde_json::from_str(include_str!("data/refpy.zarr/zarr.json"))?;
    fs::write(v2.path().join(".zgroup"), br#"{"zarr_format":2}"#)?;
    fs::write(
        v2.path().join(".zattrs"),
        serde_json::to_vec(&root["attributes"])?,
    )?;
    assert!(RasterStore::open(v2.path()).is_err());
    Ok(())
}

#[test]
fn corrupt_shards_remain_errors_for_chunk_and_window_reads() -> TestResult {
    let temporary = copy_store(true)?;
    let mut store = RasterStore::open(temporary.path())?;
    fs::write(temporary.path().join("0/c/0/0/0"), [0u8; 32])?;
    assert!(store.read_chunk(0, 0, 0).is_err());
    assert!(store.read_window(0, 0, 0, 2, 2).is_err());
    Ok(())
}

#[test]
fn enumeration_checks_the_coordinate_allocation_cap() -> TestResult {
    let temporary = copy_store(false)?;
    let path = temporary.path().join("0/zarr.json");
    let mut level: Value = serde_json::from_slice(&fs::read(&path)?)?;
    level["shape"] = serde_json::json!([1, 65_552, 65_536]);
    fs::write(&path, serde_json::to_vec(&level)?)?;
    let mut store = RasterStore::open(temporary.path())?;
    assert!(store.enumerate_chunks(0).is_err());
    Ok(())
}
