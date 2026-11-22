//! `entrar-pelo-connect`: a conta se digita no "Entrar no Hangar", a senha entra, o Entrar é tocado e a lista "Suas
//! máquinas" aparece com as ligadas já marcadas. Mesmo roteiro da animação do celular.

use std::time::Instant;
use gpui_kit::{component::ActiveTheme, prelude::FluentBuilder, *};
use hangar_connect_client::ConnectMachine;
use crate::{app, i18n::{tr, tr_shared}, theme};

const CYCLE: f32 = 6.;
const TYPE_START: f32 = 0.5;
/// Entre uma letra e a seguinte da conta.
const TYPE_GAP: f32 = 0.18;
const PASSWORD: f32 = 1.5;
const PRESS: f32 = 2.2;
const LIST: f32 = 3.;
/// Cada máquina chega um pouco depois da anterior.
const ROW_GAP: f32 = 0.12;
const MACHINES: usize = 3;
/// Movimento reduzido: a lista com as ligadas marcadas, que é o que a novidade promete.
const STILL: f32 = 4.5;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame { typed: usize, password: bool, pressed: bool, list: bool, rows: usize }

/// O quadro no instante `t` do ciclo, para uma conta de `letters` letras.
fn frame(t: f32, letters: usize) -> Frame {
    let typed = (0..letters).filter(|i| t >= TYPE_START + *i as f32 * TYPE_GAP).count();
    let rows = (0..MACHINES).filter(|i| t >= LIST + *i as f32 * ROW_GAP).count();
    Frame { typed, password: t >= PASSWORD, pressed: t >= PRESS, list: t >= LIST, rows }
}

/// Segundos até o próximo instante em que o quadro muda: o roteiro anda em degraus, não a cada quadro.
fn until_next_step(t: f32, letters: usize) -> f32 {
    (0..letters).map(|i| TYPE_START + i as f32 * TYPE_GAP).chain([PASSWORD, PRESS])
        .chain((0..MACHINES).map(|i| LIST + i as f32 * ROW_GAP)).chain([CYCLE])
        .filter(|b| *b > t).fold(CYCLE, f32::min) - t
}

/// O campo como o `Input` médio o desenha: o texto, ou o exemplo apagado, e o cursor e a borda de foco no campo ativo.
fn field_box(text: String, hint: String, focused: bool, cx: &App) -> Div {
    let look = cx.theme();
    let caret = div().flex_shrink_0().w(px(1.5)).h(px(16.)).bg(look.caret);
    div().h(px(32.)).px(px(10.)).flex().items_center().gap(px(1.)).overflow_hidden().text_sm()
        .rounded(look.radius).border_1().border_color(if focused { look.ring } else { look.input }).bg(look.input_background())
        .map(|el| if text.is_empty() {
            el.when(focused, |el| el.child(caret)).child(div().truncate().text_color(look.muted_foreground).child(hint))
        } else {
            el.child(div().truncate().text_color(theme::text()).child(text)).when(focused, |el| el.child(caret))
        })
}

/// Abaixo desta largura (o cartão deitado do "Atualizar?") ficam só o campo da conta e as máquinas ligadas.
const COMPACT_BELOW: f32 = 240.;
const COMPACT_HEIGHT: f32 = 104.;

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    let user = tr_shared("news_login_sample_user", &[]);
    let letters = user.chars().count();
    let reduce = cx.reduce_motion();
    let f = if reduce { frame(STILL, letters) } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t, letters), window, cx);
        frame(t, letters)
    };
    let compact = width < px(COMPACT_BELOW);
    // Do toque até a lista chegar, o formulário espera a resposta como na tela.
    let busy = f.pressed && !f.list;
    let account = app::sign_in_form_field(tr_shared("login_account_or_user", &[]),
        field_box(user.chars().take(f.typed).collect(), tr_shared("login_account_or_user_hint", &[]), !f.password, cx));
    let submit = app::sign_in_submit_button("news-sign-in-go", false, busy).tab_stop(false);
    let form = if compact { div().flex().flex_col().gap_2().child(account).child(submit) } else {
        let password = field_box(if f.password { "••••••••".into() } else { String::new() }, String::new(), f.password && !f.pressed, cx);
        app::sign_in_card().child(app::sign_in_form_heading("news-sign-in-mark", busy, reduce))
            .child(account).child(app::sign_in_form_field(tr("login_senha"), password)).child(submit)
    };

    let machines: Vec<ConnectMachine> = [("news_connect_sample_machine", true), ("news_tags_sample_machine", true), ("news_auto_report_sample_machine", false)]
        .into_iter().enumerate().map(|(i, (key, online))| {
            let name = tr_shared(key, &[]);
            ConnectMachine { machine_id: format!("news-sign-in-{i}"), host: format!("{}.{user}.hangar.dev.br", name.to_lowercase()), name, online }
        }).collect();
    let picked = app::sign_in_default_picks(&machines, &[]);
    let rows = machines.iter().enumerate().filter(|(_, m)| !compact || m.online).map(|(i, m)| {
        let check = app::sign_in_pick_checkbox(&m.machine_id, m.name.clone(), picked.contains(&m.machine_id), m.online).tab_stop(false);
        let subtitle = if compact { String::new() } else { app::sign_in_machine_subtitle(m) };
        app::sign_in_pick_row(m.name.clone(), subtitle, Some(m.online), None, check).opacity(if i < f.rows { 1. } else { 0. })
    });
    let list = if compact { div().flex().flex_col().gap_2().children(rows) } else {
        let sign_out = app::sign_in_sign_out_button("news-sign-in-out").tab_stop(false);
        app::sign_in_card().child(app::sign_in_account_top(None, tr_shared("connect_signed_in_as", &[("name", &user)]), sign_out))
            .child(app::sign_in_section_heading(tr_shared("connect_your_machines", &[])))
            .children(rows)
            .child(app::sign_in_enter_button("news-sign-in-enter", picked.len(), false).tab_stop(false))
    };

    // A lista, a mais alta das duas, fica no fluxo mesmo apagada: a altura não pula na troca e o botão de entrar não corta.
    let stage = div().relative().when(compact, |el| el.h(px(COMPACT_HEIGHT)))
        .child(list.opacity(if f.list { 1. } else { 0. }))
        .child(form.absolute().top_0().left_0().right_0().opacity(if f.list { 0. } else { 1. }));
    // Miniatura só leitura: a tampa por cima não deixa o mouse levar o foco para os botões.
    div().id("news-connect-sign-in").relative().w(width).p(px(if compact { 8. } else { 14. })).rounded(px(9.)).border_1()
        .border_color(theme::border().alpha(0.06)).overflow_hidden().text_color(theme::text())
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .child(stage).child(div().absolute().top_0().left_0().size_full().occlude()).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn script_types_signs_in_and_lists_the_machines() {
        assert_eq!(frame(0.4, 3), Frame { typed: 0, password: false, pressed: false, list: false, rows: 0 });
        assert_eq!(frame(0.7, 3).typed, 2);
        assert_eq!(frame(1.2, 3).typed, 3);
        assert!(frame(1.6, 3).password && !frame(1.6, 3).pressed);
        assert!(frame(2.5, 3).pressed && !frame(2.5, 3).list);
        assert_eq!((frame(3.05, 3).list, frame(3.05, 3).rows), (true, 1));
        assert_eq!(frame(3.3, 3).rows, MACHINES);
        // Parado: a lista inteira, com as ligadas marcadas.
        assert_eq!(frame(STILL, 3), Frame { typed: 3, password: true, pressed: true, list: true, rows: MACHINES });
    }

    #[test]
    fn repaint_waits_for_the_next_step() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.4, 3), 0.1));
        assert!(close(until_next_step(2.0, 3), 0.2));
        assert!(close(until_next_step(3.1, 3), 0.02));
        assert!(close(until_next_step(3.3, 3), 2.7));
    }
}
