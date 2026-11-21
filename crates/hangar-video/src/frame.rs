use std::ffi::c_void;

/// A libmpv pede ponteiro e passo de linha múltiplos de 64 para o caminho vetorizado; abaixo disso ela pode copiar o
/// quadro inteiro de novo.
#[derive(Clone, Copy)]
#[repr(C, align(64))]
struct Block([u8; 64]);

/// Quadro em memória alinhada, reaproveitado entre quadros do mesmo tamanho. A linha tem `width * 4` bytes, sem
/// folga: a largura é sempre múltipla de 16 (ver `render_size`), e quem copia para a textura não precisa de passo.
pub(crate) struct Buffer {
    blocks: Vec<Block>,
    len: usize,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl Buffer {
    pub(crate) fn new() -> Self { Buffer { blocks: Vec::new(), len: 0, width: 0, height: 0 } }

    pub(crate) fn resize(&mut self, width: u32, height: u32) {
        let len = width as usize * 4 * height as usize;
        let blocks = len.div_ceil(64);
        if self.blocks.len() != blocks { self.blocks.resize(blocks, Block([0; 64])); }
        (self.len, self.width, self.height) = (len, width, height);
    }

    pub(crate) fn stride(&self) -> usize { self.width as usize * 4 }

    pub(crate) fn as_mut_ptr(&mut self) -> *mut c_void { self.blocks.as_mut_ptr().cast() }

    pub(crate) fn bytes(&self) -> &[u8] {
        // SAFETY: `blocks` tem pelo menos `len` bytes inicializados (zerados no `resize`).
        unsafe { std::slice::from_raw_parts(self.blocks.as_ptr().cast(), self.len) }
    }

    /// O quarto byte de `rgb0`/`bgr0` é lixo, quase sempre zero; a textura usa alfa, e alfa zero seria invisível.
    pub(crate) fn make_opaque(&mut self) {
        let words = self.blocks.len() * 16;
        // SAFETY: os blocos são alinhados a 64, logo a `u32`, e cobrem `words` palavras.
        let pixels = unsafe { std::slice::from_raw_parts_mut(self.blocks.as_mut_ptr().cast::<u32>(), words) };
        let alpha = u32::from_ne_bytes([0, 0, 0, 255]);
        for pixel in pixels { *pixel |= alpha; }
    }
}

/// O último quadro pronto, emprestado a quem pinta. `pixels` tem `height` linhas de `stride` bytes.
pub struct FrameRef<'a> {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub pixels: &'a [u8],
    /// Cresce a cada quadro: quem pinta compara com o último que copiou.
    pub seq: u64,
}

/// Maior lado que o player desenha; acima disso a GPU amplia.
const MAX_RENDER: (u32, u32) = (3840, 2160);

/// Tamanho do quadro desenhado: o vídeo encaixado em `target` (a área na tela, em pixels físicos) sem ampliar, com a
/// proporção dele (a libmpv poria tarjas pretas se fosse outra) e largura múltipla de 16.
pub(crate) fn render_size(video: (u32, u32), target: (u32, u32)) -> Option<(u32, u32)> {
    let (vw, vh) = (video.0 as f64, video.1 as f64);
    if vw < 1. || vh < 1. { return None; }
    let (tw, th) = (target.0.clamp(16, MAX_RENDER.0) as f64, target.1.clamp(16, MAX_RENDER.1) as f64);
    let scale = (tw / vw).min(th / vh).min(1.);
    let width = ((vw * scale) as u32 / 16 * 16).max(16);
    let height = ((width as f64 * vh / vw).round() as u32).max(1);
    Some((width, height))
}

#[cfg(test)]
mod tests {
    use super::{Buffer, render_size};

    #[test]
    fn render_size_fits_without_enlarging_and_keeps_the_width_aligned() {
        assert_eq!(render_size((1920, 1080), (4000, 4000)), Some((1920, 1080)), "não amplia");
        assert_eq!(render_size((1920, 1080), (960, 1000)), Some((960, 540)));
        let (w, h) = render_size((1170, 2532), (800, 600)).unwrap();
        assert_eq!(w % 16, 0);
        assert!(h <= 600 && (w as f64 / h as f64 - 1170. / 2532.).abs() < 0.02, "{w}x{h}");
        assert_eq!(render_size((0, 1080), (800, 600)), None);
        assert_eq!(render_size((8000, 4000), (9000, 9000)).map(|(w, _)| w <= 3840), Some(true));
    }

    #[test]
    fn buffer_is_aligned_and_opaque() {
        let mut buffer = Buffer::new();
        buffer.resize(16, 3);
        assert_eq!(buffer.as_mut_ptr() as usize % 64, 0);
        assert_eq!(buffer.bytes().len(), 16 * 4 * 3);
        buffer.make_opaque();
        assert!(buffer.bytes().chunks_exact(4).all(|px| px == [0, 0, 0, 255]));
    }
}
