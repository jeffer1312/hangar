// Peças visuais repetidas entre compositor, lista e painel: a medida de cada uma é a do web.
use gpui_kit::{component::{ActiveTheme, Icon, Sizable, StyledExt, button::*}, prelude::FluentBuilder, *};
use gpui_kit::assets::IconName;
use crate::theme;
use crate::appearance::{self, SurfaceMaterial};
pub use crate::motion::ease_out;
use std::{cell::Cell, rc::Rc, sync::OnceLock, time::{Duration, Instant}};

/// Batida das animações que se repetem: 15 por segundo, numa grade de tempo comum a todas. Cada batida redesenha a raiz,
/// que pinta as marcas flutuantes, então a cadência é o custo da sessão trabalhando. Relógios próprios,
/// fora de fase entre si, somariam quadros; na grade, as views que batem juntas saem num quadro só.
const PULSE_TICK: Duration = Duration::from_micros(66_667);

thread_local! {
    /// A janela tem o foco. Sem ele o relógio para: a marca fica parada no último quadro e nada acorda a janela.
    static WINDOW_ACTIVE: Cell<bool> = const { Cell::new(true) };
    /// Adaptador por software (WARP no RDP sem GPU): cada batida rasteriza a janela inteira na CPU, então o relógio para.
    static SOFTWARE_GPU: Cell<bool> = const { Cell::new(false) };
}

pub fn set_window_active(active: bool) { WINDOW_ACTIVE.set(active); }
pub fn set_software_gpu(software: bool) { SOFTWARE_GPU.set(software); }
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

/// Redesenha a view a cada batida da grade, a partir de `delay`, até ela sair da tela. Com movimento reduzido, com a
/// janela sem foco, com desenho por software, ou com `awake` dizendo que ela não está à vista, não redesenha.
fn pulse<V: 'static>(delay: Duration, awake: fn(&V) -> bool, cx: &mut Context<V>) {
    cx.spawn(async move |view, cx| {
        cx.background_executor().timer(delay).await;
        // Um desenho no fim da espera mesmo com movimento reduzido: o esqueleto nasce vazio e, sem ele, ficava vazio.
        if view.update(cx, |_, cx| cx.notify()).is_err() { return; }
        loop {
            let into = Duration::from_nanos((pulse_epoch().elapsed().as_nanos() % PULSE_TICK.as_nanos()) as u64);
            cx.background_executor().timer(PULSE_TICK - into).await;
            if view.update(cx, |view, cx| if !cx.reduce_motion() && WINDOW_ACTIVE.get() && !SOFTWARE_GPU.get() && awake(view) { cx.notify() }).is_err() { break; }
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

/// O `Spinner` do kit (mesmo ícone, giro de 0,8 s com `ease_in_out`) no relógio comum, numa view própria
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

/// O `Skeleton` do kit (mesma cor) com o `zeron-pulse` do kit de movimento (2,4 s, em onda), no relógio comum,
/// numa view própria que só aparece depois de `SKELETON_DELAY`; até lá ocupa o mesmo espaço, vazio. Em lista, cada
/// linha anda um pouco atrás da de cima.
#[derive(IntoElement)]
pub struct Skeleton { key: ElementId, style: StyleRefinement, secondary: bool, lag: f32 }

impl Skeleton {
    pub fn new(key: impl Into<ElementId>) -> Self { Self { key: key.into(), style: StyleRefinement::default(), secondary: false, lag: 0. } }
    pub fn secondary(mut self) -> Self { self.secondary = true; self }
    /// A linha `row` de uma lista de esqueletos: a onda desce por elas.
    pub fn row(mut self, row: usize) -> Self { self.lag = row as f32 * 0.08; self }
}

impl Styled for Skeleton {
    fn style(&mut self) -> &mut StyleRefinement { &mut self.style }
}

struct SkeletonView { style: StyleRefinement, secondary: bool, born: Instant, lag: f32 }

impl Render for SkeletonView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let base = div().w_full().h_4().refine_style(&self.style);
        if self.born.elapsed() < SKELETON_DELAY { return base; }
        let color = if self.secondary { cx.theme().skeleton.opacity(0.5) } else { cx.theme().skeleton };
        let wave = if cx.reduce_motion() { 0. } else { crate::motion::pulse_wave((pulse_phase(crate::motion::PULSE) - self.lag).rem_euclid(1.)) };
        base.bg(color).opacity(1.0 - wave * 0.5)
    }
}

impl RenderOnce for Skeleton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (style, secondary, lag) = (self.style, self.secondary, self.lag);
        let view = keyed_view(self.key, window, cx, |cx| {
            pulse(SKELETON_DELAY, |_| true, cx);
            SkeletonView { style: style.clone(), secondary, born: Instant::now(), lag }
        });
        view.update(cx, |view, cx| {
            if view.style != style || view.secondary != secondary { (view.style, view.secondary) = (style.clone(), secondary); cx.notify(); }
        });
        view.cached(style)
    }
}

/// Ícone que respira (o `breathe` do botão Atividade do web: opacidade 0,55 → 1 e escala 0,92 → 1,05 em 1,5 s), no
/// relógio comum, numa view própria; inteiro e parado com movimento reduzido.
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
    // Os três arcos num path só, com as pontas redondas do próprio traço (o `stroke-linecap="round"` do web): um lote de
    // path por quadro. Cada lote rasteriza numa textura do tamanho da janela.
    let options = StrokeOptions::default().with_line_width(width).with_line_cap(LineCap::Round);
    let mut path = PathBuilder::stroke(px(width)).with_style(PathStyle::Stroke(options));
    let mut any = false;
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
        path.move_to(at(start));
        for step in 1..=steps { path.line_to(at(start + sweep * step as f32 / steps as f32)); }
        any = true;
    }
    if any && let Ok(path) = path.build() { window.paint_path(path, color); }
}

/// A marca animada "trabalhando" (três arcos: entrada que se desenha, onda de giro e respiro), no relógio comum,
/// numa view própria guardada entre quadros. Fora da área visível (barra rolada, aba fora da faixa) para de
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
pub struct Elapsed { key: ElementId, since: Instant, suffix: Option<SharedString> }

impl Elapsed {
    pub fn new(key: impl Into<ElementId>, since: Instant) -> Self { Self { key: key.into(), since, suffix: None } }
    /// Texto depois dos segundos, separado por " · ".
    pub fn suffix(mut self, suffix: Option<SharedString>) -> Self { self.suffix = suffix; self }
}

struct ElapsedView { since: Instant, suffix: Option<SharedString> }

impl Render for ElapsedView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let elapsed = format_elapsed(self.since.elapsed());
        div().size_full().min_w_0().flex().items_center().pt(px(1.)).text_size(px(11.)).text_color(theme::faint())
            .child(div().min_w_0().truncate().child(match &self.suffix { Some(suffix) => format!("{elapsed} · {suffix}"), None => elapsed }))
    }
}

impl RenderOnce for Elapsed {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (since, suffix) = (self.since, self.suffix);
        let view = keyed_view(self.key, window, cx, |cx| {
            cx.spawn(async move |view, cx| loop {
                let Ok(since) = view.read_with(cx, |view: &ElapsedView, _| view.since) else { break };
                let into = Duration::from_nanos((since.elapsed().as_nanos() % 1_000_000_000) as u64);
                // Vira o segundo na batida seguinte da grade comum: com a marca animando, sai no mesmo quadro que ela.
                let turn = Duration::from_secs(1) - into;
                let off = (pulse_epoch().elapsed() + turn).as_nanos() % PULSE_TICK.as_nanos();
                let turn = turn + Duration::from_nanos(((PULSE_TICK.as_nanos() - off) % PULSE_TICK.as_nanos()) as u64);
                cx.background_executor().timer(turn).await;
                // Sem foco o segundo não vira: ninguém olha, e cada virada redesenha a raiz.
                if view.update(cx, |_, cx| if WINDOW_ACTIVE.get() { cx.notify() }).is_err() { break; }
            }).detach();
            ElapsedView { since, suffix: suffix.clone() }
        });
        // O começo vem de um horário de parede convertido a cada desenho e varia em microssegundos: só um salto conta.
        view.update(cx, |view, cx| {
            if view.suffix != suffix { view.suffix = suffix; cx.notify(); }
            if since.saturating_duration_since(view.since).max(view.since.saturating_duration_since(since)) > Duration::from_millis(500) {
                view.since = since;
                cx.notify();
            }
        });
        view.cached(StyleRefinement::default().size_full())
    }
}

/// Texto apagado com uma faixa clara passando (o brilho do título do grupo que roda, como no Zeron), numa view própria
/// no relógio comum; fora da área visível para de pedir quadro, e com movimento reduzido fica parado.
#[derive(IntoElement)]
pub struct Shimmer { key: ElementId, text: SharedString }

impl Shimmer {
    pub fn new(key: impl Into<ElementId>, text: SharedString) -> Self { Self { key: key.into(), text } }
}

struct ShimmerView { text: SharedString, born: Instant, visible: Rc<Cell<bool>> }

impl Render for ShimmerView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (base, lit) = (theme::muted(), theme::text());
        let chars = self.text.chars().count().max(1) as f32;
        // Cor por letra pela posição no título: sem medir glifo, e a faixa anda por fração da largura.
        let highlights: Vec<_> = if cx.reduce_motion() { Vec::new() } else {
            let phase = (self.born.elapsed().as_secs_f32() / crate::motion::TOOL_SHIMMER.as_secs_f32()).fract();
            self.text.char_indices().enumerate().filter_map(|(n, (at, ch))| {
                let amount = crate::motion::shimmer_amount((n as f32 + 0.5) / chars, phase);
                (amount > 0.).then(|| (at..at + ch.len_utf8(), HighlightStyle { color: Some(base.blend(lit.alpha(amount))), ..Default::default() }))
            }).collect()
        };
        let visible = self.visible.clone();
        div().size_full().min_w_0().flex().items_center().text_sm().text_color(base)
            .child(div().min_w_0().truncate().child(StyledText::new(self.text.clone()).with_highlights(highlights)))
            .child(canvas(move |bounds, window, _| visible.set(window.content_mask().bounds.intersects(&bounds)), |_, _, _, _| {})
                .absolute().inset_0())
    }
}

impl RenderOnce for Shimmer {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let text = self.text;
        let view = keyed_view(self.key, window, cx, |cx| {
            pulse(Duration::ZERO, |view: &ShimmerView| view.visible.get(), cx);
            ShimmerView { text: text.clone(), born: Instant::now(), visible: Rc::new(Cell::new(true)) }
        });
        view.update(cx, |view, cx| if view.text != text { view.text = text; cx.notify(); });
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

/// Selo quadrado do provider na cor da marca, com os mesmos traçados do `ProviderGlyph` do web;
/// Pi e omp não têm marca vetorial e ficam no glifo tipográfico (π, Ω), como lá.
pub fn provider_glyph(provider: &str, size: f32) -> Div {
    let (color, glyph) = theme::provider(provider);
    let seal = div().size(px(size)).flex_shrink_0().rounded(px(4.)).bg(color.opacity(0.2)).flex().items_center().justify_center()
        .text_color(color);
    match provider_logo(provider, size * 0.72, color) {
        Some(logo) => seal.child(logo),
        None => seal.text_size(px(10.)).font_weight(FontWeight::BOLD).child(glyph),
    }
}

/// O selo do provider no canto de cima da marca da linha, quando a lista mistura providers.
pub fn provider_badge(provider: &str) -> Div {
    div().absolute().left(px(-4.)).top(px(-4.)).p(px(1.)).rounded_full()
        .bg(theme::raised()).border_1().border_color(theme::border()).child(provider_glyph(provider, 10.))
}

/// Logo vetorial do provider (Claude, Codex/OpenAI, Kimi); `None` para quem não tem marca.
pub fn provider_logo(provider: &str, size: f32, color: Hsla) -> Option<Svg> {
    let path = match provider { "claude" => "providers/claude.svg", "codex" => "providers/codex.svg", "kimi" => "providers/kimi.svg", _ => return None };
    Some(svg().path(path).size(px(size)).flex_shrink_0().text_color(color))
}

pub(super) const GLASS_BLUR: Pixels = px(16.);

/// O cartão pinta o desfoque antes do seu conteúdo, sobre o que já foi desenhado atrás dele.
pub fn paint_glass(bounds: Bounds<Pixels>, radius: Pixels, window: &mut Window) {
    window.paint_layer(bounds, |window| window.paint_backdrop_blur(bounds, Corners::all(radius), GLASS_BLUR));
}

pub struct Glass { child: AnyElement, radius: Pixels }

impl Glass {
    pub fn new(child: impl IntoElement, radius: Pixels) -> Self { Self { child: child.into_any_element(), radius } }
}

impl IntoElement for Glass {
    type Element = Self;
    fn into_element(self) -> Self { self }
}

impl Element for Glass {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> { None }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> { None }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) {
        self.child.prepaint(window, cx);
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), _: &mut (), window: &mut Window, cx: &mut App) {
        window.paint_layer(bounds, |window| {
            window.paint_backdrop_blur(bounds, Corners::all(self.radius), GLASS_BLUR);
            self.child.paint(window, cx);
        });
    }
}

/// Painel grande (barra lateral, painel direito, abas, compositor) com o vidro atrás quando há imagem para borrar.
pub fn glass_panel(panel: impl IntoElement, radius: Pixels) -> AnyElement {
    if theme::panel_glass() { Glass::new(panel, radius).into_any_element() } else { panel.into_any_element() }
}

/// Superfície dos popovers do compositor: mesma borda de vidro e sombra `--elev-2` do web.
pub fn popover(content: AnyElement, narrow: bool) -> AnyElement {
    let surface = div().when(narrow, |el| el.w(px(380.))).rounded(px(12.)).border_1().border_color(theme::glass_border())
        .bg(theme::popup_fill(theme::raised())).shadow(theme::popover_shadow()).child(content);
    if appearance::get().surface_material == SurfaceMaterial::Glass { Glass::new(surface, px(12.)).into_any_element() }
    else { surface.into_any_element() }
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
    let (cancel_focus, ok_focus) = (cx.focus_handle(), cx.focus_handle());
    let initial_focus = cancel_focus.clone();
    window.open_dialog(cx, move |dialog, window, _| {
        let (act, confirm) = (act.clone(), pressed.clone());
        let (press, cancel_from, ok_from) = (pressed.clone(), cancel_from.clone(), ok_from.clone());
        let anchor = |from: &FocusHandle| div().absolute().size_0().track_focus(from);
        let top = super::popup::centered_top(window.viewport_size().height, px(150.));
        super::popup::dialog(dialog).w(px(360.)).close_button(false).margin_top(top)
            .title(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(title.clone()))
            .child(div().text_size(px(13.)).line_height(px(19.)).text_color(theme::muted()).whitespace_normal().child(description.clone()))
            .footer(DialogFooter::new()
                .child(OwnFocus { id: "cancel".into(), focus: Some(cancel_focus.clone()),
                    button: Button::new("cancel").label(crate::i18n::tr("cancel")).child(anchor(&cancel_from))
                        .on_key_down(swap_on_arrows(cancel_focus.clone(), ok_focus.clone()))
                        .on_click(move |_, window, cx| cancel_from.dispatch_action(&Cancel, window, cx)) })
                .child(OwnFocus { id: "ok".into(), focus: Some(ok_focus.clone()),
                    button: Button::new("ok").label(ok.clone()).with_variant(variant).child(anchor(&ok_from))
                        .on_key_down(swap_on_arrows(cancel_focus.clone(), ok_focus.clone()))
                        .on_click(move |_, window, cx| {
                            press.set(true);
                            ok_from.dispatch_action(&Confirm { secondary: false }, window, cx);
                            press.set(false);
                        }) }))
            .on_ok(move |event, window, cx| {
                if confirm.take() { act(window, cx) } else { super::machines::enter_to_focused(event, window, cx) }
            })
    });
    window.on_next_frame(move |window, cx| initial_focus.focus(window, cx));
}

/// As setas trocam o foco entre os dois botões, como o Tab: com dois, ir e voltar dão a mesma volta.
fn swap_on_arrows(a: FocusHandle, b: FocusHandle) -> impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static {
    move |event, window, cx| {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.modified() || !matches!(keystroke.key.as_str(), "left" | "right" | "up" | "down") { return; }
        if a.is_focused(window) { b.focus(window, cx) } else { a.focus(window, cx) }
        cx.stop_propagation();
    }
}

/// O `Button` do kit só usa o foco que ele mesmo guarda no estado com chave pelo id, e não aceita outro. Registrado ali antes do
/// primeiro desenho dele, o foco do dono vira o do botão; o caminho na árvore é o do `FocusOnClick`.
#[derive(IntoElement)]
pub(super) struct OwnFocus {
    /// O mesmo id dado ao `Button`.
    pub(super) id: ElementId,
    pub(super) button: Button,
    pub(super) focus: Option<FocusHandle>,
}

impl RenderOnce for OwnFocus {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        if let Some(focus) = self.focus {
            window.with_id(std::any::type_name::<Button>(), |window| { window.use_keyed_state(self.id, cx, move |_, _| focus); });
        }
        self.button
    }
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
    let color = meter_color(pct);
    div().h(px(4.)).w_full().rounded_full().bg(theme::raised())
        .child(div().h_full().rounded_full().bg(color).w(relative((pct.clamp(0., 100.) / 100.) as f32)))
        .into_any_element()
}

/// Anel de uso do rodapé (caixa de 16 px, anel de 14, traço 2), com a cor do `meter`. Só quads: o `PathBuilder` soma
/// alfa na janela transparente. O trilho é um quad só de borda, na tinta do texto para aparecer sobre qualquer fundo; o
/// arco, pontos redondos opacos que se cobrem. Sem dado, só o trilho.
pub fn ring(pct: Option<f64>) -> AnyElement {
    const SIZE: f32 = 16.;
    const RING: f32 = 14.;
    const STROKE: f32 = 2.;
    let track = theme::text().opacity(0.16);
    let arc = pct.map(|pct| (pct.clamp(0., 100.) as f32 / 100., meter_color(pct)));
    div().size(px(SIZE)).flex_shrink_0().child(canvas(|_, _, _| (), move |bounds, _, window, _| {
        let ring = Bounds::centered_at(bounds.center(), size(px(RING), px(RING)));
        window.paint_quad(outline(ring, track, BorderStyle::Solid).border_widths(px(STROKE)).corner_radii(px(RING / 2.)));
        let Some((share, color)) = arc.filter(|(share, _)| *share > 0.) else { return };
        let (center, radius) = (bounds.center(), (RING - STROKE) / 2.);
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

/// Número ao lado do anel: as faixas do bloco de contexto do painel; sem dado, apagado.
pub fn ring_text(pct: Option<f64>) -> Hsla {
    match pct { Some(p) if p >= 90. => theme::danger(), Some(p) if p >= 70. => theme::warning(), Some(_) => theme::muted(), None => theme::faint() }
}

/// Marca do Hangar (dois arcos), tingida pela cor do estado.
pub fn hangar_mark(size: f32, color: Hsla) -> Svg {
    svg().path(crate::HANGAR_MARK).size(px(size)).flex_shrink_0().text_color(color)
}

/// Sessão sem terminal: o desenho do `SessionSignals` do web (lucide não tem terminal cortado).
pub fn no_terminal_mark(size: f32, color: Hsla) -> Svg {
    svg().path(crate::NO_TERMINAL).size(px(size)).flex_shrink_0().text_color(color)
}

pub fn small_icon(icon: IconName, size: f32, color: Hsla) -> Icon {
    Icon::new(icon).size(px(size)).text_color(color)
}
