//! `acoes-dos-avisos`: o aviso ganha o botão que resolve. O plugin sumido é reinstalado; o hook que falha é
//! consertado por uma sessão à parte, e o resultado volta como uma linha.

use std::time::Instant;
use gpui_kit::{prelude::FluentBuilder, *};
use crate::{app, i18n::{tr, tr_shared}, theme};

const CYCLE: f32 = 8.;
/// Os botões foram tocados: os dois andam.
const WORKING: f32 = 2.;
/// Reinstalado e consertado; a linha do resultado chega.
const DONE: f32 = 4.5;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Step { Idle, Working, Done }

fn step(t: f32) -> Step { if t >= DONE { Step::Done } else if t >= WORKING { Step::Working } else { Step::Idle } }

fn until_next_step(t: f32) -> f32 { [WORKING, DONE, CYCLE].into_iter().filter(|b| *b > t).fold(CYCLE, f32::min) - t }

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: o fim, que mostra os dois resolvidos e a linha do resultado.
    let step = if cx.reduce_motion() { Step::Done } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t), window, cx);
        self::step(t)
    };
    let (plugin, child) = ("fmt@tools", "conserto-lint");
    let head = |id: &str, hook: &str, done: bool| app::notice_head_marked(SharedString::from(format!("news-actions-{id}")),
        tr_shared("notice_hook_failed_line", &[("name", hook)]), None, None, done);
    let row = |id: &str, hook: &str, done: bool, action: AnyElement| app::notice_rule_line(div().flex().items_center().gap(px(7.)).min_w_0()
        .child(head(id, hook, done)).child(action));
    let reinstall = match step {
        Step::Idle => app::notice_action_button("news-reinstall", tr_shared("notice_reinstall", &[])).tab_stop(false).into_any_element(),
        Step::Working => app::notice_spin("news-reinstall", div().text_size(px(12.5)).text_color(theme::muted())
            .child(tr_shared("notice_reinstalling", &[("name", plugin)]))).into_any_element(),
        Step::Done => app::notice_reinstalled(plugin, false)
            .child(app::notice_action_button("news-reload", tr_shared("notice_reload", &[])).tab_stop(false)).into_any_element(),
    };
    // Reinstalado, a linha do plugin é só o resultado: com o título junto ela não cabe no cartão.
    let plugin_row = if step == Step::Done { app::notice_rule_line(reinstall) } else { row("plugin", "SessionStart", false, reinstall) };
    let fix = match step {
        Step::Idle => app::notice_action_button("news-fix", tr_shared("notice_fix", &[])).tab_stop(false).into_any_element(),
        Step::Working => app::notice_spin("news-fix", app::notice_fixing("news-fix", child)).into_any_element(),
        Step::Done => app::notice_fixed().into_any_element(),
    };
    // Altura do quadro mais alto (com o resultado): o cartão não pula entre os passos.
    div().id("news-notice-actions").w(width).min_h(px(170.)).p(px(14.)).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .flex().flex_col().gap(px(10.))
        .child(plugin_row)
        .child(row("hook", "PostToolUse:Bash", step == Step::Done, fix))
        .when(step == Step::Done, |el| el.child(app::notice_rule_line(app::fix_result_row(true,
            format!("{child}: {}", tr("news_notice_actions_sample_result")), app::fix_open_button("news-fix-result").tab_stop(false)))))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn the_buttons_work_then_both_are_resolved() {
        assert_eq!(step(1.), Step::Idle);
        assert_eq!(step(3.), Step::Working);
        assert_eq!(step(6.), Step::Done);
        assert!((until_next_step(1.) - 1.).abs() < 1e-6);
    }
}
