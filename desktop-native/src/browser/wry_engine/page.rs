//! Página da conversa no Windows: uma WebView2 própria, estacionada fora da área da janela, que o app pinta pelo
//! screencast (`paint_surface`) e dirige por CDP, como o Chromium sem janela do Linux.
use std::{cell::Cell, future::Future, rc::Rc, sync::Arc, time::Duration};

use gpui_kit::*;
use serde_json::{Value, json};
use wry::{NewWindowResponse, Rect, WebContext, WebViewBuilder, WebViewBuilderExtWindows, WebViewExtWindows, dpi::{LogicalPosition, LogicalSize, PhysicalPosition, PhysicalSize}};

use super::{BROWSER_ARGS, Engine, SAME_TAB, Starter};
use crate::browser::{Event, Pointer, cdp::Cdp, frames::Frames, keys::modifier_bits, model};

/// Fora da área do pai, que recorta a janela filha; longe demais estoura os 16 bits do WM_MOVE em DPI alto.
const PARKED_X: f64 = -3000.0;

pub(super) struct Page {
    pub(super) cdp: Rc<Cdp>,
    pub(super) frames: Arc<Frames>,
    placed: Cell<Option<(Size<Pixels>, f32)>>,
    visible: Rc<Cell<bool>>,
    pressed: Cell<bool>,
    zoom: Cell<f32>,
    /// O zoom mudou e o próximo `place` agenda o reinício do screencast depois de assentar.
    zoomed: Cell<bool>,
    /// Contagem das colocações: o reinício agendado desiste se outra (clique da série ou resize) veio depois.
    zoom_turn: Rc<Cell<u64>>,
    executor: ForegroundExecutor,
    background: BackgroundExecutor,
    _acks: Task<()>,
}

/// Espera depois do último clique de zoom antes de reiniciar o screencast.
const ZOOM_SETTLE_MS: u64 = 120;

impl Starter {
    /// Documento do agente posto direto na página. Sem `background` o fundo é transparente; com ele, opaco nessa cor.
    pub fn start_page(self, html: Rc<str>, width: f32, background: Option<(u8, u8, u8)>, events: async_channel::Sender<Event>)
        -> impl Future<Output = Result<Engine, String>> + use<> {
        // O WebView2 nasce aqui, fora de qualquer empréstimo da App (ver `start`).
        let opened = self.open(None, width, events);
        async move {
            let engine = opened?;
            let cdp = engine.page.as_ref().map(|page| page.cdp.clone()).ok_or("motor da pagina sem CDP")?;
            // Ponte do script da página (`window.hangarHost`, ver `theme.rs` do servidor): o documento inicial da WebView2
            // não tem `chrome.webview` nem roda script de inicialização, e o binding do CDP vale em qualquer documento.
            cdp.call("Runtime.enable", json!({})).await?;
            cdp.call("Runtime.addBinding", json!({"name": "hangarHost"})).await?;
            let (r, g, b, a) = background.map_or((0, 0, 0, 0), |(r, g, b)| (r, g, b, 1));
            cdp.call("Emulation.setDefaultBackgroundColorOverride", json!({"color": {"r": r, "g": g, "b": b, "a": a}})).await?;
            let tree = cdp.call("Page.getFrameTree", json!({})).await?;
            let frame = tree["frameTree"]["frame"]["id"].as_str().unwrap_or_default().to_owned();
            cdp.call("Page.setDocumentContent", json!({"frameId": frame, "html": &*html})).await?;
            Ok(engine)
        }
    }

    /// Site de verdade na conversa: o perfil do painel (cookies e login) e navegação livre dentro dele.
    pub fn start_url(self, url: String, events: async_channel::Sender<Event>) -> impl Future<Output = Result<Engine, String>> + use<> {
        std::future::ready(self.open(Some(url), 728., events))
    }

    fn open(self, url: Option<String>, width: f32, events: async_channel::Sender<Event>) -> Result<Engine, String> {
        let site = url.is_some();
        // O documento tem pasta própria, sem os cookies do painel.
        let mut context = WebContext::new(crate::appearance::dir().map(|dir| dir.join(if site { "browser" } else { "pages" })));
        let builder = WebViewBuilder::new_with_web_context(&mut context)
            .with_visible(false).with_focused(false).with_additional_browser_args(BROWSER_ARGS).with_transparent(true)
            .with_bounds(Rect {
                position: LogicalPosition::new(PARKED_X, 0.0).into(),
                size: LogicalSize::new(width as f64, 600.0).into(),
            })
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny);
        let builder = match url {
            Some(url) => {
                let loaded = events.clone();
                builder.with_initialization_script(SAME_TAB).with_url(url)
                    .with_navigation_handler(|url| model::allowed_request(&url))
                    // Endereço atual do site, para a faixa do cartão.
                    .with_on_page_load_handler(move |_, url| {
                        let _ = loaded.try_send(Event::State(model::PageState { url: Some(url), ..Default::default() }));
                    })
            }
            // O documento já chega pelo CDP: a página não navega para outro (link vai pela ponte).
            None => builder.with_navigation_handler(|_| false),
        };
        let view = builder.build_as_child(&self).map_err(|e| e.to_string())?;
        // Escondida de verdade (`SW_HIDE`) o WebView2 para de gerar quadros; estacionada visível, ninguém a vê.
        view.set_visible(true).map_err(|e| e.to_string())?;
        let cdp = Rc::new(Cdp::new(view.webview()));
        let (acks, pending) = async_channel::unbounded::<Value>();
        let frames = Frames::new(events.clone(), Box::new(move |id| { let _ = acks.try_send(id); }));
        let sink = frames.clone();
        cdp.on("Page.screencastFrame", move |params| sink.frame(params))?;
        if !site {
            let host = events.clone();
            cdp.on("Runtime.bindingCalled", move |params| {
                if params["name"] == "hangarHost" { let _ = host.try_send(Event::Host(params["payload"].as_str().unwrap_or("").to_owned())); }
            })?;
        }
        let dead = Rc::new(Cell::new(false));
        let fell = dead.clone();
        cdp.on("Inspector.targetCrashed", move |_| fell.set(true))?;
        cdp.fire("Page.enable", json!({}));
        cdp.fire("Inspector.enable", json!({}));
        // Várias páginas abertas não disputam o foco: a página finge estar sempre focada.
        cdp.fire("Emulation.setFocusEmulationEnabled", json!({"enabled": true}));
        // A confirmação volta ao WebView2 na thread da interface, dona dele.
        let weak = Rc::downgrade(&cdp);
        let acks = self.executor.spawn(async move {
            while let Ok(id) = pending.recv().await {
                let Some(cdp) = weak.upgrade() else { break };
                cdp.fire("Page.screencastFrameAck", json!({"sessionId": id}));
            }
        });
        let page = Page { cdp, frames, placed: Cell::new(None), visible: Rc::default(), pressed: Cell::new(false), zoom: Cell::new(1.),
            zoomed: Cell::new(false), zoom_turn: Rc::default(), executor: self.executor.clone(), background: self.background.clone(), _acks: acks };
        Ok(Engine {
            view, _context: context, state: Rc::default(), events, placed: Cell::new(None), asleep: Cell::new(false), dead,
            _watch: None, page: Some(page),
        })
    }
}

impl Page {
    /// Chamado na pintura do cartão, a cada quadro visível.
    pub(super) fn place(&self, view: &wry::WebView, bounds: Bounds<Pixels>, window: &mut Window) {
        let scale = window.scale_factor();
        // Desenhada no dobro da tela e reduzida pela GPU: em 1x o texto da página sai borrado ao lado do texto do app.
        // Arredondado para cima: em fator inteiro a WebView2 desenha as letras mais nítidas do que no dobro exato
        // (2,5 numa tela a 125%), mesmo com a redução saindo em razão quebrada.
        let frame_scale = (scale * 2.).ceil();
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        // Device perdido leva a textura, e página parada não manda quadro novo: o screencast recomeça como num resize.
        if self.frames.lost() { self.placed.set(None); }
        let resized = self.placed.get() != Some((bounds.size, scale));
        let first = !self.visible.replace(true);
        if resized || first {
            crate::browser::frames::trace(format_args!("cartão {w}x{h} (primeiro: {first}); screencast reiniciado"));
            self.placed.set(Some((bounds.size, scale)));
            // A densidade emulada pelo CDP não chega ao screencast da WebView2: quem desenha no dobro é a escala de
            // rasterização dela, com a janela no tamanho físico correspondente (a página continua vendo w×h de CSS).
            // Zoom da tela cheia: a WebView2 rasteriza mais denso no mesmo tamanho físico, e a página vê `w/zoom` de CSS.
            rasterize(view, frame_scale * self.zoom.get());
            let (pw, ph) = ((w * frame_scale).round().max(1.) as u32, (h * frame_scale).round().max(1.) as u32);
            if let Err(e) = view.set_bounds(Rect {
                // Toda à esquerda da área do pai, que recorta a janela filha.
                position: PhysicalPosition::new(-(pw as i32) - PARK_GAP, 0).into(),
                size: PhysicalSize::new(pw, ph).into(),
            }) { eprintln!("[pagina] tamanho da WebView2 nao aplicado: {e}"); }
            // Reiniciar sem parar o anterior a WebView2 recusa ("The parameter is incorrect").
            if !first { self.cdp.fire("Page.stopScreencast", json!({})); }
            let screencast = json!({"format": "png", "maxWidth": pw, "maxHeight": ph, "everyNthFrame": 1});
            self.cdp.fire("Page.startScreencast", screencast.clone());
            // Toda colocação nova avança a contagem: um reinício ainda esperando voltaria com o tamanho antigo.
            let turn = self.zoom_turn.get() + 1;
            self.zoom_turn.set(turn);
            if self.zoomed.replace(false) {
                // Página parada: o quadro do reinício pode sair antes da rasterização nova, e nenhum outro chega sozinho.
                // Reiniciar depois de assentar manda o do zoom final.
                let (cdp, turns, visible) = (self.cdp.clone(), self.zoom_turn.clone(), self.visible.clone());
                let timer = self.background.timer(Duration::from_millis(ZOOM_SETTLE_MS));
                self.executor.spawn(async move {
                    timer.await;
                    if turns.get() == turn && visible.get() {
                        cdp.fire("Page.stopScreencast", json!({}));
                        cdp.fire("Page.startScreencast", screencast);
                    }
                }).detach();
            }
        }
        self.frames.seen();
        if let Some(texture) = self.frames.shown() {
            // A WebView2 desenha na densidade pedida: os pixels dão o tamanho lógico do quadro.
            let frame = (texture.width() as f32 / frame_scale, texture.height() as f32 / frame_scale);
            let bounds = crate::browser::frame_rect(bounds, frame, scale);
            window.paint_surface(bounds, texture);
        }
    }

    /// Zoom da página em tela cheia: o próximo `place` refaz a escala e o screencast. Quem chama pede o quadro novo
    /// (`cx.notify`), senão o `place` só vem na próxima pintura.
    pub(super) fn set_zoom(&self, zoom: f32) {
        let zoom = crate::browser::valid_zoom(zoom);
        if self.zoom.replace(zoom) == zoom { return; }
        self.placed.set(None);
        self.zoomed.set(true);
    }

    /// Fora da tela: sem screencast e sem a textura na GPU.
    pub(super) fn hide(&self) {
        if self.visible.replace(false) { self.cdp.fire("Page.stopScreencast", json!({})); }
        self.frames.clear();
    }

    /// O `clickCount` da soltura repete o do aperto: sem ele o Chromium não dispara `dblclick`.
    pub(super) fn mouse(&self, kind: Pointer, button: MouseButton, at: Point<Pixels>, clicks: usize, modifiers: &Modifiers) {
        let at = crate::browser::unzoom(at, self.zoom.get());
        let (x, y) = (f32::from(at.x) as f64, f32::from(at.y) as f64);
        let (name, bit) = match button { MouseButton::Right => ("right", 2), MouseButton::Middle => ("middle", 4), _ => ("left", 1) };
        let left = name == "left";
        let (kind, button, buttons, clicks) = match kind {
            Pointer::Down => {
                crate::browser::frames::trace(format_args!("clique enviado à página"));
                if left { self.pressed.set(true) }
                ("mousePressed", name, bit, clicks.max(1))
            }
            Pointer::Up => { if left { self.pressed.set(false) } ("mouseReleased", name, 0, clicks.max(1)) }
            Pointer::Move if self.pressed.get() => ("mouseMoved", "left", 1, 0),
            Pointer::Move => ("mouseMoved", "none", 0, 0),
        };
        self.cdp.fire("Input.dispatchMouseEvent", json!({"type": kind, "x": x, "y": y, "button": button, "buttons": buttons,
            "clickCount": clicks, "modifiers": modifier_bits(modifiers)}));
    }

    /// `delta.y > 0` avança a página; quem chama já inverteu o sinal da GPUI.
    pub(super) fn scroll(&self, at: Point<Pixels>, delta: Point<Pixels>, modifiers: &Modifiers) {
        let (at, delta) = (crate::browser::unzoom(at, self.zoom.get()), crate::browser::unzoom(delta, self.zoom.get()));
        self.cdp.fire("Input.dispatchMouseEvent", json!({
            "type": "mouseWheel", "x": f32::from(at.x), "y": f32::from(at.y),
            "deltaX": f32::from(delta.x), "deltaY": f32::from(delta.y), "button": "none", "modifiers": modifier_bits(modifiers),
        }));
    }
}

const PARK_GAP: i32 = 200;

/// Escala fixa: a WebView2 não segue o monitor, que a página está fora dele.
fn rasterize(view: &wry::WebView, scale: f32) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller3;
    use windows::core::Interface;
    let result = view.controller().cast::<ICoreWebView2Controller3>().and_then(|c| unsafe {
        c.SetShouldDetectMonitorScaleChanges(false)?;
        c.SetRasterizationScale(scale as f64)
    });
    if let Err(e) = result { eprintln!("[pagina] escala da WebView2 nao aplicada: {}", e.message()); }
}
