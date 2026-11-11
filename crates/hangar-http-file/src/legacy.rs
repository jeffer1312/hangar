use std::path::Path;

use axum::{
    body::Body,
    http::{HeaderMap, StatusCode, header},
    response::Response,
};
use md5::{Digest, Md5};

/// Resposta de arquivo sem o 304 de arquivos estáticos.
///
/// # Erros
/// Falha ao obter entropia para separar múltiplos intervalos.
pub async fn legacy_file(
    file: tokio::fs::File,
    metadata: std::fs::Metadata,
    path: &Path,
    headers: &HeaderMap,
    media: &str,
    allow_date: bool,
) -> std::io::Result<Response> {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .unwrap_or_default();
    let mtime = hangar_core::pycompat::pyjson::float_repr(modified.as_secs_f64());
    let etag = format!(
        "\"{:x}\"",
        Md5::digest(format!("{mtime}-{}", metadata.len()).as_bytes())
    );
    let last = chrono::DateTime::<chrono::Utc>::from_timestamp(modified.as_secs() as i64, 0)
        .map(|t| t.format("%a, %d %b %Y %H:%M:%S GMT").to_string())
        .unwrap_or_default();
    let mut request_headers = headers.clone();
    // FileResponse não implementa o 304 da StaticFiles.
    request_headers.remove(header::IF_NONE_MATCH);
    if let Some(value) = request_headers.remove(header::IF_RANGE) {
        // Cada consumidor conserva sua regra de validação pelo ETag ou pela data.
        if value != etag.as_str() && !(allow_date && value == last.as_str()) {
            request_headers.remove(header::RANGE);
        }
    }
    if let Some(value) = request_headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
    {
        let value = bounded_range(value);
        if let Ok(value) = value.parse() {
            request_headers.insert(header::RANGE, value);
        }
    }
    if let Some(ranges) = request_headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|value| crate::ranges::ranges(value, metadata.len()).ok())
        .filter(|ranges| ranges.len() > 1)
    {
        return multipart_file(file, metadata.len(), media, &last, &etag, ranges);
    }
    let mut response =
        crate::opened::serve_open_file(file, path, &request_headers, false, Some(media)).await;
    if response.headers().contains_key(header::ETAG) {
        response
            .headers_mut()
            .insert(header::ETAG, etag.parse().unwrap());
    }
    for name in [header::CACHE_CONTROL, header::REFERRER_POLICY] {
        response.headers_mut().remove(name);
    }
    response.headers_mut().remove("x-content-type-options");
    Ok(response)
}

/// A fronteira aleatória impede que bytes do próprio áudio virem separadores de um intervalo.
fn multipart_file(
    file: tokio::fs::File,
    size: u64,
    media: &str,
    last: &str,
    etag: &str,
    ranges: Vec<(u64, u64)>,
) -> std::io::Result<Response> {
    use ring::rand::SecureRandom;
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    enum Piece {
        Literal(bytes::Bytes),
        Data { offset: u64, left: u64 },
    }
    let mut random = [0u8; 13];
    ring::rand::SystemRandom::new()
        .fill(&mut random)
        .map_err(|_| {
            std::io::Error::other("Não foi possível preparar os intervalos do arquivo.")
        })?;
    let boundary = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let mut pieces = std::collections::VecDeque::new();
    let mut length = 0u64;
    for (start, end) in ranges {
        let prefix = format!(
            "--{boundary}\r\nContent-Type: {media}\r\nContent-Range: bytes {start}-{}/{}\r\n\r\n",
            i128::from(end) - 1,
            size
        );
        length += prefix.len() as u64 + end - start + 2;
        pieces.push_back(Piece::Literal(prefix.into()));
        if end > start {
            pieces.push_back(Piece::Data {
                offset: start,
                left: end - start,
            });
        }
        pieces.push_back(Piece::Literal(bytes::Bytes::from_static(b"\r\n")));
    }
    let suffix = format!("--{boundary}--");
    length += suffix.len() as u64;
    pieces.push_back(Piece::Literal(suffix.into()));
    let stream =
        futures_util::stream::try_unfold((file, pieces), |(mut file, mut pieces)| async move {
            match pieces.pop_front() {
                None => Ok::<_, std::io::Error>(None),
                Some(Piece::Literal(bytes)) => Ok(Some((bytes, (file, pieces)))),
                Some(Piece::Data { offset, left }) => {
                    file.seek(std::io::SeekFrom::Start(offset)).await?;
                    let mut bytes = vec![0; (left.min(64 * 1024)) as usize];
                    let count = file.read(&mut bytes).await?;
                    if count == 0 {
                        return Err(std::io::ErrorKind::UnexpectedEof.into());
                    }
                    bytes.truncate(count);
                    if left > count as u64 {
                        pieces.push_front(Piece::Data {
                            offset: offset + count as u64,
                            left: left - count as u64,
                        });
                    }
                    Ok(Some((bytes.into(), (file, pieces))))
                }
            }
        });
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = StatusCode::PARTIAL_CONTENT;
    for (name, value) in [
        (
            header::CONTENT_TYPE,
            format!("multipart/byteranges; boundary={boundary}"),
        ),
        (header::CONTENT_LENGTH, length.to_string()),
        (header::ACCEPT_RANGES, "bytes".to_owned()),
        (header::ETAG, etag.to_owned()),
        (header::LAST_MODIFIED, last.to_owned()),
    ] {
        response.headers_mut().insert(name, value.parse().unwrap());
    }
    Ok(response)
}

/// O Python lê inteiros arbitrários no Range; para um arquivo, saturar conserva as comparações.
fn bounded_range(value: &str) -> String {
    let mut output = String::new();
    let mut chars = value.chars().peekable();
    while let Some(character) = chars.next() {
        if character.is_ascii_digit() {
            let mut number = u64::from(character as u8 - b'0');
            while chars.peek().is_some_and(char::is_ascii_digit) {
                number = number
                    .saturating_mul(10)
                    .saturating_add(u64::from(chars.next().unwrap() as u8 - b'0'));
            }
            output.push_str(&number.to_string());
        } else {
            output.push(character);
        }
    }
    output
}
