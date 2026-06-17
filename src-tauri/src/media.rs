//! Range-aware clip streaming protocol.
//!
//! Tauri's built-in `asset:` protocol (`convertFileSrc`) is a known weak path for
//! video: historically it reads from the file start up to the seek point instead
//! of honoring HTTP `Range`, so seeking is slow and playback can starve — and two
//! `<video>` elements hitting it contend on one handler. WebView2/Chromium, by
//! contrast, drives `<video>` perfectly over a normal `206 Partial Content`
//! endpoint.
//!
//! This registers a custom scheme (`hakoclip://…`, built via
//! `convertFileSrc(path, "hakoclip")`) whose handler parses `Range` and returns
//! just the requested byte slice with the headers Chromium expects. Reads are
//! scoped to `<Videos>/Hako` so the webview can't pull arbitrary files.

use std::borrow::Cow;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use tauri::http::{header, Request, Response, StatusCode};
use tauri::{Manager, Runtime, UriSchemeContext};

/// The custom URI scheme. `convertFileSrc(clip.path, "hakoclip")` builds the URL.
pub const SCHEME: &str = "hakoclip";

/// Chunk cap for a single response. Chromium asks for small ranges as it buffers,
/// so we never need the whole file in memory; this just bounds an open-ended
/// (`bytes=N-`) request.
const MAX_CHUNK: u64 = 8 * 1024 * 1024;

/// Protocol entry point registered on the Tauri builder.
pub fn handle<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
) -> Response<Cow<'static, [u8]>> {
    match respond(&ctx, &request) {
        Ok(resp) => resp,
        Err(status) => Response::builder()
            .status(status)
            .body(Cow::Owned(Vec::new()))
            .unwrap(),
    }
}

fn respond<R: Runtime>(
    ctx: &UriSchemeContext<'_, R>,
    request: &Request<Vec<u8>>,
) -> Result<Response<Cow<'static, [u8]>>, StatusCode> {
    // URL path is the percent-encoded absolute file path (after the leading `/`).
    let raw = request.uri().path();
    let decoded = percent_decode(raw.strip_prefix('/').unwrap_or(raw));
    let path = PathBuf::from(decoded);

    if !is_allowed(ctx, &path) {
        return Err(StatusCode::FORBIDDEN);
    }

    let mut file = std::fs::File::open(&path).map_err(|_| StatusCode::NOT_FOUND)?;
    let file_size = file.metadata().map_err(|_| StatusCode::NOT_FOUND)?.len();
    if file_size == 0 {
        return Err(StatusCode::NOT_FOUND);
    }

    let range = request
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| parse_range(v, file_size));

    let (status, start, end) = match range {
        Some(Some((s, e))) => (StatusCode::PARTIAL_CONTENT, s, e),
        // A Range header we couldn't satisfy → 416.
        Some(None) => return Err(StatusCode::RANGE_NOT_SATISFIABLE),
        // No Range header → full file (status 200). Media elements send a Range
        // on their first request, so this path is essentially only direct loads.
        None => (StatusCode::OK, 0, file_size - 1),
    };

    let len = end - start + 1;
    let mut buf = vec![0u8; len as usize];
    file.seek(SeekFrom::Start(start))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    file.read_exact(&mut buf)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "video/mp4")
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, len.to_string());
    if status == StatusCode::PARTIAL_CONTENT || start > 0 || end < file_size - 1 {
        builder = builder.header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{file_size}"),
        );
    }
    builder
        .body(Cow::Owned(buf))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// Parse `bytes=start-end` / `bytes=start-` / `bytes=-suffix` against `size`.
/// `Some(Some((start, end)))` = satisfiable (inclusive), `Some(None)` = a Range
/// header we can't satisfy (→ 416), `None` = no usable bytes= range.
fn parse_range(value: &str, size: u64) -> Option<Option<(u64, u64)>> {
    let spec = value.trim().strip_prefix("bytes=")?;
    // Only the first range of a (rare) multi-range request is served.
    let spec = spec.split(',').next()?.trim();
    let (a, b) = spec.split_once('-')?;
    let last = size - 1;

    let (start, end) = if a.is_empty() {
        // suffix: last N bytes
        let n: u64 = b.parse().ok()?;
        if n == 0 {
            return Some(None);
        }
        (size.saturating_sub(n), last)
    } else {
        let start: u64 = a.parse().ok()?;
        let end: u64 = if b.is_empty() {
            (start + MAX_CHUNK - 1).min(last)
        } else {
            b.parse::<u64>().ok()?.min(last)
        };
        (start, end)
    };

    if start > end || start > last {
        return Some(None);
    }
    Some(Some((start, end)))
}

/// Only serve files under `<Videos>/Hako` (same trust boundary as the asset
/// protocol scope) and only `.mp4`.
fn is_allowed<R: Runtime>(ctx: &UriSchemeContext<'_, R>, path: &Path) -> bool {
    if path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("mp4")) != Some(true)
    {
        return false;
    }
    let Ok(root) = ctx.app_handle().path().video_dir() else {
        return false;
    };
    let root = root.join("Hako");
    match (std::fs::canonicalize(&root), std::fs::canonicalize(path)) {
        (Ok(root), Ok(path)) => path.starts_with(root),
        _ => false,
    }
}

/// Decode `%XX` escapes from `convertFileSrc` (encodeURIComponent) back to a path.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
