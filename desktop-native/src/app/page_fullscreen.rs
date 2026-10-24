//! Página da conversa em tela cheia, como o `HtmlRenderDialog` do T3: diálogo grande, zoom de 50% a 200% e fechar.
//! Viva (Linux e Windows) num motor próprio do tamanho do palco; no macOS, a imagem do servidor ampliada.
use std::{cell::Cell, rc::Rc};

use super::*;
use gpui_kit::component::input::{Editor, EditorState};

use super::page_card::{Miss, PageRef, miss, shot_width};
#[cfg(any(target_os = "linux", target_os = "windows"))]
use super::page_card::{GESTURE, HostMsg, Running, forward_key, parse_host};
#[cfg(any(target_os = "linux", target_os = "windows"))]
use crate::browser::{Engine, Pointer};

const ZOOM_MIN: f32 = 0.5;
const ZOOM_MAX: f32 = 2.;
const ZOOM_STEP: f32 = 0.25;
/// Roda (px) que vale um degrau: um dente do mouse (3 linhas de 16 px) é um degrau; o touchpad manda muitos eventos
/// pequenos, que se somam até ele.
const WHEEL_STEP: f32 = 40.;
/// Faixa de cima (título e zoom) mais o respiro do diálogo.
const CHROME_H: f32 = 72.;
/// Respiro dos lados e a borda.
const CHROME_W: f32 = 34.;
const MIN_W: f32 = 360.;
const MIN_H: f32 = 200.;
const EDGE: f32 = 16.;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ZoomAction { In, Out, Reset }

pub fn step_zoom(zoom: f32, action: ZoomAction) -> f32 {
    let next = match action {
        ZoomAction::Reset => return 1.,
        ZoomAction::In => zoom + ZOOM_STEP,
        ZoomAction::Out => zoom - ZOOM_STEP,
    };
    // Arredonda ao degrau: somas seguidas não acumulam erro.
    ((next / ZOOM_STEP).round() * ZOOM_STEP).clamp(ZOOM_MIN, ZOOM_MAX)
}

/// Nome do arquivo salvo: o título sem o que os sistemas de arquivo recusam, até 120 caracteres.
pub fn file_name(title: &str) -> String {
    let clean: String = title.chars().map(|c| if c.is_control() || "\\/:*?\"<>|".contains(c) { ' ' } else { c }).collect();
    let name: String = clean.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect();
    format!("{}.html", if name.trim().is_empty() { "pagina" } else { name.trim() })
}

/// Ação do atalho do app (Ctrl, Cmd ou Alt) que a tecla completa sozinha no contexto do foco; tecla de página (Ctrl+C,
/// Ctrl+A…) não tem. Tecla sem esses modificadores é da página, mesmo que o app a use em outro lugar.
fn app_shortcut(window: &Window, k: &Keystroke, cx: &App) -> Option<Box<dyn gpui_kit::Action>> {
    if !(k.modifiers.control || k.modifiers.platform || k.modifiers.alt) { return None; }
    // `possible_bindings_for_input` só devolve atalho de várias teclas que ainda espera a próxima; o completo vem daqui,
    // já sem os `NoAction`.
    let (bindings, _) = cx.key_bindings().borrow().bindings_for_input(std::slice::from_ref(k), &window.context_stack());
    bindings.into_iter().next().map(|binding| binding.action().boxed_clone())
}

pub fn zoom_label(zoom: f32) -> String { format!("{}%", (zoom * 100.).round() as i32) }

pub fn zoom_key(key: &str, modifiers: &Modifiers) -> Option<ZoomAction> {
    if !(modifiers.control || modifiers.platform) || modifiers.alt { return None; }
    match key {
        "=" | "+" => Some(ZoomAction::In),
        "-" | "_" => Some(ZoomAction::Out),
        "0" => Some(ZoomAction::Reset),
        _ => None,
    }
}

/// `dy` da GPUI: positivo é roda para cima, que amplia, como no visor de imagem.
pub fn wheel_zoom(pending: f32, dy: f32) -> (Option<ZoomAction>, f32) {
    let total = pending + dy;
    if total.abs() < WHEEL_STEP { return (None, total); }
    (Some(if total > 0. { ZoomAction::In } else { ZoomAction::Out }), 0.)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FullSize { pub dialog_w: f32, pub stage_w: f32, pub stage_h: f32, pub top: f32 }

/// Medidas do T3: 92% da largura e `min(86%, janela − 160)` de altura, centrado na vertical.
pub fn full_size((vw, vh): (f32, f32)) -> FullSize {
    let dialog_w = (vw * 0.92).max(MIN_W);
    let stage_w = (dialog_w - CHROME_W).max(1.);
    let stage_h = (vh * 0.86).min(vh - 160.).max(MIN_H);
    let top = ((vh - stage_h - CHROME_H) / 2.).max(EDGE);
    FullSize { dialog_w, stage_w, stage_h, top }
}

fn viewport(window: &Window) -> (f32, f32) {
    let size = window.viewport_size();
    (f32::from(size.width), f32::from(size.height))
}

#[derive(Clone, Copy, PartialEq)]
enum Body { Loading, Ready, Error, Expired, NoImage }

/// O que o cartão já tem da página.
pub(super) struct Source {
    pub page: PageRef,
    /// Documento já baixado (página viva); `None` no site e na imagem.
    pub html: Option<Rc<str>>,
    pub live: bool,
    /// Site: o último endereço que o cartão visitou.
    pub start: Option<String>,
}

/// De onde a página vem e para onde vão os quadros: o app, a sessão, o runtime dos pedidos e a janela do motor.
#[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
pub(super) struct Host {
    pub hangar: WeakEntity<Hangar>,
    pub api: Api,
    pub name: String,
    pub runtime: Arc<Runtime>,
    pub window: AnyWindowHandle,
}

// No macOS não há motor: janela, gesto e aperto só servem à página viva.
#[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
pub(super) struct PageFullscreen {
    host: Host,
    source: Source,
    body: Body,
    zoom: f32,
    wheel: f32,
    size: FullSize,
    focus: FocusHandle,
    origin: Rc<Cell<Point<Pixels>>>,
    gesture: Option<Instant>,
    pressed: bool,
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    running: Option<Running>,
    /// Imagem estática e o tema (escuro?) em que foi tirada.
    shot: Option<(bool, Arc<RenderImage>)>,
    /// Tema da última imagem pedida: a troca de tema pede uma vez, não a cada quadro.
    asked_dark: bool,
    /// Pedido à vista: resposta de um anterior (nova tentativa, tema trocado) não entra.
    seq: u64,
    _load: Task<()>,
    /// Mostrando a Fonte no lugar da página.
    showing_source: bool,
    /// O HTML como o agente escreveu (sem o tema e a ponte), para a Fonte, copiar e salvar; `None` no site.
    source_text: Option<Result<SharedString, ()>>,
    source_view: Option<Entity<EditorState>>,
    /// Resultado de Salvar: o caminho gravado ou a falha.
    saved: Option<(String, bool)>,
    _source: Task<()>,
}

impl PageFullscreen {
    /// O código da página, ao abrir o diálogo (Copiar e Salvar ficam prontos) e de novo só se falhou. Site não tem.
    fn fetch_source(&mut self, cx: &mut Context<Self>) {
        if self.source.page.url.is_some() { return; }
        self.source_text = None;
        let (api, name, id) = (self.host.api.clone(), self.host.name.clone(), self.source.page.id.clone());
        let task = self.host.runtime.spawn(async move { api.page(&name, &id, &[], &[("raw", "1"), ("source", "1")]).await });
        self._source = cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|_| Err(Failure::local("network_error")));
            let _ = this.update(cx, |this, cx| {
                this.source_text = Some(result.map(|bytes| String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()).into())
                    .map_err(|error| eprintln!("[pagina] fonte da página não veio: {}", error.detail)));
                cx.notify();
            });
        });
    }

    fn toggle_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.showing_source = !self.showing_source;
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        if self.showing_source && let Some(running) = &self.running { running.engine.hide(); }
        if !self.showing_source { window.focus(&self.focus, cx); } else if matches!(self.source_text, Some(Err(()))) { self.fetch_source(cx); }
        cx.notify();
    }

    /// Salvar pergunta o destino, na pasta de downloads, como o anexo.
    fn save_source(&mut self, cx: &mut Context<Self>) {
        let Some(Ok(text)) = self.source_text.clone() else { return };
        let prompt = cx.prompt_for_new_path(&super::downloads_folder(), Some(&file_name(&self.source.page.title)));
        self._source = cx.spawn(async move |this, cx| {
            let saved = match prompt.await {
                Ok(Ok(Some(path))) => {
                    let written = cx.background_executor().spawn(async move { std::fs::write(&path, text.as_bytes()).map(|()| path) }).await;
                    Some(match written {
                        Ok(path) => (tr("saved").replace("{path}", &path.display().to_string()), false),
                        Err(_) => (tr("save_failed"), true),
                    })
                }
                Ok(Ok(None)) => None,
                // O diálogo do sistema falhou: dizer, não parecer que Salvar não fez nada.
                _ => Some((tr("save_failed"), true)),
            };
            let _ = this.update(cx, |this, cx| { this.saved = saved; cx.notify(); });
        });
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.body = Body::Loading;
        self.seq += 1;
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        if self.source.live {
            self.running = None;
            let starter = match Engine::prepare(window, cx) {
                Ok(starter) => starter,
                Err(error) => { eprintln!("[pagina] tela cheia sem motor: {error}"); self.body = Body::Error; return; }
            };
            // Sem limite: o motor manda com `try_send` e quadro perdido não volta.
            let (events, received) = async_channel::unbounded();
            let (url, html, seq, width) = (self.source.start.clone(), self.source.html.clone(), self.seq, self.size.stage_w);
            // Tema próprio nasce no branco do navegador, como a página foi desenhada.
            let background = self.source.page.own_theme.then_some((255, 255, 255));
            self._load = cx.spawn(async move |this, cx| {
                let engine = match url {
                    Some(url) => starter.start_url(url, events).await,
                    None => starter.start_page(html.unwrap_or_else(|| "".into()), width, background, events).await,
                }.map(Rc::new);
                let _ = this.update(cx, |this, cx| this.engine_ready(seq, engine, received, cx));
            });
            return;
        }
        let _ = window;
        self.fetch_shot(cx);
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    fn engine_ready(&mut self, seq: u64, engine: Result<Rc<Engine>, String>, received: async_channel::Receiver<crate::browser::Event>,
        cx: &mut Context<Self>) {
        if seq != self.seq { return; }
        let engine = match engine {
            Ok(engine) => engine,
            Err(error) => { eprintln!("[pagina] motor da tela cheia falhou: {error}"); self.body = Body::Error; cx.notify(); return; }
        };
        if self.zoom != 1. { engine.set_zoom(self.zoom); }
        let drain = cx.spawn(async move |this, cx| {
            while let Ok(event) = received.recv().await {
                if this.update(cx, |this, cx| this.event(event, cx)).is_err() { break; }
            }
        });
        self.running = Some(Running { engine, _drain: drain, framed: false, theme_sent: None, at: None });
        self.body = Body::Ready;
        cx.notify();
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    fn event(&mut self, event: crate::browser::Event, cx: &mut Context<Self>) {
        let Some(running) = &mut self.running else { return };
        match event {
            crate::browser::Event::Frame => {
                running.framed = true;
                // No Windows é aqui que o quadro vai para a textura.
                let _ = running.engine.texture_replaced();
                let engine = Rc::downgrade(&running.engine);
                let _ = cx.update_window(self.host.window, move |_, window, _| {
                    window.on_next_frame(move |_, _| if let Some(engine) = engine.upgrade() { engine.frame_seen() });
                });
                cx.notify();
            }
            // Link só logo depois de um clique ou tecla na página: sozinha ela não abre o navegador.
            crate::browser::Event::Host(raw) => if let Some(HostMsg::Link(url, external)) = parse_host(&raw)
                && self.gesture.take().is_some_and(|at| at.elapsed() < GESTURE) {
                super::links::open_link_later(self.host.window, url, external, true, cx);
            },
            crate::browser::Event::State(state) => running.at = state.url,
            crate::browser::Event::UserInput | crate::browser::Event::Escape | crate::browser::Event::Popup(_) | crate::browser::Event::Closed => {}
        }
    }

    fn fetch_shot(&mut self, cx: &mut Context<Self>) {
        let dark = theme::is_dark();
        self.asked_dark = dark;
        let (api, name, id, seq) = (self.host.api.clone(), self.host.name.clone(), self.source.page.id.clone(), self.seq);
        let width = shot_width(self.size.stage_w).to_string();
        let task = self.host.runtime.spawn(async move {
            let bytes = api.page(&name, &id, &["shot"], &[("theme", if dark { "dark" } else { "light" }), ("width", &width)]).await?;
            Ok::<_, Failure>(tokio::task::spawn_blocking(move || media::decode(&bytes, 4096, 8192, None)).await.ok().flatten())
        });
        self._load = cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|_| Err(Failure::local("network_error")));
            let _ = this.update(cx, |this, cx| {
                if this.seq != seq { return; }
                this.body = match result {
                    Ok(Some(image)) => {
                        if let Some((_, old)) = this.shot.replace((dark, image)) { cx.drop_image(old, None); }
                        Body::Ready
                    }
                    Ok(None) => Body::Error,
                    Err(error) => match miss(error.status, error.code.as_deref()) {
                        Miss::NoImage => Body::NoImage,
                        Miss::Expired => Body::Expired,
                        Miss::Other => Body::Error,
                    },
                };
                cx.notify();
            });
        });
    }

    fn zoom_to(&mut self, action: ZoomAction, cx: &mut Context<Self>) {
        let next = step_zoom(self.zoom, action);
        if next == self.zoom { return; }
        self.zoom = next;
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        if let Some(running) = &self.running { running.engine.set_zoom(next); }
        // O `notify` também pede o quadro: no Windows o zoom só vale no próximo `place`.
        cx.notify();
    }

    fn wheel_by(&mut self, dy: f32, cx: &mut Context<Self>) {
        let (action, rest) = wheel_zoom(self.wheel, dy);
        self.wheel = rest;
        if let Some(action) = action { self.zoom_to(action, cx); }
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    fn pointer(&mut self, kind: Pointer, at: Point<Pixels>, clicks: usize, modifiers: &Modifiers) {
        match kind {
            Pointer::Down => self.pressed = true,
            // Soltura de um arrasto que não começou na página não é dela.
            Pointer::Up if !std::mem::take(&mut self.pressed) => return,
            _ => {}
        }
        if let Some(running) = &self.running { running.engine.pointer(kind, at - self.origin.get(), clicks, modifiers); }
    }

    /// Ctrl+=/-/0 amplia em qualquer modo; atalho do app roda nele; o resto vai à página viva. O Esc nem chega aqui: o
    /// diálogo do kit o trata antes (ação `Cancel`) e fecha.
    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        if let Some(action) = zoom_key(&k.key, &k.modifiers) { cx.stop_propagation(); self.zoom_to(action, cx); return; }
        // O atalho casa aqui também, mas quem o trata é a raiz do app, fora do diálogo: a tela cheia sai da frente e ele
        // roda como fora dela, como com a página do cartão em foco.
        if let Some(action) = app_shortcut(window, k, cx) && let Some(hangar) = self.host.hangar.upgrade() {
            cx.stop_propagation();
            let root = hangar.read(cx).root_focus.clone();
            window.close_dialog(cx);
            window.focus(&root, cx);
            window.dispatch_action(action, cx);
            return;
        }
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        if let Some(running) = self.running.as_ref().filter(|_| !self.showing_source) {
            cx.stop_propagation();
            self.gesture = Some(Instant::now());
            forward_key(&running.engine, k, cx);
        }
    }

    fn on_key_up(&mut self, event: &KeyUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        if let Some(running) = self.running.as_ref().filter(|_| !self.showing_source) { cx.stop_propagation(); running.engine.key(false, &event.keystroke); }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        let _ = (event, cx);
    }

    fn note(text: String, role: Role, id: &'static str) -> Stateful<Div> {
        div().id(id).size_full().flex().items_center().justify_center().px_6().role(role)
            .text_sm().text_color(theme::muted()).whitespace_normal().child(text)
    }
}

impl Render for PageFullscreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let size = full_size(viewport(window));
        self.size = size;
        let title = self.source.page.title.clone();
        // Imagem estática no tema antigo: a do tema novo vem sem fechar o diálogo.
        if !self.source.live && self.body == Body::Ready && self.asked_dark != theme::is_dark() {
            self.seq += 1;
            self.fetch_shot(cx);
        }
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        if let Some(running) = self.running.as_mut().filter(|r| r.framed && !self.source.page.own_theme && self.source.page.url.is_none()) {
            running.push_theme();
        }
        let zoom = self.zoom;
        let has_source = self.source.page.url.is_none();
        let source_ready = matches!(self.source_text, Some(Ok(_)));
        let source_actions = div().flex().items_center().gap_1()
            .child(Button::new("page-full-view").ghost().small()
                .icon(if self.showing_source { IconName::Eye } else { IconName::Code })
                .label(tr_shared(if self.showing_source { "page_view_page" } else { "page_view_source" }, &[]))
                .on_click(cx.listener(|this, _, window, cx| this.toggle_source(window, cx))))
            .children(match &self.source_text {
                Some(Ok(text)) => Some(gpui_kit::component::clipboard::Clipboard::new("page-full-copy").value(text.clone())
                    .tooltip(tr_shared("page_copy_source", &[])).into_any_element()),
                _ => None,
            })
            .child(Button::new("page-full-save").ghost().small().icon(IconName::Download).tooltip(tr_shared("page_save_file", &[]))
                .accessibility_label(tr_shared("page_save_file", &[])).disabled(!source_ready)
                .on_click(cx.listener(|this, _, _, cx| this.save_source(cx))));
        let saved = self.saved.clone().map(|(text, failed)| div().max_w(px(320.)).truncate().text_xs()
            .text_color(if failed { theme::warning() } else { theme::muted() }).child(text));
        let header = div().flex().items_center().gap_2().pr(px(36.)).h(px(28.))
            .child(div().flex_1().min_w_0().truncate().text_sm().text_color(theme::text()).child(title.clone()))
            .children(saved)
            .when(has_source, |el| el.child(source_actions))
            .when(!self.showing_source, |el| el.child(div().flex().items_center().gap_1()
                .child(Button::new("page-full-zoom-out").ghost().small().icon(IconName::Minus)
                    .tooltip(tr_shared("page_zoom_out", &[])).accessibility_label(tr_shared("page_zoom_out", &[])).disabled(zoom <= ZOOM_MIN)
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_to(ZoomAction::Out, cx))))
                // Largura fixa: o número mudando de tamanho não empurra os botões.
                .child(Button::new("page-full-zoom-reset").ghost().small().w(px(56.)).label(zoom_label(zoom))
                    .tooltip(tr_shared("page_zoom_reset", &[]))
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_to(ZoomAction::Reset, cx))))
                .child(Button::new("page-full-zoom-in").ghost().small().icon(IconName::Plus)
                    .tooltip(tr_shared("page_zoom_in", &[])).accessibility_label(tr_shared("page_zoom_in", &[])).disabled(zoom >= ZOOM_MAX)
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_to(ZoomAction::In, cx))))))
            .when(!self.source.live, |el| {
                let (hangar, id) = (self.host.hangar.clone(), self.source.page.id.clone());
                el.child(Button::new("page-full-open").ghost().small().icon(IconName::ExternalLink).label(tr_shared("page_open_browser", &[]))
                    .on_click(move |_, _, cx| { let _ = hangar.update(cx, |hangar, cx| hangar.open_page_in_browser(id.clone(), cx)); }))
            });

        // Roda na fase de captura: Ctrl+roda amplia em qualquer modo; sem Ctrl, a roda é da página viva (a imagem
        // estática rola no próprio contêiner).
        let me = cx.entity().downgrade();
        let origin = self.origin.clone();
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        let shown = self.running.as_ref().map(|r| r.engine.clone());
        let surface = canvas(|bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal), move |bounds, hitbox, window, _| {
            origin.set(bounds.origin);
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            if let Some(engine) = &shown { engine.place(bounds, window); }
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            let engine = shown.clone();
            let me = me.clone();
            window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture || !hitbox.should_handle_scroll(window) { return; }
                let delta = event.delta.pixel_delta(px(16.));
                if event.modifiers.control || event.modifiers.platform {
                    let _ = me.update(cx, |this, cx| this.wheel_by(f32::from(delta.y), cx));
                    cx.stop_propagation();
                    return;
                }
                #[cfg(any(target_os = "linux", target_os = "windows"))]
                if let Some(engine) = &engine {
                    engine.wheel(event.position - bounds.origin, point(-delta.x, -delta.y));
                    cx.stop_propagation();
                }
            });
        }).absolute().inset_0();

        let title_for = title.clone();
        let body = match self.body {
            Body::Loading => Self::note(tr_shared("page_loading", &[("title", &title_for)]), Role::Status, "page-full-loading").into_any_element(),
            Body::Expired => Self::note(tr_shared("page_expired", &[]), Role::Status, "page-full-expired").into_any_element(),
            Body::NoImage => Self::note(tr_shared("page_no_image", &[]), Role::Status, "page-full-no-image").into_any_element(),
            Body::Error => v_flex().id("page-full-error").size_full().items_center().justify_center().gap_2().role(Role::Alert)
                .child(div().text_sm().text_color(theme::warning()).child(tr_shared("page_error", &[("title", &title_for)])))
                .child(Button::new("page-full-retry").ghost().small().label(tr_shared("page_retry", &[]))
                    .on_click(cx.listener(|this, _, window, cx| this.start(window, cx))))
                .into_any_element(),
            // Começa na largura do palco e o zoom amplia a partir dela.
            Body::Ready if !self.source.live => match &self.shot {
                Some((_, image)) => {
                    let natural = image.size(0);
                    let ratio = natural.height.0 as f32 / natural.width.0.max(1) as f32;
                    let w = size.stage_w * zoom;
                    div().id("page-full-scroll").size_full().overflow_scroll()
                        .child(img(image.clone()).w(px(w)).h(px(w * ratio)))
                        .into_any_element()
                }
                None => Self::note(tr_shared("page_loading", &[("title", &title_for)]), Role::Status, "page-full-loading").into_any_element(),
            },
            Body::Ready => {
                #[cfg(any(target_os = "linux", target_os = "windows"))]
                let framed = self.running.as_ref().is_some_and(|r| r.framed);
                #[cfg(not(any(target_os = "linux", target_os = "windows")))]
                let framed = false;
                match framed {
                    true => div().into_any_element(),
                    false => Self::note(tr_shared("page_loading", &[("title", &title_for)]), Role::Status, "page-full-loading").into_any_element(),
                }
            }
        };

        if self.showing_source && self.source_view.is_none() && let Some(Ok(text)) = &self.source_text {
            let text = text.clone();
            let view = cx.new(|cx| EditorState::new(window, cx).language("html").default_value(text).soft_wrap(true));
            view.update(cx, |state, cx| state.set_readonly(true, cx));
            self.source_view = Some(view);
        }
        let stage = div().id("page-full-stage").relative().w(px(size.stage_w)).h(px(size.stage_h)).overflow_hidden().rounded_md()
            .bg(theme::inset());
        if self.showing_source {
            let source = match (&self.source_text, &self.source_view) {
                (Some(Err(())), _) => Self::note(tr_shared("page_source_error", &[]), Role::Alert, "page-full-source-error").into_any_element(),
                (_, Some(view)) => div().id("page-full-source").size_full()
                    .child(Editor::new(view).readonly(true).bordered(false).h_full().font_family(theme::MONO).text_sm())
                    .into_any_element(),
                _ => Self::note(tr_shared("page_loading", &[("title", &title_for)]), Role::Status, "page-full-source-loading").into_any_element(),
            };
            // Atalho do app vale também com o foco no código.
            return div().id("page-full").flex().flex_col().gap_3().child(header)
                .child(stage.on_key_down(cx.listener(Self::on_key)).child(source));
        }
        let stage = stage.track_focus(&self.focus).key_context("PageFullscreen BrowserPage").aria_label(title)
            .on_key_down(cx.listener(Self::on_key)).on_key_up(cx.listener(Self::on_key_up))
            .child(surface)
            .child(body);
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        let stage = stage
            .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, window, cx| {
                window.focus(&this.focus, cx);
                this.gesture = Some(Instant::now());
                this.pointer(Pointer::Down, event.position, event.click_count, &event.modifiers);
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, event: &MouseUpEvent, _, _| this.pointer(Pointer::Up, event.position, 0, &event.modifiers)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, event: &MouseUpEvent, _, _| this.pointer(Pointer::Up, event.position, 0, &event.modifiers)))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, _| this.pointer(Pointer::Move, event.position, 0, &event.modifiers)));
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        let stage = stage.on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, window, cx| window.focus(&this.focus, cx)));

        div().id("page-full").flex().flex_col().gap_3().child(header).child(stage)
    }
}

/// Abre o diálogo. A entidade cai com ele (Esc, fora, X ou `close_dialog`): o motor fecha alvo e contexto no `Drop`,
/// a imagem sai do atlas e a vaga do orçamento volta.
pub(super) fn open(host: Host, source: Source, window: &mut Window, cx: &mut App) {
    let live = source.live;
    let full = cx.new(|cx| {
        let pin = host.hangar.clone();
        cx.on_release(move |full: &mut PageFullscreen, cx| {
            if let Some((_, image)) = full.shot.take() { cx.drop_image(image, None); }
            if live { let _ = pin.update(cx, |hangar, _| hangar.pages.unpin()); }
        }).detach();
        PageFullscreen {
            host, source,
            body: Body::Loading, zoom: 1., wheel: 0., size: full_size(viewport(window)), focus: cx.focus_handle(), origin: Rc::default(),
            gesture: None, pressed: false,
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            running: None,
            shot: None, asked_dark: theme::is_dark(), seq: 0, _load: Task::ready(()),
            showing_source: false, source_text: None, source_view: None, saved: None, _source: Task::ready(()),
        }
    });
    full.update(cx, |full, cx| { full.start(window, cx); full.fetch_source(cx); });
    let shown = full.clone();
    window.open_dialog(cx, move |dialog, window, _| {
        let size = full_size(viewport(window));
        dialog.w(px(size.dialog_w)).margin_top(px(size.top)).child(shown.clone())
    });
    // O kit põe o foco no diálogo ao abrir; teclado e zoom precisam dele no palco.
    let focus = full.read(cx).focus.clone();
    window.defer(cx, move |window, cx| window.focus(&focus, cx));
}

#[cfg(test)]
mod tests {
    // Sem `super::*`: o glob da gpui_kit traz um `test` próprio que sombreia o `#[test]` da std.
    use super::{FullSize, ZoomAction, file_name, full_size, step_zoom, wheel_zoom, zoom_key, zoom_label};
    use gpui_kit::Modifiers;

    #[test]
    fn zoom_steps_between_half_and_double() {
        assert_eq!(step_zoom(1., ZoomAction::In), 1.25);
        assert_eq!(step_zoom(1., ZoomAction::Out), 0.75);
        assert_eq!(step_zoom(2., ZoomAction::In), 2.);
        assert_eq!(step_zoom(0.5, ZoomAction::Out), 0.5);
        assert_eq!(step_zoom(1.75, ZoomAction::Reset), 1.);
        assert_eq!(zoom_label(1.25), "125%");
        assert_eq!(zoom_label(0.5), "50%");
    }

    #[test]
    fn zoom_key_maps_ctrl_combos_only() {
        let ctrl = Modifiers { control: true, ..Default::default() };
        let cmd = Modifiers { platform: true, ..Default::default() };
        let ctrl_alt = Modifiers { control: true, alt: true, ..Default::default() };
        assert_eq!(zoom_key("=", &ctrl), Some(ZoomAction::In));
        assert_eq!(zoom_key("+", &ctrl), Some(ZoomAction::In));
        assert_eq!(zoom_key("-", &ctrl), Some(ZoomAction::Out));
        assert_eq!(zoom_key("0", &ctrl), Some(ZoomAction::Reset));
        assert_eq!(zoom_key("=", &cmd), Some(ZoomAction::In));
        assert_eq!(zoom_key("=", &Modifiers::default()), None, "sem Ctrl a tecla é da página");
        assert_eq!(zoom_key("=", &ctrl_alt), None);
        assert_eq!(zoom_key("a", &ctrl), None);
    }

    #[test]
    fn wheel_zoom_accumulates_until_a_step() {
        assert_eq!(wheel_zoom(0., 10.), (None, 10.));
        assert_eq!(wheel_zoom(45., 10.), (Some(ZoomAction::In), 0.));
        assert_eq!(wheel_zoom(0., -120.), (Some(ZoomAction::Out), 0.));
    }

    #[test]
    fn file_name_comes_from_the_title() {
        assert_eq!(file_name("Prova de redimensionamento"), "Prova de redimensionamento.html");
        assert_eq!(file_name(" a/b: c*d?  \"e\" <f>|g\\h \t"), "a b c d e f g h.html");
        assert_eq!(file_name("///"), "pagina.html", "título só de símbolos proibidos");
        assert_eq!(file_name(&"x".repeat(300)).chars().count(), 120 + ".html".len());
    }

    #[test]
    fn dialog_is_large_like_t3_and_never_collapses() {
        assert_eq!(full_size((1600., 1000.)), FullSize { dialog_w: 1472., stage_w: 1438., stage_h: 840., top: 44. });
        let small = full_size((500., 300.));
        assert_eq!((small.dialog_w, small.stage_h, small.top), (460., 200., 16.));
    }
}
