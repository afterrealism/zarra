use std::process::{Command, Output};

use serde_json::Value;

const STORE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/refpy.zarr");
type TestResult = Result<(), Box<dyn std::error::Error>>;

fn run(arguments: &[&str]) -> Result<Output, Box<dyn std::error::Error>> {
    Ok(Command::new(env!("CARGO_BIN_EXE_zarra"))
        .args(arguments)
        .output()?)
}

fn success_json(arguments: &[&str]) -> Result<Value, Box<dyn std::error::Error>> {
    let output = run(arguments)?;
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn assert_failure(arguments: &[&str]) -> TestResult {
    let output = run(arguments)?;
    assert!(
        !output.status.success(),
        "unexpected success: {arguments:?}"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("error:"),
        "missing readable error for {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[test]
fn info_prints_manifest_and_every_level() -> TestResult {
    let output = run(&["info", STORE])?;
    assert!(
        output.status.success(),
        "info failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)?;
    for expected in [
        "store:",
        "uint16",
        "bands: 1",
        "tile_size: 16",
        "minzoom: 0",
        "maxzoom: 0",
        "encoding: rg16",
        "extent:",
        "level 0:",
        "shape=64x64",
        "inner=16",
        "offset=(0,0)",
    ] {
        assert!(stdout.contains(expected), "missing {expected:?}: {stdout}");
    }
    Ok(())
}

#[test]
fn query_emits_exact_reference_values_and_repeated_bands() -> TestResult {
    let json = success_json(&[
        "query", STORE, "--level", "0", "--x", "12", "--y", "12", "--width", "8", "--height", "8",
    ])?;
    assert_eq!(json["level"], 0);
    assert_eq!(json["x"], 12);
    assert_eq!(json["y"], 12);
    assert_eq!(json["width"], 8);
    assert_eq!(json["height"], 8);
    assert_eq!(json["bands"], serde_json::json!([0]));
    let values = json["values"].as_array().ok_or("values must be an array")?;
    assert_eq!(values.len(), 64);
    for row in 0..8u64 {
        for col in 0..8u64 {
            let index = (row * 8 + col) as usize;
            assert_eq!(
                values[index].as_f64(),
                Some(((row + 12) * 1000 + col + 12) as f64),
                "value {index}"
            );
        }
    }

    let repeated = success_json(&[
        "query", STORE, "--level", "0", "--x", "2", "--y", "2", "--width", "1", "--height", "2",
        "--bands", "0,0",
    ])?;
    assert_eq!(repeated["bands"], serde_json::json!([0, 0]));
    assert_eq!(
        repeated["values"],
        serde_json::json!([2002.0, 2002.0, 3002.0, 3002.0])
    );
    Ok(())
}

#[test]
fn query_emits_nan_padding_as_json_strings() -> TestResult {
    let json = success_json(&[
        "query", STORE, "--level", "0", "--x", "-1", "--y", "-1", "--width", "2", "--height", "2",
    ])?;
    assert_eq!(
        json["values"],
        serde_json::json!(["NaN", "NaN", "NaN", 0.0])
    );
    Ok(())
}

#[test]
fn invalid_sources_dimensions_bands_levels_and_urls_fail() -> TestResult {
    for arguments in [
        vec!["info"],
        vec!["info", STORE, "--url", "https://example.com/store"],
        vec!["info", "--url", "http://example.com/store"],
        vec!["info", "--url", "ftp://example.com/store"],
        vec!["info", "--url", "http://user@127.0.0.1:9/store"],
        vec![
            "query", STORE, "--level", "0", "--x", "0", "--y", "0", "--width", "0", "--height", "1",
        ],
        vec![
            "query", STORE, "--level", "0", "--x", "0", "--y", "0", "--width", "1", "--height", "0",
        ],
        vec![
            "query", STORE, "--level", "0", "--x", "0", "--y", "0", "--width", "1", "--height",
            "1", "--bands", "0,,1",
        ],
        vec![
            "query", STORE, "--level", "0", "--x", "0", "--y", "0", "--width", "1", "--height",
            "1", "--bands", "1",
        ],
        vec![
            "query", STORE, "--level", "1", "--x", "0", "--y", "0", "--width", "1", "--height", "1",
        ],
    ] {
        assert_failure(&arguments)?;
    }
    Ok(())
}
