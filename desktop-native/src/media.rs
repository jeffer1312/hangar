use std::{collections::HashMap, hash::Hash, io::Cursor, sync::Arc};
use gpui_kit::RenderImage;
use image::{AnimationDecoder, Delay, DynamicImage, Frame, ImageDecoder, ImageFormat, ImageReader, Limits, RgbaImage, codecs::gif::GifDecoder};

// Prévia mostrada em até 320×240; o dobro mantém nítido em tela com escala até 2.
const THUMB_W: u32 = 640;
const THUMB_H: u32 = 480;
// ponytail: teto em bytes das miniaturas decodificadas (~27 de 640×480); a cópia no atlas da GPU é só das desenhadas.
// O que está na tela não sai: se as visíveis sozinhas passam do teto, ele fica acima até elas saírem da tela.
pub const BUDGET: usize = 32 * 1024 * 1024;
// ponytail: GIF anima só até este peso somado dos quadros (~35 de 400×300, o bastante até escala 1,25); acima fica parado no 1º.
const GIF_W: u32 = 400;
const GIF_H: u32 = 300;
const GIF_BUDGET: usize = 16 * 1024 * 1024;
// Quadros minúsculos não pesam no teto de bytes, mas cada um custa decodificação: o número também tem teto.
const GIF_FRAMES: usize = 240;

pub enum MediaState { Loading, Image(Arc<RenderImage>), Failed(String) }

// Fundo de tela: o arquivo escolhido é copiado inteiro até este tamanho e desenhado reduzido a este lado.
pub const BACKDROP_MAX_BYTES: u64 = 25 * 1024 * 1024;
const BACKDROP_SIDE: u32 = 2560;

/// Só para medir: com `HANGAR_NATIVE_MEDIA_TRACE` no ambiente, cada etapa das miniaturas vai ao stderr com os ms
/// desde a primeira chamada. Sem a variável, `format_args!` nem é formatado.
pub fn trace(what: std::fmt::Arguments) {
    static START: std::sync::OnceLock<Option<std::time::Instant>> = std::sync::OnceLock::new();
    if let Some(start) = START.get_or_init(|| std::env::var_os("HANGAR_NATIVE_MEDIA_TRACE").map(|_| std::time::Instant::now())) {
        eprintln!("media {:9.1} {what}", start.elapsed().as_secs_f64() * 1000.);
    }
}

fn ms(since: std::time::Instant) -> f64 { since.elapsed().as_secs_f64() * 1000. }

/// Formato só pelo conteúdo real: nome que diz png com bytes de outra coisa não vira imagem.
fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
    Some(match bytes {
        [0x89, b'P', b'N', b'G', ..] => ImageFormat::Png,
        [0xFF, 0xD8, 0xFF, ..] => ImageFormat::Jpeg,
        [b'G', b'I', b'F', b'8', ..] => ImageFormat::Gif,
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => ImageFormat::WebP,
        [b'B', b'M', ..] => ImageFormat::Bmp,
        _ => return None,
    })
}

fn limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(256 * 1024 * 1024);
    limits
}

/// A imagem inteira, com limite de memória e já de pé pela orientação EXIF (foto de celular): é assim que ela aparece.
pub(crate) fn oriented(bytes: &[u8]) -> Option<DynamicImage> {
    let format = sniff(bytes)?;
    // GIF aqui guarda só o 1º quadro; quem anima é `animated`.
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(limits());
    let mut decoder = reader.into_decoder().ok()?;
    let orientation = decoder.orientation().ok()?;
    let mut picture = DynamicImage::from_decoder(decoder).ok()?;
    picture.apply_orientation(orientation);
    Some(picture)
}

/// Decodifica com limite de memória e reduz ao lado pedido; `None` = bytes que não são imagem legível.
pub(crate) fn decode(bytes: &[u8], w: u32, h: u32, effect: Option<(crate::effects::BackgroundEffect, bool)>) -> Option<Arc<RenderImage>> {
    let started = std::time::Instant::now();
    let picture = oriented(bytes)?;
    let (decoded, (pw, ph)) = (ms(started), (picture.width(), picture.height()));
    let started = std::time::Instant::now();
    let pixels = fit(picture, w, h, effect);
    trace(format_args!("decode {pw}x{ph} {decoded:.1} ms, fit to {}x{} {:.1} ms", pixels.width(), pixels.height(), ms(started)));
    Some(Arc::new(RenderImage::new(vec![Frame::new(pixels)])))
}

/// Guarda só a miniatura; o original é buscado de novo em Abrir/Salvar. GIF dentro do teto anima.
pub fn thumbnail(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    if sniff(bytes) == Some(ImageFormat::Gif) && let Some(frames) = animated(bytes) { return Some(Arc::new(RenderImage::new(frames))); }
    decode(bytes, THUMB_W, THUMB_H, None)
}

pub const REVEAL_FRAMES: usize = 12;
// Largura da borda esfumada, em fração da meia diagonal.
const REVEAL_FEATHER: f32 = 0.45;

/// Quadros da revelação da miniatura que acabou de chegar: a imagem a 1/4 do lado, com uma máscara radial de borda
/// esfumada crescendo do centro. A GPUI não tem máscara: o alfa vai nos pixels, e assim vale sobre papel de parede.
/// GIF animado não ganha revelação.
pub fn reveal(image: &RenderImage) -> Vec<Arc<RenderImage>> {
    if image.frame_count() != 1 { return Vec::new(); }
    let size = image.size(0);
    let (w, h) = (size.width.0.max(0) as u32, size.height.0.max(0) as u32);
    let Some(full) = image.as_bytes(0).and_then(|bytes| RgbaImage::from_raw(w, h, bytes.to_vec())) else { return Vec::new() };
    // Os canais estão em BGRA; reduzir e mexer no alfa não depende da ordem.
    let small = image::imageops::thumbnail(&full, (w / 4).max(1), (h / 4).max(1));
    let (center_x, center_y) = (small.width() as f32 / 2., small.height() as f32 / 2.);
    let half = center_x.hypot(center_y);
    let feather = half * REVEAL_FEATHER;
    (1..=REVEAL_FRAMES).map(|k| {
        let radius = k as f32 / REVEAL_FRAMES as f32 * (half + feather);
        let mut frame = small.clone();
        for (x, y, pixel) in frame.enumerate_pixels_mut() {
            let distance = (x as f32 + 0.5 - center_x).hypot(y as f32 + 0.5 - center_y);
            pixel.0[3] = (pixel.0[3] as f32 * reveal_alpha(radius - distance, feather)).round() as u8;
        }
        Arc::new(RenderImage::new(vec![Frame::new(frame)]))
    }).collect()
}

/// Alfa a `inside` px para dentro do raio: 0 no raio, 1 a `feather` px dele, em `smoothstep`.
fn reveal_alpha(inside: f32, feather: f32) -> f32 {
    let t = (inside / feather).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

/// Quadros do GIF reduzidos, com o atraso de cada um; `None` = um quadro só, ilegível ou acima do teto (fica o 1º quadro).
fn animated(bytes: &[u8]) -> Option<Vec<Frame>> {
    let mut decoder = GifDecoder::new(Cursor::new(bytes)).ok()?;
    decoder.set_limits(limits()).ok()?;
    let (mut frames, mut total) = (Vec::new(), 0);
    for frame in decoder.into_frames() {
        let frame = frame.ok()?;
        // Atraso quase zero corre como o navegador corre: 100 ms.
        let delay = match frame.delay().numer_denom_ms() { (n, d) if n < 20 * d.max(1) => Delay::from_numer_denom_ms(100, 1), _ => frame.delay() };
        let pixels = fit(DynamicImage::ImageRgba8(frame.into_buffer()), GIF_W, GIF_H, None);
        total += pixels.len();
        if total > GIF_BUDGET || frames.len() >= GIF_FRAMES { return None; }
        frames.push(Frame::from_parts(pixels, 0, 0, delay));
    }
    (frames.len() > 1).then_some(frames)
}

/// Imagem de fundo ou papel de parede, reduzida ao tamanho de uma tela grande.
pub fn backdrop(bytes: &[u8]) -> Option<Arc<RenderImage>> { decode(bytes, BACKDROP_SIDE, BACKDROP_SIDE, None) }

/// Bloqueante: valida o arquivo escolhido e guarda uma cópia em `dest`, trocando a anterior só se tudo deu certo.
/// O erro volta como chave de tradução.
pub fn adopt_backdrop(source: &std::path::Path, dest: &std::path::Path) -> Result<Arc<RenderImage>, &'static str> {
    let meta = std::fs::metadata(source).map_err(|_| "backdrop_missing")?;
    if meta.len() > BACKDROP_MAX_BYTES { return Err("backdrop_too_big"); }
    let bytes = std::fs::read(source).map_err(|_| "backdrop_missing")?;
    let image = backdrop(&bytes).ok_or("backdrop_invalid")?;
    let dir = dest.parent().ok_or("backdrop_not_saved")?;
    std::fs::create_dir_all(dir).map_err(|_| "backdrop_not_saved")?;
    let tmp = dest.with_extension("tmp");
    std::fs::write(&tmp, &bytes).and_then(|_| std::fs::rename(&tmp, dest)).map_err(|_| "backdrop_not_saved")?;
    Ok(image)
}


/// Grão da Textura: ruído cinza opaco, desenhado em ladrilhos com pouca opacidade. Tira o degrau do gradiente escuro.
pub fn grain() -> Arc<RenderImage> {
    const SIDE: u32 = 256;
    let mut state = 0x2545_f491_u32;
    let pixels = RgbaImage::from_fn(SIDE, SIDE, |_, _| {
        state ^= state << 13; state ^= state >> 17; state ^= state << 5;
        let v = (state >> 24) as u8;
        image::Rgba([v, v, v, 255])
    });
    Arc::new(RenderImage::new(vec![Frame::new(pixels)]))
}

// Reduz ao teto pedido e converte para BGRA, que é o que a GPUI desenha.
fn fit(picture: DynamicImage, w: u32, h: u32, effect: Option<(crate::effects::BackgroundEffect, bool)>) -> RgbaImage {
    let styled = effect.is_some_and(|(e, _)| e != crate::effects::BackgroundEffect::None);
    let (w, h) = if styled { (w.min(2048), h.min(2048)) } else { (w, h) };
    let picture = if styled || picture.width() > w || picture.height() > h { picture.thumbnail(w, h) } else { picture };
    let mut pixels = picture.into_rgba8();
    if let Some((effect, light)) = effect { pixels = crate::effects::apply(pixels, effect, light); }
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

struct Entry { state: MediaState, seen: u64, drawn: bool }

/// Cache das prévias com teto: sai primeiro a menos vista, nunca uma desenhada no último quadro.
pub struct MediaCache<K> { map: HashMap<K, Entry>, bytes: usize, frame: u64, budget: usize }

impl<K: Eq + Hash + Clone + std::fmt::Debug> MediaCache<K> {
    pub fn new() -> Self { Self::with_budget(BUDGET) }

    pub fn with_budget(budget: usize) -> Self { Self { map: HashMap::new(), bytes: 0, frame: 0, budget } }

    /// Chamado no início de cada quadro; o que for lido depois conta como visível neste quadro.
    pub fn next_frame(&mut self) { self.frame += 1; }

    pub fn contains(&self, key: &K) -> bool { self.map.contains_key(key) }

    pub fn get(&mut self, key: &K) -> Option<&MediaState> {
        let frame = self.frame;
        self.map.get_mut(key).map(|entry| {
            entry.seen = frame;
            if !entry.drawn && matches!(entry.state, MediaState::Image(_)) { entry.drawn = true; trace(format_args!("drawn {key:?}")); }
            &entry.state
        })
    }

    /// Marca a busca em andamento; não pesa no teto e não expulsa nada.
    pub fn start(&mut self, key: K) { self.map.insert(key, Entry { state: MediaState::Loading, seen: self.frame, drawn: false }); }

    /// Grava e devolve o que saiu pelo teto, para liberar do atlas da GPU.
    pub fn insert(&mut self, key: K, state: MediaState) -> Vec<Arc<RenderImage>> {
        self.bytes += cost(&state);
        if let Some(old) = self.map.insert(key, Entry { state, seen: self.frame, drawn: false }) { self.bytes -= cost(&old.state); }
        let mut evicted = Vec::new();
        while self.bytes > self.budget {
            let Some(victim) = self.map.iter()
                .filter(|(_, entry)| entry.seen < self.frame && !matches!(entry.state, MediaState::Loading))
                .min_by_key(|(_, entry)| entry.seen).map(|(key, _)| key.clone()) else { break };
            if let Some(entry) = self.map.remove(&victim) {
                self.bytes -= cost(&entry.state);
                if let MediaState::Image(image) = entry.state { evicted.push(image); }
            }
        }
        trace(format_args!("cache {} entries, {:.1} MiB, evicted {}", self.map.len(), self.bytes as f64 / 1048576., evicted.len()));
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
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    fn picture(side: u32) -> MediaState { MediaState::Image(Arc::new(RenderImage::new(vec![Frame::new(RgbaImage::new(side, side))]))) }

    #[test]
    fn reveal_grows_from_the_center_and_ends_opaque() {
        let opaque = RenderImage::new(vec![Frame::new(RgbaImage::from_pixel(80, 40, image::Rgba([10, 20, 30, 255])))]);
        let frames = reveal(&opaque);
        assert_eq!(frames.len(), REVEAL_FRAMES);
        let alpha = |frame: &RenderImage, x: usize, y: usize| frame.as_bytes(0).unwrap()[(y * 20 + x) * 4 + 3];
        assert_eq!(frames[0].size(0).width.0, 20);
        assert_eq!(alpha(&frames[0], 0, 0), 0, "o canto começa escondido");
        assert!(alpha(&frames[0], 10, 5) > 0, "o centro aparece primeiro");
        assert!(frames.last().unwrap().as_bytes(0).unwrap().chunks_exact(4).all(|p| p[3] == 255), "termina inteira");
        let gif = RenderImage::new(vec![Frame::new(RgbaImage::new(4, 4)), Frame::new(RgbaImage::new(4, 4))]);
        assert!(reveal(&gif).is_empty());
    }

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
    fn gif_animates_within_budget_and_stays_still_above_it() {
        let gif = |count: u8, w: u32, h: u32| {
            let mut gif = Vec::new();
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut gif);
            let frames = (0..count).map(|n| Frame::from_parts(RgbaImage::from_pixel(w, h, image::Rgba([n.wrapping_mul(6), 0, 0, 255])), 0, 0, Delay::from_numer_denom_ms(0, 1)));
            encoder.encode_frames(frames).unwrap();
            drop(encoder);
            gif
        };
        let small = thumbnail(&gif(2, 1280, 960)).unwrap();
        assert_eq!(small.frame_count(), 2);
        assert_eq!((small.size(1).width.0, small.size(1).height.0), (400, 300));
        // Atraso zero vira o dos navegadores.
        assert_eq!(small.delay(0).numer_denom_ms(), (100, 1));
        // 40 quadros de 400×300 passam do teto: parado no 1º, na miniatura de sempre.
        let heavy = thumbnail(&gif(40, 400, 300)).unwrap();
        assert_eq!(heavy.frame_count(), 1);
        assert_eq!((heavy.size(0).width.0, heavy.size(0).height.0), (400, 300));
        // Muitos quadros de 1×1 não passam do teto de bytes, mas passam do de quadros.
        assert_eq!(thumbnail(&gif(241, 1, 1)).unwrap().frame_count(), 1);
        assert_eq!(thumbnail(&gif(240, 1, 1)).unwrap().frame_count(), 240);
    }

    #[test]
    fn backdrop_copy_is_validated_before_replacing_the_previous_one() {
        let dir = std::env::temp_dir().join(format!("hangar-backdrop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("background-image");
        let mut png = Vec::new();
        DynamicImage::ImageRgb8(image::RgbImage::new(3000, 1500)).write_to(&mut Cursor::new(&mut png), ImageFormat::Png).unwrap();
        let good = dir.join("good.png");
        std::fs::write(&good, &png).unwrap();
        let image = adopt_backdrop(&good, &dest).unwrap();
        assert_eq!((image.size(0).width.0, image.size(0).height.0), (2560, 1280));
        assert_eq!(std::fs::read(&dest).unwrap(), png);

        let bad = dir.join("bad.png");
        std::fs::write(&bad, b"nao e imagem").unwrap();
        assert_eq!(adopt_backdrop(&bad, &dest).err(), Some("backdrop_invalid"));
        let huge = dir.join("huge.png");
        std::fs::File::create(&huge).unwrap().set_len(BACKDROP_MAX_BYTES + 1).unwrap();
        assert_eq!(adopt_backdrop(&huge, &dest).err(), Some("backdrop_too_big"));
        assert_eq!(adopt_backdrop(&dir.join("sumiu.png"), &dest).err(), Some("backdrop_missing"));
        // Recusas não tocam na cópia boa.
        assert_eq!(std::fs::read(&dest).unwrap(), png);
        assert!(backdrop(&std::fs::read(&dest).unwrap()).is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
