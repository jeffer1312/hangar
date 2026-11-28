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
    /// O navegador saiu: o erro do CDP no meio do verbo é ele ter fechado, não a página ter recusado.
    fn closed(&self) -> bool { false }
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

/// `Error`: o texto `erro: …` com o código fechado da causa, decidido onde o erro nasce (vai no cabeçalho do `/cmd`).
pub enum Reply { Text(String), Png(Vec<u8>), Error(&'static str, String) }

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
/// Acha o alvo que não é `@eN`: `css=`/seletor ou texto visível. Devolve o nó, ou "nenhum", "varios:N:lista" ou o erro.
const FIND: &str = r#"/*__hangarTarget*/((q)=>{const vis=e=>{const r=e.getBoundingClientRect();if(r.width<=0||r.height<=0)return false;if(e.closest('[inert],[aria-hidden=true]'))return false;if(e.checkVisibility)return e.checkVisibility({checkOpacity:true,checkVisibilityCSS:true});const s=getComputedStyle(e);return s.visibility!=='hidden'&&s.display!=='none'&&s.opacity!=='0'};const own=e=>e.type==='password'?'':String(e.value??'');const norm=t=>String(t??'').replace(/\s+/g,' ').trim().toLowerCase();const show=t=>String(t??'').replace(/\s+/g,' ').trim();let css=null,txt=null;if(q.startsWith('css='))css=q.slice(4);else if(q.startsWith('text='))txt=q.slice(5);else if(/^[#.][A-Za-z_-]|^\[[A-Za-z_-]/.test(q))css=q;else txt=q;let found;if(css!==null){try{found=[...document.querySelectorAll(css)].filter(vis)}catch(e){return 'seletor css invalido: '+css+' (texto que parece seletor: use text=)'}}else{const n=norm(txt.replace(/^["'“]|["'”]$/g,''));const label=e=>norm(e.getAttribute('aria-label')||e.innerText||own(e)||e.placeholder||e.title||e.alt);const all=[...document.querySelectorAll('a,button,input,select,textarea,summary,label,option,[role],[onclick],[tabindex],[contenteditable]')].filter(vis);found=all.filter(e=>label(e)===n);if(!found.length)found=[...document.querySelectorAll('body *')].filter(e=>vis(e)&&norm(e.innerText)===n);if(!found.length)found=all.filter(e=>label(e).includes(n));found=found.filter(e=>!found.some(o=>o!==e&&e.contains(o)))}if(found.length===1)return found[0];if(!found.length)return 'nenhum';return 'varios:'+found.length+':'+found.slice(0,3).map(e=>e.tagName.toLowerCase()+' “'+show(e.innerText||e.getAttribute('aria-label')||e.placeholder||'').slice(0,40)+'”').join(', ')})"#;
/// Conta mudanças do DOM, a saída da página e, com `net`, fetch e XHR em voo a partir de agora. O `eval` não embrulha a
/// rede: o js dele leria o embrulho no lugar do `fetch` da página. `drop` desfaz, e só devolve
/// `fetch`/`send` que ainda forem os nossos: a página ou o agente podem tê-los trocado no meio.
const QUIET_ARM: &str = "((net)=>{const p=window.__hangarQuiet;if(p)p.drop();let s;s=window.__hangarQuiet={n:0,p:0,t:performance.now(),leaving:0};const bump=d=>{s.n++;s.p+=d;s.t=performance.now()};s.o=new MutationObserver(()=>bump(0));s.o.observe(document,{subtree:true,childList:true,attributes:true,characterData:true});const leave=()=>{s.leaving=1};addEventListener('beforeunload',leave,true);const f=window.fetch;const wf=net&&f&&function(...a){bump(1);return f.apply(this,a).finally(()=>bump(-1))};if(wf)window.fetch=wf;const P=XMLHttpRequest.prototype,x=P.send;const wx=net&&function(...a){bump(1);this.addEventListener('loadend',()=>bump(-1),{once:true});return x.apply(this,a)};if(wx)P.send=wx;s.drop=()=>{s.o.disconnect();removeEventListener('beforeunload',leave,true);if(wf&&window.fetch===wf)window.fetch=f;if(wx&&P.send===wx)P.send=x;if(window.__hangarQuiet===s)delete window.__hangarQuiet};return 1})";
/// Desfaz o que o `QUIET_ARM` pôs na página, quando a ação não chega ao `calm`.
const QUIET_DROP: &str = "(()=>{const s=window.__hangarQuiet;if(s)s.drop();return 1})()";
/// Texto da página com os iframes do mesmo site; os de outro site o navegador não deixa ler.
const TEXT_ALL: &str = "/*__hangarText*/(()=>{const out=[];let cross=0;const shown=e=>!e.checkVisibility||e.checkVisibility({checkOpacity:true,checkVisibilityCSS:true});const walk=d=>{if(!d.body||!shown(d.body))return;out.push(d.body.innerText);for(const f of d.querySelectorAll('iframe,frame')){if(!shown(f))continue;let c=null;try{c=f.contentDocument}catch(e){}if(c&&c.body)walk(c);else if(!c)cross++}};walk(document);return {t:out.join('\\n'),cross}})()";
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
/// Teto da espera da página assentar depois de uma ação.
const QUIET_MAX_MS: u64 = 3_000;
/// DOM mudando sem rede (relógio, animação): até aqui, não até o teto.
const QUIET_DOM_MS: u64 = 1_500;
/// Navegação causada por uma ação. Somada ao armar (1 s), à espera (3 s) e à saída (`LEAVE_MS`), cabe no `VERB_MS`.
const NAV_IN_ACTION_MS: u64 = 11_000;
const LEAVE_MS: u64 = 3_000;
/// Teto de um verbo cuja página não responde: sem ele uma chamada CDP presa segurava a vez para sempre, e todo verbo
/// seguinte esgotava o prazo com a página viva.
const VERB_MS: u64 = 20_000;
/// Folga abaixo do prazo de quem espera, para o erro do teto ainda chegar antes de ele desistir.
const VERB_MARGIN_MS: u64 = 2_000;
/// O efeito que o verbo cortado depois de mandar texto ou tecla pode ter deixado na página.
const TEXT_SENT: &str = "o texto pode ter sido aplicado";
const KEY_SENT: &str = "a tecla pode ter sido aplicada";


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
pub struct Secret { value: String, forms: Vec<String>, host: Option<String> }

impl Secret {
    pub fn new(value: String) -> Self { Self { value, forms: Vec::new(), host: None } }
    /// Valor que só entra em página de `host`: o pedido veio pelo canal anfitrião, e a página pode ter navegado depois
    /// que o servidor de lá leu o domínio dela.
    pub fn for_host(mut self, host: String) -> Self { self.host = Some(normal_host(&host)); self }
    pub fn as_str(&self) -> &str { &self.value }
}

/// O host da página como o servidor o compara (`vault::exec::host_of`): minúsculas, sem usuário, porta nem ponto
/// final. Só http(s) tem domínio.
fn page_host(url: &str) -> Option<String> {
    let url = url.trim();
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = match host.strip_prefix('[') { Some(v6) => v6.split(']').next()?, None => host.split(':').next()? };
    let host = normal_host(host);
    (!host.is_empty()).then_some(host)
}

/// O host aprovado chega pelo canal de outra máquina: comparado na mesma forma do da página.
fn normal_host(host: &str) -> String { host.trim_start_matches('[').trim_end_matches(']').trim_end_matches('.').to_ascii_lowercase() }

impl Drop for Secret {
    fn drop(&mut self) {
        for text in std::iter::once(&mut self.value).chain(self.forms.iter_mut()) {
            // SAFETY: zero é UTF-8 válido; a escrita volátil não é cortada pelo compilador.
            for byte in unsafe { text.as_bytes_mut() } { unsafe { std::ptr::write_volatile(byte, 0) } }
        }
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    }
}

/// O alvo é o elemento com foco na raiz dele (documento, shadow root ou iframe) e, se for, se é campo de senha.
const TARGET_FOCUS: &str = "function(){const r=this.getRootNode&&this.getRootNode();if(!r||r.activeElement!==this)return 'fora';return this.tagName==='INPUT'&&this.type==='password'?'senha':'campo'}";
/// Confere e digita numa só tarefa do documento do campo: entre duas chamadas CDP a página pode navegar, e o
/// `Input.insertText` digitaria no que tivesse foco no documento novo. Argumentos: valor, host aprovado (ou `null`,
/// já em minúsculas e sem ponto final) e se o campo é de senha. O host sai só de `location` e `===`, que a página
/// não consegue trocar. Sem `select()` que cubra o campo todo, apaga pelo setter nativo (input) ou seleciona o
/// conteúdo (contenteditable); sem apagar, não digita. Depois relê o campo: input controlado, `maxlength` ou
/// `beforeinput` podem ter deixado outro valor, e aí a resposta não pode ser `ok`.
const FILL_SECRET: &str = "function(v,h,p){if(h!==null){let n=location.hostname.toLowerCase();while(n.endsWith('.'))n=n.slice(0,-1);const o=location.protocol;if((o!=='http:'&&o!=='https:')||(n!==h&&n!=='['+h+']'))return 'dominio:'+n}const r=this.getRootNode&&this.getRootNode();if(!r||r.activeElement!==this)return 'fora';if(p&&!(this.tagName==='INPUT'&&this.type==='password'))return 'campo';const d=this.ownerDocument,t=this.tagName==='INPUT'||this.tagName==='TEXTAREA';let c=false;if(t){try{if(typeof this.select==='function'){this.select();c=this.selectionStart===0&&this.selectionEnd===this.value.length}}catch(e){}if(!c)try{Object.getOwnPropertyDescriptor((this.tagName==='INPUT'?HTMLInputElement:HTMLTextAreaElement).prototype,'value').set.call(this,'');c=this.value===''}catch(e){}}else if(this.isContentEditable){try{const s=d.getSelection(),g=d.createRange();g.selectNodeContents(this);s.removeAllRanges();s.addRange(g);c=s.rangeCount===1}catch(e){}}if(!c)return 'naoapagou';let k=false;try{k=d.execCommand('insertText',false,v)}catch(e){return 'diferente'}if(!k)return 'recusado';return (t?this.value:this.textContent)===v?'ok':'diferente'}";

pub struct Controller<P: Page> {
    page: P,
    state: RefCell<State>,
    /// Um verbo por vez: dois clientes (agente e tool MCP) não intercalam eventos.
    turn: Mutex<()>,
    /// A espera do `arm_quiet` ficou na página (diálogo ou teto cortaram o verbo): o próximo verbo a desfaz.
    watching: Cell<bool>,
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
    /// Método CDP sem resposta ainda: o teto do verbo diz em qual a página parou.
    in_flight: RefCell<Option<String>>,
    /// Método e evento que soltam o botão do clique ou a tecla da `press` já apertados: o teto que corta o verbo ali
    /// os solta.
    pressed: RefCell<Option<(&'static str, Value)>>,
    /// O `fill-secret` já mandou o valor à página: cortado depois disso, a resposta não pode dizer que nada entrou.
    secret_sent: Cell<bool>,
    /// Texto ou tecla já mandados à página: cortado depois disso, a resposta diz que o efeito pode ter acontecido.
    input_sent: Cell<Option<&'static str>>,
    /// Código da recusa que o verbo vai devolver, marcado onde ela nasce (`failed`); o fim do verbo o põe na resposta.
    failure: Cell<Option<&'static str>>,
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
        Self { page, turn: Mutex::new(()), watching: Cell::new(false), wanted_hidden: Cell::new(hidden), paused: Cell::new(false), picking: Cell::new(false), on_activity: RefCell::new(None), on_layout: RefCell::new(None), on_page_error: RefCell::new(None), dialog_open: RefCell::new(None), strip_shown: Cell::new(false), settle: Cell::new(false), dialog_answered: Cell::new(false), dialog_signal: RefCell::new(None), on_dialog_answer: RefCell::new(None), in_flight: RefCell::new(None), pressed: RefCell::new(None), secret_sent: Cell::new(false), input_sent: Cell::new(None), failure: Cell::new(None), state: RefCell::new(State { refs: HashMap::new(), console: VecDeque::new(),
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
        if let Err(e) = self.bounded("layout", self.apply_layout(layout)).await { note_panel(&format!("[nav] layout nao aplicado: {e}")); }
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
        if layout == Layout::Desktop && !hidden && let Err(e) = self.bounded("escala", self.apply_layout(layout)).await { note_panel(&format!("[nav] escala nao aplicada: {e}")); }
    }

    /// Trabalho que segura a vez fora de um verbo (tamanho e escala do painel): o mesmo teto, para a vez nunca ficar presa.
    async fn bounded(&self, what: &str, work: impl Future<Output = Result<(), String>>) -> Result<(), String> {
        self.in_flight.borrow_mut().take();
        within(&self.page, VERB_MS, work).await.unwrap_or_else(|| {
            let stuck = self.in_flight.borrow_mut().take();
            note_stuck(what, stuck.as_deref(), VERB_MS);
            Err(stuck_text(what, VERB_MS, stuck.as_deref()))
        })
    }

    /// Toda chamada do controlador passa aqui. Chamada largada pelo teto fica sem quem leia: a resposta atrasada vai
    /// para o id dela no transporte, nunca para a do verbo seguinte.
    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        *self.in_flight.borrow_mut() = Some(method.to_owned());
        let reply = self.page.call(method, params).await;
        self.in_flight.borrow_mut().take();
        reply
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
        // O painel guarda e mostra esse texto: a senha que a página registrou não chega nele.
        if let (Some(error), Some(f)) = (error, self.on_page_error.borrow().as_ref()) { f(error.map(|text| self.masked(text))); }
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

    pub async fn run_with(&self, verb: &str, args: &[String], secret: Option<Secret>) -> Reply { self.run_until(verb, args, secret, None).await }

    /// `secret`: o valor do `fill-secret`, que nunca vem nos `args`. Toda resposta de texto sai com as senhas já
    /// preenchidas trocadas por `••••`. `deadline`: até quando quem pediu espera, contado da chegada do pedido; o
    /// verbo para antes, com folga.
    pub async fn run_until(&self, verb: &str, args: &[String], secret: Option<Secret>, deadline: Option<Instant>) -> Reply {
        match self.run_inner(verb, args, secret, deadline).await {
            Reply::Text(text) => Reply::Text(self.masked(text)),
            Reply::Error(code, text) => Reply::Error(code, self.masked(text)),
            png => png,
        }
    }

    /// `text`, a recusa do verbo, com a causa `code` guardada para a resposta.
    fn no_ref(&self, r: &str) -> String { self.failed("ref_invalid", format!("erro: ref {r} nao existe (rode snapshot de novo)")) }

    fn failed(&self, code: &'static str, text: String) -> String {
        self.failure.set(Some(code));
        text
    }

    fn masked(&self, text: String) -> String {
        self.state.borrow().secrets.iter().fold(text, |out, secret| mask(&out, &secret.forms))
    }

    async fn run_inner(&self, verb: &str, args: &[String], mut secret: Option<Secret>, deadline: Option<Instant>) -> Reply {
        if let Some(why) = self.blocked().filter(|_| ACTING.contains(&verb)) { return Reply::Text(why.into()); }
        // Antes da vez: o verbo que segura a vez pode estar parado justamente no diálogo.
        if verb == "dialog" { return Reply::Text(self.answer_dialog(args).await); }
        let _turn = self.turn.lock().await;
        self.in_flight.borrow_mut().take();
        self.pressed.borrow_mut().take();
        self.secret_sent.set(false);
        self.input_sent.set(None);
        self.failure.set(None);
        if let Some(why) = self.blocked().filter(|_| ACTING.contains(&verb)) { return Reply::Text(why.into()); }
        for _ in 0..20 {
            if !(self.dialog_answered.get() && self.dialog_open.borrow().is_some()) { break; }
            self.page.sleep(50).await;
        }
        let open = self.dialog_open.borrow().is_some();
        if let Some((kind, message, _)) = self.dialog_open.borrow().clone() && !WITH_DIALOG.contains(&verb) {
            return Reply::Error("dialog_open", format!("erro: a pagina esta com um dialogo {kind} aberto: “{message}”. Responda antes: {}", Self::dialog_hint(&kind)));
        }
        // Prazo vencido na fila: quem pediu já desistiu, e o verbo não mexe na página por ele.
        let left = deadline.map(|d| d.saturating_duration_since(Instant::now()).as_millis() as u64);
        if left == Some(0) { return Reply::Error("queue_deadline", format!("erro: {verb} não começou: o prazo de quem pediu venceu na fila")); }
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
            if self.watching.get() { self.drop_quiet().await; }
        }
        match verb {
            "snapshot" => self.snapshot_verb(args).await.map(Reply::Text),
            // Alvo de texto com espaço sem aspas chega em vários args: é um alvo só.
            "click" => self.click(&args.join(" ")).await.map(Reply::Text),
            "fill" => self.fill(arg(0), arg(1)).await.map(Reply::Text),
            "fill-secret" => self.fill_secret(arg(0), arg(1), secret.take()).await.map(Reply::Text),
            "type" => self.type_text(arg(0)).await.map(Reply::Text),
            "press" => self.press(arg(0)).await.map(Reply::Text),
            "hover" => self.hover(&args.join(" ")).await.map(Reply::Text),
            "wait" => self.wait(args).await.map(Reply::Text),
            "eval" => self.eval(arg(0)).await.map(Reply::Text),
            // Pelo histórico do navegador, não por JS: responde mesmo com a página presa (diálogo aberto, laço de script).
            "url" => self.current_url().await.map(Reply::Text),
            "estado" => Ok(Reply::Text(if self.paused.get() { "pausado".into() } else { "livre".into() })),
            "text" => self.page_text().await.map(Reply::Text),
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
        let raced = async {
            match select(std::pin::pin!(work), std::pin::pin!(opened)).await {
                Either::Left((result, _)) => result,
                Either::Right(_) => {
                    let _ = within(&self.page, 500, self.unmark()).await;
                    let (kind, message, _) = self.dialog_open.borrow().clone().unwrap_or_default();
                    Ok(Reply::Text(format!("ok: {verb} parou: a pagina abriu um dialogo {kind}: “{message}”. Responda com: {}", Self::dialog_hint(&kind))))
                }
            }
        };
        let ceiling = verb_ceiling(verb, args, left);
        let finished = match ceiling { Some(ms) => within(&self.page, ms, raced).await, None => Some(raced.await) };
        let result = match finished {
            Some(result) => result,
            None => {
                let ms = ceiling.unwrap_or_default();
                let stuck = self.in_flight.borrow_mut().take();
                note_stuck(verb, stuck.as_deref(), ms);
                // Sem soltar, o verbo seguinte começaria com o botão ou a tecla presos; sem esperar a resposta além de um instante.
                let release = self.pressed.borrow_mut().take();
                if let Some((method, release)) = release { let _ = within(&self.page, 500, self.page.call(method, release)).await; }
                let _ = within(&self.page, 500, self.unmark()).await;
                Ok(if self.secret_sent.get() {
                    Reply::Text(format!("erro: erro_cofre_incerto: {verb} não confirmou em {ms} ms; o valor pode ter sido digitado{}", stuck_at(stuck.as_deref())))
                } else if let Some(effect) = self.input_sent.get() {
                    Reply::Error("verb_unconfirmed", format!("erro: {verb} não confirmou em {ms} ms; {effect}{}", stuck_at(stuck.as_deref())))
                } else {
                    Reply::Error("verb_unconfirmed", format!("erro: {}", stuck_text(verb, ms, stuck.as_deref())))
                })
            }
        };
        self.dialog_signal.borrow_mut().take();
        match (result, self.failure.take()) {
            (Ok(Reply::Text(text)), Some(code)) => Reply::Error(code, text),
            (Ok(reply), _) => reply,
            (Err(e), _) if self.page.closed() => Reply::Error("cdp_closed", format!("erro: {e}")),
            (Err(e), _) => Reply::Text(format!("erro: {e}")),
        }
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
        self.call(&format!("{domain}.enable"), json!({})).await.map_err(|e| format!("{domain}.enable falhou: {e}"))?;
        let mut s = self.state.borrow_mut();
        s.enabled.insert(domain);
        if domain == "Network" { s.last_network = Instant::now(); }
        Ok(())
    }

    async fn frame(&self) {
        let _ = within(&self.page, 500, self.call("Runtime.evaluate", json!({"expression": FRAME, "awaitPromise": true}))).await;
    }

    async fn value(&self, expression: &str) -> Result<Value, String> {
        let r = self.call("Runtime.evaluate", json!({"expression": expression, "returnByValue": true})).await?;
        Ok(r["result"]["value"].clone())
    }

    /// Árvore inteira (`wait @eN` usa só as refs).
    async fn current_url(&self) -> Result<String, String> {
        let h = self.call("Page.getNavigationHistory", json!({})).await?;
        let index = h["currentIndex"].as_u64().unwrap_or(0) as usize;
        match h["entries"].get(index).and_then(|e| e["url"].as_str()) {
            Some(url) => Ok(format!("ok: {}", Value::String(url.to_owned()))),
            None => self.eval("location.href").await,
        }
    }

    async fn snapshot(&self) -> Result<Vec<String>, String> { self.tree(false).await }

    async fn tree(&self, refs_only: bool) -> Result<Vec<String>, String> {
        self.enable("Accessibility").await?;
        let mut tree = self.call("Accessibility.getFullAXTree", json!({})).await?;
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
        let _ = self.call("DOM.scrollIntoViewIfNeeded", json!({"backendNodeId": id})).await;
        let model = match self.call("DOM.getBoxModel", json!({"backendNodeId": id})).await {
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
        let _ = self.call("DOM.scrollIntoViewIfNeeded", json!({"backendNodeId": id})).await;
        let name = self.call("Accessibility.getPartialAXTree", json!({"backendNodeId": id, "fetchRelatives": false})).await.ok()
            .and_then(|t| t["nodes"].as_array()?.iter().find_map(|n| n["name"]["value"].as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)));
        self.report(verb, name);
        if self.state.borrow().hidden { return; }
        let color = |a: f64| json!({"r": 124, "g": 135, "b": 232, "a": a});
        let shown = async {
            self.enable("DOM").await?;
            self.enable("Overlay").await?;
            self.call("Overlay.highlightNode", json!({"backendNodeId": id,
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
        if self.state.borrow().enabled.contains("Overlay") { let _ = self.call("Overlay.hideHighlight", json!({})).await; }
    }

    /// Ouvinte em captura prova que o EVENTO chegou ao documento. `pointerdown` antes de `mousedown`: um
    /// `preventDefault` no primeiro suprime o segundo (combobox do Radix).
    async fn arrived(&self, types: &[&str], fire: impl AsyncFn() -> Result<(), String>) -> Result<bool, String> {
        let arm = format!("(()=>{{window.__hangarSonda=0;const ts={};const f=()=>{{window.__hangarSonda=1;ts.forEach(t=>removeEventListener(t,f,true))}};ts.forEach(t=>addEventListener(t,f,true));return 1}})()",
            serde_json::to_string(types).unwrap_or_default());
        self.call("Runtime.evaluate", json!({"expression": arm})).await.map_err(|e| format!("sonda nao armada: {e}"))?;
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

    /// Alvo que não é `@eN` (texto visível ou seletor) vira ref com o nó achado agora; `Err` é a frase da recusa.
    async fn resolve(&self, verb: &str, r: &str) -> Result<Result<(), String>, String> {
        if r.is_empty() || is_ref(r) { return Ok(Ok(())); }
        let expression = format!("{FIND}({})", serde_json::to_string(r).unwrap_or_default());
        let found = self.call("Runtime.evaluate", json!({"expression": expression})).await?;
        if let Some(e) = exception(&found) { return Ok(Err(format!("erro: {verb}: {e}"))); }
        let result = &found["result"];
        if result["subtype"] == "node" && let Some(object) = result["objectId"].as_str() {
            let node = self.call("DOM.describeNode", json!({"objectId": object})).await?;
            let Some(id) = node["node"]["backendNodeId"].as_i64() else {
                return Ok(Err(self.failed("target_missing", format!("erro: {verb}: o elemento de “{r}” sumiu antes da ação; tente de novo"))));
            };
            self.state.borrow_mut().refs.insert(r.to_owned(), id);
            return Ok(Ok(()));
        }
        let said = result["value"].as_str().unwrap_or("");
        Ok(Err(if said == "nenhum" {
            self.failed("target_missing", format!("erro: {verb}: nenhum elemento visível com “{r}” (aceita @eN do snapshot, texto visível ou css=seletor)"))
        } else if let Some(rest) = said.strip_prefix("varios:") {
            let (n, list) = rest.split_once(':').unwrap_or((rest, ""));
            self.failed("target_ambiguous", format!("erro: {verb}: “{r}” casa com {n} elementos visíveis ({list}): use o @eN do snapshot --refs ou um texto mais exato"))
        } else if !said.is_empty() {
            format!("erro: {verb}: {said}")
        } else {
            format!("erro: {verb}: a busca de “{r}” voltou sem resposta da página")
        }))
    }

    /// Começa a contar as mudanças da página antes da ação; devolve as navegações até aqui e se armou.
    async fn arm_quiet(&self, net: bool) -> (u64, bool) {
        let before = self.state.borrow().navs;
        self.watching.set(true);
        let armed = matches!(within(&self.page, 1_000, self.value(&format!("{QUIET_ARM}({net})"))).await, Some(Ok(v)) if v == 1);
        (before, armed)
    }

    async fn drop_quiet(&self) {
        if matches!(within(&self.page, 1_000, self.value(QUIET_DROP)).await, Some(Ok(_))) { self.watching.set(false); }
    }

    /// Depois da ação: espera o DOM e a rede da página pararem (`min_ms` sem nada = volta), ou o documento novo quando
    /// ela navegou. O texto é o que se acrescenta à resposta: só diz algo quando a espera não confirmou.
    async fn calm(&self, verb: &str, (before, armed): (u64, bool), min_ms: u64) -> String {
        let mut said = String::new();
        if !armed {
            // O armar pode ter rodado depois do prazo: não deixa o observador nem o embrulho na página.
            self.drop_quiet().await;
            said = " (não deu para acompanhar a página depois da ação: confira com wait --text ou snapshot)".into();
        } else if !self.state.borrow().navigated {
            let js = format!("new Promise(r=>{{const s=window.__hangarQuiet;if(!s)return r('sem');const t0=performance.now();const end=w=>{{s.drop();r(w)}};const tick=()=>{{const now=performance.now(),t=now-t0;if(s.leaving)return end('leaving');if(s.n===0&&t>={min_ms})return end('ok');if(s.p<=0&&s.n>0&&now-s.t>=300)return end('ok');if(s.p<=0&&t>={QUIET_DOM_MS})return end('mudando');if(t>={QUIET_MAX_MS})return end(s.p>0?'rede':'mudando');setTimeout(tick,50)}};tick()}})");
            let answer = within(&self.page, QUIET_MAX_MS + 1_000, self.call("Runtime.evaluate", json!({"expression": js, "awaitPromise": true, "returnByValue": true}))).await;
            said = match answer {
                Some(Ok(r)) => match { self.watching.set(false); r["result"]["value"].as_str() } {
                    Some("ok") => String::new(),
                    Some("mudando") => " (a página ainda mudava; para esperar algo, wait --text)".into(),
                    Some("rede") => format!(" (a página ainda esperava a rede depois de {} s)", QUIET_MAX_MS / 1000),
                    Some("leaving") => self.leaving(before).await,
                    // Documento trocado no meio: a navegação abaixo diz.
                    _ => String::new(),
                },
                // Contexto destruído = o documento trocou; o resto não confirmou a espera.
                Some(Err(e)) if e.contains("context") || e.contains("navigated") => String::new(),
                Some(Err(e)) => format!(" (a espera da página falhou: {e})"),
                None => { self.drop_quiet().await; " (a página não respondeu à espera)".into() }
            };
        }
        self.after_navigation_of(verb, before, said).await
    }

    /// O que a ação fez de navegação desde `before`, acrescentado a `said`.
    async fn after_navigation_of(&self, verb: &str, before: u64, said: String) -> String {
        if self.state.borrow().navs == before { return said; }
        // Rota de SPA não troca o documento: as refs seguem valendo, e não há carga a esperar.
        if !self.state.borrow().navigated {
            return match self.value("location.href").await.ok().and_then(|v| v.as_str().map(str::to_owned)) {
                Some(url) => format!(" — a página foi para {}{said}", super::server::without_token(&url)),
                None => format!(" — a página mudou de rota{said}"),
            };
        }
        let settled = within(&self.page, NAV_IN_ACTION_MS, self.settle(verb, before)).await;
        // A carga que não terminou vira nota da ação que já deu certo, não a recusa dela.
        self.failure.set(None);
        match settled {
            Some(Ok(t)) if t.starts_with("ok:") => format!(" — a página navegou para {}", super::server::without_token(t.trim_start_matches(&format!("ok: {verb} ")))),
            Some(Ok(t)) => format!(" — a página navegou, mas {}", t.trim_start_matches("erro: ")),
            Some(Err(e)) => format!(" — a página navegou, mas a carga não foi conferida ({e})"),
            None => format!(" — a página está navegando e não terminou de carregar em {} s: confira com wait --url", NAV_IN_ACTION_MS / 1000),
        }
    }

    /// A página avisou que vai sair (link ou formulário com resposta lenta): espera a navegação começar, pouco, porque
    /// download e saída cancelada também avisam e nunca navegam.
    async fn leaving(&self, before: u64) -> String {
        let mut waited = 0;
        while waited < LEAVE_MS {
            if self.state.borrow().navs != before { return String::new(); }
            self.page.sleep(POLL_MS).await;
            waited += POLL_MS;
        }
        format!(" (a página avisou que ia sair e ainda não navegou em {} s: servidor lento, download ou saída cancelada; confira com wait --url)", LEAVE_MS / 1000)
    }

    async fn page_text(&self) -> Result<String, String> {
        let v = self.value(TEXT_ALL).await?;
        let text = v["t"].as_str().unwrap_or("").to_owned();
        Ok(match v["cross"].as_u64().unwrap_or(0) { 0 => text, n => format!("{text}\n{}", cross_note(n, "text")) })
    }

    async fn click(&self, r: &str) -> Result<String, String> { self.click_as("click", r).await }

    async fn click_as(&self, verb: &str, r: &str) -> Result<String, String> {
        if let Err(why) = self.resolve(verb, r).await? { return Ok(why); }
        self.mark(verb, r).await;
        if let Some(why) = self.blocked() { self.unmark().await; return Ok(why.into()); }
        let Some((x, y)) = self.measured(r).await? else { return Ok(self.no_ref(r)) };
        // A medida esperou a página: o modo de escolha pode ter ligado nesse meio-tempo.
        if let Some(why) = self.blocked() { self.unmark().await; return Ok(why.into()); }
        let base = |t: &str| json!({"type": t, "x": x, "y": y, "button": "left", "clickCount": 1});
        // O `fill` digita logo depois: esperar a página só no clique de verdade.
        let quiet = if verb == "click" { Some(self.arm_quiet(true).await) } else { None };
        let ok = self.arrived(&["pointerdown", "mousedown"], async || {
            // Antes de apertar: o pedido pode chegar à página mesmo sem a resposta voltar.
            *self.pressed.borrow_mut() = Some(("Input.dispatchMouseEvent", base("mouseReleased")));
            self.call("Input.dispatchMouseEvent", base("mousePressed")).await?;
            let released = self.call("Input.dispatchMouseEvent", base("mouseReleased")).await;
            self.pressed.borrow_mut().take();
            released.map(|_| ())
        }).await;
        let ok = match ok {
            Ok(true) => true,
            other => { if quiet.is_some() { self.drop_quiet().await; } other? }
        };
        if !ok { return Ok(format!("erro: click {r}: o evento nao chegou na pagina (aba escondida sem quadro) — tire um shot e repita")); }
        let after = match quiet { Some(q) => self.calm(verb, q, 150).await, None => String::new() };
        Ok(format!("ok: click {r}{after}"))
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
        self.call("Input.dispatchKeyEvent", json!({"type": "keyDown", "commands": ["selectAll"]})).await?;
        // Antes de mandar: o texto pode chegar à página mesmo sem a resposta voltar.
        self.input_sent.set(Some(TEXT_SENT));
        self.call("Input.insertText", json!({"text": text})).await?;
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
        // Na vez do verbo, logo antes do clique: a leitura que o servidor de lá fez pode já ser de outra página.
        if let Some(expected) = secret.host.as_deref() {
            let now = self.current_url().await?;
            let url = now.strip_prefix("ok: ").and_then(|v| serde_json::from_str::<String>(v).ok()).unwrap_or_default();
            let host = page_host(&url);
            if host.as_deref() != Some(expected) {
                return Ok(format!("erro: erro_cofre_dominio_diferente: a pagina saiu de {expected} para {}", host.unwrap_or(url)));
            }
        }
        let clicked = self.click_as("fill-secret", r).await?;
        if clicked.starts_with("erro:") { return Ok(clicked); }
        if let Some(why) = self.blocked() { return Ok(why.into()); }
        if let Some(focus) = self.not_editable().await? {
            return Ok(format!("erro: fill-secret {r}: o clique nao deixou um campo de texto com foco (foco em {focus}) — rode snapshot"));
        }
        let Some(id) = self.state.borrow().refs.get(r).copied() else { return Ok(self.no_ref(r)) };
        self.enable("DOM").await?;
        let node = self.call("DOM.resolveNode", json!({"backendNodeId": id})).await?;
        let object = node["object"]["objectId"].as_str().ok_or("fill-secret: o alvo nao tem objeto na pagina")?;
        let found = self.call("Runtime.callFunctionOn", json!({"objectId": object, "functionDeclaration": TARGET_FOCUS, "returnByValue": true})).await?;
        match returned(&found)?.as_str() {
            Some("senha") => {}
            Some("campo") if !password => {}
            Some("campo") => return Ok(format!("erro: erro_cofre_campo_nao_senha: {r} nao e um campo de senha (input type=password)")),
            _ => return Ok(format!("erro: fill-secret {r}: o foco nao ficou no campo da ref — rode snapshot")),
        }
        // shortcut: a cópia do valor no JSON do CDP não é zerada; trocar se o transporte passar a guardar os pedidos.
        let fill = json!({"objectId": object, "functionDeclaration": FILL_SECRET, "returnByValue": true,
            "arguments": [{"value": secret.as_str()}, {"value": secret.host.as_deref()}, {"value": password}]});
        let expected = secret.host.clone().unwrap_or_default();
        // Registrada antes de digitar: se o envio falhar depois de o texto chegar à página, o snapshot já a esconde.
        if password {
            let mut s = self.state.borrow_mut();
            s.marked.insert(id);
            if !s.secrets.iter().any(|known| known.as_str() == secret.as_str()) {
                let mut secret = secret;
                secret.forms = mask_forms(secret.as_str());
                s.secrets.push(secret);
            }
        }
        self.secret_sent.set(true);
        let typed = self.call("Runtime.callFunctionOn", fill).await?;
        let said = returned(&typed)?.as_str().unwrap_or_default().to_owned();
        // O `secret_sent` e a máscara ficam como estão (conservadores); só o texto diz o que de fato aconteceu.
        match said.as_str() {
            "ok" => {}
            // Nunca o valor nem o tamanho dele: diria quanto da senha a página cortou.
            "diferente" => return Ok("erro: erro_cofre_incerto: o campo não ficou com o valor (pode ter sido alterado pela página)".into()),
            "naoapagou" => return Ok(format!("erro: fill-secret {r}: recusado: campo não suportado (não deu para apagar o que havia nele) — o valor não foi digitado")),
            "campo" => return Ok(format!("erro: erro_cofre_campo_nao_senha: {r} nao e um campo de senha (input type=password) — o valor não foi digitado")),
            "recusado" => return Ok(format!("erro: fill-secret {r}: a pagina nao aceitou o texto no campo (o campo pode ter sido apagado) — o valor não foi digitado")),
            other => match other.strip_prefix("dominio:") {
                Some(host) => return Ok(format!("erro: erro_cofre_dominio_diferente: a pagina saiu de {expected} para {host} — o valor não foi digitado")),
                None => return Ok(format!("erro: fill-secret {r}: o foco saiu do campo antes de digitar — o valor não foi digitado; rode snapshot")),
            },
        }
        self.frame().await;
        Ok(format!("ok: fill-secret {r}"))
    }

    async fn type_text(&self, text: &str) -> Result<String, String> {
        if let Some(focus) = self.not_editable().await? {
            return Ok(format!("erro: type: nenhum campo de texto com foco (foco em {focus}) — use fill @eN, ou click no campo antes"));
        }
        self.input_sent.set(Some(TEXT_SENT));
        self.call("Input.insertText", json!({"text": text})).await?;
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
        let quiet = self.arm_quiet(true).await;
        let ok = self.arrived(&["keydown"], async || {
            let mut d = down.clone();
            d["type"] = "keyDown".into();
            let mut u = up.clone();
            u["type"] = "keyUp".into();
            // Antes de apertar, como no clique: a tecla pode chegar à página mesmo sem a resposta voltar.
            *self.pressed.borrow_mut() = Some(("Input.dispatchKeyEvent", u.clone()));
            self.input_sent.set(Some(KEY_SENT));
            self.call("Input.dispatchKeyEvent", d).await?;
            let released = self.call("Input.dispatchKeyEvent", u).await;
            self.pressed.borrow_mut().take();
            released.map(|_| ())
        }).await;
        let ok = match ok {
            Ok(true) => true,
            other => { self.drop_quiet().await; other? }
        };
        Ok(if ok { format!("ok: press {key}{}", self.calm("press", quiet, 150).await) }
        else if open_list { format!("ok: press {key} (foi para a lista aberta do select, que nao avisa a pagina; confira com snapshot)") }
        else { format!("erro: press {key}: o evento nao chegou na pagina (aba escondida sem quadro) — tire um shot e repita") })
    }

    async fn hover(&self, r: &str) -> Result<String, String> {
        if let Err(why) = self.resolve("hover", r).await? { return Ok(why); }
        self.mark("hover", r).await;
        if let Some(why) = self.blocked() { self.unmark().await; return Ok(why.into()); }
        let Some((x, y)) = self.measured(r).await? else { return Ok(self.no_ref(r)) };
        self.call("Input.dispatchMouseEvent", json!({"type": "mouseMoved", "x": x, "y": y})).await?;
        self.frame().await;
        Ok(format!("ok: hover {r}"))
    }

    async fn eval(&self, js: &str) -> Result<String, String> {
        // Só a navegação: esperar o DOM atrasaria todo eval de leitura numa página animada.
        let before = self.state.borrow().navs;
        let answer = match self.eval_raw(js).await {
            // O js navegou e o documento sumiu antes de o valor voltar: é a navegação, não uma falha.
            Err(e) if e.contains("Execution context was destroyed") || e.contains("Inspected target navigated") => "ok: undefined".to_owned(),
            Ok(answer) if answer.contains("Execution context was destroyed") || answer.contains("Inspected target navigated") => "ok: undefined".to_owned(),
            other => other?,
        };
        if !answer.starts_with("ok:") { return Ok(answer); }
        Ok(format!("{answer}{}", self.after_navigation_of("eval", before, String::new()).await))
    }

    async fn eval_raw(&self, js: &str) -> Result<String, String> {
        let wrapped = format!("(async()=>{{const v=({js});await {FRAME};return v}})()");
        let run = |params: Value| within(&self.page, WAIT_MS, self.call("Runtime.evaluate", params));
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
            return Ok(if ms >= 1000 {
                format!("ok: wait {target}ms (click, press, goto e reload já esperam a página assentar; para esperar algo, wait --text, --url ou @eN)")
            } else { format!("ok: wait {target}ms") });
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
        // Iframes do mesmo site entram; os de outro site só são contados, para a recusa dizer.
        let text_js = format!("(()=>{{const n={};const out=[];let cross=0;const shown=e=>!e.checkVisibility||e.checkVisibility({{checkOpacity:true,checkVisibilityCSS:true}});const walk=d=>{{if(!d.body||!shown(d.body))return;out.push(d.body.innerText);out.push([...d.querySelectorAll('input[type=submit],input[type=button],input[type=reset]')].filter(shown).map(e=>e.value).join('\\n'));for(const f of d.querySelectorAll('iframe,frame')){{if(!shown(f))continue;let c=null;try{{c=f.contentDocument}}catch(e){{}}if(c&&c.body)walk(c);else if(!c)cross++}}}};walk(document);return {{hit:out.join('\\n').toLowerCase().includes(n.toLowerCase()),cross}}}})()",
            serde_json::to_string(&needle).unwrap_or_default());
        let cross = Cell::new(0u64);
        // Conta o tempo das checagens além das pausas: página presa num laço de JS não responde ao
        // evaluate, e sem teto o verbo seguraria a vez de todos os outros.
        let mut waited = 0;
        while waited < limit {
            let check = async {
                Ok::<_, String>(match target {
                    t if t.starts_with('@') => { self.snapshot().await?; self.state.borrow().refs.contains_key(t) }
                    "--url" => self.value("location.href").await?.as_str().is_some_and(|u| u.contains(needle.as_str())),
                    "--text" => {
                        let v = self.value(&text_js).await?;
                        cross.set(v["cross"].as_u64().unwrap_or(0));
                        v["hit"].as_bool() == Some(true)
                    }
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
        Ok(self.failed("wait_timeout", match cross.get() {
            0 => format!("erro: wait {joined} nao aconteceu em {limit}ms"),
            n => format!("erro: wait {joined} nao aconteceu em {limit}ms {}", cross_note(n, "wait --text")),
        }))
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
        Ok(self.failed("wait_timeout", format!("erro: {verb}: a pagina nao terminou de carregar em {WAIT_MS}ms")))
    }

    async fn goto(&self, url: &str) -> Result<String, String> {
        let url = url.trim();
        if url.is_empty() { return Ok("erro: goto precisa de uma url".into()); }
        let url = if url.contains("://") { url.to_owned() } else { format!("http://{url}") };
        let before = self.state.borrow().navs;
        let r = self.call("Page.navigate", json!({"url": url})).await?;
        if let Some(e) = r["errorText"].as_str().filter(|e| !e.is_empty()) { return Ok(format!("erro: goto {url}: {e}")); }
        self.settle("goto", before).await
    }

    async fn back(&self) -> Result<String, String> {
        let h = self.call("Page.getNavigationHistory", json!({})).await?;
        let index = h["currentIndex"].as_i64().unwrap_or(0);
        let Some(entry) = usize::try_from(index - 1).ok().and_then(|i| h["entries"].get(i)) else {
            return Ok("erro: back: nao ha pagina anterior".into());
        };
        let before = self.state.borrow().navs;
        self.call("Page.navigateToHistoryEntry", json!({"entryId": entry["id"]})).await?;
        self.settle("back", before).await
    }

    async fn reload(&self) -> Result<String, String> {
        let before = self.state.borrow().navs;
        self.call("Page.reload", json!({})).await?;
        self.settle("reload", before).await
    }

    async fn apply_theme(&self) -> Result<(), String> {
        let value = match self.state.borrow().theme { "claro" => "light", "escuro" => "dark", _ => "" };
        let features = if value.is_empty() { json!([]) } else { json!([{"name": "prefers-color-scheme", "value": value}]) };
        self.call("Emulation.setEmulatedMedia", json!({"features": features})).await.map(|_| ())
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
            self.call("Emulation.setDeviceMetricsOverride", metrics(390, 844, 2., true)).await?;
            self.call("Emulation.setTouchEmulationEnabled", json!({"enabled": true, "maxTouchPoints": 5})).await?;
            self.call("Emulation.setUserAgentOverride", json!({"userAgent": MOBILE_UA})).await?;
            return Ok(());
        }
        self.call("Emulation.setTouchEmulationEnabled", json!({"enabled": false})).await?;
        // Sem troca o Chromium do Linux se apresenta como "HeadlessChrome", e Google, GitHub e afins recusam o login.
        let version = self.call("Browser.getVersion", json!({})).await.unwrap_or_else(|e| {
            eprintln!("[nav] Browser.getVersion falhou ({e}): o User-Agent fica o do Chromium sem janela");
            Default::default()
        });
        self.call("Emulation.setUserAgentOverride", desktop_agent(&version)).await?;
        match layout {
            Layout::Custom(w, h) => self.call("Emulation.setDeviceMetricsOverride", metrics(w, h, 1., false)).await?,
            _ if self.state.borrow().hidden => self.call("Emulation.setDeviceMetricsOverride", metrics(HIDDEN_VIEWPORT.0, HIDDEN_VIEWPORT.1, 1., false)).await?,
            // O Chromium sem janela não segue a janela: limpar deixava a página no tamanho velho, e o quadro esticado.
            _ if let Some((w, h, scale)) = self.page.panel_size() => self.call("Emulation.setDeviceMetricsOverride", metrics(w, h, scale, false)).await?,
            _ => {
                // WebView2: a página segue a janela de verdade.
                self.call("Emulation.clearDeviceMetricsOverride", json!({})).await?;
                // Largura e altura 0 = o tamanho segue a janela; só a escala é emulada. Depois de limpar: sem isso o
                // 0×0 mantém o tamanho da emulação anterior (a da página escondida).
                let scale = self.state.borrow().scale;
                match scale { Some(scale) => self.call("Emulation.setDeviceMetricsOverride", metrics(0, 0, scale, false)).await?, None => json!({}) }
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
        let Some(r) = within(&self.page, SHOT_MS, self.call("Page.captureScreenshot", params)).await else {
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

/// Teto do verbo: `wait` tem o próprio (`--timeout` ou a pausa em ms) mais a folga; a navegação, nenhum além do
/// carregamento que ela já espera; os outros, `VERB_MS`. Com `left_ms` (o que sobra do prazo de quem espera), nunca
/// além dele, menos a folga (no máximo um quarto do que sobra). `None` = sem teto.
fn verb_ceiling(verb: &str, args: &[String], left_ms: Option<u64>) -> Option<u64> {
    let own = match (verb, split_timeout(args)) {
        ("wait", Ok((rest, limit))) => Some(rest.first().and_then(|t| t.parse::<u64>().ok()).unwrap_or(limit).saturating_add(VERB_MARGIN_MS)),
        // Página lenta para abrir não é página presa: o teto padrão cortaria uma navegação que antes terminava.
        ("goto" | "back" | "reload" | "open", _) => None,
        _ => Some(VERB_MS),
    };
    let cap = left_ms.map(|d| d - VERB_MARGIN_MS.min(d / 4));
    match (own, cap) { (Some(own), Some(cap)) => Some(own.min(cap)), (own, cap) => own.or(cap) }
}

fn is_ref(r: &str) -> bool { r.len() > 2 && r.starts_with("@e") && r[2..].bytes().all(|b| b.is_ascii_digit()) }

fn cross_note(n: u64, verb: &str) -> String {
    if n == 1 { format!("[1 iframe de outro site fica fora do {verb}: veja com shot]") }
    else { format!("[{n} iframes de outro site ficam fora do {verb}: veja com shot]") }
}

fn stuck_at(method: Option<&str>) -> String { method.map(|m| format!(" (preso em {m})")).unwrap_or_default() }

fn stuck_text(verb: &str, ms: u64, method: Option<&str>) -> String {
    format!("{verb} não respondeu em {ms} ms{}", stuck_at(method))
}

/// Falha no tamanho ou na escala que o painel pediu: ninguém lê resposta, e aberto pelo lançador o stderr vai pro nada.
fn note_panel(line: &str) {
    eprintln!("{line}");
    if !cfg!(test) { crate::log_line(line) }
}

/// Uma linha no log do app por verbo preso: só verbo, método e teto, nunca conteúdo da página.
fn note_stuck(verb: &str, method: Option<&str>, ms: u64) {
    let line = format!("nav: verb_stuck verb={verb} method={} ceiling_ms={ms}", method.unwrap_or("-"));
    if cfg!(test) { eprintln!("{line}") } else { crate::log_line(&line) }
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
    #[cfg(target_os = "linux")]
    fn closed(&self) -> bool { !self.cdp.browser().alive() }
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

    /// `stuck`: método (ou função do `callFunctionOn`) que nunca responde, como página presa num laço de JS.
    /// `panel`/`held`: o lado do motor do painel, que guarda o tamanho aplicado e obedece ao layout pedido.
    struct Fake { calls: RefCell<Vec<(String, Value)>>, answer: Box<dyn Fn(&str, &Value) -> Result<Value, String>>, stuck: &'static str,
        panel: Cell<Option<(u32, u32, f32)>>, held: Cell<bool>, gone: Cell<bool> }
    /// A espera depois da ação responde como numa página que assentou, salvo quando o teste responde ela mesma.
    fn fake(answer: impl Fn(&str, &Value) -> Result<Value, String> + 'static, stuck: &'static str) -> Fake {
        let answer = move |m: &str, p: &Value| {
            let said = answer(m, p)?;
            let quiet = p["expression"].as_str().filter(|e| e.contains("__hangarQuiet"));
            Ok(match quiet {
                Some(e) if said == json!({}) || said == json!({"result": {}}) =>
                    json!({"result": {"value": if e.starts_with("new Promise") { json!("ok") } else { json!(1) }}}),
                _ => said,
            })
        };
        Fake { calls: RefCell::default(), answer: Box::new(answer), stuck, panel: Cell::new(None), held: Cell::new(false), gone: Cell::new(false) }
    }
    impl Page for Fake {
        fn panel_size(&self) -> Option<(u32, u32, f32)> { self.panel.get() }
        fn hold_size(&self, held: bool) { self.held.set(held) }
        fn closed(&self) -> bool { self.gone.get() }
        fn call(&self, method: &str, params: Value) -> impl Future<Output = Result<Value, String>> {
            self.calls.borrow_mut().push((method.into(), params.clone()));
            // `stuck` também casa a função de um `Runtime.callFunctionOn`: só aquela chamada fica presa.
            let stuck = method == self.stuck || params["functionDeclaration"].as_str() == Some(self.stuck);
            if stuck { Either::Left(std::future::pending()) } else { Either::Right(std::future::ready((self.answer)(method, &params))) }
        }
        fn sleep(&self, _: u64) -> impl Future<Output = ()> { std::future::ready(()) }
    }
    fn ctl(answer: impl Fn(&str, &Value) -> Result<Value, String> + 'static) -> Controller<Fake> {
        Controller::new(fake(answer, ""), false)
    }
    fn text(r: Reply) -> String { match r { Reply::Text(t) | Reply::Error(_, t) => t, Reply::Png(_) => "png".into() } }
    fn code(r: &Reply) -> Option<&'static str> { match r { Reply::Error(code, _) => Some(code), _ => None } }
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
            Reply::Text(t) | Reply::Error(_, t) => panic!("{t}"),
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
            "Runtime.callFunctionOn" if p["functionDeclaration"] == FILL_SECRET => json!({"result": {"value": "ok"}}),
            "Runtime.callFunctionOn" => json!({"result": {"value": focus}}),
            "Runtime.evaluate" => match p["expression"].as_str().unwrap_or("") {
                e if e.starts_with("(typeof window.__hangarSonda") => json!({"result": {"value": 1}}),
                e if e.contains("a.shadowRoot.activeElement;if(!a") => json!({"result": {"value": ""}}),
                // O `eval` e o `text` leem o campo e devolvem o valor como a página o tem.
                e if e.contains("querySelector('input').value") => json!({"result": {"value": SECRET}}),
                e if e.contains("__hangarText") => json!({"result": {"value": {"t": SECRET, "cross": 0}}}),
                _ => json!({"result": {}}),
            },
            _ => json!({}),
        }
    }

    /// O valor saiu para a página: digitado no foco ou pela função que digita no campo.
    fn typed_secret(c: &Controller<Fake>) -> bool {
        c.page.calls.borrow().iter().any(|(m, p)| m == "Input.insertText" || p["functionDeclaration"] == FILL_SECRET)
    }

    fn fill_secret(c: &Controller<Fake>, kind: &str) -> String {
        block_on(c.run("snapshot", &[]));
        text(block_on(c.run_with("fill-secret", &s(&["@e1", kind]), Some(Secret::new(SECRET.into())))))
    }

    #[test]
    fn every_secret_filled_in_the_session_stays_masked() {
        let c = login_page("senha");
        block_on(c.run("snapshot", &[]));
        let secret = |i: usize| format!("S3nh@-distinta-{i}");
        for i in 0..9 {
            assert_eq!(text(block_on(c.run_with("fill-secret", &s(&["@e1", "password"]), Some(Secret::new(secret(i)))))), "ok: fill-secret @e1");
        }
        c.on_event("Runtime.consoleAPICalled", &json!({"type": "log", "args": [{"type": "string", "value": secret(0)}]}));
        assert!(!text(block_on(c.run("console", &[]))).contains("distinta"), "a primeira de nove segue escondida");
    }

    #[test]
    fn a_page_error_with_the_secret_reaches_the_panel_masked() {
        let c = login_page("senha");
        assert_eq!(fill_secret(&c, "password"), "ok: fill-secret @e1");
        let seen = std::rc::Rc::new(RefCell::new(None));
        let sink = seen.clone();
        c.on_page_error(move |error| *sink.borrow_mut() = error);
        c.on_event("Runtime.consoleAPICalled", &json!({"type": "error", "args": [{"type": "string", "value": format!("falhou com {SECRET}")}]}));
        let shown = seen.borrow().clone().expect("o erro chega ao painel");
        assert!(shown.contains("falhou com") && !shown.contains("S3nh"), "{shown}");
    }

    #[test]
    fn a_filled_secret_never_comes_back_in_snapshot_eval_or_text() {
        let c = login_page("senha");
        assert_eq!(fill_secret(&c, "password"), "ok: fill-secret @e1");
        let typed: Vec<Value> = c.page.calls.borrow().iter().filter(|(_, p)| p["functionDeclaration"] == FILL_SECRET).map(|(_, p)| p["arguments"].clone()).collect();
        assert_eq!(typed, vec![json!([{"value": SECRET}, {"value": null}, {"value": true}])]);
        assert!(!methods(&c).contains(&"Input.insertText".to_string()), "o valor nunca vai ao que estiver com foco");
        let snapshot = text(block_on(c.run("snapshot", &[])));
        assert_eq!(snapshot, "- RootWebArea \"Login\"\n  - textbox \"Senha\" [ref=@e1]", "o nó marcado sai sem value");
        assert_eq!(text(block_on(c.run("eval", &s(&["document.querySelector('input').value"])))), format!("ok: \"{}\"", crate::browser::preview_fmt::MASK));
        assert!(!text(block_on(c.run("text", &[]))).contains("S3nh"));
        c.on_event("Runtime.consoleAPICalled", &json!({"type": "log", "args": [{"type": "string", "value": SECRET}]}));
        assert!(!text(block_on(c.run("console", &[]))).contains("S3nh"));
    }

    #[test]
    fn a_failed_insert_still_hides_the_secret() {
        let c = ctl(|m, p| if p["functionDeclaration"] == FILL_SECRET { Err("caiu".into()) } else { Ok(login_answer("senha", m, p)) });
        assert!(!fill_secret(&c, "password").starts_with("ok:"));
        assert_eq!(text(block_on(c.run("snapshot", &[]))), "- RootWebArea \"Login\"\n  - textbox \"Senha\" [ref=@e1]");
        assert!(!text(block_on(c.run("text", &[]))).contains("S3nh"));
    }

    /// Pedido de outra máquina: o valor só entra se a página ainda é do domínio que o servidor de lá aprovou.
    #[test]
    fn a_secret_from_another_machine_only_goes_into_the_approved_domain() {
        let on = |url: &'static str| ctl(move |m, p| Ok(if m == "Page.getNavigationHistory" {
            json!({"currentIndex": 0, "entries": [{"url": url}]})
        } else { login_answer("senha", m, p) }));
        let fill = |c: &Controller<Fake>| {
            block_on(c.run("snapshot", &[]));
            text(block_on(c.run_with("fill-secret", &s(&["@e1", "password"]), Some(Secret::new(SECRET.into()).for_host("github.com".into())))))
        };
        let c = on("https://evil.io/login");
        let refused = fill(&c);
        assert!(refused.starts_with("erro: erro_cofre_dominio_diferente:") && refused.contains("evil.io") && !refused.contains("S3nh"), "{refused}");
        assert!(!methods(&c).iter().any(|m| m == "Input.insertText" || m == "Input.dispatchMouseEvent"), "nada clicado nem digitado noutro domínio");
        assert!(c.state.borrow().secrets.is_empty());
        assert_eq!(fill(&on("https://GitHub.com:443/login")), "ok: fill-secret @e1");
        assert_eq!(page_host("http://user:pw@exemplo.com.br./a").as_deref(), Some("exemplo.com.br"));
        assert_eq!(page_host("about:blank"), None);
    }

    /// A página troca de domínio depois de o alvo confirmar o foco e antes de o valor entrar. O falso confere o host
    /// como a função do campo confere: o valor nunca vai como digitação ao que estiver com foco, e o outro domínio
    /// responde com o nome dele.
    #[test]
    fn a_navigation_between_the_check_and_the_typing_never_types_the_secret() {
        let moved = std::rc::Rc::new(Cell::new(false));
        let page = moved.clone();
        let c = ctl(move |m, p| {
            let host = if page.get() { "evil.io" } else { "github.com" };
            Ok(match m {
                "Page.getNavigationHistory" => json!({"currentIndex": 0, "entries": [{"url": format!("https://{host}/login")}]}),
                "Runtime.callFunctionOn" if p["functionDeclaration"] == TARGET_FOCUS => { page.set(true); json!({"result": {"value": "senha"}}) }
                "Runtime.callFunctionOn" if p["functionDeclaration"] == FILL_SECRET => {
                    let said = if p["arguments"][1]["value"] == host { "ok".to_owned() } else { format!("dominio:{host}") };
                    json!({"result": {"value": said}})
                }
                _ => login_answer("senha", m, p),
            })
        });
        block_on(c.run("snapshot", &[]));
        let said = text(block_on(c.run_with("fill-secret", &s(&["@e1", "password"]), Some(Secret::new(SECRET.into()).for_host("github.com".into())))));
        assert!(moved.get(), "a página navegou depois da conferência de antes do clique");
        assert_eq!(said, "erro: erro_cofre_dominio_diferente: a pagina saiu de github.com para evil.io — o valor não foi digitado");
        assert!(!c.page.calls.borrow().iter().any(|(m, p)| m == "Input.insertText" || p["text"] == SECRET), "nada digitado no foco do documento novo");
    }

    /// Página cuja função de preenchimento responde `said`, com o resto da página de login.
    fn fill_answers(said: &'static str) -> Controller<Fake> {
        ctl(move |m, p| Ok(if p["functionDeclaration"] == FILL_SECRET { json!({"result": {"value": said}}) } else { login_answer("senha", m, p) }))
    }

    /// Input controlado que desfaz, `maxlength` que corta, `beforeinput` que barra: o campo releu outro valor. A
    /// resposta não é `ok`, não traz o valor nem o tamanho dele, e a senha segue escondida.
    #[test]
    fn a_field_left_with_another_value_is_never_reported_as_filled() {
        let c = fill_answers("diferente");
        let said = fill_secret(&c, "password");
        assert_eq!(said, "erro: erro_cofre_incerto: o campo não ficou com o valor (pode ter sido alterado pela página)");
        assert!(!said.contains(&SECRET.len().to_string()));
        assert_eq!(text(block_on(c.run("snapshot", &[]))), "- RootWebArea \"Login\"\n  - textbox \"Senha\" [ref=@e1]");
        assert!(!text(block_on(c.run("text", &[]))).contains("S3nh"));
    }

    /// Campo que não deu para apagar é recusado antes de digitar; nenhuma recusa diz que o valor pode ter entrado.
    #[test]
    fn a_field_that_cannot_be_cleared_is_refused_and_refusals_never_claim_typing() {
        assert_eq!(fill_secret(&fill_answers("naoapagou"), "password"),
            "erro: fill-secret @e1: recusado: campo não suportado (não deu para apagar o que havia nele) — o valor não foi digitado");
        for said in ["naoapagou", "recusado", "fora", "campo", "dominio:evil.io"] {
            let reply = fill_secret(&fill_answers(said), "password");
            assert!(reply.starts_with("erro:") && reply.contains("o valor não foi digitado"), "{said}: {reply}");
            assert!(!reply.contains("pode ter sido digitado") && !reply.contains("S3nh"), "{said}: {reply}");
        }
    }

    /// O host aprovado chega de outra máquina como veio: maiúsculas e ponto final não fazem recusar a página certa.
    #[test]
    fn the_approved_host_is_compared_in_lowercase_without_the_trailing_dot() {
        let c = ctl(|m, p| Ok(if m == "Page.getNavigationHistory" {
            json!({"currentIndex": 0, "entries": [{"url": "https://github.com./login"}]})
        } else { login_answer("senha", m, p) }));
        block_on(c.run("snapshot", &[]));
        let said = text(block_on(c.run_with("fill-secret", &s(&["@e1", "password"]), Some(Secret::new(SECRET.into()).for_host("GitHub.COM.".into())))));
        assert_eq!(said, "ok: fill-secret @e1");
        let hosts: Vec<Value> = c.page.calls.borrow().iter().filter(|(_, p)| p["functionDeclaration"] == FILL_SECRET).map(|(_, p)| p["arguments"][1]["value"].clone()).collect();
        assert_eq!(hosts, vec![json!("github.com")], "a página compara com o host já normalizado");
        assert!(FILL_SECRET.contains("location.hostname.toLowerCase()"), "e normaliza o dela do mesmo jeito");
    }

    #[test]
    fn a_password_only_goes_into_a_password_field() {
        let c = login_page("campo");
        assert!(fill_secret(&c, "password").starts_with("erro: erro_cofre_campo_nao_senha:"));
        let c = login_page("fora");
        assert!(fill_secret(&c, "password").starts_with("erro: fill-secret @e1: o foco nao ficou"));
        assert!(!typed_secret(&c), "nada digitado fora do campo de senha");
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

fn finder(p: &Value) -> bool { p["expression"].as_str().is_some_and(|e| e.contains("__hangarTarget")) }

    #[test]
    fn click_by_visible_text_resolves_and_clicks() {
        let c = ctl(|m, p| Ok(match m {
            "Runtime.evaluate" if finder(p) => json!({"result": {"type": "object", "subtype": "node", "objectId": "o9"}}),
            "DOM.describeNode" => json!({"node": {"backendNodeId": 9}}),
            "DOM.getBoxModel" => json!({"model": {"content": [0, 0, 20, 0, 20, 10, 0, 10]}}),
            "Runtime.evaluate" if p["expression"].as_str().is_some_and(|e| e.starts_with("(typeof window.__hangarSonda")) => json!({"result": {"value": 1}}),
            _ => json!({}),
        }));
        assert_eq!(text(block_on(c.run("click", &s(&["Salvar"])))), "ok: click Salvar");
        let described: Vec<Value> = c.page.calls.borrow().iter().filter(|(m, _)| m == "DOM.describeNode").map(|(_, p)| p.clone()).collect();
        assert_eq!(described, vec![json!({"objectId": "o9"})]);
    }

    #[test]
    fn click_by_text_not_found_says_what_is_accepted() {
        let c = ctl(|m, p| Ok(if m == "Runtime.evaluate" && finder(p) { json!({"result": {"type": "string", "value": "nenhum"}}) } else { json!({}) }));
        assert_eq!(text(block_on(c.run("click", &s(&["Salvar"])))),
            "erro: click: nenhum elemento visível com “Salvar” (aceita @eN do snapshot, texto visível ou css=seletor)");
        assert!(!methods(&c).contains(&"Input.dispatchMouseEvent".to_string()));
    }

    #[test]
    fn click_by_text_with_many_matches_lists_them() {
        let c = ctl(|m, p| Ok(if m == "Runtime.evaluate" && finder(p) {
            json!({"result": {"type": "string", "value": "varios:3:button “Salvar”, a “Salvar”, span “Salvar”"}})
        } else { json!({}) }));
        assert_eq!(text(block_on(c.run("click", &s(&["Salvar"])))),
            "erro: click: “Salvar” casa com 3 elementos visíveis (button “Salvar”, a “Salvar”, span “Salvar”): use o @eN do snapshot --refs ou um texto mais exato");
    }

    fn quiet_answer(arm: Value, wait: &'static str) -> Controller<Fake> {
        ctl(move |m, p| Ok(match m {
            "Accessibility.getFullAXTree" => tree(),
            "DOM.getBoxModel" => json!({"model": {"content": [0, 0, 20, 0, 20, 10, 0, 10]}}),
            "Runtime.evaluate" => match p["expression"].as_str().unwrap_or("") {
                e if e.starts_with("(typeof window.__hangarSonda") => json!({"result": {"value": 1}}),
                e if e.starts_with("new Promise(r=>{const s=window.__hangarQuiet") => json!({"result": {"value": wait}}),
                e if e.contains("window.__hangarQuiet={") => arm.clone(),
                _ => json!({}),
            },
            _ => json!({}),
        }))
    }

    /// Resposta calada quando assentou: aviso em toda resposta seria lido como problema.
    #[test]
    fn click_says_when_the_page_did_not_settle() {
        let c = quiet_answer(json!({"result": {"value": 1}}), "mudando");
        block_on(c.run("snapshot", &[]));
        assert_eq!(text(block_on(c.run("click", &s(&["@e1"])))), "ok: click @e1 (a página ainda mudava; para esperar algo, wait --text)");
        let c = quiet_answer(json!({"result": {"value": 1}}), "rede");
        block_on(c.run("snapshot", &[]));
        assert_eq!(text(block_on(c.run("click", &s(&["@e1"])))), "ok: click @e1 (a página ainda esperava a rede depois de 3 s)");
        let c = quiet_answer(json!({"exceptionDetails": {"exception": {"description": "TypeError: x"}}}), "ok");
        block_on(c.run("snapshot", &[]));
        assert_eq!(text(block_on(c.run("click", &s(&["@e1"])))),
            "ok: click @e1 (não deu para acompanhar a página depois da ação: confira com wait --text ou snapshot)");
    }

    /// A página não pode ficar com `fetch` trocado nem com o observador ligado depois de um clique que falhou.
    #[test]
    fn a_click_that_never_arrives_drops_the_watch() {
        let c = ctl(|m, p| Ok(match m {
            "Accessibility.getFullAXTree" => tree(),
            "DOM.getBoxModel" => json!({"model": {"content": [0, 0, 20, 0, 20, 10, 0, 10]}}),
            "Runtime.evaluate" if p["expression"].as_str().is_some_and(|e| e.starts_with("(typeof window.__hangarSonda")) => json!({"result": {"value": 0}}),
            _ => json!({}),
        }));
        block_on(c.run("snapshot", &[]));
        assert!(text(block_on(c.run("click", &s(&["@e1"])))).starts_with("erro: click @e1: o evento nao chegou"));
        let dropped = c.page.calls.borrow().iter().any(|(_, p)| p["expression"].as_str() == Some(QUIET_DROP));
        assert!(dropped, "o fetch e o observador da página voltam ao que eram");
    }

    /// Alvo de várias palavras sem aspas é um alvo só: `click Excluir conta` nunca vira `click Excluir`.
    #[test]
    fn a_multi_word_target_is_searched_whole() {
        let c = ctl(|m, p| Ok(if m == "Runtime.evaluate" && finder(p) { json!({"result": {"type": "string", "value": "nenhum"}}) } else { json!({}) }));
        block_on(c.run("click", &s(&["Excluir", "conta"])));
        let asked: Vec<String> = c.page.calls.borrow().iter().filter(|(_, p)| finder(p))
            .map(|(_, p)| p["expression"].as_str().unwrap().to_owned()).collect();
        assert!(asked.iter().all(|e| e.ends_with("(\"Excluir conta\")")), "{asked:?}");
    }

    /// `eval` só de leitura não arma a espera do DOM: em página animada ele voltaria tarde e com aviso falso.
    #[test]
    fn eval_does_not_watch_the_dom() {
        let c = ctl(|_, _| Ok(json!({"result": {"value": 1}})));
        assert_eq!(text(block_on(c.run("eval", &s(&["1"])))), "ok: 1");
        assert!(!c.page.calls.borrow().iter().any(|(_, p)| p["expression"].as_str().is_some_and(|e| e.contains("__hangarQuiet"))));
    }

    /// O js que navega derruba o documento antes de o valor voltar: isso não é falha do eval.
    #[test]
    fn an_eval_that_navigates_is_not_an_error() {
        let c = ctl(|m, _| if m == "Runtime.evaluate" { Err("Execution context was destroyed.".into()) } else { Ok(json!({})) });
        assert_eq!(text(block_on(c.run("eval", &s(&["location.href='/x'"])))), "ok: undefined");
    }

    /// Diálogo aberto no meio do clique corta o verbo antes do `calm`: o verbo seguinte desfaz o que ficou na página.
    #[test]
    fn a_watch_left_by_a_cut_verb_is_dropped_by_the_next_one() {
        let c = quiet_answer(json!({"result": {"value": 1}}), "ok");
        block_on(c.arm_quiet(true));
        block_on(c.run("url", &[]));
        assert!(c.page.calls.borrow().iter().any(|(_, p)| p["expression"].as_str() == Some(QUIET_DROP)));
        assert!(!c.watching.get());
    }

    #[test]
    fn wait_with_fixed_time_points_to_the_signal() {
        let c = ctl(|_, _| Ok(json!({})));
        assert_eq!(text(block_on(c.run("wait", &s(&["3000"])))),
            "ok: wait 3000ms (click, press, goto e reload já esperam a página assentar; para esperar algo, wait --text, --url ou @eN)");
    }

    /// Iframe de outro site o navegador não deixa ler: contar diz ao agente que falta texto ali.
    #[test]
    fn text_reads_iframes_and_tells_the_cross_site_ones() {
        let c = ctl(|m, p| Ok(if m == "Runtime.evaluate" && p["expression"].as_str().is_some_and(|e| e.contains("__hangarText")) {
            json!({"result": {"value": {"t": "topo\ndentro", "cross": 1}}})
        } else { json!({}) }));
        assert_eq!(text(block_on(c.run("text", &[]))), "topo\ndentro\n[1 iframe de outro site fica fora do text: veja com shot]");
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
        let c = ctl(|_, p| Ok(match p["expression"].as_str().unwrap_or("") {
            e if e.contains("__hangarText") => json!({"result": {"value": {"t": "x", "cross": 0}}}),
            e if e.contains("__hangarQuiet") => json!({}),
            _ => json!({"result": {"value": "x"}}),
        }));
        let (a, b) = block_on(futures::future::join(c.run("eval", &s(&["1"])), c.run("text", &[])));
        assert_eq!((text(a), text(b)), ("ok: \"x\"".into(), "x".into()));
        let read: Vec<bool> = c.page.calls.borrow().iter().filter(|(m, _)| m == "Runtime.evaluate")
            .map(|(_, p)| p["expression"].as_str().is_some_and(|e| e.contains("__hangarText"))).collect();
        assert_eq!(read.iter().filter(|r| **r).count(), 1);
        assert_eq!(read.last(), Some(&true), "o text só começa depois do eval inteiro");
    }

    /// Página com o clique preso no CDP: o verbo seguinte precisa da vez de volta.
    fn stuck_click() -> Controller<Fake> {
        Controller::new(fake(|m, _| Ok(match m {
            "Accessibility.getFullAXTree" => tree(),
            "DOM.getBoxModel" => boxed(),
            "Page.getNavigationHistory" => json!({"currentIndex": 0, "entries": [{"url": "http://a/"}]}),
            _ => json!({}),
        }), "Input.dispatchMouseEvent"), false)
    }

    #[test]
    fn a_stuck_click_releases_the_turn_and_names_the_method() {
        let c = stuck_click();
        block_on(c.run("snapshot", &[]));
        assert_eq!(text(block_on(c.run("click", &s(&["@e1"])))), "erro: click não respondeu em 20000 ms (preso em Input.dispatchMouseEvent)");
        assert!(!c.busy(), "a vez voltou");
        assert_eq!(text(block_on(c.run("url", &[]))), "ok: \"http://a/\"");
    }

    fn in_ms(ms: u64) -> Instant { Instant::now() + std::time::Duration::from_millis(ms) }

    /// Cada recusa sai com o código da causa, decidido onde nasceu: é ele que vai no cabeçalho do `/cmd`.
    #[test]
    fn refusals_carry_the_code_of_their_cause() {
        let c = stuck_click();
        block_on(c.run("snapshot", &[]));
        assert_eq!(code(&block_on(c.run("hover", &s(&["@e9"])))), Some("ref_invalid"));
        assert_eq!(code(&block_on(c.run("click", &s(&["@e1"])))), Some("verb_unconfirmed"));
        assert_eq!(code(&block_on(c.run("url", &[]))), None, "resposta boa não leva código");
        c.set_dialog(Some(("confirm".into(), "Excluir?".into(), String::new())));
        assert_eq!(code(&block_on(c.run("click", &s(&["@e1"])))), Some("dialog_open"));

        let missing = ctl(|m, p| Ok(if m == "Runtime.evaluate" && finder(p) { json!({"result": {"type": "string", "value": "nenhum"}}) } else { json!({}) }));
        assert_eq!(code(&block_on(missing.run("click", &s(&["Salvar"])))), Some("target_missing"));
        let many = ctl(|m, p| Ok(if m == "Runtime.evaluate" && finder(p) { json!({"result": {"type": "string", "value": "varios:2:a, b"}}) } else { json!({}) }));
        assert_eq!(code(&block_on(many.run("hover", &s(&["Salvar"])))), Some("target_ambiguous"));
        let waiting = ctl(|_, _| Ok(json!({"result": {"value": "http://a/"}})));
        assert_eq!(code(&block_on(waiting.run("wait", &s(&["--url", "/done"])))), Some("wait_timeout"));
        assert_eq!(code(&block_on(waiting.run("goto", &s(&["http://b/"])))), Some("wait_timeout"), "carga que não termina");
    }

    #[test]
    fn a_cdp_error_after_the_browser_left_is_cdp_closed() {
        let c = ctl(|_, _| Err("chromium_exited: o Chromium fechou".into()));
        assert_eq!(code(&block_on(c.run("text", &[]))), None, "erro da página com o navegador vivo não é queda");
        c.page.gone.set(true);
        assert_eq!(code(&block_on(c.run("text", &[]))), Some("cdp_closed"));
    }

    /// O teto que a resposta do verbo cortado cita (`… em N ms …`).
    fn ceiling_in(said: &str) -> u64 {
        said.split(" em ").nth(1).and_then(|rest| rest.split(' ').next()).and_then(|n| n.parse().ok()).unwrap_or_else(|| panic!("{said}"))
    }

    #[test]
    fn a_shorter_deadline_bounds_the_verb_below_it() {
        let c = stuck_click();
        block_on(c.run("snapshot", &[]));
        let said = text(block_on(c.run_until("click", &s(&["@e1"]), None, Some(in_ms(8000)))));
        assert!(said.starts_with("erro: click não respondeu em ") && said.ends_with("(preso em Input.dispatchMouseEvent)"), "{said}");
        assert!((5000..=6000).contains(&ceiling_in(&said)), "{said}");
        assert_eq!(verb_ceiling("click", &s(&["@e1"]), Some(8000)), Some(6000));
        // Prazo curto: a folga é no máximo um quarto dele.
        assert_eq!(verb_ceiling("click", &s(&["@e1"]), Some(2000)), Some(1500));
    }

    /// O prazo conta da chegada do pedido: o que passou antes de o verbo pegar a vez sai do teto dele.
    #[test]
    fn the_ceiling_uses_only_what_is_left_of_the_deadline() {
        let c = stuck_click();
        block_on(c.run("snapshot", &[]));
        let deadline = in_ms(8000);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let said = text(block_on(c.run_until("click", &s(&["@e1"]), None, Some(deadline))));
        assert!((4000..6000).contains(&ceiling_in(&said)), "{said}");
    }

    /// Verbo na fila atrás de um preso, com o prazo de quem pediu vencido ao pegar a vez: recusa sem tocar a página.
    #[test]
    fn a_verb_whose_deadline_expired_in_the_queue_never_touches_the_page() {
        let c = stuck_click();
        block_on(c.run("snapshot", &[]));
        let expired = Instant::now();
        let (first, second) = block_on(futures::future::join(c.run("click", &s(&["@e1"])),
            c.run_until("click", &s(&["@e1"]), None, Some(expired))));
        assert!(text(first).starts_with("erro: click não respondeu"));
        assert_eq!(code(&second), Some("queue_deadline"));
        assert_eq!(text(second), "erro: click não começou: o prazo de quem pediu venceu na fila");
        let input = c.page.calls.borrow().iter().filter(|(m, _)| m.starts_with("Input.")).count();
        assert_eq!(input, 2, "só o botão apertado e solto pelo primeiro");
        assert!(!c.busy());
    }

    #[test]
    fn wait_keeps_its_own_ceiling_above_the_default() {
        let c = Controller::new(fake(|_, _| Ok(json!({})), "Network.enable"), false);
        assert_eq!(text(block_on(c.run("wait", &s(&["--idle", "--timeout", "30000"])))), "erro: wait não respondeu em 32000 ms (preso em Network.enable)");
        // O prazo de quem espera continua mandando.
        assert_eq!(verb_ceiling("wait", &s(&["--idle", "--timeout", "30000"]), Some(10_000)), Some(8000));
        let said = text(block_on(c.run_until("wait", &s(&["--idle", "--timeout", "30000"]), None, Some(in_ms(10_000)))));
        assert!((7000..=8000).contains(&ceiling_in(&said)) && said.ends_with("(preso em Network.enable)"), "{said}");
        assert_eq!(verb_ceiling("wait", &s(&["30000"]), None), Some(32_000), "a pausa em ms é o teto dela");
        assert_eq!(verb_ceiling("click", &s(&["@e1"]), None), Some(VERB_MS));
    }

    /// A navegação fica com o limite que já tinha (o carregamento): o teto padrão não a encurta; o prazo de quem
    /// espera continua mandando.
    #[test]
    fn navigation_keeps_its_own_bound_and_only_the_deadline_caps_it() {
        for verb in ["goto", "back", "reload"] {
            assert_eq!(verb_ceiling(verb, &s(&["http://a/"]), None), None, "{verb}");
            assert_eq!(verb_ceiling(verb, &s(&["http://a/"]), Some(45_000)), Some(43_000), "{verb}");
        }
        let c = Controller::new(fake(|_, _| Ok(json!({})), "Page.navigate"), false);
        let said = text(block_on(c.run_until("goto", &s(&["http://a/"]), None, Some(in_ms(8000)))));
        assert!(said.starts_with("erro: goto não respondeu em ") && (5000..=6000).contains(&ceiling_in(&said)), "{said}");
    }

    /// Clique cortado com o botão apertado: o `mouseReleased` sai no mesmo ponto, e o próximo verbo não herda o botão.
    #[test]
    fn a_click_cut_with_the_button_down_releases_it() {
        let c = stuck_click();
        block_on(c.run("snapshot", &[]));
        assert!(text(block_on(c.run("click", &s(&["@e1"])))).starts_with("erro: click não respondeu"));
        let mouse: Vec<Value> = c.page.calls.borrow().iter().filter(|(m, _)| m == "Input.dispatchMouseEvent").map(|(_, p)| p.clone()).collect();
        let kinds: Vec<&str> = mouse.iter().map(|p| p["type"].as_str().unwrap_or_default()).collect();
        assert_eq!(kinds, ["mousePressed", "mouseReleased"]);
        assert_eq!((&mouse[1]["x"], &mouse[1]["y"]), (&mouse[0]["x"], &mouse[0]["y"]));
        // A soltura já saiu: não sobra para o teto do verbo seguinte.
        assert!(c.pressed.borrow().is_none());
    }

    /// `fill-secret` cortado depois de mandar o valor: a resposta diz que ele pode ter entrado, sem o valor; cortado
    /// antes, é a falha de sempre.
    #[test]
    fn a_fill_secret_cut_after_typing_says_the_value_may_be_in() {
        let c = Controller::new(fake(|m, p| Ok(login_answer("senha", m, p)), FILL_SECRET), false);
        let said = fill_secret(&c, "password");
        assert_eq!(said, "erro: erro_cofre_incerto: fill-secret não confirmou em 20000 ms; o valor pode ter sido digitado (preso em Runtime.callFunctionOn)");
        let c = Controller::new(fake(|m, p| Ok(login_answer("senha", m, p)), "DOM.resolveNode"), false);
        assert_eq!(fill_secret(&c, "password"), "erro: fill-secret não respondeu em 20000 ms (preso em DOM.resolveNode)");
        assert!(!typed_secret(&c));
    }

    /// `type` e `fill` cortados depois de mandar o texto: a resposta diz que ele pode ter entrado; cortado antes de
    /// digitar, é a falha de sempre.
    #[test]
    fn text_cut_after_it_was_sent_says_it_may_be_in() {
        let c = Controller::new(fake(|m, p| Ok(login_answer("campo", m, p)), "Input.insertText"), false);
        assert_eq!(text(block_on(c.run("type", &s(&["abc"])))), "erro: type não confirmou em 20000 ms; o texto pode ter sido aplicado (preso em Input.insertText)");
        block_on(c.run("snapshot", &[]));
        assert_eq!(text(block_on(c.run("fill", &s(&["@e1", "abc"])))), "erro: fill não confirmou em 20000 ms; o texto pode ter sido aplicado (preso em Input.insertText)");
        let c = Controller::new(fake(|m, p| Ok(login_answer("campo", m, p)), "Runtime.evaluate"), false);
        assert_eq!(text(block_on(c.run("type", &s(&["abc"])))), "erro: type não respondeu em 20000 ms (preso em Runtime.evaluate)");
    }

    /// `press` cortado com a tecla apertada: a resposta diz que ela pode ter sido aplicada, e o `keyUp` sai na hora,
    /// para o verbo seguinte não herdar a tecla.
    #[test]
    fn a_press_cut_with_the_key_down_says_so_and_releases_it() {
        let c = Controller::new(fake(|_, _| Ok(json!({})), "Input.dispatchKeyEvent"), false);
        assert_eq!(text(block_on(c.run("press", &s(&["Enter"])))), "erro: press não confirmou em 20000 ms; a tecla pode ter sido aplicada (preso em Input.dispatchKeyEvent)");
        let keys: Vec<Value> = c.page.calls.borrow().iter().filter(|(m, _)| m == "Input.dispatchKeyEvent").map(|(_, p)| p.clone()).collect();
        let kinds: Vec<&str> = keys.iter().map(|p| p["type"].as_str().unwrap_or_default()).collect();
        assert_eq!(kinds, ["keyDown", "keyUp"]);
        assert_eq!(keys[1]["key"], keys[0]["key"]);
        assert!(keys[1].get("text").is_none(), "a soltura não digita de novo");
        assert!(c.pressed.borrow().is_none());
        // Cortado antes da tecla: a falha de sempre.
        let c = Controller::new(fake(|_, _| Ok(json!({})), "Runtime.evaluate"), false);
        assert_eq!(text(block_on(c.run("press", &s(&["Enter"])))), "erro: press não respondeu em 20000 ms (preso em Runtime.evaluate)");
    }

    #[test]
    fn a_stuck_layout_from_the_panel_never_keeps_the_turn() {
        let c = Controller::new(fake(|m, _| Ok(if m == "Page.getNavigationHistory" { json!({"currentIndex": 0, "entries": [{"url": "http://a/"}]}) } else { json!({}) }),
            "Emulation.setTouchEmulationEnabled"), false);
        block_on(c.set_hidden(true));
        assert_eq!(text(block_on(c.run("url", &[]))), "ok: \"http://a/\"");
    }

    #[test]
    fn tabs_and_unknown_verbs_are_refused() {
        let c = ctl(|_, _| Ok(json!({})));
        assert!(text(block_on(c.run("tab-list", &[]))).starts_with("erro: o app nativo ainda nao tem abas"));
        assert_eq!(text(block_on(c.run("voar", &[]))), "erro: verbo desconhecido: voar");
    }
}
