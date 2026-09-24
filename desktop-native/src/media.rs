use std::{collections::HashMap, hash::Hash, io::Cursor, sync::Arc};
use gpui_kit::RenderImage;
use image::{DynamicImage, Frame, ImageDecoder, ImageFormat, ImageReader, Limits, RgbaImage};

// Prévia mostrada em até 320×240; o dobro mantém nítido em tela com escala até 2.
const THUMB_W: u32 = 640;
const THUMB_H: u32 = 480;
// ponytail: teto em bytes das miniaturas decodificadas (~27 de 640×480); a cópia no atlas da GPU é só das desenhadas.
// O que está na tela não sai: se as visíveis sozinhas passam do teto, ele fica acima até elas saírem da tela.
pub const BUDGET: usize = 32 * 1024 * 1024;

pub enum MediaState { Loading, Image(Arc<RenderImage>), Failed(String) }

/// Decodifica com limite de memória e guarda só a miniatura; o original é buscado de novo em Abrir/Salvar.
/// Formato só pelo conteúdo real: nome que diz png com bytes de outra coisa não vira imagem.
pub fn thumbnail(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let format = match bytes {
        [0x89, b'P', b'N', b'G', ..] => ImageFormat::Png,
        [0xFF, 0xD8, 0xFF, ..] => ImageFormat::Jpeg,
        [b'G', b'I', b'F', b'8', ..] => ImageFormat::Gif,
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => ImageFormat::WebP,
        [b'B', b'M', ..] => ImageFormat::Bmp,
        _ => return None,
    };
    let mut limits = Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(256 * 1024 * 1024);
    // GIF guarda só o 1º quadro: a prévia não anima (a `img` da conversa não tem id).
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().ok()?;
    let orientation = decoder.orientation().ok()?;
    let mut picture = DynamicImage::from_decoder(decoder).ok()?;
    picture.apply_orientation(orientation);
    Some(Arc::new(RenderImage::new(vec![Frame::new(fit(picture))])))
}

// Reduz ao teto da miniatura e converte para BGRA, que é o que a GPUI desenha.
fn fit(picture: DynamicImage) -> RgbaImage {
    let picture = if picture.width() > THUMB_W || picture.height() > THUMB_H { picture.thumbnail(THUMB_W, THUMB_H) } else { picture };
    let mut pixels = picture.into_rgba8();
    for pixel in pixels.chunks_exact_mut(4) { pixel.swap(0, 2); }
    pixels
}

// Falha pesa pouco mas pesa: link quebrado repetido também não cresce sem fim.
fn cost(state: &MediaState) -> usize {
    match state {
        MediaState::Image(image) => (0..image.frame_count()).filter_map(|n| image.as_bytes(n)).map(<[u8]>::len).sum(),
        MediaState::Failed(_) => 1024,
        MediaState::Loading => 0,
    }
}

struct Entry { state: MediaState, seen: u64 }

/// Cache das prévias com teto: sai primeiro a menos vista, nunca uma desenhada no último quadro.
pub struct MediaCache<K> { map: HashMap<K, Entry>, bytes: usize, frame: u64 }

impl<K: Eq + Hash + Clone> MediaCache<K> {
    pub fn new() -> Self { Self { map: HashMap::new(), bytes: 0, frame: 0 } }

    /// Chamado no início de cada quadro; o que for lido depois conta como visível neste quadro.
    pub fn next_frame(&mut self) { self.frame += 1; }

    pub fn contains(&self, key: &K) -> bool { self.map.contains_key(key) }

    pub fn get(&mut self, key: &K) -> Option<&MediaState> {
        let frame = self.frame;
        self.map.get_mut(key).map(|entry| { entry.seen = frame; &entry.state })
    }

    /// Marca a busca em andamento; não pesa no teto e não expulsa nada.
    pub fn start(&mut self, key: K) { self.map.insert(key, Entry { state: MediaState::Loading, seen: self.frame }); }

    /// Grava e devolve o que saiu pelo teto, para liberar do atlas da GPU.
    pub fn insert(&mut self, key: K, state: MediaState) -> Vec<Arc<RenderImage>> {
        self.bytes += cost(&state);
        if let Some(old) = self.map.insert(key, Entry { state, seen: self.frame }) { self.bytes -= cost(&old.state); }
        let mut evicted = Vec::new();
        while self.bytes > BUDGET {
            let Some(victim) = self.map.iter()
                .filter(|(_, entry)| entry.seen < self.frame && !matches!(entry.state, MediaState::Loading))
                .min_by_key(|(_, entry)| entry.seen).map(|(key, _)| key.clone()) else { break };
            if let Some(entry) = self.map.remove(&victim) {
                self.bytes -= cost(&entry.state);
                if let MediaState::Image(image) = entry.state { evicted.push(image); }
            }
        }
        evicted
    }

    /// Esvazia tudo (inclusive o que estava "Carregando") e devolve as imagens para liberar.
    pub fn clear(&mut self) -> Vec<Arc<RenderImage>> {
        self.bytes = 0;
        self.map.drain().filter_map(|(_, entry)| match entry.state { MediaState::Image(image) => Some(image), _ => None }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(side: u32) -> MediaState { MediaState::Image(Arc::new(RenderImage::new(vec![Frame::new(RgbaImage::new(side, side))]))) }

    #[test]
    fn budget_evicts_oldest_but_keeps_what_was_drawn_last_frame() {
        let mut cache = MediaCache::new();
        let side = 1024; // 4 MiB cada
        for n in 0..8 { cache.next_frame(); assert!(cache.insert(n, picture(side)).is_empty()); }
        // Quadro atual desenha 0 e 1: são as mais antigas, mas estão na tela.
        cache.next_frame();
        cache.get(&0);
        cache.get(&1);
        let evicted = cache.insert(8, picture(side));
        assert_eq!(evicted.len(), 1);
        assert!(cache.contains(&0) && cache.contains(&1) && !cache.contains(&2));
        assert!(cache.bytes <= BUDGET);
    }

    #[test]
    fn thumbnail_is_bounded_and_keeps_aspect() {
        let mut png = Vec::new();
        DynamicImage::ImageRgb8(image::RgbImage::new(1920, 1200)).write_to(&mut Cursor::new(&mut png), ImageFormat::Png).unwrap();
        let thumb = thumbnail(&png).unwrap();
        let size = thumb.size(0);
        assert_eq!((size.width.0, size.height.0), (640, 400));
        assert!(thumbnail(b"nao e imagem").is_none());
    }

    #[test]
    fn gif_shows_first_frame() {
        let mut gif = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut gif);
            let frames = (0..2).map(|n| Frame::new(RgbaImage::from_pixel(1280, 960, image::Rgba([n * 200, 0, 0, 255]))));
            encoder.encode_frames(frames).unwrap();
        }
        let thumb = thumbnail(&gif).unwrap();
        assert_eq!(thumb.frame_count(), 1);
        assert_eq!((thumb.size(0).width.0, thumb.size(0).height.0), (640, 480));
    }
}
