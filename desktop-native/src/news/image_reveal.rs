//! `revelacao-imagem`: a imagem citada brilha enquanto carrega e abre do centro, com borda suave, quando chega.

use std::{cell::{Cell, OnceCell}, sync::Arc, time::{Duration, Instant}};
use gpui_kit::*;
use image::{Frame, Rgba, RgbaImage};
use crate::{app, media, theme};

const CYCLE: f32 = 6.;
/// Até aqui a caixa carrega; depois a imagem abre durante `OPEN` segundos e fica à vista até o ciclo recomeçar.
const ARRIVE: f32 = 1.8;
const OPEN: f32 = 1.4;

/// Ponto da abertura no instante `t` do ciclo: `None` enquanto carrega, 0 a 1 enquanto abre, 1 depois.
fn opening(t: f32) -> Option<f32> {
    (t >= ARRIVE).then(|| ease_in_out(((t - ARRIVE) / OPEN).min(1.)))
}

thread_local! {
    static SAMPLE: OnceCell<(Arc<RenderImage>, Vec<Arc<RenderImage>>)> = const { OnceCell::new() };
    static NEXT_REPAINT: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Imagem de exemplo desenhada aqui (faixas de cor em diagonal), já em BGRA, com os quadros da abertura dela.
fn sample() -> (Arc<RenderImage>, Vec<Arc<RenderImage>>) {
    SAMPLE.with(|cell| cell.get_or_init(|| {
        let (w, h) = (320u32, 200u32);
        let pixels = RgbaImage::from_fn(w, h, |x, y| {
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            let wave = ((u * 5. + v * 3.).sin() * 0.5 + 0.5) * 0.6;
            let (r, g, b) = (0.35 + 0.55 * u, 0.25 + wave * 0.6, 0.85 - 0.5 * v);
            let byte = |c: f32| (c.clamp(0., 1.) * 255.) as u8;
            Rgba([byte(b), byte(g), byte(r), 255])
        });
        let picture = Arc::new(RenderImage::new(vec![Frame::new(pixels)]));
        let frames = media::reveal(&picture);
        (picture, frames)
    }).clone())
}

/// Carregando, a caixa anima sozinha; abrindo, cada quadro conta; parada, só acorda no próximo degrau.
fn schedule_repaint(t: f32, window: &mut Window, cx: &mut App) {
    if (ARRIVE..ARRIVE + OPEN).contains(&t) { window.request_animation_frame(); return; }
    let next = if t < ARRIVE { ARRIVE } else { CYCLE };
    let at = Instant::now() + Duration::from_secs_f32(next - t) + Duration::from_millis(5);
    if NEXT_REPAINT.get().is_some_and(|scheduled| scheduled > Instant::now() && scheduled <= at) { return; }
    NEXT_REPAINT.set(Some(at));
    window.spawn(cx, async move |cx| {
        cx.background_executor().timer(at.saturating_duration_since(Instant::now())).await;
        let _ = cx.update(|window, _| window.refresh());
    }).detach();
}

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: quadro parado no meio da abertura.
    let (cycle, open) = if cx.reduce_motion() { (0, Some(0.45)) } else {
        let elapsed = started.elapsed().as_secs_f32();
        let t = elapsed % CYCLE;
        schedule_repaint(t, window, cx);
        ((elapsed / CYCLE) as u32, opening(t))
    };
    let thumb = match open {
        // O id muda a cada volta para o brilho recomeçar do mesmo ponto.
        None => app::loading_thumb(&format!("news-{cycle}"), cx.reduce_motion()),
        Some(t) => {
            let (picture, frames) = sample();
            if frames.is_empty() { return div().into_any_element(); }
            app::reveal_layers(app::reveal_box(), &picture, &frames, t).into_any_element()
        }
    };
    let stage_w = if width < px(240.) { width } else { px(348.).min(width) };
    div().id("news-image-reveal").w(stage_w).h(px(228.)).p(px(14.)).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .flex().items_start().overflow_hidden().child(thumb).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn loads_then_opens_then_holds() {
        assert_eq!(opening(1.0), None);
        assert_eq!(opening(ARRIVE), Some(0.));
        assert!(opening(ARRIVE + OPEN / 2.).is_some_and(|t| t > 0.3 && t < 0.7));
        assert_eq!(opening(5.0), Some(1.));
    }
}
