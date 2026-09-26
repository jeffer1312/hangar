//! Visor de imagem da conversa (`lib/visor.ts` do web): diálogo do kit com a imagem maior, zoom ancorado no cursor,
//! arrasto com a imagem ampliada e ←/→ entre as imagens da mesma mensagem. O Esc é do diálogo, que devolve o foco
//! à miniatura que abriu o visor.
use super::*;
use super::machines::enter_to_focused;

/// Teto do zoom, o mesmo do visor do Zeron.
const MAX_SCALE: f32 = 32.;
/// Passo dos botões e das teclas + e −.
const STEP: f32 = 1.25;
/// Folga do diálogo às bordas da janela.
const EDGE: f32 = 24.;
/// Altura da janela que não é palco: bordas, respiro do diálogo e a faixa com nome e botões.
const CHROME_H: f32 = 128.;

/// Onde a imagem está no palco: pixels da tela por pixel da imagem e o canto de cima à esquerda.
#[derive(Clone, Copy, Debug, PartialEq)]
struct View { scale: f32, x: f32, y: f32 }

/// Escala de encaixe: a imagem inteira cabe no palco, sem ampliar a que já é menor.
fn fit_scale(image: (f32, f32), stage: (f32, f32)) -> f32 { (stage.0 / image.0).min(stage.1 / image.1).min(1.) }

/// Em cada eixo, a imagem menor que o palco fica centrada, e a maior não deixa sobra de palco vazio.
fn clamp(view: View, image: (f32, f32), stage: (f32, f32)) -> View {
    let axis = |pos: f32, size: f32, room: f32| if size <= room { (room - size) / 2. } else { pos.clamp(room - size, 0.) };
    View { scale: view.scale, x: axis(view.x, image.0 * view.scale, stage.0), y: axis(view.y, image.1 * view.scale, stage.1) }
}

fn fitted(image: (f32, f32), stage: (f32, f32)) -> View { clamp(View { scale: fit_scale(image, stage), x: 0., y: 0. }, image, stage) }

/// Zoom que mantém parado o ponto da imagem sob `anchor` (coordenada do palco); nunca abaixo do encaixe.
fn zoom(view: View, image: (f32, f32), stage: (f32, f32), anchor: (f32, f32), factor: f32) -> View {
    let fit = fit_scale(image, stage);
    let scale = (view.scale * factor).clamp(fit, MAX_SCALE.max(fit));
    let ratio = scale / view.scale;
    clamp(View { scale, x: anchor.0 - (anchor.0 - view.x) * ratio, y: anchor.1 - (anchor.1 - view.y) * ratio }, image, stage)
}

enum Shown { Loading, Image(Arc<RenderImage>), Failed(String) }

pub(super) struct Viewer {
    hangar: WeakEntity<Hangar>,
    api: Api,
    runtime: Arc<Runtime>,
    key: SessionKey,
    sources: Vec<Source>,
    index: usize,
    shown: Shown,
    /// Número do pedido à vista: a resposta de uma imagem que já ficou para trás não entra.
    seq: u64,
    _load: Task<()>,
    /// Busca em curso no runtime: trocar de imagem a interrompe em vez de baixar e decodificar à toa.
    fetching: Option<tokio::task::AbortHandle>,
    focus: FocusHandle,
    /// Palco medido no último quadro; o zoom e o arrasto precisam da origem dele.
    stage: Option<Bounds<Pixels>>,
    view: Option<View>,
    drag: Option<(Point<Pixels>, View)>,
}

/// Nome com que a imagem é salva, o mesmo que a conversa mostra embaixo da miniatura.
fn source_name(source: &Source) -> String {
    match source {
        Source::Upload(name) => name.clone(),
        Source::Cited(path) => composer::basename(path).to_owned(),
        Source::Transcript(_, index) => format!("imagem-{}.png", index + 1),
        Source::Remote(url) => composer::url_name(url).to_owned(),
    }
}

impl Viewer {
    fn image_size(&self) -> Option<(f32, f32)> {
        let Shown::Image(image) = &self.shown else { return None };
        let size = image.size(0);
        Some((size.width.0 as f32, size.height.0 as f32))
    }

    fn stage_size(&self) -> Option<(f32, f32)> { self.stage.map(|b| (f32::from(b.size.width), f32::from(b.size.height))) }

    /// A imagem inteira é buscada de novo aqui e solta ao trocar ou fechar: o cache da conversa guarda só miniaturas.
    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.release(window, cx);
        self.seq += 1;
        let (seq, api, name, source) = (self.seq, self.api.clone(), self.key.name.clone(), self.sources[self.index].clone());
        // ponytail: a decodificação do fundo reduz a 2560 px de lado; zoom além disso amplia pixels. Subir se incomodar.
        let job = self.runtime.spawn(async move {
            let bytes = api.fetch(&name, &source).await?;
            Ok(tokio::task::spawn_blocking(move || media::backdrop(&bytes)).await.ok().flatten())
        });
        if let Some(old) = self.fetching.replace(job.abort_handle()) { old.abort(); }
        self._load = cx.spawn(async move |this, cx| {
            let result: Result<Option<Arc<RenderImage>>, Failure> = job.await.unwrap_or_else(|_| Err(Failure::local("invalid_response")));
            let _ = this.update(cx, |this, cx| {
                if this.seq != seq { return; }
                let name = source_name(&this.sources[this.index]);
                this.shown = match result {
                    Ok(Some(image)) => Shown::Image(image),
                    Ok(None) => Shown::Failed(tr("media_failed").replace("{name}", &name).replace("{reason}", &tr("media_unreadable"))),
                    Err(error) => Shown::Failed(tr("media_failed").replace("{name}", &name).replace("{reason}", &Hangar::fetch_failure(&error))),
                };
                this.view = this.image_size().zip(this.stage_size()).map(|(image, stage)| fitted(image, stage));
                cx.notify();
            });
        });
        cx.notify();
    }

    /// Tira a imagem do atlas da GPU; sem isso cada imagem vista ficaria lá até a janela fechar.
    fn release(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Shown::Image(image) = std::mem::replace(&mut self.shown, Shown::Loading) { cx.drop_image(image, Some(window)); }
        self.view = None;
        self.drag = None;
    }

    /// ←/→ andam pelas imagens da mensagem e dão a volta nas pontas.
    fn step(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let total = self.sources.len();
        if total < 2 { return; }
        self.index = if forward { (self.index + 1) % total } else { (self.index + total - 1) % total };
        self.load(window, cx);
    }

    fn zoom_at(&mut self, anchor: Option<(f32, f32)>, factor: f32, cx: &mut Context<Self>) {
        let (Some(image), Some(stage), Some(view)) = (self.image_size(), self.stage_size(), self.view) else { return };
        let anchor = anchor.unwrap_or((stage.0 / 2., stage.1 / 2.));
        let next = zoom(view, image, stage, anchor, factor);
        if next != view { self.view = Some(next); cx.notify(); }
    }

    fn fit(&mut self, cx: &mut Context<Self>) {
        let (Some(image), Some(stage)) = (self.image_size(), self.stage_size()) else { return };
        self.view = Some(fitted(image, stage));
        cx.notify();
    }

    fn set_stage(&mut self, bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        // A origem só serve para os eventos; ela anda na animação de abrir sem a imagem precisar mudar.
        let resized = self.stage.map(|stage| stage.size) != Some(bounds.size);
        self.stage = Some(bounds);
        if !resized { return; }
        // Janela redimensionada: a imagem volta a caber inteira.
        self.view = self.image_size().map(|image| fitted(image, (f32::from(bounds.size.width), f32::from(bounds.size.height))));
        cx.notify();
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "left" => self.step(false, window, cx),
            "right" => self.step(true, window, cx),
            "+" | "=" => self.zoom_at(None, STEP, cx),
            "-" => self.zoom_at(None, 1. / STEP, cx),
            "0" => self.fit(cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let source = self.sources[self.index].clone();
        let name = source_name(&source);
        let _ = self.hangar.update(cx, |hangar, cx| hangar.keep_file(source, name, false, cx));
    }
}

impl Render for Viewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let stage_h = (f32::from(window.viewport_size().height) - CHROME_H).max(160.);
        let total = self.sources.len();
        let zoomed = match (self.view, self.image_size(), self.stage_size()) {
            (Some(view), Some(image), Some(stage)) => view.scale > fit_scale(image, stage) + 0.001,
            _ => false,
        };
        let percent = self.view.map(|view| format!("{}%", (view.scale * 100.).round() as i32));
        let has_image = matches!(self.shown, Shown::Image(_));

        let header = div().flex().items_center().gap_2().pr(px(36.)).h(px(28.))
            .child(div().flex_1().min_w_0().truncate().text_sm().text_color(theme::text()).child(source_name(&self.sources[self.index])))
            .when(total > 1, |el| el.child(div().text_xs().text_color(theme::muted()).child(format!("{} / {}", self.index + 1, total))))
            .child(div().flex().items_center().gap_1()
                .child(Button::new("viewer-zoom-out").ghost().small().icon(IconName::Minus).tooltip(tr("viewer_zoom_out")).disabled(!has_image)
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_at(None, 1. / STEP, cx))))
                // Largura fixa: o número mudando de tamanho não empurra o contador; sem imagem, não há o que mostrar.
                .when_some(percent, |el, percent| el.child(Button::new("viewer-fit").ghost().small().w(px(56.)).label(percent)
                    .tooltip(tr("viewer_fit")).on_click(cx.listener(|this, _, _, cx| this.fit(cx)))))
                .child(Button::new("viewer-zoom-in").ghost().small().icon(IconName::Plus).tooltip(tr("viewer_zoom_in")).disabled(!has_image)
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_at(None, STEP, cx)))))
            .child(Button::new("viewer-save").ghost().small().icon(IconName::Download).label(tr("save"))
                .on_click(cx.listener(|this, _, _, cx| this.save(cx))));

        let entity = cx.entity();
        let body = match (&self.shown, self.view) {
            (Shown::Image(image), Some(view)) => {
                let (w, h) = self.image_size().map(|(w, h)| (w * view.scale, h * view.scale)).unwrap_or_default();
                img(image.clone()).absolute().left(px(view.x)).top(px(view.y)).w(px(w)).h(px(h)).into_any_element()
            }
            (Shown::Image(_), None) => div().into_any_element(),
            (Shown::Loading, _) => div().id("viewer-loading").size_full().flex().items_center().justify_center().role(Role::Status)
                .text_sm().text_color(theme::muted()).child(tr("media_loading")).into_any_element(),
            (Shown::Failed(reason), _) => div().id("viewer-failed").size_full().flex().items_center().justify_center().px_6().role(Role::Alert)
                .text_sm().text_color(theme::warning()).whitespace_normal().child(reason.clone()).into_any_element(),
        };
        let edge = |id: &'static str, icon: IconName, label: String, forward: bool| {
            div().absolute().top_0().bottom_0().when(forward, |el| el.right_2()).when(!forward, |el| el.left_2()).flex().items_center()
                // Fundo próprio: sobre a imagem ampliada, a seta sem fundo some.
                .child(div().rounded_md().bg(theme::scrim()).child(Button::new(id).ghost().icon(icon).tooltip(label)
                    .on_click(cx.listener(move |this: &mut Self, _, window, cx| this.step(forward, window, cx)))))
        };
        let stage = div().id("viewer-stage").relative().w_full().h(px(stage_h)).overflow_hidden().rounded_md().bg(theme::inset())
            .when(zoomed, |el| el.cursor(if self.drag.is_some() { CursorStyle::ClosedHand } else { CursorStyle::OpenHand }))
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                let Some(stage) = this.stage else { return };
                let dy = f32::from(event.delta.pixel_delta(window.line_height()).y);
                let at = event.position - stage.origin;
                this.zoom_at(Some((f32::from(at.x), f32::from(at.y))), (dy * 0.0025).exp(), cx);
                cx.stop_propagation();
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, window, cx| {
                window.focus(&this.focus, cx);
                if let Some(view) = this.view { this.drag = Some((event.position, view)); cx.notify(); }
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                let Some((start, view)) = this.drag else { return };
                if event.pressed_button != Some(MouseButton::Left) { this.drag = None; cx.notify(); return; }
                let (Some(image), Some(stage)) = (this.image_size(), this.stage_size()) else { return };
                let moved = event.position - start;
                let next = clamp(View { x: view.x + f32::from(moved.x), y: view.y + f32::from(moved.y), ..view }, image, stage);
                if this.view != Some(next) { this.view = Some(next); cx.notify(); }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| { this.drag = None; cx.notify(); }))
            // A medida chega no meio do desenho, onde `notify` não pede quadro novo: aplicada depois dele, o reencaixe
            // aparece sem esperar o próximo movimento do mouse.
            .child(canvas(move |bounds, window, cx| {
                if entity.read(cx).stage == Some(bounds) { return; }
                let entity = entity.clone();
                window.defer(cx, move |_, cx| entity.update(cx, |this, cx| this.set_stage(bounds, cx)));
            }, |_, _, _, _| {}).absolute().size_full())
            .child(body)
            .when(total > 1, |el| el
                .child(edge("viewer-previous", IconName::ChevronLeft, tr("viewer_previous"), false))
                .child(edge("viewer-next", IconName::ChevronRight, tr("viewer_next"), true)));

        div().id("viewer").track_focus(&self.focus).key_context("Viewer").on_key_down(cx.listener(Self::on_key))
            .flex().flex_col().gap_3().child(header).child(stage)
    }
}

impl Hangar {
    /// Abre o visor em `sources[index]`; `sources` são as imagens da mensagem, na ordem em que aparecem.
    pub(super) fn open_image(&mut self, key: SessionKey, sources: Vec<Source>, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        if index >= sources.len() { return; }
        let (hangar, runtime) = (cx.entity().downgrade(), self.runtime.clone());
        let viewer = cx.new(|cx| {
            // Rede de segurança para um fechamento que não passe pelo `on_close`: a entidade morre com o diálogo.
            cx.on_release(|viewer: &mut Viewer, cx| {
                if let Some(job) = viewer.fetching.take() { job.abort(); }
                if let Shown::Image(image) = &viewer.shown { cx.drop_image(image.clone(), None); }
            }).detach();
            Viewer {
                hangar, api, runtime, key, sources, index, shown: Shown::Loading, seq: 0, _load: Task::ready(()), fetching: None,
                focus: cx.focus_handle(), stage: None, view: None, drag: None,
            }
        });
        viewer.update(cx, |viewer, cx| viewer.load(window, cx));
        let (shown, closed) = (viewer.clone(), viewer.clone());
        window.open_dialog(cx, move |dialog, window, _| {
            let width = f32::from(window.viewport_size().width) - EDGE * 2.;
            let closed = closed.clone();
            dialog.w(px(width)).margin_top(px(EDGE)).on_ok(enter_to_focused).child(shown.clone())
                .on_close(move |_, window, cx| closed.update(cx, |viewer, cx| viewer.release(window, cx)))
        });
        // O kit põe o foco no diálogo ao abrir; as setas e o zoom pelo teclado precisam dele no visor.
        let focus = viewer.read(cx).focus.clone();
        window.defer(cx, move |window, cx| window.focus(&focus, cx));
    }
}

#[cfg(test)]
mod tests {
    // Sem glob: o `test` da gpui colide com o atributo padrão.
    use super::{MAX_SCALE, View, clamp, fitted, zoom};

    #[test]
    fn fit_never_enlarges_and_centers() {
        assert_eq!(fitted((100., 50.), (400., 300.)), View { scale: 1., x: 150., y: 125. });
        let big = fitted((800., 400.), (400., 300.));
        assert_eq!((big.scale, big.x, big.y), (0.5, 0., 50.));
    }

    #[test]
    fn zoom_keeps_the_anchor_and_stops_at_fit() {
        let (image, stage) = ((800., 400.), (400., 300.));
        let start = fitted(image, stage);
        let anchor = (100., 150.);
        let before = ((anchor.0 - start.x) / start.scale, (anchor.1 - start.y) / start.scale);
        let zoomed = zoom(start, image, stage, anchor, 2.);
        let after = ((anchor.0 - zoomed.x) / zoomed.scale, (anchor.1 - zoomed.y) / zoomed.scale);
        assert_eq!(zoomed.scale, 1.);
        assert!((before.0 - after.0).abs() < 0.01 && (before.1 - after.1).abs() < 0.01);
        assert_eq!(zoom(zoomed, image, stage, anchor, 0.01), start);
        assert_eq!(zoom(start, image, stage, anchor, 1e6).scale, MAX_SCALE);
    }

    #[test]
    fn pan_never_shows_empty_stage() {
        let (image, stage) = ((800., 400.), (400., 300.));
        let view = View { scale: 1., x: 50., y: -500. };
        assert_eq!(clamp(view, image, stage), View { scale: 1., x: 0., y: -100. });
    }
}
