use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clap::Args as ClapArgs;
use zarra::{render_tile, Error, Result, Tile, TileCoord};

use crate::source::Source;

/// Arguments for rendering one XYZ PNG tile.
#[derive(Debug, ClapArgs)]
pub(crate) struct TileArgs {
    #[command(flatten)]
    source: Source,
    /// XYZ zoom level (0 through 22).
    #[arg(long)]
    z: u8,
    /// XYZ horizontal tile coordinate.
    #[arg(long)]
    x: u32,
    /// XYZ vertical tile coordinate.
    #[arg(long)]
    y: u32,
    /// Ordered comma-separated band indices; repeats are allowed.
    #[arg(long)]
    bands: Option<String>,
    /// New PNG output path; an existing path is never replaced.
    #[arg(short, long, value_name = "TILE.png")]
    output: PathBuf,
}

pub(crate) fn run(arguments: TileArgs) -> Result<()> {
    let bands = match &arguments.bands {
        Some(text) => zarra::parse_band_list(text)?,
        None => Vec::new(),
    };
    let mut store = arguments.source.open()?;
    let tile = TileCoord {
        z: arguments.z,
        x: arguments.x,
        y: arguments.y,
    };
    match render_tile(&mut store, tile, &bands)? {
        Tile::Empty => println!("empty"),
        Tile::Png(png) => {
            write_new(&arguments.output, &png)?;
            println!("{}", arguments.output.display());
        }
    }
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.flush()) {
        drop(file);
        return match fs::remove_file(path) {
            Ok(()) => Err(Error::Io(error)),
            Err(cleanup) => Err(Error::Io(io::Error::new(
                error.kind(),
                format!(
                    "{error}; also could not remove partial tile {}: {cleanup}",
                    path.display()
                ),
            ))),
        };
    }
    Ok(())
}
