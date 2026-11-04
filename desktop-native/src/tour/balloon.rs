//! Desenho do balão preso à peça: o resto da janela escurecido com recorte, anel pulsando em volta da peça, o balão
//! sólido com a seta, e a comemoração do passo feito. O escurecido não tem área de clique: o clique, o arrasto e a tecla
//! chegam à peça real. Os botões do balão não tiram o foco do campo de mensagem (o Shift+Tab só vale com ele).
use std::{cell::{Cell, RefCell}, rc::Rc, sync::OnceLock, time::{Duration, Instant}};
use gpui_kit::{component::text::TextView, prelude::FluentBuilder, *};
use crate::{i18n::tr, motion, theme};
use super::{CELEBRATION, Choice, Kind, Surface};

pub type OnChoice = Rc<dyn Fn(Choice, &mut Window, &mut App)>;

const WIDTH: f32 = 320.;
/// Altura típica do balão, para escolher o lado antes de ele ter tamanho.
const EST_HEIGHT: f32 = 200.;
/// Distância entre a peça e o balão; a seta ocupa ela.
const GAP: f32 = 14.;
const MARGIN: f32 = 12.;
/// O balão menos o texto da novidade: respiros, cabeçalho, título e rodapé.
const NEWS_CHROME: f32 = 140.;
pub(crate) const ARROW_W: f32 = 18.;
pub(crate) const ARROW_H: f32 = 9.;
/// Folga do recorte em volta da peça.
const HOLE_PAD: f32 = 6.;
/// Ritmo do pulso enquanto espera: o suficiente para o anel respirar, sem desenhar a raiz a cada quadro.
const PULSE_STEP: Duration = Duration::from_millis(50);
/// Quanto o pulso dura sem resposta; depois o anel fica parado até a pessoa mexer, voltar à janela ou o passo mudar.
const PULSE_FOR: Duration = Duration::from_secs(6);
/// O check se desenha nesta fração da comemoração; o resto é confete.
const CHECK_DRAW: f32 = 0.3;

// Escuro e sólido em qualquer tema, como no mock aprovado: o balão fica sobre a conversa e não pode deixar ler o que
// passa atrás. As partes `pub(crate)` também desenham a animação da novidade do tour (`news::tour`).
pub(crate) fn fill_color() -> Hsla { rgb(0x1e1e24).into() }
pub(crate) fn border_color() -> Hsla { hsla(0., 0., 1., 0.12) }
pub(crate) fn text_color() -> Hsla { hsla(0., 0., 1., 0.95) }
pub(crate) fn muted_color() -> Hsla { hsla(0., 0., 1., 0.66) }
fn faint_color() -> Hsla { hsla(0., 0., 1., 0.22) }
pub(crate) fn green() -> Hsla { rgb(0x34c759).into() }

/// Onde o balão fica: à esquerda `left`, com `width`, abaixo da peça (`top`) ou acima (`bottom`, contado do pé da
/// janela), e a seta a `arrow_x` da borda esquerda do balão.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Placed { left: f32, width: f32, top: Option<f32>, bottom: Option<f32>, arrow_x: f32 }

fn place(target: Bounds<Pixels>, viewport: Size<Pixels>) -> Placed {
    let (vw, vh) = (f32::from(viewport.width), f32::from(viewport.height));
    let (top, bottom) = (f32::from(target.top()), f32::from(target.bottom()));
    // Acima sempre que couber: abaixo das peças do meio fica o compositor, que o balão cobriria.
    let below = !(top >= EST_HEIGHT + GAP + MARGIN || top > vh - bottom);
    // Janela estreita: o balão encolhe para os botões do rodapé não saírem pela direita.
    let width = WIDTH.min(vw - 2. * MARGIN).max(ARROW_W + 32.);
    let center = f32::from(target.center().x);
    let left = (center - width / 2.).min(vw - width - MARGIN).max(MARGIN);
    let arrow_x = (center - left - ARROW_W / 2.).clamp(16., width - 16. - ARROW_W);
    if below { Placed { left, width, top: Some(bottom + GAP), bottom: None, arrow_x } }
    else { Placed { left, width, top: None, bottom: Some(vh - top + GAP), arrow_x } }
}

thread_local! {
    /// O redesenho já marcado: cada quadro só marca outro se o dele cair antes.
    static NEXT_REPAINT: Cell<Option<Instant>> = const { Cell::new(None) };
    /// Balão, posição do mouse e janela ativa do último quadro, e desde quando o pulso corre.
    static PULSE_SINCE: RefCell<Option<(String, Point<Pixels>, bool, Instant)>> = const { RefCell::new(None) };
}

/// Pulsa só com a janela ativa e por `PULSE_FOR` desde a última mudança: outro balão, o mouse mexeu ou a janela voltou.
/// Sem isso, balão aberto com a pessoa longe redesenhava a raiz para sempre.
fn pulsing(id: &str, window: &Window) -> bool {
    let (mouse, active) = (window.mouse_position(), window.is_window_active());
    PULSE_SINCE.with_borrow_mut(|last| {
        let since = match last.take() {
            Some((seen, at, was_active, since)) if seen == id && at == mouse && (was_active || !active) => since,
            _ => Instant::now(),
        };
        *last = Some((id.to_owned(), mouse, active, since));
        active && since.elapsed() < PULSE_FOR
    })
}

/// Redesenha só a raiz daqui a `delay`. Só é marcado enquanto o balão é desenhado: sumiu, para.
fn repaint_in(delay: Duration, window: &mut Window, cx: &mut App) {
    let at = Instant::now() + delay;
    if NEXT_REPAINT.get().is_some_and(|next| next > Instant::now() && next <= at) { return; }
    NEXT_REPAINT.set(Some(at));
    window.spawn(cx, async move |cx| {
        cx.background_executor().timer(delay).await;
        let _ = cx.update(|_, cx| { motion::tick_root(cx); });
    }).detach();
}

/// Fase do pulso (0 a 1), num relógio só para o anel e o ponto da espera.
fn pulse_phase() -> f32 {
    static START: OnceLock<Instant> = OnceLock::new();
    (START.get_or_init(Instant::now).elapsed().as_secs_f32() / motion::PULSE.as_secs_f32()).fract()
}

/// A camada do balão sobre a janela inteira. `target` é a posição da peça no quadro anterior.
pub fn render(surface: &Surface, target: Bounds<Pixels>, on_choice: OnChoice, window: &mut Window, cx: &mut App) -> AnyElement {
    let still = cx.reduce_motion();
    let entered = motion::enter(SharedString::from(format!("tour-balloon-{}", surface.id)), motion::FADE_IN, window, cx);
    let celebrating = surface.celebrating.map(|at| (at.elapsed().as_secs_f32() / CELEBRATION.as_secs_f32()).min(1.));
    let pulse = !still && celebrating.is_none() && pulsing(&surface.id, window);
    match (celebrating, still) {
        (Some(_), false) => motion::request_frame(window, cx),
        // Parado, a comemoração só precisa de um quadro no fim dela.
        (Some(_), true) => repaint_in(surface.celebrating.map_or(Duration::ZERO, |at| CELEBRATION.saturating_sub(at.elapsed())), window, cx),
        (None, _) if pulse => repaint_in(PULSE_STEP, window, cx),
        (None, _) => {}
    }
    let wave = pulse.then(|| motion::pulse_wave(pulse_phase()));
    let dim = if surface.kind == Kind::Tip { 0. } else { 0.5 * entered };
    let ring = if celebrating.is_some() { green() } else { theme::accent() };
    let spotlight = canvas(|_, _, _| (), move |bounds, _, window, _| paint_spotlight(bounds, target, dim, ring, wave, window))
        .absolute().inset_0();
    let placed = place(target, window.viewport_size());
    let below = placed.top.is_some();
    // O lado é escolhido pela altura típica; novidade longa em janela curta rola dentro do balão, sem tirar o título
    // e o Fechar da janela.
    let vh = f32::from(window.viewport_size().height);
    let news_room = (placed.top.or(placed.bottom).map_or(vh, |edge| vh - edge) - MARGIN - NEWS_CHROME).max(60.);
    let content = div().flex().flex_col().gap(px(10.))
        .child(header(surface))
        .map(|el| match celebrating {
            Some(progress) => el.child(celebration(progress, still)),
            None => el.child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(text_color()).child(surface.title.clone()))
                .child(div().text_size(px(13.)).line_height(relative(1.45)).text_color(muted_color()).whitespace_normal()
                    // A novidade traz o texto do cartão, escrito em Markdown; tour e dicas são texto simples.
                    .map(|el| if surface.kind == Kind::News {
                        el.child(div().id("tour-news-scroll").max_h(px(news_room)).overflow_y_scroll()
                            .child(TextView::markdown(SharedString::from(format!("tour-news-body-{}", surface.id)), surface.body.clone()).scrollable(false)))
                    } else { el.child(surface.body.clone()) }))
                .when(surface.kind == Kind::Tour, |el| el.child(waiting(wave)))
                .child(footer(surface.kind, on_choice)),
        });
    let balloon = div().id("tour-balloon").absolute().left(px(placed.left)).w(px(placed.width))
        .map(|el| match (placed.top, placed.bottom) { (Some(top), _) => el.top(px(top)), (_, Some(bottom)) => el.bottom(px(bottom)), _ => el })
        .p(px(16.)).rounded(px(14.)).bg(fill_color()).border_1().border_color(border_color())
        .shadow(vec![BoxShadow { color: hsla(0., 0., 0., 0.45), offset: point(px(0.), px(12.)), blur_radius: px(36.), spread_radius: px(0.), inset: false }])
        .opacity(entered).occlude()
        .role(Role::Dialog).aria_label(surface.title.clone()).aria_description(surface.body.clone())
        // O clique em qualquer parte do balão deixa o foco onde está.
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        .child(content)
        .child(arrow(below).absolute().left(px(placed.arrow_x)).map(|el| if below { el.top(px(-ARROW_H)) } else { el.bottom(px(-ARROW_H)) }));
    div().absolute().inset_0().child(spotlight).child(balloon).into_any_element()
}

/// Escurecido em quatro faixas em volta do recorte (a GPUI não tem máscara) e o anel na borda dele, com uma segunda
/// volta que cresce e some no pulso.
pub(crate) fn paint_spotlight(window_bounds: Bounds<Pixels>, target: Bounds<Pixels>, dim: f32, ring: Hsla, wave: Option<f32>, window: &mut Window) {
    let hole = target.dilate(px(HOLE_PAD));
    if dim > 0. {
        let veil = hsla(0., 0., 0., dim);
        let (l, t, r, b) = (window_bounds.left(), window_bounds.top(), window_bounds.right(), window_bounds.bottom());
        let (hl, ht, hr, hb) = (hole.left().max(l), hole.top().max(t), hole.right().min(r), hole.bottom().min(b));
        for (a, z) in [(point(l, t), point(r, ht)), (point(l, hb), point(r, b)), (point(l, ht), point(hl, hb)), (point(hr, ht), point(r, hb))] {
            if z.x > a.x && z.y > a.y { window.paint_quad(fill(Bounds::from_corners(a, z), veil)); }
        }
    }
    window.paint_quad(outline(hole, ring, BorderStyle::Solid).border_widths(px(2.)).corner_radii(px(8.)));
    if let Some(w) = wave {
        let outer = hole.dilate(px(2. + 6. * w));
        window.paint_quad(outline(outer, ring.opacity(0.6 * (1. - w)), BorderStyle::Solid).border_widths(px(2.)).corner_radii(px(8. + 6. * w)));
    }
}

/// Triângulo da seta na cor do balão, com a borda dele; a base entra 1 px no balão e cobre a borda ali.
pub(crate) fn arrow(up: bool) -> Canvas<()> {
    canvas(|_, _, _| (), move |b, _, window, _| {
        let (l, r, cx) = (b.left(), b.right(), b.center().x);
        for (color, inset) in [(border_color(), 0.), (fill_color(), 1.4)] {
            let inset = px(inset);
            let mut path = PathBuilder::fill();
            if up {
                path.move_to(point(l + inset, b.bottom()));
                path.line_to(point(cx, b.top() + inset));
                path.line_to(point(r - inset, b.bottom()));
            } else {
                path.move_to(point(l + inset, b.top()));
                path.line_to(point(cx, b.bottom() - inset));
                path.line_to(point(r - inset, b.top()));
            }
            path.close();
            if let Ok(path) = path.build() { window.paint_path(path, color); }
        }
    }).w(px(ARROW_W)).h(px(ARROW_H + 1.))
}

/// Pontos do progresso e "2 de 9" no tour; etiqueta amarela na dica, roxa na novidade.
pub(crate) fn header(surface: &Surface) -> Div {
    let row = div().flex().items_center().gap(px(6.));
    match (surface.kind, surface.progress) {
        (Kind::Tour, Some((at, total))) => row
            .children((1..=total).map(|n| div().size(px(6.)).rounded_full()
                .bg(if n == at { theme::accent() } else if n < at { muted_color() } else { faint_color() })))
            .child(div().flex_1())
            .child(div().text_size(px(11.)).text_color(muted_color())
                .child(tr("tour_progress").replace("{n}", &at.to_string()).replace("{total}", &total.to_string()))),
        (Kind::Tip, _) => row.child(tag(tr("tour_tip_tag"), rgb(0xf5b83d).into(), rgb(0x2a1d00).into())),
        (Kind::News, _) => row.child(tag(tr("tour_news_tag"), rgb(0x8b5cf6).into(), hsla(0., 0., 1., 1.))),
        (Kind::Tour, None) => row,
    }
}

fn tag(text: String, bg: Hsla, color: Hsla) -> Div {
    div().px(px(7.)).py(px(1.)).rounded(px(6.)).bg(bg).text_color(color).text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).child(text)
}

/// "Esperando você…" com o ponto pulsando.
pub(crate) fn waiting(wave: Option<f32>) -> Div {
    let alpha = wave.map_or(1., |w| 0.35 + 0.65 * w);
    div().flex().items_center().gap(px(8.)).text_size(px(12.)).text_color(muted_color())
        .child(div().size(px(7.)).rounded_full().bg(theme::accent().opacity(alpha)))
        .child(tr("tour_waiting"))
}

fn footer(kind: Kind, on_choice: OnChoice) -> Div {
    let row = div().flex().items_center().justify_end().gap(px(6.)).pt(px(2.));
    match kind {
        Kind::Tour => row.child(button("tour-skip-all", tr("tour_skip_all"), false, Choice::SkipTour, on_choice.clone()))
            .child(div().flex_1())
            .child(button("tour-later", tr("tour_later"), false, Choice::Later, on_choice.clone()))
            .child(button("tour-skip", tr("tour_skip"), true, Choice::Skip, on_choice)),
        Kind::Tip => row.child(button("tour-tips-off", tr("tour_tips_off"), false, Choice::TipsOff, on_choice.clone()))
            .child(button("tour-tip-ok", tr("tour_tip_ok"), true, Choice::TipOk, on_choice)),
        Kind::News => row.child(button("tour-news-close", tr("tour_news_close"), true, Choice::Close, on_choice)),
    }
}

/// Botão sem foco próprio: o `prevent_default` no `mousedown` deixa o foco no campo onde ele estava.
fn button(id: &'static str, label: String, primary: bool, choice: Choice, on_choice: OnChoice) -> Stateful<Div> {
    let on_a11y = on_choice.clone();
    div().id(id).px(px(10.)).py(px(5.)).rounded(px(8.)).cursor_pointer().text_size(px(12.5)).font_weight(FontWeight::MEDIUM)
        .map(|el| if primary { el.bg(theme::accent()).text_color(hsla(0., 0., 1., 1.)).hover(|el| el.bg(theme::accent_press())) }
            else { el.text_color(muted_color()).hover(|el| el.bg(hsla(0., 0., 1., 0.08)).text_color(text_color())) })
        .role(Role::Button).aria_label(label.clone())
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        .on_click(move |_, window, cx| on_choice(choice, window, cx))
        .on_a11y_action(AccessibleAction::Click, move |_, window, cx| on_a11y(choice, window, cx))
        .child(label)
}

/// O check se desenhando num círculo verde e o confete em volta; `progress` vai de 0 a 1 na comemoração.
pub(crate) fn celebration(progress: f32, still: bool) -> Div {
    let check = if still { 1. } else { (progress / CHECK_DRAW).min(1.) };
    let confetti = (!still).then_some(progress);
    let size = 36.;
    let spread = 150.;
    div().flex().items_center().gap(px(12.)).py(px(6.))
        .child(div().relative().size(px(size)).flex_none()
            .child(canvas(|_, _, _| (), move |b, _, window, _| {
                window.paint_quad(fill(b, green()).corner_radii(px(size / 2.)));
                paint_check(b, check, window);
            }).absolute().inset_0())
            .when_some(confetti, |el, progress| el.child(canvas(|_, _, _| (), move |b, _, window, _| paint_confetti(b, progress, window))
                .absolute().left(px((size - spread) / 2.)).top(px((size - spread) / 2.)).size(px(spread)))))
        .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(text_color()).child(tr("tour_nice")))
}

/// O traço do check percorre os dois segmentos na ordem, até a fração `progress` do comprimento.
fn paint_check(b: Bounds<Pixels>, progress: f32, window: &mut Window) {
    if progress <= 0. { return; }
    let s = f32::from(b.size.width) / 28.;
    let at = |(x, y): (f32, f32)| point(b.origin.x + px(x * s), b.origin.y + px(y * s));
    let points = [(8.5, 14.5), (12.5, 18.5), (20., 10.)];
    let length = |a: (f32, f32), z: (f32, f32)| ((z.0 - a.0).powi(2) + (z.1 - a.1).powi(2)).sqrt();
    let (first, second) = (length(points[0], points[1]), length(points[1], points[2]));
    let mut left = (first + second) * progress;
    let width = 2.6 * s;
    let options = StrokeOptions::default().with_line_width(width).with_line_cap(LineCap::Round);
    let mut path = PathBuilder::stroke(px(width)).with_style(PathStyle::Stroke(options));
    path.move_to(at(points[0]));
    for (a, z, len) in [(points[0], points[1], first), (points[1], points[2], second)] {
        if left <= 0. { break; }
        let f = (left / len).min(1.);
        path.line_to(at((a.0 + (z.0 - a.0) * f, a.1 + (z.1 - a.1) * f)));
        left -= len;
    }
    if let Ok(path) = path.build() { window.paint_path(path, hsla(0., 0., 1., 1.)); }
}

/// Pedacinhos que saem do centro, caem com gravidade e somem até o fim da comemoração.
fn paint_confetti(b: Bounds<Pixels>, progress: f32, window: &mut Window) {
    const COLORS: [u32; 6] = [0x34c759, 0xf5b83d, 0x8b5cf6, 0x3b9fe0, 0xec4899, 0xe0563b];
    const PIECES: usize = 16;
    let t = progress * CELEBRATION.as_secs_f32();
    let fade = 1. - progress * progress;
    let center = b.center();
    for i in 0..PIECES {
        // Espalhamento fixo por peça: o mesmo confete a cada quadro.
        let jitter = ((i * 7919) % 97) as f32 / 97.;
        let angle = std::f32::consts::TAU * (i as f32 + jitter * 0.6) / PIECES as f32;
        let speed = 70. + 50. * jitter;
        let (x, y) = (speed * t * angle.cos(), speed * t * angle.sin() + 90. * t * t);
        let color: Hsla = rgb(COLORS[i % COLORS.len()]).into();
        let piece = Bounds::centered_at(point(center.x + px(x), center.y + px(y)), size(px(4.), px(6.)));
        window.paint_quad(fill(piece, color.opacity(fade)).corner_radii(px(1.)));
    }
}

#[cfg(test)]
mod tests {
    use super::{ARROW_W, GAP, MARGIN, WIDTH, place};
    use gpui_kit::{Bounds, point, px, size};

    fn at(x: f32, y: f32, w: f32, h: f32) -> Bounds<gpui_kit::Pixels> { Bounds::new(point(px(x), px(y)), size(px(w), px(h))) }

    #[test]
    fn balloon_goes_above_when_it_fits_and_below_near_the_top() {
        let viewport = size(px(1200.), px(800.));
        let top_piece = place(at(500., 40., 100., 30.), viewport);
        assert_eq!(top_piece.top, Some(70. + GAP));
        let composer = place(at(300., 700., 600., 80.), viewport);
        assert_eq!((composer.top, composer.bottom), (None, Some(800. - 700. + GAP)));
        // O painel "Na fila" fica logo acima do compositor: o balão vai para cima dele, não sobre o campo.
        let queue = place(at(30., 450., 400., 40.), viewport);
        assert_eq!((queue.top, queue.bottom), (None, Some(800. - 450. + GAP)));
    }

    #[test]
    fn balloon_stays_inside_the_window_and_the_arrow_points_at_the_piece() {
        let viewport = size(px(1200.), px(800.));
        let left_edge = place(at(4., 100., 24., 24.), viewport);
        assert_eq!(left_edge.left, MARGIN);
        assert!(left_edge.arrow_x >= 16. && left_edge.arrow_x + ARROW_W / 2. < 40.);
        let right_edge = place(at(1170., 100., 24., 24.), viewport);
        assert_eq!(right_edge.left, 1200. - WIDTH - MARGIN);
        assert_eq!(right_edge.arrow_x, WIDTH - 16. - ARROW_W);
        let middle = place(at(590., 100., 20., 20.), viewport);
        assert_eq!(middle.left + middle.arrow_x + ARROW_W / 2., 600.);
        assert_eq!(middle.width, WIDTH);
    }

    #[test]
    fn a_narrow_window_shrinks_the_balloon_inside_it() {
        let narrow = place(at(200., 100., 24., 24.), size(px(300.), px(800.)));
        assert_eq!((narrow.left, narrow.width), (MARGIN, 300. - 2. * MARGIN));
        assert!(narrow.arrow_x >= 16. && narrow.arrow_x <= narrow.width - 16. - ARROW_W);
    }
}
