// Peças visuais repetidas entre compositor, lista e painel: a medida de cada uma é a do web.
use gpui_kit::{component::{ActiveTheme, Icon, Sizable, StyledExt, button::*}, prelude::FluentBuilder, *};
use gpui_kit::assets::IconName;
use crate::theme;
use std::{cell::Cell, rc::Rc, sync::OnceLock, time::{Duration, Instant}};

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

/// Redesenha a view a cada batida da grade, a partir de `delay`, até ela sair da tela. Com movimento reduzido, ou com
/// `awake` dizendo que ela não está à vista, não redesenha.
fn pulse<V: 'static>(delay: Duration, awake: fn(&V) -> bool, cx: &mut Context<V>) {
    cx.spawn(async move |view, cx| {
        cx.background_executor().timer(delay).await;
        loop {
            let into = Duration::from_nanos((pulse_epoch().elapsed().as_nanos() % PULSE_TICK.as_nanos()) as u64);
            cx.background_executor().timer(PULSE_TICK - into).await;
            if view.update(cx, |view, cx| if !cx.reduce_motion() && awake(view) { cx.notify() }).is_err() { break; }
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
        let view = keyed_view(self.key, window, cx, |cx| { pulse(Duration::ZERO, |_| true, cx); SpinnerView { icon: icon.clone(), size, color } });
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
            pulse(SKELETON_DELAY, |_| true, cx);
            SkeletonView { style: style.clone(), secondary, born: Instant::now() }
        });
        view.update(cx, |view, cx| {
            if view.style != style || view.secondary != secondary { (view.style, view.secondary) = (style.clone(), secondary); cx.notify(); }
        });
        view.cached(style)
    }
}

/// Ícone que respira (o `breathe` do botão Atividade do web: opacidade 0,55 → 1 e escala 0,92 → 1,05 em 1,5 s), no
/// relógio comum de 30 batidas, numa view própria; inteiro e parado com movimento reduzido.
#[derive(IntoElement)]
pub struct Breathing { key: ElementId, icon: IconName, size: Pixels, color: Hsla }

impl Breathing {
    pub fn new(key: impl Into<ElementId>, icon: IconName, size: Pixels, color: Hsla) -> Self { Self { key: key.into(), icon, size, color } }
}

struct BreathingView { icon: IconName, size: Pixels, color: Hsla }

impl Render for BreathingView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let k = if cx.reduce_motion() { 1. } else {
            let p = pulse_phase(Duration::from_millis(1500));
            ease_in_out(if p < 0.5 { 2. * p } else { 2. - 2. * p })
        };
        let (opacity, scale) = (0.55 + 0.45 * k, 0.92 + 0.13 * k);
        // A escala muda o tamanho do ícone, não um `transform`: o da GPUI corta o desenho de ícone que não é quadrado cheio.
        div().size(self.size).flex().items_center().justify_center()
            .child(Icon::new(self.icon.clone()).size(self.size * scale).text_color(self.color.opacity(opacity)))
    }
}

impl RenderOnce for Breathing {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (icon, size, color) = (self.icon, self.size, self.color);
        let view = keyed_view(self.key, window, cx, |cx| { pulse(Duration::ZERO, |_| true, cx); BreathingView { icon: icon.clone(), size, color } });
        view.update(cx, |view, cx| {
            if view.icon != icon || view.size != size || view.color != color { (view.icon, view.size, view.color) = (icon, size, color); cx.notify(); }
        });
        // Sem encolher: dentro do botão a caixa guardada perdia largura e o ícone saía cortado à direita.
        view.cached(StyleRefinement::default().size(size).flex_shrink_0())
    }
}

/// `--ease-out` do web: `cubic-bezier(0.23, 1, 0.32, 1)`.
pub fn ease_out(x: f32) -> f32 {
    let curve = |s: f32, a: f32, b: f32| 3. * (1. - s) * (1. - s) * s * a + 3. * (1. - s) * s * s * b + s * s * s;
    let (mut lo, mut hi) = (0f32, 1f32);
    for _ in 0..20 {
        let mid = (lo + hi) / 2.;
        if curve(mid, 0.23, 0.32) < x { lo = mid } else { hi = mid }
    }
    curve((lo + hi) / 2., 1., 1.)
}

// A marca "trabalhando" do web (HangarWorking.svelte), no quadro de 24: raio, abertura e atraso de entrada e de giro de cada arco.
const MARK_ARCS: [(f32, f32, f32, f32); 3] = [(9.1, 30., 0., 0.), (6.35, 18., 0.09, 0.48), (3.6, 6., 0.18, 0.96)];
const MARK_STROKE: f32 = 1.55;
const MARK_DRAW: f32 = 0.72;
const MARK_LOOP: f32 = 0.9;
const MARK_CYCLE: f32 = 3.2;

/// Um quadro da marca: escala do conjunto e, por arco, quanto do traço está desenhado e o giro total em graus.
#[derive(Clone, Copy, Debug, PartialEq)]
struct MarkFrame { scale: f32, arcs: [(f32, f32); 3] }

/// `t` em segundos desde que a marca apareceu; `None` é a marca completa e parada (movimento reduzido).
fn mark_frame(t: Option<f32>) -> MarkFrame {
    let Some(t) = t else { return MarkFrame { scale: 1., arcs: [(1., 0.); 3] } };
    let cycle = |start: f32| (t >= start).then(|| (t - start) % MARK_CYCLE / MARK_CYCLE);
    // Trecho de `from` a `to` do ciclo, com a curva aplicada em cada trecho, como o keyframe do CSS.
    let span = |c: f32, from: f32, to: f32| ease_out(((c - from) / (to - from)).clamp(0., 1.));
    let whole = cycle(MARK_LOOP);
    let scale = whole.map_or(1., |c| match c {
        c if c < 0.63 => 1.,
        c if c < 0.78 => 1. - 0.56 * span(c, 0.63, 0.78),
        c if c < 0.90 => 0.44 + 0.62 * span(c, 0.78, 0.90),
        c => 1.06 - 0.06 * span(c, 0.90, 1.),
    });
    let mut arcs = [(0., 0.); 3];
    for (k, &(_, _, enter, phase)) in MARK_ARCS.iter().enumerate() {
        let drawn = if t < enter { 0. } else { ease_out(((t - enter) / MARK_DRAW).min(1.)) };
        let turn = cycle(MARK_LOOP + phase).map_or(0., |p| if p < 0.33 { 360. * span(p, 0., 0.33) } else { 0. });
        let spiral = whole.map_or(0., |c| if c < 0.63 { 0. } else { 360. * (k + 1) as f32 * span(c, 0.63, 1.) });
        arcs[k] = (drawn, turn + spiral);
    }
    MarkFrame { scale, arcs }
}

fn paint_mark(bounds: Bounds<Pixels>, frame: MarkFrame, color: Hsla, window: &mut Window) {
    let unit = f32::from(bounds.size.width) / 24.;
    let center = bounds.center();
    let width = MARK_STROKE * unit * frame.scale;
    for (&(radius, gap, _, _), &(drawn, turn)) in MARK_ARCS.iter().zip(&frame.arcs) {
        if drawn <= 0. { continue; }
        let radius = radius * unit * frame.scale;
        let start = 180. - gap + turn;
        let sweep = (180. + 2. * gap) * drawn;
        let at = |deg: f32| {
            let rad = deg.to_radians();
            point(center.x + px(radius * rad.cos()), center.y + px(radius * rad.sin()))
        };
        let steps = (sweep / 6.).ceil().max(2.) as usize;
        let mut path = PathBuilder::stroke(px(width));
        path.move_to(at(start));
        for step in 1..=steps { path.line_to(at(start + sweep * step as f32 / steps as f32)); }
        if let Ok(path) = path.build() { window.paint_path(path, color); }
        // Pontas redondas, como o `stroke-linecap="round"` do web.
        for end in [at(start), at(start + sweep)] {
            window.paint_quad(fill(Bounds::centered_at(end, size(px(width), px(width))), color).corner_radii(px(width / 2.)));
        }
    }
}

/// A marca animada "trabalhando" (três arcos: entrada que se desenha, onda de giro e respiro), no relógio comum de 30
/// batidas, numa view própria guardada entre quadros. Fora da área visível (barra rolada, aba fora da faixa) para de
/// pedir quadro; com movimento reduzido fica completa e parada.
#[derive(IntoElement)]
pub struct WorkingMark { key: ElementId, size: f32, color: Hsla }

impl WorkingMark {
    pub fn new(key: impl Into<ElementId>, size: f32, color: Hsla) -> Self { Self { key: key.into(), size, color } }
}

struct WorkingMarkView { size: f32, color: Hsla, born: Instant, visible: Rc<Cell<bool>> }

impl Render for WorkingMarkView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let frame = mark_frame((!cx.reduce_motion()).then(|| self.born.elapsed().as_secs_f32()));
        let (color, visible) = (self.color, self.visible.clone());
        div().size(px(self.size)).flex_shrink_0().child(canvas(
            move |bounds, window, _| visible.set(window.content_mask().bounds.intersects(&bounds)),
            move |bounds, _, window, _| paint_mark(bounds, frame, color, window),
        ).size_full())
    }
}

impl RenderOnce for WorkingMark {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (size, color) = (self.size, self.color);
        let view = keyed_view(self.key, window, cx, |cx| {
            pulse(Duration::ZERO, |view: &WorkingMarkView| view.visible.get(), cx);
            WorkingMarkView { size, color, born: Instant::now(), visible: Rc::new(Cell::new(true)) }
        });
        view.update(cx, |view, cx| {
            if view.size != size || view.color != color { (view.size, view.color) = (size, color); cx.notify(); }
        });
        view.cached(StyleRefinement::default().size(px(size)))
    }
}

/// Segundos desde `since` ("6s", "1m 5s", "1h 2m"), numa view própria com chave que se redesenha só na virada de cada
/// segundo, como a `WorkingMark`: a área em volta não acorda.
#[derive(IntoElement)]
pub struct Elapsed { key: ElementId, since: Instant }

impl Elapsed {
    pub fn new(key: impl Into<ElementId>, since: Instant) -> Self { Self { key: key.into(), since } }
}

struct ElapsedView { since: Instant }

impl Render for ElapsedView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().flex().items_center().pt(px(1.)).text_size(px(11.)).text_color(theme::faint())
            .child(format_elapsed(self.since.elapsed()))
    }
}

impl RenderOnce for Elapsed {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let since = self.since;
        let view = keyed_view(self.key, window, cx, |cx| {
            cx.spawn(async move |view, cx| loop {
                let Ok(since) = view.read_with(cx, |view: &ElapsedView, _| view.since) else { break };
                let into = Duration::from_nanos((since.elapsed().as_nanos() % 1_000_000_000) as u64);
                // Vira o segundo na batida seguinte da grade comum: com a marca animando, sai no mesmo quadro que ela.
                let turn = Duration::from_secs(1) - into;
                let off = (pulse_epoch().elapsed() + turn).as_nanos() % PULSE_TICK.as_nanos();
                let turn = turn + Duration::from_nanos(((PULSE_TICK.as_nanos() - off) % PULSE_TICK.as_nanos()) as u64);
                cx.background_executor().timer(turn).await;
                if view.update(cx, |_, cx| cx.notify()).is_err() { break; }
            }).detach();
            ElapsedView { since }
        });
        // O começo vem de um horário de parede convertido a cada desenho e varia em microssegundos: só um salto conta.
        view.update(cx, |view, cx| {
            if since.saturating_duration_since(view.since).max(view.since.saturating_duration_since(since)) > Duration::from_millis(500) {
                view.since = since;
                cx.notify();
            }
        });
        view.cached(StyleRefinement::default().size_full())
    }
}

pub fn format_elapsed(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m {}s", secs / 60, secs % 60),
        _ => format!("{}h {}m", secs / 3600, secs % 3600 / 60),
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

/// Alerta de sim ou não. O Enter do kit confirma o alerta ao descer a tecla, com o foco onde estiver: no Cancelar, Enter
/// confirmaria. Aqui o Enter segue para o botão focado, e só o clique no botão de confirmar entra no confirmar do kit,
/// que fecha com a animação e devolve o foco como antes. `act` devolve se o alerta fecha.
pub fn confirm_alert(window: &mut Window, cx: &mut App, title: String, description: String, ok: String, variant: ButtonVariant,
    act: impl Fn(&mut Window, &mut App) -> bool + 'static) {
    use gpui_kit::{base::actions::{Cancel, Confirm}, component::{WindowExt, dialog::DialogFooter}};
    let act = Rc::new(act);
    let pressed = Rc::new(Cell::new(false));
    // Cada botão despacha a partir de um nó dentro dele, como o rodapé do kit: pelo foco, uma superfície que o
    // tomasse deixaria o botão mudo. O nó não entra na ordem do Tab.
    let (cancel_from, ok_from) = (cx.focus_handle(), cx.focus_handle());
    window.open_alert_dialog(cx, move |alert, _, _| {
        let (act, confirm) = (act.clone(), pressed.clone());
        let (press, cancel_from, ok_from) = (pressed.clone(), cancel_from.clone(), ok_from.clone());
        let anchor = |from: &FocusHandle| div().absolute().size_0().track_focus(from);
        alert.title(SharedString::from(title.clone())).description(SharedString::from(description.clone()))
            .footer(DialogFooter::new()
                .child(Button::new("cancel").label(crate::i18n::tr("cancel")).child(anchor(&cancel_from))
                    .on_click(move |_, window, cx| cancel_from.dispatch_action(&Cancel, window, cx)))
                .child(Button::new("ok").label(ok.clone()).with_variant(variant).child(anchor(&ok_from))
                    .on_click(move |_, window, cx| {
                        press.set(true);
                        ok_from.dispatch_action(&Confirm { secondary: false }, window, cx);
                        press.set(false);
                    })))
            .on_ok(move |event, window, cx| {
                if confirm.take() { act(window, cx) } else { super::machines::enter_to_focused(event, window, cx) }
            })
    });
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

/// O chip "Trabalhando" da lista com a marca animada no lugar do ponto; o resto igual ao `state_chip`.
pub fn working_chip(key: impl Into<ElementId>, label: String) -> AnyElement {
    let (bg, fg) = theme::pill("working");
    div().flex_shrink_0().h(px(22.)).px(px(9.)).flex().items_center().gap(px(6.)).rounded_full().bg(bg).text_color(fg)
        .text_size(px(12.)).font_weight(FontWeight::MEDIUM)
        .child(WorkingMark::new(key, 12., fg)).child(label).into_any_element()
}

pub fn meter(pct: f64) -> AnyElement {
    let color = meter_color(pct);
    div().h(px(4.)).w_full().rounded_full().bg(theme::raised())
        .child(div().h_full().rounded_full().bg(color).w(relative((pct.clamp(0., 100.) / 100.) as f32)))
        .into_any_element()
}

/// Anel de uso do rodapé (16 px, traço 1,8), com a cor do `meter`. Só quads: o `PathBuilder` soma alfa na janela
/// transparente. O trilho é um quad só de borda; o arco, pontos redondos opacos que se cobrem. Sem dado, só o trilho.
pub fn ring(pct: Option<f64>) -> AnyElement {
    const SIZE: f32 = 16.;
    const STROKE: f32 = 1.8;
    let track = theme::raised();
    let arc = pct.map(|pct| (pct.clamp(0., 100.) as f32 / 100., meter_color(pct)));
    div().size(px(SIZE)).flex_shrink_0().child(canvas(|_, _, _| (), move |bounds, _, window, _| {
        window.paint_quad(outline(bounds, track, BorderStyle::Solid).border_widths(px(STROKE)).corner_radii(px(SIZE / 2.)));
        let Some((share, color)) = arc.filter(|(share, _)| *share > 0.) else { return };
        let (center, radius) = (bounds.center(), (SIZE - STROKE) / 2.);
        // Um ponto a cada meio pixel de arco, do topo em sentido horário.
        let steps = (std::f32::consts::TAU * radius * share / 0.5).ceil().max(1.) as usize;
        for step in 0..=steps {
            let angle = std::f32::consts::TAU * share * step as f32 / steps as f32 - std::f32::consts::FRAC_PI_2;
            let at = point(center.x + px(radius * angle.cos()), center.y + px(radius * angle.sin()));
            window.paint_quad(fill(Bounds::centered_at(at, size(px(STROKE), px(STROKE))), color).corner_radii(px(STROKE / 2.)));
        }
    }).size_full()).into_any_element()
}

fn meter_color(pct: f64) -> Hsla {
    if pct >= 90. { theme::danger() } else if pct >= 70. { theme::warning() } else { theme::accent() }
}

/// Marca do Hangar (dois arcos), tingida pela cor do estado.
pub fn hangar_mark(size: f32, color: Hsla) -> Svg {
    svg().path(crate::HANGAR_MARK).size(px(size)).flex_shrink_0().text_color(color)
}

pub fn small_icon(icon: IconName, size: f32, color: Hsla) -> Icon {
    Icon::new(icon).size(px(size)).text_color(color)
}
