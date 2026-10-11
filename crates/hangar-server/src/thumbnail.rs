//! Miniatura de imagem pedida por `?w=<largura>` nas rotas de arquivo do dono.

use axum::{
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::json;
use image::ImageDecoder;
use std::{io::Cursor, path::Path};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

const MIN_WIDTH: u32 = 16;
const MAX_WIDTH: u32 = 1024;
// Arquivo maior que isso nem é lido: decodificar custaria mais que mandar o original.
const MAX_SOURCE_BYTES: u64 = 64 << 20;
// Panorama extremo: sem teto, "lado menor = w" geraria uma faixa enorme.
const MAX_LONG_SIDE: u32 = 2048;
// Uma página com dezenas de fotos pediria todas juntas; duas por vez seguram CPU e memória.
static DECODES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
const RASTER: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/bmp",
];

/// `w` da query: ausente → `None`; fora de 16..=1024 ou não inteiro → 422.
pub(crate) fn parse_width(value: Option<&String>) -> Result<Option<u32>, Response> {
    let Some(value) = value else { return Ok(None) };
    match value.parse::<u32>() {
        Ok(w) if (MIN_WIDTH..=MAX_WIDTH).contains(&w) => Ok(Some(w)),
        _ => Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            [(header::CONTENT_TYPE, "application/json")],
            json!({"detail":"largura inválida"}).to_string(),
        )
            .into_response()),
    }
}

/// Serve a miniatura quando dá; em qualquer outro caso, o original como `serve_open_file`.
pub(crate) async fn serve(
    mut file: tokio::fs::File,
    path: &Path,
    headers: &HeaderMap,
    download: bool,
    media: Option<&str>,
    width: Option<u32>,
) -> Response {
    use crate::workspace_routes::serve_open_file;
    let Some(width) = width.filter(|_| !download) else {
        return serve_open_file(file, path, headers, download, media).await;
    };
    let guessed = mime_guess::from_path(path).first_or_octet_stream().to_string();
    let is_raster = RASTER.contains(&media.unwrap_or(&guessed));
    let meta = match file.metadata().await {
        Ok(m) => m,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    // FIFO ou link para /dev/zero não tem fim nem tamanho confiável: nada de decodificar.
    if !is_raster || !meta.is_file() || meta.len() > MAX_SOURCE_BYTES {
        return serve_open_file(file, path, headers, download, media).await;
    }
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .unwrap_or_default();
    let etag = format!("\"thumb{width}-{:x}-{:x}\"", modified.as_nanos(), meta.len());
    if headers.get(header::IF_NONE_MATCH).is_some_and(|h| h == etag.as_str()) {
        let mut r = StatusCode::NOT_MODIFIED.into_response();
        r.headers_mut().insert(header::ETAG, etag.parse().unwrap());
        r.headers_mut()
            .insert(header::CACHE_CONTROL, "max-age=60".parse().unwrap());
        return r;
    }
    let Ok(_permit) = DECODES.acquire().await else {
        return serve_open_file(file, path, headers, download, media).await;
    };
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    // O arquivo pode crescer depois do metadata: o teto vale na leitura também.
    let outcome = match (&mut file).take(MAX_SOURCE_BYTES).read_to_end(&mut bytes).await {
        Ok(_) => tokio::task::spawn_blocking(move || reduce(&bytes, width))
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r.map_err(|e| e.to_string())),
        Err(e) => Err(e.to_string()),
    };
    let (body, content_type) = match outcome {
        Ok(Some(thumb)) => thumb,
        Ok(None) => return original(file, path, headers, media).await,
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "miniatura falhou; servindo o original");
            return original(file, path, headers, media).await;
        }
    };
    let mut r = body.into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, content_type.parse().unwrap());
    h.insert(header::ETAG, etag.parse().unwrap());
    h.insert(header::CACHE_CONTROL, "max-age=60".parse().unwrap());
    h.insert("x-content-type-options", "nosniff".parse().unwrap());
    h.insert("referrer-policy", "no-referrer".parse().unwrap());
    r
}

/// O arquivo já foi lido até o fim: volta ao início antes de servir o original.
async fn original(
    mut file: tokio::fs::File,
    path: &Path,
    headers: &HeaderMap,
    media: Option<&str>,
) -> Response {
    if file.seek(std::io::SeekFrom::Start(0)).await.is_err() {
        return StatusCode::NOT_FOUND.into_response();
    }
    crate::workspace_routes::serve_open_file(file, path, headers, false, media).await
}

/// Reduz até o lado menor ter `width` (a miniatura é um quadrado recortado); o lado maior para em
/// `MAX_LONG_SIDE`. `None` quando o lado menor já cabe (nunca amplia).
fn reduce(bytes: &[u8], width: u32) -> image::ImageResult<Option<(Vec<u8>, &'static str)>> {
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(256 << 20);
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    reader.limits(limits.clone());
    let mut decoder = reader.into_decoder()?;
    // `into_decoder` não reserva o buffer de saída (só `decode` faz): sem isto o `max_alloc` não valeria aqui.
    limits.reserve(decoder.total_bytes())?;
    // Foto de celular vem deitada com a rotação no EXIF; o navegador a aplica no original, e a
    // miniatura (sem EXIF) tem de sair já girada.
    let orientation = decoder.orientation()?;
    let mut img = image::DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    let (w, h) = (img.width() as u64, img.height() as u64);
    let short = w.min(h);
    if short <= width as u64 {
        return Ok(None);
    }
    let (nw, nh) = ((w * width as u64 / short) as u32, (h * width as u64 / short) as u32);
    let thumb = img.thumbnail(nw.min(MAX_LONG_SIDE), nh.min(MAX_LONG_SIDE));
    let mut out = Vec::new();
    // Com alfa vai PNG: o JPEG não tem transparência e o fundo viraria preto.
    if img.color().has_alpha() {
        thumb.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)?;
        Ok(Some((out, "image/png")))
    } else {
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80)
            .encode_image(&thumb.to_rgb8())?;
        Ok(Some((out, "image/jpeg")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageFormat, RgbImage, RgbaImage};

    fn encode(img: DynamicImage, format: ImageFormat) -> Vec<u8> {
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), format).unwrap();
        out
    }

    fn dims_and_format(bytes: &[u8]) -> (u32, u32, ImageFormat) {
        let format = image::guess_format(bytes).unwrap();
        let img = image::load_from_memory(bytes).unwrap();
        (img.width(), img.height(), format)
    }

    #[test]
    fn png_with_alpha_stays_png() {
        let src = encode(DynamicImage::ImageRgba8(RgbaImage::new(400, 200)), ImageFormat::Png);
        let (out, mime) = reduce(&src, 192).unwrap().unwrap();
        assert_eq!(mime, "image/png");
        assert_eq!(dims_and_format(&out), (384, 192, ImageFormat::Png));
    }

    #[test]
    fn jpeg_stays_jpeg() {
        let src = encode(DynamicImage::ImageRgb8(RgbImage::new(400, 200)), ImageFormat::Jpeg);
        let (out, mime) = reduce(&src, 192).unwrap().unwrap();
        assert_eq!(mime, "image/jpeg");
        assert_eq!(dims_and_format(&out), (384, 192, ImageFormat::Jpeg));
    }

    #[test]
    fn panorama_long_side_is_capped() {
        let src = encode(DynamicImage::ImageRgb8(RgbImage::new(8000, 400)), ImageFormat::Png);
        let (out, _) = reduce(&src, 192).unwrap().unwrap();
        let (w, h, _) = dims_and_format(&out);
        assert_eq!(w, MAX_LONG_SIDE);
        assert!(h < 192);
    }

    #[test]
    fn small_image_is_not_enlarged() {
        let src = encode(DynamicImage::ImageRgb8(RgbImage::new(100, 50)), ImageFormat::Png);
        assert!(reduce(&src, 192).unwrap().is_none());
    }

    // No Windows um diretório nem abre como arquivo; o caso só existe onde abre.
    #[cfg(unix)]
    #[tokio::test]
    async fn non_regular_file_is_not_decoded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.png");
        std::fs::create_dir(&path).unwrap();
        let file = tokio::fs::File::open(&path).await.unwrap();
        let r = serve(file, &path, &HeaderMap::new(), false, None, Some(64)).await;
        let etag = r.headers().get(header::ETAG).map(|h| h.to_str().unwrap().to_owned()).unwrap_or_default();
        assert!(!etag.contains("thumb"), "{etag}");
    }

    #[test]
    fn width_bounds() {
        assert_eq!(parse_width(None).unwrap(), None);
        assert_eq!(parse_width(Some(&"16".into())).unwrap(), Some(16));
        assert!(parse_width(Some(&"15".into())).is_err());
        assert!(parse_width(Some(&"1025".into())).is_err());
        assert!(parse_width(Some(&"abc".into())).is_err());
    }
}
