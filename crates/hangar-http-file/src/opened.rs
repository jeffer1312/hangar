use std::path::Path;

use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use tokio::io::AsyncReadExt;

pub fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

pub async fn serve_open_file(
    file: tokio::fs::File,
    path: &Path,
    headers: &HeaderMap,
    download: bool,
    media: Option<&str>,
) -> Response {
    let meta = match file.metadata().await {
        Ok(m) => m,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    let size = meta.len();
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .unwrap_or_default();
    let representation = if download { "download" } else { "isolated" };
    let etag = format!("\"{representation}-{:x}-{size:x}\"", modified.as_nanos());
    if headers
        .get(header::IF_NONE_MATCH)
        .is_some_and(|h| h == etag.as_str())
    {
        let mut r = StatusCode::NOT_MODIFIED.into_response();
        r.headers_mut().insert(header::ETAG, etag.parse().unwrap());
        r.headers_mut()
            .insert(header::CACHE_CONTROL, "max-age=60".parse().unwrap());
        return r;
    }
    let guessed = mime_guess::from_path(path)
        .first_or_octet_stream()
        .to_string();
    let media = media.unwrap_or(&guessed).to_owned();
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let html = !download && ["text/html", "application/xhtml+xml"].contains(&media.as_str());
    // Como o Starlette: texto declara utf-8, e o invólucro do HTML é sempre text/html.
    let content_type = if html {
        "text/html; charset=utf-8".to_owned()
    } else if media.starts_with("text/") {
        format!("{media}; charset=utf-8")
    } else {
        media.clone()
    };
    let mut r = Response::new(Body::empty());
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, content_type.parse().unwrap());
    h.insert(header::ETAG, etag.parse().unwrap());
    h.insert(header::CACHE_CONTROL, "max-age=60".parse().unwrap());
    h.insert("x-content-type-options", "nosniff".parse().unwrap());
    h.insert("referrer-policy", "no-referrer".parse().unwrap());
    if download {
        h.insert(
            "content-security-policy",
            "sandbox; script-src 'none'".parse().unwrap(),
        );
        // O `quote` do Python, como o Starlette: qualquer byte fora dele vai em `filename*`, e
        // caractere de controle nunca chega cru ao cabeçalho.
        const QUOTE: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
            .remove(b'_')
            .remove(b'.')
            .remove(b'-')
            .remove(b'~')
            .remove(b'/');
        let encoded = percent_encoding::utf8_percent_encode(&name, QUOTE).to_string();
        let disposition = if encoded == name {
            format!("attachment; filename=\"{name}\"")
        } else {
            format!("attachment; filename*=utf-8''{encoded}")
        };
        h.insert(header::CONTENT_DISPOSITION, disposition.parse().unwrap());
    }
    if html {
        use base64::Engine;
        let title = html_escape(&name);
        let prefix = format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{title}</title><style>html,body{{margin:0;height:100%;overflow:hidden}}iframe{{display:block;width:100%;height:100%;border:0}}</style></head><body><iframe title=\"{title}\" sandbox=\"allow-scripts allow-popups\" referrerpolicy=\"no-referrer\" src=\"data:{media};charset=utf-8;base64,"
        );
        let suffix = b"\"></iframe></body></html>";
        let body = Body::from_stream(futures_util::stream::try_unfold(
            (file, 0u8, Vec::<u8>::new(), prefix),
            move |(mut file, phase, mut remainder, prefix)| async move {
                if phase == 0 {
                    return Ok::<_, std::io::Error>(Some((
                        Bytes::from(prefix.clone()),
                        (file, 1, remainder, prefix),
                    )));
                }
                if phase == 2 {
                    return Ok(None);
                }
                let mut bytes = vec![0; 48 * 1024];
                let n = file.read(&mut bytes).await?;
                if n == 0 {
                    let tail = base64::engine::general_purpose::STANDARD.encode(&remainder)
                        + std::str::from_utf8(suffix).unwrap();
                    return Ok(Some((Bytes::from(tail), (file, 2, Vec::new(), prefix))));
                }
                bytes.truncate(n);
                remainder.extend(bytes);
                let cut = remainder.len() / 3 * 3;
                let encoded = base64::engine::general_purpose::STANDARD.encode(&remainder[..cut]);
                let rest = remainder.split_off(cut);
                Ok(Some((Bytes::from(encoded), (file, 1, rest, prefix))))
            },
        ));
        *r.body_mut() = body;
        return r;
    }
    if ["image/svg+xml", "application/xml", "text/xml"].contains(&media.as_str()) {
        r.headers_mut().insert(
            "content-security-policy",
            "sandbox; script-src 'none'".parse().unwrap(),
        );
    }
    r.headers_mut()
        .insert(header::ACCEPT_RANGES, "bytes".parse().unwrap());
    let last = chrono::DateTime::<chrono::Utc>::from_timestamp(modified.as_secs() as i64, 0)
        .map(|t| t.format("%a, %d %b %Y %H:%M:%S GMT").to_string())
        .unwrap_or_default();
    if let Ok(h) = last.parse() {
        r.headers_mut().insert(header::LAST_MODIFIED, h);
    }
    crate::ranges::serve_ranges(file, headers, r, &media, size, &etag, &last).await
}
