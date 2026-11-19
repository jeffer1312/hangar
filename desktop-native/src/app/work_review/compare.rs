//! Cortina de antes e depois (`.lb`/`.cortina` do desenho): as duas imagens uma sobre a outra, a de depois cortada na
//! divisória. A divisória anda pela faixa, pelo arrasto na imagem e, entre as corrigidas, ←/→ trocam o par. O Esc é do
//! diálogo, que devolve o foco a quem abriu.
use super::*;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};

/// Folga do diálogo às bordas da janela e o que ele gasta além do palco (respiro e borda), como no visor.
const EDGE: f32 = 24.;
const CHROME_W: f32 = 34.;
/// Altura da janela que não é palco: título, faixa, navegação e respiros.
const CHROME_H: f32 = 200.;
const MAX_W: f32 = 980.;

pub(super) struct Pair { pub(super) title: String, pub(super) before: String, pub(super) after: String,
    /// Tamanho declarado pelo conector; sem ele, o da imagem de antes.
    pub(super) size: Option<(u32, u32)> }

pub(super) struct Compare {
    hangar: WeakEntity<Hangar>,
    pairs: Vec<Pair>,
    index: usize,
    labels: (String, String),
    /// Comparação de uma decisão: sem navegação entre pares.
    single: bool,
    /// Posição da divisória, de 0 a 100.
    split: f32,
    slider: Entity<SliderState>,
    focus: FocusHandle,
    stage: Option<Bounds<Pixels>>,
    _subscriptions: Vec<Subscription>,
}

/// Palco que cabe na janela mantendo a proporção da imagem.
pub(super) fn stage_size(viewport: (f32, f32), image: (f32, f32)) -> (f32, f32) {
    let w = (MAX_W.min(viewport.0 - EDGE * 2.) - CHROME_W).max(160.);
    let max_h = (viewport.1 - CHROME_H - EDGE * 2.).max(120.);
    let h = w * image.1 / image.0.max(1.);
    if h > max_h { (max_h * image.0 / image.1.max(1.), max_h) } else { (w, h) }
}

/// Posição da divisória (0 a 100) para o ponteiro em `x`, dentro do palco que começa em `left` e tem `width`.
pub(super) fn split_at(x: f32, left: f32, width: f32) -> f32 {
    if width <= 0. { return 50.; }
    ((x - left) / width * 100.).clamp(0., 100.).round()
}

impl Compare {
    /// Ler pelo `update` marca as duas como desenhadas: o teto do cache não as tira da cortina aberta.
    fn images(&self, cx: &mut App) -> (Option<Arc<RenderImage>>, Option<Arc<RenderImage>>) {
        let pair = &self.pairs[self.index];
        self.hangar.upgrade().map(|h| h.update(cx, |h, _| { let p = &mut h.work_review; (p.image(&pair.before, true), p.image(&pair.after, true)) }))
            .unwrap_or_default()
    }

    fn stage(&self, window: &Window, cx: &mut App) -> (f32, f32) {
        let pair = &self.pairs[self.index];
        let image = pair.size.map(|(w, h)| (w as f32, h as f32)).or_else(|| self.images(cx).0.map(|i| {
            let s = i.size(0);
            (s.width.0 as f32, s.height.0 as f32)
        })).unwrap_or((16., 9.));
        let viewport = window.viewport_size();
        stage_size((f32::from(viewport.width), f32::from(viewport.height)), image)
    }

    fn set_split(&mut self, split: f32, window: &mut Window, cx: &mut Context<Self>) {
        if self.split == split { return; }
        self.split = split;
        self.slider.update(cx, |slider, cx| slider.set_value(split, window, cx));
        cx.notify();
    }

    fn drag_to(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(stage) = self.stage else { return };
        self.set_split(split_at(f32::from(at.x), f32::from(stage.origin.x), f32::from(stage.size.width)), window, cx);
    }

    fn step(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.pairs.len();
        if self.single || n < 2 { return; }
        self.index = if forward { (self.index + 1) % n } else { (self.index + n - 1) % n };
        let (before, after) = (self.pairs[self.index].before.clone(), self.pairs[self.index].after.clone());
        let _ = self.hangar.update(cx, |hangar, cx| {
            hangar.want_review_image(&before, true, window, cx);
            hangar.want_review_image(&after, true, window, cx);
        });
        self.set_split(50., window, cx);
        cx.notify();
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "left" if !self.single => self.step(false, window, cx),
            "right" if !self.single => self.step(true, window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }
}

fn tag(text: String, right: bool) -> Div {
    div().absolute().top(px(8.)).when(right, |el| el.right(px(8.))).when(!right, |el| el.left(px(8.)))
        .px(px(7.)).py(px(2.)).rounded(px(6.)).bg(theme::scrim()).text_size(px(11.)).text_color(theme::text()).child(text)
}

impl Render for Compare {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (w, h) = self.stage(window, cx);
        let (before, after) = self.images(cx);
        let cut = (w * self.split / 100.).round();
        let entity = cx.entity();
        let stage = div().id("work-review-curtain").relative().w(px(w)).h(px(h)).overflow_hidden().rounded(px(6.))
            .border_1().border_color(theme::border()).bg(theme::inset()).cursor(CursorStyle::ResizeLeftRight)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, window, cx| {
                window.focus(&this.focus, cx);
                this.drag_to(event.position, window, cx);
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if event.pressed_button == Some(MouseButton::Left) { this.drag_to(event.position, window, cx); }
            }))
            .child(canvas(move |bounds, window, cx| {
                if entity.read(cx).stage == Some(bounds) { return; }
                let entity = entity.clone();
                window.defer(cx, move |_, cx| entity.update(cx, |this, _| this.stage = Some(bounds)));
            }, |_, _, _, _| {}).absolute().size_full())
            .when(before.is_none() && after.is_none(), |el| el.flex().items_center().justify_center()
                .child(chrome::Spinner::new("work-review-curtain-loading", IconName::LoaderCircle, px(16.), theme::accent())))
            .children(before.map(|image| super::cover(image, w, h, 0.)))
            .children(after.map(|image| div().absolute().top_0().left(px(cut)).w(px((w - cut).max(0.))).h(px(h)).overflow_hidden()
                .child(super::cover(image, w, h, -cut))))
            .child(div().absolute().top_0().bottom_0().left(px(cut - 1.)).w(px(2.)).bg(theme::accent()))
            .child(tag(self.labels.0.clone(), false))
            .child(tag(self.labels.1.clone(), true));
        let n = self.pairs.len();
        let nav = (!self.single && n > 1).then(|| div().mt(px(8.)).flex().items_center().gap(px(6.)).text_xs().text_color(theme::muted())
            .child(tr_shared("work_review_position", &[("index", &(self.index + 1).to_string()), ("total", &n.to_string())]))
            .child(div().flex_1())
            .child(Button::new("work-review-curtain-previous").outline().xsmall().label(tr_shared("work_review_previous", &[]))
                .on_click(cx.listener(|this, _, window, cx| this.step(false, window, cx))))
            .child(Button::new("work-review-curtain-next").outline().xsmall().label(tr_shared("work_review_next", &[]))
                .on_click(cx.listener(|this, _, window, cx| this.step(true, window, cx)))));
        div().id("work-review-compare").track_focus(&self.focus).key_context("WorkReviewCompare").on_key_down(cx.listener(Self::on_key))
            .flex().flex_col()
            .child(div().flex().items_center().gap_2().mb(px(10.)).text_size(px(13.))
                .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).child(self.pairs[self.index].title.clone()))
                .child(Button::new("work-review-curtain-close").outline().xsmall().label(tr("close"))
                    .on_click(|_, window, cx| window.close_dialog(cx))))
            .child(stage)
            .child(div().mt(px(8.)).child(Slider::new(&self.slider).accessibility_label(tr_shared("work_review_divider_label", &[]))
                .w_full().bg(theme::accent()).text_color(theme::text())))
            .children(nav)
    }
}

/// Abre a cortina no par `index`; as imagens já foram pedidas por quem chama.
pub(super) fn open(pairs: Vec<Pair>, index: usize, labels: (String, String), single: bool, window: &mut Window, cx: &mut Context<Hangar>) {
    let owner = cx.entity();
    let compare = cx.new(|cx| {
        let slider = cx.new(|_| SliderState::new().min(0.).max(100.).step(1.).default_value(50.));
        let slide = cx.subscribe(&slider, |this: &mut Compare, _, event: &SliderEvent, cx| {
            let (SliderEvent::Change(value) | SliderEvent::Release(value)) = event;
            this.split = value.start();
            cx.notify();
        });
        // As imagens chegam no `Hangar`: a cortina redesenha junto.
        let images = cx.observe(&owner, |_, _, cx| cx.notify());
        Compare { hangar: owner.downgrade(), pairs, index, labels, single, split: 50., slider, focus: cx.focus_handle(), stage: None,
            _subscriptions: vec![slide, images] }
    });
    let shown = compare.clone();
    window.open_dialog(cx, move |dialog, window, cx| {
        let (w, h) = shown.update(cx, |compare, cx| compare.stage(window, cx));
        let top = ((f32::from(window.viewport_size().height) - h - CHROME_H) / 2.).max(EDGE);
        dialog.w(px(w + CHROME_W)).margin_top(px(top)).close_button(false).child(shown.clone())
    });
    let focus = compare.read(cx).focus.clone();
    window.defer(cx, move |window, cx| window.focus(&focus, cx));
}

#[cfg(test)]
mod tests {
    use super::{split_at, stage_size};

    #[test]
    fn stage_keeps_the_ratio_inside_the_window() {
        // Janela larga: o palco usa os 980 menos a borda do diálogo, e a altura segue 16:9.
        assert_eq!(stage_size((1600., 1200.), (1600., 900.)), (946., 532.125));
        // Janela baixa: a altura manda e a largura encolhe na mesma proporção.
        let (w, h) = stage_size((1600., 600.), (1600., 900.));
        assert!((w - 625.778).abs() < 0.01 && h == 352., "{w}×{h}");
    }

    #[test]
    fn split_follows_the_pointer_and_stays_inside() {
        assert_eq!(split_at(150., 100., 200.), 25.);
        assert_eq!(split_at(50., 100., 200.), 0.);
        assert_eq!(split_at(400., 100., 200.), 100.);
    }
}
