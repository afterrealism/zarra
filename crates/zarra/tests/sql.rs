#![cfg(all(feature = "local", feature = "sql"))]

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::json;
use zarra::sql::{execute_with_clock, SqlRequest};
use zarra::RasterStore;

const STORE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/refpy.zarr");
type TestResult = Result<(), Box<dyn std::error::Error>>;

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
fn sql_keeps_json_shape_with_a_frozen_clock() -> TestResult {
    let mut store = RasterStore::open(Path::new(STORE))?;
    let response = execute_with_clock(
        &mut store,
        &SqlRequest {
            sql: "SELECT x, y, band, value FROM raster".into(),
            x: 2,
            y: 2,
            width: Some(1),
            height: Some(1),
            ..request("")
        },
        || 0.0,
    )?;
    assert_eq!(
        response.rows,
        vec![vec![json!(2), json!(2), json!(0), json!(2002.0)]]
    );
    assert_eq!(
        serde_json::to_value(&response)?,
        json!({
            "columns": ["x", "y", "band", "value"],
            "rows": [[2, 2, 0, 2002.0]],
            "truncated": false,
            "level": 0,
            "x": 2,
            "y": 2,
            "width": 1,
            "height": 1,
            "source_values": 1
        })
    );
    Ok(())
}

#[test]
fn sql_timeout_uses_the_injected_monotonic_clock() -> TestResult {
    let mut store = RasterStore::open(Path::new(STORE))?;
    let calls = Arc::new(AtomicU64::new(0));
    let clock = Arc::clone(&calls);
    let result = execute_with_clock(
        &mut store,
        &request(
            "WITH RECURSIVE slow(value) AS (\
               VALUES(0) UNION ALL SELECT value + 1 FROM slow WHERE value < 1000000\
             ) SELECT SUM(value) FROM slow",
        ),
        move || clock.fetch_add(10_001, Ordering::SeqCst) as f64,
    );
    let Err(error) = result else {
        return Err("the injected 10-second timeout did not interrupt SQL".into());
    };
    assert!(error.to_string().contains("interrupted"), "{error}");
    assert!(
        calls.load(Ordering::SeqCst) >= 2,
        "the clock was never polled"
    );
    Ok(())
}
