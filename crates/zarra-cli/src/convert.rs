use std::path::PathBuf;

use clap::Args as ClapArgs;
use zarra::{ConvertOptions, Resampling};

/// Arguments for local GDAL conversion.
#[derive(Debug, ClapArgs)]
pub(crate) struct ConvertArgs {
    /// Local GeoTIFF/COG input.
    pub(crate) input: PathBuf,
    /// New Zarr output directory; an existing path is never overwritten.
    #[arg(short = 'o', long = "output")]
    pub(crate) output: PathBuf,
    /// Highest zoom to emit; defaults to the source-derived native zoom.
    #[arg(long)]
    pub(crate) maxzoom: Option<u8>,
    /// GDAL resampling: auto, nearest, bilinear, cubic, or average.
    #[arg(long, default_value = "auto")]
    pub(crate) resampling: Resampling,
    /// Inner-chunk ZSTD compression level (1-22).
    #[arg(long, default_value_t = 5)]
    pub(crate) zstd_level: u8,
}

pub(crate) fn run(arguments: ConvertArgs) -> zarra::Result<()> {
    let options = ConvertOptions {
        maxzoom: arguments.maxzoom,
        resampling: arguments.resampling,
        zstd_level: arguments.zstd_level,
    };
    let report = zarra::convert_with_options(&arguments.input, &arguments.output, options)?;
    println!("output: {}", report.output.display());
    println!("levels: {}", report.levels);
    println!("files: {}", report.files);
    println!("dtype: {}", report.manifest.dtype.as_str());
    println!("bands: {}", report.manifest.bands);
    println!("maxzoom: {}", report.manifest.maxzoom);
    Ok(())
}
