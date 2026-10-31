//! Verbos do hangar-preview sobre CDP, portados de `shell/preview_ctl.cjs`. Um controlador por navegador: é ele quem
//! guarda refs, tema, layout e os registros de console e rede entre um comando e outro.
use std::{cell::{Cell, RefCell}, collections::{HashMap, HashSet, VecDeque}, future::Future, time::Instant};

use futures::{future::{Either, select}, lock::Mutex};
use serde_json::{Value, json};

use super::preview_fmt::{Layout, SNAPSHOT_MAX, capped, compact_ax, key_event, mask, mask_forms, parse_layout};

pub trait Page {
    fn call(&self, method: &str, params: Value) -> impl Future<Output = Result<Value, String>>;
    fn sleep(&self, ms: u64) -> impl Future<Output = ()>;
    /// Tamanho e densidade em que o painel pôs a página por último; `None` onde a página já segue a janela (WebView2).
    fn panel_size(&self) -> Option<(u32, u32, f32)> { None }
    /// Layout pedido (`mobile`, `<l> <a>`) manda no tamanho: o painel para de aplicar o dele até voltar ao desktop.
    fn hold_size(&self, _held: bool) {}
    /// Trabalho pesado de CPU (decodificar e recodificar imagem) fora da thread de quem espera; sem executor, ali mesmo.
    fn background<T: Send + 'static>(&self, work: impl FnOnce() -> T + Send + 'static) -> impl Future<Output = T> { std::future::ready(work()) }
}

/// `fut`, ou `None` quando a página não responde em `ms`.
pub async fn within<P: Page, T>(page: &P, ms: u64, fut: impl Future<Output = T>) -> Option<T> {
    match select(std::pin::pin!(fut), std::pin::pin!(page.sleep(ms))).await { Either::Left((v, _)) => Some(v), Either::Right(_) => None }
}

/// A exceção que o script lançou (`exceptionDetails` de `Runtime.evaluate`/`callFunctionOn`), como o DevTools a descreve.
pub fn exception(r: &Value) -> Option<String> {
    let details = &r["exceptionDetails"];
    details["exception"]["description"].as_str().or(details["text"].as_str()).map(str::to_owned)
}

/// O valor que o script devolveu (`returnByValue`), ou a exceção dele.
pub fn returned(r: &Value) -> Result<&Value, String> {
    if r.get("exceptionDetails").is_some() { return Err(exception(r).unwrap_or_else(|| "excecao na pagina".into())); }
    Ok(&r["result"]["value"])
}

/// Os bytes do PNG de um `Page.captureScreenshot`.
pub fn screenshot_bytes(r: &Value) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(r["data"].as_str().unwrap_or("")).map_err(|e| e.to_string())
}

pub enum Reply { Text(String), Png(Vec<u8>) }

/// Agente do layout de desktop. O Chromium do Linux roda headless e se apresenta como "HeadlessChrome", que Google,
/// GitHub e afins recusam no login: vira o de um Chrome comum, com as marcas dos client hints, que a troca sem
/// metadados apagaria. Os outros motores voltam ao próprio agente (`""`).
fn desktop_agent(version: &Value) -> Value {
    let agent = version["userAgent"].as_str().unwrap_or_default();
    if !agent.contains("HeadlessChrome") { return json!({"userAgent": ""}); }
    let full = version["product"].as_str().unwrap_or_default().rsplit('/').next().unwrap_or_default();
    let major = full.split('.').next().unwrap_or_default();
    let brands = |v: &str, filler: &str| json!([{"brand": "Not;A=Brand", "version": filler},
        {"brand": "Chromium", "version": v}, {"brand": "Google Chrome", "version": v}]);
    json!({
        "userAgent": agent.replace("HeadlessChrome", "Chrome"),
        "userAgentMetadata": {"brands": brands(major, "99"), "fullVersionList": brands(full, "99.0.0.0"), "fullVersion": full,
            "platform": "Linux", "platformVersion": "", "architecture": "x86", "bitness": "64", "model": "", "mobile": false},
    })
}

/// O que o agente está fazendo agora, para a faixa do painel. `target` é o nome do alvo na árvore da página ou a tecla.
pub struct Activity { pub verb: String, pub target: Option<String> }

pub const EVENTS: [&str; 12] = ["Runtime.consoleAPICalled", "Runtime.exceptionThrown", "Log.entryAdded", "Network.requestWillBeSent",
    "Network.responseReceived", "Network.loadingFinished", "Network.loadingFailed", "Page.frameNavigated", "Page.navigatedWithinDocument",
    "Page.javascriptDialogOpening", "Page.javascriptDialogClosed", "Page.downloadWillBegin"];

const LOG_MAX: usize = 200;
const WAIT_MS: u64 = 15_000;
const WAIT_MAX_MS: u64 = 40_000;
const SHOT_MS: u64 = 15_000;
const POLL_MS: u64 = 120;
/// Escondida, a página fica sem tamanho; um viewport fixo a mantém desenhando para o `shot`.
const HIDDEN_VIEWPORT: (u32, u32) = (1280, 800);
const MOBILE_UA: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";
/// Dois quadros: só o segundo garante que o que a ação mudou já está pintado. O timer cobre página escondida.
const FRAME: &str = "new Promise(r=>{requestAnimationFrame(()=>requestAnimationFrame(r));setTimeout(r,100)})";
/// '' = dá para digitar; senão, o que está com foco. Desce o shadow DOM: lá o foco do documento é só o invólucro.
const FOCUS: &str = "(()=>{let a=document.activeElement;while(a&&a.shadowRoot&&a.shadowRoot.activeElement)a=a.shadowRoot.activeElement;if(!a||a===document.body)return 'nada';const t=a.tagName.toLowerCase();const ok=a.isContentEditable||t==='textarea'||(t==='input'&&!/^(button|submit|reset|checkbox|radio|file|image|range|color)$/i.test(a.type||''));return ok?'':t+(a.type?'['+a.type+']':'')})()";
const PROBE_READ: &str = "(typeof window.__hangarSonda===\"undefined\"?2:window.__hangarSonda)";
/// Lista nativa do `<select>` aberta: ela consome a tecla e o documento não vê o keydown.
const OPEN_LIST: &str = "(()=>{const a=document.activeElement;try{return !!a&&a.tagName==='SELECT'&&a.matches(':open')}catch(e){return false}})()";
/// Verbos que mudam a página: pausado, só estes são recusados; ler continua livre.
const ACTING: [&str; 13] = ["click", "fill", "fill-secret", "type", "press", "hover", "eval", "layout", "tema", "goto", "back", "reload", "dialog"];
/// Com diálogo aberto a página não roda script: só estes respondem sem ele.
const WITH_DIALOG: [&str; 3] = ["url", "estado", "console"];
pub const PAUSED: &str = "erro: o usuario pausou o navegador; espere ele clicar em Retomar no painel e tente de novo";
/// No modo "Escolher elemento" o Chromium toma o clique para a escolha: o do agente viraria um elemento na mensagem
/// da pessoa. A pessoa está no controle, como na pausa.
pub const PICKING: &str = "erro: o usuário está escolhendo um elemento no painel do navegador; espere ele terminar (ou cancelar com Esc) e tente de novo";
/// Espera da primeira ação depois de a faixa do agente ter sumido: ela volta e encolhe a página, e o alvo tem de ser
/// medido depois disso. Com a faixa já na tela, nenhuma espera.
const SETTLE_MS: u64 = 150;


struct State {
    refs: HashMap<String, i64>,
    console: VecDeque<String>,
    network: VecDeque<String>,
    /// Requisições sem resposta, por id: redirecionamento repete o id e não conta duas vezes.
    inflight: HashSet<String>,
    /// Método e endereço por id, até a requisição terminar: a falha (`loadingFailed`) não traz o endereço.
    requests: HashMap<String, (String, String)>,
    /// Diálogo aberto agora (tipo e texto), lido quando ele fecha.
    dialog: Option<(String, String)>,
    last_network: Instant,
    /// Endereços (sem a consulta) que a página tenta em laço sem servidor do outro lado: não contam para o `--idle`.
    noise: HashSet<String>,
    theme: &'static str,
    layout: Layout,
    hidden: bool,
    /// Escala da janela onde o painel está; `None` onde o motor já desenha na escala certa (WebView2).
    scale: Option<f32>,
    navigated: bool,
    /// Conta navegações do quadro principal (documento novo ou só o endereço): `goto`/`back`/`reload` esperam mudar.
    navs: u64,
    enabled: HashSet<&'static str>,
    /// Campos (`backendNodeId`) com senha do cofre: o snapshot não mostra o valor deles. Somem na navegação.
    marked: HashSet<i64>,
    /// Senhas já preenchidas, trocadas por `••••` em toda resposta enquanto o navegador viver (a página seguinte
    /// pode ecoá-las numa URL ou no console).
    secrets: Vec<Secret>,
}

/// Senha do cofre na memória do controlador, com as formas que a resposta troca por `••••` (preenchidas ao guardá-la):
/// tudo zerado ao sair. Sem `Debug` de propósito.
pub struct Secret { value: String, forms: Vec<String> }

impl Secret {
    pub fn new(value: String) -> Self { Self { value, forms: Vec::new() } }
    pub fn as_str(&self) -> &str { &self.value }
}

impl Drop for Secret {
    fn drop(&mut self) {
        for text in std::iter::once(&mut self.value).chain(self.forms.iter_mut()) {
            // SAFETY: zero é UTF-8 válido; a escrita volátil não é cortada pelo compilador.
            for byte in unsafe { text.as_bytes_mut() } { unsafe { std::ptr::write_volatile(byte, 0) } }
        }
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    }
}

/// Teto de senhas lembradas para a máscara.
const SECRETS_MAX: usize = 8;
/// O alvo é o elemento com foco na raiz dele (documento, shadow root ou iframe) e, se for, se é campo de senha.
const TARGET_FOCUS: &str = "function(){const r=this.getRootNode&&this.getRootNode();if(!r||r.activeElement!==this)return 'fora';return this.tagName==='INPUT'&&this.type==='password'?'senha':'campo'}";

pub struct Controller<P: Page> {
    page: P,
    state: RefCell<State>,
    /// Um verbo por vez: dois clientes (agente e tool MCP) não intercalam eventos.
    turn: Mutex<()>,
    /// Último `hidden` pedido: quem pega o `turn` aplica este, não o seu, porque a fila do `turn` não garante ordem.
    wanted_hidden: Cell<bool>,
    paused: Cell<bool>,
    /// A pessoa está escolhendo elemento no painel: os verbos de `ACTING` voltam com `PICKING`.
    picking: Cell<bool>,
    on_activity: RefCell<Option<Box<dyn Fn(Activity)>>>,
    on_layout: RefCell<Option<Box<dyn Fn(Option<(u32, u32)>)>>>,
    on_page_error: RefCell<Option<Box<dyn Fn(Option<String>)>>>,
    /// Diálogo aberto na página (tipo, mensagem), vindo do motor pelo painel.
    dialog_open: RefCell<Option<(String, String, String)>>,
    /// A faixa do agente está na tela (o painel avisa); o verbo que a faz aparecer espera `SETTLE_MS`.
    strip_shown: Cell<bool>,
    settle: Cell<bool>,
    /// Resposta mandada, fechamento ainda não visto: o próximo verbo espera um instante por ele.
    dialog_answered: Cell<bool>,
    /// Acorda o verbo em andamento quando um diálogo abre: o clique que o abriu só voltaria depois da resposta.
    dialog_signal: RefCell<Option<futures::channel::oneshot::Sender<()>>>,
    on_dialog_answer: RefCell<Option<Box<dyn Fn(bool, Option<String>, futures::channel::oneshot::Sender<Result<(), String>>)>>>,
}

fn push(list: &mut VecDeque<String>, line: String) {
    list.push_back(line);
    if list.len() > LOG_MAX { list.pop_front(); }
}

impl State {
    /// Fora da espera do `--idle`: o laço de quem não tem servidor, o overlay de erro do Next em dev e a exportação de
    /// telemetria (OpenTelemetry), que sai sozinha em intervalo e não muda a tela.
    fn ignored(&self, url: &str) -> bool {
        let base = without_query(url);
        self.noise.contains(base) || base.contains("/__nextjs_") || ["/v1/traces", "/v1/metrics", "/v1/logs"].iter().any(|e| base.ends_with(e) && base.contains("otlp"))
    }
}

/// Teto de endereços em laço lembrados: além disso, voltam a contar para o `--idle`.
const NOISE_MAX: usize = 64;

fn without_query(url: &str) -> &str { url.split(['?', '#']).next().unwrap_or(url) }

impl<P: Page> Controller<P> {
    pub fn new(page: P, hidden: bool) -> Self {
        Self { page, turn: Mutex::new(()), wanted_hidden: Cell::new(hidden), paused: Cell::new(false), picking: Cell::new(false), on_activity: RefCell::new(None), on_layout: RefCell::new(None), on_page_error: RefCell::new(None), dialog_open: RefCell::new(None), strip_shown: Cell::new(false), settle: Cell::new(false), dialog_answered: Cell::new(false), dialog_signal: RefCell::new(None), on_dialog_answer: RefCell::new(None), state: RefCell::new(State { refs: HashMap::new(), console: VecDeque::new(),
            network: VecDeque::new(), inflight: HashSet::new(), requests: HashMap::new(), dialog: None, last_network: Instant::now(), noise: HashSet::new(), theme: "sistema", layout: Layout::Desktop,
            hidden: false, scale: None, navigated: false, navs: 0, enabled: HashSet::new(), marked: HashSet::new(), secrets: Vec::new() }) }
    }

    /// Algum verbo segurando a vez agora (um `wait` longo, por exemplo).
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub fn busy(&self) -> bool { self.turn.try_lock().is_none() }

    /// Aplica o pedido mais novo, não o do nascimento: o painel pode aparecer enquanto os domínios ligam.
    pub async fn start(&self) {
        self.enable_base().await;
        self.set_hidden(self.wanted_hidden.get()).await;
    }

    /// Sem Runtime/Log/Page o console e as refs morrem calados; `run` tenta de novo a cada verbo (já ligado não chama o CDP).
    async fn enable_base(&self) {
        for domain in ["Runtime", "Log", "Page"] {
            if let Err(e) = self.enable(domain).await { eprintln!("[nav] {e}"); }
        }
    }

    pub async fn set_hidden(&self, hidden: bool) {
        self.wanted_hidden.set(hidden);
        let _turn = self.turn.lock().await;
        self.state.borrow_mut().hidden = self.wanted_hidden.get();
        let layout = self.state.borrow().layout;
        if let Err(e) = self.apply_layout(layout).await { eprintln!("[nav] layout nao aplicado: {e}"); }
    }

    /// A página caiu e voltou no mesmo alvo: os domínios ligados morreram com ela, e o próximo verbo os liga de novo.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub fn reset_domains(&self) { self.state.borrow_mut().enabled.clear(); }

    /// Pausado pelo usuário: os verbos de `ACTING` voltam com `PAUSED` até ele retomar.
    pub fn set_paused(&self, paused: bool) { self.paused.set(paused); }

    /// Modo "Escolher elemento" ligado no painel.
    pub fn set_picking(&self, picking: bool) { self.picking.set(picking); }

    /// Por que um verbo que mexe na página não pode seguir agora.
    fn blocked(&self) -> Option<&'static str> {
        if self.paused.get() { Some(PAUSED) } else if self.picking.get() { Some(PICKING) } else { None }
    }

    pub fn on_activity(&self, f: impl Fn(Activity) + 'static) { *self.on_activity.borrow_mut() = Some(Box::new(f)); }

    /// Tela emulada (`layout mobile` ou `<l> <a>`), para o painel desenhá-la na proporção certa; `None` = a do painel.
    pub fn on_layout(&self, f: impl Fn(Option<(u32, u32)>) + 'static) { *self.on_layout.borrow_mut() = Some(Box::new(f)); }

    pub fn set_strip_shown(&self, shown: bool) { self.strip_shown.set(shown); }

    /// O motor viu um diálogo abrir (`Some`) ou fechar (`None`).
    /// `(tipo, mensagem, texto sugerido do prompt)`.
    pub fn set_dialog(&self, dialog: Option<(String, String, String)>) {
        let opened = dialog.is_some();
        *self.dialog_open.borrow_mut() = dialog;
        self.dialog_answered.set(false);
        if opened && let Some(signal) = self.dialog_signal.borrow_mut().take() { let _ = signal.send(()); }
    }

    /// Quem responde o diálogo de verdade (o motor, pelo painel): o verbo `dialog` só repassa.
    pub fn on_dialog_answer(&self, f: impl Fn(bool, Option<String>, futures::channel::oneshot::Sender<Result<(), String>>) + 'static) {
        *self.on_dialog_answer.borrow_mut() = Some(Box::new(f));
    }

    fn dialog_hint(kind: &str) -> &'static str {
        if kind == "prompt" { "dialog accept <texto> | dialog accept | dialog dismiss" } else if kind == "alert" { "dialog accept" } else { "dialog accept | dialog dismiss" }
    }

    async fn answer_dialog(&self, args: &[String]) -> String {
        let Some((kind, _, default)) = self.dialog_open.borrow().clone() else { return "erro: dialog: nenhum dialogo aberto na pagina".into() };
        let accept = match args.first().map(String::as_str) {
            Some("accept" | "aceitar" | "ok") => true,
            Some("dismiss" | "recusar" | "cancel") => false,
            _ => return format!("erro: dialog: use {}", Self::dialog_hint(&kind)),
        };
        // Sem texto, o sugerido pela página, como no OK da caixa do painel.
        let text = (accept && kind == "prompt").then(|| if args.len() > 1 { args[1..].join(" ") } else { default });
        // O gancho só põe a resposta no canal do painel: chamá-lo com o empréstimo aberto não reentra.
        // O diálogo só some com o fechamento visto pelo motor: resposta que falhou deixa ele aberto para outra tentativa.
        let (done, result) = futures::channel::oneshot::channel();
        match self.on_dialog_answer.borrow().as_ref() { Some(f) => f(accept, text, done), None => return "erro: dialog: o painel nao respondeu".into() }
        // O ok só depois de o motor confirmar: aceite que falhou vira recusa na página, e o agente precisa saber.
        match within(&self.page, 5000, result).await {
            Some(Ok(Ok(()))) => {
                // Só com a resposta aceita o próximo verbo espera o fechamento; com erro o diálogo segue aberto.
                self.dialog_answered.set(true);
                format!("ok: dialog {}", if accept { "accept" } else { "dismiss" })
            }
            Some(Ok(Err(e))) => format!("erro: dialog: a pagina nao recebeu a resposta: {e}"),
            Some(Err(_)) => "erro: dialog: o painel do navegador fechou antes de responder".into(),
            None => "erro: dialog: o navegador nao confirmou a resposta em 5 s".into(),
        }
    }

    /// Erro da página para o selo do painel (`Some`), ou `None` quando ela navega e a contagem recomeça.
    pub fn on_page_error(&self, f: impl Fn(Option<String>) + 'static) { *self.on_page_error.borrow_mut() = Some(Box::new(f)); }

    fn report(&self, verb: &str, target: Option<String>) {
        if let Some(f) = self.on_activity.borrow().as_ref() { f(Activity { verb: verb.into(), target }); }
    }

    /// O painel apareceu numa janela com esta escala: a página desktop passa a desenhar nela, sem mexer no tamanho.
    pub async fn set_scale(&self, scale: f32) {
        let _turn = self.turn.lock().await;
        let (layout, hidden) = { let mut s = self.state.borrow_mut(); s.scale = Some(scale); (s.layout, s.hidden) };
        if layout == Layout::Desktop && !hidden && let Err(e) = self.apply_layout(layout).await { eprintln!("[nav] escala nao aplicada: {e}"); }
    }

    pub fn on_event(&self, method: &str, p: &Value) {
        self.log_event(method, p);
        // Depois do registro, com o estado solto: quem ouve pode pedir algo ao controlador.
        let error = match method {
            "Runtime.consoleAPICalled" if matches!(p["type"].as_str(), Some("error" | "assert")) => self.state.borrow().console.back().cloned().map(Some),
            "Runtime.exceptionThrown" => self.state.borrow().console.back().cloned().map(Some),
            // A linha do registro não leva o endereço: para a pessoa, "falhou ao carregar" sem saber o quê não ajuda.
            "Log.entryAdded" if p["entry"]["level"] == "error" => Some(Some(match p["entry"]["url"].as_str().filter(|u| !u.is_empty()) {
                Some(url) => format!("{} — {url}", p["entry"]["text"].as_str().unwrap_or("")),
                None => p["entry"]["text"].as_str().unwrap_or("").to_owned(),
            })),
            "Page.frameNavigated" if p["frame"]["parentId"].is_null() => Some(None),
            _ => None,
        };
        if let (Some(error), Some(f)) = (error, self.on_page_error.borrow().as_ref()) { f(error); }
    }

    fn log_event(&self, method: &str, p: &Value) {
        let mut s = self.state.borrow_mut();
        match method {
            "Runtime.consoleAPICalled" => {
                let args: Vec<String> = p["args"].as_array().into_iter().flatten().map(remote_text).collect();
                push(&mut s.console, format!("{}: {}", p["type"].as_str().unwrap_or("log"), args.join(" ")));
            }
            "Runtime.exceptionThrown" => push(&mut s.console, exception_line(&p["exceptionDetails"])),
            "Log.entryAdded" => push(&mut s.console, format!("{}: {}", p["entry"]["level"].as_str().unwrap_or(""), p["entry"]["text"].as_str().unwrap_or(""))),
            // Extensão do perfil não é a página: fica fora do registro e da espera do `--idle`.
            "Network.requestWillBeSent" if p["request"]["url"].as_str().is_some_and(|u| u.starts_with("chrome-extension:")) => {}
            "Network.requestWillBeSent" => {
                let id = p["requestId"].as_str().unwrap_or("").to_owned();
                // Teto contra requisição que nunca termina (stream, socket): o mapa só serve à linha de falha.
                if s.requests.len() > 500 { s.requests.clear(); }
                let url = p["request"]["url"].as_str().unwrap_or("").to_owned();
                if !s.ignored(&url) { s.inflight.insert(id.clone()); }
                s.requests.insert(id, (p["request"]["method"].as_str().unwrap_or("GET").to_owned(), url));
            }
            "Network.responseReceived" if p["response"]["url"].as_str().is_some_and(|u| u.starts_with("chrome-extension:")) => {}
            "Network.responseReceived" => {
                // A resposta encerra a espera do `--idle` mesmo em stream (SSE), que nunca termina de carregar.
                if let Some(id) = p["requestId"].as_str() { s.inflight.remove(id); }
                let url = p["response"]["url"].as_str().unwrap_or("");
                // Respondeu: o servidor subiu, e o endereço volta a contar a partir da próxima.
                let revived = s.noise.remove(without_query(url));
                if !revived && !s.ignored(url) { s.last_network = Instant::now(); }
                let method = p["requestId"].as_str().and_then(|id| s.requests.get(id)).map_or("GET".to_owned(), |(m, _)| m.clone());
                push(&mut s.network, format!("{method} {} {}", p["response"]["status"], p["response"]["url"].as_str().unwrap_or("")));
            }
            "Network.loadingFinished" => { if let Some(id) = p["requestId"].as_str() { s.requests.remove(id); } }
            "Network.loadingFailed" => {
                let id = p["requestId"].as_str().unwrap_or("");
                s.inflight.remove(id);
                let (method, url) = s.requests.remove(id).unwrap_or_default();
                // Conexão recusada = nada escutando (SignalR, HMR sem servidor): a página tenta de novo em laço.
                if p["errorText"].as_str().is_some_and(|e| e.ends_with("ERR_CONNECTION_REFUSED")) && s.noise.len() < NOISE_MAX {
                    s.noise.insert(without_query(&url).to_owned());
                }
                if !s.ignored(&url) { s.last_network = Instant::now(); }
                let reason = p["blockedReason"].as_str().map(|r| format!("bloqueada: {r}"))
                    .unwrap_or_else(|| p["errorText"].as_str().unwrap_or("").to_owned());
                let canceled = if p["canceled"] == true { " (cancelada)" } else { "" };
                push(&mut s.network, format!("{method} FALHOU {url}: {reason}{canceled}"));
            }
            "Page.javascriptDialogOpening" => {
                let message: String = p["message"].as_str().unwrap_or("").chars().take(200).collect();
                s.dialog = Some((p["type"].as_str().unwrap_or("dialog").to_owned(), message));
            }
            "Page.javascriptDialogClosed" => {
                let (kind, message) = s.dialog.take().unwrap_or_default();
                let outcome = if p["result"] == true { "aceito" } else { "recusado" };
                push(&mut s.console, format!("dialogo {kind} {outcome}: {message}"));
            }
            "Page.downloadWillBegin" => {
                let outcome = if cfg!(target_os = "linux") { "recusado (o navegador embutido nao baixa arquivos)" } else { "pedido" };
                push(&mut s.console, format!("download {outcome}: {} {}", p["suggestedFilename"].as_str().unwrap_or(""), p["url"].as_str().unwrap_or("")));
            }
            // Só o quadro principal: iframe navegando não invalida as refs da página.
            "Page.frameNavigated" if p["frame"]["parentId"].is_null() => { s.navigated = true; s.navs += 1; }
            "Page.navigatedWithinDocument" => s.navs += 1,
            _ => {}
        }
    }

    pub async fn run(&self, verb: &str, args: &[String]) -> Reply { self.run_with(verb, args, None).await }

    /// `secret`: o valor do `fill-secret`, que nunca vem nos `args`. Toda resposta de texto sai com as senhas já
    /// preenchidas trocadas por `••••`.
    pub async fn run_with(&self, verb: &str, args: &[String], secret: Option<Secret>) -> Reply {
        match self.run_inner(verb, args, secret).await {
            Reply::Text(text) => Reply::Text(self.masked(text)),
            png => png,
        }
    }

    fn masked(&self, text: String) -> String {
        self.state.borrow().secrets.iter().fold(text, |out, secret| mask(&out, &secret.forms))
    }

    async fn run_inner(&self, verb: &str, args: &[String], mut secret: Option<Secret>) -> Reply {
        if let Some(why) = self.blocked().filter(|_| ACTING.contains(&verb)) { return Reply::Text(why.into()); }
        // Antes da vez: o verbo que segura a vez pode estar parado justamente no diálogo.
        if verb == "dialog" { return Reply::Text(self.answer_dialog(args).await); }
        let _turn = self.turn.lock().await;
        if let Some(why) = self.blocked().filter(|_| ACTING.contains(&verb)) { return Reply::Text(why.into()); }
        for _ in 0..20 {
            if !(self.dialog_answered.get() && self.dialog_open.borrow().is_some()) { break; }
            self.page.sleep(50).await;
        }
        let open = self.dialog_open.borrow().is_some();
        if let Some((kind, message, _)) = self.dialog_open.borrow().clone() && !WITH_DIALOG.contains(&verb) {
            return Reply::Text(format!("erro: a pagina esta com um dialogo {kind} aberto: “{message}”. Responda antes: {}", Self::dialog_hint(&kind)));
        }
        let (signal, opened) = futures::channel::oneshot::channel();
        *self.dialog_signal.borrow_mut() = Some(signal);
        let arg = |i: usize| args.get(i).map(String::as_str).unwrap_or("");
        // Anuncia antes de medir: a faixa que aparece muda a altura da página. Verbo com ref repete com o nome do alvo.
        if verb != "estado" {
            self.settle.set(!self.strip_shown.get());
            self.report(verb, (verb == "press").then(|| arg(0).to_owned()));
        }
        let work = async {
        // Com diálogo aberto a página não responde: nada que vá a ela antes do verbo.
        if !open {
            self.enable_base().await;
            self.after_navigation().await;
        }
        match verb {
            "snapshot" => self.snapshot_verb(args).await.map(Reply::Text),
            "click" => self.click(arg(0)).await.map(Reply::Text),
            "fill" => self.fill(arg(0), arg(1)).await.map(Reply::Text),
            "fill-secret" => self.fill_secret(arg(0), arg(1), secret.take()).await.map(Reply::Text),
            "type" => self.type_text(arg(0)).await.map(Reply::Text),
            "press" => self.press(arg(0)).await.map(Reply::Text),
            "hover" => self.hover(arg(0)).await.map(Reply::Text),
            "wait" => self.wait(args).await.map(Reply::Text),
            "eval" => self.eval(arg(0)).await.map(Reply::Text),
            // Pelo histórico do navegador, não por JS: responde mesmo com a página presa (diálogo aberto, laço de script).
            "url" => self.current_url().await.map(Reply::Text),
            "estado" => Ok(Reply::Text(if self.paused.get() { "pausado".into() } else { "livre".into() })),
            "text" => self.value("document.body.innerText").await.map(|v| Reply::Text(v.as_str().unwrap_or("").to_owned())),
            "tema" => self.theme(arg(0)).await.map(Reply::Text),
            "layout" => self.layout(args).await.map(Reply::Text),
            "console" => Ok(Reply::Text(self.console(arg(0) == "--limpar"))),
            "network" => self.network().await.map(Reply::Text),
            "shot" => self.shot(args).await,
            "goto" => self.goto(arg(0)).await.map(Reply::Text),
            "back" => self.back().await.map(Reply::Text),
            "reload" => self.reload().await.map(Reply::Text),
            "tab-list" | "tab-new" | "tab-switch" | "tab-close" => Ok(Reply::Text("erro: o app nativo ainda nao tem abas: e um navegador por sessao".into())),
            _ => Ok(Reply::Text(format!("erro: verbo desconhecido: {verb}"))),
        }
        };
        // Diálogo aberto no meio do verbo (o clique em "Excluir"): a página para até a resposta, e o verbo volta agora.
        let opened = async { if opened.await.is_err() { std::future::pending::<()>().await } };
        let result = match select(std::pin::pin!(work), std::pin::pin!(opened)).await {
            Either::Left((result, _)) => result,
            Either::Right(_) => {
                let _ = within(&self.page, 500, self.unmark()).await;
                let (kind, message, _) = self.dialog_open.borrow().clone().unwrap_or_default();
                Ok(Reply::Text(format!("ok: {verb} parou: a pagina abriu um dialogo {kind}: “{message}”. Responda com: {}", Self::dialog_hint(&kind))))
            }
        };
        self.dialog_signal.borrow_mut().take();
        result.unwrap_or_else(|e| Reply::Text(format!("erro: {e}")))
    }

    async fn after_navigation(&self) {
        if !std::mem::take(&mut self.state.borrow_mut().navigated) { return; }
        {
            let mut s = self.state.borrow_mut();
            s.refs.clear();
            s.marked.clear();
            // Requisição cancelada pela navegação nem sempre vira loadingFailed.
            s.inflight.clear();
        }
        // O tema emulado se perde ao navegar; o tamanho não.
        if self.state.borrow().theme != "sistema" && let Err(e) = self.apply_theme().await { eprintln!("[nav] tema nao reaplicado: {e}"); }
    }

    async fn enable(&self, domain: &'static str) -> Result<(), String> {
        if self.state.borrow().enabled.contains(domain) { return Ok(()); }
        self.page.call(&format!("{domain}.enable"), json!({})).await.map_err(|e| format!("{domain}.enable falhou: {e}"))?;
        let mut s = self.state.borrow_mut();
        s.enabled.insert(domain);
        if domain == "Network" { s.last_network = Instant::now(); }
        Ok(())
    }

    async fn frame(&self) {
        let _ = within(&self.page, 500, self.page.call("Runtime.evaluate", json!({"expression": FRAME, "awaitPromise": true}))).await;
    }

    async fn value(&self, expression: &str) -> Result<Value, String> {
        let r = self.page.call("Runtime.evaluate", json!({"expression": expression, "returnByValue": true})).await?;
        Ok(r["result"]["value"].clone())
    }

    /// Árvore inteira (`wait @eN` usa só as refs).
    async fn current_url(&self) -> Result<String, String> {
        let h = self.page.call("Page.getNavigationHistory", json!({})).await?;
        let index = h["currentIndex"].as_u64().unwrap_or(0) as usize;
        match h["entries"].get(index).and_then(|e| e["url"].as_str()) {
            Some(url) => Ok(format!("ok: {}", Value::String(url.to_owned()))),
            None => self.eval("location.href").await,
        }
    }

    async fn snapshot(&self) -> Result<Vec<String>, String> { self.tree(false).await }

    async fn tree(&self, refs_only: bool) -> Result<Vec<String>, String> {
        self.enable("Accessibility").await?;
        let mut tree = self.page.call("Accessibility.getFullAXTree", json!({})).await?;
        let mut nodes = tree["nodes"].take();
        {
            let marked = &self.state.borrow().marked;
            for node in nodes.as_array_mut().into_iter().flatten() {
                if node["backendDOMNodeId"].as_i64().is_some_and(|id| marked.contains(&id)) { node["value"] = Value::Null; }
            }
        }
        let out = compact_ax(nodes.as_array().map_or(&[][..], Vec::as_slice), refs_only);
        self.state.borrow_mut().refs = out.refs;
        Ok(out.lines)
    }

    /// `snapshot [--refs] [--max <caracteres>]`.
    async fn snapshot_verb(&self, args: &[String]) -> Result<String, String> {
        let refs_only = args.iter().any(|a| a == "--refs");
        let max = match args.iter().position(|a| a == "--max") {
            None => SNAPSHOT_MAX,
            Some(i) => match args.get(i + 1).and_then(|v| v.parse::<usize>().ok()).filter(|m| *m >= 1000) {
                Some(m) => m,
                None => return Ok("erro: snapshot --max precisa de um numero de caracteres (1000 ou mais)".into()),
            },
        };
        Ok(capped(&self.tree(refs_only).await?, max))
    }

    /// Ref velha e ref inexistente dão no mesmo lugar: o nó morre em qualquer re-render, e a saída é o mesmo snapshot novo.
    /// Outra falha do CDP (página travada, sessão caída) não é ref velha: sobe como erro, porque snapshot novo não resolve.
    async fn center(&self, r: &str) -> Result<Option<(i64, i64)>, String> {
        let Some(id) = self.state.borrow().refs.get(r).copied() else { return Ok(None) };
        let _ = self.page.call("DOM.scrollIntoViewIfNeeded", json!({"backendNodeId": id})).await;
        let model = match self.page.call("DOM.getBoxModel", json!({"backendNodeId": id})).await {
            Ok(m) => m,
            Err(e) if e.contains("No node") => return Ok(None),
            Err(e) => return Err(format!("{r}: {e}")),
        };
        let Some(content) = model["model"]["content"].as_array() else { return Ok(None) };
        let q: Vec<f64> = content.iter().filter_map(Value::as_f64).collect();
        Ok((q.len() >= 6).then(|| (((q[0] + q[4]) / 2.).round() as i64, ((q[1] + q[5]) / 2.).round() as i64)))
    }

    /// Anuncia a ação com o nome do alvo e, com a página à vista, realça o alvo sem esperar: o contorno fica até meio
    /// segundo depois da ação (o painel o apaga). O realce é do próprio Chromium (fora do DOM): não entra no snapshot
    /// nem segura o clique, e no Windows aparece dentro do WebView2. Roda antes de `center`.
    async fn mark(&self, verb: &str, r: &str) {
        let Some(id) = self.state.borrow().refs.get(r).copied() else { return };
        let _ = self.page.call("DOM.scrollIntoViewIfNeeded", json!({"backendNodeId": id})).await;
        let name = self.page.call("Accessibility.getPartialAXTree", json!({"backendNodeId": id, "fetchRelatives": false})).await.ok()
            .and_then(|t| t["nodes"].as_array()?.iter().find_map(|n| n["name"]["value"].as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)));
        self.report(verb, name);
        if self.state.borrow().hidden { return; }
        let color = |a: f64| json!({"r": 124, "g": 135, "b": 232, "a": a});
        let shown = async {
            self.enable("DOM").await?;
            self.enable("Overlay").await?;
            self.page.call("Overlay.highlightNode", json!({"backendNodeId": id,
                "highlightConfig": {"borderColor": color(1.), "contentColor": color(0.18), "showInfo": false}})).await
        };
        match shown.await {
            Ok(_) if self.settle.get() => self.page.sleep(SETTLE_MS).await,
            Ok(_) => {}
            Err(e) => eprintln!("[nav] realce do alvo falhou: {e}"),
        }
    }

    /// `center` depois do realce: sem alvo medido, o realce sai antes de o erro subir.
    async fn measured(&self, r: &str) -> Result<Option<(i64, i64)>, String> {
        let c = self.center(r).await;
        if !matches!(c, Ok(Some(_))) { self.unmark().await; }
        c
    }

    async fn unmark(&self) {
        if self.state.borrow().enabled.contains("Overlay") { let _ = self.page.call("Overlay.hideHighlight", json!({})).await; }
    }

    /// Ouvinte em captura prova que o EVENTO chegou ao documento. `pointerdown` antes de `mousedown`: um
    /// `preventDefault` no primeiro suprime o segundo (combobox do Radix).
    async fn arrived(&self, types: &[&str], fire: impl AsyncFn() -> Result<(), String>) -> Result<bool, String> {
        let arm = format!("(()=>{{window.__hangarSonda=0;const ts={};const f=()=>{{window.__hangarSonda=1;ts.forEach(t=>removeEventListener(t,f,true))}};ts.forEach(t=>addEventListener(t,f,true));return 1}})()",
            serde_json::to_string(types).unwrap_or_default());
        self.page.call("Runtime.evaluate", json!({"expression": arm})).await.map_err(|e| format!("sonda nao armada: {e}"))?;
        fire().await?;
        self.frame().await;
        // Marcador sumido (2) ou contexto da sonda destruído = documento novo: o evento navegou, logo chegou.
        // Outra falha de leitura não prova entrega.
        match self.value(PROBE_READ).await {
            Ok(v) => Ok(matches!(v.as_i64(), Some(1 | 2))),
            Err(e) if e.contains("Execution context was destroyed") || e.contains("Cannot find default execution context") => Ok(true),
            Err(e) => { eprintln!("[nav] sonda nao lida: {e}"); Ok(false) }
        }
    }

    async fn click(&self, r: &str) -> Result<String, String> { self.click_as("click", r).await }

    async fn click_as(&self, verb: &str, r: &str) -> Result<String, String> {
        self.mark(verb, r).await;
        if let Some(why) = self.blocked() { self.unmark().await; return Ok(why.into()); }
        let Some((x, y)) = self.measured(r).await? else { return Ok(format!("erro: ref {r} nao existe (rode snapshot de novo)")) };
        // A medida esperou a página: o modo de escolha pode ter ligado nesse meio-tempo.
        if let Some(why) = self.blocked() { self.unmark().await; return Ok(why.into()); }
        let base = |t: &str| json!({"type": t, "x": x, "y": y, "button": "left", "clickCount": 1});
        let ok = self.arrived(&["pointerdown", "mousedown"], async || {
            self.page.call("Input.dispatchMouseEvent", base("mousePressed")).await?;
            self.page.call("Input.dispatchMouseEvent", base("mouseReleased")).await.map(|_| ())
        }).await;
        let ok = ok?;
        Ok(if ok { format!("ok: click {r}") } else { format!("erro: click {r}: o evento nao chegou na pagina (aba escondida sem quadro) — tire um shot e repita") })
    }

    async fn not_editable(&self) -> Result<Option<String>, String> {
        let v = self.value(FOCUS).await?;
        Ok(match v.as_str() { Some("") => None, Some(t) => Some(t.to_owned()), None => Some("desconhecido".into()) })
    }

    async fn fill(&self, r: &str, text: &str) -> Result<String, String> {
        let clicked = self.click_as("fill", r).await?;
        if clicked.starts_with("erro:") { return Ok(clicked); }
        // Assumir durante o clique: a pessoa pode já estar digitando no campo.
        if let Some(why) = self.blocked() { return Ok(why.into()); }
        if let Some(focus) = self.not_editable().await? {
            return Ok(format!("erro: fill {r}: o clique nao deixou um campo de texto com foco (foco em {focus}) — a ref e mesmo um campo? rode snapshot"));
        }
        // `commands` seleciona o campo inteiro; insertText substitui a seleção, vazio incluído.
        self.page.call("Input.dispatchKeyEvent", json!({"type": "keyDown", "commands": ["selectAll"]})).await?;
        self.page.call("Input.insertText", json!({"text": text})).await?;
        self.frame().await;
        Ok(format!("ok: fill {r}"))
    }

    /// `fill-secret @eN username|password`, com o valor do cofre fora dos args. O valor só entra no próprio alvo com
    /// foco, e a senha só em `input[type=password]`: digitada noutro campo ela apareceria na tela e no snapshot.
    async fn fill_secret(&self, r: &str, kind: &str, secret: Option<Secret>) -> Result<String, String> {
        let Some(secret) = secret.filter(|s| !s.as_str().is_empty()) else {
            return Ok("erro: fill-secret: so pela tool vault_fill, com o valor no corpo do pedido".into());
        };
        let password = match kind {
            "password" => true,
            "username" => false,
            _ => return Ok("erro: fill-secret: o campo e username ou password".into()),
        };
        let clicked = self.click_as("fill-secret", r).await?;
        if clicked.starts_with("erro:") { return Ok(clicked); }
        if let Some(why) = self.blocked() { return Ok(why.into()); }
        if let Some(focus) = self.not_editable().await? {
            return Ok(format!("erro: fill-secret {r}: o clique nao deixou um campo de texto com foco (foco em {focus}) — rode snapshot"));
        }
        let Some(id) = self.state.borrow().refs.get(r).copied() else { return Ok(format!("erro: ref {r} nao existe (rode snapshot de novo)")) };
        self.enable("DOM").await?;
        let node = self.page.call("DOM.resolveNode", json!({"backendNodeId": id})).await?;
        let object = node["object"]["objectId"].as_str().ok_or("fill-secret: o alvo nao tem objeto na pagina")?;
        let found = self.page.call("Runtime.callFunctionOn", json!({"objectId": object, "functionDeclaration": TARGET_FOCUS, "returnByValue": true})).await?;
        match returned(&found)?.as_str() {
            Some("senha") => {}
            Some("campo") if !password => {}
            Some("campo") => return Ok(format!("erro: erro_cofre_campo_nao_senha: {r} nao e um campo de senha (input type=password)")),
            _ => return Ok(format!("erro: fill-secret {r}: o foco nao ficou no campo da ref — rode snapshot")),
        }
        self.page.call("Input.dispatchKeyEvent", json!({"type": "keyDown", "commands": ["selectAll"]})).await?;
        // shortcut: a cópia do valor no JSON do CDP não é zerada; trocar se o transporte passar a guardar os pedidos.
        let insert = json!({"text": secret.as_str()});
        // Registrada antes de digitar: se o envio falhar depois de o texto chegar à página, o snapshot já a esconde.
        if password {
            let mut s = self.state.borrow_mut();
            s.marked.insert(id);
            if !s.secrets.iter().any(|known| known.as_str() == secret.as_str()) {
                if s.secrets.len() >= SECRETS_MAX {
                    s.secrets.remove(0);
                    crate::log_line("cofre: o navegador deixou de esconder a senha preenchida mais antiga (limite de senhas por navegador)");
                }
                let mut secret = secret;
                secret.forms = mask_forms(secret.as_str());
                s.secrets.push(secret);
            }
        }
        self.page.call("Input.insertText", insert).await?;
        self.frame().await;
        Ok(format!("ok: fill-secret {r}"))
    }

    async fn type_text(&self, text: &str) -> Result<String, String> {
        if let Some(focus) = self.not_editable().await? {
            return Ok(format!("erro: type: nenhum campo de texto com foco (foco em {focus}) — use fill @eN, ou click no campo antes"));
        }
        self.page.call("Input.insertText", json!({"text": text})).await?;
        self.frame().await;
        Ok("ok: type".into())
    }

    async fn press(&self, key: &str) -> Result<String, String> {
        let down = key_event(key);
        let mut up = down.clone();
        if let Some(o) = up.as_object_mut() { o.remove("text"); o.remove("unmodifiedText"); }
        // Aba escondida engole a tecla mesmo com a lista aberta: lá a sonda continua sendo a prova.
        let hidden = self.state.borrow().hidden;
        let open_list = !hidden && self.value(OPEN_LIST).await.is_ok_and(|v| v == true);
        let ok = self.arrived(&["keydown"], async || {
            let mut d = down.clone();
            d["type"] = "keyDown".into();
            self.page.call("Input.dispatchKeyEvent", d).await?;
            let mut u = up.clone();
            u["type"] = "keyUp".into();
            self.page.call("Input.dispatchKeyEvent", u).await.map(|_| ())
        }).await?;
        Ok(if ok { format!("ok: press {key}") }
        else if open_list { format!("ok: press {key} (foi para a lista aberta do select, que nao avisa a pagina; confira com snapshot)") }
        else { format!("erro: press {key}: o evento nao chegou na pagina (aba escondida sem quadro) — tire um shot e repita") })
    }

    async fn hover(&self, r: &str) -> Result<String, String> {
        self.mark("hover", r).await;
        if let Some(why) = self.blocked() { self.unmark().await; return Ok(why.into()); }
        let Some((x, y)) = self.measured(r).await? else { return Ok(format!("erro: ref {r} nao existe (rode snapshot de novo)")) };
        self.page.call("Input.dispatchMouseEvent", json!({"type": "mouseMoved", "x": x, "y": y})).await?;
        self.frame().await;
        Ok(format!("ok: hover {r}"))
    }

    async fn eval(&self, js: &str) -> Result<String, String> {
        let wrapped = format!("(async()=>{{const v=({js});await {FRAME};return v}})()");
        let run = |params: Value| within(&self.page, WAIT_MS, self.page.call("Runtime.evaluate", params));
        let Some(mut r) = run(json!({"expression": wrapped, "awaitPromise": true, "returnByValue": true})).await else {
            return Ok(format!("erro: eval nao respondeu em {WAIT_MS}ms"));
        };
        // Declaração não cabe entre parênteses: roda cru, como no console do DevTools.
        if r.as_ref().ok().and_then(exception).is_some_and(|e| e.starts_with("SyntaxError")) {
            let Some(raw) = run(json!({"expression": js, "awaitPromise": true, "returnByValue": true, "replMode": true})).await else {
                return Ok(format!("erro: eval nao respondeu em {WAIT_MS}ms"));
            };
            r = raw;
            if r.as_ref().is_ok_and(|v| exception(v).is_none()) { self.frame().await; }
        }
        let r = r?;
        if let Some(e) = exception(&r) { return Ok(format!("erro: {e}")); }
        Ok(match r["result"].get("value") { Some(v) => format!("ok: {v}"), None => "ok: undefined".into() })
    }

    async fn wait(&self, args: &[String]) -> Result<String, String> {
        let (args, limit) = match split_timeout(args) { Ok(v) => v, Err(e) => return Ok(e) };
        let target = args.first().map(String::as_str).unwrap_or("");
        let joined = args.join(" ");
        if let Some(ms) = target.bytes().all(|b| b.is_ascii_digit()).then(|| target.parse::<u64>().ok()).flatten() {
            self.page.sleep(ms).await;
            return Ok(format!("ok: wait {target}ms"));
        }
        // O resto da linha é o trecho: `wait --text create account` sem aspas também vale.
        let needle = args.get(1..).map(|rest| rest.join(" ")).unwrap_or_default();
        // Alvo torto esperaria o prazo inteiro para dar um erro que já se sabe agora.
        let valid = match target {
            "--url" | "--text" => !needle.is_empty(),
            "--idle" => true,
            t => t.len() > 2 && t.starts_with("@e") && t[2..].bytes().all(|b| b.is_ascii_digit()),
        };
        if !valid { return Ok(format!("erro: wait: alvo invalido: {joined} (use @eN, --url texto, --text texto, --idle ou ms; teto com --timeout ms)")); }
        if target == "--idle" { self.enable("Network").await?; }
        // O que a pessoa lê como texto inclui o rótulo de botão `<input>`, que o innerText não traz.
        let text_js = format!("(()=>{{const n={};const t=document.body.innerText+'\\n'+[...document.querySelectorAll('input[type=submit],input[type=button],input[type=reset]')].map(e=>e.value).join('\\n');return t.toLowerCase().includes(n.toLowerCase())}})()",
            serde_json::to_string(&needle).unwrap_or_default());
        // Conta o tempo das checagens além das pausas: página presa num laço de JS não responde ao
        // evaluate, e sem teto o verbo seguraria a vez de todos os outros.
        let mut waited = 0;
        while waited < limit {
            let check = async {
                Ok::<_, String>(match target {
                    t if t.starts_with('@') => { self.snapshot().await?; self.state.borrow().refs.contains_key(t) }
                    "--url" => self.value("location.href").await?.as_str().is_some_and(|u| u.contains(needle.as_str())),
                    "--text" => self.value(&text_js).await?.as_bool() == Some(true),
                    // Rede parada (contador zero e 500 ms sem resposta) e documento completo.
                    "--idle" => {
                        let quiet = { let s = self.state.borrow(); s.inflight.is_empty() && s.last_network.elapsed().as_millis() > 500 };
                        quiet && self.value("document.readyState").await?.as_str() == Some("complete")
                    }
                    _ => false,
                })
            };
            let t0 = Instant::now();
            let Some(hit) = within(&self.page, limit.saturating_sub(waited), check).await else { break };
            if hit? { return Ok(format!("ok: wait {joined}")); }
            let spent = t0.elapsed().as_millis() as u64;
            self.page.sleep(POLL_MS).await;
            waited += POLL_MS + spent;
        }
        Ok(format!("erro: wait {joined} nao aconteceu em {limit}ms"))
    }

    /// Navegação pedida por verbo: as refs antigas morrem já, e a resposta só sai com o documento novo carregado.
    async fn settle(&self, verb: &str, before: u64) -> Result<String, String> {
        self.state.borrow_mut().refs.clear();
        let mut waited = 0;
        while waited < WAIT_MS {
            let t0 = Instant::now();
            // A navegação conta antes de perguntar: lido depois, o "complete" ainda podia ser do documento antigo.
            let moved = self.state.borrow().navs != before;
            // Contexto destruído no meio = o documento ainda está trocando; tenta de novo.
            let ready = within(&self.page, WAIT_MS - waited, self.value("document.readyState")).await;
            if moved && let Some(Ok(v)) = &ready && v.as_str() == Some("complete") {
                let url = self.value("location.href").await?;
                return Ok(format!("ok: {verb} {}", url.as_str().unwrap_or("")));
            }
            if ready.is_none() { break; }
            self.page.sleep(POLL_MS).await;
            waited += POLL_MS + t0.elapsed().as_millis() as u64;
        }
        Ok(format!("erro: {verb}: a pagina nao terminou de carregar em {WAIT_MS}ms"))
    }

    async fn goto(&self, url: &str) -> Result<String, String> {
        let url = url.trim();
        if url.is_empty() { return Ok("erro: goto precisa de uma url".into()); }
        let url = if url.contains("://") { url.to_owned() } else { format!("http://{url}") };
        let before = self.state.borrow().navs;
        let r = self.page.call("Page.navigate", json!({"url": url})).await?;
        if let Some(e) = r["errorText"].as_str().filter(|e| !e.is_empty()) { return Ok(format!("erro: goto {url}: {e}")); }
        self.settle("goto", before).await
    }

    async fn back(&self) -> Result<String, String> {
        let h = self.page.call("Page.getNavigationHistory", json!({})).await?;
        let index = h["currentIndex"].as_i64().unwrap_or(0);
        let Some(entry) = usize::try_from(index - 1).ok().and_then(|i| h["entries"].get(i)) else {
            return Ok("erro: back: nao ha pagina anterior".into());
        };
        let before = self.state.borrow().navs;
        self.page.call("Page.navigateToHistoryEntry", json!({"entryId": entry["id"]})).await?;
        self.settle("back", before).await
    }

    async fn reload(&self) -> Result<String, String> {
        let before = self.state.borrow().navs;
        self.page.call("Page.reload", json!({})).await?;
        self.settle("reload", before).await
    }

    async fn apply_theme(&self) -> Result<(), String> {
        let value = match self.state.borrow().theme { "claro" => "light", "escuro" => "dark", _ => "" };
        let features = if value.is_empty() { json!([]) } else { json!([{"name": "prefers-color-scheme", "value": value}]) };
        self.page.call("Emulation.setEmulatedMedia", json!({"features": features})).await.map(|_| ())
    }

    async fn theme(&self, mode: &str) -> Result<String, String> {
        let Some(mode) = ["claro", "escuro", "sistema"].into_iter().find(|m| *m == mode) else { return Ok(format!("erro: tema desconhecido: {mode}")) };
        self.state.borrow_mut().theme = mode;
        self.apply_theme().await?;
        Ok(format!("tema: {mode}"))
    }

    async fn apply_layout(&self, layout: Layout) -> Result<(), String> {
        let metrics = |w: u32, h: u32, scale: f32, mobile: bool| json!({"width": w, "height": h, "deviceScaleFactor": scale, "mobile": mobile});
        // Antes de qualquer chamada: um ajuste do painel que chegue no meio não desfaz o layout pedido.
        self.page.hold_size(layout != Layout::Desktop);
        if layout == Layout::Mobile {
            self.page.call("Emulation.setDeviceMetricsOverride", metrics(390, 844, 2., true)).await?;
            self.page.call("Emulation.setTouchEmulationEnabled", json!({"enabled": true, "maxTouchPoints": 5})).await?;
            self.page.call("Emulation.setUserAgentOverride", json!({"userAgent": MOBILE_UA})).await?;
            return Ok(());
        }
        self.page.call("Emulation.setTouchEmulationEnabled", json!({"enabled": false})).await?;
        // Sem troca o Chromium do Linux se apresenta como "HeadlessChrome", e Google, GitHub e afins recusam o login.
        let version = self.page.call("Browser.getVersion", json!({})).await.unwrap_or_else(|e| {
            eprintln!("[nav] Browser.getVersion falhou ({e}): o User-Agent fica o do Chromium sem janela");
            Default::default()
        });
        self.page.call("Emulation.setUserAgentOverride", desktop_agent(&version)).await?;
        match layout {
            Layout::Custom(w, h) => self.page.call("Emulation.setDeviceMetricsOverride", metrics(w, h, 1., false)).await?,
            _ if self.state.borrow().hidden => self.page.call("Emulation.setDeviceMetricsOverride", metrics(HIDDEN_VIEWPORT.0, HIDDEN_VIEWPORT.1, 1., false)).await?,
            // O Chromium sem janela não segue a janela: limpar deixava a página no tamanho velho, e o quadro esticado.
            _ if let Some((w, h, scale)) = self.page.panel_size() => self.page.call("Emulation.setDeviceMetricsOverride", metrics(w, h, scale, false)).await?,
            _ => {
                // WebView2: a página segue a janela de verdade.
                self.page.call("Emulation.clearDeviceMetricsOverride", json!({})).await?;
                // Largura e altura 0 = o tamanho segue a janela; só a escala é emulada. Depois de limpar: sem isso o
                // 0×0 mantém o tamanho da emulação anterior (a da página escondida).
                let scale = self.state.borrow().scale;
                match scale { Some(scale) => self.page.call("Emulation.setDeviceMetricsOverride", metrics(0, 0, scale, false)).await?, None => json!({}) }
            }
        };
        Ok(())
    }

    async fn layout(&self, args: &[String]) -> Result<String, String> {
        if args.is_empty() { return Ok(format!("layout: {}", self.state.borrow().layout.label())); }
        let wanted = match parse_layout(args) { Ok(l) => l, Err(e) => return Ok(e) };
        let previous = self.state.borrow().layout;
        if let Err(e) = self.apply_layout(wanted).await {
            let _ = self.apply_layout(previous).await;
            return Ok(format!("erro: layout {}: {e}", args.join(" ")));
        }
        self.state.borrow_mut().layout = wanted;
        let screen = match wanted { Layout::Mobile => Some((390, 844)), Layout::Custom(w, h) => Some((w, h)), Layout::Desktop => None };
        if let Some(f) = self.on_layout.borrow().as_ref() { f(screen); }
        Ok(format!("layout: {}", wanted.label()))
    }

    fn console(&self, clear: bool) -> String {
        let mut s = self.state.borrow_mut();
        let out = s.console.iter().cloned().collect::<Vec<_>>().join("\n");
        if clear { s.console.clear(); }
        out
    }

    async fn network(&self) -> Result<String, String> {
        self.enable("Network").await?;
        Ok(self.state.borrow().network.iter().cloned().collect::<Vec<_>>().join("\n"))
    }

    async fn shot(&self, args: &[String]) -> Result<Reply, String> {
        let max = match shot_max(args) { Ok(m) => m, Err(e) => return Ok(Reply::Text(e)) };
        self.frame().await;
        let mut params = json!({"format": "png"});
        if let Layout::Custom(w, h) = self.state.borrow().layout {
            params = json!({"format": "png", "captureBeyondViewport": true, "clip": {"x": 0, "y": 0, "width": w, "height": h, "scale": 1}});
        }
        let Some(r) = within(&self.page, SHOT_MS, self.page.call("Page.captureScreenshot", params)).await else {
            return Ok(Reply::Text("erro: o navegador desta sessao nao produziu quadro — text/snapshot/click funcionam".into()));
        };
        let data = r.map_err(|e| format!("captureScreenshot falhou: {e}"))?;
        let png = screenshot_bytes(&data)?;
        if png.is_empty() { return Ok(Reply::Text("erro: o navegador desta sessao nao produziu quadro — text/snapshot/click funcionam".into())); }
        let Some(max) = max else { return Ok(Reply::Png(png)) };
        let png = self.page.background(move || reduzir_png(&png, max)).await.map_err(|e| format!("shot --max {max}: {e}"))?;
        Ok(Reply::Png(png))
    }
}

/// Tira `--max <px>` dos argumentos do `shot` (o caminho é o primeiro, lido pelo servidor).
fn shot_max(args: &[String]) -> Result<Option<u32>, String> {
    let Some(i) = args.iter().position(|a| a == "--max") else { return Ok(None) };
    args.get(i + 1).and_then(|v| v.parse::<u32>().ok()).filter(|px| *px > 0).map(Some)
        .ok_or_else(|| "erro: shot: --max precisa de um numero de pixels maior que zero".into())
}

/// O PNG com o maior lado em até `max`, na mesma proporção; já dentro do limite, os mesmos bytes.
pub fn reduzir_png(png: &[u8], max: u32) -> Result<Vec<u8>, String> {
    let reader = || image::ImageReader::with_format(std::io::Cursor::new(png), image::ImageFormat::Png);
    // Só o cabeçalho decide: quadro já pequeno nem é decodificado.
    let (w, h) = reader().into_dimensions().map_err(|e| e.to_string())?;
    if w.max(h) <= max { return Ok(png.to_vec()); }
    let image = reader().decode().map_err(|e| e.to_string())?;
    let mut out = std::io::Cursor::new(Vec::new());
    image.resize(max, max, image::imageops::FilterType::CatmullRom).write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

/// Argumento do console como o DevTools mostra: texto cru, número, e objeto pela prévia (`{a: 1, b: "x"}`).
fn remote_text(a: &Value) -> String {
    match a.get("value") {
        Some(Value::String(t)) => return t.clone(),
        Some(v) => return v.to_string(),
        None => {}
    }
    let preview = &a["preview"];
    let Some(props) = preview["properties"].as_array() else {
        return a["description"].as_str().or(a["type"].as_str()).unwrap_or("").to_owned();
    };
    let value = |p: &Value| {
        let v = p["value"].as_str().unwrap_or("");
        if p["type"] == "string" { format!("{v:?}") } else { v.to_owned() }
    };
    let array = preview["subtype"] == "array";
    let mut items: Vec<String> = props.iter().map(|p| if array { value(p) } else { format!("{}: {}", p["name"].as_str().unwrap_or(""), value(p)) }).collect();
    if preview["overflow"] == true { items.push("…".into()); }
    let body = items.join(", ");
    if array { format!("[{body}]") } else {
        let class = a["className"].as_str().filter(|c| *c != "Object").map(|c| format!("{c} ")).unwrap_or_default();
        format!("{class}{{{body}}}")
    }
}

/// Erro de JS que ninguém tratou: mensagem e onde aconteceu.
fn exception_line(d: &Value) -> String {
    let text = d["exception"]["description"].as_str().and_then(|t| t.lines().next())
        .or(d["exception"]["value"].as_str()).or(d["text"].as_str()).unwrap_or("");
    let at = d["url"].as_str().filter(|u| !u.is_empty())
        .map(|u| format!(" ({u}:{}:{})", d["lineNumber"].as_i64().unwrap_or(0) + 1, d["columnNumber"].as_i64().unwrap_or(0) + 1))
        .unwrap_or_default();
    format!("erro nao tratado: {text}{at}")
}

/// Tira `--timeout <ms>` dos argumentos do `wait`. O teto fica abaixo dos 45 s que o servidor espera pela resposta.
fn split_timeout(args: &[String]) -> Result<(Vec<String>, u64), String> {
    let Some(i) = args.iter().position(|a| a == "--timeout") else { return Ok((args.to_vec(), WAIT_MS)) };
    let ms = args.get(i + 1).and_then(|v| v.parse::<u64>().ok()).filter(|ms| (1..=WAIT_MAX_MS).contains(ms))
        .ok_or_else(|| format!("erro: wait: --timeout precisa de ms entre 1 e {WAIT_MAX_MS}"))?;
    let rest = args.iter().enumerate().filter(|(j, _)| *j != i && *j != i + 1).map(|(_, a)| a.clone()).collect();
    Ok((rest, ms))
}

/// A página de verdade: o CDP do motor (WebView2 no Windows, Chromium sem janela no Linux) e o relógio da GPUI.
#[cfg(not(target_os = "macos"))]
pub struct CdpPage {
    pub cdp: std::rc::Rc<super::cdp::Cdp>,
    pub executor: gpui_kit::BackgroundExecutor,
}

#[cfg(not(target_os = "macos"))]
impl Page for CdpPage {
    fn call(&self, method: &str, params: Value) -> impl Future<Output = Result<Value, String>> { self.cdp.call(method, params) }
    fn sleep(&self, ms: u64) -> impl Future<Output = ()> {
        let timer = self.executor.timer(std::time::Duration::from_millis(ms));
        async move { timer.await; }
    }
    #[cfg(target_os = "linux")]
    fn panel_size(&self) -> Option<(u32, u32, f32)> { self.cdp.viewport.panel.get() }
    #[cfg(target_os = "linux")]
    fn hold_size(&self, held: bool) { self.cdp.viewport.held.set(held) }
    fn background<T: Send + 'static>(&self, work: impl FnOnce() -> T + Send + 'static) -> impl Future<Output = T> {
        self.executor.spawn(async move { work() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use serde_json::json;
    use std::cell::RefCell;

    /// `stuck`: método que nunca responde, como página presa num laço de JS.
    /// `panel`/`held`: o lado do motor do painel, que guarda o tamanho aplicado e obedece ao layout pedido.
    struct Fake { calls: RefCell<Vec<(String, Value)>>, answer: Box<dyn Fn(&str, &Value) -> Result<Value, String>>, stuck: &'static str,
        panel: Cell<Option<(u32, u32, f32)>>, held: Cell<bool> }
    fn fake(answer: impl Fn(&str, &Value) -> Result<Value, String> + 'static, stuck: &'static str) -> Fake {
        Fake { calls: RefCell::default(), answer: Box::new(answer), stuck, panel: Cell::new(None), held: Cell::new(false) }
    }
    impl Page for Fake {
        fn panel_size(&self) -> Option<(u32, u32, f32)> { self.panel.get() }
        fn hold_size(&self, held: bool) { self.held.set(held) }
        fn call(&self, method: &str, params: Value) -> impl Future<Output = Result<Value, String>> {
            self.calls.borrow_mut().push((method.into(), params.clone()));
            if method == self.stuck { Either::Left(std::future::pending()) } else { Either::Right(std::future::ready((self.answer)(method, &params))) }
        }
        fn sleep(&self, _: u64) -> impl Future<Output = ()> { std::future::ready(()) }
    }
    fn ctl(answer: impl Fn(&str, &Value) -> Result<Value, String> + 'static) -> Controller<Fake> {
        Controller::new(fake(answer, ""), false)
    }
    fn text(r: Reply) -> String { match r { Reply::Text(t) => t, Reply::Png(_) => "png".into() } }
    fn methods(c: &Controller<Fake>) -> Vec<String> { c.page.calls.borrow().iter().map(|(m, _)| m.clone()).collect() }
    fn s(v: &[&str]) -> Vec<String> { v.iter().map(|x| x.to_string()).collect() }

    fn tree() -> Value { json!({"nodes": [
        {"nodeId": "1", "role": {"value": "RootWebArea"}, "name": {"value": "x"}, "childIds": ["2"]},
        {"nodeId": "2", "role": {"value": "button"}, "name": {"value": "Ok"}, "backendDOMNodeId": 7, "childIds": []}]}) }

    fn png_of(w: u32, h: u32) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(w, h, image::Rgba([30, 60, 90, 255])).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }
    fn size_of(png: &[u8]) -> (u32, u32) {
        let i = image::load_from_memory_with_format(png, image::ImageFormat::Png).unwrap();
        (i.width(), i.height())
    }

    #[test]
    fn reduzir_png_fits_the_largest_side_and_keeps_small_bytes() {
        assert_eq!(size_of(&reduzir_png(&png_of(2560, 1600), 1568).unwrap()), (1568, 980));
        let small = png_of(1280, 800);
        assert_eq!(reduzir_png(&small, 1568).unwrap(), small);
    }

    #[test]
    fn reduzir_png_via_shot_verb_and_rejects_bad_max() {
        use base64::Engine as _;
        let data = base64::engine::general_purpose::STANDARD.encode(png_of(2560, 1600));
        let c = ctl(move |m, _| Ok(if m == "Page.captureScreenshot" { json!({"data": data}) } else { json!({}) }));
        match block_on(c.run("shot", &s(&["/tmp/x.png", "--max", "1568"]))) {
            Reply::Png(png) => assert_eq!(size_of(&png), (1568, 980)),
            Reply::Text(t) => panic!("{t}"),
        }
        assert!(text(block_on(c.run("shot", &s(&["/tmp/x.png", "--max", "0"])))).starts_with("erro: shot: --max"));
        assert!(text(block_on(c.run("shot", &s(&["/tmp/x.png", "--max", "abc"])))).starts_with("erro: shot: --max"));
    }

    #[test]
    fn snapshot_lists_refs() {
        let c = ctl(|m, _| Ok(if m == "Accessibility.getFullAXTree" { tree() } else { json!({}) }));
        assert_eq!(text(block_on(c.run("snapshot", &[]))), "- RootWebArea \"x\"\n  - button \"Ok\" [ref=@e1]");
    }

    const SECRET: &str = "S3nh@ \"x\"/é";

    /// Página com um campo de senha (`@e1`, backend 7) que a árvore mostra com o valor cru, como mostraria com
    /// as senhas expostas à acessibilidade; `focus` é o que o alvo responde (`senha`, `campo`, `fora`).
    fn login_page(focus: &'static str) -> Controller<Fake> { ctl(move |m, p| Ok(login_answer(focus, m, p))) }

    fn login_answer(focus: &'static str, m: &str, p: &Value) -> Value {
        match m {
            "Accessibility.getFullAXTree" => json!({"nodes": [
                {"nodeId": "1", "role": {"value": "RootWebArea"}, "name": {"value": "Login"}, "childIds": ["2"]},
                {"nodeId": "2", "role": {"value": "textbox"}, "name": {"value": "Senha"}, "value": {"value": SECRET}, "backendDOMNodeId": 7, "childIds": []}]}),
            "DOM.getBoxModel" => json!({"model": {"content": [0, 0, 20, 0, 20, 10, 0, 10]}}),
            "DOM.resolveNode" => json!({"object": {"objectId": "o7"}}),
            "Runtime.callFunctionOn" => json!({"result": {"value": focus}}),
            "Runtime.evaluate" => match p["expression"].as_str().unwrap_or("") {
                e if e.starts_with("(typeof window.__hangarSonda") => json!({"result": {"value": 1}}),
                e if e.contains("a.shadowRoot.activeElement;if(!a") => json!({"result": {"value": ""}}),
                // O `eval` e o `text` leem o campo e devolvem o valor como a página o tem.
                e if e.contains("querySelector('input').value") || e == "document.body.innerText" => json!({"result": {"value": SECRET}}),
                _ => json!({"result": {}}),
            },
            _ => json!({}),
        }
    }

    fn fill_secret(c: &Controller<Fake>, kind: &str) -> String {
        block_on(c.run("snapshot", &[]));
        text(block_on(c.run_with("fill-secret", &s(&["@e1", kind]), Some(Secret::new(SECRET.into())))))
    }

    #[test]
    fn a_filled_secret_never_comes_back_in_snapshot_eval_or_text() {
        let c = login_page("senha");
        assert_eq!(fill_secret(&c, "password"), "ok: fill-secret @e1");
        let typed: Vec<Value> = c.page.calls.borrow().iter().filter(|(m, _)| m == "Input.insertText").map(|(_, p)| p.clone()).collect();
        assert_eq!(typed, vec![json!({"text": SECRET})]);
        let snapshot = text(block_on(c.run("snapshot", &[])));
        assert_eq!(snapshot, "- RootWebArea \"Login\"\n  - textbox \"Senha\" [ref=@e1]", "o nó marcado sai sem value");
        assert_eq!(text(block_on(c.run("eval", &s(&["document.querySelector('input').value"])))), format!("ok: \"{}\"", crate::browser::preview_fmt::MASK));
        assert!(!text(block_on(c.run("text", &[]))).contains("S3nh"));
        c.on_event("Runtime.consoleAPICalled", &json!({"type": "log", "args": [{"type": "string", "value": SECRET}]}));
        assert!(!text(block_on(c.run("console", &[]))).contains("S3nh"));
    }

    #[test]
    fn a_failed_insert_still_hides_the_secret() {
        let c = ctl(|m, p| if m == "Input.insertText" { Err("caiu".into()) } else { Ok(login_answer("senha", m, p)) });
        assert!(!fill_secret(&c, "password").starts_with("ok:"));
        assert_eq!(text(block_on(c.run("snapshot", &[]))), "- RootWebArea \"Login\"\n  - textbox \"Senha\" [ref=@e1]");
        assert!(!text(block_on(c.run("text", &[]))).contains("S3nh"));
    }

    #[test]
    fn a_password_only_goes_into_a_password_field() {
        let c = login_page("campo");
        assert!(fill_secret(&c, "password").starts_with("erro: erro_cofre_campo_nao_senha:"));
        let c = login_page("fora");
        assert!(fill_secret(&c, "password").starts_with("erro: fill-secret @e1: o foco nao ficou"));
        assert!(!methods(&c).contains(&"Input.insertText".to_string()), "nada digitado fora do campo de senha");
        // O usuário vai em campo de texto, e sem valor o verbo nem toca a página.
        assert_eq!(fill_secret(&login_page("campo"), "username"), "ok: fill-secret @e1");
        let c = login_page("senha");
        assert!(text(block_on(c.run("fill-secret", &s(&["@e1", "password"])))).starts_with("erro: fill-secret: so pela tool vault_fill"));
    }

    #[test]
    fn navigation_drops_the_mark_but_keeps_masking() {
        let c = login_page("senha");
        fill_secret(&c, "password");
        c.on_event("Page.frameNavigated", &json!({"frame": {"id": "f"}}));
        block_on(c.run("url", &[]));
        assert!(c.state.borrow().marked.is_empty());
        c.on_event("Network.responseReceived", &json!({"requestId": "1", "response": {"status": 302, "url": format!("https://x.test/?p={}", url::form_urlencoded::byte_serialize(SECRET.as_bytes()).collect::<String>())}}));
        assert!(!text(block_on(c.run("network", &[]))).contains("S3nh"));
    }

    #[test]
    fn idle_ignores_retry_loops_without_a_server_and_next_dev_overlay() {
        let c = ctl(|_, _| Ok(json!({})));
        let start = |id: &str, url: &str| c.on_event("Network.requestWillBeSent", &json!({"requestId": id, "request": {"method": "POST", "url": url}}));
        let hub = "http://localhost:8124/callbackhub/negotiate?negotiateVersion=1";
        start("1", hub);
        c.on_event("Network.loadingFailed", &json!({"requestId": "1", "errorText": "net::ERR_CONNECTION_REFUSED"}));
        start("2", hub);
        start("3", "http://localhost:3000/__nextjs_original-stack-frames");
        start("5", "http://localhost:3000/pss/geral/v1/telemetria/otlp/v1/traces");
        assert!(c.state.borrow().inflight.is_empty());
        start("4", "http://localhost:3000/api/x");
        start("6", "http://localhost:3000/v1/logs");
        assert_eq!(c.state.borrow().inflight.len(), 2);
        // O servidor subiu: a resposta tira o endereço do ruído, e a próxima chamada a ele conta.
        c.on_event("Network.responseReceived", &json!({"requestId": "2", "response": {"status": 200, "url": hub}}));
        start("7", hub);
        assert!(c.state.borrow().inflight.contains("7"));
    }

    #[test]
    fn click_with_stale_ref_asks_for_new_snapshot() {
        let c = ctl(|m, _| match m {
            "Accessibility.getFullAXTree" => Ok(tree()),
            "DOM.getBoxModel" => Err("No node with given id found".into()),
            _ => Ok(json!({})),
        });
        block_on(c.run("snapshot", &[]));
        assert_eq!(text(block_on(c.run("click", &s(&["@e1"])))), "erro: ref @e1 nao existe (rode snapshot de novo)");
        assert_eq!(text(block_on(c.run("click", &s(&["@e9"])))), "erro: ref @e9 nao existe (rode snapshot de novo)");
        assert!(!methods(&c).contains(&"Input.dispatchMouseEvent".to_string()));
    }

    #[test]
    fn picking_refuses_acting_verbs_without_touching_the_page() {
        let c = ctl(|m, _| Ok(if m == "Accessibility.getFullAXTree" { tree() } else { json!({}) }));
        block_on(c.run("snapshot", &[]));
        c.set_picking(true);
        for verb in ["click", "fill", "hover", "goto"] {
            assert_eq!(text(block_on(c.run(verb, &s(&["@e1", "x"])))), PICKING, "{verb}");
        }
        assert!(!methods(&c).contains(&"Input.dispatchMouseEvent".to_string()), "nada chega à página: viraria a escolha");
        // Ler continua livre.
        assert!(text(block_on(c.run("snapshot", &[]))).contains("@e1"));
        c.set_picking(false);
        assert_ne!(text(block_on(c.run("hover", &s(&["@e1"])))), PICKING);
    }

    #[test]
    fn click_confirms_the_event_arrived() {
        let c = ctl(|m, p| Ok(match m {
            "Accessibility.getFullAXTree" => tree(),
            "DOM.getBoxModel" => json!({"model": {"content": [0, 0, 20, 0, 20, 10, 0, 10]}}),
            // A sonda lida devolve 1 = o evento chegou.
            "Runtime.evaluate" if p["expression"].as_str().is_some_and(|e| e.starts_with("(typeof window.__hangarSonda")) => json!({"result": {"value": 1}}),
            _ => json!({}),
        }));
        block_on(c.run("snapshot", &[]));
        assert_eq!(text(block_on(c.run("click", &s(&["@e1"])))), "ok: click @e1");
        let presses: Vec<Value> = c.page.calls.borrow().iter().filter(|(m, _)| m == "Input.dispatchMouseEvent").map(|(_, p)| p.clone()).collect();
        assert_eq!(presses[0]["type"], "mousePressed");
        assert_eq!((presses[0]["x"].as_i64(), presses[0]["y"].as_i64()), (Some(10), Some(5)));
    }

    #[test]
    fn click_that_never_arrives_is_an_error() {
        let c = ctl(|m, p| Ok(match m {
            "Accessibility.getFullAXTree" => tree(),
            "DOM.getBoxModel" => json!({"model": {"content": [0, 0, 20, 0, 20, 10, 0, 10]}}),
            "Runtime.evaluate" if p["expression"].as_str().is_some_and(|e| e.starts_with("(typeof window.__hangarSonda")) => json!({"result": {"value": 0}}),
            _ => json!({}),
        }));
        block_on(c.run("snapshot", &[]));
        assert!(text(block_on(c.run("click", &s(&["@e1"])))).starts_with("erro: click @e1: o evento nao chegou"));
    }

    fn is_open_list(p: &Value) -> bool { p["expression"].as_str().is_some_and(|e| e.contains(":open")) }

    #[test]
    fn press_into_an_open_select_list_is_not_an_error() {
        let c = ctl(|m, p| Ok(match m {
            "Runtime.evaluate" if is_open_list(p) => json!({"result": {"value": true}}),
            // A lista nativa consome a tecla: o documento não vê o keydown.
            "Runtime.evaluate" if is_probe_read(p) => json!({"result": {"value": 0}}),
            _ => json!({}),
        }));
        let r = text(block_on(c.run("press", &s(&["ArrowDown"]))));
        assert!(r.starts_with("ok: press ArrowDown"), "{r}");
        assert_eq!(c.page.calls.borrow().iter().filter(|(m, _)| m == "Input.dispatchKeyEvent").count(), 2, "uma tecla, sem repetir");
    }

    #[test]
    fn press_that_never_arrives_without_an_open_list_is_an_error() {
        for (hidden, open) in [(false, false), (true, true)] {
            let c = Controller::new(fake(move |m, p| Ok(match m {
                "Runtime.evaluate" if is_open_list(p) => json!({"result": {"value": open}}),
                "Runtime.evaluate" if is_probe_read(p) => json!({"result": {"value": 0}}),
                _ => json!({}),
            }), ""), false);
            block_on(c.set_hidden(hidden));
            assert!(text(block_on(c.run("press", &s(&["Enter"])))).starts_with("erro: press Enter: o evento nao chegou"), "hidden={hidden} open={open}");
        }
    }

    fn is_probe_read(p: &Value) -> bool { p["expression"].as_str().is_some_and(|e| e.starts_with("(typeof window.__hangarSonda")) }
    fn is_probe_arm(p: &Value) -> bool { p["expression"].as_str().is_some_and(|e| e.starts_with("(()=>{window.__hangarSonda")) }
    fn boxed() -> Value { json!({"model": {"content": [0, 0, 20, 0, 20, 10, 0, 10]}}) }

    #[test]
    fn click_with_unreadable_probe_is_not_confirmed() {
        let c = ctl(|m, p| match m {
            "Accessibility.getFullAXTree" => Ok(tree()),
            "DOM.getBoxModel" => Ok(boxed()),
            "Runtime.evaluate" if is_probe_read(p) => Err("Internal error".into()),
            _ => Ok(json!({})),
        });
        block_on(c.run("snapshot", &[]));
        assert!(text(block_on(c.run("click", &s(&["@e1"])))).starts_with("erro: click @e1: o evento nao chegou"));
    }

    #[test]
    fn click_that_navigates_away_counts_as_delivered() {
        let c = ctl(|m, p| match m {
            "Accessibility.getFullAXTree" => Ok(tree()),
            "DOM.getBoxModel" => Ok(boxed()),
            "Runtime.evaluate" if is_probe_read(p) => Err("Execution context was destroyed.".into()),
            _ => Ok(json!({})),
        });
        block_on(c.run("snapshot", &[]));
        assert_eq!(text(block_on(c.run("click", &s(&["@e1"])))), "ok: click @e1");
    }

    #[test]
    fn click_with_unarmed_probe_is_an_error() {
        let c = ctl(|m, p| match m {
            "Accessibility.getFullAXTree" => Ok(tree()),
            "DOM.getBoxModel" => Ok(boxed()),
            "Runtime.evaluate" if is_probe_arm(p) => Err("Cannot find context".into()),
            _ => Ok(json!({})),
        });
        block_on(c.run("snapshot", &[]));
        assert_eq!(text(block_on(c.run("click", &s(&["@e1"])))), "erro: sonda nao armada: Cannot find context");
        assert!(!methods(&c).contains(&"Input.dispatchMouseEvent".to_string()));
    }

    #[test]
    fn box_model_failure_other_than_missing_node_surfaces() {
        let c = ctl(|m, _| match m {
            "Accessibility.getFullAXTree" => Ok(tree()),
            "DOM.getBoxModel" => Err("Could not compute box model.".into()),
            _ => Ok(json!({})),
        });
        block_on(c.run("snapshot", &[]));
        assert_eq!(text(block_on(c.run("hover", &s(&["@e1"])))), "erro: @e1: Could not compute box model.");
    }

    #[test]
    fn wait_with_invalid_target_fails_at_once() {
        let c = ctl(|_, _| Ok(json!({})));
        for args in [&["--url"][..], &["--texto", "x"], &["@x1"], &[]] {
            assert_eq!(text(block_on(c.run("wait", &s(args)))),
                format!("erro: wait: alvo invalido: {} (use @eN, --url texto, --text texto, --idle ou ms; teto com --timeout ms)", args.join(" ")));
        }
        assert!(!methods(&c).contains(&"Runtime.evaluate".to_string()));
    }

    #[test]
    fn type_without_editable_focus_is_refused() {
        let c = ctl(|m, _| Ok(if m == "Runtime.evaluate" { json!({"result": {"value": "button"}}) } else { json!({}) }));
        assert_eq!(text(block_on(c.run("type", &s(&["oi"])))),
            "erro: type: nenhum campo de texto com foco (foco em button) — use fill @eN, ou click no campo antes");
        assert!(!methods(&c).contains(&"Input.insertText".to_string()));
    }

    #[test]
    fn eval_falls_back_to_statements_on_syntax_error() {
        let c = ctl(|m, p| Ok(if m != "Runtime.evaluate" { json!({}) }
            else if p["replMode"] == true { json!({"result": {"value": 2}}) }
            else if p["expression"].as_str().is_some_and(|e| e.starts_with("(async()=>")) { json!({"exceptionDetails": {"exception": {"description": "SyntaxError: Unexpected token 'const'"}}}) }
            else { json!({"result": {}}) }));
        assert_eq!(text(block_on(c.run("eval", &s(&["const a=1; a+1"])))), "ok: 2");
    }

    #[test]
    fn eval_without_value_prints_undefined() {
        let c = ctl(|_, _| Ok(json!({"result": {}})));
        assert_eq!(text(block_on(c.run("eval", &s(&["void 0"])))), "ok: undefined");
    }

    #[test]
    fn console_keeps_the_last_200_and_clears() {
        let c = ctl(|_, _| Ok(json!({})));
        for i in 0..205 { c.on_event("Runtime.consoleAPICalled", &json!({"type": "log", "args": [{"value": i}]})); }
        let out = text(block_on(c.run("console", &[])));
        assert_eq!(out.lines().count(), 200);
        assert!(out.starts_with("log: 5\n"));
        block_on(c.run("console", &s(&["--limpar"])));
        assert_eq!(text(block_on(c.run("console", &[]))), "");
    }

    #[test]
    fn wait_gives_up_with_the_electron_text() {
        let c = ctl(|_, _| Ok(json!({"result": {"value": "http://a/"}})));
        assert_eq!(text(block_on(c.run("wait", &s(&["--url", "/done"])))), "erro: wait --url /done nao aconteceu em 15000ms");
        assert_eq!(text(block_on(c.run("wait", &s(&["300"])))), "ok: wait 300ms");
    }

    #[test]
    fn wait_gives_up_when_the_page_never_answers() {
        let c = Controller::new(fake(|_, _| Ok(json!({})), "Runtime.evaluate"), false);
        assert_eq!(text(block_on(c.run("wait", &s(&["--text", "x"])))), "erro: wait --text x nao aconteceu em 15000ms");
    }

    #[test]
    fn hidden_desktop_uses_fixed_viewport() {
        let c = ctl(|_, _| Ok(json!({})));
        block_on(c.set_hidden(true));
        let last = c.page.calls.borrow().iter().rev().find(|(m, _)| m == "Emulation.setDeviceMetricsOverride").cloned();
        assert_eq!(last.map(|(_, p)| (p["width"].clone(), p["height"].clone())), Some((json!(1280), json!(800))));
        block_on(c.set_hidden(false));
        assert_eq!(methods(&c).last().map(String::as_str), Some("Emulation.clearDeviceMetricsOverride"));
    }

    #[test]
    fn visible_desktop_draws_at_the_window_scale_and_custom_layout_stays_exact() {
        let c = ctl(|_, _| Ok(json!({})));
        block_on(c.set_scale(1.25));
        let last = |c: &Controller<Fake>| c.page.calls.borrow().iter().rev().find(|(m, _)| m == "Emulation.setDeviceMetricsOverride").cloned().map(|(_, p)| p);
        assert_eq!(last(&c), Some(json!({"width": 0, "height": 0, "deviceScaleFactor": 1.25, "mobile": false})));
        block_on(c.run("layout", &s(&["1280", "800"])));
        assert_eq!(last(&c), Some(json!({"width": 1280, "height": 800, "deviceScaleFactor": 1.0, "mobile": false})));
    }

    /// Escondida → à vista → painel cresce (barra lateral recolhida) → controlador reaplica: a página termina no
    /// tamanho e na densidade do painel, nunca limpa nem nos 1280×800 da escondida.
    #[test]
    fn visible_desktop_keeps_the_panel_size_and_a_requested_layout_holds_it() {
        let c = ctl(|_, _| Ok(json!({})));
        let last = |c: &Controller<Fake>| c.page.calls.borrow().iter().rev()
            .find(|(m, _)| m.starts_with("Emulation.") && m.ends_with("DeviceMetricsOverride")).cloned();
        let panel = |w: u32, h: u32| Some(("Emulation.setDeviceMetricsOverride".to_owned(),
            json!({"width": w, "height": h, "deviceScaleFactor": 1.25, "mobile": false})));
        block_on(c.set_hidden(true));
        c.page.panel.set(Some((1076, 729, 1.25)));
        block_on(c.set_hidden(false));
        assert_eq!(last(&c), panel(1076, 729));
        c.page.panel.set(Some((1400, 729, 1.25)));
        block_on(c.set_scale(1.25));
        assert_eq!(last(&c), panel(1400, 729));
        block_on(c.run("layout", &s(&["800", "600"])));
        assert!(c.page.held.get(), "layout pedido manda no tamanho");
        block_on(c.run("layout", &s(&["desktop"])));
        assert!(!c.page.held.get());
        assert_eq!(last(&c), panel(1400, 729));
    }

    #[test]
    fn late_hidden_request_never_overrides_a_newer_one() {
        let c = ctl(|_, _| Ok(json!({})));
        let busy = block_on(c.turn.lock());
        let waker = futures::task::noop_waker();
        let mut cx = std::task::Context::from_waker(&waker);
        let (mut hide, mut show) = (Box::pin(c.set_hidden(true)), Box::pin(c.set_hidden(false)));
        assert!(hide.as_mut().poll(&mut cx).is_pending() && show.as_mut().poll(&mut cx).is_pending());
        drop(busy);
        // O pedido antigo pega o `turn` por último e ainda assim aplica o mais novo (visível).
        block_on(show);
        block_on(hide);
        assert!(!c.state.borrow().hidden);
        assert_eq!(methods(&c).last().map(String::as_str), Some("Emulation.clearDeviceMetricsOverride"));
    }

    #[test]
    fn start_applies_the_panel_shown_while_it_was_starting() {
        let c = Controller::new(fake(|_, _| Ok(json!({})), ""), true);
        let start = c.start();
        block_on(c.set_hidden(false));
        block_on(start);
        assert!(!c.state.borrow().hidden);
        assert_eq!(methods(&c).last().map(String::as_str), Some("Emulation.clearDeviceMetricsOverride"));
    }

    #[test]
    fn layout_and_theme_answer_like_electron() {
        let c = ctl(|_, _| Ok(json!({})));
        assert_eq!(text(block_on(c.run("layout", &[]))), "layout: desktop");
        assert_eq!(text(block_on(c.run("layout", &s(&["390", "844"])))), "layout: 390x844");
        assert_eq!(text(block_on(c.run("tema", &s(&["escuro"])))), "tema: escuro");
        assert_eq!(text(block_on(c.run("tema", &s(&["roxo"])))), "erro: tema desconhecido: roxo");
    }

    #[test]
    fn navigation_drops_refs() {
        let c = ctl(|m, _| Ok(if m == "Accessibility.getFullAXTree" { tree() } else { json!({}) }));
        block_on(c.run("snapshot", &[]));
        c.on_event("Page.frameNavigated", &json!({"frame": {"id": "main"}}));
        assert_eq!(text(block_on(c.run("hover", &s(&["@e1"])))), "erro: ref @e1 nao existe (rode snapshot de novo)");
    }

    #[test]
    fn runs_are_serialized() {
        let c = ctl(|_, _| Ok(json!({"result": {"value": "x"}})));
        let (a, b) = block_on(futures::future::join(c.run("eval", &s(&["1"])), c.run("text", &[])));
        assert_eq!((text(a), text(b)), ("ok: \"x\"".into(), "x".into()));
        assert_eq!(methods(&c).iter().filter(|m| !m.ends_with(".enable")).count(), 2);
    }

    #[test]
    fn tabs_and_unknown_verbs_are_refused() {
        let c = ctl(|_, _| Ok(json!({})));
        assert!(text(block_on(c.run("tab-list", &[]))).starts_with("erro: o app nativo ainda nao tem abas"));
        assert_eq!(text(block_on(c.run("voar", &[]))), "erro: verbo desconhecido: voar");
    }
}
