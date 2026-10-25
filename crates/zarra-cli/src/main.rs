//! Command-line entry point for Zarra raster stores.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod convert;
mod info;
mod query;
mod source;
mod tile;

/// Work with canonical multiscale Zarr v3 raster stores.
#[derive(Parser)]
#[command(name = "zarra", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Convert a local GeoTIFF/COG into a canonical multiscale Zarr store.
    Convert(convert::ConvertArgs),
    /// Print a store manifest and every available level.
    Info(info::InfoArgs),
    /// Read an array-relative pixel window as JSON.
    Query(query::QueryArgs),
    /// Render one XYZ tile as a new RGBA PNG file.
    Tile(tile::TileArgs),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Convert(arguments) => convert::run(arguments),
        Command::Info(arguments) => info::run(arguments),
        Command::Query(arguments) => query::run(arguments),
        Command::Tile(arguments) => tile::run(arguments),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
