//! CLI and server adapters for core read-only SQLite queries.

use std::io::{self, Write};
use std::time::Instant;

use clap::Args as ClapArgs;
use zarra::{Error, RasterStore, Result};

use crate::source::Source;

pub(crate) use zarra::sql::SqlRequest;

/// Arguments for read-only SQL over raster samples.
#[derive(Debug, ClapArgs)]
pub(crate) struct SqlArgs {
    #[command(flatten)]
    source: Source,
    /// One read-only SELECT/WITH statement over raster(x, y, band, value).
    #[arg(value_name = "SQL")]
    sql: Option<String>,
    /// XYZ level to read; defaults to the store's highest level.
    #[arg(long)]
    level: Option<u8>,
    /// Array-relative window origin X.
    #[arg(long, default_value_t = 0, allow_negative_numbers = true)]
    x: i64,
    /// Array-relative window origin Y.
    #[arg(long, default_value_t = 0, allow_negative_numbers = true)]
    y: i64,
    /// Window width; width and height must be supplied together.
    #[arg(long, requires = "height")]
    width: Option<u32>,
    /// Window height; omit both dimensions to read the whole level.
    #[arg(long, requires = "width")]
    height: Option<u32>,
}

pub(crate) fn run(arguments: SqlArgs) -> Result<()> {
    let mut store = arguments.source.open()?;
    let response = execute(
        &mut store,
        &SqlRequest {
            sql: arguments.sql.ok_or_else(|| {
                Error::InvalidInput("one read-only SQL statement is required".into())
            })?,
            level: arguments.level,
            x: arguments.x,
            y: arguments.y,
            width: arguments.width,
            height: arguments.height,
        },
    )?;
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    serde_json::to_writer(&mut stdout, &response)
        .map_err(|error| Error::Format(format!("cannot stream SQL JSON: {error}")))?;
    stdout.write_all(b"\n")?;
    Ok(())
}

pub(crate) fn execute(
    store: &mut RasterStore,
    request: &SqlRequest,
) -> Result<zarra::sql::SqlResponse> {
    let started = Instant::now();
    zarra::sql::execute_with_clock(store, request, move || {
        started.elapsed().as_secs_f64() * 1_000.0
    })
}
