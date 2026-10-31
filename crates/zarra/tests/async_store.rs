#![cfg(all(feature = "async", feature = "local"))]

use std::fs;
use std::path::Path;
use std::sync::Arc;

use futures::executor::block_on;
use zarra::{
    render_async_tile, render_tile, AsyncRasterStore, RasterStore, Tile, TileCoord, Window,
};
use zarrs::storage::store::AsyncMemoryStore;
use zarrs::storage::{AsyncReadableStorage, AsyncWritableStorageTraits, Bytes, StoreKey};

const STORE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/refpy.zarr");
type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Clone, Copy)]
enum Shard {
    Present,
    Missing,
    Corrupt,
}

async fn async_store(shard: Shard) -> Result<AsyncRasterStore, Box<dyn std::error::Error>> {
    let memory = Arc::new(AsyncMemoryStore::new());
    for relative in ["zarr.json", "0/zarr.json"] {
        memory
            .set(
                &StoreKey::new(relative)?,
                Bytes::from(fs::read(Path::new(STORE).join(relative))?),
            )
            .await?;
    }
    match shard {
        Shard::Present | Shard::Corrupt => {
            let bytes = if matches!(shard, Shard::Present) {
                fs::read(Path::new(STORE).join("0/c/0/0/0"))?
            } else {
                vec![0; 32]
            };
            memory
                .set(&StoreKey::new("0/c/0/0/0")?, Bytes::from(bytes))
                .await?;
        }
        Shard::Missing => {}
    }
    let storage: AsyncReadableStorage = memory;
    Ok(AsyncRasterStore::open(storage).await?)
}

fn expected(y: i64, x: i64) -> f64 {
    ((y * 1000 + x) & 0xffff) as f64
}

fn assert_same_window(actual: &Window, expected: &Window) {
    assert_eq!(actual.width, expected.width);
    assert_eq!(actual.height, expected.height);
    assert_eq!(actual.bands, expected.bands);
    assert_eq!(actual.values.len(), expected.values.len());
    for (index, (actual, expected)) in actual.values.iter().zip(&expected.values).enumerate() {
        if expected.is_nan() {
            assert!(actual.is_nan(), "index {index}");
        } else {
            assert_eq!(actual.to_bits(), expected.to_bits(), "index {index}");
        }
    }
}

#[test]
fn async_reads_exact_chunks_and_windows() -> TestResult {
    block_on(async {
        let mut store = async_store(Shard::Present).await?;
        assert_eq!(store.manifest().bands, 1);
        let meta = store.level_meta(0).await?;
        assert_eq!(
            (meta.bands, meta.width, meta.height, meta.inner),
            (1, 64, 64, 16)
        );
        assert!(store.level_meta(1).await.is_err());

        let chunk = store
            .read_chunk(0, 1, 1)
            .await?
            .ok_or("reference chunk is missing")?;
        assert_eq!((chunk.bands, chunk.width, chunk.height), (1, 16, 16));
        for row in 0..16i64 {
            for col in 0..16i64 {
                assert_eq!(
                    chunk.value(0, row as u32, col as u32),
                    Some(expected(16 + row, 16 + col))
                );
            }
        }
        assert!(store.read_chunk(0, 4, 4).await?.is_none());

        let cross_chunk = store.read_bands(0, 12, 14, 8, 6, &[0]).await?;
        for row in 0..6i64 {
            for col in 0..8i64 {
                let index = (row * 8 + col) as usize;
                assert_eq!(cross_chunk.values[index], expected(14 + row, 12 + col));
            }
        }

        let negative = store.read_bands(0, -3, -2, 7, 6, &[0]).await?;
        for row in 0..6i64 {
            for col in 0..7i64 {
                let x = col - 3;
                let y = row - 2;
                let value = negative.values[(row * 7 + col) as usize];
                if x < 0 || y < 0 {
                    assert!(value.is_nan(), "x={x}, y={y}");
                } else {
                    assert_eq!(value, expected(y, x));
                }
            }
        }

        let repeated = store.read_bands(0, 15, 15, 3, 3, &[0, 0]).await?;
        assert_eq!(repeated.bands, [0, 0]);
        for row in 0..3i64 {
            for col in 0..3i64 {
                let index = ((row * 3 + col) * 2) as usize;
                let value = expected(15 + row, 15 + col);
                assert_eq!(repeated.values[index], value);
                assert_eq!(repeated.values[index + 1], value);
            }
        }

        let mut native = RasterStore::open(Path::new(STORE))?;
        for (x, y, width, height, bands) in [
            (-3, -2, 7, 6, &[0][..]),
            (12, 14, 8, 6, &[0][..]),
            (15, 15, 3, 3, &[0, 0][..]),
        ] {
            let actual = store.read_bands(0, x, y, width, height, bands).await?;
            let expected = native.read_bands(0, x, y, width, height, bands)?;
            assert_same_window(&actual, &expected);
        }
        assert!(store.read_bands(0, 0, 0, 4097, 4096, &[0]).await.is_err());
        assert!(store.read_bands(0, 0, 0, 1, 1, &[1]).await.is_err());
        Ok(())
    })
}

#[test]
fn async_distinguishes_sparse_chunks_from_decode_errors() -> TestResult {
    block_on(async {
        let mut sparse = async_store(Shard::Missing).await?;
        assert!(sparse.read_chunk(0, 0, 0).await?.is_none());
        let window = sparse.read_bands(0, -1, -1, 3, 3, &[0]).await?;
        assert!(window.values.iter().all(|value| value.is_nan()));

        let mut corrupt = async_store(Shard::Corrupt).await?;
        assert!(corrupt.read_chunk(0, 0, 0).await.is_err());
        assert!(corrupt.read_bands(0, 0, 0, 2, 2, &[0]).await.is_err());
        Ok(())
    })
}

#[test]
fn async_and_native_render_identical_png_bytes_and_empty_tiles() -> TestResult {
    block_on(async {
        let mut native = RasterStore::open(Path::new(STORE))?;
        let mut asynchronous = async_store(Shard::Present).await?;
        for (tile, bands) in [
            (TileCoord { z: 0, x: 0, y: 0 }, &[][..]),
            (
                TileCoord {
                    z: 22,
                    x: (1 << 22) - 1,
                    y: (1 << 22) - 1,
                },
                &[][..],
            ),
            (TileCoord { z: 0, x: 0, y: 0 }, &[0, 0][..]),
        ] {
            let expected = render_tile(&mut native, tile, bands)?;
            let actual = render_async_tile(&mut asynchronous, tile, bands).await?;
            assert_eq!(actual, expected, "tile {tile:?}, bands {bands:?}");
            assert!(matches!(actual, Tile::Png(_)));
        }

        let temporary = tempfile::tempdir()?;
        fs::create_dir(temporary.path().join("0"))?;
        for relative in ["zarr.json", "0/zarr.json"] {
            fs::copy(
                Path::new(STORE).join(relative),
                temporary.path().join(relative),
            )?;
        }
        let mut native = RasterStore::open(temporary.path())?;
        let mut asynchronous = async_store(Shard::Missing).await?;
        let tile = TileCoord { z: 0, x: 0, y: 0 };
        assert_eq!(render_tile(&mut native, tile, &[])?, Tile::Empty);
        assert_eq!(
            render_async_tile(&mut asynchronous, tile, &[]).await?,
            Tile::Empty
        );
        Ok(())
    })
}
