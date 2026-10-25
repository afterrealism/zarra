//! Nested-object path validation and single-range parsing for the loopback server.

use std::path::{Component, Path, PathBuf};

/// The single byte range selected for a response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Range {
    /// No range, or a malformed/unsupported range that must be ignored.
    None,
    /// An inclusive byte span within the object.
    Span(u64, u64),
    /// A syntactically valid range that cannot be satisfied.
    Unsatisfiable,
}

/// Parses `bytes=a-b`, `bytes=a-`, and `bytes=-b` against an object length.
///
/// Malformed values and multipart ranges are ignored. Well-formed ranges on an
/// empty object, zero-length suffixes, and spans outside the object are 416s.
pub(crate) fn range_header(value: Option<&str>, len: u64) -> Range {
    let Some(value) = value else {
        return Range::None;
    };
    let Some((unit, spec)) = value.trim().split_once('=') else {
        return Range::None;
    };
    if !unit.eq_ignore_ascii_case("bytes") || spec.contains(',') {
        return Range::None;
    }
    let Some((first, last)) = spec.trim().split_once('-') else {
        return Range::None;
    };

    if first.is_empty() {
        let Ok(suffix) = last.parse::<u64>() else {
            return Range::None;
        };
        if suffix == 0 || len == 0 {
            return Range::Unsatisfiable;
        }
        return Range::Span(len.saturating_sub(suffix), len - 1);
    }

    let Ok(start) = first.parse::<u64>() else {
        return Range::None;
    };
    let requested_end = if last.is_empty() {
        None
    } else {
        let Ok(end) = last.parse::<u64>() else {
            return Range::None;
        };
        Some(end)
    };
    if len == 0 || start >= len {
        return Range::Unsatisfiable;
    }
    let end = requested_end.unwrap_or(len - 1).min(len - 1);
    if start > end {
        Range::Unsatisfiable
    } else {
        Range::Span(start, end)
    }
}

/// Percent-decodes a key once, preserving literal malformed percent sequences.
pub(crate) fn percent_decode(input: &str) -> Result<String, u16> {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                decoded.push((high << 4) | low);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(decoded).map_err(|_| 400)
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Resolves one raw nested key below an already-canonical store root.
///
/// Returns 400 for invalid UTF-8, 403 for path/symlink escapes, and 404 for
/// missing objects, directories, and other non-files.
pub(crate) fn resolve(root: &Path, raw_key: &str) -> Result<PathBuf, u16> {
    let decoded = percent_decode(raw_key)?;
    if decoded.is_empty() {
        return Err(404);
    }
    if decoded.contains(['\0', '\\'])
        || decoded.starts_with('/')
        || Path::new(&decoded).is_absolute()
    {
        return Err(403);
    }

    let mut joined = root.to_path_buf();
    let mut count = 0usize;
    for component in decoded.split('/') {
        if component.is_empty() {
            continue;
        }
        if component == "." || component == ".." {
            return Err(403);
        }
        let mut parsed = Path::new(component).components();
        if !matches!(
            (parsed.next(), parsed.next()),
            (Some(Component::Normal(_)), None)
        ) {
            return Err(403);
        }
        joined.push(component);
        count += 1;
    }
    if count == 0 {
        return Err(404);
    }

    let canonical = joined.canonicalize().map_err(|_| 404u16)?;
    if !canonical.starts_with(root) {
        return Err(403);
    }
    if !canonical.is_file() {
        return Err(404);
    }
    Ok(canonical)
}
