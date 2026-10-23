//! Página publicada pelo agente desenhada dentro da conversa: viva no Linux e no Windows, imagem estática no macOS.
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, VecDeque},
    rc::{Rc, Weak},
};

use serde::Deserialize;

use super::*;
use crate::browser::{Engine, Pointer};

pub const LIVE_MAX: usize = 4;
const MIN: u32 = 80;
const MAX: u32 = 2000;
const FALLBACK: f32 = 240.;
/// Moldura do site (modo URL) sem altura pedida; o servidor já manda esta, o app repete para resultado antigo.
const SITE_HEIGHT: u32 = 640;
/// Clique ou tecla na página que ainda vale como gesto para ela abrir um link.
pub(super) const GESTURE: Duration = Duration::from_secs(2);
/// Espera do último quadro da página que sai do orçamento.
#[cfg(any(target_os = "linux", target_os = "windows"))]
const PARK_SHOT: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct PageRef {
    pub id: String, pub title: String, #[serde(default)] pub height: Option<u32>, #[serde(default)] pub heights: BTreeMap<u32, u32>,
    // A página leva as próprias cores: o app não lhe passa o tema.
    #[serde(default)] pub own_theme: bool,
    // Site de verdade (modo URL): o cartão abre o endereço em vez de um documento.
    #[serde(default)] pub url: Option<String>,
}

/// O resultado de MCP chega como texto, como lista de blocos `{type: "text", text}` ou, no Codex, como o
/// CallToolResult inteiro (`content` + `structuredContent`): o estruturado vence, senão os blocos são juntados.
pub fn page_from_result(tool_name: &str, result: &str) -> Option<PageRef> {
    if !conversation::is_page_call(Some(tool_name)) { return None; }
    #[derive(Deserialize)]
    struct Out { hangar_page: PageRef }
    let mut page = serde_json::from_value::<Out>(result_value(result)?).ok()?.hangar_page;
    page.url = page.url.filter(|u| u.starts_with("http://") || u.starts_with("https://"));
    Some(page)
}

fn result_value(result: &str) -> Option<Value> {
    let join = |blocks: &[Value]| serde_json::from_str::<Value>(&blocks.iter()
        .filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<String>()).ok();
    let value: Value = serde_json::from_str(result).ok()?;
    Some(match value {
        Value::Array(blocks) => join(&blocks)?,
        Value::Object(ref o) if o.get("hangar_page").is_none() && o.get("content").is_some_and(Value::is_array) => {
            match o.get("structuredContent") {
                Some(s) if s.get("hangar_page").is_some() => s.clone(),
                _ => join(o["content"].as_array()?)?,
            }
        }
        other => other,
    })
}

/// Chamada de página que terminou sem `hangar_page` (erro da tool, formato desconhecido): o texto recebido,
/// cortado, para o cartão mostrar. Rascunho não é falha: segue como chamada comum.
pub fn page_failure(tool_name: &str, result: &str) -> Option<String> {
    if !conversation::is_page_call(Some(tool_name)) || page_from_result(tool_name, result).is_some() { return None; }
    if result_value(result).is_some_and(|v| v.get("draft").is_some()) { return None; }
    let (text, cut) = conversation::clip(result.trim(), 300);
    Some(if cut { format!("{text}…") } else { text.to_owned() })
}

fn natural(page: &PageRef, width: f32, reported: Option<f32>) -> f32 {
    reported.unwrap_or_else(|| {
        page.heights.iter().filter(|(_, h)| **h > 0)
            .min_by_key(|(w, _)| (**w as f32 - width).abs() as u32).map_or(FALLBACK, |(_, h)| *h as f32)
    })
}

pub fn frame_height(page: &PageRef, width: f32, reported: Option<f32>) -> f32 {
    if page.url.is_some() { return page.height.unwrap_or(SITE_HEIGHT).clamp(MIN, MAX) as f32; }
    let natural = natural(page, width, reported);
    let capped = page.height.map_or(natural, |h| natural.min(h as f32));
    capped.clamp(MIN as f32, MAX as f32)
}

/// A página rola dentro do quadro (o agente limitou a altura ou ela passa do teto): só então a roda é dela.
fn scrolls_inside(page: &PageRef, width: f32, reported: Option<f32>) -> bool {
    natural(page, width, reported) > frame_height(page, width, reported) + 0.5
}

/// Fila das páginas vivas, mais recente no fim: passou do teto, a mais antiga sai. A tela cheia ocupa uma vaga fora da
/// fila enquanto aberta: o motor dela não é de nenhum cartão.
#[derive(Default)]
pub struct Budget { order: VecDeque<String>, pinned: usize }

impl Budget {
    /// Marca `id` como usada agora; devolve quem deve fechar.
    pub fn touch(&mut self, id: &str) -> Option<String> {
        self.order.retain(|x| x != id);
        self.order.push_back(id.to_owned());
        (self.order.len() + self.pinned > LIVE_MAX).then(|| self.order.pop_front()).flatten()
    }

    /// Tela cheia abriu: reserva a vaga e devolve quem deve fechar para ela caber.
    pub fn pin(&mut self) -> Vec<String> {
        self.pinned += 1;
        let mut out = Vec::new();
        while self.order.len() + self.pinned > LIVE_MAX && let Some(old) = self.order.pop_front() { out.push(old); }
        out
    }

    pub fn unpin(&mut self) { self.pinned = self.pinned.saturating_sub(1); }

    /// Conversa trocada: a fila esvazia, a vaga da tela cheia aberta continua dela.
    pub fn clear(&mut self) { self.order.clear(); }
}

#[derive(Debug, PartialEq)]
pub enum HostMsg { Height(f32), Link(String, bool) }

/// Mensagem da página (`window.hangarHost`). Entrada não confiável: os iframes http(s) permitidos também chamam a
/// ponte. Só os dois métodos, altura finita e positiva, link só http(s).
pub fn parse_host(raw: &str) -> Option<HostMsg> {
    let message: Value = serde_json::from_str(raw).ok()?;
    let params = &message["params"];
    match message["method"].as_str()? {
        "ui/notifications/size-changed" => params["height"].as_f64().filter(|h| h.is_finite() && *h > 0.).map(|h| HostMsg::Height(h as f32)),
        "ui/open-link" => {
            let url = url::Url::parse(params["url"].as_str()?).ok()?;
            let external = params["ctrlKey"] == true || params["metaKey"] == true;
            matches!(url.scheme(), "http" | "https").then(|| HostMsg::Link(url.into(), external))
        }
        _ => None,
    }
}

/// Quem pintou a página neste quadro. A lista não pinta linha fora da tela: quem estava no quadro anterior e não
/// voltou neste esconde o motor e fica "fora", e a página estacionada só recarrega depois de ter saído e voltado.
#[derive(Default)]
pub struct Paint {
    now: RefCell<Vec<(String, Option<Weak<Engine>>)>>,
    before: RefCell<Vec<(String, Option<Weak<Engine>>)>>,
    away: RefCell<HashSet<String>>,
}

impl Paint {
    fn mark(&self, id: &str, engine: Option<&Rc<Engine>>) { self.now.borrow_mut().push((id.to_owned(), engine.map(Rc::downgrade))); }

    /// Canvas antes (`start`) e depois da lista da conversa.
    pub fn edge(this: &Rc<Self>, start: bool) -> impl IntoElement {
        let paint = this.clone();
        canvas(|_, _, _| {}, move |_, _, _, _| if start { paint.now.borrow_mut().clear() } else { paint.end() }).absolute().size_0()
    }

    fn end(&self) {
        let now = self.now.borrow();
        for (id, engine) in self.before.borrow().iter().filter(|(id, _)| !now.iter().any(|(n, _)| n == id)) {
            if let Some(engine) = engine.as_ref().and_then(Weak::upgrade) { engine.hide(); }
            self.away.borrow_mut().insert(id.clone());
        }
        drop(now);
        self.before.swap(&self.now);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ViewState { Loading, Ready, Error, Expired }

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Miss { NoImage, Expired, Other }

/// O 404 da página traz o motivo no código; sem ele é erro comum.
pub(super) fn miss(status: Option<u16>, code: Option<&str>) -> Miss {
    match (status, code) {
        (Some(404), Some("erro_pagina_sem_imagem")) => Miss::NoImage,
        (Some(404), Some("erro_pagina_expirou")) => Miss::Expired,
        _ => Miss::Other,
    }
}

/// Tela cheia só com a página à vista. Site sem página viva só abre no navegador, e sem imagem não há o que ampliar.
fn expandable(state: ViewState, live: bool, site: bool, no_image: bool) -> bool {
    state == ViewState::Ready && !no_image && (live || !site)
}

/// Tecla para a página viva. O Chromium sem janela tem área de transferência própria: colar lê a do sistema e digita.
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(super) fn forward_key(engine: &Engine, k: &Keystroke, cx: &App) {
    let paste = (k.modifiers.control && !k.modifiers.alt && k.key == "v") || (k.modifiers.shift && k.key == "insert");
    match paste.then(|| cx.read_from_clipboard().and_then(|item| item.text())).flatten() {
        Some(text) => engine.insert_text(&text),
        None => engine.key(true, k),
    }
}

/// Motor vivo da página e o laço que lê os eventos dele; caem juntos.
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(super) struct Running {
    pub(super) engine: Rc<Engine>, pub(super) _drain: Task<()>, pub(super) framed: bool, pub(super) theme_sent: Option<ThemeKey>,
    pub(super) at: Option<String>,
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
impl Running {
    /// Entrega o tema do app à página quando ele mudou desde o último envio; o primeiro leva antes as fontes.
    pub(super) fn push_theme(&mut self) {
        let key = theme_key();
        if self.theme_sent == Some(key) { return; }
        if self.theme_sent.is_none() { self.engine.evaluate(&APP_FONTS); }
        self.engine.evaluate(&theme_script());
        self.theme_sent = Some(key);
    }
}

struct PageView {
    page: PageRef,
    /// Id da chamada, que é o da linha na lista: a altura nova remede só ela.
    row: String,
    state: ViewState,
    /// Viva (Linux com Chromium); `false` é a imagem estática do servidor.
    live: bool,
    /// Pedido ou motor nascendo: não pede de novo.
    busy: bool,
    html: Option<Rc<str>>,
    reported: Option<f32>,
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    running: Option<Running>,
    /// Saiu do orçamento: mostra o último quadro até voltar à tela.
    parked: bool,
    last_frame: Option<Arc<RenderImage>>,
    /// Imagem estática, o tema (escuro?) e a largura do servidor em que foi tirada.
    shot: Option<(bool, u32, Arc<RenderImage>)>,
    no_image: bool,
    focus: FocusHandle,
    gesture: Option<Instant>,
    origin: Rc<Cell<Point<Pixels>>>,
    /// O botão desceu nesta página: só ela recebe a soltura de fora.
    pressed: bool,
    /// Recolhida: só o cabeçalho, sem motor nem quadro guardado, para liberar memória e CPU.
    collapsed: bool,
}

pub struct Pages {
    views: HashMap<String, PageView>,
    budget: Budget,
    pub paint: Rc<Paint>,
    window: AnyWindowHandle,
    /// Largura do cartão no último desenho: escolhe a altura medida mais próxima e a imagem.
    width: Rc<Cell<f32>>,
    live: bool,
    /// Chromium já de pé para a conversa aberta, que tem página.
    #[cfg(target_os = "linux")]
    warm: Option<crate::browser::Warm>,
}

impl Pages {
    pub fn new(window: AnyWindowHandle) -> Self {
        let live = cfg!(any(target_os = "linux", target_os = "windows")) && Engine::available().is_ok();
        Self {
            views: HashMap::new(), budget: Budget::default(), paint: Rc::default(), window, width: Rc::new(Cell::new(COLUMN)), live,
            #[cfg(target_os = "linux")]
            warm: None,
        }
    }

    /// Conversa com página: o Chromium das páginas sobe já, em paralelo ao resto, e não no primeiro desenho do cartão
    /// (que pode nem estar à vista). Sem página, ele é solto e nada sobe.
    pub fn warm(&mut self, wanted: bool, window: &Window, cx: &App) {
        #[cfg(target_os = "linux")]
        {
            if !wanted || !self.live { self.warm = None; return; }
            if self.warm.is_some() { return; }
            match Engine::warm_pages(window, cx) {
                Ok(warm) => self.warm = Some(warm),
                Err(error) => eprintln!("[pagina] Chromium não subiu antes da página: {error}"),
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (wanted, window, cx);
    }

    /// Conversa trocada ou recarregada: nada da anterior é reaproveitado (a página pode ter expirado). Os motores caem
    /// junto com as visões.
    pub fn clear(&mut self) {
        self.views.clear();
        self.budget.clear();
        self.paint.away.borrow_mut().clear();
    }

    /// Tela cheia fechou: a vaga dela volta para os cartões.
    pub fn unpin(&mut self) { self.budget.unpin(); }

    /// Conversa refeita: sai quem não tem mais linha.
    pub fn retain_rows(&mut self, keep: impl Fn(&String) -> bool) {
        self.views.retain(|_, view| keep(&view.row));
        let views = &self.views;
        self.budget.order.retain(|id| views.contains_key(id));
    }
}

/// Largura do cartão na pintura. Mudou: o próximo quadro redesenha, para a altura e a imagem seguirem a coluna (no
/// meio da pintura o `refresh` não vale).
fn record_width(cell: &Cell<f32>, bounds: Bounds<Pixels>, window: &mut Window) {
    let width = f32::from(bounds.size.width);
    if (cell.replace(width) - width).abs() > 0.5 { window.on_next_frame(|window, _| window.refresh()); }
}

/// Larguras em que o servidor tira a imagem (`WIDTHS` da rota `shot`).
pub(super) fn shot_width(width: f32) -> u32 {
    [360, 728, 1000].into_iter().min_by_key(|w| (*w as f32 - width).abs() as u32).unwrap_or(728)
}

fn hex(color: Hsla) -> String {
    let c = color.to_rgb();
    let ch = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    // Borda e texto apagado do tema são translúcidos: o alfa vai junto, senão a linha sai pesada.
    if c.a >= 1. { format!("#{:02x}{:02x}{:02x}", ch(c.r), ch(c.g), ch(c.b)) } else { format!("#{:02x}{:02x}{:02x}{:02x}", ch(c.r), ch(c.g), ch(c.b), ch(c.a)) }
}

/// A fonte do app vem embutida nele, não instalada: sem isto o Chromium da página cai na fonte do sistema, mais fina,
/// e a página destoa do texto ao lado.
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(super) static APP_FONTS: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    use base64::Engine as _;
    let face = |bytes: &[u8], weight: u16| format!("@font-face{{font-family:\"{}\";font-weight:{weight};src:url(data:font/ttf;base64,{})}}",
        theme::SANS, base64::engine::general_purpose::STANDARD.encode(bytes));
    let css = [face(include_bytes!("../../assets/fonts/Geist-Regular.ttf"), 400), face(include_bytes!("../../assets/fonts/Geist-Medium.ttf"), 500),
        face(include_bytes!("../../assets/fonts/Geist-SemiBold.ttf"), 600), face(include_bytes!("../../assets/fonts/Geist-Bold.ttf"), 700)].concat();
    format!("(()=>{{if(document.getElementById('hangar-fonts'))return;const s=document.createElement('style');s.id='hangar-fonts';s.textContent={};document.head.appendChild(s)}})()",
        serde_json::to_string(&css).unwrap_or_default())
});

/// Cores do app que a página recebe, uma tabela só para a chave e o script. O que o app não tem (`--info`, gráficos)
/// fica com o padrão do esquema que o servidor guardou na página.
#[cfg(any(target_os = "linux", target_os = "windows"))]
fn palette() -> [(&'static str, Hsla); 10] {
    [("--foreground", theme::text()), ("--muted-foreground", theme::muted()), ("--surface", theme::surface()),
        ("--border", theme::border()), ("--accent", theme::accent()), ("--accent-foreground", theme::on_accent()),
        ("--danger", theme::danger()), ("--warning", theme::warning()), ("--success", theme::success()),
        ("--code-background", theme::inset())]
}

/// O que o script do tema lê do app: chave igual, script igual, sem montá-lo a cada quadro.
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[derive(Clone, Copy, PartialEq)]
pub(super) struct ThemeKey { dark: bool, colors: [Hsla; 10] }

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn theme_key() -> ThemeKey { ThemeKey { dark: theme::is_dark(), colors: palette().map(|(_, c)| c) } }

/// Script que entrega o tema do app à página, nos nomes de variável da spec.
#[cfg(any(target_os = "linux", target_os = "windows"))]
fn theme_script() -> String {
    let mut vars: serde_json::Map<String, Value> = palette().into_iter().map(|(name, c)| (name.to_owned(), hex(c).into())).collect();
    vars.insert("--background".into(), "transparent".into());
    vars.insert("--font-sans".into(), format!("\"{}\", system-ui, sans-serif", theme::SANS).into());
    vars.insert("--font-mono".into(), format!("\"{}\", ui-monospace, monospace", theme::MONO).into());
    let params = json!({"theme": if theme::is_dark() { "dark" } else { "light" }, "styles": {"variables": vars}});
    format!("window.__hangarApply&&window.__hangarApply({params})")
}

impl Hangar {
    /// Página publicada no lugar da chamada `html_render`, quando o resultado pareado traz `hangar_page`.
    pub(super) fn tool_page(&self, tool: Tool) -> Option<PageRef> {
        let call = &self.chat.events[tool.call];
        page_from_result(call.tool_name.as_deref()?, self.chat.events[tool.result?].result.as_deref()?)
    }

    /// Chamada de página já respondida sem página: aviso visível com o que veio, nunca a chamada genérica calada.
    /// Ainda rodando (sem resultado) fica com o cartão comum.
    pub(super) fn render_page_failure(&self, tool: Tool) -> Option<AnyElement> {
        let call = &self.chat.events[tool.call];
        let result = self.chat.events[tool.result?].result.clone().unwrap_or_default();
        let received = page_failure(call.tool_name.as_deref()?, &result)?;
        let title = call.tool_input.as_ref().and_then(|i| i.get("title")).and_then(Value::as_str).unwrap_or_default().to_owned();
        Some(v_flex().id(SharedString::from(format!("page-failed-{}", call.id))).py(px(12.)).gap_1().role(Role::Alert)
            .child(div().text_size(px(13.)).text_color(theme::warning()).whitespace_normal().child(tr_shared("page_error", &[("title", &title)])))
            .child(div().text_size(px(12.)).text_color(theme::muted()).whitespace_normal().child(received))
            .into_any_element())
    }

    fn remeasure_page(&mut self, id: &str) {
        let Some(row) = self.pages.views.get(id).map(|v| v.row.clone()) else { return };
        if let Some(i) = self.row_ids.iter().position(|r| *r == row) { self.list_state.remeasure_items(i..i + 1); }
    }

    fn page_failed(&mut self, id: &str, error: &Failure) {
        let Some(view) = self.pages.views.get_mut(id) else { return };
        view.busy = false;
        match miss(error.status, error.code.as_deref()) {
            Miss::NoImage => { view.no_image = true; view.state = ViewState::Ready; }
            Miss::Expired => view.state = ViewState::Expired,
            Miss::Other => { eprintln!("[pagina] {id}: {}", error.detail); view.state = ViewState::Error; }
        }
    }

    /// Busca o que falta para a página aparecer: o HTML e o motor (viva) ou a imagem no tema atual (estática).
    fn drive_page(&mut self, id: &str, cx: &mut Context<Self>) {
        let (Some(api), Some(key)) = (self.session_api(), self.selected_key()) else { return };
        let width = self.pages.width.get();
        let Some(view) = self.pages.views.get_mut(id) else { return };
        if view.busy || matches!(view.state, ViewState::Error | ViewState::Expired) { return; }
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        if view.live {
            if view.html.is_none() && view.page.url.is_none() {
                view.busy = true;
                let (page_id, name) = (id.to_owned(), key.name.clone());
                let task = self.runtime.spawn(async move { api.page(&name, &page_id, &[], &[("raw", "1")]).await });
                let id = id.to_owned();
                cx.spawn(async move |this, cx| {
                    let result = task.await.unwrap_or_else(|_| Err(Failure::local("network_error")));
                    let _ = this.update(cx, |this, cx| {
                        match result {
                            Ok(bytes) => if let Some(view) = this.pages.views.get_mut(&id) {
                                view.busy = false;
                                view.html = Some(String::from_utf8_lossy(&bytes).into());
                            },
                            Err(error) => this.page_failed(&id, &error),
                        }
                        this.remeasure_page(&id);
                        cx.notify();
                    });
                }).detach();
            } else if view.running.is_none() && !view.parked {
                self.start_page_engine(id, cx);
            }
            return;
        }
        // Site sem página viva: o cartão só abre o endereço, nada a buscar.
        if view.page.url.is_some() { view.state = ViewState::Ready; return; }
        let dark = theme::is_dark();
        let bucket = shot_width(width);
        if view.no_image || view.shot.as_ref().is_some_and(|(d, w, _)| *d == dark && *w == bucket) { return; }
        view.busy = true;
        let (page_id, name, w) = (id.to_owned(), key.name.clone(), bucket.to_string());
        let task = self.runtime.spawn(async move {
            let bytes = api.page(&name, &page_id, &["shot"], &[("theme", if dark { "dark" } else { "light" }), ("width", &w)]).await?;
            Ok::<_, Failure>(tokio::task::spawn_blocking(move || media::decode(&bytes, 4096, 8192, None)).await.ok().flatten())
        });
        let id = id.to_owned();
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|_| Err(Failure::local("network_error")));
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(image) => if let Some(view) = this.pages.views.get_mut(&id) {
                        view.busy = false;
                        match image {
                            Some(image) => { view.shot = Some((dark, bucket, image)); view.state = ViewState::Ready; }
                            None => view.state = ViewState::Error,
                        }
                    },
                    Err(error) => this.page_failed(&id, &error),
                }
                this.remeasure_page(&id);
                cx.notify();
            });
        }).detach();
    }

    /// Abre o motor da página sem travar a janela: o cartão mostra o carregando, na altura reservada, até ele chegar.
    /// Falhou, a página cai para a imagem.
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    fn start_page_engine(&mut self, id: &str, cx: &mut Context<Self>) {
        let (handle, width) = (self.pages.window, self.pages.width.get());
        let Some(view) = self.pages.views.get_mut(id) else { return };
        let (url, html) = (view.page.url.clone(), view.html.clone());
        if url.is_none() && html.is_none() { return; }
        // Tema próprio nasce no branco do navegador, como a página foi desenhada; ela pinta por cima.
        let background = view.page.own_theme.then_some((255, 255, 255));
        view.busy = true;
        let id = id.to_owned();
        cx.spawn(async move |this, cx| {
            // Sem limite: o motor manda com `try_send` e quadro perdido não volta.
            let (events, received) = async_channel::unbounded();
            let starter = handle.update(cx, |_, window, cx| Engine::prepare(window, cx)).map_err(|e| e.to_string()).and_then(|r| r);
            let engine = match (starter, url) {
                (Ok(starter), Some(url)) => starter.start_url(url, events).await,
                (Ok(starter), None) => starter.start_page(html.unwrap_or_else(|| "".into()), width, background, events).await,
                (Err(error), _) => Err(error),
            }.map(Rc::new);
            // Cartão que saiu ou recolheu no meio: o motor cai em `page_engine_ready` e fecha alvo e contexto.
            let _ = this.update(cx, |this, cx| this.page_engine_ready(id, engine, received, cx));
        }).detach();
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    fn page_engine_ready(&mut self, id: String, engine: Result<Rc<Engine>, String>, received: async_channel::Receiver<crate::browser::Event>, cx: &mut Context<Self>) {
        let Some(view) = self.pages.views.get_mut(&id) else { return };
        view.busy = false;
        // Recolhida enquanto o motor nascia: ele cai aqui mesmo.
        if view.collapsed { return; }
        let engine = match engine {
            Ok(engine) => engine,
            Err(error) => {
                eprintln!("[pagina] motor da página {id} falhou: {error}");
                view.live = false;
                cx.notify();
                return;
            }
        };
        let owner = id.clone();
        let drain = cx.spawn(async move |this, cx| {
            while let Ok(event) = received.recv().await {
                if this.update(cx, |this, cx| this.page_event(&owner, event, cx)).is_err() { break; }
            }
        });
        view.running = Some(Running { engine, _drain: drain, framed: false, theme_sent: None, at: None });
        view.state = ViewState::Ready;
        if let Some(old) = self.pages.budget.touch(&id) { self.park_page(&old, cx); }
        cx.notify();
    }

    /// Fora do orçamento: guarda a tela atual como imagem e fecha o motor (o `Drop` fecha alvo e contexto).
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    fn park_page(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(view) = self.pages.views.get_mut(id) else { return };
        let Some(running) = view.running.take() else { return };
        view.parked = true;
        self.pages.paint.away.borrow_mut().remove(id);
        let shot = running.engine.cdp().call("Page.captureScreenshot", json!({"format": "png"}));
        let id = id.to_owned();
        cx.spawn(async move |this, cx| {
            use base64::Engine as _;
            use futures::future::{Either, select};
            // Página escondida que não responde não segura o motor fora do orçamento: cai sem o último quadro.
            let timer = cx.background_executor().timer(PARK_SHOT);
            let reply = match select(std::pin::pin!(shot), std::pin::pin!(timer)).await { Either::Left((reply, _)) => reply.ok(), Either::Right(_) => None };
            let frame = reply
                .and_then(|v| base64::engine::general_purpose::STANDARD.decode(v["data"].as_str().unwrap_or("")).ok())
                .and_then(|bytes| media::decode(&bytes, 4096, 8192, None));
            drop(running);
            let _ = this.update(cx, |this, cx| {
                if let Some(view) = this.pages.views.get_mut(&id) { view.last_frame = frame; }
                cx.notify();
            });
        }).detach();
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    fn page_event(&mut self, id: &str, event: crate::browser::Event, cx: &mut Context<Self>) {
        let Some(view) = self.pages.views.get_mut(id) else { return };
        match event {
            crate::browser::Event::Frame => {
                let Some(running) = &mut view.running else { return };
                let first = !std::mem::replace(&mut running.framed, true);
                let replaced = running.engine.texture_replaced();
                // Visto só quando a janela desenhar: quadro que chega antes espera, e só o último é decodificado.
                let engine = Rc::downgrade(&running.engine);
                let _ = cx.update_window(self.pages.window, move |_, window, _| {
                    window.on_next_frame(move |_, _| if let Some(engine) = engine.upgrade() { engine.frame_seen() });
                });
                // Mesma textura, com o conteúdo novo: a cópia guardada da conversa já a pinta, basta a raiz redesenhar.
                // Textura nova (primeiro quadro, outro tamanho) ou sem relógio na raiz: a conversa redesenha.
                if first || replaced || !crate::motion::tick_root(cx) { self.redraw(panes::Area::Conversation, cx); }
            }
            crate::browser::Event::Host(raw) => match parse_host(&raw) {
                Some(HostMsg::Height(height)) if view.reported != Some(height) => {
                    #[cfg(target_os = "windows")]
                    crate::browser::frames::trace(format_args!("página pediu altura {height}"));
                    view.reported = Some(height);
                    self.remeasure_page(id);
                    self.redraw(panes::Area::Conversation, cx);
                }
                // Só logo depois de um clique ou tecla na página: sozinha ela não abre o navegador.
                Some(HostMsg::Link(url, external)) if view.gesture.take().is_some_and(|at| at.elapsed() < GESTURE) => {
                    super::links::open_link_later(self.pages.window, url, external, false, cx);
                }
                _ => {}
            },
            // Endereço atual do site, para a faixa do cartão.
            crate::browser::Event::UserInput | crate::browser::Event::Escape | crate::browser::Event::Popup(_) | crate::browser::Event::Closed => {}
            crate::browser::Event::State(state) => if let Some(running) = &mut view.running && running.at != state.url {
                running.at = state.url;
                self.redraw(panes::Area::Conversation, cx);
            },
        }
    }

    /// Recolher fecha o motor sem guardar quadro; abrir de novo recarrega (o site, no último endereço visitado).
    fn toggle_page(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(view) = self.pages.views.get_mut(id) else { return };
        view.collapsed = !view.collapsed;
        if view.collapsed {
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            if let Some(running) = view.running.take() && view.page.url.is_some() && let Some(at) = running.at {
                view.page.url = Some(at);
            }
            (view.parked, view.last_frame, view.pressed) = (false, None, false);
            self.pages.budget.order.retain(|x| x != id);
            self.pages.paint.away.borrow_mut().remove(id);
        }
        self.remeasure_page(id);
        cx.notify();
    }

    fn retry_page(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(view) = self.pages.views.get_mut(id) {
            (view.state, view.busy, view.html, view.shot, view.no_image) = (ViewState::Loading, false, None, None, false);
            view.live = self.pages.live;
        }
        self.remeasure_page(id);
        cx.notify();
    }

    /// Baixa a casca isolada pelo `Api` (token no cabeçalho, nunca na URL), grava numa cópia privada e a abre.
    pub(super) fn open_page_in_browser(&mut self, id: String, cx: &mut Context<Self>) {
        let (Some(api), Some(key)) = (self.session_api(), self.selected_key()) else { return };
        let name = key.name.clone();
        let task = self.runtime.spawn(async move {
            let bytes = api.page(&name, &id, &[], &[]).await.map_err(|error| Self::fetch_failure(&error))?;
            let path = private_copy(&format!("{}.html", composer::safe_name(&id))).map_err(|_| tr("save_failed"))?;
            tokio::task::spawn_blocking(move || std::fs::write(&path, bytes).map(|()| path)).await
                .ok().and_then(Result::ok).ok_or_else(|| tr("save_failed"))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|_| Err(tr("save_failed")));
            let _ = this.update(cx, |this, cx| {
                match result.and_then(|path| url::Url::from_file_path(&path).map_err(|()| tr("save_failed"))) {
                    Ok(url) => cx.open_url(url.as_str()),
                    Err(error) => { this.action_feedback.insert(key, (error, true)); }
                }
                cx.notify();
            });
        }).detach();
    }

    /// Abre a página em tela cheia. Viva: motor próprio, que toma uma vaga do orçamento até o diálogo fechar.
    fn open_page_fullscreen(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(api), Some(key)) = (self.session_api(), self.selected_key()) else { return };
        let Some(view) = self.pages.views.get(id) else { return };
        if !expandable(view.state, view.live, view.page.url.is_some(), view.no_image) { return; }
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        let start = view.page.url.clone().map(|url| view.running.as_ref().and_then(|r| r.at.clone()).unwrap_or(url));
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        let start = None;
        let source = page_fullscreen::Source { page: view.page.clone(), html: view.html.clone(), live: view.live, start };
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        if source.live { for old in self.pages.budget.pin() { self.park_page(&old, cx); } }
        let host = page_fullscreen::Host { hangar: cx.entity().downgrade(), api, name: key.name.clone(), runtime: self.runtime.clone(),
            window: self.pages.window };
        page_fullscreen::open(host, source, window, cx);
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    fn page_engine(&self, id: &str) -> Option<Rc<Engine>> {
        self.pages.views.get(id)?.running.as_ref().map(|r| r.engine.clone())
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    /// Os modificadores vão junto: Ctrl/Cmd+clique num link da página vai ao navegador do sistema, como no chat.
    fn page_pointer(&mut self, id: &str, kind: Pointer, at: Point<Pixels>, clicks: usize, modifiers: &Modifiers) {
        let Some(view) = self.pages.views.get_mut(id) else { return };
        match kind {
            Pointer::Down => view.pressed = true,
            // Soltura de um arrasto que não começou nesta página não é dela.
            Pointer::Up if !std::mem::take(&mut view.pressed) => return,
            _ => {}
        }
        let origin = view.origin.get();
        if let Some(engine) = self.page_engine(id) { engine.pointer(kind, at - origin, clicks, modifiers); }
    }

    pub(super) fn render_page_card(&mut self, tool: Tool, page: PageRef, cx: &mut Context<Self>) -> AnyElement {
        let id = page.id.clone();
        let row = self.chat.events[tool.call].id.clone();
        let live = self.pages.live;
        self.pages.views.entry(id.clone()).or_insert_with(|| PageView {
            page: page.clone(), row, state: ViewState::Loading, live, busy: false, html: None, reported: None,
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            running: None,
            parked: false, last_frame: None, shot: None, no_image: false, focus: cx.focus_handle(), gesture: None, origin: Rc::default(), pressed: false, collapsed: false,
        });
        if let Some(view) = self.pages.views.get(&id).filter(|v| v.collapsed) {
            let title = div().min_w_0().truncate().text_size(px(12.)).text_color(theme::muted()).child(view.page.title.clone());
            let address = view.page.url.clone().map(|url| div().flex_1().min_w_0().truncate().text_size(px(12.)).text_color(theme::faint()).child(url));
            return h_flex().w_full().gap_2()
                .child(h_flex().flex_1().min_w_0().gap_2().child(title).children(address))
                .child(toggle_button(&id, true, cx))
                .into_any_element();
        }
        // Estacionada que saiu da tela e voltou recarrega.
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        if self.pages.views.get(&id).is_some_and(|v| v.parked) && self.pages.paint.away.borrow_mut().remove(&id) {
            if let Some(view) = self.pages.views.get_mut(&id) { view.parked = false; }
        }
        self.drive_page(&id, cx);
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        if self.pages.views.get(&id).is_some_and(|v| v.running.is_some()) { self.pages.budget.touch(&id); }
        let width = self.pages.width.get();
        let Some(view) = self.pages.views.get_mut(&id) else { return div().into_any_element() };
        let full = expandable(view.state, view.live, view.page.url.is_some(), view.no_image);
        let title = view.page.title.clone();
        let height = px(frame_height(&view.page, width, view.reported));
        let note = |text: String| div().py(px(12.)).text_size(px(13.)).text_color(theme::muted()).whitespace_normal().child(text);
        match view.state {
            ViewState::Expired => return note(tr_shared("page_expired", &[])).into_any_element(),
            ViewState::Error => {
                let retry = id.clone();
                return h_flex().id(SharedString::from(format!("page-error-{id}"))).gap_2().role(Role::Alert)
                    .child(note(tr_shared("page_error", &[("title", &title)])))
                    .child(Button::new(SharedString::from(format!("page-retry-{id}"))).ghost().xsmall().label(tr_shared("page_retry", &[]))
                        .on_click(cx.listener(move |this, _, _, cx| this.retry_page(&retry, cx))))
                    .into_any_element();
            }
            _ => {}
        }
        let loading = || div().id(SharedString::from(format!("page-loading-{id}"))).size_full().flex().items_center().role(Role::Status).child(note(tr_shared("page_loading", &[("title", &title)])));
        if !view.live && let Some(url) = view.page.url.clone() {
            return v_flex().w_full().gap_1()
                .child(div().truncate().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(title))
                .child(h_flex().gap_2()
                    .child(div().flex_1().min_w_0().truncate().text_size(px(12.)).text_color(theme::muted()).child(url.clone()))
                    .child(Button::new(SharedString::from(format!("page-open-{id}"))).ghost().xsmall().icon(IconName::ExternalLink)
                        .label(tr_shared("page_open_browser", &[]))
                        .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&url))))
                    .child(toggle_button(&id, false, cx)))
                .into_any_element();
        }
        if !view.live {
            let open = id.clone();
            let image = view.shot.as_ref().map(|(_, _, image)| image.clone());
            let width_cell = self.pages.width.clone();
            // A imagem e a altura estimada vêm da largura real da coluna, não só da página viva.
            let measure = canvas(|_, _, _| {}, move |bounds, _, window, _| record_width(&width_cell, bounds, window)).absolute().inset_0();
            let bar = h_flex().gap_2()
                .child(div().flex_1().min_w_0().truncate().text_size(px(12.)).text_color(theme::muted()).child(title.clone()))
                .child(Button::new(SharedString::from(format!("page-open-{id}"))).ghost().xsmall().icon(IconName::ExternalLink)
                    .label(tr_shared("page_open_browser", &[]))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_page_in_browser(open.clone(), cx))))
                .children(full.then(|| fullscreen_button(&id, cx)))
                .child(toggle_button(&id, false, cx));
            let picture = match (view.state, image, view.no_image) {
                (_, _, true) => Some(note(tr_shared("page_no_image", &[]))),
                (_, Some(image), _) => Some(div().w_full().h(height).child(img(image).size_full().object_fit(ObjectFit::Contain))),
                _ => Some(div().w_full().h(height).child(loading())),
            };
            return v_flex().relative().w_full().gap_1().child(measure).children(picture).child(bar).into_any_element();
        }
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        {
            let follows = !view.page.own_theme && view.page.url.is_none();
            if let Some(running) = view.running.as_mut().filter(|r| follows && r.framed) { running.push_theme(); }
            let engine = view.running.as_ref().map(|r| r.engine.clone());
            let framed = view.running.as_ref().is_some_and(|r| r.framed);
            // O site tem altura fixa e rola por dentro: a roda é sempre dele.
            let inside = view.page.url.is_some() || scrolls_inside(&view.page, width, view.reported);
            let site = view.page.url.clone().map(|start| view.running.as_ref().and_then(|r| r.at.clone()).unwrap_or(start));
            let (paint, origin, width_cell, mark) = (self.pages.paint.clone(), view.origin.clone(), self.pages.width.clone(), id.clone());
            let shown = engine.clone();
            let surface = canvas(|bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal), move |bounds, hitbox, window, _| {
                origin.set(bounds.origin);
                record_width(&width_cell, bounds, window);
                paint.mark(&mark, shown.as_ref());
                let Some(engine) = shown else { return };
                engine.place(bounds, window);
                // Roda na fase de captura: antes da lista, que senão rolaria a conversa por cima da página.
                if inside {
                    window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                        if phase != DispatchPhase::Capture || !hitbox.should_handle_scroll(window) { return; }
                        let delta = event.delta.pixel_delta(px(16.));
                        engine.wheel(event.position - bounds.origin, point(-delta.x, -delta.y));
                        cx.stop_propagation();
                    });
                }
            }).absolute().inset_0();
            let cover = (!framed).then(|| match view.last_frame.clone() {
                // Tirado no tamanho em que saiu da tela: o cartão pode ter mudado desde então, e esticado ele deformava.
                Some(frame) => img(frame).size_full().object_fit(ObjectFit::Contain).into_any_element(),
                None => loading().into_any_element(),
            });
            let focused = view.focus.clone();
            let (down, up, up_out, moved, key_down, key_up) = (id.clone(), id.clone(), id.clone(), id.clone(), id.clone(), id.clone());
            let card = div().id(SharedString::from(format!("page-{id}"))).relative().w_full().h(height).overflow_hidden()
                .rounded(px(8.)).border_1().border_color(transparent_black())
                .track_focus(&view.focus).key_context("BrowserPage").aria_label(title.clone())
                .focus(|el| el.border_color(theme::accent_focus()))
                .child(surface)
                .children(cover)
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    focused.focus(window, cx);
                    let Some(view) = this.pages.views.get_mut(&down) else { return };
                    view.gesture = Some(Instant::now());
                    if view.parked { view.parked = false; cx.notify(); }
                    this.page_pointer(&down, Pointer::Down, event.position, event.click_count, &event.modifiers);
                }))
                .on_mouse_up(MouseButton::Left, cx.listener(move |this, event: &MouseUpEvent, _, _| this.page_pointer(&up, Pointer::Up, event.position, 0, &event.modifiers)))
                .on_mouse_up_out(MouseButton::Left, cx.listener(move |this, event: &MouseUpEvent, _, _| this.page_pointer(&up_out, Pointer::Up, event.position, 0, &event.modifiers)))
                .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, _| this.page_pointer(&moved, Pointer::Move, event.position, 0, &event.modifiers)))
                // A tecla é da página; Esc devolve o foco à conversa.
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                    cx.stop_propagation();
                    let k = &event.keystroke;
                    if k.key == "escape" { this.root_focus.focus(window, cx); return; }
                    if let Some(view) = this.pages.views.get_mut(&key_down) { view.gesture = Some(Instant::now()); }
                    if let Some(engine) = this.page_engine(&key_down) { forward_key(&engine, k, cx); }
                }))
                .on_key_up(cx.listener(move |this, event: &KeyUpEvent, _, cx| {
                    cx.stop_propagation();
                    if let Some(engine) = this.page_engine(&key_up) { engine.key(false, &event.keystroke); }
                }));
            let strip = match site {
                Some(at) => {
                    let panel = at.clone();
                    h_flex().gap_2()
                        .child(div().flex_1().min_w_0().truncate().text_size(px(12.)).text_color(theme::muted()).child(at))
                        .child(Button::new(SharedString::from(format!("page-panel-{id}"))).ghost().xsmall().icon(IconName::PanelRight)
                            .label(tr("page_open_panel"))
                            .on_click(cx.listener(move |this, _, window, cx| this.open_in_browser_panel(panel.clone(), window, cx))))
                }
                None => h_flex().gap_2().child(div().flex_1().min_w_0().truncate().text_size(px(12.)).text_color(theme::muted()).child(title)),
            };
            let strip = strip.children(full.then(|| fullscreen_button(&id, cx))).child(toggle_button(&id, false, cx));
            return v_flex().w_full().gap_1().child(strip).child(card).into_any_element();
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        div().w_full().h(height).child(loading()).into_any_element()
    }
}

fn fullscreen_button(id: &str, cx: &mut Context<Hangar>) -> Button {
    let tip = tr_shared("page_fullscreen", &[]);
    let target = id.to_owned();
    Button::new(SharedString::from(format!("page-full-{id}"))).ghost().xsmall().icon(IconName::Maximize)
        .tooltip(tip.clone()).accessibility_label(tip)
        .on_click(cx.listener(move |this, _, window, cx| this.open_page_fullscreen(&target, window, cx)))
}

fn toggle_button(id: &str, collapsed: bool, cx: &mut Context<Hangar>) -> Button {
    let tip = tr(if collapsed { "page_expand" } else { "page_collapse" });
    let target = id.to_owned();
    Button::new(SharedString::from(format!("page-toggle-{id}"))).ghost().xsmall()
        .icon(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown })
        .tooltip(tip.clone()).accessibility_label(tip)
        .on_click(cx.listener(move |this, _, _, cx| this.toggle_page(&target, cx)))
}

#[cfg(test)]
mod tests {
    // Sem `super::*`: o glob da gpui_kit traz um `test` próprio que sombreia o `#[test]` da std.
    use super::{Budget, HostMsg, Miss, PageRef, ViewState, expandable, frame_height, miss, page_failure, page_from_result, parse_host,
        scrolls_inside};

    #[test]
    fn answered_call_without_page_is_a_visible_failure() {
        let name = "mcp__hangar__html_render";
        assert_eq!(page_failure(name, "erro_paginas_indisponivel: OSError").as_deref(), Some("erro_paginas_indisponivel: OSError"));
        assert_eq!(page_failure(name, &"x".repeat(400)).unwrap().chars().count(), 301, "cortado com reticências");
        assert!(page_failure(name, r#"{"hangar_page":{"id":"a","title":"T"}}"#).is_none());
        assert!(page_failure(name, r#"{"draft":{"id":"b"}}"#).is_none(), "rascunho segue como chamada comum");
        assert!(page_failure("Read", "erro").is_none());
    }

    #[test]
    fn reads_only_hangar_page() {
        let ok = r#"{"hangar_page":{"id":"a","title":"T","height":null,"heights":{"728":300}},"message":"x"}"#;
        assert_eq!(page_from_result("mcp__hangar__html_render", ok).unwrap().id, "a");
        assert!(!page_from_result("mcp__hangar__html_render", ok).unwrap().own_theme, "página antiga segue o tema");
        let own = r#"{"hangar_page":{"id":"a","title":"T","own_theme":true}}"#;
        assert!(page_from_result("mcp__hangar__html_render", own).unwrap().own_theme);
        assert!(page_from_result("mcp__hangar__html_render", r#"{"draft":{"id":"b"}}"#).is_none());
        assert!(page_from_result("Read", ok).is_none());
    }

    #[test]
    fn reads_mcp_content_blocks() {
        let blocks = serde_json::json!([{"type": "text", "text": r#"{"hangar_page":{"id":"a","#}, {"type": "text", "text": r#""title":"T"}}"#}]).to_string();
        assert_eq!(page_from_result("mcp__hangar__html_render", &blocks).unwrap().title, "T");
    }

    #[test]
    fn reads_codex_call_tool_result() {
        let both = serde_json::json!({"content": [{"type": "text", "text": "outro"}],
            "structuredContent": {"hangar_page": {"id": "s", "title": "T"}}}).to_string();
        assert_eq!(page_from_result("mcp__hangar__html_render", &both).unwrap().id, "s");
        let text = serde_json::json!({"content": [{"type": "text", "text": r#"{"hangar_page":{"id":"a","#}, {"type": "text", "text": r#""title":"T"}}"#}]}).to_string();
        assert_eq!(page_from_result("hangar.html_render", &text).unwrap().id, "a");
        let err = serde_json::json!({"content": [{"type": "text", "text": "erro"}], "isError": true}).to_string();
        assert!(page_from_result("mcp__hangar__html_render", &err).is_none());
    }

    #[test]
    fn budget_evicts_oldest() {
        let mut b = Budget::default();
        for id in ["a", "b", "c", "d"] { assert_eq!(b.touch(id), None); }
        assert_eq!(b.touch("a"), None);
        assert_eq!(b.touch("e"), Some("b".into()));
    }

    #[test]
    fn pinned_fullscreen_takes_a_slot_until_released() {
        let mut b = Budget::default();
        for id in ["a", "b", "c", "d"] { assert_eq!(b.touch(id), None); }
        assert_eq!(b.pin(), vec!["a".to_owned()]);
        assert_eq!(b.touch("b"), None, "quem já está vivo não expulsa ninguém");
        assert_eq!(b.touch("e"), Some("c".into()));
        b.unpin();
        b.unpin();
        assert_eq!(b.touch("f"), None, "a vaga voltou, e soltar de novo não fica negativo");
    }

    #[test]
    fn clear_keeps_the_pinned_slot() {
        let mut b = Budget::default();
        assert!(b.pin().is_empty());
        b.clear();
        for id in ["a", "b", "c"] { assert_eq!(b.touch(id), None); }
        assert_eq!(b.touch("d"), Some("a".into()), "conversa recarregada com a tela cheia aberta: a vaga dela continua");
    }

    #[test]
    fn fullscreen_only_for_a_page_on_screen() {
        assert!(expandable(ViewState::Ready, true, false, false));
        assert!(expandable(ViewState::Ready, true, true, false), "site vivo abre no diálogo");
        assert!(expandable(ViewState::Ready, false, false, false), "imagem estática amplia");
        assert!(!expandable(ViewState::Ready, false, true, false), "site sem página viva fica com o navegador");
        assert!(!expandable(ViewState::Ready, false, false, true), "sem imagem não há o que ampliar");
        for state in [ViewState::Loading, ViewState::Error, ViewState::Expired] { assert!(!expandable(state, true, false, false)); }
    }

    #[test]
    fn page_failures_by_server_code() {
        assert_eq!(miss(Some(404), Some("erro_pagina_sem_imagem")), Miss::NoImage);
        assert_eq!(miss(Some(404), Some("erro_pagina_expirou")), Miss::Expired);
        assert_eq!(miss(Some(404), None), Miss::Other);
        assert_eq!(miss(None, Some("erro_pagina_expirou")), Miss::Other);
    }

    #[test]
    fn height_rules() {
        let p = PageRef { id: "a".into(), title: "T".into(), height: Some(300), heights: [(728, 380)].into(), own_theme: false, url: None };
        assert_eq!(frame_height(&p, 700., None), 300.);
        assert!(scrolls_inside(&p, 700., None));
        let free = PageRef { height: None, ..p.clone() };
        assert_eq!(frame_height(&free, 700., Some(5000.)), 2000.);
        assert!(scrolls_inside(&free, 700., Some(5000.)));
        assert!(!scrolls_inside(&free, 700., None));
        let site = PageRef { url: Some("http://localhost:3000/cidades".into()), height: None, ..p.clone() };
        assert_eq!(frame_height(&site, 700., Some(100.)), 640., "site tem moldura fixa");
        assert_eq!(frame_height(&PageRef { height: Some(900), ..site }, 700., None), 900.);
    }

    #[test]
    fn site_page_keeps_only_web_url() {
        let read = |url: &str| page_from_result("mcp__hangar__html_render",
            &serde_json::json!({"hangar_page": {"id": "a", "title": "T", "height": 640, "url": url}}).to_string()).unwrap().url;
        assert_eq!(read("http://localhost:3000/cidades").as_deref(), Some("http://localhost:3000/cidades"));
        assert_eq!(read("javascript:alert(1)"), None);
    }

    #[test]
    fn host_messages_are_filtered() {
        let size = |h: &str| format!(r#"{{"jsonrpc":"2.0","method":"ui/notifications/size-changed","params":{{"height":{h}}}}}"#);
        assert_eq!(parse_host(&size("320")), Some(HostMsg::Height(320.)));
        assert_eq!(parse_host(&size("-1")), None);
        assert_eq!(parse_host(&size("\"x\"")), None);
        let link = |u: &str| format!(r#"{{"method":"ui/open-link","params":{{"url":"{u}"}}}}"#);
        assert_eq!(parse_host(&link("https://a.dev/x")), Some(HostMsg::Link("https://a.dev/x".into(), false)));
        // Ctrl ou Cmd no clique: o link vai ao navegador do sistema, como no chat.
        let with = |m: &str| format!(r#"{{"method":"ui/open-link","params":{{"url":"https://a.dev/x","{m}":true}}}}"#);
        assert_eq!(parse_host(&with("ctrlKey")), Some(HostMsg::Link("https://a.dev/x".into(), true)));
        assert_eq!(parse_host(&with("metaKey")), Some(HostMsg::Link("https://a.dev/x".into(), true)));
        assert_eq!(parse_host(&link("javascript:alert(1)")), None);
        assert_eq!(parse_host(&link("file:///etc/passwd")), None);
        assert_eq!(parse_host(r#"{"method":"ui/other"}"#), None);
        assert_eq!(parse_host("not json"), None);
    }
}
