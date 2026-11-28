//! `connect-remover-computador`: o "⋯" de um computador fora do ar abre "Remover da conta", a confirmação aparece e o
//! bloco sai da grade "Seus computadores".

use std::time::Instant;
use gpui_kit::{base::Selectable, component::{ActiveTheme, StyledExt, button::*, dialog::DialogFooter}, prelude::FluentBuilder, *};
use crate::{app::{self, chrome, ComputerLook}, i18n::{tr, tr_shared}, theme};

const CYCLE: f32 = 6.;
/// O ponteiro chega ao "⋯" do computador fora do ar.
const HOVER: f32 = 0.6;
/// O menu abre com "Remover da conta".
const MENU: f32 = 1.1;
/// O item fica realçado, a um passo do clique.
const PICK: f32 = 1.8;
/// O clique: o menu fecha e a confirmação aparece.
const CONFIRM: f32 = 2.3;
/// Confirmado: o bloco diz que está removendo.
const REMOVING: f32 = 3.5;
/// O bloco some em `FADE_STEPS` degraus de `FADE_STEP`.
const FADE: f32 = 4.3;
const FADE_STEP: f32 = 0.08;
const FADE_STEPS: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame {
    hover: bool,
    menu: bool,
    picked: bool,
    confirm: bool,
    removing: bool,
    /// Degrau da saída do bloco (0 = começando); `None` fora da saída.
    fading: Option<u8>,
    gone: bool,
}

fn frame(t: f32) -> Frame {
    let gone = FADE + FADE_STEPS as f32 * FADE_STEP;
    Frame {
        hover: t >= HOVER && t < CONFIRM,
        menu: t >= MENU && t < CONFIRM,
        picked: t >= PICK && t < CONFIRM,
        confirm: t >= CONFIRM && t < REMOVING,
        removing: t >= REMOVING && t < gone,
        fading: (t >= FADE && t < gone).then(|| ((t - FADE) / FADE_STEP).floor() as u8),
        gone: t >= gone,
    }
}

/// Movimento reduzido: o menu aberto com "Remover da conta", que é o que a novidade mostra.
const STILL: Frame = Frame { hover: true, menu: true, picked: false, confirm: false, removing: false, fading: None, gone: false };

/// Segundos até o próximo instante em que o quadro muda: o roteiro anda em degraus, não a cada quadro.
fn until_next_step(t: f32) -> f32 {
    let fade = (1..=FADE_STEPS).map(|k| FADE + k as f32 * FADE_STEP);
    fade.chain([HOVER, MENU, PICK, CONFIRM, REMOVING, FADE, CYCLE]).filter(|b| *b > t).fold(CYCLE, f32::min) - t
}

/// Abaixo desta largura (o cartão deitado do "Atualizar?") fica só o bloco que sai, com o menu.
const COMPACT_BELOW: f32 = 240.;
/// Alturas fixas do palco: o cartão não pula quando o bloco sai.
const STAGE_HEIGHT: f32 = 336.;
const COMPACT_HEIGHT: f32 = 118.;
/// Onde o menu nasce, medido do topo do bloco: logo abaixo do "⋯".
const MENU_TOP: f32 = 46.;
/// O computador que sai, na ordem das amostras.
const TARGET: usize = 1;

/// Os outros computadores da conta: (chave do nome, no ar).
const MACHINES: [(&str, bool); 2] = [("news_tags_sample_machine", true), ("news_auto_report_sample_machine", false)];

/// O menu do "⋯" com o visual do `menu_style`; o item escolhido tem o realce do kit.
fn menu(picked: bool, cx: &App) -> Div {
    let item = div().flex().items_center().h(app::menu_item_height()).refine_style(&app::menu_item_look())
        .when(picked, |el| el.bg(cx.theme().tokens.accent)).child(app::computer_remove_item());
    div().w(px(app::COMPUTER_MENU_WIDTH)).refine_style(&app::menu_surface_look())
        .child(div().flex().flex_col().refine_style(&app::menu_list_look()).child(item))
}

/// O alerta de confirmação como o `confirm_alert` o abre, num `div` com a superfície do diálogo.
fn confirm_card(name: &str, cx: &App) -> Div {
    let ok = chrome::alert_ok_button(Some(ButtonVariant::Danger), true, tr("connect_computer_remove"), cx).tab_stop(false);
    div().w(px(360.)).max_w_full().border_1().flex().flex_col().gap_3().refine_style(&app::popup::dialog_surface_look())
        .shadow(theme::popover_shadow())
        .child(chrome::alert_title(tr("connect_computer_remove_title").replace("{name}", name)))
        .child(chrome::alert_description(tr("connect_computer_remove_body")))
        .child(DialogFooter::new().pt_1().child(chrome::alert_cancel_button().tab_stop(false)).child(ok))
}

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    let f = if cx.reduce_motion() { STILL } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t), window, cx);
        frame(t)
    };
    let compact = width < px(COMPACT_BELOW);
    let pad = if compact { 8. } else { 14. };
    let account = tr_shared("news_login_sample_user", &[]);
    let target_name = tr_shared(MACHINES[TARGET].0, &[]);
    // Os blocos como a página os monta: esta máquina com o selo e o Desligar, os outros com o "⋯".
    let this = app::computer_block(ComputerLook { id: "news-this".into(), name: tr_shared("news_connect_sample_machine", &[]), online: true,
        this: true, removing: false, error: None },
        None, Some(app::this_computer_off_button(false).tab_stop(false).into_any_element()));
    let others = MACHINES.iter().enumerate().map(|(i, (key, online))| {
        let id = format!("news-remove-{i}");
        let name = tr_shared(key, &[]);
        let target = i == TARGET;
        let menu_button = app::computer_menu_button(&id, &name, f.removing).tab_stop(false).selected(target && f.hover);
        // No ar e fora deste app: o Adicionar; fora do ar, o bloco já diz tudo.
        let footer = online.then(|| app::computer_add_button(&id, false, false).tab_stop(false).into_any_element());
        let look = ComputerLook { id, name, online: *online, this: false, removing: target && f.removing, error: None };
        let block = app::computer_block(look, Some(menu_button.into_any_element()), footer).relative();
        let block = match f.fading { Some(step) if target => block.opacity(1. - (step as f32 + 1.) / (FADE_STEPS as f32 + 1.)), _ => block };
        block.when(target && f.menu, |el| el.child(div().absolute().top(px(MENU_TOP)).right(px(12.)).child(menu(f.picked, cx))))
    });
    let mut blocks: Vec<AnyElement> = Vec::new();
    for (i, block) in others.enumerate() {
        if i == TARGET && f.gone { continue; }
        if compact && i != TARGET { continue; }
        blocks.push(block.into_any_element());
    }
    let content = if compact {
        div().flex().flex_col().children(blocks)
    } else {
        let columns = app::computer_columns(f32::from(width) - 2. * pad);
        div().flex().flex_col().gap_3().child(app::computers_head(&account))
            .child(app::computers_grid(columns).child(this).children(blocks))
    };
    // A confirmação cobre a grade como o diálogo cobre a janela; no cartão deitado não cabe e o roteiro vai direto à saída.
    let confirm = (f.confirm && !compact).then(|| div().absolute().top_0().left_0().size_full().flex().items_center().justify_center()
        .bg(theme::background().alpha(0.55)).child(confirm_card(&target_name, cx)));
    div().id("news-connect-remove-computer").relative().w(width).h(px(if compact { COMPACT_HEIGHT } else { STAGE_HEIGHT })).p(px(pad))
        .rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06)).overflow_hidden().text_color(theme::text())
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .child(content).children(confirm)
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
    fn script_opens_the_menu_confirms_and_removes_the_computer() {
        let none = Frame { hover: false, menu: false, picked: false, confirm: false, removing: false, fading: None, gone: false };
        assert_eq!(frame(0.3), none);
        assert_eq!(frame(0.8), Frame { hover: true, ..none });
        assert_eq!(frame(1.3), Frame { hover: true, menu: true, ..none });
        assert_eq!(frame(2.0), Frame { hover: true, menu: true, picked: true, ..none });
        assert_eq!(frame(2.8), Frame { confirm: true, ..none });
        assert_eq!(frame(3.8), Frame { removing: true, ..none });
        assert_eq!(frame(4.33), Frame { removing: true, fading: Some(0), ..none });
        assert_eq!(frame(5.0), Frame { gone: true, ..none });
        // Parado: o menu aberto, antes da escolha.
        assert!(STILL.menu && !STILL.picked && !STILL.gone);
    }

    #[test]
    fn repaint_waits_for_the_next_step() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.2), 0.4));
        assert!(close(until_next_step(2.5), 1.0));
        assert!(close(until_next_step(4.33), 0.05));
        assert!(close(until_next_step(5.0), 1.0));
    }
}
