use std::path::PathBuf;

use clap::Args as ClapArgs;

/// Arguments for local GDAL conversion.
#[derive(Debug, ClapArgs)]
pub(crate) struct ConvertArgs {
    /// Local GeoTIFF/COG input.
    pub(crate) input: PathBuf,
    /// New Zarr output directory; an existing path is never overwritten.
    #[arg(short = 'o', long = "output")]
    pub(crate) output: PathBuf,
}

pub(crate) fn run(arguments: ConvertArgs) -> zarra::Result<()> {
    let report = zarra::convert(&arguments.input, &arguments.output)?;
    println!("output: {}", report.output.display());
    println!("levels: {}", report.levels);
    println!("files: {}", report.files);
    println!("dtype: {}", report.manifest.dtype.as_str());
    println!("bands: {}", report.manifest.bands);
    println!("maxzoom: {}", report.manifest.maxzoom);
    Ok(())
}
