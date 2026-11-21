//! `galeria-da-conversa`: o visor anda pela mídia da conversa inteira na faixa de baixo, com "Mostrar na conversa", e
//! a aba Mídia mostra tudo em grade, filtrado por quem mandou.

use std::{cell::OnceCell, sync::Arc, time::Instant};
use gpui_kit::{prelude::FluentBuilder, *};
use image::{Frame, Rgba, RgbaImage};
use crate::{app, i18n::{tr, tr_shared}, theme};

const CYCLE: f32 = 6.;
/// Quando o visor passa para a mídia seguinte.
const STEPS: [f32; 3] = [0.9, 1.8, 2.7];
/// Daqui em diante a aba Mídia; depois de `FILTER`, só o que o agente mandou.
const GRID: f32 = 3.5;
const FILTER: f32 = 4.8;
/// Abaixo desta largura (o cartão deitado do "Atualizar?") fica só a faixa.
const COMPACT_BELOW: f32 = 240.;
const PAD: f32 = 14.;
const STAGE_H: f32 = 170.;
/// Altura do palco: as duas cenas cabem nela, e a troca não mexe no tamanho do cartão.
const SCENE_H: f32 = 330.;
/// A grade não passa disto: com o cartão largo, duas linhas ainda cabem no palco.
const GRID_MAX_W: f32 = 420.;
const COMPACT_THUMB: (f32, f32) = (44., 30.);

/// Nome (chave), vídeo, do agente e as duas cores da imagem de exemplo.
const SAMPLES: [(&str, bool, bool, u32, u32); 6] = [
    ("news_gallery_sample_1", false, true, 0x5b6cff, 0x3fb6a8),
    ("news_gallery_sample_2", false, false, 0xf2a65a, 0xe5484d),
    ("news_gallery_sample_3", false, true, 0x30a46c, 0x8bd5c8),
    ("news_gallery_sample_4", true, true, 0x8e6cf0, 0x3e63dd),
    ("news_gallery_sample_5", false, false, 0xf76b15, 0xffd166),
    ("news_gallery_sample_6", false, true, 0x0f766e, 0x93c5fd),
];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Scene { Viewer(usize), Media { agent_only: bool } }

/// Quadro do roteiro no instante `t` do ciclo.
fn frame(t: f32) -> Scene {
    if t >= GRID { Scene::Media { agent_only: t >= FILTER } } else { Scene::Viewer(STEPS.iter().filter(|s| t >= **s).count()) }
}

/// Segundos até o próximo degrau do roteiro.
fn until_next_step(t: f32) -> f32 {
    STEPS.iter().chain(&[GRID, FILTER, CYCLE]).copied().filter(|b| *b > t).fold(CYCLE, f32::min) - t
}

thread_local! {
    static PICTURES: OnceCell<Vec<Arc<RenderImage>>> = const { OnceCell::new() };
}

/// Imagens de exemplo desenhadas aqui, já em BGRA: degradê em diagonal com um cartão claro no meio, como um print.
fn pictures() -> Vec<Arc<RenderImage>> {
    PICTURES.with(|cell| cell.get_or_init(|| SAMPLES.iter().map(|&(_, _, _, from, to)| {
        let (w, h) = (320u32, 180u32);
        let channel = |color: u32, shift: u32| ((color >> shift) & 0xff) as f32 / 255.;
        let pixels = RgbaImage::from_fn(w, h, |x, y| {
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            let k = (u * 0.6 + v * 0.4).clamp(0., 1.);
            let mix = |shift| channel(from, shift) * (1. - k) + channel(to, shift) * k;
            let card = (0.18..0.82).contains(&u) && (0.22..0.72).contains(&v);
            let lift = |c: f32| if card { c + (1. - c) * 0.35 } else { c };
            let byte = |c: f32| (c.clamp(0., 1.) * 255.) as u8;
            Rgba([byte(lift(mix(0))), byte(lift(mix(8))), byte(lift(mix(16))), 255])
        });
        Arc::new(RenderImage::new(vec![Frame::new(pixels)]))
    }).collect()).clone())
}

fn origin(agent: bool, short: bool) -> String {
    tr(match (agent, short) {
        (true, true) => "media_origin_agent_short",
        (false, true) => "media_origin_user_short",
        (true, false) => "media_origin_agent",
        (false, false) => "media_origin_user",
    })
}

fn counter(index: usize) -> String {
    tr("media_counter").replace("{n}", &(index + 1).to_string()).replace("{total}", &SAMPLES.len().to_string())
}

/// A faixa do visor com as vizinhas de `index` que cabem em `inner`.
fn strip(inner: f32, thumb: (f32, f32), index: usize, pictures: &[Arc<RenderImage>]) -> Div {
    let (start, end) = app::strip_window(inner, thumb.0, SAMPLES.len(), index);
    let row = app::strip_row("news-gallery-strip").when(start == 0, |el| el.child(app::strip_day(tr("media_day_today"))))
        .children((start..end).map(|i| app::strip_thumb(SharedString::from(format!("news-gallery-strip-{i}")), thumb,
            Some(pictures[i].clone()), i == index, SAMPLES[i].1)));
    app::strip_frame(row)
}

fn viewer(inner: f32, index: usize, pictures: &[Arc<RenderImage>]) -> AnyElement {
    let (key, video, agent, ..) = SAMPLES[index];
    let detail = format!("{} 11:42 · {}", tr("media_day_today"), origin(agent, false));
    // Sem clique e fora do Tab: miniatura só de leitura.
    let header = div().flex().items_center().gap_2()
        .child(app::viewer_title(tr_shared(key, &[]), detail)).child(app::viewer_counter(counter(index)))
        .child(app::show_in_chat_button().tab_stop(false));
    let stage = app::viewer_stage((STAGE_H * 16. / 9.).min(inner), STAGE_H)
        .child(img(pictures[index].clone()).size_full().object_fit(ObjectFit::Contain))
        .when(video, |el| el.child(div().absolute().inset_0().flex().items_center().justify_center().child(app::play_badge(56., 24.))));
    div().flex().flex_col().gap_3()
        .child(header).child(div().flex().justify_center().child(stage)).child(strip(inner, app::STRIP_THUMB, index, pictures))
        .into_any_element()
}

fn media(inner: f32, agent_only: bool, pictures: &[Arc<RenderImage>]) -> AnyElement {
    let kept: Vec<usize> = (0..SAMPLES.len()).rev().filter(|&i| !agent_only || SAMPLES[i].2).collect();
    // Arquivos e áudio do exemplo: dois arquivos do agente e o áudio do ditado, que é seu.
    let counts = [kept.len(), 2, if agent_only { 0 } else { 1 }];
    let chips = app::media_filter_row().children((0..3).map(|i| app::media_filter_chip(i, i == usize::from(agent_only)).tab_stop(false)));
    let grid_w = inner.min(GRID_MAX_W);
    let side = app::media_cell_side(grid_w);
    let rows = kept.chunks(4).map(|row| app::media_grid_row(row.iter().map(|&i| {
        let (key, video, agent, ..) = SAMPLES[i];
        app::media_cell(SharedString::from(format!("news-gallery-cell-{i}")), side, Some(pictures[i].clone()), video,
            tr_shared(key, &[]), Some(origin(agent, true))).into_any_element()
    }).collect()));
    div().flex().flex_col()
        .child(app::media_section_tabs(counts, 0))
        .child(div().pt(px(10.)).pb(px(4.)).child(chips))
        .child(div().w(px(grid_w)).child(app::media_day_title(tr("media_group_today"))).children(rows))
        .into_any_element()
}

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: o visor parado na terceira mídia, com a faixa e o "Mostrar na conversa" à vista.
    let scene = if cx.reduce_motion() { Scene::Viewer(2) } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t), window, cx);
        frame(t)
    };
    let pictures = pictures();
    let stage = div().id("news-conversation-gallery").w(width).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .overflow_hidden().text_color(theme::text());
    if width < px(COMPACT_BELOW) {
        // Só a faixa: na grade a mídia seguinte fica parada na última que o visor mostrou.
        let index = match scene { Scene::Viewer(index) => index, Scene::Media { .. } => STEPS.len() };
        let inner = f32::from(width) - 16. - 2.;
        return stage.p(px(8.)).flex().flex_col().gap_1()
            .child(strip(inner, COMPACT_THUMB, index, &pictures))
            .child(app::viewer_counter(counter(index)))
            .into_any_element();
    }
    let inner = f32::from(width) - PAD * 2. - 2.;
    let body = match scene {
        Scene::Viewer(index) => viewer(inner, index, &pictures),
        Scene::Media { agent_only } => media(inner, agent_only, &pictures),
    };
    stage.h(px(SCENE_H + PAD * 2.)).p(px(PAD)).child(body).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn script_walks_the_viewer_then_filters_the_media_tab() {
        assert_eq!(frame(0.2), Scene::Viewer(0));
        assert_eq!(frame(1.0), Scene::Viewer(1));
        assert_eq!(frame(2.8), Scene::Viewer(3));
        assert_eq!(frame(3.6), Scene::Media { agent_only: false });
        assert_eq!(frame(5.0), Scene::Media { agent_only: true });
    }

    #[test]
    fn repaint_waits_for_the_next_step() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.), 0.9));
        assert!(close(until_next_step(3.0), 0.5));
        assert!(close(until_next_step(4.0), 0.8));
        assert!(close(until_next_step(5.5), 0.5));
    }

    #[test]
    fn samples_have_a_video_and_both_senders() {
        assert!(SAMPLES.iter().any(|s| s.1));
        assert!(SAMPLES.iter().any(|s| s.2) && SAMPLES.iter().any(|s| !s.2));
    }

    #[test]
    fn the_shown_media_is_always_inside_the_strip() {
        for inner in [202., 490.] {
            for index in 0..SAMPLES.len() {
                let (start, end) = app::strip_window(inner, COMPACT_THUMB.0, SAMPLES.len(), index);
                assert!((start..end).contains(&index), "{inner}: {index} fora de {start}..{end}");
            }
        }
    }
}
