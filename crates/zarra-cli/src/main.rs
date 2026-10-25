//! Command-line entry point for Zarra raster stores.

#![forbid(unsafe_code)]

use clap::Parser;

/// Work with canonical multiscale Zarr v3 raster stores.
#[derive(Parser)]
#[command(name = "zarra", version, about)]
struct Cli {}

fn main() {
    let _cli = Cli::parse();
}
