// Peças visuais repetidas entre compositor, lista e painel: a medida de cada uma é a do web.
use gpui_kit::{component::{Icon, button::*}, prelude::FluentBuilder, *};
use gpui_kit::assets::IconName;
use crate::theme;

/// Botão só com ícone (`.attach-btn` do web): 44×44, raio 12, ícone 20.
pub fn icon_button(id: impl Into<ElementId>, icon: IconName, tip: String, cx: &App) -> Button {
    Button::new(id).custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::muted())
        .hover(theme::raised()).active(theme::elevated()))
        .icon(Icon::new(icon).size(px(20.))).tooltip(tip.clone()).accessibility_label(tip)
        .w(px(44.)).h(px(44.)).rounded(px(12.))
}

/// Pílula de seletor (`.model-pill`): 30 de altura, fundo accent-dim, texto 12.
pub fn pill_button(id: impl Into<ElementId>, cx: &App) -> Button {
    Button::new(id).custom(ButtonCustomVariant::new(cx).color(theme::accent_dim()).foreground(theme::text())
        .hover(theme::accent_hover()).active(theme::accent_hover()))
        .bg(theme::accent_dim()).h(px(30.)).pl(px(12.)).pr(px(8.)).rounded(px(12.)).text_xs()
}

/// Superfície dos popovers do compositor: mesma borda de vidro e sombra `--elev-2` do web.
pub fn popover(content: AnyElement, narrow: bool) -> AnyElement {
    div().when(narrow, |el| el.w(px(380.))).rounded(px(12.)).border_1().border_color(theme::glass_border()).bg(theme::raised())
        .shadow(theme::popover_shadow()).child(content).into_any_element()
}

/// Rótulo de seção do painel: 11px, 700, caixa alta.
pub fn section_label(title: String) -> Div {
    div().text_size(px(11.)).font_weight(FontWeight::BOLD).text_color(theme::faint()).child(title.to_uppercase())
}

/// `StateChip`: na lista o ocioso vira só o ponto verde; no tamanho grande e nos outros estados, pílula com o nome.
pub fn state_chip(state: &str, label: String, large: bool) -> AnyElement {
    if state == "idle" && !large { return div().size(px(8.)).flex_shrink_0().rounded_full().bg(theme::success()).into_any_element(); }
    let (bg, fg) = theme::pill(state);
    div().flex_shrink_0().rounded_full().bg(bg).text_color(fg).font_weight(FontWeight::SEMIBOLD)
        .when(large, |el| el.px(px(10.)).py(px(3.)).text_size(px(11.)))
        .when(!large, |el| el.px(px(8.)).py(px(2.)).text_size(px(10.)))
        .child(label).into_any_element()
}

pub fn meter(pct: f64) -> AnyElement {
    let color = if pct >= 90. { theme::danger() } else if pct >= 70. { theme::warning() } else { theme::accent() };
    div().h(px(4.)).w_full().rounded_full().bg(theme::raised())
        .child(div().h_full().rounded_full().bg(color).w(relative((pct.clamp(0., 100.) / 100.) as f32)))
        .into_any_element()
}

/// `ContextRing` de 22px: trilho, arco a partir do topo e o número no meio.
pub fn context_ring(pct: Option<f64>) -> AnyElement {
    let color: Hsla = match pct { Some(p) if p >= 90. => theme::danger(), Some(p) if p >= 70. => theme::warning(), _ => theme::muted() };
    let shown = pct.map(|p| format!("{}", p.round() as i64)).unwrap_or_else(|| "—".into());
    let fraction = pct.map(|p| (p / 100.).clamp(0., 1.) as f32).unwrap_or(0.);
    div().relative().size(px(22.)).flex_shrink_0()
        .child(canvas(|_, _, _| (), move |bounds, _, window, _| {
            let center = bounds.center();
            let (r, stroke) = (px(8.25), px(2.75));
            let point = |turn: f32| {
                let angle = turn * std::f32::consts::TAU;
                point(center.x + r * angle.sin(), center.y - r * angle.cos())
            };
            let mut track = PathBuilder::stroke(stroke);
            track.move_to(point(0.));
            track.arc_to(gpui_kit::point(r, r), px(0.), false, true, point(0.5));
            track.arc_to(gpui_kit::point(r, r), px(0.), false, true, point(0.));
            if let Ok(path) = track.build() { window.paint_path(path, theme::border_strong()); }
            if fraction > 0. {
                let mut arc = PathBuilder::stroke(stroke);
                arc.move_to(point(0.));
                if fraction >= 0.999 {
                    arc.arc_to(gpui_kit::point(r, r), px(0.), false, true, point(0.5));
                    arc.arc_to(gpui_kit::point(r, r), px(0.), false, true, point(0.9999));
                } else {
                    arc.arc_to(gpui_kit::point(r, r), px(0.), fraction > 0.5, true, point(fraction));
                }
                if let Ok(path) = arc.build() { window.paint_path(path, color); }
            }
        }).absolute().inset_0())
        .child(div().absolute().inset_0().flex().items_center().justify_center()
            .text_size(px(9.)).font_weight(FontWeight::SEMIBOLD).text_color(color).child(shown))
        .into_any_element()
}

/// Marca do Hangar (dois arcos), tingida pela cor do estado.
pub fn hangar_mark(size: f32, color: Hsla) -> Svg {
    svg().path(crate::HANGAR_MARK).size(px(size)).flex_shrink_0().text_color(color)
}

/// Selo do provider no canto da marca, só quando a lista mistura providers.
pub fn provider_badge(provider: &str) -> AnyElement {
    let (color, glyph) = theme::provider(provider);
    div().absolute().top(px(-4.)).left(px(-4.)).size(px(12.)).rounded(px(3.5)).bg(color.opacity(0.16))
        .border_1().border_color(theme::border()).flex().items_center().justify_center()
        .text_size(px(8.)).font_weight(FontWeight::BOLD).text_color(color).child(glyph)
        .into_any_element()
}

pub fn small_icon(icon: IconName, size: f32, color: Hsla) -> Icon {
    Icon::new(icon).size(px(size)).text_color(color)
}
