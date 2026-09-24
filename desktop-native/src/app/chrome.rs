// Peças visuais repetidas entre compositor, lista e painel: a medida de cada uma é a do web.
use gpui_kit::{component::{Icon, button::*}, prelude::FluentBuilder, *};
use gpui_kit::assets::IconName;
use crate::theme;

/// Botão só com ícone (`.btn.icon` do mock): 28×28, raio 6, ícone 16.
pub fn icon_button(id: impl Into<ElementId>, icon: IconName, tip: String, cx: &App) -> Button {
    Button::new(id).custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::muted())
        .hover(theme::hover()).active(theme::hover()))
        .icon(Icon::new(icon).size(px(16.))).tooltip(tip.clone()).accessibility_label(tip)
        .w(px(28.)).h(px(28.)).rounded(px(6.))
}

/// Pílula de seletor do compositor: 26 de altura; só a caixa solta tem fundo de destaque, colado é texto quieto.
pub fn pill_button(id: impl Into<ElementId>, cx: &App) -> Button {
    let fill = if theme::is_floating() { theme::accent_dim() } else { transparent_black() };
    Button::new(id).custom(ButtonCustomVariant::new(cx).color(fill).foreground(theme::text())
        .hover(theme::hover()).active(theme::hover()))
        .bg(fill).h(px(26.)).px(px(8.)).rounded(px(6.)).text_size(px(12.5))
}

/// Tecla de atalho mostrada ao lado de uma ação.
pub fn kbd(keys: &'static str) -> Div {
    div().flex_shrink_0().px(px(4.)).rounded(px(4.)).border_1().border_color(theme::border_strong())
        .font_family(theme::MONO).text_size(px(11.)).text_color(theme::faint()).child(keys)
}

/// Selo quadrado do provider (C, X, π…) na cor da marca, como na lista e no cabeçalho do mock.
pub fn provider_glyph(provider: &str, size: f32) -> Div {
    let (color, glyph) = theme::provider(provider);
    div().size(px(size)).flex_shrink_0().rounded(px(4.)).bg(color.opacity(0.2)).flex().items_center().justify_center()
        .text_size(px(10.)).font_weight(FontWeight::BOLD).text_color(color).child(glyph)
}

/// Superfície dos popovers do compositor: mesma borda de vidro e sombra `--elev-2` do web.
pub fn popover(content: AnyElement, narrow: bool) -> AnyElement {
    div().when(narrow, |el| el.w(px(380.))).rounded(px(12.)).border_1().border_color(theme::glass_border()).bg(theme::raised())
        .shadow(theme::popover_shadow()).child(content).into_any_element()
}

/// Rótulo de seção (lista e painel): 12px, peso médio, sem caixa alta, como no mock.
pub fn section_label(title: String) -> Div {
    div().text_xs().font_weight(FontWeight::MEDIUM).text_color(theme::faint()).child(title)
}

/// `StateChip`: na lista o ocioso vira só o ponto verde; nos outros estados, pílula com ponto e nome.
/// O limite de uso não leva ponto: não é atividade, é um aviso.
pub fn state_chip(state: &str, label: String, large: bool) -> AnyElement {
    if state == "idle" && !large { return div().size(px(7.)).mx(px(4.)).flex_shrink_0().rounded_full().bg(theme::success()).into_any_element(); }
    let (bg, fg) = theme::pill(state);
    div().flex_shrink_0().h(px(22.)).px(px(9.)).flex().items_center().gap(px(6.)).rounded_full().bg(bg).text_color(fg)
        .text_size(px(12.)).font_weight(FontWeight::MEDIUM)
        .when(state != "limited", |el| el.child(div().size(px(7.)).flex_shrink_0().rounded_full().bg(fg)))
        .child(label).into_any_element()
}

pub fn meter(pct: f64) -> AnyElement {
    let color = if pct >= 90. { theme::danger() } else if pct >= 70. { theme::warning() } else { theme::accent() };
    div().h(px(4.)).w_full().rounded_full().bg(theme::raised())
        .child(div().h_full().rounded_full().bg(color).w(relative((pct.clamp(0., 100.) / 100.) as f32)))
        .into_any_element()
}

/// Marca do Hangar (dois arcos), tingida pela cor do estado.
pub fn hangar_mark(size: f32, color: Hsla) -> Svg {
    svg().path(crate::HANGAR_MARK).size(px(size)).flex_shrink_0().text_color(color)
}

pub fn small_icon(icon: IconName, size: f32, color: Hsla) -> Icon {
    Icon::new(icon).size(px(size)).text_color(color)
}
