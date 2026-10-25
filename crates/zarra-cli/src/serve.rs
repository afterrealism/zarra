//! Sequential loopback HTTP server for local raster stores and the embedded viewer.

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Args as ClapArgs;
use zarra::{parse_band_list, render_tile, Error, RasterStore, Result, Tile, TileCoord};

use crate::range::{range_header, resolve, Range};
use crate::viewer;

const MAX_HEAD: usize = 16 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(30);
const TILE_CACHE: &str = "public, max-age=3600, s-maxage=86400";
const NO_CACHE: &str = "no-cache";
const NO_STORE: &str = "no-store";

/// Arguments for the local store and viewer server.
#[derive(Debug, ClapArgs)]
pub(crate) struct ServeArgs {
    /// Local Zarr store directory.
    #[arg(value_name = "STORE")]
    store: PathBuf,
    /// Loopback TCP port; use 0 to select an available port.
    #[arg(long, default_value_t = 8090)]
    port: u16,
}

pub(crate) fn run(arguments: ServeArgs) -> Result<()> {
    let root = fs::canonicalize(&arguments.store)?;
    let mut store = RasterStore::open(&root)?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, arguments.port))?;
    let address = listener.local_addr()?;
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    writeln!(stdout, "http://{address}")?;
    stdout.flush()?;
    drop(stdout);

    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                if let Err(error) = handle(stream, &root, &mut store) {
                    eprintln!("serve connection error: {error}");
                }
            }
            Err(error) => eprintln!("serve accept error: {error}"),
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Method {
    Get,
    Head,
    Options,
}

struct Request {
    method: Method,
    target: String,
    range: Option<String>,
}

impl Request {
    fn parse(head: &[u8]) -> std::result::Result<Self, u16> {
        let text = std::str::from_utf8(head).map_err(|_| 400u16)?;
        let mut lines = text.split("\r\n");
        let request_line = lines.next().ok_or(400u16)?;
        let mut parts = request_line.split_whitespace();
        let method = match parts.next() {
            Some("GET") => Method::Get,
            Some("HEAD") => Method::Head,
            Some("OPTIONS") => Method::Options,
            Some(_) => return Err(405),
            None => return Err(400),
        };
        let target = parts.next().ok_or(400u16)?.to_owned();
        let version = parts.next().ok_or(400u16)?;
        if parts.next().is_some() || !matches!(version, "HTTP/1.0" | "HTTP/1.1") {
            return Err(400);
        }

        let mut range = None;
        for line in lines {
            if line.is_empty() {
                break;
            }
            let (name, value) = line.split_once(':').ok_or(400u16)?;
            if name.eq_ignore_ascii_case("range") {
                if range.is_some() {
                    // Duplicate Range fields are malformed and therefore ignored.
                    range = Some(String::new());
                } else {
                    range = Some(value.trim().to_owned());
                }
            }
        }
        Ok(Self {
            method,
            target,
            range,
        })
    }
}

enum Body {
    Empty,
    Static(&'static [u8]),
    Owned(Vec<u8>),
    File {
        file: File,
        offset: u64,
        length: u64,
    },
}

struct Response {
    status: u16,
    content_type: Option<&'static str>,
    cache_control: Option<&'static str>,
    accept_ranges: bool,
    content_range: Option<String>,
    allow: bool,
    body: Body,
}

impl Response {
    fn empty(status: u16) -> Self {
        Self {
            status,
            content_type: None,
            cache_control: Some(NO_STORE),
            accept_ranges: false,
            content_range: None,
            allow: false,
            body: Body::Empty,
        }
    }

    fn owned(status: u16, content_type: &'static str, cache: &'static str, body: Vec<u8>) -> Self {
        Self {
            status,
            content_type: Some(content_type),
            cache_control: Some(cache),
            body: Body::Owned(body),
            ..Self::empty(status)
        }
    }

    fn static_asset(asset: viewer::Asset) -> Self {
        Self {
            status: 200,
            content_type: Some(asset.content_type),
            cache_control: Some(NO_CACHE),
            accept_ranges: false,
            content_range: None,
            allow: false,
            body: Body::Static(asset.body),
        }
    }

    fn object(
        file: File,
        status: u16,
        offset: u64,
        length: u64,
        content_range: Option<String>,
    ) -> Self {
        Self {
            status,
            content_type: Some("application/octet-stream"),
            cache_control: Some(NO_CACHE),
            accept_ranges: true,
            content_range,
            allow: false,
            body: Body::File {
                file,
                offset,
                length,
            },
        }
    }

    fn object_empty(status: u16) -> Self {
        Self {
            cache_control: Some(NO_CACHE),
            accept_ranges: true,
            ..Self::empty(status)
        }
    }

    fn options() -> Self {
        Self {
            status: 204,
            accept_ranges: true,
            ..Self::empty(204)
        }
    }

    fn content_length(&self) -> u64 {
        match &self.body {
            Body::Empty => 0,
            Body::Static(bytes) => bytes.len() as u64,
            Body::Owned(bytes) => bytes.len() as u64,
            Body::File { length, .. } => *length,
        }
    }
}

fn handle(mut stream: TcpStream, root: &Path, store: &mut RasterStore) -> io::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let head = match read_head(&mut stream) {
        Ok(Some(head)) => head,
        Ok(None) => return Ok(()),
        Err(error) if error.kind() == io::ErrorKind::InvalidData => {
            return send_response(&mut stream, false, false, Response::empty(431));
        }
        Err(error) => return Err(error),
    };
    let request = match Request::parse(&head) {
        Ok(request) => request,
        Err(status) => {
            let mut response = Response::empty(status);
            response.allow = status == 405;
            return send_response(&mut stream, false, false, response);
        }
    };
    let response = route(store, root, &request);
    send_response(
        &mut stream,
        request.method == Method::Head,
        request.method == Method::Options,
        response,
    )
}

fn route(store: &mut RasterStore, root: &Path, request: &Request) -> Response {
    let (path, query) = match request.target.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (request.target.as_str(), None),
    };
    if !path.starts_with('/') {
        return Response::empty(400);
    }
    if request.method == Method::Options {
        return if known_route(path) {
            Response::options()
        } else {
            Response::empty(404)
        };
    }

    if path == "/api/store" {
        return api_response(store);
    }
    if path.starts_with("/raster/") {
        return tile_response(store, path, query);
    }
    if let Some(key) = path.strip_prefix("/store/") {
        return object_response(root, key, request.range.as_deref());
    }
    if let Some(asset) = viewer::asset(path) {
        return Response::static_asset(asset);
    }
    Response::empty(404)
}

fn known_route(path: &str) -> bool {
    path == "/api/store"
        || path.starts_with("/raster/")
        || path
            .strip_prefix("/store/")
            .is_some_and(|key| !key.is_empty())
        || viewer::asset(path).is_some()
}

fn api_response(store: &RasterStore) -> Response {
    let mut value = match store.manifest().to_value() {
        Ok(value) => value,
        Err(error) => return processing_error(500, error),
    };
    let Some(object) = value.as_object_mut() else {
        return processing_error(500, Error::Format("manifest is not a JSON object".into()));
    };
    object.insert(
        "tiles".into(),
        serde_json::Value::String("/raster/{z}/{x}/{y}.png".into()),
    );
    match serde_json::to_vec(&value) {
        Ok(body) => Response::owned(200, "application/json; charset=utf-8", NO_STORE, body),
        Err(error) => {
            eprintln!("serve manifest encoding error: {error}");
            Response::empty(500)
        }
    }
}

fn tile_response(store: &mut RasterStore, path: &str, query: Option<&str>) -> Response {
    let raw = &path["/raster/".len()..];
    let mut parts = raw.split('/');
    let (Some(z), Some(x), Some(y), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Response::empty(400);
    };
    let Some(y) = y.strip_suffix(".png") else {
        return Response::empty(400);
    };
    let coordinates = (|| -> std::result::Result<TileCoord, std::num::ParseIntError> {
        Ok(TileCoord {
            z: z.parse()?,
            x: x.parse()?,
            y: y.parse()?,
        })
    })();
    let coordinates = match coordinates {
        Ok(coordinates) => coordinates,
        Err(_) => return Response::empty(400),
    };
    let bands = match query {
        None => Vec::new(),
        Some(query) => {
            let Some(value) = query.strip_prefix("bands=") else {
                return Response::empty(400);
            };
            match parse_band_list(value) {
                Ok(bands) => bands,
                Err(_) => return Response::empty(400),
            }
        }
    };
    match render_tile(store, coordinates, &bands) {
        Ok(Tile::Png(body)) => Response::owned(200, "image/png", TILE_CACHE, body),
        Ok(Tile::Empty) => Response {
            cache_control: Some(TILE_CACHE),
            ..Response::empty(204)
        },
        Err(Error::InvalidInput(_)) => Response::empty(400),
        Err(error) => processing_error(502, error),
    }
}

fn processing_error(status: u16, error: Error) -> Response {
    eprintln!("serve processing error: {error}");
    Response::empty(status)
}

fn object_response(root: &Path, key: &str, range: Option<&str>) -> Response {
    let path = match resolve(root, key) {
        Ok(path) => path,
        Err(status) => return Response::object_empty(status),
    };
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            return Response::object_empty(403);
        }
        Err(_) => return Response::object_empty(404),
    };
    let metadata = match file.metadata() {
        Ok(metadata) => metadata,
        Err(_) => return Response::object_empty(404),
    };
    if !metadata.is_file() {
        return Response::object_empty(404);
    }
    let len = metadata.len();
    match range_header(range, len) {
        Range::None => Response::object(file, 200, 0, len, None),
        Range::Span(start, end) => {
            let length = end - start + 1;
            Response::object(
                file,
                206,
                start,
                length,
                Some(format!("bytes {start}-{end}/{len}")),
            )
        }
        Range::Unsatisfiable => {
            let mut response = Response::object_empty(416);
            response.content_range = Some(format!("bytes */{len}"));
            response
        }
    }
}

fn read_head(stream: &mut TcpStream) -> io::Result<Option<Vec<u8>>> {
    let mut head = Vec::with_capacity(512);
    let mut byte = [0u8; 1];
    while head.len() < MAX_HEAD {
        if stream.read(&mut byte)? == 0 {
            return Ok(None);
        }
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            return Ok(Some(head));
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "request head exceeds 16 KiB",
    ))
}

fn send_response(
    stream: &mut TcpStream,
    head_only: bool,
    options: bool,
    response: Response,
) -> io::Result<()> {
    let length = response.content_length();
    let mut head = format!(
        "HTTP/1.1 {} {}\r\n",
        response.status,
        reason(response.status)
    );
    if response.status != 204 {
        head.push_str(&format!("Content-Length: {length}\r\n"));
    }
    if let Some(content_type) = response.content_type {
        head.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    if let Some(cache_control) = response.cache_control {
        head.push_str(&format!("Cache-Control: {cache_control}\r\n"));
    }
    if response.accept_ranges {
        head.push_str("Accept-Ranges: bytes\r\n");
    }
    if let Some(content_range) = response.content_range {
        head.push_str(&format!("Content-Range: {content_range}\r\n"));
    }
    if response.allow {
        head.push_str("Allow: GET, HEAD, OPTIONS\r\n");
    }
    head.push_str("Access-Control-Allow-Origin: *\r\n");
    if options {
        head.push_str("Access-Control-Allow-Methods: GET, HEAD, OPTIONS\r\n");
        head.push_str("Access-Control-Allow-Headers: Range\r\n");
        head.push_str(
            "Access-Control-Expose-Headers: Accept-Ranges, Content-Length, Content-Range\r\n",
        );
    }
    head.push_str("X-Content-Type-Options: nosniff\r\n");
    head.push_str("Referrer-Policy: no-referrer\r\n");
    head.push_str("Connection: close\r\n\r\n");
    stream.write_all(head.as_bytes())?;
    if head_only || response.status == 204 {
        return Ok(());
    }
    match response.body {
        Body::Empty => Ok(()),
        Body::Static(bytes) => stream.write_all(bytes),
        Body::Owned(bytes) => stream.write_all(&bytes),
        Body::File {
            mut file,
            offset,
            length,
        } => {
            file.seek(SeekFrom::Start(offset))?;
            let copied = io::copy(&mut file.take(length), stream)?;
            if copied == length {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "object changed while streaming its response",
                ))
            }
        }
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        206 => "Partial Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        416 => "Range Not Satisfiable",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        _ => "Error",
    }
}
