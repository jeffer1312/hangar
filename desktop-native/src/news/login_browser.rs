//! `login-navegador`: o login do Claude escolhe a máquina, depois o perfil, e autoriza vendo e clicando na janela
//! daquele navegador, sem copiar código.

use std::{cell::Cell, time::{Duration, Instant}};
use gpui_kit::{assets::IconName, base::Selectable, component::{Sizable, button::*}, *};
use crate::{app::{self, chrome}, i18n::tr, theme};

const CYCLE: f32 = 8.;
const PROFILE: f32 = 1.8;
const PAGE: f32 = 3.4;
/// O botão da página só liga um tempo depois de a janela ganhar foco, como na página de verdade.
const READY: f32 = 4.4;
const DONE: f32 = 5.8;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Step { Machine, Profile, Page, Ready, Done }

fn frame(t: f32) -> Step {
    if t >= DONE { Step::Done } else if t >= READY { Step::Ready } else if t >= PAGE { Step::Page } else if t >= PROFILE { Step::Profile } else { Step::Machine }
}

/// Segundos até o próximo instante em que o quadro muda: o roteiro anda em degraus, não a cada quadro.
fn until_next_step(t: f32) -> f32 {
    [PROFILE, PAGE, READY, DONE, CYCLE].into_iter().filter(|b| *b > t).fold(CYCLE, f32::min) - t
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

/// A página de autorização no lugar da imagem da janela: o botão acende quando a página o libera.
fn page(ready: bool) -> Div {
    let authorize = div().w(px(160.)).py(px(6.)).rounded(px(7.)).flex().justify_center().text_size(px(12.5)).font_weight(FontWeight::MEDIUM)
        .bg(if ready { theme::text() } else { theme::text().alpha(0.1) })
        .text_color(if ready { theme::background() } else { theme::muted() })
        .child(tr("news_login_browser_authorize"));
    div().flex_1().min_h_0().rounded(px(7.)).border_1().border_color(theme::border()).bg(theme::inset())
        .flex().flex_col().items_center().justify_center().gap(px(10.))
        .child(div().text_size(px(13.)).text_color(theme::text()).child(tr("news_login_browser_page"))).child(authorize)
}

/// Abaixo desta largura (o cartão deitado do "Atualizar?") a descrição não cabe: ficam só o título e a escolha.
const COMPACT_BELOW: f32 = 240.;
/// Altura do passo mais alto: o cartão não pula entre os passos.
const HEIGHT: f32 = 214.;
const COMPACT_HEIGHT: f32 = 124.;

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: quadro parado na escolha da máquina, que é o que muda para quem já conhece o login.
    let step = if cx.reduce_motion() { Step::Machine } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        schedule_repaint(t, window, cx);
        frame(t)
    };
    let compact = width < px(COMPACT_BELOW);
    let laptop = tr("news_login_browser_laptop");
    let row = |id: &'static str, name: String, detail: String, picked: bool| {
        app::target_row(id, name, if compact { String::new() } else { detail }, false).selected(picked).tab_stop(false)
    };
    let heading = |title: String, help: String| {
        if compact { div().text_size(px(11.)).font_weight(FontWeight::MEDIUM).truncate().child(title) } else { app::login_heading(title, help) }
    };
    let body = match step {
        Step::Machine => div().flex().flex_col().gap(px(8.))
            .child(heading(tr("accounts_login_where_title"), tr("accounts_login_where_help")))
            .child(row("news-login-laptop", laptop.clone(), tr("news_login_browser_profile"), true))
            .child(row("news-login-home", tr("news_login_browser_home"), tr("accounts_login_where_default"), false)),
        Step::Profile => div().flex().flex_col().gap(px(8.))
            .child(heading(tr("accounts_login_where_profile_title").replace("{machine}", &laptop), tr("accounts_login_where_profile_help")))
            .child(row("news-login-profile", tr("news_login_browser_profile"), tr("news_login_browser_email"), true))
            .child(row("news-login-default", tr("accounts_login_where_default"), String::new(), false)),
        Step::Page | Step::Ready => {
            let toolbar = div().flex().items_center().gap(px(8.))
                .child(Button::new("news-login-back").outline().xsmall().icon(IconName::ArrowLeft).label(tr("accounts_login_view_back")).tab_stop(false))
                .child(div().flex_1().min_w_0().truncate().text_size(px(12.)).text_color(theme::muted()).child(tr("accounts_login_browser_open").replace("{machine}", &laptop)));
            if compact {
                div().size_full().flex().flex_col().child(page(step == Step::Ready))
            } else {
                div().size_full().flex().flex_col().gap(px(8.)).child(toolbar).child(page(step == Step::Ready))
            }
        }
        Step::Done => div().size_full().flex().flex_col().justify_center().gap(px(8.)).text_size(px(13.))
            .child(div().flex().items_center().gap(px(8.)).font_weight(FontWeight::MEDIUM)
                .child(chrome::small_icon(IconName::Check, 15., theme::success())).child(tr("accounts_login_connected")))
            .child(div().truncate().text_color(theme::muted()).child(tr("news_login_browser_email"))),
    };
    // Miniatura só leitura: a tampa por cima não deixa o mouse levar o foco para os botões.
    div().id("news-login-browser").relative().w(width).h(px(if compact { COMPACT_HEIGHT } else { HEIGHT })).p(px(if compact { 8. } else { 14. }))
        .rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06)).overflow_hidden().text_color(theme::text())
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .child(body).child(div().absolute().top_0().left_0().size_full().occlude()).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn script_goes_machine_profile_page_ready_done() {
        assert_eq!(frame(0.5), Step::Machine);
        assert_eq!(frame(2.), Step::Profile);
        assert_eq!(frame(3.5), Step::Page);
        assert_eq!(frame(5.), Step::Ready);
        assert_eq!(frame(7.), Step::Done);
    }

    #[test]
    fn repaint_waits_for_the_next_step() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.8), 1.));
        assert!(close(until_next_step(6.), 2.));
    }
}
