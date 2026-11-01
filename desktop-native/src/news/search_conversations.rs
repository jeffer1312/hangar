//! `busca-conversas`: o Ctrl+K acha a palavra nas conversas de todos os agentes, agrupa por conversa e mostra o trecho
//! na conversa ao lado.

use std::{cell::Cell, time::{Duration, Instant}};
use gpui_kit::{component::ActiveTheme, prelude::FluentBuilder, *};
use crate::{app::{self, chrome, ScopeFound}, i18n::{tr, tr_shared}, theme};

const CYCLE: f32 = 6.;
/// Cada tecla da palavra buscada.
const KEYS: [f32; 4] = [0.4, 0.7, 1.0, 1.3];
/// As conversas chegam.
const RESULTS: f32 = 1.7;
/// O primeiro trecho fica escolhido e a conversa dele abre ao lado.
const SELECT: f32 = 2.8;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame { typed: usize, results: bool, selected: bool }

/// Movimento reduzido: a palavra digitada, as conversas e o trecho aberto ao lado.
const STILL: Frame = Frame { typed: KEYS.len(), results: true, selected: true };

fn frame(t: f32) -> Frame {
    Frame { typed: KEYS.iter().filter(|k| t >= **k).count(), results: t >= RESULTS, selected: t >= SELECT }
}

/// Segundos até o próximo instante em que o quadro muda: o roteiro anda em degraus, não a cada quadro.
fn until_next_step(t: f32) -> f32 {
    KEYS.into_iter().chain([RESULTS, SELECT, CYCLE]).filter(|b| *b > t).fold(CYCLE, f32::min) - t
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

/// Abaixo desta largura lista e conversa não cabem lado a lado: fica só a lista, como na paleta estreita.
const COMPACT_BELOW: f32 = 440.;
const STAGE_HEIGHT: f32 = 350.;
const COMPACT_HEIGHT: f32 = 230.;
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

/// A conversa do trecho com as linhas do chat: a resposta antes, a mensagem achada no contorno e a resposta depois.
fn preview(sample: &Sample, now: f64) -> Div {
    let account = app::account_chip(Some(&sample.conta)).map(|(label, _)| label);
    let sub = app::search_chat_sub(sample.agent, account, PLACE.into(), sample.live, now - sample.age);
    let open = app::search_open_look("news-search-open".into(), sample.live).tab_stop(false).into_any_element();
    let header = app::search_chat_header(avatar(sample, 28.), sample.title.clone(), sub, vec![open]);
    let row = || div().w_full().px(px(10.)).py(px(6.));
    let reply = |text: String| row().child(app::conversation_text(div().flex().flex_col().gap_2(), false)
        .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::muted()).child(app::search_who(false, sample.agent)))
        .child(text));
    let found = row().flex().flex_col().items_end().child(app::user_bubble(app::conversation_text(div(), true).child(sample.line.clone())));
    div().size_full().flex().flex_col().overflow_hidden().child(header)
        .child(div().flex_1().min_h_0().pt(px(6.)).flex().flex_col().justify_center().overflow_hidden()
            .child(reply(tr("news_search_reply_before")))
            .child(app::search_found_frame(found.into_any_element()))
            .child(reply(tr("news_search_reply_after"))))
}

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    let f = if cx.reduce_motion() { STILL } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        schedule_repaint(t, window, cx);
        frame(t)
    };
    let compact = width < px(COMPACT_BELOW);
    let now = chrono::Local::now().timestamp() as f64;
    let query = tr("news_search_query");
    let typed: String = query.chars().take((query.chars().count() * f.typed).div_ceil(KEYS.len())).collect();
    let words = vec![query.to_lowercase()];
    let caret = div().flex_shrink_0().w(px(1.5)).h(px(17.)).bg(cx.theme().caret);
    let field = app::search_field(div().flex_1().min_w_0().flex().items_center().gap(px(1.)).text_size(px(14.)).overflow_hidden()
        .map(|el| if typed.is_empty() {
            el.child(caret).child(div().truncate().text_color(cx.theme().muted_foreground).child(tr_shared("switcher_buscar_conversas", &[])))
        } else { el.child(div().text_color(theme::text()).child(typed.clone())).child(caret) }));
    let samples = samples();
    let mut list = div().size_full().pt(px(8.)).pl(px(8.)).pr(px(if compact { 8. } else { 14. })).flex().flex_col().gap(px(4.)).overflow_hidden();
    list = if f.results {
        list.children(samples.iter().enumerate().map(|(i, sample)| group(sample, f.selected && i == 0, &words, now)))
    } else {
        let status = tr_shared(if f.typed == 0 { "busca_digite_todas" } else { "switcher_buscando" }, &[]);
        list.child(app::search_status_line(status, theme::muted()))
    };
    let mut card = div().size_full().flex().flex_col().rounded(px(12.)).border_1().border_color(theme::glass_border())
        .bg(theme::popup_fill(theme::raised())).overflow_hidden().text_color(theme::text())
        .child(div().flex_shrink_0().px(px(if compact { 8. } else { 16. })).pt(px(if compact { 8. } else { 14. })).pb(px(8.)).flex().child(field));
    if compact {
        card = card.child(div().flex_1().min_h_0().border_t_1().border_color(theme::border()).child(list));
    } else {
        let found = f.results.then_some(ScopeFound { hits: 2, conversations: 2, many_terms: false, partial: 1 });
        let line = app::search_scope_summary(tr("search_scope_all"), found);
        let scope = if f.results {
            let chips = ["claude", "codex"].map(|agent| app::search_filter_chip(format!("news-search-filter-{agent}"), false, app::search_agent_label(agent), 1))
                .into_iter().chain([true, false].map(|live| app::search_filter_chip(format!("news-search-filter-live-{live}"), false, app::search_live_label(live), 1)))
                .map(|chip| chip.tab_stop(false).into_any_element()).collect();
            app::search_scope_block(line, chips).into_any_element()
        } else { line.into_any_element() };
        let chat = if f.selected { preview(&samples[0], now) }
            else { div().size_full().flex().items_center().justify_center().child(app::search_status_line(tr("search_pick"), theme::faint())) };
        card = card.child(scope).child(div().flex_1().min_h_0().flex().border_t_1().border_color(theme::border())
            .child(div().h_full().min_w_0().w(relative(0.44)).flex_shrink_0().border_r_1().border_color(theme::border()).child(list))
            .child(div().flex_1().min_w_0().h_full().child(chat)));
    }
    let pad = if compact { 6. } else { 12. };
    div().id("news-search-conversations").relative().w(width).h(px(if compact { COMPACT_HEIGHT } else { STAGE_HEIGHT })).p(px(pad))
        .rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06)).overflow_hidden()
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
    fn script_types_the_word_shows_the_conversations_and_opens_the_snippet() {
        assert_eq!(frame(0.2), Frame { typed: 0, results: false, selected: false });
        assert_eq!(frame(0.8), Frame { typed: 2, results: false, selected: false });
        assert_eq!(frame(1.5), Frame { typed: 4, results: false, selected: false });
        assert_eq!(frame(2.0), Frame { typed: 4, results: true, selected: false });
        assert_eq!(frame(3.0), STILL);
        assert_eq!(frame(5.9), STILL);
    }

    #[test]
    fn repaint_waits_for_the_next_step() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.1), 0.3));
        assert!(close(until_next_step(1.3), 0.4));
        assert!(close(until_next_step(2.0), 0.8));
        assert!(close(until_next_step(3.0), 3.0));
    }
}
