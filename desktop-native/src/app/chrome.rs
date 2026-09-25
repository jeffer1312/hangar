// Peças visuais repetidas entre compositor, lista e painel: a medida de cada uma é a do web.
use gpui_kit::{component::{ActiveTheme, Icon, Sizable, StyledExt, button::*}, prelude::FluentBuilder, *};
use gpui_kit::assets::IconName;
use crate::theme;
use std::{sync::OnceLock, time::{Duration, Instant}};

/// Batida das animações que se repetem: 30 por segundo, numa grade de tempo comum a todas. O `Spinner`/`Skeleton` do
/// kit pedem um quadro por atualização da tela enquanto montados; relógios próprios de 30 Hz, fora de fase entre si,
/// somariam mais que 30 quadros. Na grade, as views que batem juntas saem num quadro só.
const PULSE_TICK: Duration = Duration::from_micros(33_333);
/// Resposta que chega antes disso não mostra esqueleto: o espaço fica vazio e a lista entra direto.
const SKELETON_DELAY: Duration = Duration::from_millis(150);

fn pulse_epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

/// Ponto do ciclo de `period` no relógio comum, de 0 a 1: toda animação igual anda em fase, como as do kit que nascem juntas.
fn pulse_phase(period: Duration) -> f32 {
    (pulse_epoch().elapsed().as_secs_f64() % period.as_secs_f64() / period.as_secs_f64()) as f32
}

/// Redesenha a view a cada batida da grade, a partir de `delay`, até ela sair da tela. Com movimento reduzido, não redesenha.
fn pulse<V: 'static>(delay: Duration, cx: &mut Context<V>) {
    cx.spawn(async move |view, cx| {
        cx.background_executor().timer(delay).await;
        loop {
            let into = Duration::from_nanos((pulse_epoch().elapsed().as_nanos() % PULSE_TICK.as_nanos()) as u64);
            cx.background_executor().timer(PULSE_TICK - into).await;
            if view.update(cx, |_, cx| if !cx.reduce_motion() { cx.notify() }).is_err() { break; }
        }
    }).detach();
}

/// A view da animação, guardada pela chave enquanto for desenhada em quadros seguidos. Sem o `use_keyed_state`, que
/// avisaria a view de fora a cada batida: aqui só a view pequena é marcada para redesenhar.
fn keyed_view<V: 'static>(key: ElementId, window: &mut Window, cx: &mut App, init: impl FnOnce(&mut Context<V>) -> V) -> Entity<V> {
    window.with_global_id(key, |id, window| window.with_element_state(id, |previous: Option<Entity<V>>, _| {
        let view = previous.unwrap_or_else(|| cx.new(init));
        (view.clone(), view)
    }))
}

/// O `Spinner` do kit (mesmo ícone, giro de 0,8 s com `ease_in_out`) no relógio comum de 30 batidas, numa view própria
/// guardada entre quadros. Parado com movimento reduzido, como o do kit.
#[derive(IntoElement)]
pub struct Spinner { key: ElementId, icon: IconName, size: Pixels, color: Hsla }

impl Spinner {
    pub fn new(key: impl Into<ElementId>, icon: IconName, size: Pixels, color: Hsla) -> Self {
        Self { key: key.into(), icon, size, color }
    }
}

struct SpinnerView { icon: IconName, size: Pixels, color: Hsla }

impl Render for SpinnerView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let turn = if cx.reduce_motion() { 0. } else { ease_in_out(pulse_phase(Duration::from_millis(800))) };
        div().child(Icon::new(self.icon.clone()).with_size(self.size).text_color(self.color).transform(Transformation::rotate(percentage(turn))))
    }
}

impl RenderOnce for Spinner {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (icon, size, color) = (self.icon, self.size, self.color);
        let view = keyed_view(self.key, window, cx, |cx| { pulse(Duration::ZERO, cx); SpinnerView { icon: icon.clone(), size, color } });
        view.update(cx, |view, cx| {
            if view.icon != icon || view.size != size || view.color != color { (view.icon, view.size, view.color) = (icon, size, color); cx.notify(); }
        });
        view.cached(StyleRefinement::default().size(size))
    }
}

/// O `Skeleton` do kit (mesma cor, pulso de 2 s) no relógio comum de 30 batidas, numa view própria que só aparece depois
/// de `SKELETON_DELAY`; até lá ocupa o mesmo espaço, vazio.
#[derive(IntoElement)]
pub struct Skeleton { key: ElementId, style: StyleRefinement, secondary: bool }

impl Skeleton {
    pub fn new(key: impl Into<ElementId>) -> Self { Self { key: key.into(), style: StyleRefinement::default(), secondary: false } }
    pub fn secondary(mut self) -> Self { self.secondary = true; self }
}

impl Styled for Skeleton {
    fn style(&mut self) -> &mut StyleRefinement { &mut self.style }
}

struct SkeletonView { style: StyleRefinement, secondary: bool, born: Instant }

impl Render for SkeletonView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let base = div().w_full().h_4().refine_style(&self.style);
        if self.born.elapsed() < SKELETON_DELAY { return base; }
        let color = if self.secondary { cx.theme().skeleton.opacity(0.5) } else { cx.theme().skeleton };
        let delta = if cx.reduce_motion() { 0. } else { bounce(ease_in_out)(pulse_phase(Duration::from_secs(2))) };
        base.bg(color).opacity(1.0 - delta * 0.5)
    }
}

impl RenderOnce for Skeleton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (style, secondary) = (self.style, self.secondary);
        let view = keyed_view(self.key, window, cx, |cx| {
            pulse(SKELETON_DELAY, cx);
            SkeletonView { style: style.clone(), secondary, born: Instant::now() }
        });
        view.update(cx, |view, cx| {
            if view.style != style || view.secondary != secondary { (view.style, view.secondary) = (style.clone(), secondary); cx.notify(); }
        });
        view.cached(style)
    }
}

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
