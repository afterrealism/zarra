//! Explicitly allowlisted, compile-time embedded viewer assets.

pub(crate) struct Asset {
    pub(crate) body: &'static [u8],
    pub(crate) content_type: &'static str,
}

pub(crate) fn asset(path: &str) -> Option<Asset> {
    let (body, content_type) = match path {
        "/" | "/index.html" => (
            &include_bytes!("../assets/index.html")[..],
            "text/html; charset=utf-8",
        ),
        "/app.js" => (
            &include_bytes!("../assets/app.js")[..],
            "text/javascript; charset=utf-8",
        ),
        "/geo.js" => (
            &include_bytes!("../assets/geo.js")[..],
            "text/javascript; charset=utf-8",
        ),
        "/styles.css" => (
            &include_bytes!("../assets/styles.css")[..],
            "text/css; charset=utf-8",
        ),
        "/favicon.svg" => (
            &include_bytes!("../assets/favicon.svg")[..],
            "image/svg+xml",
        ),
        "/vendor/maplibre-gl.js" => (
            &include_bytes!("../assets/vendor/maplibre-gl.js")[..],
            "text/javascript; charset=utf-8",
        ),
        "/vendor/maplibre-gl.css" => (
            &include_bytes!("../assets/vendor/maplibre-gl.css")[..],
            "text/css; charset=utf-8",
        ),
        "/vendor/maplibre-LICENSE.txt" => (
            &include_bytes!("../assets/vendor/maplibre-LICENSE.txt")[..],
            "text/plain; charset=utf-8",
        ),
        _ => return None,
    };
    Some(Asset { body, content_type })
}
