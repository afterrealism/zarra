//! Read-only SQLite queries over a bounded raster window.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use clap::Args as ClapArgs;
use rusqlite::limits::Limit;
use rusqlite::types::ValueRef;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};
use zarra::{Error, Nodata, RasterStore, Result};

use crate::source::Source;

const MAX_SQL_VALUES: u64 = 1_048_576;
const MAX_RESULT_ROWS: usize = 1_000;
const MAX_RESULT_BYTES: usize = 8 * 1024 * 1024;
const MAX_SQL_TEXT: i32 = 64 * 1024;
const MAX_SQLITE_VALUE: i32 = 8 * 1024 * 1024;
const QUERY_TIMEOUT: Duration = Duration::from_secs(10);

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

/// JSON body accepted by the viewer's SQL endpoint.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SqlRequest {
    pub(crate) sql: String,
    pub(crate) level: Option<u8>,
    #[serde(default)]
    pub(crate) x: i64,
    #[serde(default)]
    pub(crate) y: i64,
    pub(crate) width: Option<u32>,
    pub(crate) height: Option<u32>,
}

/// Bounded JSON rows and the exact source window used to produce them.
#[derive(Debug, Serialize)]
pub(crate) struct SqlResponse {
    columns: Vec<String>,
    rows: Vec<Vec<Value>>,
    truncated: bool,
    level: u8,
    x: i64,
    y: i64,
    width: u32,
    height: u32,
    source_values: u64,
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

pub(crate) fn execute(store: &mut RasterStore, request: &SqlRequest) -> Result<SqlResponse> {
    if request.sql.trim().is_empty() {
        return Err(Error::InvalidInput("SQL must not be empty".into()));
    }
    let level = request.level.unwrap_or(store.manifest().maxzoom);
    let meta = store.level_meta(level)?;
    let (width, height) = match (request.width, request.height) {
        (Some(width), Some(height)) => (width, height),
        (None, None) => (
            u32::try_from(meta.width).map_err(|_| {
                Error::InvalidInput("full level width exceeds the SQL window limit".into())
            })?,
            u32::try_from(meta.height).map_err(|_| {
                Error::InvalidInput("full level height exceeds the SQL window limit".into())
            })?,
        ),
        _ => {
            return Err(Error::InvalidInput(
                "width and height must be supplied together".into(),
            ));
        }
    };
    if width == 0 || height == 0 {
        return Err(Error::InvalidInput(
            "SQL window dimensions must be positive".into(),
        ));
    }
    let source_values = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|count| {
            u64::try_from(meta.bands)
                .ok()
                .and_then(|bands| count.checked_mul(bands))
        })
        .ok_or_else(|| Error::InvalidInput("SQL source value count overflows u64".into()))?;
    if source_values > MAX_SQL_VALUES {
        return Err(Error::InvalidInput(format!(
            "SQL window contains {source_values} band values; reduce it to {MAX_SQL_VALUES} or fewer"
        )));
    }
    request
        .x
        .checked_add(i64::from(width - 1))
        .ok_or_else(|| Error::InvalidInput("SQL window X coordinates overflow i64".into()))?;
    request
        .y
        .checked_add(i64::from(height - 1))
        .ok_or_else(|| Error::InvalidInput("SQL window Y coordinates overflow i64".into()))?;

    let window = store.read_window(level, request.x, request.y, width, height)?;
    let nodata = store.manifest().nodata;
    let mut connection = Connection::open_in_memory().map_err(staging_error)?;
    connection
        .set_limit(Limit::SQLITE_LIMIT_SQL_LENGTH, MAX_SQL_TEXT)
        .map_err(staging_error)?;
    connection
        .set_limit(Limit::SQLITE_LIMIT_LENGTH, MAX_SQLITE_VALUE)
        .map_err(staging_error)?;
    connection
        .execute_batch(
            "CREATE TABLE raster (\
               x INTEGER NOT NULL, \
               y INTEGER NOT NULL, \
               band INTEGER NOT NULL, \
               value REAL\
             )",
        )
        .map_err(staging_error)?;

    let transaction = connection.transaction().map_err(staging_error)?;
    {
        let mut insert = transaction
            .prepare("INSERT INTO raster (x, y, band, value) VALUES (?1, ?2, ?3, ?4)")
            .map_err(staging_error)?;
        let bands = window.bands.len();
        let row_width = usize::try_from(width)
            .map_err(|_| Error::InvalidInput("SQL window width does not fit usize".into()))?;
        for (pixel, samples) in window.values.chunks_exact(bands).enumerate() {
            let column = pixel % row_width;
            let row = pixel / row_width;
            let x = request
                .x
                .checked_add(i64::try_from(column).map_err(|_| {
                    Error::InvalidInput("SQL window column does not fit i64".into())
                })?)
                .ok_or_else(|| Error::InvalidInput("SQL pixel X overflows i64".into()))?;
            let y =
                request
                    .y
                    .checked_add(i64::try_from(row).map_err(|_| {
                        Error::InvalidInput("SQL window row does not fit i64".into())
                    })?)
                    .ok_or_else(|| Error::InvalidInput("SQL pixel Y overflows i64".into()))?;
            for (band, value) in samples.iter().copied().enumerate() {
                let band = i64::try_from(band)
                    .map_err(|_| Error::InvalidInput("SQL band does not fit i64".into()))?;
                let value = if is_missing(value, nodata) {
                    None
                } else {
                    Some(value)
                };
                insert
                    .execute(params![x, y, band, value])
                    .map_err(staging_error)?;
            }
        }
    }
    transaction.commit().map_err(staging_error)?;

    // SQLite blocks changes in query_only mode; Statement::readonly provides a
    // second statement-level check, and rusqlite rejects multiple statements.
    // Sources: https://www.sqlite.org/pragma.html#pragma_query_only
    // https://docs.rs/rusqlite/0.41.0/rusqlite/struct.Statement.html#method.readonly
    connection
        .execute_batch("PRAGMA query_only = ON")
        .map_err(staging_error)?;
    let started = Instant::now();
    connection
        .progress_handler(1_000, Some(move || started.elapsed() > QUERY_TIMEOUT))
        .map_err(staging_error)?;
    let mut statement = connection.prepare(&request.sql).map_err(query_error)?;
    if !statement.readonly() {
        return Err(Error::InvalidInput(
            "SQL must be one read-only SELECT or WITH statement".into(),
        ));
    }
    let columns = statement
        .column_names()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if columns.is_empty() {
        return Err(Error::InvalidInput("SQL must return columns".into()));
    }
    let column_count = columns.len();
    let mut response = SqlResponse {
        columns,
        rows: Vec::new(),
        truncated: false,
        level,
        x: request.x,
        y: request.y,
        width,
        height,
        source_values,
    };
    let base_bytes = response_budget(&response)?;
    if base_bytes > MAX_RESULT_BYTES {
        return Err(Error::InvalidInput(
            "SQL result columns exceed the 8 MiB response limit".into(),
        ));
    }
    let mut rows_bytes = 0usize;
    let mut query = statement.query([]).map_err(query_error)?;
    'rows: loop {
        if response.rows.len() == MAX_RESULT_ROWS {
            response.truncated = query.next().map_err(query_error)?.is_some();
            break;
        }
        let Some(row) = query.next().map_err(query_error)? else {
            break;
        };
        let mut output = Vec::with_capacity(column_count);
        let mut row_bytes = 2usize; // JSON array brackets.
        let row_separator = usize::from(!response.rows.is_empty());
        for index in 0..column_count {
            let value = json_value(row.get_ref(index).map_err(query_error)?);
            let encoded = serde_json::to_vec(&value).map_err(json_error)?;
            let additional = encoded.len() + usize::from(!output.is_empty());
            if base_bytes
                .saturating_add(rows_bytes)
                .saturating_add(row_separator)
                .saturating_add(row_bytes)
                .saturating_add(additional)
                > MAX_RESULT_BYTES
            {
                response.truncated = true;
                break 'rows;
            }
            row_bytes += additional;
            output.push(value);
        }
        response.rows.push(output);
        rows_bytes += row_separator + row_bytes;
    }

    Ok(response)
}

fn response_budget(response: &SqlResponse) -> Result<usize> {
    let probe = SqlResponse {
        columns: response.columns.clone(),
        rows: Vec::new(),
        truncated: false,
        level: response.level,
        x: response.x,
        y: response.y,
        width: response.width,
        height: response.height,
        source_values: response.source_values,
    };
    serde_json::to_vec(&probe)
        .map_err(json_error)?
        .len()
        // The CLI writes one trailing newline; reserve it for both output paths.
        .checked_add(1)
        .ok_or_else(|| Error::Format("SQL response size overflows usize".into()))
}

fn is_missing(value: f64, nodata: Option<Nodata>) -> bool {
    value.is_nan()
        || matches!(
            nodata,
            Some(Nodata::Number(declared)) if (value - declared).abs() <= 1e-9
        )
}

fn staging_error(error: rusqlite::Error) -> Error {
    Error::Zarr(format!("cannot stage raster for SQL: {error}"))
}

fn query_error(error: rusqlite::Error) -> Error {
    Error::InvalidInput(format!("SQL query failed: {error}"))
}

fn json_error(error: serde_json::Error) -> Error {
    Error::Format(format!("cannot encode SQL response: {error}"))
}

fn json_value(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(value) => Value::from(value),
        ValueRef::Real(value) => {
            if let Some(number) = Number::from_f64(value) {
                Value::Number(number)
            } else {
                let text = if value.is_nan() {
                    "NaN"
                } else if value.is_sign_positive() {
                    "Infinity"
                } else {
                    "-Infinity"
                };
                Value::String(text.into())
            }
        }
        ValueRef::Text(bytes) => Value::String(String::from_utf8_lossy(bytes).into_owned()),
        ValueRef::Blob(bytes) => {
            const HEX: &[u8; 16] = b"0123456789abcdef";
            let mut text = String::with_capacity(2 + bytes.len() * 2);
            text.push_str("0x");
            for &byte in bytes {
                text.push(HEX[usize::from(byte >> 4)] as char);
                text.push(HEX[usize::from(byte & 0x0f)] as char);
            }
            Value::String(text)
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use zarra::RasterStore;

    use super::{execute, response_budget, SqlRequest, SqlResponse, MAX_RESULT_ROWS};

    type TestResult = Result<(), Box<dyn std::error::Error>>;
    const STORE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/refpy.zarr");

    fn request(sql: &str) -> SqlRequest {
        SqlRequest {
            sql: sql.into(),
            level: None,
            x: 0,
            y: 0,
            width: None,
            height: None,
        }
    }

    #[test]
    fn sql_reads_actual_pixels_aggregates_and_nan_as_null() -> TestResult {
        let mut store = RasterStore::open(std::path::Path::new(STORE))?;
        let point = execute(
            &mut store,
            &request("SELECT x, y, band, value FROM raster WHERE x = 2 AND y = 2"),
        )?;
        assert_eq!(point.columns, ["x", "y", "band", "value"]);
        assert_eq!(
            point.rows,
            vec![vec![json!(2), json!(2), json!(0), json!(2002.0)]]
        );

        let count = execute(
            &mut store,
            &request("SELECT COUNT(*) AS samples, COUNT(value) AS valid FROM raster"),
        )?;
        assert_eq!(count.rows, vec![vec![json!(4096), json!(4096)]]);

        let padded = execute(
            &mut store,
            &SqlRequest {
                sql: "SELECT COUNT(*) AS samples, COUNT(value) AS valid FROM raster".into(),
                x: -1,
                y: -1,
                width: Some(2),
                height: Some(2),
                ..request("")
            },
        )?;
        assert_eq!(padded.rows, vec![vec![json!(4), json!(1)]]);
        assert_eq!(padded.source_values, 4);
        Ok(())
    }

    #[test]
    fn sql_treats_declared_numeric_nodata_as_null() -> TestResult {
        let temporary = tempfile::tempdir()?;
        let root = temporary.path().join("store.zarr");
        std::fs::create_dir_all(root.join("0/c/0/0"))?;
        for relative in ["zarr.json", "0/zarr.json", "0/c/0/0/0"] {
            std::fs::copy(
                std::path::Path::new(STORE).join(relative),
                root.join(relative),
            )?;
        }
        let manifest_path = root.join("zarr.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path)?)?;
        manifest["attributes"]["zarra"]["nodata"] = json!(0);
        std::fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;

        let mut store = RasterStore::open(&root)?;
        let result = execute(
            &mut store,
            &request(
                "SELECT COUNT(*) AS total, COUNT(value) AS valid, \
                 COUNT(*) - COUNT(value) AS missing FROM raster",
            ),
        )?;
        assert_eq!(result.rows, vec![vec![json!(4096), json!(4095), json!(1)]]);
        let point = execute(
            &mut store,
            &request("SELECT value FROM raster WHERE x = 0 AND y = 0"),
        )?;
        assert_eq!(point.rows, vec![vec![serde_json::Value::Null]]);
        Ok(())
    }

    #[test]
    fn response_budget_reserves_the_longer_nontruncated_form() -> TestResult {
        let response = SqlResponse {
            columns: vec!["value".into()],
            rows: Vec::new(),
            truncated: false,
            level: 0,
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            source_values: 1,
        };
        let actual = serde_json::to_vec(&response)?.len() + 1;
        assert_eq!(actual, response_budget(&response)?);
        let truncated = SqlResponse {
            truncated: true,
            ..response
        };
        let actual = serde_json::to_vec(&truncated)?.len() + 1;
        assert!(actual <= response_budget(&truncated)?);
        Ok(())
    }

    #[test]
    fn sql_is_single_statement_read_only_and_bounded() -> TestResult {
        let mut store = RasterStore::open(std::path::Path::new(STORE))?;
        for sql in [
            "DELETE FROM raster",
            "SELECT 1; SELECT 2",
            "PRAGMA query_only = OFF",
            "",
        ] {
            assert!(execute(&mut store, &request(sql)).is_err(), "{sql}");
        }
        let limited = execute(&mut store, &request("SELECT * FROM raster"))?;
        assert_eq!(limited.rows.len(), MAX_RESULT_ROWS);
        assert!(limited.truncated);
        Ok(())
    }
}
