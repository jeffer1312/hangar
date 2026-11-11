//! Efeitos em pixels da fonte; a janela só estica a textura pronta.
use std::{path::Path, sync::{Arc, Mutex}};
use gpui_kit::RenderImage;
use image::{Pixel, Rgba, RgbaImage};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundEffect { Dither, Ascii, Halftone, Scanlines, #[serde(other)] None }

pub const CHOICES: [(BackgroundEffect, &str); 5] = [
    (BackgroundEffect::None, "settings_effect_none"), (BackgroundEffect::Dither, "settings_effect_dither"),
    (BackgroundEffect::Ascii, "settings_effect_ascii"), (BackgroundEffect::Halftone, "settings_effect_halftone"),
    (BackgroundEffect::Scanlines, "settings_effect_scanlines"),
];

type Entry = ((BackgroundEffect, bool), Arc<RenderImage>);
// ponytail: só a fonte atual e suas oito variantes; várias imagens simultâneas pediriam teto por bytes.
static CACHE: Mutex<(Vec<u8>, Vec<Entry>)> = Mutex::new((Vec::new(), Vec::new()));

fn key(effect: BackgroundEffect, light: bool) -> (BackgroundEffect, bool) {
    (effect, light && !matches!(effect, BackgroundEffect::None | BackgroundEffect::Dither))
}

/// Bloqueante; serializa também pedidos simultâneos da mesma variante.
pub fn load(path: &Path, effect: BackgroundEffect, light: bool) -> Result<Arc<RenderImage>, &'static str> {
    if std::fs::metadata(path).map_err(|_| "backdrop_missing")?.len() > crate::media::BACKDROP_MAX_BYTES { return Err("backdrop_too_big"); }
    let bytes = std::fs::read(path).map_err(|_| "backdrop_missing")?;
    if bytes.len() as u64 > crate::media::BACKDROP_MAX_BYTES { return Err("backdrop_too_big"); }
    let key = key(effect, light);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if cache.0 != bytes { *cache = (bytes, Vec::new()); }
    if let Some((_, image)) = cache.1.iter().find(|(k, _)| *k == key) { return Ok(image.clone()); }
    let image = crate::media::decode(&cache.0, 2560, 2560, Some(key)).ok_or("backdrop_invalid")?;
    cache.1.push((key, image.clone()));
    Ok(image)
}

/// A textura na tela já é esta variante? Trava ocupada responde não, e quem chama recarrega.
pub fn shows(image: &Arc<RenderImage>, effect: BackgroundEffect, light: bool) -> bool {
    CACHE.try_lock().is_ok_and(|cache| cache.1.iter().any(|(k, cached)| *k == key(effect, light) && Arc::ptr_eq(cached, image)))
}

pub fn apply(source: RgbaImage, effect: BackgroundEffect, light: bool) -> RgbaImage {
    if effect == BackgroundEffect::None || source.width() == 0 || source.height() == 0 { return source; }
    const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    const GLYPHS: [[u8; 7]; 10] = [
        [0,0,0,0,0,0,0], [0,0,0,0,0,4,0], [0,4,0,0,4,0,0], [0,0,0,14,0,0,0], [0,0,14,0,14,0,0],
        [0,4,4,31,4,4,0], [0,21,14,31,14,21,0], [10,10,31,10,31,10,10], [17,2,4,4,8,16,17], [14,17,23,21,23,16,14],
    ];
    let sample = |x: u32, y: u32| *source.get_pixel(x.min(source.width() - 1), y.min(source.height() - 1));
    let density = |pixel: Rgba<u8>| { let luma = pixel.to_luma()[0]; if light { 255 - luma } else { luma } };
    let paper = if light { 255.0 } else { 0.0 };
    RgbaImage::from_fn(source.width(), source.height(), |x, y| {
        let original = sample(x, y);
        let mut result = original;
        match effect {
            BackgroundEffect::Dither => {
                result = sample(x / 2 * 2 + 1, y / 2 * 2 + 1);
                let peak = result[0].max(result[1]).max(result[2]) as f32;
                let threshold = BAYER[y as usize / 2 % 4][x as usize / 2 % 4];
                let gain = if peak / 255.0 > (threshold as f32 + 0.5) / 16.0 { 255.0 / peak.max(1.0) } else { 0.08 };
                for channel in &mut result.0[..3] { *channel = (*channel as f32 * gain).round() as u8; }
            }
            BackgroundEffect::Ascii => {
                let center = sample(x / 6 * 6 + 3, y / 8 * 8 + 4);
                let glyph = (f32::from(density(center)) / 255.0).sqrt() * 9.0;
                let ink = x % 6 < 5 && y % 8 < 7 && GLYPHS[glyph as usize][y as usize % 8] & (1 << (4 - x % 6)) != 0;
                for c in 0..3 { result[c] = (original[c] as f32 * 0.60 + if ink { center[c] as f32 * 0.40 } else { paper * 0.40 }) as u8; }
            }
            BackgroundEffect::Halftone => {
                let (left, top) = (x / 4 * 4, y / 4 * 4);
                let center = sample(left + 2, top + 2);
                let radius = 2.0 * (0.3 + 0.7 * (density(sample(left, top)) as f32 / 255.0).sqrt());
                let distance = ((x as f32 - left as f32 - 1.5).powi(2) + (y as f32 - top as f32 - 1.5).powi(2)).sqrt();
                let coverage = (radius + 0.5 - distance).clamp(0.0, 1.0) * center[3] as f32 / 255.0;
                for c in 0..3 { result[c] = (original[c] as f32 * 0.60 + (center[c] as f32 * coverage + paper * (1.0 - coverage)) * 0.40) as u8; }
            }
            BackgroundEffect::Scanlines if y % 3 == 0 => {
                for c in 0..3 { result[c] = if light { (original[c] as f32 + (255.0 - original[c] as f32) * (1.0 - 0.52)) as u8 } else { (original[c] as f32 * 0.52) as u8 }; }
            }
            _ => {}
        }
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;
    #[test]
    fn shown_variant_tracks_only_effective_theme_changes() {
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("hangar-effects-{}-{nonce}.png", std::process::id()));
        RgbaImage::from_pixel(8, 8, Rgba([100, 150, 200, 255])).save(&path).unwrap();
        let ascii = load(&path, BackgroundEffect::Ascii, false).unwrap();
        assert!(shows(&ascii, BackgroundEffect::Ascii, false) && !shows(&ascii, BackgroundEffect::Ascii, true));
        let dither = load(&path, BackgroundEffect::Dither, false).unwrap();
        assert!(shows(&dither, BackgroundEffect::Dither, true));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn reference_cells_and_legacy_settings() {
        let source = RgbaImage::from_pixel(8, 8, Rgba([100, 150, 200, 255]));
        assert_eq!(apply(source.clone(), BackgroundEffect::None, true), source);
        let dither = apply(source.clone(), BackgroundEffect::Dither, false);
        assert_eq!(dither.get_pixel(0, 0).0, [128, 191, 255, 255]);
        assert_eq!(dither.get_pixel(0, 0), dither.get_pixel(1, 1));
        assert_eq!(dither.get_pixel(4, 2).0, [8, 12, 16, 255]);
        assert_eq!(dither, apply(source.clone(), BackgroundEffect::Dither, true));
        let ascii = apply(source.clone(), BackgroundEffect::Ascii, false);
        assert_eq!(ascii.get_pixel(5, 7).0, [60, 90, 120, 255]);
        assert_eq!(ascii.get_pixel(2, 2), source.get_pixel(2, 2));
        let dots = apply(source.clone(), BackgroundEffect::Halftone, false);
        assert_eq!(dots.get_pixel(1, 1), source.get_pixel(1, 1));
        assert!(dots.get_pixel(0, 0)[0] < dots.get_pixel(1, 1)[0]);
        let scan = apply(source.clone(), BackgroundEffect::Scanlines, false);
        assert_eq!(scan.get_pixel(0, 0).0, [52, 78, 104, 255]);
        assert_eq!(scan.get_pixel(0, 1), source.get_pixel(0, 1));
        for effect in [BackgroundEffect::Ascii, BackgroundEffect::Halftone, BackgroundEffect::Scanlines] {
            assert_ne!(apply(source.clone(), effect, false), apply(source.clone(), effect, true));
            assert_eq!(apply(RgbaImage::from_pixel(1, 1, Rgba([20, 30, 40, 80])), effect, false).get_pixel(0, 0)[3], 80);
        }
        for json in ["{}", r#"{"background_effect":"old-value"}"#] {
            assert_eq!(serde_json::from_str::<crate::appearance::Appearance>(json).unwrap().background_effect, BackgroundEffect::None);
        }
    }
}
