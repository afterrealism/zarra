use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, RANGE};
use reqwest::{Method, StatusCode};

#[path = "../src/range.rs"]
mod range;
use range::{percent_decode, range_header, resolve, Range};

const STORE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/refpy.zarr");
const TILE_CACHE: &str = "public, max-age=3600, s-maxage=86400";
type TestResult = Result<(), Box<dyn std::error::Error>>;

struct Server {
    child: Child,
    origin: String,
}

impl Server {
    fn spawn(store: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let mut child = Command::new(env!("CARGO_BIN_EXE_zarra"))
            .arg("serve")
            .arg(store)
            .args(["--port", "0"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdout = child.stdout.take().ok_or("server stdout was not piped")?;
        let (sender, receiver) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let result = (|| -> std::io::Result<String> {
                let mut line = String::new();
                BufReader::new(stdout).read_line(&mut line)?;
                Ok(line)
            })();
            let _ = sender.send(result.map_err(|error| error.to_string()));
        });
        let line = match receiver.recv_timeout(Duration::from_secs(15)) {
            Ok(Ok(line)) => line,
            Ok(Err(error)) => {
                terminate(&mut child);
                return Err(error.into());
            }
            Err(error) => {
                terminate(&mut child);
                return Err(error.into());
            }
        };
        let port = line
            .trim()
            .strip_prefix("http://127.0.0.1:")
            .ok_or_else(|| format!("server printed an invalid URL: {line:?}"))?
            .parse::<u16>()?;
        assert_ne!(port, 0, "the actual bound port must be printed");
        Ok(Self {
            child,
            origin: format!("http://127.0.0.1:{port}"),
        })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        terminate(&mut self.child);
    }
}

fn terminate(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

struct ResponseData {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

fn client() -> Result<Client, Box<dyn std::error::Error>> {
    Ok(Client::builder().timeout(Duration::from_secs(15)).build()?)
}

fn request(
    client: &Client,
    method: Method,
    origin: &str,
    path: &str,
    range: Option<&str>,
) -> Result<ResponseData, Box<dyn std::error::Error>> {
    let mut builder = client.request(method, format!("{origin}{path}"));
    if let Some(value) = range {
        builder = builder.header(RANGE, value);
    }
    let response = builder.send()?;
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.bytes()?.to_vec();
    assert_eq!(header(&headers, "access-control-allow-origin"), Some("*"));
    assert_eq!(header(&headers, "x-content-type-options"), Some("nosniff"));
    assert_eq!(header(&headers, "referrer-policy"), Some("no-referrer"));
    Ok(ResponseData {
        status,
        headers,
        body,
    })
}

fn get(
    client: &Client,
    origin: &str,
    path: &str,
) -> Result<ResponseData, Box<dyn std::error::Error>> {
    request(client, Method::GET, origin, path, None)
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn content_length(response: &ResponseData) -> Result<u64, Box<dyn std::error::Error>> {
    Ok(header(&response.headers, "content-length")
        .ok_or("missing Content-Length")?
        .parse()?)
}

fn assert_png_16(bytes: &[u8]) -> TestResult {
    assert!(bytes.len() >= 33, "PNG is too short");
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(&bytes[12..16], b"IHDR");
    assert_eq!(u32::from_be_bytes(bytes[16..20].try_into()?), 16);
    assert_eq!(u32::from_be_bytes(bytes[20..24].try_into()?), 16);
    assert_eq!(bytes[24], 8);
    assert_eq!(bytes[25], 6);
    Ok(())
}

fn copy_metadata_store(parent: &Path, name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let root = parent.join(name);
    fs::create_dir_all(root.join("0"))?;
    fs::copy(Path::new(STORE).join("zarr.json"), root.join("zarr.json"))?;
    fs::copy(
        Path::new(STORE).join("0/zarr.json"),
        root.join("0/zarr.json"),
    )?;
    Ok(root)
}

#[test]
fn range_parser_handles_spans_suffixes_empty_files_and_malformed_values() {
    assert_eq!(range_header(None, 100), Range::None);
    assert_eq!(range_header(Some("bytes=3-9"), 100), Range::Span(3, 9));
    assert_eq!(range_header(Some("bytes=25-"), 100), Range::Span(25, 99));
    assert_eq!(range_header(Some("bytes=-17"), 100), Range::Span(83, 99));
    assert_eq!(range_header(Some("bytes=-200"), 100), Range::Span(0, 99));
    assert_eq!(range_header(Some("Bytes=90-200"), 100), Range::Span(90, 99));
    assert_eq!(range_header(Some("bytes=100-"), 100), Range::Unsatisfiable);
    assert_eq!(range_header(Some("bytes=9-2"), 100), Range::Unsatisfiable);
    assert_eq!(range_header(Some("bytes=-0"), 100), Range::Unsatisfiable);
    assert_eq!(range_header(Some("bytes=0-"), 0), Range::Unsatisfiable);
    assert_eq!(range_header(Some("bytes=-1"), 0), Range::Unsatisfiable);
    for malformed in ["items=0-1", "bytes=word", "bytes=0-1,4-5", "bytes="] {
        assert_eq!(
            range_header(Some(malformed), 100),
            Range::None,
            "{malformed}"
        );
        assert_eq!(range_header(Some(malformed), 0), Range::None, "{malformed}");
    }
}

#[test]
fn percent_decoding_and_resolution_reject_escapes_and_non_files() -> TestResult {
    assert_eq!(percent_decode("0%2Fc%2f0%2F0"), Ok("0/c/0/0".into()));
    assert_eq!(percent_decode("%252e%252e"), Ok("%2e%2e".into()));
    assert_eq!(percent_decode("literal%name"), Ok("literal%name".into()));
    assert_eq!(percent_decode("%ff"), Err(400));

    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("store");
    fs::create_dir_all(root.join("nested"))?;
    fs::write(root.join("nested/object.bin"), b"nested")?;
    let root = root.canonicalize()?;
    assert_eq!(
        resolve(&root, "nested%2Fobject.bin"),
        Ok(root.join("nested/object.bin"))
    );
    for forbidden in [
        "../outside",
        "%2e%2e%2Foutside",
        ".%2Fzarr.json",
        "%2Fetc/passwd",
        "nested%5Cobject.bin",
        "nested%00object.bin",
    ] {
        assert_eq!(resolve(&root, forbidden), Err(403), "{forbidden}");
    }
    assert_eq!(resolve(&root, "nested"), Err(404));
    assert_eq!(resolve(&root, "missing"), Err(404));
    assert_eq!(resolve(&root, ""), Err(404));
    Ok(())
}

#[test]
fn api_and_explicit_static_assets_are_served_with_headers() -> TestResult {
    let server = Server::spawn(Path::new(STORE))?;
    let client = client()?;
    let api = get(&client, &server.origin, "/api/store")?;
    assert_eq!(api.status, StatusCode::OK);
    assert_eq!(header(&api.headers, "cache-control"), Some("no-store"));
    assert!(header(&api.headers, "content-type")
        .is_some_and(|value| value.starts_with("application/json")));
    let manifest: serde_json::Value = serde_json::from_slice(&api.body)?;
    assert_eq!(manifest["tiles"], "/raster/{z}/{x}/{y}.png");
    assert_eq!(manifest["tile_size"], 16);
    assert_eq!(manifest["maxzoom"], 0);

    let assets: &[(&str, &str, &[u8])] = &[
        ("/", "text/html", include_bytes!("../assets/index.html")),
        (
            "/index.html",
            "text/html",
            include_bytes!("../assets/index.html"),
        ),
        (
            "/app.js",
            "text/javascript",
            include_bytes!("../assets/app.js"),
        ),
        (
            "/geo.js",
            "text/javascript",
            include_bytes!("../assets/geo.js"),
        ),
        (
            "/styles.css",
            "text/css",
            include_bytes!("../assets/styles.css"),
        ),
        (
            "/favicon.svg",
            "image/svg+xml",
            include_bytes!("../assets/favicon.svg"),
        ),
        (
            "/vendor/maplibre-gl.js",
            "text/javascript",
            include_bytes!("../assets/vendor/maplibre-gl.js"),
        ),
        (
            "/vendor/maplibre-gl.css",
            "text/css",
            include_bytes!("../assets/vendor/maplibre-gl.css"),
        ),
        (
            "/vendor/maplibre-LICENSE.txt",
            "text/plain",
            include_bytes!("../assets/vendor/maplibre-LICENSE.txt"),
        ),
    ];
    for (path, mime, expected) in assets {
        let response = get(&client, &server.origin, path)?;
        assert_eq!(response.status, StatusCode::OK, "{path}");
        assert_eq!(response.body, *expected, "{path}");
        assert_eq!(content_length(&response)?, expected.len() as u64, "{path}");
        assert!(
            header(&response.headers, "content-type").is_some_and(|value| value.starts_with(mime)),
            "{path}"
        );
        assert_eq!(
            header(&response.headers, "cache-control"),
            Some("no-cache"),
            "{path}"
        );
    }
    assert_eq!(
        get(&client, &server.origin, "/geo.js.map")?.status,
        StatusCode::NOT_FOUND
    );
    Ok(())
}

#[test]
fn tiles_support_native_deep_overzoom_bands_and_head() -> TestResult {
    let server = Server::spawn(Path::new(STORE))?;
    let client = client()?;
    for path in [
        "/raster/0/0/0.png",
        "/raster/13/0/0.png",
        "/raster/22/0/0.png",
        "/raster/0/0/0.png?bands=0,0",
        "/raster/0/0/0.png?bands=b0",
        "/raster/0/0/0.png?bands=B0,B0",
    ] {
        let response = get(&client, &server.origin, path)?;
        assert_eq!(response.status, StatusCode::OK, "{path}");
        assert_eq!(
            header(&response.headers, "cache-control"),
            Some(TILE_CACHE),
            "{path}"
        );
        assert_eq!(
            header(&response.headers, "content-type"),
            Some("image/png"),
            "{path}"
        );
        assert_eq!(
            content_length(&response)?,
            response.body.len() as u64,
            "{path}"
        );
        assert_png_16(&response.body)?;
    }
    let rendered = get(&client, &server.origin, "/raster/0/0/0.png")?;
    let head = request(
        &client,
        Method::HEAD,
        &server.origin,
        "/raster/0/0/0.png",
        None,
    )?;
    assert_eq!(head.status, StatusCode::OK);
    assert!(head.body.is_empty());
    assert_eq!(content_length(&head)?, rendered.body.len() as u64);
    assert_eq!(header(&head.headers, "cache-control"), Some(TILE_CACHE));
    Ok(())
}

#[test]
fn invalid_tiles_are_400_sparse_is_204_and_decode_errors_are_502() -> TestResult {
    let server = Server::spawn(Path::new(STORE))?;
    let client = client()?;
    for path in [
        "/raster/0/1/0.png",
        "/raster/0/0/1.png",
        "/raster/23/0/0.png",
        "/raster/-1/0/0.png",
        "/raster/0/0/0.png?bands=",
        "/raster/0/0/0.png?bands=0,,0",
        "/raster/0/0/0.png?bands=b",
        "/raster/0/0/0.png?bands=1",
        "/raster/0/0/0.png?bands=0,0,0,0",
        "/raster/0/0/0.png?other=0",
    ] {
        assert_eq!(
            get(&client, &server.origin, path)?.status,
            StatusCode::BAD_REQUEST,
            "{path}"
        );
    }

    let temporary = tempfile::tempdir()?;
    let sparse = copy_metadata_store(temporary.path(), "sparse.zarr")?;
    let sparse_server = Server::spawn(&sparse)?;
    for method in [Method::GET, Method::HEAD] {
        let response = request(
            &client,
            method,
            &sparse_server.origin,
            "/raster/0/0/0.png",
            None,
        )?;
        assert_eq!(response.status, StatusCode::NO_CONTENT);
        assert!(response.body.is_empty());
        assert_eq!(header(&response.headers, "cache-control"), Some(TILE_CACHE));
        assert!(header(&response.headers, "content-length").is_none());
    }

    let corrupt = copy_metadata_store(temporary.path(), "corrupt.zarr")?;
    fs::create_dir_all(corrupt.join("0/c/0/0"))?;
    fs::write(corrupt.join("0/c/0/0/0"), b"not a shard")?;
    let corrupt_server = Server::spawn(&corrupt)?;
    assert_eq!(
        get(&client, &corrupt_server.origin, "/raster/0/0/0.png")?.status,
        StatusCode::BAD_GATEWAY
    );
    Ok(())
}

#[test]
fn nested_objects_have_exact_full_head_and_single_range_responses() -> TestResult {
    let server = Server::spawn(Path::new(STORE))?;
    let client = client()?;
    let path = "/store/0/c/0/0/0";
    let expected = fs::read(Path::new(STORE).join("0/c/0/0/0"))?;
    let len = expected.len() as u64;

    let full = get(&client, &server.origin, path)?;
    assert_eq!(full.status, StatusCode::OK);
    assert_eq!(full.body, expected);
    assert_eq!(content_length(&full)?, len);
    assert_eq!(header(&full.headers, "accept-ranges"), Some("bytes"));
    assert_eq!(
        header(&full.headers, "content-type"),
        Some("application/octet-stream")
    );

    let head = request(&client, Method::HEAD, &server.origin, path, None)?;
    assert_eq!(head.status, StatusCode::OK);
    assert!(head.body.is_empty());
    assert_eq!(content_length(&head)?, len);
    assert_eq!(header(&head.headers, "accept-ranges"), Some("bytes"));

    for (range, start, end) in [
        ("bytes=-17", len - 17, len - 1),
        ("bytes=3-19", 3, 19),
        ("bytes=25-", 25, len - 1),
    ] {
        let response = request(&client, Method::GET, &server.origin, path, Some(range))?;
        assert_eq!(response.status, StatusCode::PARTIAL_CONTENT, "{range}");
        assert_eq!(
            response.body,
            expected[start as usize..=end as usize],
            "{range}"
        );
        assert_eq!(content_length(&response)?, end - start + 1, "{range}");
        assert_eq!(
            header(&response.headers, "content-range"),
            Some(format!("bytes {start}-{end}/{len}").as_str()),
            "{range}"
        );
        assert_eq!(
            header(&response.headers, "accept-ranges"),
            Some("bytes"),
            "{range}"
        );
    }
    let head_range = request(
        &client,
        Method::HEAD,
        &server.origin,
        path,
        Some("bytes=3-19"),
    )?;
    assert_eq!(head_range.status, StatusCode::PARTIAL_CONTENT);
    assert!(head_range.body.is_empty());
    assert_eq!(content_length(&head_range)?, 17);

    for malformed in ["bytes=word", "bytes=0-1,4-5"] {
        let response = request(&client, Method::GET, &server.origin, path, Some(malformed))?;
        assert_eq!(response.status, StatusCode::OK, "{malformed}");
        assert_eq!(response.body, expected, "{malformed}");
    }
    let unsatisfiable = request(
        &client,
        Method::GET,
        &server.origin,
        path,
        Some(&format!("bytes={len}-")),
    )?;
    assert_eq!(unsatisfiable.status, StatusCode::RANGE_NOT_SATISFIABLE);
    assert!(unsatisfiable.body.is_empty());
    assert_eq!(content_length(&unsatisfiable)?, 0);
    assert_eq!(
        header(&unsatisfiable.headers, "content-range"),
        Some(format!("bytes */{len}").as_str())
    );
    assert_eq!(
        header(&unsatisfiable.headers, "accept-ranges"),
        Some("bytes")
    );
    Ok(())
}

#[test]
fn object_routes_cover_empty_files_traversal_symlinks_and_missing_files() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let store = copy_metadata_store(temporary.path(), "objects.zarr")?;
    fs::write(store.join("empty.bin"), [])?;
    let outside = temporary.path().join("outside.txt");
    fs::write(&outside, b"outside")?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, store.join("escape"))?;
    let server = Server::spawn(&store)?;
    let client = client()?;

    let empty = get(&client, &server.origin, "/store/empty.bin")?;
    assert_eq!(empty.status, StatusCode::OK);
    assert!(empty.body.is_empty());
    assert_eq!(content_length(&empty)?, 0);
    let empty_range = request(
        &client,
        Method::GET,
        &server.origin,
        "/store/empty.bin",
        Some("bytes=0-"),
    )?;
    assert_eq!(empty_range.status, StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(
        header(&empty_range.headers, "content-range"),
        Some("bytes */0")
    );

    for path in [
        "/store/%2e%2e%2Foutside.txt",
        "/store/.%2Fzarr.json",
        "/store/%2Fetc/passwd",
        "/store/zarr%00.json",
        "/store/zarr%5C.json",
    ] {
        assert_eq!(
            get(&client, &server.origin, path)?.status,
            StatusCode::FORBIDDEN,
            "{path}"
        );
    }
    assert_eq!(
        get(&client, &server.origin, "/store/missing")?.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&client, &server.origin, "/store/0")?.status,
        StatusCode::NOT_FOUND
    );
    #[cfg(unix)]
    assert_eq!(
        get(&client, &server.origin, "/store/escape")?.status,
        StatusCode::FORBIDDEN
    );
    Ok(())
}

#[test]
fn options_cors_mutations_and_unknown_routes_follow_the_contract() -> TestResult {
    let server = Server::spawn(Path::new(STORE))?;
    let client = client()?;
    for path in [
        "/api/store",
        "/store/zarr.json",
        "/raster/0/0/0.png",
        "/geo.js",
    ] {
        let response = request(&client, Method::OPTIONS, &server.origin, path, None)?;
        assert_eq!(response.status, StatusCode::NO_CONTENT, "{path}");
        assert!(response.body.is_empty(), "{path}");
        assert!(
            header(&response.headers, "content-length").is_none(),
            "{path}"
        );
        let methods = header(&response.headers, "access-control-allow-methods")
            .ok_or("missing allowed methods")?;
        for method in ["GET", "HEAD"] {
            assert!(methods.contains(method), "{methods}");
        }
        assert_eq!(
            header(&response.headers, "access-control-allow-headers"),
            Some("Range")
        );
        let exposed = header(&response.headers, "access-control-expose-headers")
            .ok_or("missing exposed headers")?;
        for name in ["Accept-Ranges", "Content-Length", "Content-Range"] {
            assert!(exposed.contains(name), "{exposed}");
        }
    }
    for method in [Method::POST, Method::PUT, Method::DELETE] {
        let response = request(&client, method, &server.origin, "/store/zarr.json", None)?;
        assert_eq!(response.status, StatusCode::METHOD_NOT_ALLOWED);
        assert!(response.body.is_empty());
        assert!(header(&response.headers, "allow").is_some_and(|value| value.contains("GET")));
    }
    for path in ["/not-a-route", "/vendor/", "/store"] {
        assert_eq!(
            get(&client, &server.origin, path)?.status,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    assert_eq!(
        request(
            &client,
            Method::OPTIONS,
            &server.origin,
            "/not-a-route",
            None
        )?
        .status,
        StatusCode::NOT_FOUND
    );
    Ok(())
}

#[test]
fn real_cli_info_and_query_read_the_running_nested_range_server() -> TestResult {
    let server = Server::spawn(Path::new(STORE))?;
    let url = format!("{}/store", server.origin);
    let info = Command::new(env!("CARGO_BIN_EXE_zarra"))
        .args(["info", "--url", &url])
        .output()?;
    assert!(
        info.status.success(),
        "info failed: {}",
        String::from_utf8_lossy(&info.stderr)
    );
    let info_text = String::from_utf8(info.stdout)?;
    assert!(info_text.contains("uint16"), "{info_text}");
    assert!(info_text.contains("shape=64x64"), "{info_text}");

    let query = Command::new(env!("CARGO_BIN_EXE_zarra"))
        .args([
            "query", "--url", &url, "--level", "0", "--x", "2", "--y", "2", "--width", "1",
            "--height", "1",
        ])
        .output()?;
    assert!(
        query.status.success(),
        "query failed: {}",
        String::from_utf8_lossy(&query.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&query.stdout)?;
    assert_eq!(json["values"], serde_json::json!([2002.0]));
    Ok(())
}
