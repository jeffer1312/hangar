//! `busca-conversas`: o Ctrl+K acha a palavra nas conversas de todos os agentes, agrupa por conversa e abre a conversa
//! no trecho achado.

use std::{cell::Cell, time::{Duration, Instant}};
use gpui_kit::{component::ActiveTheme, prelude::FluentBuilder, *};
use crate::{app::{self, chrome, ScopeFound}, i18n::{tr, tr_shared}, theme};

const CYCLE: f32 = 7.5;
/// Cada tecla da palavra buscada.
const KEYS: [f32; 4] = [0.4, 0.7, 1.0, 1.3];
/// As conversas chegam.
const RESULTS: f32 = 1.7;
/// O toque no primeiro trecho.
const SELECT: f32 = 3.0;
/// Em uma coluna, a conversa do trecho toma o lugar da lista.
const OPEN: f32 = 3.6;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame { typed: usize, results: bool, selected: bool, opened: bool }

/// Movimento reduzido: a palavra digitada e as conversas achadas, com o trecho escolhido (em duas colunas, aberto ao lado).
const STILL: Frame = Frame { typed: KEYS.len(), results: true, selected: true, opened: false };

fn frame(t: f32) -> Frame {
    Frame { typed: KEYS.iter().filter(|k| t >= **k).count(), results: t >= RESULTS, selected: t >= SELECT, opened: t >= OPEN }
}

/// Segundos até o próximo instante em que o quadro muda: o roteiro anda em degraus, não a cada quadro.
fn until_next_step(t: f32) -> f32 {
    KEYS.into_iter().chain([RESULTS, SELECT, OPEN, CYCLE]).filter(|b| *b > t).fold(CYCLE, f32::min) - t
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

/// Como a cena se arruma na largura que recebe.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Layout {
    /// Cartão deitado do "Atualizar?": o campo e os trechos marcados.
    Tiny,
    /// Cartão em pé e prévia: busca, lista e conversa em sequência, uma por vez.
    Column,
    /// Lista e conversa lado a lado, como a paleta.
    Columns,
}

/// Acima da maior largura de cartão do "O que chegou para você" (780 px do modal menos os respiros): no cartão, a lista
/// fica sempre com a largura toda.
const COLUMNS_FROM: f32 = 760.;
const TINY_BELOW: f32 = 300.;

fn layout(width: f32) -> Layout {
    if width < TINY_BELOW { Layout::Tiny } else if width < COLUMNS_FROM { Layout::Column } else { Layout::Columns }
}

/// Altura mínima por arranjo, a da fase mais alta: o cartão não pula entre as fases, e o que passar dela cresce em vez de
/// ser cortado.
fn min_height(layout: Layout) -> f32 {
    match layout { Layout::Tiny => 128., Layout::Column => 420., Layout::Columns => 400. }
}

const PLACE: &str = "~/Projetos/hangar";

/// Uma conversa achada: a primeira tem a palavra inteira, a segunda só um pedaço dela.
struct Sample { agent: &'static str, title: String, line: String, user: bool, live: bool, conta: String, age: f64 }

fn samples() -> [Sample; 2] {
    [
        Sample { agent: "claude", title: tr("news_search_title_1"), line: tr("news_search_line_1"), user: true, live: true,
            conta: format!("claude:~/.claude-{}", tr("news_search_account")), age: 25. * 60. },
        Sample { agent: "codex", title: tr("news_search_title_2"), line: tr("news_search_line_2"), user: false, live: false,
            conta: "codex:~/.codex".into(), age: 3. * 86_400. },
    ]
}

/// O avatar da busca: a aberta com o anel de aberta agora.
fn avatar(sample: &Sample, side: f32) -> Div {
    let look = chrome::AvatarLook::new(sample.agent, None, if sample.live { "idle" } else { "" }, false);
    chrome::session_avatar(&if sample.live { look.ring(theme::success()) } else { look }, side, None)
}

fn group(sample: &Sample, selected: bool, words: &[String], now: f64) -> Div {
    let at = now - sample.age;
    let header = app::search_group_header(avatar(sample, 22.), sample.title.clone(), sample.live, app::account_chip(Some(&sample.conta)),
        app::search_group_sub(sample.agent, None, PLACE.into(), at));
    app::search_group_frame(selected).child(header)
        .child(app::search_hit_line(selected, app::search_who(sample.user, sample.agent), at, sample.line.clone(), words, false))
}

/// O trecho no cartão deitado: a marca do agente e a mensagem com a palavra marcada.
fn tiny_line(sample: &Sample, selected: bool, words: &[String]) -> Div {
    let (color, _) = theme::provider(sample.agent);
    let logo = chrome::provider_logo(sample.agent, 11., if sample.agent == "codex" { theme::text() } else { color });
    div().px(px(6.)).py(px(4.)).rounded(px(6.)).flex().items_start().gap(px(5.)).when(selected, |el| el.bg(theme::accent_dim()))
        .children(logo.map(|logo| div().flex_shrink_0().pt(px(2.)).child(logo)))
        .child(div().flex_1().min_w_0().text_size(px(11.)).line_height(relative(1.4)).text_color(theme::text()).line_clamp(2).text_ellipsis()
            .child(app::search_highlighted(sample.line.clone(), words)))
}

/// A conversa do trecho com as linhas do chat: a resposta antes, a mensagem achada no contorno e a resposta depois.
fn preview(sample: &Sample, now: f64) -> Div {
    let account = app::account_chip(Some(&sample.conta)).map(|(label, _)| label);
    let sub = app::search_chat_sub(sample.agent, account, PLACE.into(), sample.live, now - sample.age);
    let open = app::search_open_look("news-search-open".into(), sample.live).tab_stop(false).into_any_element();
    let header = app::search_chat_header(avatar(sample, 28.), sample.title.clone(), sub, vec![open]);
    let row = || div().w_full().px(px(10.)).py(px(6.));
    let reply = |text: String| row().child(app::conversation_text(div().flex().flex_col().gap_2().whitespace_normal(), false)
        .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::muted()).child(app::search_who(false, sample.agent)))
        .child(text));
    let found = row().flex().flex_col().items_end()
        .child(app::user_bubble(app::conversation_text(div().whitespace_normal(), true).child(sample.line.clone())));
    div().w_full().flex().flex_col().child(header)
        .child(div().w_full().pt(px(6.)).pb(px(8.)).flex().flex_col()
            .child(reply(tr("news_search_reply_before")))
            .child(app::search_found_frame(found.into_any_element()))
            .child(reply(tr("news_search_reply_after"))))
}

/// O campo com o que já foi digitado; vazio, o cursor antes do texto de exemplo, como o `Input`.
fn field(typed: &str, cx: &App) -> Div {
    let caret = div().flex_shrink_0().w(px(1.5)).h(px(17.)).bg(cx.theme().caret);
    app::search_field(div().flex_1().min_w_0().flex().items_center().gap(px(1.)).text_size(px(14.)).overflow_hidden()
        .map(|el| if typed.is_empty() {
            el.child(caret).child(div().min_w_0().truncate().text_color(cx.theme().muted_foreground).child(tr_shared("switcher_buscar_conversas", &[])))
        } else { el.child(div().text_color(theme::text()).child(typed.to_owned())).child(caret) }))
}

/// A linha de escopo e, com resultado, os filtros por agente e por estado.
fn scope(results: bool) -> AnyElement {
    let line = app::search_scope_summary(tr("search_scope_all"), results.then_some(ScopeFound { hits: 2, conversations: 2, many_terms: false, partial: 1 }));
    if !results { return line.into_any_element(); }
    let chips = ["claude", "codex"].map(|agent| app::search_filter_chip(format!("news-search-filter-{agent}"), false, app::search_agent_label(agent), 1))
        .into_iter().chain([true, false].map(|live| app::search_filter_chip(format!("news-search-filter-live-{live}"), false, app::search_live_label(live), 1)))
        .map(|chip| chip.tab_stop(false).into_any_element()).collect();
    app::search_scope_block(line, chips).into_any_element()
}

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    let f = if cx.reduce_motion() { STILL } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        schedule_repaint(t, window, cx);
        frame(t)
    };
    let layout = layout(f32::from(width));
    let now = chrono::Local::now().timestamp() as f64;
    let query = tr("news_search_query");
    let typed: String = query.chars().take((query.chars().count() * f.typed).div_ceil(KEYS.len())).collect();
    let words = vec![query.to_lowercase()];
    let samples = samples();
    let tiny = layout == Layout::Tiny;
    let list = || {
        let list = div().w_full().p(px(if tiny { 4. } else { 8. })).flex().flex_col().gap(px(4.));
        if f.results {
            list.children(samples.iter().enumerate().map(|(i, sample)| {
                let on = f.selected && i == 0;
                if tiny { tiny_line(sample, on, &words) } else { group(sample, on, &words, now) }
            }))
        } else if tiny { list } else {
            list.child(app::search_status_line(tr_shared(if f.typed == 0 { "busca_digite_todas" } else { "switcher_buscando" }, &[]), theme::muted()))
        }
    };
    let head = div().flex_shrink_0().px(px(if tiny { 6. } else { 16. })).pt(px(if tiny { 6. } else { 14. })).pb(px(if tiny { 4. } else { 8. }))
        .flex().child(field(&typed, cx));
    let mut card = div().flex_1().w_full().flex().flex_col().rounded(px(12.)).border_1().border_color(theme::glass_border())
        .bg(theme::popup_fill(theme::raised())).overflow_hidden().text_color(theme::text());
    card = match layout {
        Layout::Tiny => card.child(head).child(list()),
        // Uma coisa por vez: a busca com a lista e, depois do toque, a conversa no cartão inteiro.
        Layout::Column if f.opened => card.child(preview(&samples[0], now)),
        Layout::Column => card.child(head).child(scope(f.results)).child(div().w_full().border_t_1().border_color(theme::border()).child(list())),
        Layout::Columns => {
            let chat = if f.selected { preview(&samples[0], now) }
                else { div().size_full().flex().items_center().child(app::search_status_line(tr("search_pick"), theme::faint())) };
            card.child(head).child(scope(f.results)).child(div().flex_1().w_full().flex().border_t_1().border_color(theme::border())
                .child(div().min_w_0().w(relative(0.44)).flex_shrink_0().border_r_1().border_color(theme::border()).child(list()))
                .child(div().flex_1().min_w_0().child(chat)))
        }
    };
    div().id("news-search-conversations").relative().w(width).min_h(px(min_height(layout))).flex().flex_col().p(px(if tiny { 4. } else { 12. }))
        .rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .child(card)
        // Miniatura só leitura: nada aqui dentro recebe o mouse.
        .child(div().absolute().top_0().left_0().size_full().block_mouse_except_scroll())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn script_types_the_word_shows_the_conversations_then_opens_the_snippet() {
        let none = Frame { typed: 0, results: false, selected: false, opened: false };
        assert_eq!(frame(0.2), none);
        assert_eq!(frame(0.8), Frame { typed: 2, ..none });
        assert_eq!(frame(1.5), Frame { typed: 4, ..none });
        assert_eq!(frame(2.0), Frame { typed: 4, results: true, ..none });
        assert_eq!(frame(3.2), STILL);
        assert_eq!(frame(4.0), Frame { opened: true, ..STILL });
        assert_eq!(frame(7.4), Frame { opened: true, ..STILL });
    }

    #[test]
    fn repaint_waits_for_the_next_step() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.1), 0.3));
        assert!(close(until_next_step(1.3), 0.4));
        assert!(close(until_next_step(2.0), 1.0));
        assert!(close(until_next_step(3.2), 0.4));
        assert!(close(until_next_step(4.0), 3.5));
    }

    /// As larguras que a animação recebe: o cartão deitado do "Atualizar?" (160), o diálogo de prévia (520) e o cartão do
    /// "O que chegou para você" (até 716, o modal de 780 menos os respiros). Em nenhuma delas lista e conversa dividem a
    /// largura.
    #[test]
    fn every_card_width_is_one_column() {
        assert_eq!(layout(160.), Layout::Tiny);
        for width in [300., 520., 640., 690., 716.] { assert_eq!(layout(width), Layout::Column, "{width}"); }
        assert_eq!(layout(800.), Layout::Columns);
    }
}
