#![cfg(feature = "http")]

use std::fs;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};

use zarra::RasterStore;

const STORE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/refpy.zarr");
const PREFIX: &str = "/nested/store/";
type TestResult = Result<(), Box<dyn std::error::Error>>;

struct Request {
    method: String,
    path: String,
    range: Option<String>,
}

fn read_request(stream: &mut TcpStream) -> io::Result<Option<Request>> {
    let mut head = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    while head.len() < 16 * 1024 && !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte)? == 0 {
            return Ok(None);
        }
        head.push(byte[0]);
    }
    if !head.ends_with(b"\r\n\r\n") {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&head);
    let mut lines = text.split("\r\n");
    let Some(request_line) = lines.next() else {
        return Ok(None);
    };
    let mut parts = request_line.split_whitespace();
    let (Some(method), Some(path)) = (parts.next(), parts.next()) else {
        return Ok(None);
    };
    let range = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("range")
            .then(|| value.trim().to_string())
    });
    Ok(Some(Request {
        method: method.into(),
        path: path.into(),
        range,
    }))
}

fn parse_range(value: &str, len: usize) -> Option<(usize, usize)> {
    let spec = value.strip_prefix("bytes=")?;
    if len == 0 || spec.contains(',') {
        return None;
    }
    if let Some(suffix) = spec.strip_prefix('-') {
        let suffix = suffix.parse::<usize>().ok().filter(|value| *value > 0)?;
        return Some((len.saturating_sub(suffix), len - 1));
    }
    let (start, end) = spec.split_once('-')?;
    let start = start.parse::<usize>().ok()?;
    if start >= len {
        return None;
    }
    let end = if end.is_empty() {
        len - 1
    } else {
        end.parse::<usize>().ok()?.min(len - 1)
    };
    (start <= end).then_some((start, end))
}

fn response(
    stream: &mut TcpStream,
    status: &str,
    body: &[u8],
    extra: &str,
    head_only: bool,
) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n",
        body.len()
    )?;
    if !head_only {
        stream.write_all(body)?;
    }
    Ok(())
}

fn handle(mut stream: TcpStream, root: &Path, fail_shards: bool) -> io::Result<()> {
    let Some(request) = read_request(&mut stream)? else {
        return Ok(());
    };
    let head_only = request.method == "HEAD";
    if request.method != "GET" && !head_only {
        return response(&mut stream, "405 Method Not Allowed", &[], "", false);
    }
    if fail_shards && request.path.starts_with("/nested/store/0/c/") {
        return response(&mut stream, "500 Internal Server Error", &[], "", head_only);
    }
    let Some(relative) = request.path.strip_prefix(PREFIX) else {
        return response(&mut stream, "404 Not Found", &[], "", head_only);
    };
    if relative.is_empty()
        || relative.contains("..")
        || relative.contains(['\\', '\0'])
        || relative.starts_with('/')
    {
        return response(&mut stream, "403 Forbidden", &[], "", head_only);
    }
    let body = match fs::read(root.join(relative)) {
        Ok(body) => body,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return response(&mut stream, "404 Not Found", &[], "", head_only);
        }
        Err(error) => return Err(error),
    };

    if let Some(range) = request.range {
        if range.contains(',') {
            return response(
                &mut stream,
                "400 Multiple Ranges Disabled",
                &[],
                "",
                head_only,
            );
        }
        let Some((start, end)) = parse_range(&range, body.len()) else {
            return response(
                &mut stream,
                "416 Range Not Satisfiable",
                &[],
                &format!("Content-Range: bytes */{}\r\n", body.len()),
                head_only,
            );
        };
        let slice = &body[start..=end];
        return response(
            &mut stream,
            "206 Partial Content",
            slice,
            &format!(
                "Content-Range: bytes {start}-{end}/{}\r\nAccept-Ranges: bytes\r\n",
                body.len()
            ),
            head_only,
        );
    }
    response(
        &mut stream,
        "200 OK",
        &body,
        "Accept-Ranges: bytes\r\n",
        head_only,
    )
}

fn serve(fail_shards: bool) -> io::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let root = PathBuf::from(STORE);
    let _server = std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else {
                continue;
            };
            let _ = handle(stream, &root, fail_shards);
        }
    });
    Ok(port)
}

#[test]
fn opens_nested_loopback_store_and_reads_exact_values() -> TestResult {
    let port = serve(false)?;
    let url = format!("http://127.0.0.1:{port}/nested/store/");
    let mut store = RasterStore::open_url(&url)?;
    let chunk = store
        .read_chunk(0, 0, 0)?
        .ok_or("HTTP reference chunk is missing")?;
    assert_eq!(chunk.values[2 * 16 + 9], 2009.0);
    let window = store.read_window(0, 12, 12, 8, 8)?;
    for row in 0..8u64 {
        for col in 0..8u64 {
            assert_eq!(
                window.values[(row * 8 + col) as usize],
                ((row + 12) * 1000 + col + 12) as f64
            );
        }
    }
    Ok(())
}

#[test]
fn http_transport_errors_are_not_sparse_chunks() -> TestResult {
    let port = serve(true)?;
    let url = format!("http://127.0.0.1:{port}/nested/store");
    let mut store = RasterStore::open_url(&url)?;
    assert!(store.read_chunk(0, 0, 0).is_err());
    assert!(store.read_window(0, 0, 0, 2, 2).is_err());
    Ok(())
}

#[test]
fn rejects_unsafe_urls_before_network_access() {
    for invalid in [
        "ftp://example.com/store",
        "file:///etc/passwd",
        "not-a-url",
        "http://example.com/store",
        "http://128.0.0.1:9/store",
        "http://localhost.example:9/store",
        "http://[::2]:9/store",
        "http://user@127.0.0.1:9/store",
        "http://@127.0.0.1:9/store",
        "http://127.0.0.1:9/store?key=value",
        "http://127.0.0.1:9/store#fragment",
    ] {
        assert!(
            RasterStore::open_url(invalid).is_err(),
            "accepted {invalid}"
        );
    }
}
