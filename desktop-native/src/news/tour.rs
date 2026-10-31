//! `tour-nativo`: o balão do tour aponta uma aba, o anel pulsa em volta dela e o check comemora a ação feita.

use std::time::Instant;
use gpui_kit::{prelude::FluentBuilder, *};
use crate::{app::{self, chrome}, i18n::{tr, tr_shared}, motion, theme, tour::{self, balloon}};

const CYCLE: f32 = 4.;
/// Abaixo desta largura (o cartão deitado do "Atualizar?") cabe só uma aba e o título do balão.
const COMPACT_BELOW: f32 = 240.;
/// Espaço entre as abas e o balão; a seta ocupa ele.
const GAP: f32 = 14.;
const TAB_GAP: f32 = 8.;

/// A comemoração fecha o ciclo, com a mesma duração da do tour.
fn celebrate_at() -> f32 { CYCLE - tour::CELEBRATION.as_secs_f32() }

/// Quadro no instante `t` do ciclo: a fração da comemoração, ou `None` com o balão esperando.
fn frame(t: f32) -> Option<f32> {
    let at = celebrate_at();
    (t >= at).then(|| ((t - at) / tour::CELEBRATION.as_secs_f32()).min(1.))
}

/// A aba como a barra a desenha, sem foco nem clique.
fn tab(key: &str, provider: &str, on: bool, width: f32) -> Div {
    let avatar = chrome::session_avatar(&chrome::AvatarLook::new(provider, None, "idle", false), 20., None);
    app::tab_frame(on, false, false).w(px(width)).child(avatar)
        .child(app::tab_title(tr_shared(key, &[]), None, false, false, None, None))
}

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: o balão parado apontando a aba, sem pulso.
    let (celebrating, wave) = if cx.reduce_motion() { (None, None) } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        motion::request_frame(window, cx);
        let celebrating = frame(t);
        (celebrating, celebrating.is_none().then(|| motion::pulse_wave((t / motion::PULSE.as_secs_f32()).fract())))
    };
    let compact = width < px(COMPACT_BELOW);
    let pad = if compact { 8. } else { 14. };
    let inner = f32::from(width) - 2. * pad;
    let tab_width = if compact { inner } else { ((inner - 2. * TAB_GAP) / 3.).min(150.) };
    let ring = if celebrating.is_some() { balloon::green() } else { theme::accent() };
    let target = div().relative().child(tab("news_tags_sample_session_other", "codex", true, tab_width))
        .child(canvas(|_, _, _| (), move |b, _, window, _| balloon::paint_spotlight(b, b, 0., ring, wave, window)).absolute().inset_0());
    let tabs = div().flex().gap(px(TAB_GAP)).map(|el| if compact { el.child(target) } else {
        el.child(tab("news_tags_sample_session", "claude", false, tab_width)).child(target)
            .child(tab("news_rename_sample_generic", "claude", false, tab_width))
    });

    // O balão do passo "botão direito", com a seta no meio da aba apontada.
    let center = if compact { inner / 2. } else { tab_width * 1.5 + TAB_GAP };
    let balloon_width = inner.min(300.);
    let left = (center - balloon_width / 2.).clamp(0., inner - balloon_width);
    let arrow_x = (center - left - balloon::ARROW_W / 2.).clamp(16., balloon_width - 16. - balloon::ARROW_W);
    let surface = tour::Surface { kind: tour::Kind::Tour, id: "news-tour".into(), anchor: tour::anchors::SESSION_ROW.into(),
        title: tr("tour_menu_title"), body: tr("tour_menu_body"), progress: Some((2, 9)), celebrating: None };
    // O texto continua lá, invisível, na comemoração: o cartão não muda de altura a cada ciclo.
    let waiting = div().flex().flex_col().gap(px(if compact { 6. } else { 10. }))
        .child(div().text_size(px(if compact { 12. } else { 14. })).font_weight(FontWeight::SEMIBOLD).text_color(balloon::text_color())
            .whitespace_normal().child(surface.title.clone()))
        .when(!compact, |el| el.child(div().text_size(px(13.)).line_height(relative(1.45)).text_color(balloon::muted_color())
            .whitespace_normal().child(surface.body.clone())).child(balloon::waiting(wave)))
        .when(celebrating.is_some(), |el| el.invisible());
    let content = div().flex().flex_col().gap(px(if compact { 6. } else { 10. })).child(balloon::header(&surface))
        .child(div().relative().child(waiting)
            .when_some(celebrating, |el, progress| el.child(balloon::celebration(progress, false).absolute().top_0().left_0())));
    let bubble = div().relative().ml(px(left)).w(px(balloon_width)).p(px(if compact { 10. } else { 16. })).rounded(px(14.))
        .bg(balloon::fill_color()).border_1().border_color(balloon::border_color())
        .child(content)
        .child(balloon::arrow(true).absolute().left(px(arrow_x)).top(px(-balloon::ARROW_H)));

    div().id("news-tour").w(width).p(px(pad)).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .flex().flex_col().gap(px(GAP)).child(tabs).child(bubble).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn balloon_waits_then_celebrates_at_the_end_of_the_cycle() {
        assert_eq!(frame(0.), None);
        assert_eq!(frame(celebrate_at() - 0.01), None);
        assert_eq!(frame(celebrate_at()), Some(0.));
        let half = frame(celebrate_at() + tour::CELEBRATION.as_secs_f32() / 2.).unwrap();
        assert!((half - 0.5).abs() < 1e-3);
        assert!(frame(CYCLE - 0.001).unwrap() <= 1.);
        assert!(celebrate_at() > 2., "o balão espera tempo bastante para ser lido");
    }
}
