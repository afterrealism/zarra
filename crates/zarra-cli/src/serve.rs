//! Sequential loopback HTTP server for local raster stores and the embedded viewer.

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Args as ClapArgs;
use serde_json::{json, Value};
use url::{form_urlencoded, Position};
use zarra::{
    parse_band_list, render_tile, ConvertOptions, Error, RasterStore, Resampling, Result, Tile,
    TileCoord,
};

use crate::range::{range_header, resolve, Range};
use crate::sql::{self, SqlRequest};
use crate::viewer;

const MAX_HEAD: usize = 16 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(30);
const NO_CACHE: &str = "no-cache";
const NO_STORE: &str = "no-store";
const MAX_UPLOAD_BYTES: u64 = 512 * 1024 * 1024;
const MAX_QUERY_BYTES: u64 = 64 * 1024;
const ALLOW_READ: &str = "GET, HEAD, OPTIONS";
const ALLOW_ALL: &str = "GET, HEAD, POST, OPTIONS";

/// Arguments for the local store and viewer server.
#[derive(Debug, ClapArgs)]
pub(crate) struct ServeArgs {
    /// Local Zarr store directory.
    #[arg(value_name = "STORE")]
    store: PathBuf,
    /// Loopback TCP port; use 0 to select an available port.
    #[arg(long, default_value_t = 8090)]
    port: u16,
    /// Directory for no-overwrite GeoTIFF conversion uploads; disables uploads when omitted.
    #[arg(long, value_name = "DIRECTORY")]
    upload_root: Option<PathBuf>,
}

struct ServeState {
    root: PathBuf,
    store: RasterStore,
    upload_root: Option<PathBuf>,
    address: SocketAddr,
}

pub(crate) fn run(arguments: ServeArgs) -> Result<()> {
    let root = fs::canonicalize(&arguments.store)?;
    let store = RasterStore::open(&root)?;
    let upload_root = arguments
        .upload_root
        .map(|path| {
            fs::create_dir_all(&path)?;
            fs::canonicalize(path)
        })
        .transpose()?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, arguments.port))?;
    let address = listener.local_addr()?;
    let mut state = ServeState {
        root,
        store,
        upload_root,
        address,
    };
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    writeln!(stdout, "http://{address}")?;
    stdout.flush()?;
    drop(stdout);

    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                if let Err(error) = handle(stream, &mut state) {
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
    Post,
    Options,
}

struct Request {
    method: Method,
    target: String,
    range: Option<String>,
    content_length: Option<u64>,
    host: Option<String>,
    origin: Option<String>,
    sec_fetch_site: Option<String>,
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
            Some("POST") => Method::Post,
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
        let mut content_length = None;
        let mut host = None;
        let mut origin = None;
        let mut sec_fetch_site = None;
        for line in lines {
            if line.is_empty() {
                break;
            }
            let (name, value) = line.split_once(':').ok_or(400u16)?;
            let value = value.trim();
            if name.eq_ignore_ascii_case("transfer-encoding") {
                return Err(400);
            } else if name.eq_ignore_ascii_case("range") {
                if range.is_some() {
                    // Duplicate Range fields are malformed and therefore ignored.
                    range = Some(String::new());
                } else {
                    range = Some(value.to_owned());
                }
            } else if name.eq_ignore_ascii_case("content-length") {
                set_once(&mut content_length, value.parse().map_err(|_| 400u16)?)?;
            } else if name.eq_ignore_ascii_case("host") {
                set_once(&mut host, value.to_owned())?;
            } else if name.eq_ignore_ascii_case("origin") {
                set_once(&mut origin, value.to_owned())?;
            } else if name.eq_ignore_ascii_case("sec-fetch-site") {
                set_once(&mut sec_fetch_site, value.to_owned())?;
            }
        }
        Ok(Self {
            method,
            target,
            range,
            content_length,
            host,
            origin,
            sec_fetch_site,
        })
    }
}

fn set_once<T>(slot: &mut Option<T>, value: T) -> std::result::Result<(), u16> {
    if slot.is_some() {
        return Err(400);
    }
    *slot = Some(value);
    Ok(())
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
    allow: Option<&'static str>,
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
            allow: None,
            body: Body::Empty,
        }
    }

    fn method_not_allowed(allow: &'static str) -> Self {
        Self {
            allow: Some(allow),
            ..Self::empty(405)
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
            allow: None,
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
            allow: None,
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

fn handle(mut stream: TcpStream, state: &mut ServeState) -> io::Result<()> {
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
            let response = if status == 405 {
                Response::method_not_allowed(ALLOW_ALL)
            } else {
                Response::empty(status)
            };
            return send_response(&mut stream, false, false, response);
        }
    };
    let response = if request.method == Method::Post {
        post_route(&mut stream, state, &request)
    } else {
        route(state, &request)
    };
    send_response(
        &mut stream,
        request.method == Method::Head,
        request.method == Method::Options,
        response,
    )
}

fn route(state: &mut ServeState, request: &Request) -> Response {
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

    if matches!(path, "/api/uploads" | "/api/sql") {
        return Response::method_not_allowed("POST, OPTIONS");
    }
    if path == "/api/store" {
        return api_response(state);
    }
    if path.starts_with("/raster/") {
        return tile_response(&mut state.store, path, query);
    }
    if let Some(key) = path.strip_prefix("/store/") {
        return object_response(&state.root, key, request.range.as_deref());
    }
    if let Some(asset) = viewer::asset(path) {
        return Response::static_asset(asset);
    }
    Response::empty(404)
}

fn known_route(path: &str) -> bool {
    path == "/api/store"
        || matches!(path, "/api/uploads" | "/api/sql")
        || path.starts_with("/raster/")
        || path
            .strip_prefix("/store/")
            .is_some_and(|key| !key.is_empty())
        || viewer::asset(path).is_some()
}

fn json_response(status: u16, value: Value) -> Response {
    match serde_json::to_vec(&value) {
        Ok(body) => Response::owned(status, "application/json; charset=utf-8", NO_STORE, body),
        Err(error) => {
            eprintln!("serve JSON encoding error: {error}");
            Response::empty(500)
        }
    }
}

fn api_error(status: u16, message: impl Into<String>) -> Response {
    json_response(status, json!({ "error": message.into() }))
}

fn api_response(state: &ServeState) -> Response {
    let mut value = match state.store.manifest().to_value() {
        Ok(value) => value,
        Err(error) => return processing_error(500, error),
    };
    let Some(object) = value.as_object_mut() else {
        return processing_error(500, Error::Format("manifest is not a JSON object".into()));
    };
    object.insert(
        "tiles".into(),
        Value::String("/raster/{z}/{x}/{y}.png".into()),
    );
    object.insert(
        "store_name".into(),
        Value::String(
            state
                .root
                .file_name()
                .unwrap_or(state.root.as_os_str())
                .to_string_lossy()
                .into_owned(),
        ),
    );
    object.insert(
        "upload_enabled".into(),
        Value::Bool(state.upload_root.is_some()),
    );
    json_response(200, value)
}

struct ApiError {
    status: u16,
    message: String,
}

impl ApiError {
    fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn internal(context: &str, error: impl std::fmt::Display) -> Self {
        eprintln!("serve {context} error: {error}");
        Self::new(500, "Internal server error")
    }

    fn response(self) -> Response {
        api_error(self.status, self.message)
    }
}

type ApiResult<T> = std::result::Result<T, ApiError>;

fn finish_api(result: ApiResult<Value>, success_status: u16) -> Response {
    match result {
        Ok(value) => json_response(success_status, value),
        Err(error) => error.response(),
    }
}

fn post_route(stream: &mut TcpStream, state: &mut ServeState, request: &Request) -> Response {
    let (path, query) = match request.target.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (request.target.as_str(), None),
    };
    if !path.starts_with('/') {
        return api_error(400, "Request target must be an absolute path");
    }
    if !matches!(path, "/api/uploads" | "/api/sql") {
        return Response::method_not_allowed(ALLOW_READ);
    }
    if let Err(error) = reject_cross_site_post(request, state.address) {
        return error.response();
    }
    match path {
        "/api/sql" => finish_api(sql_api(state, stream, request), 200),
        "/api/uploads" => finish_api(upload_api(state, stream, request, query), 201),
        _ => Response::empty(404),
    }
}

fn reject_cross_site_post(request: &Request, address: SocketAddr) -> ApiResult<()> {
    if request
        .sec_fetch_site
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("cross-site"))
    {
        return Err(ApiError::new(
            403,
            "Cross-site POST requests are not allowed",
        ));
    }
    let expected_authority = address.to_string();
    if request.host.as_deref() != Some(expected_authority.as_str()) {
        return Err(ApiError::new(
            403,
            "POST Host must be the exact loopback listener",
        ));
    }
    let Some(origin) = request.origin.as_deref() else {
        return Ok(());
    };
    let parsed = url::Url::parse(origin)
        .map_err(|_| ApiError::new(403, "Cross-site POST requests are not allowed"))?;
    let authority = &parsed[Position::BeforeHost..Position::AfterPort];
    let valid = parsed.scheme() == "http"
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.query().is_none()
        && parsed.fragment().is_none()
        && parsed.path() == "/"
        && authority == expected_authority.as_str();
    if valid {
        Ok(())
    } else {
        Err(ApiError::new(
            403,
            "Cross-site POST requests are not allowed",
        ))
    }
}

fn read_body(stream: &mut TcpStream, request: &Request, maximum: u64) -> ApiResult<Vec<u8>> {
    let length = request
        .content_length
        .ok_or_else(|| ApiError::new(411, "Content-Length is required"))?;
    if length > maximum {
        return Err(ApiError::new(
            413,
            format!("Request body exceeds the {maximum}-byte limit"),
        ));
    }
    let length = usize::try_from(length)
        .map_err(|_| ApiError::new(413, "Request body does not fit this platform"))?;
    let mut body = Vec::new();
    body.try_reserve_exact(length)
        .map_err(|_| ApiError::new(413, "Cannot allocate the request body"))?;
    body.resize(length, 0);
    stream
        .read_exact(&mut body)
        .map_err(|_| ApiError::new(400, "Request body was incomplete"))?;
    Ok(body)
}

fn sql_api(state: &mut ServeState, stream: &mut TcpStream, request: &Request) -> ApiResult<Value> {
    let body = read_body(stream, request, MAX_QUERY_BYTES)?;
    let query: SqlRequest = serde_json::from_slice(&body)
        .map_err(|_| ApiError::new(400, "Invalid SQL JSON request body"))?;
    let response = sql::execute(&mut state.store, &query).map_err(|error| {
        let status = if matches!(error, Error::InvalidInput(_)) {
            400
        } else {
            502
        };
        ApiError::new(status, error.to_string())
    })?;
    serde_json::to_value(response).map_err(|error| ApiError::internal("SQL response", error))
}

struct UploadParams {
    filename: String,
    output: String,
    options: ConvertOptions,
}

fn set_param<T>(slot: &mut Option<T>, value: T, name: &str) -> ApiResult<()> {
    if slot.is_some() {
        return Err(ApiError::new(400, format!("{name} must be supplied once")));
    }
    *slot = Some(value);
    Ok(())
}

fn safe_basename(value: String, label: &str) -> ApiResult<String> {
    if value.is_empty()
        || matches!(value.as_str(), "." | "..")
        || value.len() > 255
        || value.contains(['/', '\\'])
        || value.chars().any(char::is_control)
    {
        return Err(ApiError::new(
            400,
            format!("{label} must be a safe basename of at most 255 bytes"),
        ));
    }
    Ok(value)
}

fn upload_params(query: Option<&str>) -> ApiResult<UploadParams> {
    let mut filename = None;
    let mut output = None;
    let mut resampling = None;
    let mut zstd_level = None;
    let mut maxzoom = None;
    let mut maxzoom_seen = false;
    for (name, value) in form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        match name.as_ref() {
            "filename" => set_param(&mut filename, value.into_owned(), "filename")?,
            "output" => set_param(&mut output, value.into_owned(), "output")?,
            "resampling" => {
                let parsed = value
                    .parse::<Resampling>()
                    .map_err(|error| ApiError::new(400, error.to_string()))?;
                set_param(&mut resampling, parsed, "resampling")?;
            }
            "zstd_level" => {
                let parsed = value.parse::<u8>().map_err(|_| {
                    ApiError::new(400, "zstd_level must be an integer from 1 to 22")
                })?;
                set_param(&mut zstd_level, parsed, "zstd_level")?;
            }
            "maxzoom" => {
                if maxzoom_seen {
                    return Err(ApiError::new(400, "maxzoom must be supplied once"));
                }
                maxzoom_seen = true;
                if !value.is_empty() {
                    maxzoom = Some(value.parse::<u8>().map_err(|_| {
                        ApiError::new(400, "maxzoom must be an integer from 0 to 22")
                    })?);
                }
            }
            _ => {
                return Err(ApiError::new(
                    400,
                    format!("Unknown upload parameter: {name}"),
                ))
            }
        }
    }
    let filename = filename
        .ok_or_else(|| ApiError::new(400, "filename is required"))
        .and_then(|value| safe_basename(value, "filename"))?;
    let lower = filename.to_ascii_lowercase();
    if !(lower.ends_with(".tif") || lower.ends_with(".tiff")) {
        return Err(ApiError::new(400, "Upload must be a .tif or .tiff file"));
    }
    let output = output
        .ok_or_else(|| ApiError::new(400, "output is required"))
        .and_then(|value| safe_basename(value, "output"))?;
    if output.len() <= ".zarr".len() || !output.to_ascii_lowercase().ends_with(".zarr") {
        return Err(ApiError::new(
            400,
            "output must be a nonempty basename ending in .zarr",
        ));
    }
    Ok(UploadParams {
        filename,
        output,
        options: ConvertOptions {
            maxzoom,
            resampling: resampling.unwrap_or_default(),
            zstd_level: zstd_level.unwrap_or(5),
        },
    })
}

fn upload_api(
    state: &mut ServeState,
    stream: &mut TcpStream,
    request: &Request,
    query: Option<&str>,
) -> ApiResult<Value> {
    let upload_root = state.upload_root.clone().ok_or_else(|| {
        ApiError::new(
            403,
            "Uploads are disabled; pass --upload-root to enable them",
        )
    })?;
    let params = upload_params(query)?;
    let output_path = upload_root.join(&params.output);
    match fs::symlink_metadata(&output_path) {
        Ok(_) => {
            return Err(ApiError::new(
                409,
                format!("Output already exists: {}", params.output),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(ApiError::internal("upload output check", error)),
    }
    let body = read_body(stream, request, MAX_UPLOAD_BYTES)?;
    if body.is_empty() {
        return Err(ApiError::new(400, "GeoTIFF upload must not be empty"));
    }
    let source_bytes =
        u64::try_from(body.len()).map_err(|error| ApiError::internal("upload size", error))?;
    let temporary = tempfile::Builder::new()
        .prefix("zarra-upload-")
        .tempdir()
        .map_err(|error| ApiError::internal("upload temporary directory", error))?;
    let input = temporary.path().join("input.tif");
    fs::write(&input, body).map_err(|error| ApiError::internal("upload write", error))?;
    let report = zarra::convert_with_options(&input, &output_path, params.options)
        .map_err(|error| ApiError::new(400, error.to_string()))?;
    let opened = RasterStore::open(&report.output)
        .map_err(|error| ApiError::internal("converted store open", error))?;
    let manifest = report
        .manifest
        .to_value()
        .map_err(|error| ApiError::internal("converted manifest", error))?;
    state.root = report.output;
    state.store = opened;
    Ok(json!({
        "source": params.filename,
        "source_bytes": source_bytes,
        "output": params.output,
        "levels": report.levels,
        "files": report.files,
        "manifest": manifest,
    }))
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
        Ok(Tile::Png(body)) => Response::owned(200, "image/png", NO_CACHE, body),
        Ok(Tile::Empty) => Response {
            cache_control: Some(NO_CACHE),
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
    if let Some(allow) = response.allow {
        head.push_str(&format!("Allow: {allow}\r\n"));
    }
    head.push_str("Access-Control-Allow-Origin: *\r\n");
    if options {
        head.push_str("Access-Control-Allow-Methods: GET, HEAD, POST, OPTIONS\r\n");
        head.push_str("Access-Control-Allow-Headers: Range, Content-Type\r\n");
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
        201 => "Created",
        204 => "No Content",
        206 => "Partial Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Payload Too Large",
        416 => "Range Not Satisfiable",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        _ => "Error",
    }
}

#[cfg(test)]
mod tests {
    use zarra::Resampling;

    use super::upload_params;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn upload_parameters_reach_conversion_options() -> TestResult {
        let params = upload_params(Some(
            "filename=sample.tif&output=custom.zarr&maxzoom=7&resampling=average&zstd_level=11",
        ))
        .map_err(|error| error.message)?;
        assert_eq!(params.options.maxzoom, Some(7));
        assert_eq!(params.options.resampling, Resampling::Average);
        assert_eq!(params.options.zstd_level, 11);
        Ok(())
    }
}
