//! Visor de imagem da conversa (`lib/visor.ts` do web): diálogo do kit com a imagem maior, zoom ancorado no cursor,
//! arrasto com a imagem ampliada e ←/→ entre as imagens da mesma mensagem. O Esc é do diálogo, que devolve o foco
//! à miniatura que abriu o visor.
use super::*;
use super::machines::enter_to_focused;
use gpui_kit::{LineCap, PathBuilder, PathStyle, StrokeOptions};

/// Teto do zoom, o mesmo do visor do Zeron.
const MAX_SCALE: f32 = 32.;
/// Passo dos botões e das teclas + e −.
const STEP: f32 = 1.25;
/// Folga do diálogo às bordas da janela.
const EDGE: f32 = 24.;
/// Altura da janela que não é palco: bordas, respiro do diálogo e a faixa com nome e botões.
const CHROME_H: f32 = 128.;
/// Largura do diálogo que não é palco: respiro dos lados e a borda.
const CHROME_W: f32 = 34.;
/// Menor palco: a faixa de cima precisa caber nome, zoom, Abrir e Salvar.
const MIN_W: f32 = 480.;
const MIN_H: f32 = 160.;

/// Cores das marcações (a primeira é a laranja das notas de print) e espessuras da caneta, em pixels da tela.
const PEN_COLORS: [u32; 4] = [0xea580c, 0xe11d48, 0x2563eb, 0x16a34a];
const PEN_WIDTHS: [f32; 3] = [3., 6., 12.];
/// Borda do retângulo, em pixels da tela.
const RECT_WIDTH: f32 = 3.;
/// Medidas da nota em pixels da tela: respiro, círculo do número, vão, letra e linha.
const NOTE_PAD: (f32, f32) = (10., 7.);
const NOTE_DOT: f32 = 18.;
const NOTE_GAP: f32 = 7.;
const NOTE_FONT: f32 = 12.;
const NOTE_LINE: f32 = 16.;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Tool { Pen, Rect, Note }

/// Uma marcação, em pixels da imagem mostrada: acompanha o zoom e o arrasto. Espessura e tamanho da nota são os da
/// tela na hora em que nasceram, guardados em pixels da imagem.
#[derive(Clone, Debug, PartialEq)]
enum Mark {
    Pen { color: u32, width: f32, points: Vec<(f32, f32)> },
    Rect { color: u32, width: f32, from: (f32, f32), to: (f32, f32) },
    /// `unit`: pixels da imagem por pixel da tela quando a nota nasceu; `size`: a caixa medida na tela, em pixels da imagem.
    Note { color: u32, number: usize, at: (f32, f32), text: String, unit: f32, size: Option<(f32, f32)> },
}

fn hex(color: u32) -> String { format!("#{color:06x}") }

/// Caractere de controle colado no campo (tab, ESC) não é XML válido e derrubaria o SVG inteiro: sai.
fn escape(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect::<String>()
        .replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// As marcações como SVG do tamanho da original; `k` leva os pixels da mostrada aos da original, em cada eixo.
fn marks_svg(marks: &[Mark], size: (u32, u32), k: (f32, f32)) -> String {
    let mut body = String::new();
    for mark in marks {
        match mark {
            Mark::Pen { color, width, points } => {
                let pts: Vec<String> = points.iter().map(|(x, y)| format!("{x},{y}")).collect();
                // Um clique sem arrastar ainda deixa um ponto.
                let pts = if pts.len() == 1 { format!("{0} {0}", pts[0]) } else { pts.join(" ") };
                body.push_str(&format!(r#"<polyline points="{pts}" fill="none" stroke="{}" stroke-width="{width}" stroke-linecap="round" stroke-linejoin="round"/>"#, hex(*color)));
            }
            Mark::Rect { color, width, from, to } => {
                let (x, y, w, h) = (from.0.min(to.0), from.1.min(to.1), (to.0 - from.0).abs(), (to.1 - from.1).abs());
                body.push_str(&format!(r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{}" fill="none" stroke="{}" stroke-width="{width}"/>"#, width * 2., hex(*color)));
            }
            Mark::Note { color, number, at, text, unit: u, size } => {
                let Some((w, h)) = size else { continue };
                let (dot_x, dot_y) = (at.0 + NOTE_PAD.0 * u + NOTE_DOT * u / 2., at.1 + NOTE_PAD.1 * u + NOTE_DOT * u / 2.);
                body.push_str(&format!(r#"<rect x="{}" y="{}" width="{w}" height="{h}" rx="{}" fill="{}"/>"#, at.0, at.1, 6. * u, hex(*color)));
                body.push_str(&format!(r##"<circle cx="{dot_x}" cy="{dot_y}" r="{}" fill="#ffffff"/>"##, NOTE_DOT * u / 2.));
                body.push_str(&format!(r#"<text x="{dot_x}" y="{}" font-family="Geist" font-weight="700" font-size="{}" text-anchor="middle" fill="{}">{number}</text>"#,
                    dot_y + 4. * u, 11. * u, hex(*color)));
                body.push_str(&format!(r##"<text x="{}" y="{}" font-family="Geist" font-weight="600" font-size="{}" fill="#ffffff">{}</text>"##,
                    at.0 + (NOTE_PAD.0 + NOTE_DOT + NOTE_GAP) * u, at.1 + NOTE_PAD.1 * u + (NOTE_LINE - 4.) * u + 1. * u, NOTE_FONT * u, escape(text)));
            }
        }
    }
    format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}"><g transform="scale({},{})">{body}</g></svg>"#, size.0, size.1, k.0, k.1)
}

/// Grava as marcações na imagem original, em PNG. `shown` é o tamanho da mostrada, que pode vir reduzida da original.
fn burn(bytes: &[u8], shown: (f32, f32), marks: &[Mark]) -> Option<Vec<u8>> {
    // De pé como o visor a mostra (foto de celular com orientação EXIF): senão as marcações cairiam giradas.
    let mut image = crate::media::oriented(bytes)?.to_rgba8();
    let (w, h) = image.dimensions();
    let mut options = resvg::usvg::Options::default();
    // A fonte do app, embutida: o texto da nota sai igual em qualquer máquina.
    options.fontdb_mut().load_font_data(include_bytes!("../../assets/fonts/Geist-SemiBold.ttf").to_vec());
    options.fontdb_mut().load_font_data(include_bytes!("../../assets/fonts/Geist-Bold.ttf").to_vec());
    let tree = resvg::usvg::Tree::from_str(&marks_svg(marks, (w, h), (w as f32 / shown.0, h as f32 / shown.1)), &options).ok()?;
    let mut layer = resvg::tiny_skia::Pixmap::new(w, h)?;
    resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut layer.as_mut());
    // Camada pré-multiplicada sobre fundo de alfa reto (a original pode ser PNG transparente): "por cima" e de volta ao reto.
    for (dst, src) in image.pixels_mut().zip(layer.data().chunks_exact(4)) {
        let sa = src[3] as f32 / 255.;
        if sa == 0. { continue; }
        let da = dst[3] as f32 / 255.;
        let out = sa + da * (1. - sa);
        for c in 0..3 { dst[c] = ((src[c] as f32 / 255. + dst[c] as f32 / 255. * da * (1. - sa)) / out * 255.).round().min(255.) as u8; }
        dst[3] = (out * 255.).round() as u8;
    }
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(image).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).ok()?;
    Some(out)
}

/// Nome da cópia riscada: o da original com "-riscado", sempre PNG.
fn burned_name(name: &str) -> String {
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    format!("{stem}-riscado.png")
}

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

/// Maior palco que a janela comporta.
fn limit(window: &Window) -> (f32, f32) {
    let size = window.viewport_size();
    ((f32::from(size.width) - EDGE * 2. - CHROME_W).max(1.), (f32::from(size.height) - CHROME_H).max(MIN_H))
}

/// O palco abraça a imagem na escala dada, entre o mínimo e o `limit` da janela.
fn stage_for(image: (f32, f32), scale: f32, limit: (f32, f32)) -> (f32, f32) {
    ((image.0 * scale).clamp(MIN_W.min(limit.0), limit.0), (image.1 * scale).clamp(MIN_H.min(limit.1), limit.1))
}

fn fitted(image: (f32, f32), limit: (f32, f32)) -> View {
    let scale = fit_scale(image, limit);
    clamp(View { scale, x: 0., y: 0. }, image, stage_for(image, scale, limit))
}

/// Zoom que mantém parado o ponto da imagem sob `anchor` (coordenada do palco); nunca abaixo do encaixe. O diálogo é
/// centrado, então o palco que cresce se abre metade para cada lado e a âncora anda metade do crescimento.
fn zoom(view: View, image: (f32, f32), limit: (f32, f32), anchor: (f32, f32), factor: f32) -> View {
    let fit = fit_scale(image, limit);
    let scale = (view.scale * factor).clamp(fit, MAX_SCALE.max(fit));
    let ratio = scale / view.scale;
    let (before, after) = (stage_for(image, view.scale, limit), stage_for(image, scale, limit));
    let (dx, dy) = ((after.0 - before.0) / 2., (after.1 - before.1) / 2.);
    clamp(View { scale, x: anchor.0 + dx - (anchor.0 - view.x) * ratio, y: anchor.1 + dy - (anchor.1 - view.y) * ratio }, image, after)
}

/// Imagens inteiras já abertas no visor, fora do atlas da GPU: reabrir a mesma não busca nem decodifica de novo.
pub(super) type FullImages = std::rc::Rc<std::cell::RefCell<media::MediaCache<(SessionKey, Source)>>>;

/// ponytail: ~8 capturas de tela de 1920×1080 decodificadas; sai a aberta há mais tempo. Subir se reabrir antigas pesar.
const FULL_BUDGET: usize = 64 * 1024 * 1024;

pub(super) fn full_images() -> FullImages { std::rc::Rc::new(std::cell::RefCell::new(media::MediaCache::with_budget(FULL_BUDGET))) }

enum Shown { Loading, Image(Arc<RenderImage>), Failed(String) }

pub(super) struct Viewer {
    hangar: WeakEntity<Hangar>,
    api: Api,
    /// Anexos do cofre lidos do disco quando a sessão é desta máquina.
    uploads: super::disk::Uploads,
    runtime: Arc<Runtime>,
    full: FullImages,
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
    /// Maior palco que a janela comporta, lido no último quadro.
    limit: Option<(f32, f32)>,
    /// Último palco com imagem: carregando a próxima, o diálogo fica do tamanho que estava em vez de piscar.
    held: Option<(f32, f32)>,
    view: Option<View>,
    drag: Option<(Point<Pixels>, View)>,
    /// Caneta ligada: o arrasto risca em vez de mover a imagem.
    pen: bool,
    pen_color: usize,
    pen_width: usize,
    tool: Tool,
    marks: Vec<Mark>,
    /// Traço ou retângulo sendo arrastado.
    drawing: Option<Mark>,
    /// Nota sendo escrita: o índice dela em `marks` e o campo.
    editing: Option<(usize, Entity<InputState>)>,
    /// Cópia riscada sendo gravada ou anexada.
    burning: bool,
    problem: Option<String>,
}

/// Nome com que a imagem é salva, o mesmo que a conversa mostra embaixo da miniatura.
fn source_name(source: &Source) -> String {
    match source {
        Source::Upload(name) => name.clone(),
        Source::Cited(path) | Source::Workspace(path) => composer::basename(path).to_owned(),
        Source::Transcript(_, index) => format!("imagem-{}.png", index + 1),
        Source::Remote(url) => composer::url_name(url).to_owned(),
        Source::Memory(name, _) => name.clone(),
    }
}

impl Viewer {
    fn image_size(&self) -> Option<(f32, f32)> {
        let Shown::Image(image) = &self.shown else { return None };
        let size = image.size(0);
        Some((size.width.0 as f32, size.height.0 as f32))
    }

    /// Tamanho do palco dentro de `limit`; o diálogo usa o mesmo cálculo para a própria largura.
    fn stage_box(&self, limit: (f32, f32)) -> (f32, f32) {
        match self.image_size().zip(self.view) {
            Some((image, view)) => stage_for(image, view.scale, limit),
            None => self.held.map_or((MIN_W.min(limit.0), (MIN_H * 1.5).min(limit.1)), |(w, h)| (w.min(limit.0), h.min(limit.1))),
        }
    }

    fn stage_size(&self) -> Option<(f32, f32)> { self.limit.map(|limit| self.stage_box(limit)) }

    /// A imagem inteira sai do atlas ao trocar ou fechar, mas fica em `full`: reabrir é na hora. O cache da conversa
    /// guarda só miniaturas.
    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.release(window, cx);
        self.seq += 1;
        let slot = (self.key.clone(), self.sources[self.index].clone());
        let hit = {
            let mut full = self.full.borrow_mut();
            // Cada abertura é um "quadro": a que está à vista nunca é a expulsa.
            full.next_frame();
            match full.get(&slot) { Some(media::MediaState::Image(image)) => Some(image.clone()), _ => None }
        };
        if let Some(image) = hit {
            media::trace(format_args!("viewer hit {:?}", slot.1));
            if let Some(old) = self.fetching.take() { old.abort(); }
            self._load = Task::ready(());
            self.shown = Shown::Image(image);
            self.view = self.image_size().zip(self.limit).map(|(image, limit)| fitted(image, limit));
            cx.notify();
            return;
        }
        let (seq, api, name, source, uploads) = (self.seq, self.api.clone(), self.key.name.clone(), self.sources[self.index].clone(), self.uploads.clone());
        // ponytail: a decodificação do fundo reduz a 2560 px de lado; zoom além disso amplia pixels. Subir se incomodar.
        let job = self.runtime.spawn(async move {
            let started = std::time::Instant::now();
            media::trace(format_args!("viewer start {source:?}"));
            let bytes = uploads.fetch(&api, &name, &source).await?;
            media::trace(format_args!("viewer fetched {source:?} {} B in {:.1} ms", bytes.len(), started.elapsed().as_secs_f64() * 1000.));
            let image = tokio::task::spawn_blocking(move || media::backdrop(&bytes)).await.ok().flatten();
            media::trace(format_args!("viewer ready {source:?} in {:.1} ms", started.elapsed().as_secs_f64() * 1000.));
            Ok(image)
        });
        if let Some(old) = self.fetching.replace(job.abort_handle()) { old.abort(); }
        self._load = cx.spawn(async move |this, cx| {
            let result: Result<Option<Arc<RenderImage>>, Failure> = job.await.unwrap_or_else(|_| Err(Failure::local("invalid_response")));
            let _ = this.update(cx, |this, cx| {
                if this.seq != seq { return; }
                let name = source_name(&this.sources[this.index]);
                this.shown = match result {
                    Ok(Some(image)) => {
                        let evicted = this.full.borrow_mut().insert(slot, media::MediaState::Image(image.clone()));
                        for old in evicted { cx.drop_image(old, None); }
                        Shown::Image(image)
                    }
                    Ok(None) => Shown::Failed(tr("media_failed").replace("{name}", &name).replace("{reason}", &tr("media_unreadable"))),
                    Err(error) => Shown::Failed(tr("media_failed").replace("{name}", &name).replace("{reason}", &Hangar::fetch_failure(&error))),
                };
                this.view = this.image_size().zip(this.limit).map(|(image, limit)| fitted(image, limit));
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
        // Os traços são da imagem que saiu.
        self.marks.clear();
        self.drawing = None;
        self.editing = None;
        self.problem = None;
    }

    /// Ponto da tela em pixels da imagem mostrada.
    fn image_point(&self, at: Point<Pixels>) -> Option<(f32, f32)> {
        let (stage, view) = (self.stage?, self.view?);
        let local = at - stage.origin;
        Some(((f32::from(local.x) - view.x) / view.scale, (f32::from(local.y) - view.y) / view.scale))
    }

    /// Traço ou retângulo arrastado entra na lista; retângulo sem área (um clique) não.
    fn commit_drawing(&mut self) {
        match self.drawing.take() {
            Some(Mark::Rect { from, to, .. }) if (to.0 - from.0).abs() < 2. || (to.1 - from.1).abs() < 2. => {}
            Some(mark) => self.marks.push(mark),
            None => {}
        }
    }

    /// As notas por cima da imagem, no tamanho em que nasceram vezes o zoom; a em edição mostra o campo. A caixa medida
    /// volta para a marcação: é ela que o PNG grava.
    fn render_notes(&self, view: View, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let entity = cx.entity();
        self.marks.iter().enumerate().filter_map(|(i, mark)| {
            let Mark::Note { color, number, at, text, unit, size } = mark else { return None };
            let z = unit * view.scale;
            let editing = self.editing.as_ref().filter(|(at, _)| *at == i).map(|(_, input)| input.clone());
            let (entity, known) = (entity.clone(), *size);
            let measure = canvas(move |bounds, window, cx| {
                let size = (f32::from(bounds.size.width) / view.scale, f32::from(bounds.size.height) / view.scale);
                if known.is_some_and(|k| (k.0 - size.0).abs() < 0.5 && (k.1 - size.1).abs() < 0.5) { return; }
                let entity = entity.clone();
                window.defer(cx, move |_, cx| entity.update(cx, |this, cx| {
                    if let Some(Mark::Note { size: slot, .. }) = this.marks.get_mut(i) { *slot = Some(size); cx.notify(); }
                }));
            }, |_, _, _, _| {}).absolute().size_full();
            Some(div().absolute().left(px(view.x + at.0 * view.scale)).top(px(view.y + at.1 * view.scale))
                .flex().items_start().gap(px(NOTE_GAP * z)).px(px(NOTE_PAD.0 * z)).py(px(NOTE_PAD.1 * z)).rounded(px(6. * z))
                .bg(Hsla::from(rgb(*color))).text_color(Hsla::from(rgb(0xffffff))).text_size(px(NOTE_FONT * z)).line_height(px(NOTE_LINE * z))
                .font_weight(FontWeight::SEMIBOLD).whitespace_nowrap()
                // Clicar na nota (ou no campo dela) não cria outra embaixo.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(measure)
                .child(div().flex_shrink_0().size(px(NOTE_DOT * z)).rounded_full().bg(Hsla::from(rgb(0xffffff))).flex().items_center().justify_center()
                    .text_color(Hsla::from(rgb(*color))).text_size(px(11. * z)).font_weight(FontWeight::BOLD).child(number.to_string()))
                .map(|el| match editing {
                    Some(input) => el.child(div().w(px(220. * z)).child(Input::new(&input).appearance(false).small())),
                    None => el.child(text.clone()),
                })
                .into_any_element())
        }).collect()
    }

    /// Tira a última marcação; a nota que some leva junto o campo dela.
    fn undo(&mut self, cx: &mut Context<Self>) {
        if self.marks.pop().is_none() { return; }
        if self.editing.as_ref().is_some_and(|(at, _)| *at >= self.marks.len()) { self.editing = None; }
        cx.notify();
    }

    /// Nota em edição vira texto; vazia, some.
    fn finish_note(&mut self, cx: &mut Context<Self>) {
        let Some((at, input)) = self.editing.take() else { return };
        let value = input.read(cx).value().trim().to_owned();
        if value.is_empty() {
            if at < self.marks.len() { self.marks.remove(at); }
        } else if let Some(Mark::Note { text, size, .. }) = self.marks.get_mut(at) {
            *text = value;
            // A caixa muda com o texto final: o próximo quadro mede de novo.
            *size = None;
        }
        cx.notify();
    }

    /// Nota nova onde a pessoa clicou, com o próximo número, já com o campo para digitar.
    fn start_note(&mut self, at: (f32, f32), view: View, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_note(cx);
        let number = self.marks.iter().filter(|m| matches!(m, Mark::Note { .. })).count() + 1;
        self.marks.push(Mark::Note { color: PEN_COLORS[self.pen_color], number, at, text: String::new(), unit: 1. / view.scale, size: None });
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(tr("viewer_note_placeholder")));
        input.update(cx, |input, cx| input.focus(window, cx));
        // O Blur de um campo antigo chega depois da nota nova nascer: só o campo da nota em edição fecha a nota.
        cx.subscribe_in(&input, window, |this, input, event: &InputEvent, _, cx| match event {
            InputEvent::PressEnter { .. } | InputEvent::Blur if this.editing.as_ref().is_some_and(|(_, own)| own == input) => this.finish_note(cx),
            _ => cx.notify(),
        }).detach();
        self.editing = Some((self.marks.len() - 1, input));
        cx.notify();
    }

    /// Lê a original, grava os traços e entrega o PNG a `done`, na janela.
    fn burned(&mut self, window: &mut Window, cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, Vec<u8>, String, &mut Window, &mut Context<Self>) + 'static) {
        self.burn_when_measured(0, window, cx, done);
    }

    /// A caixa da nota é medida na tela depois de desenhada: espera alguns quadros até todas terem a medida do texto final.
    fn burn_when_measured(&mut self, tries: u8, window: &mut Window, cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, Vec<u8>, String, &mut Window, &mut Context<Self>) + 'static) {
        self.finish_note(cx);
        let unmeasured = self.marks.iter().any(|m| matches!(m, Mark::Note { size: None, .. }));
        if unmeasured && tries < 10 {
            cx.on_next_frame(window, move |this, window, cx| this.burn_when_measured(tries + 1, window, cx, done));
            cx.notify();
            return;
        }
        // Nota que não chegou a ser medida sairia do PNG calada: melhor não gravar e dizer.
        if unmeasured {
            self.problem = Some(tr("viewer_burn_failed").replace("{reason}", &tr("viewer_note_unmeasured")));
            cx.notify();
            return;
        }
        let Some(shown) = self.image_size() else { return };
        if self.marks.is_empty() || self.burning { return; }
        self.burning = true;
        self.problem = None;
        let (api, name, source, uploads, marks) = (self.api.clone(), self.key.name.clone(), self.sources[self.index].clone(),
            self.uploads.clone(), self.marks.clone());
        let file = burned_name(&source_name(&source));
        let job = self.runtime.spawn(async move {
            let bytes = uploads.fetch(&api, &name, &source).await.map_err(|error| Hangar::fetch_failure(&error))?;
            tokio::task::spawn_blocking(move || burn(&bytes, shown, &marks)).await.ok().flatten().ok_or_else(|| tr("media_unreadable"))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await.unwrap_or_else(|_| Err(tr("invalid_response")));
            let _ = this.update_in(cx, |this, window, cx| {
                this.burning = false;
                match result {
                    Ok(bytes) => done(this, bytes, file, window, cx),
                    Err(error) => this.problem = Some(tr("viewer_burn_failed").replace("{reason}", &error)),
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    /// A cópia riscada entra como anexo no campo da conversa aberta, como um print colado; o visor fecha.
    fn send_burned(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Só uma nota vazia em edição: ela some ao fechar, e não há o que enviar.
        self.finish_note(cx);
        if self.marks.is_empty() { self.problem = Some(tr("viewer_nothing_drawn")); cx.notify(); return; }
        self.burned(window, cx, |this, bytes, file, window, cx| {
            let attached = this.hangar.update(cx, |hangar, cx| {
                let key = hangar.composer_key().ok_or_else(|| tr("viewer_send_no_field"))?;
                let result = hangar.add_attachment(&key, file, bytes);
                cx.notify();
                result
            }).unwrap_or_else(|_| Err(tr("invalid_response")));
            match attached {
                Ok(()) => window.close_dialog(cx),
                Err(error) => this.problem = Some(error),
            }
        });
    }

    /// Salvar com traços grava a cópia riscada onde a pessoa escolher; a original não muda.
    fn save_burned(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.burned(window, cx, |_, bytes, file, _, cx| {
            let prompt = cx.prompt_for_new_path(&downloads_folder(), Some(&file));
            cx.spawn(async move |this, cx| {
                let path = match prompt.await {
                    Ok(Ok(Some(path))) => path,
                    // O seletor que falhou não é a pessoa desistindo: diz.
                    Ok(Err(_)) => { let _ = this.update(cx, |this, cx| { this.problem = Some(tr("save_failed")); cx.notify(); }); return; }
                    _ => return,
                };
                let written = cx.background_executor().spawn(async move { std::fs::write(&path, bytes) }).await;
                let _ = this.update(cx, |this, cx| {
                    if written.is_err() { this.problem = Some(tr("save_failed")); cx.notify(); }
                });
            }).detach();
        });
    }

    /// ←/→ andam pelas imagens da mensagem e dão a volta nas pontas.
    fn step(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let total = self.sources.len();
        if total < 2 { return; }
        self.index = if forward { (self.index + 1) % total } else { (self.index + total - 1) % total };
        self.load(window, cx);
    }

    fn zoom_at(&mut self, anchor: Option<(f32, f32)>, factor: f32, cx: &mut Context<Self>) {
        let (Some(image), Some(stage), Some(limit), Some(view)) = (self.image_size(), self.stage_size(), self.limit, self.view) else { return };
        let anchor = anchor.unwrap_or((stage.0 / 2., stage.1 / 2.));
        let next = zoom(view, image, limit, anchor, factor);
        if next != view { self.view = Some(next); cx.notify(); }
    }

    fn fit(&mut self, cx: &mut Context<Self>) {
        let (Some(image), Some(limit)) = (self.image_size(), self.limit) else { return };
        self.view = Some(fitted(image, limit));
        cx.notify();
    }

    /// A origem só serve para os eventos; o tamanho do palco sai do zoom e da janela, não da medida.
    fn set_stage(&mut self, bounds: Bounds<Pixels>) { self.stage = Some(bounds); }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Ctrl+Z (Cmd+Z no Mac) tira o último traço.
        // Nota em edição: toda tecla é do campo (o "-" e o "0" não podem virar zoom, nem o Ctrl+Z tirar a marcação).
        if self.editing.is_some() { return; }
        if event.keystroke.modifiers.secondary(cx) && event.keystroke.key == "z" {
            self.undo(cx);
            cx.stop_propagation();
            return;
        }
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

    /// `open` entrega uma cópia ao programa do sistema; sem ele, pergunta onde salvar.
    fn keep(&mut self, open: bool, cx: &mut Context<Self>) {
        let source = self.sources[self.index].clone();
        let name = source_name(&source);
        let _ = self.hangar.update(cx, |hangar, cx| hangar.keep_file(source, name, open, cx));
    }
}

impl Render for Viewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let limit = limit(window);
        if self.limit != Some(limit) {
            // Janela redimensionada: a imagem volta a caber inteira.
            self.limit = Some(limit);
            self.view = self.image_size().map(|image| fitted(image, limit));
        }
        let (stage_w, stage_h) = self.stage_box(limit);
        if self.image_size().is_some() { self.held = Some((stage_w, stage_h)); }
        let total = self.sources.len();
        let zoomed = match (self.view, self.image_size(), self.stage_size()) {
            (Some(view), Some(image), Some(stage)) => view.scale > fit_scale(image, stage) + 0.001,
            _ => false,
        };
        let percent = self.view.map(|view| format!("{}%", (view.scale * 100.).round() as i32));
        let has_image = matches!(self.shown, Shown::Image(_));
        let openable = composer::openable(&composer::safe_name(&source_name(&self.sources[self.index])));

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
            .child(Button::new("viewer-pen").ghost().small().icon(IconName::Pencil).label(tr("viewer_pen")).selected(self.pen)
                .disabled(!has_image).tooltip(tr("viewer_pen_tip"))
                .on_click(cx.listener(|this, _, _, cx| { this.pen = !this.pen; this.drag = None; cx.notify(); })))
            .when(openable, |el| el.child(Button::new("viewer-open").ghost().small().icon(IconName::ExternalLink).label(tr("open"))
                .on_click(cx.listener(|this, _, _, cx| this.keep(true, cx)))))
            // Com traços, Salvar grava a cópia riscada; sem eles, a original como antes.
            .child(Button::new("viewer-save").ghost().small().icon(IconName::Download).label(tr("save")).loading(self.burning)
                // A nota vazia em edição some aqui: sobrando nada, Salvar grava a original.
                .on_click(cx.listener(|this, _, window, cx| {
                    this.finish_note(cx);
                    if this.marks.is_empty() { this.keep(false, cx) } else { this.save_burned(window, cx) }
                })));

        let tools = self.pen.then(|| {
            let kinds = [(Tool::Pen, IconName::Pencil, "viewer_tool_pen"), (Tool::Rect, IconName::Square, "viewer_tool_rect"),
                (Tool::Note, IconName::MessageSquare, "viewer_tool_note")].into_iter().map(|(tool, icon, label)| {
                Button::new(SharedString::from(format!("viewer-tool-{label}"))).ghost().xsmall().icon(icon).label(tr(label)).selected(self.tool == tool)
                    .on_click(cx.listener(move |this, _, _, cx| { this.finish_note(cx); this.tool = tool; cx.notify(); }))
            }).collect::<Vec<_>>();
            let swatches = PEN_COLORS.iter().enumerate().map(|(i, &color)| {
                let on = i == self.pen_color;
                div().id(SharedString::from(format!("viewer-pen-color-{i}"))).size(px(18.)).rounded_full().cursor_pointer()
                    .bg(Hsla::from(rgb(color))).border_2().border_color(if on { theme::text() } else { transparent_black() })
                    .role(Role::Button).aria_selected(on).aria_label(format!("{} {}", tr("viewer_pen_color"), i + 1))
                    .on_click(cx.listener(move |this, _, _, cx| { this.pen_color = i; cx.notify(); }))
            }).collect::<Vec<_>>();
            let widths = [tr("viewer_pen_thin"), tr("viewer_pen_medium"), tr("viewer_pen_thick")].into_iter().enumerate().map(|(i, label)| {
                Button::new(SharedString::from(format!("viewer-pen-width-{i}"))).ghost().xsmall().label(label).selected(i == self.pen_width)
                    .on_click(cx.listener(move |this, _, _, cx| { this.pen_width = i; cx.notify(); }))
            }).collect::<Vec<_>>();
            let drawn = !self.marks.is_empty();
            let sep = || div().w(px(1.)).h(px(16.)).bg(theme::border_strong());
            // Quebra em duas linhas na imagem estreita: o diálogo tem a largura dela.
            div().flex().flex_wrap().items_center().gap_2().text_xs().text_color(theme::muted())
                .children(kinds).child(sep())
                .child(tr("viewer_pen_color")).children(swatches)
                // A espessura é só da caneta; retângulo e nota têm a deles.
                .when(self.tool == Tool::Pen, |el| el.child(sep()).child(tr("viewer_pen_width")).children(widths))
                .child(sep())
                .child(Button::new("viewer-pen-undo").ghost().xsmall().label(tr("viewer_pen_undo")).disabled(!drawn)
                    .on_click(cx.listener(|this, _, _, cx| this.undo(cx))))
                .child(Button::new("viewer-pen-clear").ghost().xsmall().label(tr("viewer_pen_clear")).disabled(!drawn)
                    .on_click(cx.listener(|this, _, _, cx| { this.editing = None; this.marks.clear(); cx.notify(); })))
                .children(self.problem.clone().map(|problem| div().min_w_0().truncate().text_color(theme::warning()).child(problem)))
                .child(div().flex_1())
                .child(Button::new("viewer-pen-send").primary().small().label(tr("viewer_pen_send")).disabled(!drawn).loading(self.burning)
                    .on_click(cx.listener(|this, _, window, cx| this.send_burned(window, cx))))
        });
        // Traços e retângulos desenhados por cima da imagem, na posição e no zoom atuais; as notas são caixas, logo abaixo.
        let lines: Vec<Mark> = self.marks.iter().chain(self.drawing.iter()).filter(|m| !matches!(m, Mark::Note { .. })).cloned().collect();
        let painted = self.view;
        let ink = canvas(|_, _, _| (), move |bounds, _, window, _| {
            let Some(view) = painted else { return };
            let at = |(x, y): (f32, f32)| point(bounds.origin.x + px(view.x + x * view.scale), bounds.origin.y + px(view.y + y * view.scale));
            for mark in &lines {
                let (color, width, points) = match mark {
                    Mark::Pen { color, width, points } => (*color, *width, points.clone()),
                    Mark::Rect { color, width, from, to } => (*color, *width, vec![*from, (to.0, from.1), *to, (from.0, to.1), *from]),
                    Mark::Note { .. } => continue,
                };
                let Some(&first) = points.first() else { continue };
                let width = (width * view.scale).max(1.);
                let options = StrokeOptions::default().with_line_width(width).with_line_cap(LineCap::Round);
                let mut path = PathBuilder::stroke(px(width)).with_style(PathStyle::Stroke(options));
                path.move_to(at(first));
                // Um clique sem arrastar ainda deixa um ponto.
                if points.len() == 1 { path.line_to(at((first.0 + 0.01, first.1))); }
                for &p in &points[1..] { path.line_to(at(p)); }
                if let Ok(path) = path.build() { window.paint_path(path, Hsla::from(rgb(color))); }
            }
        }).absolute().size_full();
        let notes = self.view.map(|view| self.render_notes(view, cx)).unwrap_or_default();

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
        let stage = div().id("viewer-stage").relative().w(px(stage_w)).h(px(stage_h)).overflow_hidden().rounded_md().bg(theme::inset())
            .when(self.pen, |el| el.cursor(CursorStyle::Crosshair))
            .when(zoomed && !self.pen, |el| el.cursor(if self.drag.is_some() { CursorStyle::ClosedHand } else { CursorStyle::OpenHand }))
            .on_scroll(cx.listener(|this, event: &ScrollEvent, window, cx| {
                let Some(stage) = this.stage else { return };
                let dy = f32::from(event.delta.pixel_delta(window.line_height()).y);
                let at = event.position - stage.origin;
                this.zoom_at(Some((f32::from(at.x), f32::from(at.y))), (dy * 0.0025).exp(), cx);
                cx.stop_propagation();
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, window, cx| {
                window.focus(&this.focus, cx);
                if this.pen {
                    let Some((at, view)) = this.image_point(event.position).zip(this.view) else { return };
                    let color = PEN_COLORS[this.pen_color];
                    // Espessuras são as da tela agora: guardadas em pixels da imagem, a marcação engrossa com o zoom.
                    match this.tool {
                        Tool::Pen => { this.finish_note(cx); this.drawing = Some(Mark::Pen { color, width: PEN_WIDTHS[this.pen_width] / view.scale, points: vec![at] }); }
                        Tool::Rect => { this.finish_note(cx); this.drawing = Some(Mark::Rect { color, width: RECT_WIDTH / view.scale, from: at, to: at }); }
                        Tool::Note => this.start_note(at, view, window, cx),
                    }
                    cx.notify();
                    return;
                }
                if let Some(view) = this.view { this.drag = Some((event.position, view)); cx.notify(); }
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.drawing.is_some() {
                    if event.pressed_button != Some(MouseButton::Left) { this.commit_drawing(); }
                    else if let Some(at) = this.image_point(event.position) {
                        match this.drawing.as_mut() {
                            Some(Mark::Pen { points, .. }) => points.push(at),
                            Some(Mark::Rect { to, .. }) => *to = at,
                            _ => {}
                        }
                    }
                    cx.notify();
                    return;
                }
                let Some((start, view)) = this.drag else { return };
                if event.pressed_button != Some(MouseButton::Left) { this.drag = None; cx.notify(); return; }
                let (Some(image), Some(stage)) = (this.image_size(), this.stage_size()) else { return };
                let moved = event.position - start;
                let next = clamp(View { x: view.x + f32::from(moved.x), y: view.y + f32::from(moved.y), ..view }, image, stage);
                if this.view != Some(next) { this.view = Some(next); cx.notify(); }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.drag = None;
                this.commit_drawing();
                cx.notify();
            }))
            // A medida chega no meio do desenho, onde `notify` não pede quadro novo: aplicada depois dele, o reencaixe
            // aparece sem esperar o próximo movimento do mouse.
            .child(canvas(move |bounds, window, cx| {
                if entity.read(cx).stage == Some(bounds) { return; }
                let entity = entity.clone();
                window.defer(cx, move |_, cx| entity.update(cx, |this, _| this.set_stage(bounds)));
            }, |_, _, _, _| {}).absolute().size_full())
            .child(body)
            .child(ink)
            .children(notes)
            .when(total > 1, |el| el
                .child(edge("viewer-previous", IconName::ChevronLeft, tr("viewer_previous"), false))
                .child(edge("viewer-next", IconName::ChevronRight, tr("viewer_next"), true)));

        div().id("viewer").track_focus(&self.focus).key_context("Viewer").on_key_down(cx.listener(Self::on_key))
            .flex().flex_col().gap_3().child(header).children(tools).child(stage)
    }
}

impl Hangar {
    /// Abre o visor em `sources[index]`; `sources` são as imagens da mensagem, na ordem em que aparecem.
    pub(super) fn open_image(&mut self, key: SessionKey, sources: Vec<Source>, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.api_for(&key.server) else { return };
        if index >= sources.len() { return; }
        let (hangar, runtime, uploads, full) = (cx.entity().downgrade(), self.runtime.clone(), self.uploads_for(&key), self.full_images.clone());
        let viewer = cx.new(|cx| {
            // Rede de segurança para um fechamento que não passe pelo `on_close`: a entidade morre com o diálogo.
            cx.on_release(|viewer: &mut Viewer, cx| {
                if let Some(job) = viewer.fetching.take() { job.abort(); }
                if let Shown::Image(image) = &viewer.shown { cx.drop_image(image.clone(), None); }
            }).detach();
            Viewer {
                hangar, api, uploads, runtime, full, key, sources, index, shown: Shown::Loading, seq: 0, _load: Task::ready(()), fetching: None,
                focus: cx.focus_handle(), stage: None, limit: None, held: None, view: None, drag: None,
                pen: false, pen_color: 0, pen_width: 1, tool: Tool::Rect, marks: Vec::new(), drawing: None, editing: None,
                burning: false, problem: None,
            }
        });
        viewer.update(cx, |viewer, cx| viewer.load(window, cx));
        let (shown, closed) = (viewer.clone(), viewer.clone());
        window.open_dialog(cx, move |dialog, window, cx| {
            // Do tamanho da imagem até o da janela, centrado; o construtor roda a cada quadro e acompanha zoom e troca.
            let (w, h) = shown.read(cx).stage_box(limit(window));
            let top = ((f32::from(window.viewport_size().height) - h - (CHROME_H - EDGE * 2.)) / 2.).max(EDGE);
            let closed = closed.clone();
            dialog.w(px(w + CHROME_W)).margin_top(px(top)).on_ok(enter_to_focused).child(shown.clone())
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
    use super::{MAX_SCALE, MIN_H, MIN_W, View, clamp, fitted, stage_for, zoom};

    #[test]
    fn fit_never_enlarges_and_centers() {
        assert_eq!(fitted((100., 50.), (400., 300.)), View { scale: 1., x: 150., y: 55. });
        let big = fitted((800., 400.), (400., 300.));
        assert_eq!((big.scale, big.x, big.y), (0.5, 0., 0.));
    }

    #[test]
    fn stage_hugs_the_image_between_the_minimum_and_the_window() {
        let limit = (1800., 900.);
        assert_eq!(stage_for((425., 41.), 1., limit), (MIN_W, MIN_H));
        assert_eq!(stage_for((425., 41.), 4., limit), (1700., 164.));
        assert_eq!(stage_for((4000., 3000.), 1., limit), limit);
    }

    #[test]
    fn zoom_keeps_the_anchor_and_stops_at_fit() {
        let (image, stage) = ((800., 400.), (400., 300.));
        let start = fitted(image, stage);
        let anchor = (100., 150.);
        let before = ((anchor.0 - start.x) / start.scale, (anchor.1 - start.y) / start.scale);
        let zoomed = zoom(start, image, stage, anchor, 2.);
        // O palco cresceu de 200 para 300 de altura, metade para cima: o ponto sob o cursor desceu 50.
        let after = ((anchor.0 - zoomed.x) / zoomed.scale, (anchor.1 + 50. - zoomed.y) / zoomed.scale);
        assert_eq!(zoomed.scale, 1.);
        assert!((before.0 - after.0).abs() < 0.01 && (before.1 - after.1).abs() < 0.01);
        assert_eq!(zoom(zoomed, image, stage, anchor, 0.01), start);
        assert_eq!(zoom(start, image, stage, anchor, 1e6).scale, MAX_SCALE);
    }

    #[test]
    fn burn_paints_the_stroke_scaled_to_the_original() {
        // Original 20×20 mostrada reduzida a 10×10: o traço no meio da mostrada cai no meio da original.
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(20, 20, image::Rgba([0, 0, 0, 255])))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let pen = super::Mark::Pen { color: 0xff0000, width: 2., points: vec![(2., 5.), (8., 5.)] };
        let rect = super::Mark::Rect { color: 0x00ff00, width: 1., from: (1., 1.), to: (9., 3.) };
        let out = image::load_from_memory(&super::burn(&png, (10., 10.), &[pen, rect]).unwrap()).unwrap().to_rgba8();
        assert_eq!(out.get_pixel(10, 10), &image::Rgba([255, 0, 0, 255]));
        assert_eq!(out.get_pixel(10, 2)[1], 255, "a borda de cima do retângulo, verde");
        assert_eq!(out.get_pixel(10, 4), &image::Rgba([0, 0, 0, 255]), "dentro do retângulo não pinta");
        assert_eq!(super::burned_name("colado-6.png"), "colado-6-riscado.png");
    }

    #[test]
    fn pan_never_shows_empty_stage() {
        let (image, stage) = ((800., 400.), (400., 300.));
        let view = View { scale: 1., x: 50., y: -500. };
        assert_eq!(clamp(view, image, stage), View { scale: 1., x: 0., y: -100. });
    }
}
