use std::fs;
use std::io::Cursor;
use std::path::Path;

use serde_json::Value;
use tempfile::TempDir;
use zarra::{render_tile, RasterStore, Tile, TileCoord};

const STORE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/refpy.zarr");
type TestResult = Result<(), Box<dyn std::error::Error>>;

struct DecodedPng {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

fn decode_png(bytes: &[u8]) -> Result<DecodedPng, Box<dyn std::error::Error>> {
    let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info()?;
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
    Ok(DecodedPng {
        width: info.width,
        height: info.height,
        pixels,
    })
}

fn rendered_png(store: &mut RasterStore, tile: TileCoord, bands: &[usize]) -> TestResult {
    let Tile::Png(bytes) = render_tile(store, tile, bands)? else {
        return Err("expected a PNG tile".into());
    };
    let decoded = decode_png(&bytes)?;
    assert_eq!((decoded.width, decoded.height), (16, 16));
    assert_eq!(decoded.pixels.len(), 16 * 16 * 4);
    Ok(())
}

fn pixel(png: &DecodedPng, row: u32, col: u32) -> &[u8] {
    let index = ((row * png.width + col) * 4) as usize;
    &png.pixels[index..index + 4]
}

fn copy_store(level: &str, with_shard: bool) -> Result<TempDir, Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    fs::create_dir(temporary.path().join(level))?;
    fs::copy(
        Path::new(STORE).join("zarr.json"),
        temporary.path().join("zarr.json"),
    )?;
    fs::copy(
        Path::new(STORE).join("0/zarr.json"),
        temporary.path().join(level).join("zarr.json"),
    )?;
    if with_shard {
        fs::create_dir_all(temporary.path().join(level).join("c/0/0"))?;
        fs::copy(
            Path::new(STORE).join("0/c/0/0/0"),
            temporary.path().join(level).join("c/0/0/0"),
        )?;
    }
    Ok(temporary)
}

#[test]
fn renders_golden_native_rg16_and_repeated_band_rgb8() -> TestResult {
    let mut store = RasterStore::open(Path::new(STORE))?;
    let native = TileCoord { z: 0, x: 0, y: 0 };

    let Tile::Png(bytes) = render_tile(&mut store, native, &[])? else {
        return Err("native reference tile must not be empty".into());
    };
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    let decoded = decode_png(&bytes)?;
    assert_eq!((decoded.width, decoded.height), (16, 16));
    assert_eq!(pixel(&decoded, 0, 0), [0, 0, 0, 255]);
    // Row 2, column 9 is 2009: round(2009 / 63063 * 65535) = 2088 = 0x0828.
    assert_eq!(pixel(&decoded, 2, 9), [8, 40, 0, 255]);

    let Tile::Png(repeated) = render_tile(&mut store, native, &[0, 0])? else {
        return Err("repeated-band reference tile must not be empty".into());
    };
    let decoded = decode_png(&repeated)?;
    assert_eq!(pixel(&decoded, 2, 9), [8, 8, 8, 255]);
    Ok(())
}

#[test]
fn overzoom_always_reads_maxzoom_through_zoom_22() -> TestResult {
    let mut store = RasterStore::open(Path::new(STORE))?;
    for z in [0, 1, 12, 13, 22] {
        rendered_png(&mut store, TileCoord { z, x: 0, y: 0 }, &[])?;
    }

    let edge = (1u32 << 22) - 1;
    let Tile::Png(bytes) = render_tile(
        &mut store,
        TileCoord {
            z: 22,
            x: edge,
            y: edge,
        },
        &[],
    )?
    else {
        return Err("zoom-22 edge tile must not be empty".into());
    };
    let decoded = decode_png(&bytes)?;
    // The deepest southeast window samples source value 15015 = 0x3cf4 after RG16 scaling.
    assert_eq!(pixel(&decoded, 0, 0), [60, 244, 0, 255]);
    Ok(())
}

#[test]
fn applies_level_offsets_and_distinguishes_sparse_and_invalid_tiles() -> TestResult {
    let offset = copy_store("3", true)?;
    let root_path = offset.path().join("zarr.json");
    let mut root: Value = serde_json::from_slice(&fs::read(&root_path)?)?;
    root["attributes"]["zarra"]["maxzoom"] = serde_json::json!(3);
    root["attributes"]["zarra"]["levels"] = serde_json::json!([
        { "z": 0, "xoff": 0, "yoff": 0 },
        { "z": 1, "xoff": 0, "yoff": 0 },
        { "z": 2, "xoff": 0, "yoff": 0 },
        { "z": 3, "xoff": 4, "yoff": 3 }
    ]);
    fs::write(&root_path, serde_json::to_vec_pretty(&root)?)?;

    let mut store = RasterStore::open(offset.path())?;
    rendered_png(&mut store, TileCoord { z: 3, x: 4, y: 3 }, &[])?;
    assert_eq!(
        render_tile(&mut store, TileCoord { z: 3, x: 0, y: 0 }, &[])?,
        Tile::Empty
    );

    let sparse = copy_store("0", false)?;
    let mut store = RasterStore::open(sparse.path())?;
    assert_eq!(
        render_tile(&mut store, TileCoord { z: 0, x: 0, y: 0 }, &[])?,
        Tile::Empty
    );

    let mut store = RasterStore::open(Path::new(STORE))?;
    assert!(render_tile(&mut store, TileCoord { z: 0, x: 1, y: 0 }, &[]).is_err());
    Ok(())
}

#[test]
fn rejects_invalid_xyz_and_band_selections() -> TestResult {
    let mut store = RasterStore::open(Path::new(STORE))?;
    for tile in [
        TileCoord { z: 23, x: 0, y: 0 },
        TileCoord { z: 0, x: 0, y: 1 },
        TileCoord { z: 1, x: 2, y: 0 },
        TileCoord { z: 1, x: 0, y: 2 },
        TileCoord {
            z: 22,
            x: 1 << 22,
            y: 0,
        },
    ] {
        assert!(
            render_tile(&mut store, tile, &[]).is_err(),
            "accepted {tile:?}"
        );
    }
    assert!(render_tile(&mut store, TileCoord { z: 0, x: 0, y: 0 }, &[1]).is_err());
    assert!(render_tile(&mut store, TileCoord { z: 0, x: 0, y: 0 }, &[0, 0, 0, 0]).is_err());
    Ok(())
}

#[test]
fn decode_failures_do_not_become_empty_tiles() -> TestResult {
    let corrupt = copy_store("0", true)?;
    fs::write(corrupt.path().join("0/c/0/0/0"), [0u8; 32])?;
    let mut store = RasterStore::open(corrupt.path())?;
    assert!(render_tile(&mut store, TileCoord { z: 0, x: 0, y: 0 }, &[]).is_err());
    Ok(())
}
