//! O quadro do player na tela. Linux e Windows: uma textura do device da GPUI, reaproveitada enquanto o tamanho não
//! muda, pintada com `paint_surface` como a página da conversa. macOS: o `paint_surface` de lá só aceita
//! `CVPixelBuffer`, então cada quadro vira uma `RenderImage` e a anterior sai do atlas.
use gpui_kit::*;
use hangar_video::Player;

pub(super) struct Surface {
    /// Número do último quadro copiado; 0 faz o próximo desenho copiar o que estiver pronto.
    seq: u64,
    size: Option<(u32, u32)>,
    shown: Option<platform::Shown>,
}

impl Surface {
    pub(super) fn new() -> Self { Surface { seq: 0, size: None, shown: None } }

    pub(super) fn seq(&self) -> u64 { self.seq }

    /// Copia o quadro novo, se houver, e pinta o último centrado em `bounds`, na proporção dele.
    pub(super) fn paint(&mut self, player: &Player, bounds: Bounds<Pixels>, window: &mut Window) {
        if self.shown.as_ref().is_some_and(|shown| !platform::current(shown)) {
            // Device perdido: a textura nova sai do quadro que o player ainda guarda, sem esperar o próximo.
            self.shown = None;
            self.seq = 0;
        }
        let uploaded = player.with_frame(self.seq, |frame| (frame.seq, (frame.width, frame.height), platform::upload(&mut self.shown, &frame, window)));
        if let Some((seq, size, result)) = uploaded {
            match result {
                Ok(()) => { self.seq = seq; self.size = Some(size); }
                Err(error) => crate::log_line(&format!("video: quadro não foi para a textura: {error}")),
            }
        }
        let (Some(shown), Some(size)) = (self.shown.as_ref(), self.size) else { return };
        platform::paint(shown, fit(bounds, size), window);
    }

    /// Solta a textura (macOS: tira a imagem do atlas, que não a solta sozinho).
    pub(super) fn release(&mut self, window: Option<&mut Window>, cx: &mut App) {
        if let Some(shown) = self.shown.take() { platform::release(shown, window, cx); }
        self.seq = 0;
        self.size = None;
    }
}

/// O quadro encaixado e centrado em `bounds`, sem passar dele.
fn fit(bounds: Bounds<Pixels>, (width, height): (u32, u32)) -> Bounds<Pixels> {
    let (bw, bh) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let (fw, fh) = (width.max(1) as f32, height.max(1) as f32);
    let scale = (bw / fw).min(bh / fh);
    let (w, h) = (fw * scale, fh * scale);
    Bounds { origin: point(bounds.origin.x + px((bw - w) / 2.), bounds.origin.y + px((bh - h) / 2.)), size: size(px(w), px(h)) }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::sync::Arc;
    use gpui_kit::{App, Bounds, Pixels, Window};
    use gpui_wgpu::wgpu;
    use hangar_video::FrameRef;

    pub(super) struct Shown { texture: wgpu::Texture, device: wgpu::Device }

    pub(super) fn current(shown: &Shown) -> bool {
        gpui_wgpu::WgpuContext::shared_device().is_some_and(|(device, _)| device == shown.device)
    }

    pub(super) fn upload(shown: &mut Option<Shown>, frame: &FrameRef<'_>, _: &mut Window) -> Result<(), String> {
        let (device, queue) = gpui_wgpu::WgpuContext::shared_device().ok_or("a GPUI não expôs o device wgpu")?;
        let (width, height) = (frame.width, frame.height);
        let reuse = shown.as_ref().filter(|s| s.device == device && s.texture.width() == width && s.texture.height() == height);
        let texture = match reuse {
            Some(s) => s.texture.clone(),
            None => device.create_texture(&wgpu::TextureDescriptor {
                label: Some("video"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            }),
        };
        // A fila do wgpu ordena a escrita depois do último desenho que ainda lê a textura.
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            frame.pixels,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(frame.stride as u32), rows_per_image: Some(height) },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        *shown = Some(Shown { texture, device });
        Ok(())
    }

    pub(super) fn paint(shown: &Shown, rect: Bounds<Pixels>, window: &mut Window) { window.paint_surface(rect, Arc::new(shown.texture.clone())); }

    pub(super) fn release(_: Shown, _: Option<&mut Window>, _: &mut App) {}
}

#[cfg(target_os = "windows")]
mod platform {
    use std::sync::Arc;
    use gpui_kit::{App, Bounds, Pixels, Window};
    use gpui_windows::SurfaceTexture;
    use hangar_video::FrameRef;

    pub(super) type Shown = Arc<SurfaceTexture>;

    pub(super) fn current(shown: &Shown) -> bool { shown.is_current() }

    /// Na thread da interface: a escrita passa pelo contexto imediato do D3D11, que é dela.
    pub(super) fn upload(shown: &mut Option<Shown>, frame: &FrameRef<'_>, _: &mut Window) -> Result<(), String> {
        let reuse = shown.as_ref().filter(|t| t.is_current() && t.width() == frame.width && t.height() == frame.height).cloned();
        let texture = match reuse {
            Some(texture) => texture,
            None => Arc::new(SurfaceTexture::new(frame.width, frame.height).map_err(|e| e.to_string())?),
        };
        // A linha do quadro tem `width * 4` bytes, o passo que o `write` supõe.
        texture.write(frame.pixels).map_err(|e| e.to_string())?;
        *shown = Some(texture);
        Ok(())
    }

    pub(super) fn paint(shown: &Shown, rect: Bounds<Pixels>, window: &mut Window) { window.paint_surface(rect, shown.clone()); }

    pub(super) fn release(_: Shown, _: Option<&mut Window>, _: &mut App) {}
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod platform {
    use std::sync::Arc;
    use gpui_kit::{App, Bounds, Corners, Pixels, RenderImage, Window};
    use hangar_video::FrameRef;

    pub(super) type Shown = Arc<RenderImage>;

    pub(super) fn current(_: &Shown) -> bool { true }

    /// O player entrega BGRA aqui (`video::FORMAT`), que é o que a GPUI desenha de uma `RenderImage`.
    pub(super) fn upload(shown: &mut Option<Shown>, frame: &FrameRef<'_>, window: &mut Window) -> Result<(), String> {
        let bytes = frame.width as usize * frame.height as usize * 4;
        let pixels = frame.pixels.get(..bytes).ok_or("quadro menor que o tamanho dele")?.to_vec();
        let buffer = image::RgbaImage::from_raw(frame.width, frame.height, pixels).ok_or("quadro sem o tamanho declarado")?;
        let image = Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
        // A anterior já foi desenhada no quadro passado e não entra neste.
        if let Some(old) = shown.replace(image) { let _ = window.drop_image(old); }
        Ok(())
    }

    pub(super) fn paint(shown: &Shown, rect: Bounds<Pixels>, window: &mut Window) {
        let _ = window.paint_image(rect, rect, Corners::default(), shown.clone(), 0, false);
    }

    pub(super) fn release(shown: Shown, window: Option<&mut Window>, cx: &mut App) { cx.drop_image(shown, window); }
}

#[cfg(test)]
mod tests {
    use super::fit;
    use gpui_kit::{Bounds, point, px, size};

    #[test]
    fn fit_centers_and_keeps_the_frame_proportion() {
        let stage = Bounds { origin: point(px(10.), px(20.)), size: size(px(800.), px(450.)) };
        let wide = fit(stage, (1920, 1080));
        assert_eq!((wide.origin, wide.size), (point(px(10.), px(20.)), size(px(800.), px(450.))));
        let tall = fit(stage, (1080, 1920));
        assert_eq!(tall.size.height, px(450.));
        assert!((f32::from(tall.size.width) - 253.125).abs() < 0.01);
        assert!((f32::from(tall.origin.x) - (10. + (800. - 253.125) / 2.)).abs() < 0.01);
    }
}
