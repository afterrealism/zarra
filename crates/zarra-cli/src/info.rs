use clap::Args as ClapArgs;
use zarra::{Channels, Error, Nodata, Result};

use crate::source::Source;

/// Arguments for raster store metadata output.
#[derive(Debug, ClapArgs)]
pub(crate) struct InfoArgs {
    #[command(flatten)]
    source: Source,
}

pub(crate) fn run(arguments: InfoArgs) -> Result<()> {
    let label = arguments.source.label()?;
    let mut store = arguments.source.open()?;
    let maxzoom = store.manifest().maxzoom;
    let manifest = store.manifest();
    println!("store: {label}");
    println!("profile_version: {}", manifest.profile_version);
    println!("crs: {}", manifest.crs);
    println!("dtype: {}", manifest.dtype.as_str());
    println!("bands: {}", manifest.bands);
    println!(
        "nodata: {}",
        match manifest.nodata {
            Some(Nodata::Number(value)) => value.to_string(),
            Some(Nodata::NaN) => "NaN".into(),
            None => "none".into(),
        }
    );
    println!("tile_size: {}", manifest.tile_size);
    println!("minzoom: {}", manifest.minzoom);
    println!("maxzoom: {}", manifest.maxzoom);
    println!(
        "encoding: {}",
        match manifest.tile_encoding.channels {
            Channels::Rg16 => "rg16",
            Channels::Rgba8 => "rgba8",
        }
    );
    println!(
        "encoded_range: [{}, {}]",
        manifest.tile_encoding.encoded_min, manifest.tile_encoding.encoded_max
    );
    for (band, range) in manifest.per_band_range.iter().enumerate() {
        println!("band {band} range: [{}, {}]", range[0], range[1]);
    }
    match manifest.extent {
        Some(extent) => println!(
            "extent: [{}, {}, {}, {}]",
            extent[0], extent[1], extent[2], extent[3]
        ),
        None => println!("extent: none"),
    }

    for z in 0..=maxzoom {
        let offset = store
            .manifest()
            .level(z)
            .ok_or_else(|| Error::Format(format!("manifest has no level {z}")))?;
        let meta = store.level_meta(z)?;
        println!(
            "level {z}: bands={} shape={}x{} inner={} dtype={} offset=({},{})",
            meta.bands,
            meta.height,
            meta.width,
            meta.inner,
            meta.dtype.as_str(),
            offset.xoff,
            offset.yoff
        );
    }
    Ok(())
}
