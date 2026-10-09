//! Estado da interface dos mods por sessão atendida pelo Rust, sem terminal (superfície) ou com terminal
//! (fase 3): quem leva os pedidos dos apps (o ator ou o elo do terminal), o último `plugin_ui`, os avisos
//! vivos e o clique do app em aberto. Liga o ator do runtime, as rotas dos apps e o hub de eventos dos
//! aparelhos, que vivem em lugares diferentes.
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Notify;

use super::model::{ModsCall, ModsError, BAND_SITE, TOAST_DEFAULT_MS};
use crate::side::{WeakHubs, TOASTS_KEPT};

pub type CallFuture = Pin<Box<dyn Future<Output = Result<Value, ModsError>> + Send>>;

/// O caminho de um pedido de app à sessão: quem o leva à superfície e a vez da sessão, que faz os
/// pedidos de aparelhos diferentes correrem um por vez.
pub struct Turn {
    pub link: Arc<dyn SurfaceLink>,
    pub lock: Arc<tokio::sync::Mutex<()>>,
}

/// Quem leva o pedido do app à superfície da sessão: o ator do runtime (`RuntimeHandle`). `deadline` é o
/// prazo de quem pediu: depois dele a resposta não serve, e a ação não pode rodar no mod.
pub trait SurfaceLink: Send + Sync {
    fn call(&self, call: ModsCall, deadline: Instant) -> CallFuture;
}

pub type ShownFuture = Pin<Box<dyn Future<Output = Option<String>> + Send>>;

/// O elo da sessão com terminal (`mods::terminal::TerminalLink`): os pedidos dos apps, a leitura do
/// painel na frente pela tela e o fim do vigia de tamanho.
pub trait TerminalProbe: SurfaceLink {
    fn read_shown(&self) -> ShownFuture;
    fn stop(&self);
    /// A âncora da faixa do último `/ui` (`tree::anchor`), para o executor reconhecer a faixa inteira focada.
    fn anchor(&self, anchor: Option<String>);
}

/// Janela que junta as leituras do painel na frente (risco "shown_id na troca de aba sem redesenho").
pub const SHOWN_READ_WINDOW: Duration = Duration::from_millis(300);
/// Com terminal o press vem depois da leitura, da roda ou da reserva por teclado: a janela cobre o
/// orçamento do pedido inteiro, nada além dele, porque a rota fecha o clique no fim (`finish_click`).
const TERMINAL_CLICK_WINDOW: Duration = super::routes::REQUEST_BUDGET;
const PRESSED_KEPT: usize = 20;
const FOCUS_KEPT: usize = 20;

/// Um painel no espelho da sessão com terminal, como o plugin do Hangar o mandou.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TerminalPane {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default = "inline")]
    pub placement: String,
    #[serde(default)]
    pub columns: Option<u64>,
    #[serde(default)]
    pub tree: Value,
    /// Estado estruturado do painel, quando o plugin manda um para o app desenhar com o tema dele (o `/btw`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

fn inline() -> String {
    "inline".into()
}

/// A faixa e os painéis que o plugin do Hangar viu no terminal (`/api/plugin/ui`). `columns` é o
/// `bodyColumns` da faixa.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TerminalView {
    pub above: Value,
    pub columns: Option<u64>,
    pub panes: Vec<TerminalPane>,
    pub shown: Option<String>,
    pub caps: Vec<String>,
}

impl TerminalView {
    pub fn ids(&self) -> Vec<String> {
        self.panes.iter().map(|pane| pane.id.clone()).collect()
    }

    pub fn titles(&self) -> Vec<String> {
        self.panes.iter().map(|pane| if pane.title.is_empty() { pane.id.clone() } else { pane.title.clone() }).collect()
    }

    /// A tela é a fonte; sem a linha de abas na tela vale o `shown` do plugin (T1); sem nenhum, o último.
    fn shown_id(&self, screen: Option<&str>) -> Option<String> {
        let has = |id: &str| self.panes.iter().any(|pane| pane.id == id);
        screen.filter(|id| has(id)).or(self.shown.as_deref().filter(|id| has(id))).map(str::to_owned)
            .or_else(|| self.panes.last().map(|pane| pane.id.clone()))
    }

    fn app_json(&self, screen: Option<&str>) -> Value {
        json!({"above": if super::tree::is_engine_only(&self.above) { Value::Null } else { self.above.clone() },
               "panes": self.panes, "shown_id": self.shown_id(screen), "columns": self.columns, "source": "terminal", "caps": self.caps})
    }
}

/// A resposta ao hook de `ui.focus` do plugin: se há alvo armado e, quando cabe, o elemento que entra
/// no lugar do pedido.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FocusTarget {
    pub armed: bool,
    pub attempt: Option<String>,
    pub rewrite: Option<String>,
}

/// Um foco que o plugin viu (`/api/plugin/focused`), na ordem de `seq`.
#[derive(Clone, Debug, PartialEq)]
pub struct FocusSeen {
    pub seq: u64,
    pub attempt: String,
    pub request_id: String,
    pub element: Option<String>,
    /// O mod do elemento focado. `None`: parada do motor, ou plugin do Hangar de antes de o foco levá-lo.
    pub plugin: Option<String>,
    pub denied: bool,
}

struct Focus {
    attempt: String,
    site: String,
    plugin: Option<String>,
    key: String,
    rewritten: bool,
}

/// Um botão de mod pressionado no terminal, como o plugin contou.
struct Pressed {
    at: Instant,
    site: String,
    /// `None`: plugin do Hangar carregado antes de o press levar o mod.
    plugin: Option<String>,
    key: String,
}

/// O press é do mod `expected`. Sem o mod no press, vale só o lugar e a `key`, como antes: o plugin do
/// Hangar já carregado numa sessão viva não o manda.
fn same_mod(seen: Option<&str>, expected: &str) -> bool {
    seen.is_none_or(|seen| seen == expected)
}

/// O que só a sessão com terminal tem.
struct Terminal {
    probe: Arc<dyn TerminalProbe>,
    /// Por `Arc`, como o `Ui`: quem lê copia o ponteiro sob a trava global e monta o JSON fora dela.
    view: Option<Arc<TerminalView>>,
    /// Muda a cada vista nova e a cada leitura da tela: uma publicação montada fora da trava com versão
    /// velha não passa na frente da mais nova.
    version: u64,
    screen_shown: Option<String>,
    reading: bool,
    pressed: Vec<Pressed>,
    scrolls: HashMap<String, (u64, i64)>,
    focus: Option<Focus>,
    seen: Vec<FocusSeen>,
}

impl Terminal {
    fn new(probe: Arc<dyn TerminalProbe>) -> Self {
        Self { probe, view: None, version: 0, screen_shown: None, reading: false, pressed: Vec::new(), scrolls: HashMap::new(),
            focus: None, seen: Vec::new() }
    }
}

/// Mesmos tetos do Python (`plugin_bridge`): o app trata o aviso igual nas duas fontes. O máximo e a
/// quantidade guardada são os do `side.rs`, que já os aplica aos avisos que vêm do Python.
const TOAST_MIN_MS: u64 = 1000;
const TOAST_MAX_MS: u64 = crate::side::TOAST_MAX_MS as u64;
const TOAST_MAX_CHARS: usize = 2000;
const TOAST_PLUGIN_CHARS: usize = 64;
/// Por quanto tempo o efeito (cópia, URL) ainda é do clique: a mesma janela do plugin (`APP_PRESS_MS`).
const CLICK_WINDOW: Duration = Duration::from_millis(1500);

struct Click {
    site: String,
    key: String,
    /// O mod do botão, que veio no pedido do app: só a cópia dele é do clique (A11).
    plugin: String,
    attempt: String,
    until: Instant,
    matched: bool,
    copied: Option<String>,
    opened: Option<String>,
    /// Acorda quem espera o efeito do clique (`finish_click`) quando a cópia ou a URL chega.
    effect: Arc<tokio::sync::Notify>,
}

struct Session {
    /// A vida do ator que atende a sessão, única no servidor (`Mods::new_life`): a publicação de um ator
    /// velho, de outra sessão que teve o mesmo nome, não casa com ela.
    life: u64,
    /// O processo do `claude -p` (chave durável e cano): o renomear fecha e reabre a sessão no mesmo.
    process: String,
    /// O nome com que o processo nasceu (`CP_SESSION_NAME`), que é o que ele manda à ponte junto com o
    /// token desse nome. Muda só com processo novo, não com o renomear.
    born: String,
    link: Arc<dyn SurfaceLink>,
    lock: Arc<tokio::sync::Mutex<()>>,
    /// O último `plugin_ui`, no texto que sai aos aparelhos e que se compara. Por `Arc`: quem lê copia o
    /// ponteiro sob a trava global e trabalha fora dela.
    ui: Option<Arc<str>>,
    toasts: Vec<(Instant, Value)>,
    click: Option<Click>,
    /// Só na sessão com terminal (fase 3): o espelho que o plugin manda, o elo e as esperas do clique.
    terminal: Option<Terminal>,
    /// Sem terminal: o que o plugin do Hangar manda pela ponte, juntado à vista da superfície. Por `Arc`: sob
    /// a trava global só se copia o ponteiro, e a identidade diz se uma publicação ainda é a da vez.
    extra: Arc<SurfaceExtra>,
    /// Sem terminal: a última vista da superfície antes da junção, republicada quando o `extra` muda.
    surface_view: Option<Arc<Value>>,
}

/// O que o plugin do Hangar manda pela ponte numa sessão sem terminal: o que ele atende (`caps`) e o estado
/// estruturado de painéis por id (`data`). A árvore dos painéis vem da superfície; isto só se junta a ela.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SurfaceExtra {
    pub caps: Vec<String>,
    pub data: BTreeMap<String, Value>,
}

/// A vista da superfície com o que o plugin mandou pela ponte; montada fora da trava global.
fn with_extra(view: &Value, extra: &SurfaceExtra) -> Value {
    let mut view = view.clone();
    // Sem nada do plugin, a vista sai como sempre saiu.
    if extra.caps.is_empty() && extra.data.is_empty() { return view; }
    view["caps"] = json!(extra.caps);
    if let Some(panes) = view["panes"].as_array_mut() {
        for pane in panes {
            if let Some(data) = pane["id"].as_str().and_then(|id| extra.data.get(id)) { pane["data"] = data.clone(); }
        }
    }
    view
}

#[derive(Default)]
struct Inner {
    sessions: HashMap<String, Session>,
    /// O nome de nascimento dos processos cujas sessões saíram do Rust: o renomear fecha e reabre a
    /// sessão com o mesmo processo, e a reabertura o herda daqui.
    departed: VecDeque<(String, String)>,
    toast_seq: u64,
    /// Ordem dos avisos de foco e rolagem da sessão com terminal.
    seq: u64,
}

/// Quantos processos que saíram do Rust guardam o nome de nascimento para uma reabertura.
const DEPARTED_KEPT: usize = 64;

#[derive(Clone, Default)]
pub struct Mods {
    inner: Arc<Mutex<Inner>>,
    hubs: Arc<OnceLock<WeakHubs>>,
    lives: Arc<std::sync::atomic::AtomicU64>,
    /// Acorda quem espera press, fechar, foco e rolagem da sessão com terminal.
    notify: Arc<Notify>,
}

pub(crate) fn random_hex(bytes: usize) -> String {
    use ring::rand::SecureRandom;
    let mut buffer = vec![0u8; bytes];
    ring::rand::SystemRandom::new().fill(&mut buffer).expect("fonte de aleatoriedade do sistema");
    buffer.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// O número dos avisos recomeça a cada subida: o prefixo impede o app de tomar um aviso novo por um
/// já mostrado, como o `_TOAST_BOOT` do Python.
fn boot() -> &'static str {
    static BOOT: OnceLock<String> = OnceLock::new();
    BOOT.get_or_init(|| random_hex(4))
}

/// `plugin_ui` sem faixa e sem painel: a sessão saiu do Rust ou foi substituída.
fn empty_ui(source: &str) -> Value {
    json!({"above": null, "panes": [], "shown_id": null, "columns": null, "source": source})
}

impl Mods {
    pub fn bind_hubs(&self, hubs: WeakHubs) {
        let _ = self.hubs.set(hubs);
    }

    /// Identificador de uma vida de ator, único no servidor. A geração da sessão não serve: ela conta por
    /// chave durável, e duas sessões que tiveram o mesmo nome podem ter a mesma.
    pub fn new_life(&self) -> u64 {
        self.lives.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
    }

    /// O ator abriu a sessão no Rust: daqui em diante os pedidos dos apps são dele (dono único). Os
    /// avisos vivos ficam; a faixa espera o primeiro desenho do processo novo.
    pub fn attach(&self, name: &str, life: u64, link: Arc<dyn SurfaceLink>) {
        self.attach_process(name, name, life, link);
    }

    /// `attach` com o processo do `claude -p`. Reaberta no mesmo processo (renomear), a sessão herda o nome
    /// de nascimento dele e os avisos vivos. Outro processo com o mesmo nome não herda nada: nem avisos, nem
    /// faixa, nem clique em aberto, nem o nome de nascimento de quem saiu.
    pub fn attach_process(&self, name: &str, process: &str, life: u64, link: Arc<dyn SurfaceLink>) {
        self.attach_with(name, process, life, link, None);
    }

    /// A sessão com terminal abriu no Rust (fase 3): os pedidos dos apps vão ao elo dela (dono único). O
    /// processo é a chave durável mais o pane e a criação dele (`key:pane:created`, Task 16): o renomear
    /// reabre no mesmo e herda como a sessão sem terminal.
    pub fn attach_terminal(&self, name: &str, process: &str, life: u64, probe: Arc<dyn TerminalProbe>) {
        let link: Arc<dyn SurfaceLink> = probe.clone();
        self.attach_with(name, process, life, link, Some(Terminal::new(probe)));
    }

    fn attach_with(&self, name: &str, process: &str, life: u64, link: Arc<dyn SurfaceLink>, terminal: Option<Terminal>) {
        let source = if terminal.is_some() { "terminal" } else { "surface" };
        let (replaced, old_probe) = {
            let mut inner = self.inner.lock().unwrap();
            let old = inner.sessions.remove(name);
            let replaced = old.as_ref().is_some_and(|old| old.process != process);
            let old_probe = old.as_ref().and_then(|old| old.terminal.as_ref().map(|terminal| terminal.probe.clone()));
            let (toasts, extra) = old.filter(|old| old.process == process).map(|old| (old.toasts, old.extra)).unwrap_or_default();
            let born = match inner.departed.iter().position(|(departed, _)| departed == process) {
                Some(at) => inner.departed.remove(at).map(|(_, born)| born).unwrap_or_else(|| name.to_owned()),
                None => name.to_owned(),
            };
            inner.sessions.insert(name.to_owned(), Session { life, process: process.to_owned(), born, link,
                lock: Arc::default(), ui: None, toasts, click: None, terminal, extra, surface_view: None });
            (replaced, old_probe)
        };
        // O vigia da sessão anterior para: o elo novo tem o dele.
        if let Some(probe) = old_probe {
            probe.stop();
        }
        // A faixa que os aparelhos guardam é da sessão substituída.
        if replaced {
            self.deliver(name, "plugin_ui", &empty_ui(source).to_string());
        }
        self.notify.notify_waiters();
    }

    /// A sessão saiu do Rust (S9): esquece o estado, para o elo da sessão com terminal e limpa a faixa dos
    /// aparelhos. Só a vida `life`: outra sessão com o mesmo nome fica.
    pub fn forget(&self, name: &str, life: u64) {
        let removed = {
            let mut inner = self.inner.lock().unwrap();
            match inner.sessions.get(name).is_some_and(|session| session.life == life).then(|| inner.sessions.remove(name)).flatten() {
                Some(session) => {
                    inner.departed.retain(|(process, _)| *process != session.process);
                    inner.departed.push_back((session.process.clone(), session.born.clone()));
                    if inner.departed.len() > DEPARTED_KEPT {
                        inner.departed.pop_front();
                    }
                    Some(session)
                }
                None => None,
            }
        };
        if let Some(session) = removed {
            let source = match &session.terminal {
                Some(terminal) => {
                    terminal.probe.stop();
                    "terminal"
                }
                None => "surface",
            };
            self.deliver(name, "plugin_ui", &empty_ui(source).to_string());
            self.notify.notify_waiters();
        }
    }

    pub fn owns(&self, name: &str) -> bool {
        self.inner.lock().unwrap().sessions.contains_key(name)
    }

    /// A sessão que a ponte do plugin quer dizer com `sessao`, o nome com que o processo dela nasceu (que
    /// difere do nome atual depois de um renomear sem relançar o `claude -p`). Devolve o nome atual.
    ///
    /// O token da ponte é derivado só do nome: dois processos que nasceram com o mesmo nome têm o mesmo
    /// token, e o servidor não os distingue. Com duas sessões vivas nessa situação (uma renomeada, outra
    /// criada depois com o nome antigo), a ponte não atende nenhuma das duas, para um processo não agir no
    /// clique da outra: o pedido segue ao Python, que não tem o clique, e o mod abre a URL no servidor. Uma
    /// sessão de fora do Rust com esse nome não aparece aqui: quem confere é a ponte (`bridge::owned`).
    pub fn bridge_session(&self, sessao: &str) -> Option<String> {
        let inner = self.inner.lock().unwrap();
        let mut found = inner.sessions.iter().filter(|(_, session)| session.born == sessao).map(|(name, _)| name.clone());
        match (found.next(), found.next()) {
            (Some(name), None) => Some(name),
            _ => None,
        }
    }

    pub fn link(&self, name: &str) -> Option<Turn> {
        self.inner.lock().unwrap().sessions.get(name).map(|session| Turn { link: session.link.clone(), lock: session.lock.clone() })
    }

    /// Guarda e entrega o `plugin_ui`; devolve se mudou. É o único ponto que compara a vista nova com a
    /// anterior: a superfície publica a cada desenho guardado, sem guardar cópia para comparar.
    pub fn publish_ui(&self, name: &str, life: u64, data: Value) -> bool {
        if data["source"] != "surface" { return self.publish_if(name, life, data, |_| true); }
        let view = Arc::new(data);
        let extra = {
            let mut inner = self.inner.lock().unwrap();
            let Some(session) = inner.sessions.get_mut(name).filter(|session| session.life == life) else { return false };
            session.surface_view = Some(view.clone());
            session.extra.clone()
        };
        self.publish_surface(name, life, view, extra)
    }

    /// O `/ui` do plugin numa sessão sem terminal: guarda o `extra` e republica a última vista com ele.
    pub fn surface_extra(&self, name: &str, extra: SurfaceExtra) {
        let extra = Arc::new(extra);
        let (life, view) = {
            let mut inner = self.inner.lock().unwrap();
            let Some(session) = inner.sessions.get_mut(name) else { return };
            session.extra = extra.clone();
            (session.life, session.surface_view.clone())
        };
        if let Some(view) = view { self.publish_surface(name, life, view, extra); }
    }

    /// Junta fora da trava e publica só se vista e `extra` ainda forem os da sessão: um desenho e um `/ui` que se
    /// cruzam não deixam o mais velho por último.
    fn publish_surface(&self, name: &str, life: u64, view: Arc<Value>, extra: Arc<SurfaceExtra>) -> bool {
        let data = with_extra(&view, &extra);
        self.publish_if(name, life, data, |session| {
            session.surface_view.as_ref().is_some_and(|now| Arc::ptr_eq(now, &view)) && Arc::ptr_eq(&session.extra, &extra)
        })
    }

    /// `current`: conferido sob a trava, diz se a publicação ainda é a da vez (a da sessão com terminal é
    /// montada fora da trava e não pode passar na frente de uma mais nova).
    fn publish_if(&self, name: &str, life: u64, data: Value, current: impl Fn(&Session) -> bool) -> bool {
        let raw: Arc<str> = data.to_string().into();
        {
            let mut inner = self.inner.lock().unwrap();
            let Some(session) = inner.sessions.get_mut(name).filter(|session| session.life == life && current(session)) else { return false };
            if session.ui.as_ref() == Some(&raw) {
                return false;
            }
            session.ui = Some(raw.clone());
        }
        self.deliver(name, "plugin_ui", &raw);
        true
    }

    /// O ator morreu sem passar pelo `close`: a faixa e os painéis somem dos apps, e a sessão segue com o
    /// mesmo dono até o `close` a esquecer.
    pub fn clear_ui(&self, name: &str, life: u64) {
        // O que o plugin anunciou morreu com o processo: sem isso o app seguiria mandando `/btw` a ninguém.
        if let Some(session) = self.inner.lock().unwrap().sessions.get_mut(name).filter(|session| session.life == life) {
            session.extra = Arc::default();
            session.surface_view = None;
        }
        self.publish_ui(name, life, empty_ui("surface"));
    }

    /// Aviso de mod (`ui_toast`, S6): o Claude Code já descarta o que vem a menos de 2 s do anterior do
    /// mesmo mod, e o Hangar não limita de novo.
    pub fn toast(&self, name: &str, life: u64, plugin: &str, text: &str, timeout_ms: u64) {
        if text.trim().is_empty() {
            return;
        }
        let toast = {
            let mut inner = self.inner.lock().unwrap();
            inner.toast_seq += 1;
            let seq = inner.toast_seq;
            let Some(session) = inner.sessions.get_mut(name).filter(|session| session.life == life) else { return };
            let ms = if timeout_ms == 0 { TOAST_DEFAULT_MS } else { timeout_ms }.clamp(TOAST_MIN_MS, TOAST_MAX_MS);
            let toast = json!({"id": format!("rs-{}-{seq}", boot()),
                "text": text.chars().take(TOAST_MAX_CHARS).collect::<String>(),
                "plugin": plugin.chars().take(TOAST_PLUGIN_CHARS).collect::<String>(), "timeoutMs": ms});
            let now = Instant::now();
            session.toasts.retain(|(until, _)| *until > now);
            session.toasts.push((now + Duration::from_millis(ms), toast.clone()));
            if session.toasts.len() > TOASTS_KEPT {
                let extra = session.toasts.len() - TOASTS_KEPT;
                session.toasts.drain(..extra);
            }
            toast
        };
        self.deliver(name, "plugin_toast", &toast.to_string());
    }

    /// O mod copiou um texto. Com clique do app em aberto, o texto volta na resposta do clique, para o
    /// aparelho de quem clicou; sem clique, vira aviso (spec, "Fonte superfície", passo 6).
    pub fn copied(&self, name: &str, life: u64, plugin: &str, text: &str) {
        let taken = {
            let mut inner = self.inner.lock().unwrap();
            let Some(session) = inner.sessions.get_mut(name).filter(|session| session.life == life) else { return };
            match session.click.as_mut().filter(|click| click.until > Instant::now() && click.plugin == plugin) {
                Some(click) => {
                    click.copied = Some(text.to_owned());
                    click.effect.notify_one();
                    true
                }
                None => false,
            }
        };
        if !taken {
            self.toast(name, life, plugin, text, TOAST_DEFAULT_MS);
        }
    }

    /// O mod do controle `key` (de um dos `kinds`) no lugar `site` do último `plugin_ui`, para o app de antes
    /// de o pedido levar o mod. A `key` em mais de um mod no lugar não diz de qual é: nenhum.
    pub fn plugin_of(&self, name: &str, site: &str, key: &str, kinds: &[&str]) -> Option<String> {
        let raw = self.inner.lock().unwrap().sessions.get(name)?.ui.clone()?;
        // O parse da árvore (até ~400 KB) fica fora da trava de todas as sessões.
        let ui: Value = serde_json::from_str(&raw).ok()?;
        let tree = if site == BAND_SITE { &ui["above"] }
            else { &ui["panes"].as_array()?.iter().find(|pane| pane["id"] == site)?["tree"] };
        if super::tree::ambiguous(tree, None, key, kinds) { return None; }
        super::tree::find(tree, None, key, kinds).map(|control| control.plugin)
    }

    /// Abre o clique do app: o plugin do Hangar casa o press com ele (`press-start`) e o efeito volta
    /// para quem clicou.
    pub fn begin_click(&self, name: &str, site: &str, plugin: &str, key: &str) -> String {
        let attempt = random_hex(8);
        if let Some(session) = self.inner.lock().unwrap().sessions.get_mut(name) {
            let window = if session.terminal.is_some() { TERMINAL_CLICK_WINDOW } else { CLICK_WINDOW };
            session.click = Some(Click { site: site.to_owned(), key: key.to_owned(), plugin: plugin.to_owned(), attempt: attempt.clone(),
                until: Instant::now() + window, matched: false, copied: None, opened: None, effect: Arc::default() });
        }
        attempt
    }

    /// O press é o clique que o app pediu? Sim uma vez só, como o `_do_app` do Python. `plugin`: o mod do
    /// press, que o plugin do Hangar carregado antes desta versão não manda.
    pub fn match_click(&self, name: &str, site: &str, plugin: Option<&str>, key: &str) -> Option<String> {
        let mut inner = self.inner.lock().unwrap();
        let click = inner.sessions.get_mut(name)?.click.as_mut()?;
        if click.matched || click.site != site || click.key != key || !same_mod(plugin, &click.plugin) || click.until <= Instant::now() {
            return None;
        }
        click.matched = true;
        Some(click.attempt.clone())
    }

    /// A URL que o mod abriria, para o aparelho de quem clicou. Fora do clique, `false`: o plugin deixa
    /// o mod abrir na máquina do servidor.
    pub fn opened(&self, name: &str, attempt: &str, url: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let Some(click) = inner.sessions.get_mut(name).and_then(|session| session.click.as_mut()) else { return false };
        if click.attempt != attempt || click.until <= Instant::now() {
            return false;
        }
        click.opened = Some(url.to_owned());
        click.effect.notify_one();
        true
    }

    /// Fecha o clique e devolve a cópia e a URL dele. O `onPress` do mod costuma copiar ou abrir sem
    /// `await`: com o press casado pelo plugin e nada ainda, espera o efeito até `wait`.
    pub async fn finish_click(&self, name: &str, attempt: &str, wait: Duration) -> (Option<String>, Option<String>) {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            // O `notify_one` guarda a vez quando ninguém espera ainda: o efeito que chega entre a leitura e
            // a espera não se perde.
            let effect = {
                let inner = self.inner.lock().unwrap();
                match inner.sessions.get(name).and_then(|session| session.click.as_ref()).filter(|click| click.attempt == attempt) {
                    Some(click) if click.matched && click.copied.is_none() && click.opened.is_none() => click.effect.clone(),
                    _ => break,
                }
            };
            if tokio::time::timeout_at(deadline, effect.notified()).await.is_err() {
                break;
            }
        }
        let mut inner = self.inner.lock().unwrap();
        let Some(session) = inner.sessions.get_mut(name) else { return (None, None) };
        match session.click.take() {
            Some(click) if click.attempt == attempt => (click.copied, click.opened),
            other => { session.click = other; (None, None) }
        }
    }

    /// O que um hub novo precisa para nascer em dia: a última faixa e os avisos vivos, com o tempo
    /// que resta a cada um.
    pub fn replay(&self, name: &str) -> Vec<(&'static str, String)> {
        let (ui, toasts) = {
            let inner = self.inner.lock().unwrap();
            let Some(session) = inner.sessions.get(name) else { return Vec::new() };
            (session.ui.clone(), session.toasts.clone())
        };
        let now = Instant::now();
        let mut frames: Vec<(&'static str, String)> = ui.iter().map(|ui| ("plugin_ui", ui.to_string())).collect();
        for (until, toast) in toasts.iter().filter(|(until, _)| *until > now) {
            let mut toast = toast.clone();
            toast["timeoutMs"] = json!(crate::side::remaining_ms(*until, now));
            frames.push(("plugin_toast", toast.to_string()));
        }
        frames
    }

    pub fn life(&self, name: &str) -> Option<u64> {
        self.inner.lock().unwrap().sessions.get(name).map(|session| session.life)
    }

    pub fn is_terminal(&self, name: &str) -> bool {
        self.inner.lock().unwrap().sessions.get(name).is_some_and(|session| session.terminal.is_some())
    }

    /// O espelho da sessão com terminal só na vida `life`: o clique de uma sessão substituída não lê o da nova.
    pub fn terminal_view_in(&self, name: &str, life: u64) -> Option<Arc<TerminalView>> {
        Self::terminal_in(&self.inner.lock().unwrap(), name, life)?.view.clone()
    }

    /// O `/ui` do plugin numa sessão com terminal do Rust: guarda o espelho e publica o `plugin_ui`. A
    /// árvore (até ~400 KB) vira JSON fora da trava de todas as sessões.
    pub fn terminal_ui(&self, name: &str, view: TerminalView) -> bool {
        let view = Arc::new(view);
        let (life, version, screen, probe) = {
            let mut inner = self.inner.lock().unwrap();
            let Some(session) = inner.sessions.get_mut(name) else { return false };
            let life = session.life;
            let Some(terminal) = session.terminal.as_mut() else { return false };
            terminal.version += 1;
            terminal.view = Some(view.clone());
            (life, terminal.version, terminal.screen_shown.clone(), terminal.probe.clone())
        };
        self.notify.notify_waiters();
        probe.anchor(super::tree::anchor(&view.above));
        let data = view.app_json(screen.as_deref());
        self.publish_if(name, life, data, |session| session.terminal.as_ref().is_some_and(|terminal| terminal.version == version))
    }

    /// O painel que a linha de abas mostra na frente; None quando ela não está na tela. Só na vida `life`: o
    /// clique de uma sessão substituída não escreve na que reabriu com o mesmo nome.
    pub fn set_screen_shown(&self, name: &str, life: u64, shown: Option<String>) {
        self.screen_shown(name, Some(life), shown);
    }

    /// `set_screen_shown` só na vida `life` quando dada: a leitura agendada por uma sessão não cai na que
    /// reabriu com o mesmo nome enquanto ela lia.
    fn screen_shown(&self, name: &str, only: Option<u64>, shown: Option<String>) {
        let (life, version, view, screen) = {
            let mut inner = self.inner.lock().unwrap();
            let Some(session) = inner.sessions.get_mut(name).filter(|session| only.is_none_or(|life| session.life == life)) else { return };
            let life = session.life;
            let Some(terminal) = session.terminal.as_mut() else { return };
            terminal.reading = false;
            terminal.screen_shown = shown;
            terminal.version += 1;
            let Some(view) = terminal.view.clone() else { return };
            (life, terminal.version, view, terminal.screen_shown.clone())
        };
        let data = view.app_json(screen.as_deref());
        self.publish_if(name, life, data, |session| session.terminal.as_ref().is_some_and(|terminal| terminal.version == version));
    }

    /// Lê o painel na frente pela tela, uma vez por janela: a cada `/ui` e depois de cada operação do app.
    pub fn schedule_shown(&self, name: &str) {
        self.schedule(name, None);
    }

    /// `schedule_shown` pedido pelo clique da vida `life`: o de uma sessão substituída não agenda leitura na
    /// que reabriu com o mesmo nome.
    pub fn schedule_shown_in(&self, name: &str, life: u64) {
        self.schedule(name, Some(life));
    }

    fn schedule(&self, name: &str, only: Option<u64>) {
        let (life, probe) = {
            let mut inner = self.inner.lock().unwrap();
            let Some(session) = inner.sessions.get_mut(name).filter(|session| only.is_none_or(|life| session.life == life)) else { return };
            let life = session.life;
            let Some(terminal) = session.terminal.as_mut() else { return };
            if terminal.reading {
                return;
            }
            terminal.reading = true;
            (life, terminal.probe.clone())
        };
        let (mods, name) = (self.clone(), name.to_owned());
        tokio::spawn(async move {
            tokio::time::sleep(SHOWN_READ_WINDOW).await;
            let shown = probe.read_shown().await;
            mods.screen_shown(&name, Some(life), shown);
        });
    }

    /// O terminal da vida `life`; nenhum quando a sessão saiu ou reabriu com outra vida.
    fn terminal_in<'a>(inner: &'a Inner, name: &str, life: u64) -> Option<&'a Terminal> {
        inner.sessions.get(name).filter(|session| session.life == life)?.terminal.as_ref()
    }

    /// Espera `check` dar algo no terminal da vida `life`, acordando a cada aviso do plugin, até `wait`. A
    /// sessão que sai (`forget`) ou reabre com outra vida encerra a espera na hora: o press, o fechar, o foco
    /// e a rolagem da sessão nova não são do clique da antiga.
    async fn wait_for<T>(&self, name: &str, life: u64, wait: Duration, mut check: impl FnMut(&Terminal) -> Option<T>) -> Option<T> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            // Inscrito antes de olhar: o aviso que chega entre a conferência e a espera não se perde.
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let found = {
                let inner = self.inner.lock().unwrap();
                check(Self::terminal_in(&inner, name, life)?)
            };
            if found.is_some() {
                return found;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                let inner = self.inner.lock().unwrap();
                return Self::terminal_in(&inner, name, life).and_then(check);
            }
        }
    }

    /// Muda o terminal da sessão; com `only`, só na vida dada (o que o clique escreve). Os avisos da ponte
    /// valem para quem tem o nome agora: a ponte acha a sessão pelo nome de nascimento (`bridge_session`).
    fn with_terminal(&self, name: &str, only: Option<u64>, change: impl FnOnce(&mut Terminal, u64)) {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.seq += 1;
            let seq = inner.seq;
            if let Some(terminal) = inner.sessions.get_mut(name).filter(|session| only.is_none_or(|life| session.life == life))
                .and_then(|session| session.terminal.as_mut()) {
                change(terminal, seq);
            }
        }
        self.notify.notify_waiters();
    }

    /// Um botão de mod foi pressionado no terminal (`/api/plugin/pressed`).
    pub fn pressed(&self, name: &str, site: &str, plugin: Option<&str>, element: &str) {
        self.with_terminal(name, None, |terminal, _| {
            terminal.pressed.push(Pressed { at: Instant::now(), site: site.to_owned(), plugin: plugin.map(str::to_owned), key: element.to_owned() });
            let extra = terminal.pressed.len().saturating_sub(PRESSED_KEPT);
            terminal.pressed.drain(..extra);
        });
    }

    pub async fn wait_pressed(&self, name: &str, life: u64, site: &str, plugin: &str, key: &str, since: Instant, wait: Duration) -> bool {
        self.wait_for(name, life, wait, |terminal| terminal.pressed.iter().any(|seen| seen.at >= since && seen.site == site && seen.key == key
            && same_mod(seen.plugin.as_deref(), plugin))
            .then_some(())).await.is_some()
    }

    /// O painel saiu do espelho: o plugin viu o `ui.close`. Outros que o mod fecha junto não importam ((e)).
    pub async fn wait_pane_gone(&self, name: &str, life: u64, site: &str, wait: Duration) -> bool {
        self.wait_for(name, life, wait, |terminal| (!terminal.view.as_ref().is_some_and(|view| view.panes.iter().any(|pane| pane.id == site)))
            .then_some(())).await.is_some()
    }

    /// Arma o alvo da reserva por teclado (T5): o hook de `ui.focus` do plugin pergunta por ele.
    pub fn arm_focus(&self, name: &str, life: u64, site: &str, plugin: Option<&str>, key: &str) -> String {
        let attempt = random_hex(8);
        let focus = Focus { attempt: attempt.clone(), site: site.to_owned(), plugin: plugin.map(str::to_owned), key: key.to_owned(),
            rewritten: false };
        self.with_terminal(name, Some(life), move |terminal, _| terminal.focus = Some(focus));
        attempt
    }

    pub fn disarm_focus(&self, name: &str, life: u64, attempt: &str) {
        self.with_terminal(name, Some(life), |terminal, _| {
            if terminal.focus.as_ref().is_some_and(|focus| focus.attempt == attempt) {
                terminal.focus = None;
            }
        });
    }

    /// O alvo armado agora; só os testes perguntam.
    #[doc(hidden)]
    pub fn armed_focus(&self, name: &str) -> Option<String> {
        Some(self.inner.lock().unwrap().sessions.get(name)?.terminal.as_ref()?.focus.as_ref()?.attempt.clone())
    }

    /// A reescrita só troca por outro elemento do mesmo mod e nunca numa parada do motor (sem `plugin`);
    /// uma por alvo armado ((r), (t)).
    pub fn focus_target(&self, name: &str, request_id: &str, plugin: Option<&str>, element: Option<&str>) -> FocusTarget {
        let inner = self.inner.lock().unwrap();
        let Some(focus) = inner.sessions.get(name).and_then(|session| session.terminal.as_ref()).and_then(|terminal| terminal.focus.as_ref()) else {
            return FocusTarget { armed: false, attempt: None, rewrite: None };
        };
        let fits = !focus.rewritten && request_id == focus.site && plugin.is_some() && element.is_some()
            && focus.plugin.as_deref().is_none_or(|armed| Some(armed) == plugin);
        FocusTarget { armed: true, attempt: Some(focus.attempt.clone()), rewrite: fits.then(|| focus.key.clone()) }
    }

    /// O plugin viu um foco com o alvo `attempt` armado. Recusado quando o alvo já não é esse.
    pub fn focused(&self, name: &str, attempt: &str, request_id: &str, plugin: Option<&str>, element: Option<&str>, denied: bool) -> bool {
        let mut accepted = false;
        self.with_terminal(name, None, |terminal, seq| {
            let Some(focus) = terminal.focus.as_mut().filter(|focus| focus.attempt == attempt) else { return };
            if request_id == focus.site && element == Some(focus.key.as_str()) && !denied {
                focus.rewritten = true;
            }
            terminal.seen.push(FocusSeen { seq, attempt: attempt.to_owned(), request_id: request_id.to_owned(),
                element: element.map(str::to_owned), plugin: plugin.map(str::to_owned), denied });
            let extra = terminal.seen.len().saturating_sub(FOCUS_KEPT);
            terminal.seen.drain(..extra);
            accepted = true;
        });
        accepted
    }

    pub fn focus_seq(&self, name: &str, life: u64) -> u64 {
        let inner = self.inner.lock().unwrap();
        Self::terminal_in(&inner, name, life).and_then(|terminal| terminal.seen.last()).map_or(0, |seen| seen.seq)
    }

    pub async fn wait_focus(&self, name: &str, life: u64, attempt: &str, after: u64, wait: Duration,
        accept: impl Fn(&FocusSeen) -> bool) -> Option<FocusSeen> {
        self.wait_for(name, life, wait, |terminal| terminal.seen.iter()
            .find(|seen| seen.seq > after && seen.attempt == attempt && accept(seen)).cloned()).await
    }

    /// Um painel rolou (`/api/plugin/scroll`): o clique acompanha o `offset`, nunca conta eventos ((u)).
    pub fn scrolled(&self, name: &str, site: &str, offset: i64) {
        self.with_terminal(name, None, |terminal, seq| {
            terminal.scrolls.insert(site.to_owned(), (seq, offset));
        });
    }

    pub fn last_scroll(&self, name: &str, life: u64, site: &str) -> (u64, Option<i64>) {
        let inner = self.inner.lock().unwrap();
        Self::terminal_in(&inner, name, life).and_then(|terminal| terminal.scrolls.get(site))
            .map_or((0, None), |&(seq, offset)| (seq, Some(offset)))
    }

    pub async fn wait_scroll(&self, name: &str, life: u64, site: &str, after: u64, wait: Duration) -> Option<(u64, i64)> {
        self.wait_for(name, life, wait, |terminal| terminal.scrolls.get(site).copied().filter(|(seq, _)| *seq > after)).await
    }

    /// O mod copiou um texto num clique do app com terminal (`/api/plugin/copied`); fora do clique, `false`
    /// e o plugin deixa a cópia acontecer no terminal. Acorda o `finish_click`, como o `copied` e o `opened`.
    pub fn click_copied(&self, name: &str, attempt: &str, text: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let Some(click) = inner.sessions.get_mut(name).and_then(|session| session.click.as_mut()) else { return false };
        if click.attempt != attempt || click.until <= Instant::now() {
            return false;
        }
        click.copied = Some(text.to_owned());
        click.effect.notify_one();
        true
    }

    fn deliver(&self, name: &str, event: &str, data: &str) {
        if let Some(hubs) = self.hubs.get().and_then(WeakHubs::upgrade) {
            hubs.deliver(name, event, data);
        }
    }
}
