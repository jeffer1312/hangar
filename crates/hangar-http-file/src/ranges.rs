use std::collections::VecDeque;

use axum::{
    body::Bytes,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};

use crate::stream::{Piece, file_stream};

#[derive(Debug, PartialEq, Eq)]
pub enum RangeError {
    Malformed(&'static str),
    Unsatisfiable,
}

pub fn ranges(value: &str, size: u64) -> Result<Vec<(u64, u64)>, RangeError> {
    let (units, text) = value
        .split_once('=')
        .ok_or(RangeError::Malformed("Malformed range header."))?;
    if !units.trim().eq_ignore_ascii_case("bytes") {
        return Err(RangeError::Malformed("Only support bytes range"));
    }
    let mut out = Vec::new();
    for part in text.split(',') {
        let Some((a, b)) = part.trim().split_once('-') else {
            continue;
        };
        let (a, b) = (a.trim(), b.trim());
        if a.is_empty() && b.is_empty() {
            continue;
        }
        let parsed = if a.is_empty() {
            b.parse::<i128>().ok().map(|count| {
                (
                    i128::from(size).saturating_sub(count).max(0),
                    i128::from(size),
                )
            })
        } else {
            a.parse::<i128>().ok().and_then(|start| {
                if b.is_empty() {
                    Some((start, i128::from(size)))
                } else {
                    b.parse::<i128>()
                        .ok()
                        .map(|end| (start, end.saturating_add(1).min(i128::from(size))))
                }
            })
        };
        if let Some(range) = parsed {
            out.push(range);
        }
    }
    if out.is_empty() {
        return Err(RangeError::Malformed(
            "Range header: range must be requested",
        ));
    }
    if out
        .iter()
        .any(|(start, _)| *start < 0 || *start >= i128::from(size))
    {
        return Err(RangeError::Unsatisfiable);
    }
    if out.iter().any(|(start, end)| start > end) {
        return Err(RangeError::Malformed(
            "Range header: start must be less than end",
        ));
    }
    out.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::new();
    for (start, end) in out {
        let (start, end) = (start as u64, end as u64);
        if let Some(last) = merged.last_mut().filter(|last| start <= last.1) {
            last.1 = last.1.max(end);
        } else {
            merged.push((start, end));
        }
    }
    Ok(merged)
}

pub(crate) async fn serve_ranges(
    file: tokio::fs::File,
    headers: &HeaderMap,
    mut r: Response,
    media: &str,
    size: u64,
    etag: &str,
    last: &str,
) -> Response {
    let use_range = headers
        .get(header::IF_RANGE)
        .is_none_or(|h| h == etag || h == last);
    let range = if use_range {
        headers.get(header::RANGE).and_then(|h| h.to_str().ok())
    } else {
        None
    };
    let mut pieces = VecDeque::new();
    if let Some(range) = range {
        let ranges = match ranges(range, size) {
            Ok(ranges) => ranges,
            Err(RangeError::Malformed(message)) => {
                return (
                    StatusCode::BAD_REQUEST,
                    [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                    message,
                )
                    .into_response();
            }
            Err(RangeError::Unsatisfiable) => {
                return (
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    [
                        (header::CONTENT_RANGE, format!("bytes */{size}")),
                        (header::CONTENT_TYPE, "text/plain; charset=utf-8".into()),
                    ],
                    "",
                )
                    .into_response();
            }
        };
        *r.status_mut() = StatusCode::PARTIAL_CONTENT;
        if ranges.len() == 1 {
            let (start, end) = ranges[0];
            r.headers_mut().insert(
                header::CONTENT_RANGE,
                format!("bytes {start}-{}/{size}", i128::from(end) - 1)
                    .parse()
                    .unwrap(),
            );
            r.headers_mut().insert(
                header::CONTENT_LENGTH,
                (end - start).to_string().parse().unwrap(),
            );
            if end > start {
                pieces.push_back(Piece::Data {
                    offset: start,
                    left: end - start,
                });
            }
        } else {
            let boundary = "hangar-workspace-range";
            r.headers_mut().insert(
                header::CONTENT_TYPE,
                format!("multipart/byteranges; boundary={boundary}")
                    .parse()
                    .unwrap(),
            );
            for (start, end) in ranges {
                pieces.push_back(Piece::Bytes(Bytes::from(format!("--{boundary}\r\nContent-Type: {media}\r\nContent-Range: bytes {start}-{}/{size}\r\n\r\n", i128::from(end)-1))));
                if end > start {
                    pieces.push_back(Piece::Data {
                        offset: start,
                        left: end - start,
                    });
                }
                pieces.push_back(Piece::Bytes(Bytes::from_static(b"\r\n")));
            }
            pieces.push_back(Piece::Bytes(Bytes::from(format!("--{boundary}--\r\n"))));
        }
    } else {
        r.headers_mut()
            .insert(header::CONTENT_LENGTH, size.to_string().parse().unwrap());
        if size > 0 {
            pieces.push_back(Piece::Data {
                offset: 0,
                left: size,
            });
        }
    }
    *r.body_mut() = file_stream(file, pieces);
    r
}
