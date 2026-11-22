use std::time::Instant;

use gpui_kit::{assets::IconName, component::{Disableable, Sizable, button::{Button, ButtonVariants}}, prelude::FluentBuilder, *};

use crate::{app::{chrome, model_picker, provider_picker}, i18n::tr, theme};

const CYCLE: f32 = 6.5;
const TRANSITIONS: [f32; 3] = [1.3, 2.7, 4.2];
const MODEL: &str = "gpt-6.2-sol";
const ACCOUNT: &str = "ChatGPT Pro";

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    let step = if cx.reduce_motion() { 3 } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        let step = TRANSITIONS.partition_point(|at| *at <= t);
        super::repaint_in(TRANSITIONS.get(step).copied().unwrap_or(CYCLE) - t, window, cx);
        step
    };
    let tab = |id, text: String, glyph: AnyElement| model_picker::rail_tab(model_picker::rail_button(id, false, text, glyph), false).into_any_element();
    let rail = model_picker::rail_view(vec![
        tab("favorites", tr("picker_favorites"), chrome::small_icon(IconName::Star, 16., theme::accent()).into_any_element()),
        tab("claude", "Claude".into(), chrome::provider_glyph("claude", 20.).into_any_element()),
        tab("codex", "Codex".into(), chrome::provider_glyph("codex", 20.).into_any_element()),
        model_picker::rail_tab(provider_picker::tab_button(true), true).into_any_element(),
    ]);
    let accounts = div().min_w_0().flex_1().px_2().py_1().flex().flex_col()
        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(tr("claude_providers")))
        .child(provider_picker::account_row("demo-pro", ACCOUNT, "CLIProxyAPI", step >= 1, false, false))
        .child(provider_picker::account_row("demo-plus", "ChatGPT Plus", "CLIProxyAPI", false, false, false));
    let models = div().min_w_0().flex_1().px_2().py_1().flex().flex_col()
        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(tr("claude_provider_models").replace("{name}", ACCOUNT)))
        .child(provider_picker::model_row(MODEL, step >= 2))
        .child(provider_picker::model_row("gpt-6.1-sol", false));
    let columns = div().flex().when(width < px(400.), |el| el.flex_col()).child(accounts).when(step >= 1, |el| el.child(models));
    let body = div().min_w_0().flex_1().flex().flex_col().justify_between().min_h(px(185.));
    let body = if step >= 3 {
        body.child(provider_picker::confirmation(ACCOUNT, MODEL)).child(div().pt_2().flex().justify_end()
            .child(Button::new("news-provider-confirm").primary().small().label(tr("claude_provider_confirm"))))
    } else {
        body.child(columns).child(div().pt_2().flex().justify_end()
            .child(Button::new("news-provider-apply").primary().small().disabled(step < 2).label(tr("claude_provider_apply"))))
    };
    div().id("news-claude-providers").w_full().p(px(6.)).rounded(px(12.)).border_1().border_color(theme::glass_border())
        .bg(theme::popup_fill(theme::raised())).child(div().flex().child(rail).child(body)).into_any_element()
}
