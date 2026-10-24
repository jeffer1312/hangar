//! Navegador do painel no Linux: um alvo do Chromium sem janela por sessão. A página chega como screencast, cada
//! quadro gravado numa textura do device da GPUI, e a entrada do painel vira `Input.*`.
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    future::Future,
    pin::pin,
    rc::Rc,
    sync::{
        Arc, Condvar, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use base64::Engine as _;
use futures::future::{Either, select};
use gpui_kit::*;
use gpui_wgpu::wgpu;
use serde_json::{Value, json};

use super::{
    launch,
    pipe::{Acker, Browser, CLOSED, Kind, Session},
};
use crate::browser::{Event, Pointer, keys::{key_event, modifier_bits}, model};

/// Último quadro do screencast, já na GPU, com o tamanho em px CSS da página que ele mostra.
struct Shown {
    texture: wgpu::Texture,
    device: wgpu::Device,
    page: Option<(f32, f32)>,
}

/// Tamanho em px CSS da página que um quadro mostra, pelo `metadata` dele (`deviceWidth` × `deviceHeight`). Os pixels
/// do quadro não dizem: a densidade deles é a do processo do Chromium, não a emulada.
fn page_size(metadata: &Value) -> Option<(f32, f32)> {
    let side = |key: &str| metadata[key].as_f64().filter(|v| v.is_finite() && *v > 0.).map(|v| v as f32);
    Some((side("deviceWidth")?, side("deviceHeight")?))
}

/// Um quadro chegado: base64, número para a confirmação e o tamanho da página.
type Arrived = (String, Value, Option<(f32, f32)>);

/// Dividido com a thread de leitura do pipe, que só guarda o quadro, e com a que o decodifica: nada passa pela interface.
struct Surface {
    shown: Mutex<Option<Shown>>,
    slot: Mutex<Slot>,
    wake: Condvar,
    /// A textura foi recriada (primeiro quadro, outro tamanho): um desenho guardado ainda aponta para a antiga.
    replaced: AtomicBool,
    events: async_channel::Sender<Event>,
    /// JPEG no painel, PNG nas páginas da conversa.
    format: image::ImageFormat,
    /// Confirma um quadro ao Chromium. Ele só manda o próximo com poucos sem confirmação, então confirmar depois de
    /// desenhar faz a página produzir quadros no ritmo da tela, e nenhum quando a janela não desenha.
    ack: Acker,
}

#[derive(Default)]
struct Slot {
    /// Último quadro chegado: o que chega antes de ele ser decodificado o substitui.
    data: Option<Arrived>,
    /// Há um `Event::Frame` que a tela ainda não desenhou: o próximo quadro espera, sem decodificar à toa.
    unseen: bool,
    /// Número do quadro decodificado, confirmado quando a tela o desenhar.
    drawn: Option<Value>,
    /// Quadros trocados antes de decodificar, confirmados junto com o próximo desenho: confirmar na hora liberaria o
    /// Chromium a mandar outro, e ele seguiria a todo vapor com a tela parada.
    owed: Vec<Value>,
    closed: bool,
}

impl Slot {
    /// O quadro a decodificar agora: só o mais recente, e só depois de a tela ter desenhado o anterior.
    fn next(&mut self) -> Option<Arrived> { if self.unseen { None } else { self.data.take() } }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> { m.lock().unwrap_or_else(PoisonError::into_inner) }

/// Só para medir: com `HANGAR_NATIVE_NAV_TRACE` no ambiente, uma linha por segundo no stderr com os quadros que
/// chegaram, os decodificados (e o tempo médio deles) e as pinturas da página.
mod trace {
    use std::sync::{OnceLock, atomic::{AtomicU64, Ordering::Relaxed}};
    pub static ARRIVED: AtomicU64 = AtomicU64::new(0);
    pub static DECODED: AtomicU64 = AtomicU64::new(0);
    pub static DECODE_US: AtomicU64 = AtomicU64::new(0);
    pub static PAINTED: AtomicU64 = AtomicU64::new(0);

    pub fn on() -> bool {
        static ON: OnceLock<bool> = OnceLock::new();
        *ON.get_or_init(|| {
            let on = std::env::var_os("HANGAR_NATIVE_NAV_TRACE").is_some();
            if on {
                let _ = std::thread::Builder::new().name("nav-trace".into()).spawn(|| loop {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    let (a, d, us, p) = (ARRIVED.swap(0, Relaxed), DECODED.swap(0, Relaxed), DECODE_US.swap(0, Relaxed), PAINTED.swap(0, Relaxed));
                    eprintln!("nav chegaram={a} decodificados={d} decod_ms={:.1} pinturas={p}", if d > 0 { us as f64 / d as f64 / 1000. } else { 0. });
                });
            }
            on
        })
    }

    pub fn add(counter: &AtomicU64, n: u64) { if on() { counter.fetch_add(n, Relaxed); } }
}

impl Surface {
    fn new(events: async_channel::Sender<Event>, format: image::ImageFormat, ack: Acker) -> Arc<Self> {
        let surface = Arc::new(Surface {
            shown: Mutex::new(None), slot: Mutex::default(), wake: Condvar::new(), replaced: AtomicBool::new(false), events, format, ack,
        });
        let worker = surface.clone();
        if let Err(e) = std::thread::Builder::new().name("chromium-frames".into()).spawn(move || worker.decode_loop()) {
            eprintln!("[nav] sem thread para os quadros: {e}");
        }
        surface
    }

    /// Na thread de leitura do pipe: guarda e acorda quem decodifica.
    fn frame(&self, mut params: Value) {
        let id = params["sessionId"].take();
        let Some(Value::String(data)) = params.get_mut("data").map(Value::take) else { return (self.ack)(id) };
        trace::add(&trace::ARRIVED, 1);
        let page = page_size(&params["metadata"]);
        let mut slot = lock(&self.slot);
        if let Some((_, old, _)) = slot.data.replace((data, id, page)) { slot.owed.push(old); }
        drop(slot);
        self.wake.notify_one();
    }

    fn decode_loop(&self) {
        // Buffers da thread, reaproveitados de um quadro para o outro: um quadro do painel tem megabytes.
        let (mut bytes, mut pixels) = (Vec::new(), Vec::new());
        loop {
            let (data, id, page) = {
                let mut slot = lock(&self.slot);
                loop {
                    if slot.closed { return; }
                    if let Some(next) = slot.next() { break next; }
                    slot = self.wake.wait(slot).unwrap_or_else(PoisonError::into_inner);
                }
            };
            let started = std::time::Instant::now();
            match self.upload(&data, page, &mut bytes, &mut pixels) {
                Ok(()) => {
                    trace::add(&trace::DECODED, 1);
                    trace::add(&trace::DECODE_US, started.elapsed().as_micros() as u64);
                    let mut slot = lock(&self.slot);
                    slot.unseen = true;
                    if let Some(old) = slot.drawn.replace(id) { slot.owed.push(old); }
                    drop(slot);
                    let _ = self.events.try_send(Event::Frame);
                }
                Err(e) => { eprintln!("[nav] quadro do Chromium descartado: {e}"); (self.ack)(id); }
            }
        }
    }

    /// A tela desenhou: confirma o quadro ao Chromium e libera o próximo para decodificar.
    fn seen(&self) {
        let (drawn, owed) = { let mut slot = lock(&self.slot); slot.unseen = false; (slot.drawn.take(), std::mem::take(&mut slot.owed)) };
        if drawn.is_some() { trace::add(&trace::PAINTED, 1); }
        for id in drawn.into_iter().chain(owed) { (self.ack)(id); }
        self.wake.notify_one();
    }

    fn close(&self) {
        lock(&self.slot).closed = true;
        self.wake.notify_one();
    }

    fn upload(&self, data: &str, page: Option<(f32, f32)>, bytes: &mut Vec<u8>, pixels: &mut Vec<u8>) -> Result<(), String> {
        bytes.clear();
        base64::engine::general_purpose::STANDARD.decode_vec(data, bytes).map_err(|e| e.to_string())?;
        let (width, height) = if self.format == image::ImageFormat::Jpeg {
            use zune_jpeg::{JpegDecoder, zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions}};
            let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
            let mut decoder = JpegDecoder::new_with_options(ZCursor::new(&bytes[..]), options);
            decoder.decode_headers().map_err(|e| e.to_string())?;
            let size = decoder.output_buffer_size().ok_or("quadro JPEG sem tamanho")?;
            pixels.resize(size, 0);
            decoder.decode_into(pixels).map_err(|e| e.to_string())?;
            let info = decoder.info().ok_or("quadro JPEG sem cabeçalho")?;
            (u32::from(info.width), u32::from(info.height))
        } else {
            let rgba = image::load_from_memory_with_format(bytes, self.format).map_err(|e| e.to_string())?.into_rgba8();
            let size = rgba.dimensions();
            *pixels = rgba.into_raw();
            size
        };
        let (device, queue) = gpui_wgpu::WgpuContext::shared_device().ok_or("a GPUI não expôs o device wgpu")?;
        let mut shown = lock(&self.shown);
        // Mesma textura enquanto o tamanho e o device não mudam: a fila do wgpu ordena a escrita depois do último desenho.
        let reuse = shown.as_ref().filter(|s| s.device == device && s.texture.width() == width && s.texture.height() == height);
        if reuse.is_none() { self.replaced.store(true, Ordering::SeqCst); }
        let texture = match reuse {
            Some(s) => s.texture.clone(),
            None => device.create_texture(&wgpu::TextureDescriptor {
                label: Some("navegador"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            }),
        };
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            pixels,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * width), rows_per_image: Some(height) },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        *shown = Some(Shown { texture, device, page });
        Ok(())
    }
}

/// Início do erro de uma abertura cuja página em branco caiu antes de ficar pronta.
const BORN_DEAD: &str = "page_crashed_at_start";

/// Início do erro de um `adopt` cujo popup já tinha fechado.
pub const POPUP_GONE: &str = "popup_closed_before_adopted";

/// A janela da GPUI não entra: o Chromium não tem janela. Guarda os executores para os eventos do pipe e os prazos.
pub struct Starter {
    executor: ForegroundExecutor,
    background: BackgroundExecutor,
}

/// Mantém o Chromium compartilhado de pé enquanto existir, para a primeira página não esperar o processo nascer.
pub struct Warm { _browser: Rc<Browser> }

/// Resposta do CDP sem travar a interface, com o prazo das chamadas que bloqueavam.
async fn answer<T>(background: &BackgroundExecutor, method: &str, call: impl Future<Output = Result<T, String>>) -> Result<T, String> {
    let timer = background.timer(Duration::from_secs(10));
    match select(pin!(call), pin!(timer)).await {
        Either::Left((reply, _)) => reply,
        Either::Right(_) => Err(format!("{method}: o Chromium nao respondeu")),
    }
}

/// Documento do agente posto direto na página: HTML, largura da coluna e fundo opaco (tema próprio).
type Document = (Rc<str>, f32, Option<(u8, u8, u8)>);

type Publish = Rc<dyn Fn(&dyn Fn(&mut model::PageState))>;

type Popups = Rc<RefCell<Vec<String>>>;

pub struct Engine {
    session: Rc<Session>,
    /// Edita o estado da barra e o manda ao painel se mudou.
    publish: Publish,
    /// Último estado publicado: o painel o relê ao voltar a esta página depois de um popup.
    state: Rc<RefCell<model::PageState>>,
    target: String,
    window: i64,
    /// Altura da barra que o Chrome completo sem janela ainda desconta da janela (o shell não tem).
    decoration: f32,
    executor: ForegroundExecutor,
    background: BackgroundExecutor,
    surface: Arc<Surface>,
    placed: Cell<Option<(Size<Pixels>, f32)>>,
    /// Conta os tamanhos pedidos durante um arrasto: só o último, parado por um instante, chega ao Chromium.
    resizes: Rc<Cell<u64>>,
    visible: Rc<Cell<bool>>,
    pressed: Cell<bool>,
    /// A página caiu ou se soltou com o Chromium vivo.
    dead: Rc<Cell<bool>>,
    /// Página e site da conversa: a escala do processo, em que o quadro sai; o painel segue a da janela.
    page_scale: Option<f32>,
    /// Zoom da tela cheia: a página vê `1/zoom` do espaço em densidade `zoom` vezes maior; o quadro não muda de tamanho.
    zoom: Cell<f32>,
    /// Popups anunciados por esta página e ainda vivos: fecham junto com ela, mesmo se ninguém os adotou.
    announced: Popups,
    /// Contexto próprio da página da conversa, fechado junto com ela.
    context: Option<String>,
}

impl Engine {
    pub fn available() -> Result<(), String> { launch::find().map(|_| ()) }

    pub fn prepare(_window: &Window, cx: &App) -> Result<Starter, String> {
        Ok(Starter { executor: cx.foreground_executor().clone(), background: cx.background_executor().clone() })
    }

    /// Sobe o Chromium do painel agora, sem esperar ele responder.
    pub fn warm(_window: &Window, cx: &App) -> Result<Warm, String> { Self::warm_kind(cx, Kind::Panel) }

    /// Sobe o Chromium das páginas da conversa agora, sem esperar ele responder.
    pub fn warm_pages(_window: &Window, cx: &App) -> Result<Warm, String> { Self::warm_kind(cx, Kind::Pages) }

    fn warm_kind(cx: &App, kind: Kind) -> Result<Warm, String> {
        Browser::shared_async(cx.foreground_executor(), cx.background_executor(), kind).map(|browser| Warm { _browser: browser })
    }
}

impl Starter {
    /// O navegador do painel: sobe o Chromium (se ainda não há um) e abre o alvo desta sessão sem travar a interface.
    pub fn start(self, events: async_channel::Sender<Event>) -> impl Future<Output = Result<Engine, String>> + use<> {
        async move { self.fresh(Kind::Panel, async |browser| self.open(browser, None, None, events.clone(), true).await).await }
    }

    /// Site de verdade na conversa: o mesmo perfil do painel (cookies e login), navegação livre dentro do site e popup
    /// no próprio alvo, como o painel. Nasce sem travar a interface.
    pub fn start_url(self, url: String, events: async_channel::Sender<Event>) -> impl Future<Output = Result<Engine, String>> + use<> {
        async move {
            let engine = self.fresh(Kind::Panel, async |browser| self.open(browser, None, None, events.clone(), false).await).await?;
            engine.load(&url);
            Ok(engine)
        }
    }

    /// Página da conversa: alvo num contexto próprio (sem os cookies do painel) e documento posto direto, sem travar a
    /// interface. Sem `background` o fundo é transparente; com ele, opaco nessa cor.
    pub fn start_page(self, html: Rc<str>, width: f32, background: Option<(u8, u8, u8)>, events: async_channel::Sender<Event>)
        -> impl Future<Output = Result<Engine, String>> + use<> {
        async move {
            self.fresh(Kind::Pages, async |browser| {
                let created = self.browser_call(browser, "Target.createBrowserContext", json!({"disposeOnDetach": true})).await?;
                let context = created["browserContextId"].as_str().ok_or("createBrowserContext sem id")?.to_owned();
                let opened = self.open(browser, Some(context.clone()), Some((html.clone(), width, background)), events.clone(), false).await;
                // Falhou no meio: fechar o contexto fecha junto o alvo que já tenha nascido nele.
                if opened.is_err() { drop(browser.call(None, "Target.disposeBrowserContext", json!({"browserContextId": context}))); }
                opened
            }).await
        }
    }

    /// Abre no Chromium compartilhado e, se ele morreu no meio ou não consegue mais criar página (o zygote caiu e toda
    /// página nova cai ao nascer), descarta esse processo e tenta uma vez num novo. Aberturas simultâneas esperam o mesmo
    /// processo novo: o `shared_async` devolve o que já está subindo.
    async fn fresh(&self, kind: Kind, open: impl AsyncFn(&Rc<Browser>) -> Result<Engine, String>) -> Result<Engine, String> {
        let browser = self.browser(kind).await?;
        match open(&browser).await {
            Err(e) if !browser.alive() || e.starts_with(BORN_DEAD) => {
                browser.retire(&e);
                open(&self.browser(kind).await?).await
            }
            opened => opened,
        }
    }

    async fn browser(&self, kind: Kind) -> Result<Rc<Browser>, String> {
        let browser = Browser::shared_async(&self.executor, &self.background, kind)?;
        browser.ready().await?;
        Ok(browser)
    }

    async fn browser_call(&self, browser: &Browser, method: &str, params: Value) -> Result<Value, String> {
        answer(&self.background, method, browser.call(None, method, params)).await
    }

    async fn page_call(&self, session: &Session, method: &str, params: Value) -> Result<Value, String> {
        answer(&self.background, method, session.call(method, params)).await
    }

    /// Alvo do painel (`panel`: JPEG na escala da tela) ou da conversa (PNG no dobro). Falhou depois de nascer: o alvo
    /// fecha aqui, que ninguém mais o conhece.
    async fn open(&self, browser: &Rc<Browser>, context: Option<String>, document: Option<Document>, events: async_channel::Sender<Event>,
        panel: bool) -> Result<Engine, String> {
        let mut create = json!({"url": "about:blank", "newWindow": true});
        if let Some(context) = &context { create["browserContextId"] = context.as_str().into(); }
        let created = self.browser_call(browser, "Target.createTarget", create).await?;
        let target = created["targetId"].as_str().ok_or("createTarget sem targetId")?.to_owned();
        browser.own(&target, true);
        browser.close_initial();
        let opened = self.open_target(browser, &target, context, document, events, panel, panel).await;
        if opened.is_err() {
            browser.own(&target, false);
            drop(browser.call(None, "Target.closeTarget", json!({"targetId": target})));
        }
        opened
    }

    /// `popups`: popup de verdade que a página abrir vira `Event::Popup`, em vez de abrir nela mesma.
    #[allow(clippy::too_many_arguments)]
    async fn open_target(
        &self, browser: &Rc<Browser>, target: &str, context: Option<String>, document: Option<Document>, events: async_channel::Sender<Event>,
        panel: bool, popups: bool,
    ) -> Result<Engine, String> {
        let session = Rc::new(answer(&self.background, "Target.attachToTarget", Session::attach_async(browser, target)).await?);
        // Caiu antes de haver ouvinte (renderer que nem subiu): sem isto o `Page.enable` espera o prazo inteiro.
        if browser.crashed(target) { return Err(format!("{BORN_DEAD}: a pagina caiu ao nascer")); }
        // Ouvintes antes de qualquer espera: a interface segue rodando, e o que a página mandar enquanto nasce (a altura,
        // um pedido pausado) não pode chegar sem dono.
        let state = Rc::new(RefCell::new(model::PageState::default()));
        let surface = Surface::new(events.clone(), image::ImageFormat::Png, browser.acker(session.id()));
        let sink = surface.clone();
        browser.sink(session.id(), Box::new(move |params| sink.frame(params)));
        let dead = Rc::new(Cell::new(false));
        let announced = Popups::default();
        let publish = listen(&session, target, &state, &events, &self.executor, &dead, document.is_some(), popups.then_some(&announced));
        let host = events.clone();
        let _ = session.on("Runtime.bindingCalled", move |params| {
            if params["name"] == "hangarHost" { let _ = host.try_send(Event::Host(params["payload"].as_str().unwrap_or("").to_owned())); }
        });
        // Falhou aqui: quem fecha o alvo é o `open`, o único que chama.
        match self.setup_target(browser, &session, target, &document).await {
            // Página em branco que cai antes de nascer não é a página: é o Chromium que não consegue mais criá-las.
            Err(e) if dead.get() => { surface.close(); Err(format!("{BORN_DEAD}: a pagina caiu ao nascer ({e})")) }
            _ if dead.get() => { surface.close(); Err(format!("{BORN_DEAD}: a pagina caiu ao nascer")) }
            Ok((window, decoration)) => Ok(Engine {
                session, publish, state, announced, target: target.to_owned(), window, decoration, executor: self.executor.clone(), background: self.background.clone(),
                surface, dead, page_scale: (!panel).then(|| browser.scale() as f32), context, placed: Cell::new(None), resizes: Rc::default(), visible: Rc::default(),
                pressed: Cell::new(false), zoom: Cell::new(1.),
            }),
            Err(e) => { surface.close(); Err(e) }
        }
    }

    /// Janela, domínios e documento do alvo; devolve a janela e o desconto da barra.
    async fn setup_target(&self, browser: &Rc<Browser>, session: &Session, target: &str, document: &Option<Document>) -> Result<(i64, f32), String> {
        let window = self.browser_call(browser, "Browser.getWindowForTarget", json!({"targetId": target})).await?["windowId"]
            .as_i64().ok_or("getWindowForTarget sem windowId")?;
        self.page_call(session, "Page.enable", json!({})).await?;
        // Queda do renderer chega só por aqui (`Inspector.targetCrashed`, na sessão da página).
        self.page_call(session, "Inspector.enable", json!({})).await?;
        if let Some((_, _, background)) = &document {
            self.page_call(session, "Runtime.enable", json!({})).await?;
            self.page_call(session, "Runtime.addBinding", json!({"name": "hangarHost"})).await?;
            let (r, g, b, a) = background.map_or((0, 0, 0, 0), |(r, g, b)| (r, g, b, 1));
            self.page_call(session, "Emulation.setDefaultBackgroundColorOverride", json!({"color": {"r": r, "g": g, "b": b, "a": a}})).await?;
        }
        // Várias páginas abertas não disputam o foco: `focus`/`release_focus` contam com isto.
        self.page_call(session, "Emulation.setFocusEmulationEnabled", json!({"enabled": true})).await?;
        // Mesma regra de endereço do painel: só web (http/https sem usuário e senha); o resto não carrega.
        self.page_call(session, "Fetch.enable", json!({"patterns": [{"resourceType": "Document", "requestStage": "Request"}]})).await?;
        let (width, height) = document.as_ref().map_or((1280, 800), |(_, width, _)| (width.round() as i64, 600));
        self.browser_call(browser, "Browser.setWindowBounds", json!({"windowId": window, "bounds": {"width": width, "height": height}})).await?;
        let measured = self.page_call(session, "Runtime.evaluate", json!({"expression": "outerHeight-innerHeight", "returnByValue": true})).await?;
        let decoration = measured["result"]["value"].as_f64().unwrap_or(0.).max(0.) as f32;
        if let Some((html, _, _)) = &document {
            let tree = self.page_call(session, "Page.getFrameTree", json!({})).await?;
            let frame = tree["frameTree"]["frame"]["id"].as_str().unwrap_or_default().to_owned();
            self.page_call(session, "Page.setDocumentContent", json!({"frameId": frame, "html": &**html})).await?;
        }
        Ok((window, decoration))
    }
}

/// `window.open` pediu janela, não aba: a regra do HTML ("popup window is requested"), que o Chromium já aplicou ao
/// listar no `Page.windowOpen` só as barras visíveis. Aba traz as cinco; `width=…`, `popup` e afins tiram alguma.
fn is_popup_window(features: &Value) -> bool {
    let shown: Vec<&str> = features.as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    ["menubar", "toolbar", "status", "scrollbars", "resizable"].iter().any(|bar| !shown.contains(bar))
}

/// Estado da barra (endereço, título, carregando, voltar/avançar), diálogos e janelas novas. `block_documents`:
/// o frame principal não navega para documento nenhum (a página da conversa já nasce com o seu).
#[allow(clippy::too_many_arguments)]
fn listen(
    session: &Rc<Session>, target: &str, state: &Rc<RefCell<model::PageState>>, events: &async_channel::Sender<Event>, executor: &ForegroundExecutor,
    dead: &Rc<Cell<bool>>, block_documents: bool, popups: Option<&Popups>,
) -> Publish {
    let publish: Publish = {
        let (state, events) = (state.clone(), events.clone());
        Rc::new(move |edit: &dyn Fn(&mut model::PageState)| {
            let mut page = state.borrow_mut();
            let before = page.clone();
            edit(&mut page);
            if *page != before { let _ = events.try_send(Event::State(page.clone())); }
        })
    };
    let history = {
        let (session, publish, executor) = (Rc::downgrade(session), publish.clone(), executor.clone());
        Rc::new(move || {
            let Some(session) = session.upgrade() else { return };
            let (call, publish) = (session.call("Page.getNavigationHistory", json!({})), publish.clone());
            executor.spawn(async move {
                let Ok(h) = call.await else { return };
                let (index, count) = (h["currentIndex"].as_i64().unwrap_or(0), h["entries"].as_array().map_or(0, Vec::len) as i64);
                publish(&|page| { page.can_back = index > 0; page.can_forward = index + 1 < count; });
            }).detach();
        })
    };
    let main = target.to_owned();
    let (p, h) = (publish.clone(), history.clone());
    let _ = session.on("Page.frameNavigated", move |params| {
        let frame = &params["frame"];
        if !frame["parentId"].is_null() { return; }
        // Página de erro do Chromium: o endereço é o que não abriu (`unreachableUrl`), e o erro da carga fica.
        let failed = frame["unreachableUrl"].as_str().filter(|u| !u.is_empty());
        let url = failed.unwrap_or(frame["url"].as_str().unwrap_or("")).to_owned();
        let keep = failed.is_some();
        // A página de erro de uma navegação bloqueada traz o aviso do bloqueio: ele fica com ela.
        p(&|page| { page.url = Some(url.clone()); page.dialog = None; if !keep { page.error = None; page.notice = None; } });
        h();
    });
    let (p, h, m) = (publish.clone(), history.clone(), main.clone());
    let _ = session.on("Page.navigatedWithinDocument", move |params| {
        if params["frameId"] != m.as_str() { return; }
        let url = params["url"].as_str().unwrap_or("").to_owned();
        p(&|page| page.url = Some(url.clone()));
        h();
    });
    for (event, loading) in [("Page.frameStartedLoading", true), ("Page.frameStoppedLoading", false)] {
        let (p, m) = (publish.clone(), main.clone());
        let _ = session.on(event, move |params| if params["frameId"] == m.as_str() { p(&|page| page.loading = loading) });
    }
    let (weak, p, m) = (Rc::downgrade(session), publish.clone(), main.clone());
    let _ = session.on("Fetch.requestPaused", move |params| {
        let Some(session) = weak.upgrade() else { return };
        let (id, url) = (params["requestId"].clone(), params["request"]["url"].as_str().unwrap_or("").to_owned());
        // `Aborted` deixa a página onde está; `BlockedByClient` a trocaria pela tela de erro do Chromium.
        if block_documents && params["frameId"] == m.as_str() {
            return drop(session.call("Fetch.failRequest", json!({"requestId": id, "errorReason": "Aborted"})));
        }
        if model::allowed_request(&url) { return drop(session.call("Fetch.continueRequest", json!({"requestId": id}))); }
        drop(session.call("Fetch.failRequest", json!({"requestId": id, "errorReason": "BlockedByClient"})));
        // Bloqueio é aviso, não tela de falha: tentar de novo bloquearia de novo.
        if params["frameId"] == m.as_str() {
            let text = crate::i18n::tr("browser_blocked").replace("{url}", &url);
            p(&|page| { page.notice = Some(text.clone()); page.notice_seq += 1; });
        }
    });
    // O diálogo fica aberto até alguém responder: o agente (verbo `dialog`) ou a pessoa (caixa do painel). Testar
    // um "Excluir? OK/Cancelar" exige escolher. Só o `beforeunload` passa sozinho: sem isso a navegação travaria.
    let (weak, p) = (Rc::downgrade(session), publish.clone());
    let _ = session.on("Page.javascriptDialogOpening", move |params| {
        let kind = params["type"].as_str().unwrap_or("").to_owned();
        if kind == "beforeunload" {
            if let Some(session) = weak.upgrade() { drop(session.call("Page.handleJavaScriptDialog", json!({"accept": true}))); }
            return;
        }
        let dialog = model::Dialog { kind, message: params["message"].as_str().unwrap_or("").chars().take(500).collect(),
            default: params["defaultPrompt"].as_str().unwrap_or("").to_owned() };
        p(&|page| page.dialog = Some(dialog.clone()));
    });
    let p = publish.clone();
    let _ = session.on("Page.javascriptDialogClosed", move |_| p(&|page| page.dialog = None));
    // O download é negado no navegador inteiro (`Browser.setDownloadBehavior`); sem aviso o clique parecia não fazer nada.
    let p = publish.clone();
    let _ = session.on("Page.downloadWillBegin", move |params| {
        let file = params["suggestedFilename"].as_str().filter(|f| !f.is_empty()).or(params["url"].as_str()).unwrap_or("").to_owned();
        let text = crate::i18n::tr("browser_download_denied").replace("{file}", &file);
        p(&|page| { page.notice = Some(text.clone()); page.notice_seq += 1; });
    });
    let (p, m) = (publish.clone(), main.clone());
    session.watch("Target.targetInfoChanged", move |params| {
        let info = &params["targetInfo"];
        if info["targetId"] == m.as_str() {
            let title = info["title"].as_str().unwrap_or("").to_owned();
            // Sem título o Chromium devolve o endereço; a barra mostra o nome do app nesse caso.
            let title = if info["url"].as_str() == Some(title.as_str()) { String::new() } else { title };
            p(&|page| page.title = title.clone());
        }
    });
    // Popup de verdade (`popups`): o `window.open` pediu janela e a página guarda a referência (`canAccessOpener`). É o
    // login OAuth, que só termina com o `postMessage` ao `window.opener`. O `Page.windowOpen` chega antes do alvo nascer.
    let popup_asked = Rc::new(Cell::new(false));
    if popups.is_some() {
        let asked = popup_asked.clone();
        let _ = session.on("Page.windowOpen", move |params| asked.set(is_popup_window(&params["windowFeatures"])));
    }
    // O nativo não tem abas: link com `target=_blank` e `window.open` sem janela abrem nesta mesma página. Página que
    // nenhum navegador do app abriu (popup `noopener`, sem dono) fecha: sem painel, ninguém veria o JS dela rodando.
    let (weak, m, closing) = (Rc::downgrade(session), main.clone(), Rc::new(RefCell::new(HashSet::<String>::new())));
    for event in ["Target.targetCreated", "Target.targetInfoChanged"] {
        let (weak, m, closing, asked, events, announced) = (weak.clone(), m.clone(), closing.clone(), popup_asked.clone(), events.clone(), popups.cloned());
        session.watch(event, move |params| {
            let info = &params["targetInfo"];
            let Some(id) = info["targetId"].as_str() else { return };
            let Some(session) = weak.upgrade() else { return };
            let browser = session.browser();
            let (opener, url) = (info["openerId"].as_str().unwrap_or(""), info["url"].as_str().unwrap_or(""));
            // Sem opener só nasce do `createTarget` de outra sessão do app, que o anuncia antes de responder e de marcá-lo
            // como dela: fechá-lo derrubava a abertura dela e o Chromium inteiro. Popup, mesmo `noopener`, traz o opener.
            if info["type"] != "page" || opener.is_empty() || browser.owns(id) { return; }
            let mine = opener == m;
            // Só o nascimento conta: o pedido é do último `window.open`, e um alvo antigo ainda sem endereço não o leva.
            if mine && event == "Target.targetCreated" && asked.take() && info["canAccessOpener"] == true {
                browser.own(id, true);
                if let Some(announced) = &announced { announced.borrow_mut().push(id.to_owned()); }
                let _ = events.try_send(Event::Popup(id.to_owned()));
                return;
            }
            // Aberta por esta página: espera o endereço chegar. Por outra do app: o navegador dela cuida.
            if (mine && (url.is_empty() || url == "about:blank")) || (!mine && browser.owns(opener)) { return; }
            if !closing.borrow_mut().insert(id.to_owned()) { return; }
            if mine { drop(session.call("Page.navigate", json!({"url": url}))); }
            drop(browser.call(None, "Target.closeTarget", json!({"targetId": id})));
        });
    }
    let (closed, m, announced) = (events.clone(), main.clone(), popups.cloned());
    session.watch("Target.targetDestroyed", move |params| {
        let Some(id) = params["targetId"].as_str() else { return };
        closing.borrow_mut().remove(id);
        if let Some(announced) = &announced { announced.borrow_mut().retain(|p| p != id); }
        if id == m { let _ = closed.try_send(Event::Closed); }
    });
    let p = publish.clone();
    session.watch(CLOSED, move |_| p(&|page| { page.loading = false; page.dialog = None; page.error = Some(crate::i18n::tr("browser_err_closed")); }));
    // A página caiu com o Chromium vivo: sem isto o painel congela no último quadro, calado.
    let fell: Rc<dyn Fn(&str, &Value)> = {
        let (weak, p, dead) = (Rc::downgrade(session), publish.clone(), dead.clone());
        Rc::new(move |event, params| {
            if dead.replace(true) { return; }
            eprintln!("[nav] a pagina caiu ({event} {params})");
            if let Some(session) = weak.upgrade() { session.fail_pending(); }
            p(&|page| { page.loading = false; page.dialog = None; page.error = Some(crate::i18n::tr("browser_err_crashed")); });
        })
    };
    // Renderer morto por fora (sem zygote) só manda `Inspector.detached`, e o `Page.enable` pendente nunca responde.
    for event in ["Inspector.targetCrashed", "Inspector.detached"] {
        let f = fell.clone();
        let _ = session.on(event, move |params| f(event, &params));
    }
    for (event, key, id) in [("Target.targetCrashed", "targetId", main.clone()), ("Target.detachedFromTarget", "sessionId", session.id().to_owned())] {
        let f = fell.clone();
        session.watch(event, move |params| { if params[key] == id.as_str() { f(event, &params); } });
    }
    publish
}

impl Engine {
    pub fn alive(&self) -> bool { self.session.browser().alive() && !self.dead.get() }

    pub fn page(&self) -> model::PageState { self.state.borrow().clone() }

    /// Motor próprio no popup anunciado por `Event::Popup`, com o `window.opener` dele intacto. Popup aberto por ele
    /// abre nele mesmo; falhou, o alvo fecha aqui.
    pub fn adopt(&self, target: String, events: async_channel::Sender<Event>) -> impl Future<Output = Result<Engine, String>> + use<> {
        let starter = Starter { executor: self.executor.clone(), background: self.background.clone() };
        let browser = self.session.browser().clone();
        async move {
            let opened = starter.open_target(&browser, &target, None, None, events, true, false).await;
            let info = browser.call(None, "Target.getTargetInfo", json!({"targetId": target})).await;
            match (opened, info) {
                // O popup navegou antes de haver sessão nele: o endereço e o título vêm do alvo.
                (Ok(engine), Ok(info)) => {
                    let info = &info["targetInfo"];
                    let url = info["url"].as_str().unwrap_or("").to_owned();
                    let title = info["title"].as_str().filter(|t| *t != url).unwrap_or("").to_owned();
                    (engine.publish)(&|page| { page.url = Some(url.clone()); page.title = title.clone(); });
                    Ok(engine)
                }
                (Ok(engine), Err(e)) => {
                    eprintln!("[nav] popup sem endereco na adocao: {e}");
                    Ok(engine)
                }
                (Err(e), info) => {
                    // Login que se resolve sozinho fecha o popup antes de ele ser adotado: não é falha.
                    if info.is_err_and(|i| i.contains("No target with given id")) { return Err(format!("{POPUP_GONE}: {e}")); }
                    drop(browser.call(None, "Target.closeTarget", json!({"targetId": target})));
                    Err(e)
                }
            }
        }
    }

    /// A sessão do painel, para o controlador do hangar-preview.
    pub fn cdp(&self) -> Rc<Session> { self.session.clone() }

    /// O pipe e a página visível desta sessão, para o cliente CDP de fora (`/cdp/browser`).
    pub fn port(&self) -> (super::pipe::Port, String) { (self.session.browser().port(), self.target.clone()) }

    /// Sessão própria para a tela remota do celular: o screencast dela tem o tamanho do aparelho, sem mexer no do painel.
    pub fn viewer_session(&self) -> impl Future<Output = Result<Rc<Session>, String>> + use<> {
        let (attach, background) = (Session::attach_async(self.session.browser(), &self.target), self.background.clone());
        async move {
            let session = answer(&background, "Target.attachToTarget", attach).await?;
            answer(&background, "Page.enable", session.call("Page.enable", json!({}))).await?;
            Ok(Rc::new(session))
        }
    }

    fn send(&self, method: &str, params: Value) { drop(self.session.call(method, params)); }

    /// Roda um script na página sem esperar o resultado.
    pub fn evaluate(&self, expression: &str) { self.send("Runtime.evaluate", json!({"expression": expression})); }

    pub fn load(&self, url: &str) {
        let (call, publish, target) = (self.session.call("Page.navigate", json!({"url": url})), self.publish.clone(), url.to_owned());
        self.executor.spawn(async move {
            // Falha de rede: o Chromium mostra a página de erro dele, e a barra o motivo.
            // ABORTED = a página não saiu do lugar (download, 204, navegação trocada por outra); BLOCKED_BY_CLIENT já
            // virou aviso no `Fetch.requestPaused`. Nenhum dos dois traz página de erro que limpe o erro depois.
            let error = match call.await {
                Ok(r) => r["errorText"].as_str().filter(|e| !e.is_empty() && !e.ends_with("ERR_ABORTED") && !e.ends_with("ERR_BLOCKED_BY_CLIENT"))
                    .map(|e| crate::browser::model::load_error(e, &target)),
                Err(e) => Some(crate::browser::model::load_error(&e, &target)),
            };
            if let Some(error) = error { publish(&|page| { page.loading = false; page.error = Some(error.clone()); }); }
        }).detach();
    }

    pub fn back(&self) { self.history_step(-1); }
    pub fn forward(&self) { self.history_step(1); }
    pub fn reload(&self) { self.send("Page.reload", json!({})); }
    pub fn stop(&self) { self.send("Page.stopLoading", json!({})); }

    /// Responde o diálogo aberto; o `javascriptDialogClosed` tira a caixa do painel.
    pub fn answer_dialog(&self, accept: bool, text: Option<&str>) -> impl std::future::Future<Output = Result<(), String>> + 'static + use<> {
        let mut params = json!({"accept": accept});
        if let Some(text) = text { params["promptText"] = text.into(); }
        let call = self.session.call("Page.handleJavaScriptDialog", params);
        async move { call.await.map(|_| ()) }
    }

    fn history_step(&self, step: i64) {
        let (call, session) = (self.session.call("Page.getNavigationHistory", json!({})), Rc::downgrade(&self.session));
        self.executor.spawn(async move {
            let Ok(h) = call.await else { return };
            let index = h["currentIndex"].as_i64().unwrap_or(0) + step;
            let Some(entry) = usize::try_from(index).ok().and_then(|i| h["entries"].get(i)) else { return };
            if let Some(session) = session.upgrade() { drop(session.call("Page.navigateToHistoryEntry", json!({"entryId": entry["id"]}))); }
        }).detach();
    }

    fn fit(&self, size: Size<Pixels>, scale: f32) -> Fit {
        // Densidade emulada acima da do processo não chega ao quadro: a página só rasterizaria pixels jogados fora.
        let frame_scale = self.page_scale.unwrap_or(scale);
        Fit { session: self.session.clone(), window: self.window, decoration: self.decoration,
            w: f32::from(size.width), h: f32::from(size.height), frame_scale, zoom: self.zoom.get() }
    }

    /// Zoom da página em tela cheia. Aplicado na hora, sem a espera do arrasto; quando a série de cliques assenta, o
    /// screencast reinicia para mandar o quadro do zoom final.
    pub fn set_zoom(&self, zoom: f32) {
        let zoom = crate::browser::valid_zoom(zoom);
        if self.zoom.replace(zoom) == zoom { return; }
        // Avança a contagem: um ajuste de arrasto ainda esperando levaria o zoom antigo e desfaria este.
        let turn = self.resizes.get() + 1;
        self.resizes.set(turn);
        // Escondida, o próximo `place` já aplica o zoom novo; aqui o screencast voltaria sem ninguém olhando.
        if !self.visible.get() { return; }
        let Some((size, scale)) = self.placed.get() else { return };
        let fit = self.fit(size, scale);
        fit.apply(true);
        self.settle(turn, fit, false);
    }

    /// Página parada não manda quadro sozinha, e o que o screencast manda ao ser pedido sai antes do relayout, no
    /// tamanho ou zoom anterior; os que vêm sem confirmação no meio de uma série são descartados. Depois de assentar, se
    /// nada mudou e a página segue à vista: aplica o tamanho (`apply`, o do arrasto) e reinicia o screencast.
    fn settle(&self, turn: u64, fit: Fit, apply: bool) {
        let (resizes, visible, background) = (self.resizes.clone(), self.visible.clone(), self.background.clone());
        let current = move || resizes.get() == turn && visible.get();
        let wait = move || background.timer(Duration::from_millis(RESIZE_SETTLE_MS));
        self.executor.spawn(async move {
            if apply {
                wait().await;
                if !current() { return; }
                fit.apply(true);
            }
            wait().await;
            if current() { fit.restart(); }
        }).detach();
    }

    /// Chamado na pintura da página, a cada quadro visível.
    pub fn place(&self, bounds: Bounds<Pixels>, window: &mut Window) {
        let scale = window.scale_factor();
        let resized = self.placed.get() != Some((bounds.size, scale));
        let first = !self.visible.replace(true);
        if resized { self.placed.set(Some((bounds.size, scale))); }
        if first || resized {
            let fit = self.fit(bounds.size, scale);
            let turn = self.resizes.get() + 1;
            self.resizes.set(turn);
            // O tamanho aplicado antes de esconder pode ter ficado para trás (ajuste pendente descartado). Durante um
            // arrasto o Chromium só recebe o tamanho em que ele parou.
            if first { fit.apply(true); }
            self.settle(turn, fit, !first);
        }
        // Antes de ler: um quadro que chegue no meio ainda avisa a tela.
        self.surface.seen();
        let device = gpui_wgpu::WgpuContext::shared_device().map(|(device, _)| device);
        let shown = {
            let mut latest = lock(&self.surface.shown);
            if latest.as_ref().is_some_and(|s| Some(&s.device) != device.as_ref()) { *latest = None; }
            latest.as_ref().map(|s| (s.texture.clone(), s.page))
        };
        let Some((texture, page)) = shown else { return };
        // Tela emulada pelo `layout` do agente: o painel já encaixou a moldura na proporção dela, e o quadro estica até
        // ela. Fora isso o quadro fica no tamanho da página que ele mostra (vezes o zoom da tela cheia), que nem sempre é
        // o da moldura (resize em curso).
        let zoom = self.zoom.get();
        let frame = page.filter(|_| !self.session.viewport.held.get()).map(|(w, h)| (w * zoom, h * zoom))
            .unwrap_or((f32::from(bounds.size.width), f32::from(bounds.size.height)));
        window.paint_surface(crate::browser::frame_rect(bounds, frame, scale), Arc::new(texture));
    }

    /// A tela desenhou o último quadro: o próximo já pode ser decodificado.
    pub fn frame_seen(&self) { self.surface.seen(); }

    /// A textura mudou desde a última pergunta: um desenho guardado ainda mostraria a antiga.
    pub fn texture_replaced(&self) -> bool { self.surface.replaced.swap(false, Ordering::SeqCst) }

    /// Página fora da tela: sem screencast e sem a textura na GPU. O controlador cuida do tamanho dela para o `shot`.
    pub fn hide(&self) {
        if self.visible.replace(false) { self.send("Page.stopScreencast", json!({})); }
        *lock(&self.surface.shown) = None;
    }

    /// `at` relativo à origem da página, em px lógicos. Botão esquerdo, com os modificadores do clique.
    pub fn pointer(&self, kind: Pointer, at: Point<Pixels>, clicks: usize, modifiers: &Modifiers) { self.mouse(kind, MouseButton::Left, at, clicks, modifiers) }

    /// Qualquer botão, com os modificadores. O `clickCount` da soltura repete o do aperto: sem ele o Chromium não
    /// dispara `dblclick`. Arrasto só se acompanha com o esquerdo.
    pub fn mouse(&self, kind: Pointer, button: MouseButton, at: Point<Pixels>, clicks: usize, modifiers: &Modifiers) {
        let at = crate::browser::unzoom(at, self.zoom.get());
        let (x, y) = (f32::from(at.x) as f64, f32::from(at.y) as f64);
        let (name, bit) = match button { MouseButton::Right => ("right", 2), MouseButton::Middle => ("middle", 4), _ => ("left", 1) };
        let left = name == "left";
        let (kind, button, buttons, clicks) = match kind {
            Pointer::Down => { if left { self.pressed.set(true) } ("mousePressed", name, bit, clicks.max(1)) }
            Pointer::Up => { if left { self.pressed.set(false) } ("mouseReleased", name, 0, clicks.max(1)) }
            Pointer::Move if self.pressed.get() => ("mouseMoved", "left", 1, 0),
            Pointer::Move => ("mouseMoved", "none", 0, 0),
        };
        self.send("Input.dispatchMouseEvent", json!({"type": kind, "x": x, "y": y, "button": button, "buttons": buttons,
            "clickCount": clicks, "modifiers": modifier_bits(modifiers)}));
    }

    /// `delta` em px lógicos; `delta.y > 0` avança a página. Quem chama já inverteu o sinal da GPUI.
    pub fn wheel(&self, at: Point<Pixels>, delta: Point<Pixels>) { self.scroll(at, delta, &Modifiers::default()) }

    /// Com modificadores: Ctrl+roda é zoom da página e Shift+roda rola de lado.
    pub fn scroll(&self, at: Point<Pixels>, delta: Point<Pixels>, modifiers: &Modifiers) {
        let (at, delta) = (crate::browser::unzoom(at, self.zoom.get()), crate::browser::unzoom(delta, self.zoom.get()));
        self.send("Input.dispatchMouseEvent", json!({
            "type": "mouseWheel", "x": f32::from(at.x), "y": f32::from(at.y),
            "deltaX": f32::from(delta.x), "deltaY": f32::from(delta.y), "button": "none", "modifiers": modifier_bits(modifiers),
        }));
    }

    pub fn key(&self, down: bool, keystroke: &Keystroke) {
        if let Some(event) = key_event(down, keystroke) { self.send("Input.dispatchKeyEvent", event); }
    }

    pub fn insert_text(&self, text: &str) { self.send("Input.insertText", json!({"text": text})); }

    /// A página finge estar sempre focada (`setFocusEmulationEnabled`): o foco do painel não precisa ir a ela.
    pub fn focus(&self, _focused: bool) {}
    pub fn release_focus(&self) {}
}

const RESIZE_SETTLE_MS: u64 = 120;

/// Viewport CSS e densidade com zoom: o quadro continua com os mesmos pixels, a página é que vê menos (ou mais) espaço.
fn zoomed_metrics(w: f32, h: f32, frame_scale: f32, zoom: f32) -> (u32, u32, f32) {
    let zoom = crate::browser::valid_zoom(zoom);
    ((w / zoom).round() as u32, (h / zoom).round() as u32, frame_scale * zoom)
}

/// Tamanho e escala da página no painel, e como pedir ao Chromium.
struct Fit { session: Rc<Session>, window: i64, decoration: f32, w: f32, h: f32, frame_scale: f32, zoom: f32 }

impl Fit {
    fn apply(&self, resized: bool) {
        let Fit { session, window, decoration, w, h, frame_scale, zoom } = self;
        if resized {
            let size = json!({"width": w.round() as i64, "height": (h + decoration).round() as i64});
            drop(session.browser().call(None, "Browser.setWindowBounds", json!({"windowId": window, "bounds": size})));
            // Só a janela nova não muda a página no Chromium sem janela, e depois de redimensionar o quadro chegava no
            // tamanho antigo, esticado; em tela 1,25 sem a densidade dela o quadro também era esticado. Com layout pedido
            // pelo agente o tamanho é dele: o do painel só fica guardado, para o `layout desktop` voltar ao atual.
            let viewport = &session.viewport;
            let (width, height, density) = zoomed_metrics(*w, *h, *frame_scale, *zoom);
            viewport.panel.set(Some((width, height, density)));
            if !viewport.held.get() {
                drop(session.call("Emulation.setDeviceMetricsOverride", json!({
                    "width": width, "height": height, "deviceScaleFactor": density, "mobile": false,
                })));
            }
        }
        let (pw, ph) = ((w * frame_scale).round() as i64, (h * frame_scale).round() as i64);
        // PNG: o painel tem que mostrar a página como o navegador mostra; JPEG borrava o texto.
        let params = json!({"format": "png", "maxWidth": pw.max(1), "maxHeight": ph.max(1), "everyNthFrame": 1});
        drop(session.call("Page.startScreencast", params));
    }

    /// Para e reabre o screencast com os mesmos parâmetros: o Chromium manda o quadro atual mesmo com a página parada.
    fn restart(&self) {
        drop(self.session.call("Page.stopScreencast", json!({})));
        self.apply(false);
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.surface.close();
        for popup in self.announced.take() { drop(self.session.browser().call(None, "Target.closeTarget", json!({"targetId": popup}))); }
        drop(self.session.browser().call(None, "Target.closeTarget", json!({"targetId": self.target})));
        if let Some(context) = &self.context {
            drop(self.session.browser().call(None, "Target.disposeBrowserContext", json!({"browserContextId": context})));
        }
    }
}

#[cfg(test)]
mod tests {
    // Sem `super::*`: o glob da gpui_kit traz um `test` próprio que sombreia o `#[test]` da std.
    use super::Slot;
    use serde_json::json;

    #[test]
    fn only_the_latest_frame_is_decoded_after_the_last_one_was_drawn() {
        let mut slot = Slot::default();
        let frame = |d: &str| Some((d.to_owned(), serde_json::Value::Null, None));
        slot.data = frame("a");
        slot.data = frame("b");
        assert_eq!(slot.next().map(|f| f.0).as_deref(), Some("b"), "o quadro trocado antes de decodificar não volta");
        slot.unseen = true;
        slot.data = frame("c");
        slot.data = frame("d");
        assert_eq!(slot.next(), None, "quadro anterior ainda não desenhado: espera");
        slot.unseen = false;
        assert_eq!(slot.next().map(|f| f.0).as_deref(), Some("d"));
        assert_eq!(slot.next(), None);
    }

    #[test]
    fn page_size_comes_from_the_frame_metadata() {
        let meta = json!({"deviceWidth": 718, "deviceHeight": 485, "pageScaleFactor": 1});
        assert_eq!(super::page_size(&meta), Some((718., 485.)));
        assert_eq!(super::page_size(&json!({})), None);
        assert_eq!(super::page_size(&json!({"deviceWidth": 0, "deviceHeight": 485})), None);
    }

    /// Listas como o Chromium manda no `Page.windowOpen` (conferidas no headless-shell e no Chrome completo).
    #[test]
    fn only_a_window_open_with_window_features_is_a_popup() {
        assert!(super::is_popup_window(&json!(["width=500", "height=600", "resizable"])), "login do Vault");
        assert!(!super::is_popup_window(&json!(["menubar", "toolbar", "status", "scrollbars", "resizable"])), "window.open(url) é aba");
        assert!(!super::is_popup_window(&json!(["left=0", "menubar", "toolbar", "status", "scrollbars", "resizable"])));
    }

    #[test]
    fn zoom_keeps_frame_pixels_and_divides_the_css_viewport() {
        assert_eq!(super::zoomed_metrics(1000., 600., 2., 1.), (1000, 600, 2.));
        assert_eq!(super::zoomed_metrics(1000., 600., 2., 2.), (500, 300, 4.));
        assert_eq!(super::zoomed_metrics(1000., 600., 2., 0.5), (2000, 1200, 1.));
        assert_eq!(super::zoomed_metrics(1000., 600., 2., 0.), (1000, 600, 2.), "zoom inválido vale 1");
    }

    /// Prova de ponta a ponta com o Chromium da máquina e a GPUI sem janela, num perfil e num HOME descartáveis:
    /// processo morto (SIGSEGV) e processo doente (sem zygote, toda página nova cai ao nascer) são trocados pela
    /// abertura seguinte, e aberturas simultâneas sobem um processo só.
    #[test]
    #[ignore = "sobe o Chromium da máquina"]
    fn a_dead_or_sick_chromium_is_replaced_by_the_next_open() {
        use std::{cell::RefCell, rc::Rc, time::{Duration, Instant}};
        super::launch::find().unwrap();
        let home = std::env::temp_dir().join(format!("hangar-relaunch-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        // SAFETY: antes de qualquer outra thread; o Chromium já foi achado com o HOME de verdade.
        unsafe { std::env::set_var("XDG_CONFIG_HOME", home.join("config")); std::env::set_var("HOME", &home); }
        let outcome: Rc<RefCell<Option<Result<(), String>>>> = Rc::default();
        let out = outcome.clone();
        gpui_kit::platform::headless().run(move |cx| {
            let (executor, background) = (cx.foreground_executor().clone(), cx.background_executor().clone());
            let starter = move || super::Starter { executor: executor.clone(), background: background.clone() };
            cx.spawn(async move |cx| {
                *out.borrow_mut() = Some(scenario(starter).await);
                let _ = cx.update(|cx| cx.quit());
            }).detach();
        });
        let native_log = std::fs::read_to_string(home.join(".hangar/logs/privado/native.log")).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&home);
        outcome.take().expect("o cenario nao terminou").unwrap();
        // A queda vai ao diário com o sinal e o fim do chromium.log; o fim do doente vai como descarte, não queda. Sem
        // zygote o doente às vezes aborta sozinho antes do descarte, e aí o fim dele é queda de verdade.
        let (exits, retired) = (native_log.matches("chromium_exited").count(), native_log.matches("Chromium descartado").count());
        assert_eq!((exits + retired, retired <= 1), (2, true), "{native_log}");
        assert!(native_log.contains("(sinal 11); fim do chromium.log:") && native_log.contains("Received signal 11"), "{native_log}");

        async fn scenario(starter: impl Fn() -> super::Starter) -> Result<(), String> {
            let check = |ok: bool, what: String| if ok { Ok(()) } else { Err(what) };
            let pid = |e: &super::Engine| e.session.browser().pid().unwrap_or(0);
            let (events, _kept) = async_channel::unbounded();
            let first = starter().start(events.clone()).await?;
            let old = pid(&first);
            let hanging = first.cdp().call("Runtime.evaluate", json!({"expression": "new Promise(() => {})", "awaitPromise": true}));
            // SAFETY: o Chromium que este teste subiu.
            unsafe { libc::kill(old as i32, libc::SIGSEGV); }
            let crashed = Instant::now();
            let failed = hanging.await.err().ok_or("a chamada pendente nao falhou")?;
            check(failed.contains("chromium_exited") && failed.contains("sinal 11"), format!("erro da chamada pendente: {failed}"))?;
            check(crashed.elapsed() < Duration::from_secs(5), format!("pendente falhou em {:?}", crashed.elapsed()))?;
            check(!first.alive(), "o motor do Chromium morto segue vivo".into())?;
            let (a, b) = futures::join!(starter().start(events.clone()), starter().start(events.clone()));
            let (a, b) = (a?, b?);
            eprintln!("refeito {:?} depois da queda", crashed.elapsed());
            check(crashed.elapsed() < Duration::from_secs(10), format!("refeito em {:?}", crashed.elapsed()))?;
            check(pid(&a) == pid(&b) && pid(&a) != old, format!("aberturas simultaneas: {} e {} (antigo {old})", pid(&a), pid(&b)))?;
            // Doente: sem zygote o processo segue de pé, mas toda página nova cai ao nascer.
            let sick = pid(&a);
            for entry in std::fs::read_dir("/proc").map_err(|e| e.to_string())?.flatten() {
                let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else { continue };
                let parent = stat.rsplit(')').next().and_then(|r| r.split_whitespace().nth(1)).and_then(|p| p.parse::<u32>().ok());
                let cmdline = std::fs::read(entry.path().join("cmdline")).unwrap_or_default();
                if parent == Some(sick) && cmdline.windows(13).any(|w| w == b"--type=zygote")
                    && let Some(zygote) = entry.file_name().to_str().and_then(|p| p.parse::<i32>().ok()) {
                    // SAFETY: filho do Chromium que este teste subiu.
                    unsafe { libc::kill(zygote, libc::SIGKILL); }
                }
            }
            let asked = Instant::now();
            let c = starter().start(events.clone()).await?;
            eprintln!("doente trocado em {:?}", asked.elapsed());
            check(pid(&c) != sick && c.alive(), format!("abertura no Chromium doente: pid {} (doente {sick})", pid(&c)))?;
            check(asked.elapsed() < Duration::from_secs(10), format!("troca do doente em {:?}", asked.elapsed()))?;
            check(!a.alive() && !b.alive(), "motores do Chromium descartado seguem vivos".into())?;
            Ok(())
        }
    }

    /// Duas sessões no mesmo Chromium: a página que a segunda abre não é popup de ninguém, e a primeira não a fecha (o
    /// que derrubava a abertura, descartava o processo e levava junto a página da primeira).
    #[test]
    #[ignore = "sobe o Chromium da máquina"]
    fn a_second_session_opens_beside_the_first_in_the_same_chromium() {
        use std::{cell::RefCell, rc::Rc, time::Duration};
        super::launch::find().unwrap();
        let home = std::env::temp_dir().join(format!("hangar-second-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        // SAFETY: antes de qualquer outra thread; o Chromium já foi achado com o HOME de verdade.
        unsafe { std::env::set_var("XDG_CONFIG_HOME", home.join("config")); std::env::set_var("HOME", &home); }
        let outcome: Rc<RefCell<Option<Result<(), String>>>> = Rc::default();
        let out = outcome.clone();
        gpui_kit::platform::headless().run(move |cx| {
            let (executor, background) = (cx.foreground_executor().clone(), cx.background_executor().clone());
            let starter = move || super::Starter { executor: executor.clone(), background: background.clone() };
            cx.spawn(async move |cx| {
                *out.borrow_mut() = Some(scenario(starter).await);
                let _ = cx.update(|cx| cx.quit());
            }).detach();
        });
        let _ = std::fs::remove_dir_all(&home);
        outcome.take().expect("o cenario nao terminou").unwrap();

        async fn scenario(starter: impl Fn() -> super::Starter) -> Result<(), String> {
            let pid = |e: &super::Engine| e.session.browser().pid().unwrap_or(0);
            let (events, _kept) = async_channel::unbounded();
            let first = starter().start(events.clone()).await?;
            let old = pid(&first);
            for round in 0..3 {
                let second = starter().start(events.clone()).await?;
                // O fechamento indevido chega depois da resposta: dá tempo dele aparecer.
                starter().background.timer(Duration::from_millis(500)).await;
                if !(first.alive() && second.alive() && pid(&second) == old) {
                    return Err(format!("rodada {round}: primeira viva {}, segunda viva {}, pid {} (antes {old})",
                        first.alive(), second.alive(), pid(&second)));
                }
            }
            // Popup `noopener` de página do app segue fechando: ele traz o opener, que o filtro acima exige.
            let browser = first.session.browser();
            let (born, gone): (Rc<RefCell<Vec<(String, String)>>>, Rc<RefCell<Vec<String>>>) = (Rc::default(), Rc::default());
            let b = born.clone();
            browser.watch("teste", "Target.targetCreated", move |p| b.borrow_mut().push((
                p["targetInfo"]["targetId"].as_str().unwrap_or("").into(), p["targetInfo"]["openerId"].as_str().unwrap_or("").into())));
            let g = gone.clone();
            browser.watch("teste", "Target.targetDestroyed", move |p| g.borrow_mut().push(p["targetId"].as_str().unwrap_or("").into()));
            first.cdp().call("Runtime.evaluate", json!({"expression": "window.open('http://127.0.0.1:9/', '_blank', 'noopener')", "userGesture": true})).await?;
            starter().background.timer(Duration::from_secs(2)).await;
            let popup = born.borrow().iter().find(|(id, _)| *id != first.target).cloned().ok_or("o popup nao nasceu")?;
            if popup.1 != first.target || !gone.borrow().contains(&popup.0) {
                return Err(format!("popup {popup:?} (opener esperado {}), fechados {:?}", first.target, gone.borrow()));
            }
            Ok(())
        }
    }

    /// Login por popup (o OIDC do Vault): `window.open` com features de janela vira alvo próprio, adotado com o opener
    /// intacto; o `postMessage` dele chega à origem e o `window.close()` avisa o painel. `window.open` sem features
    /// segue abrindo na própria página.
    #[test]
    #[ignore = "sobe o Chromium da máquina"]
    fn a_login_popup_keeps_its_opener_and_its_close_returns_to_the_origin() {
        use std::{cell::RefCell, io::{Read, Write}, net::TcpListener, rc::Rc, sync::{Arc, atomic::{AtomicBool, Ordering}}, time::Duration};
        super::launch::find().unwrap();
        let home = std::env::temp_dir().join(format!("hangar-popup-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        // SAFETY: antes de qualquer outra thread; o Chromium já foi achado com o HOME de verdade.
        unsafe { std::env::set_var("XDG_CONFIG_HOME", home.join("config")); std::env::set_var("HOME", &home); }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let base = format!("http://{addr}");
        let stop = Arc::new(AtomicBool::new(false));
        let server = {
            let stop = stop.clone();
            std::thread::spawn(move || for stream in listener.incoming() {
                if stop.load(Ordering::SeqCst) { break; }
                let Ok(mut stream) = stream else { continue };
                let mut request = [0u8; 4096];
                let n = stream.read(&mut request).unwrap_or(0);
                let path = String::from_utf8_lossy(&request[..n]).split_whitespace().nth(1).unwrap_or("/").to_owned();
                let body = match path.as_str() {
                    "/" => "<title>origem</title><script>addEventListener('message', e => document.title = 'got:' + e.data)</script>",
                    "/popup" => "<title>login</title><p>login</p>",
                    "/sozinho" => "<script>opener.postMessage('sozinho', '*'); close()</script>",
                    _ => "<title>outra</title>",
                };
                let _ = write!(stream, "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
            })
        };
        let outcome: Rc<RefCell<Option<Result<(), String>>>> = Rc::default();
        let out = outcome.clone();
        gpui_kit::platform::headless().run(move |cx| {
            let (executor, background) = (cx.foreground_executor().clone(), cx.background_executor().clone());
            let starter = move || super::Starter { executor: executor.clone(), background: background.clone() };
            cx.spawn(async move |cx| {
                *out.borrow_mut() = Some(scenario(starter, base).await);
                let _ = cx.update(|cx| cx.quit());
            }).detach();
        });
        // A conexão acorda o `accept`, que vê o pedido de parada.
        stop.store(true, Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(addr);
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(&home);
        outcome.take().expect("o cenario nao terminou").unwrap();

        async fn scenario(starter: impl Fn() -> super::Starter, base: String) -> Result<(), String> {
            let background = starter().background;
            let eval = async |engine: &super::Engine, expression: &str| -> Result<serde_json::Value, String> {
                let r = engine.cdp().call("Runtime.evaluate", json!({"expression": expression, "returnByValue": true, "userGesture": true})).await?;
                Ok(r["result"]["value"].clone())
            };
            // Espera por condição, com teto que só estoura se a página travar.
            let until = async |engine: &super::Engine, expression: &str, want: serde_json::Value| -> Result<(), String> {
                for _ in 0..100 {
                    if eval(engine, expression).await.ok() == Some(want.clone()) { return Ok(()); }
                    background.timer(Duration::from_millis(100)).await;
                }
                Err(format!("{expression} nunca deu {want}: {:?}", eval(engine, expression).await))
            };
            let next = async |received: &async_channel::Receiver<super::Event>, popup: bool| -> Option<super::Event> {
                let deadline = std::time::Instant::now() + Duration::from_secs(10);
                while std::time::Instant::now() < deadline {
                    match received.try_recv() {
                        Ok(e @ super::Event::Popup(_)) if popup => return Some(e),
                        Ok(e @ super::Event::Closed) if !popup => return Some(e),
                        Ok(_) => {}
                        Err(_) => background.timer(Duration::from_millis(50)).await,
                    }
                }
                None
            };
            let (events, received) = async_channel::unbounded();
            let origin = starter().start(events).await?;
            origin.load(&format!("{base}/"));
            until(&origin, "document.title", json!("origem")).await?;
            eval(&origin, "window.open('/popup', 'login', 'width=500,height=600'), 0").await?;
            let Some(super::Event::Popup(target)) = next(&received, true).await else { return Err("o popup nao foi anunciado".into()) };
            let (popup_events, popup_received) = async_channel::unbounded();
            let popup = origin.adopt(target, popup_events).await?;
            let shown = popup.page().url.unwrap_or_default();
            if !shown.ends_with("/popup") { return Err(format!("barra do popup sem o endereço dele: {shown:?}")); }
            until(&popup, "location.pathname + ' ' + !!window.opener", json!("/popup true")).await?;
            until(&origin, "location.pathname", json!("/")).await?;
            eval(&popup, "window.opener.postMessage('ok', '*'); window.close(); 0").await?;
            next(&popup_received, false).await.ok_or("o fechamento do popup nao chegou")?;
            drop(popup);
            until(&origin, "document.title + ' ' + location.pathname", json!("got:ok /")).await?;
            // Login já autorizado: o popup responde e fecha antes ou durante a adoção, e isso não é falha.
            eval(&origin, "window.open('/sozinho', 'login', 'width=500,height=600'), 0").await?;
            let Some(super::Event::Popup(target)) = next(&received, true).await else { return Err("o segundo popup nao foi anunciado".into()) };
            let (popup_events, popup_received) = async_channel::unbounded();
            match origin.adopt(target, popup_events).await {
                Ok(popup) => { next(&popup_received, false).await.ok_or("o popup que fechou sozinho nao avisou")?; drop(popup); }
                Err(e) if e.starts_with(super::POPUP_GONE) => {}
                Err(e) => return Err(format!("popup que fechou sozinho virou falha: {e}")),
            }
            until(&origin, "document.title", json!("got:sozinho")).await?;
            // "Fechar janela": o painel larga o popup, e a origem não abre o endereço dele no lugar.
            eval(&origin, "window.open('/popup', 'login', 'width=500,height=600'), 0").await?;
            let Some(super::Event::Popup(target)) = next(&received, true).await else { return Err("o terceiro popup nao foi anunciado".into()) };
            let (popup_events, _popup_received) = async_channel::unbounded();
            drop(origin.adopt(target, popup_events).await?);
            background.timer(Duration::from_secs(1)).await;
            let at = eval(&origin, "location.pathname").await?;
            if at != json!("/") { return Err(format!("fechar o popup levou a origem para {at}")); }
            // Sem features de janela é aba: abre na própria página, sem popup.
            eval(&origin, "window.open('/outra'), 0").await?;
            until(&origin, "location.pathname", json!("/outra")).await?;
            while let Ok(e) = received.try_recv() { if let super::Event::Popup(id) = e { return Err(format!("aba virou popup: {id}")); } }
            // Página descartada com um popup anunciado que ninguém adotou: ele fecha junto, sem ficar rodando sem dono.
            eval(&origin, "window.open('/popup', 'login', 'width=500,height=600'), 0").await?;
            let Some(super::Event::Popup(target)) = next(&received, true).await else { return Err("o quarto popup nao foi anunciado".into()) };
            let browser = origin.session.browser().clone();
            drop(origin);
            for _ in 0..50 {
                if browser.call(None, "Target.getTargetInfo", json!({"targetId": target})).await.is_err() { return Ok(()); }
                background.timer(Duration::from_millis(100)).await;
            }
            Err("popup nao adotado sobreviveu a pagina que o abriu".into())
        }
    }

}
