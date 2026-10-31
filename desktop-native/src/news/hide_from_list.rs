//! `ocultar-da-lista`: o menu da linha leva a sessão para o grupo "Ocultas" no fim da barra, sem fechar nada.

use std::{cell::Cell, time::{Duration, Instant}};
use gpui_kit::{component::{ActiveTheme, StyledExt}, prelude::FluentBuilder, *};
use crate::{app::{self, chrome, RowSub}, i18n::{tr, tr_shared}, theme};

const CYCLE: f32 = 6.5;
/// O clique direito abre o menu na linha.
const MENU: f32 = 0.8;
/// O ponteiro chega em "Ocultar da lista".
const PICK: f32 = 1.7;
/// O clique: a linha some em `FADE_STEPS` degraus de `FADE_STEP` e o grupo aparece.
const HIDE: f32 = 2.6;
const FADE_STEP: f32 = 0.06;
const FADE_STEPS: u8 = 3;
/// O grupo é aberto e mostra a linha.
const OPEN: f32 = 4.0;

/// A linha que é ocultada, na ordem das amostras.
const TARGET: usize = 1;
/// Posição do "Ocultar da lista" entre os itens do menu.
const HIDE_ITEM: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame {
    menu: bool,
    picked: bool,
    /// Degrau da saída da linha (0 = começando); `None` fora da saída.
    fading: Option<u8>,
    hidden: bool,
    open: bool,
}

fn frame(t: f32) -> Frame {
    let gone = HIDE + FADE_STEPS as f32 * FADE_STEP;
    Frame {
        menu: t >= MENU && t < HIDE,
        picked: t >= PICK && t < HIDE,
        fading: (t >= HIDE && t < gone).then(|| ((t - HIDE) / FADE_STEP).floor() as u8),
        hidden: t >= gone,
        open: t >= OPEN,
    }
}

/// Segundos até o próximo instante em que o quadro muda: o roteiro anda em degraus, não a cada quadro.
fn until_next_step(t: f32) -> f32 {
    let fade = (1..=FADE_STEPS).map(|k| HIDE + k as f32 * FADE_STEP);
    fade.chain([MENU, PICK, HIDE, OPEN, CYCLE]).filter(|b| *b > t).fold(CYCLE, f32::min) - t
}

thread_local! {
    /// O redesenho já marcado: cada render só marca outro se o dele cair antes.
    static NEXT_REPAINT: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Redesenha a janela no próximo degrau. Só é marcado enquanto a animação é desenhada: fechou o modal, para.
fn schedule_repaint(t: f32, window: &mut Window, cx: &mut App) {
    // Margem para o timer não acordar um pouco antes do degrau e desenhar o quadro velho.
    let at = Instant::now() + Duration::from_secs_f32(until_next_step(t)) + Duration::from_millis(5);
    if NEXT_REPAINT.get().is_some_and(|next| next > Instant::now() && next <= at) { return; }
    NEXT_REPAINT.set(Some(at));
    window.spawn(cx, async move |cx| {
        cx.background_executor().timer(at.saturating_duration_since(Instant::now())).await;
        let _ = cx.update(|window, _| window.refresh());
    }).detach();
}

/// Abaixo desta largura (o cartão deitado do "Atualizar?") o menu cobre a lista em vez de ficar ao lado.
const COMPACT_BELOW: f32 = 240.;
/// Altura do palco com o grupo aberto: o cartão não pula quando a linha muda de lugar.
const STAGE_HEIGHT: f32 = 268.;
/// Onde o menu nasce, medido do topo do palco: logo abaixo do nome da linha clicada.
const MENU_TOP: f32 = 72.;

/// Amostra de linha: nome, agente, estado e a segunda linha.
struct Sample { name: String, provider: &'static str, state: &'static str, sub: Option<(String, RowSub)> }

fn samples() -> [Sample; 4] {
    [
        Sample { name: tr_shared("news_rename_sample_name", &[]), provider: "claude", state: "working",
            sub: Some((tr_shared("news_codex_agent_task", &[]), RowSub::Working)) },
        Sample { name: tr_shared("news_tags_sample_session_other", &[]), provider: "codex", state: "idle",
            sub: Some((tr_shared("news_codex_split", &[]), RowSub::Reply)) },
        Sample { name: tr_shared("news_tags_sample_session", &[]), provider: "claude", state: "idle", sub: None },
        Sample { name: tr_shared("news_rename_sample_generic", &[]), provider: "claude", state: "idle", sub: None },
    ]
}

/// A linha como a barra a desenha: marca do agente, nome e a segunda linha.
fn row(i: usize, sample: &Sample, hovered: bool) -> Stateful<Div> {
    let avatar = chrome::session_avatar(&chrome::AvatarLook::new(sample.provider, None, sample.state, false), 30., None);
    let italic = sample.state == "working";
    app::session_row_frame(SharedString::from(format!("news-hide-row-{i}")), false).when(hovered, |el| el.bg(theme::hover()))
        .child(avatar)
        .child(app::session_row_text().child(div().flex().items_center().child(app::session_row_name(sample.name.clone())))
            .when_some(sample.sub.clone(), |el, (text, kind)| el.child(app::session_row_sub(text, kind, theme::muted(), italic))))
}

/// O menu da linha com o visual do `menu_style`; o item escolhido tem o realce do kit.
fn menu(name: String, picked: bool, cx: &App) -> Div {
    let items = [tr("sidebar_rename"), tr("sidebar_mute"), tr("sidebar_hide"), tr_shared("compartilhar_menu", &[])];
    let items = items.into_iter().enumerate().map(|(i, label)| {
        let on = picked && i == HIDE_ITEM;
        div().flex().items_center().h(app::menu_item_height()).text_color(cx.theme().foreground).refine_style(&app::menu_item_look())
            .when(on, |el| el.bg(cx.theme().tokens.accent).text_color(cx.theme().accent_foreground))
            .child(div().truncate().child(label))
    });
    div().refine_style(&app::menu_surface_look())
        .child(div().flex().flex_col().refine_style(&app::menu_list_look()).child(app::menu_label(name)).children(items))
}

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: quadro parado com a linha já dentro de "Ocultas (1)".
    let f = if cx.reduce_motion() { Frame { menu: false, picked: false, fading: None, hidden: true, open: true } } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        schedule_repaint(t, window, cx);
        frame(t)
    };
    let compact = width < px(COMPACT_BELOW);
    let pad = if compact { 6. } else { 14. };
    let list_width = if compact { f32::from(width) - 2. * pad } else { 260. };
    let samples = samples();
    let mut list = div().w(px(list_width)).flex().flex_col().gap(px(2.));
    for (i, sample) in samples.iter().enumerate() {
        if i == TARGET && f.hidden { continue; }
        let el = row(i, sample, i == TARGET && f.menu);
        list = list.child(match f.fading {
            Some(step) if i == TARGET => el.opacity(1. - (step as f32 + 1.) / (FADE_STEPS as f32 + 1.)).into_any_element(),
            _ => el.into_any_element(),
        });
    }
    if f.hidden {
        list = list.child(app::group_header_look(div(), tr("sidebar_hidden_group").replace("{n}", "1"), f.open, None, 0));
        if f.open { list = list.child(row(TARGET + samples.len(), &samples[TARGET], false)); }
    }
    let menu_left = if compact { 18. } else { pad + 150. };
    let menu_width = if compact { list_width - 24. } else { 220. };
    div().id("news-hide-from-list").relative().w(width).h(px(STAGE_HEIGHT)).p(px(pad)).rounded(px(9.)).border_1()
        .border_color(theme::border().alpha(0.06)).overflow_hidden().text_color(theme::text())
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .child(list)
        .when(f.menu, |el| el.child(div().absolute().top(px(MENU_TOP)).left(px(menu_left)).w(px(menu_width))
            .child(menu(samples[TARGET].name.clone(), f.picked, cx))))
        // Miniatura só leitura: nada aqui dentro recebe o mouse.
        .child(div().absolute().top_0().left_0().size_full().occlude())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn script_opens_the_menu_hides_the_row_and_opens_the_group() {
        let none = Frame { menu: false, picked: false, fading: None, hidden: false, open: false };
        assert_eq!(frame(0.3), none);
        assert_eq!(frame(1.0), Frame { menu: true, ..none });
        assert_eq!(frame(2.0), Frame { menu: true, picked: true, ..none });
        assert_eq!(frame(2.63), Frame { fading: Some(0), ..none });
        assert_eq!(frame(3.0), Frame { hidden: true, ..none });
        assert_eq!(frame(4.5), Frame { hidden: true, open: true, ..none });
    }

    #[test]
    fn repaint_waits_for_the_next_step() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.2), 0.6));
        assert!(close(until_next_step(2.0), 0.6));
        assert!(close(until_next_step(2.63), 0.03));
        assert!(close(until_next_step(3.0), 1.0));
        assert!(close(until_next_step(4.5), 2.0));
    }
}
