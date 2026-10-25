//! Command-line entry point for Zarra raster stores.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod convert;

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
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Convert(arguments) => convert::run(arguments),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
