//! Canal anfitrião: o app desktop de outra máquina empresta o navegador dele às sessões deste servidor.
//! Aqui ficam os anfitriões ligados, o foco por sessão, os pedidos em curso e o estado do navegador
//! de cada sessão; o transporte (WebSocket, autenticação, gravação do print) é do servidor.
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, oneshot};

/// O mesmo teto do `/cmd` local quando o CLI não manda prazo.
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(45);
/// O `login` espera a pessoa no painel; mais que isso é pedido esquecido.
pub const MAX_DEADLINE: Duration = Duration::from_secs(600);
const UNSUPPORTED: [&str; 2] = ["objetivo", "confere"];
const NOT_ALLOWED: &str = "browser_host_not_allowed";
/// Os nomes de sessão vêm do app: sem teto, um anfitrião encheria a memória.
const MAX_SESSIONS: usize = 1024;

/// Resposta do anfitrião a um verbo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// Texto do `/cmd` (`ok: …` ou `erro: …`).
    Text(String),
    /// O PNG do `shot`; quem grava é o servidor.
    Png(Vec<u8>),
}

/// O verbo não chegou a um resultado do navegador.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Nenhum anfitrião ligado deu foco nesta sessão.
    NoHost,
    /// O app desktop recusou o pedido.
    NotAllowed,
    /// O canal caiu com o pedido em curso.
    Gone,
    Timeout,
    Unsupported,
}

impl Refusal {
    pub fn code(self) -> &'static str {
        match self {
            Refusal::NoHost => "browser_host_none",
            Refusal::NotAllowed => NOT_ALLOWED,
            Refusal::Gone => "browser_host_gone",
            Refusal::Timeout => "browser_host_timeout",
            Refusal::Unsupported => "browser_host_verb_unsupported",
        }
    }

    pub fn status(self) -> u16 {
        match self {
            Refusal::NoHost => 409,
            Refusal::NotAllowed => 403,
            Refusal::Gone => 502,
            Refusal::Timeout => 504,
            Refusal::Unsupported => 422,
        }
    }

    pub fn message(self, verb: &str) -> String {
        match self {
            Refusal::NoHost => "nenhum app desktop do Hangar está com esta sessão aberta".into(),
            Refusal::NotAllowed => "o app desktop recusou usar o navegador dele".into(),
            Refusal::Gone => format!("o app desktop desligou no meio do comando; o {verb} pode ter sido executado"),
            Refusal::Timeout => format!("o app desktop não respondeu o {verb} no prazo"),
            Refusal::Unsupported => format!("{verb} não funciona no navegador de outra máquina"),
        }
    }
}

/// O que se sabe do navegador de uma sessão, para o `list` e a espera do `open`.
#[derive(Debug, Clone, PartialEq)]
pub struct BrowserState {
    pub url: String,
    pub aba: Value,
    pub ts: u64,
}

#[derive(Default)]
pub struct Registry {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    next_host: u64,
    next_id: u64,
    next_focus: u64,
    hosts: HashMap<u64, Host>,
    /// Sessão → anfitrião → ordem do último foco dele nela.
    focus: HashMap<String, HashMap<u64, u64>>,
    state: BTreeMap<String, BrowserState>,
}

struct Host {
    out: mpsc::UnboundedSender<String>,
    pending: HashMap<u64, Pending>,
}

struct Pending {
    reply: oneshot::Sender<Result<Reply, Refusal>>,
    session: String,
    /// URL de um `open`/`goto`, gravada no estado quando o anfitrião confirma.
    url: Option<String>,
}

impl Inner {
    // ponytail: varredura O(n) só quando entra sessão nova com o teto cheio; índice por ts se o teto subir muito.
    fn state_entry(&mut self, session: String) -> &mut BrowserState {
        if !self.state.contains_key(&session) && self.state.len() >= MAX_SESSIONS
            && let Some(old) = self.state.iter().min_by_key(|(_, st)| st.ts).map(|(k, _)| k.clone())
        {
            self.state.remove(&old);
        }
        self.state.entry(session).or_insert(BrowserState { url: String::new(), aba: Value::Null, ts: 0 })
    }

    fn chosen(&self, session: &str) -> Option<u64> {
        let hosts = self.focus.get(session)?;
        hosts.iter().filter(|(id, _)| self.hosts.contains_key(id)).max_by_key(|(_, order)| **order).map(|(id, _)| *id)
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn deadline_of(body: &Map<String, Value>) -> Duration {
    body.get("deadline_ms").and_then(Value::as_u64).map_or(DEFAULT_DEADLINE, |ms| Duration::from_millis(ms.max(1)).min(MAX_DEADLINE))
}

impl Registry {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Um anfitrião novo. O receptor leva os pedidos em texto para o socket; soltar a `Connection` o remove.
    pub fn attach(self: &Arc<Self>) -> (Connection, mpsc::UnboundedReceiver<String>) {
        let (out, rx) = mpsc::unbounded_channel();
        let mut inner = self.lock();
        inner.next_host += 1;
        let id = inner.next_host;
        inner.hosts.insert(id, Host { out, pending: HashMap::new() });
        (Connection { registry: self.clone(), id, awaiting_png: None }, rx)
    }

    /// Repassa um verbo do corpo do `/cmd` ao anfitrião com foco na sessão e espera até o `deadline_ms`.
    pub async fn command(&self, session: &str, mut body: Map<String, Value>) -> Result<Reply, Refusal> {
        let verb = body.get("verbo").and_then(Value::as_str).unwrap_or_default().to_owned();
        // `--cdp` é página da máquina do app: o agente daqui não a alcança.
        if UNSUPPORTED.contains(&verb.as_str()) || body.contains_key("cdp") {
            return Err(Refusal::Unsupported);
        }
        if verb == "list" {
            return Ok(Reply::Text(self.list()));
        }
        let deadline = deadline_of(&body);
        let url = matches!(verb.as_str(), "open" | "goto")
            .then(|| body.get("args").and_then(|a| a.get(0)).and_then(Value::as_str).map(str::to_owned))
            .flatten();
        let (host, id, rx) = {
            let mut inner = self.lock();
            let host = inner.chosen(session).ok_or(Refusal::NoHost)?;
            inner.next_id += 1;
            let id = inner.next_id;
            body.remove("chave");
            body.insert("id".into(), json!(id));
            body.insert("session".into(), json!(session));
            body.insert("deadline_ms".into(), json!(deadline.as_millis() as u64));
            let (reply, rx) = oneshot::channel();
            let entry = inner.hosts.get_mut(&host).ok_or(Refusal::NoHost)?;
            // Canal já fechando: o verbo não saiu, então não "pode ter sido executado".
            entry.out.send(Value::Object(body).to_string()).map_err(|_| Refusal::NoHost)?;
            entry.pending.insert(id, Pending { reply, session: session.to_owned(), url });
            (host, id, rx)
        };
        // O CLI que desiste solta este futuro antes do prazo: o pedido não pode ficar preso no anfitrião.
        let _forget = Forget { registry: self, host, id };
        match tokio::time::timeout(deadline, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(Refusal::Gone),
            Err(_) => Err(Refusal::Timeout),
        }
    }

    /// O `list` do CLI: as sessões com navegador conhecido; vivo = algum anfitrião ligado com foco nela.
    pub fn list(&self) -> String {
        let inner = self.lock();
        if inner.state.is_empty() {
            return "nenhum navegador registrado".into();
        }
        inner.state.iter().map(|(session, st)| {
            let live = if inner.chosen(session).is_some() { "vivo".to_owned() } else { format!("MORTO (reabra com: hangar-preview open {})", st.url) };
            let tab = st.aba.as_i64().map(|n| format!("  (aba {n})")).unwrap_or_default();
            format!("{session}  {}  {live}{tab}", st.url)
        }).collect::<Vec<_>>().join("\n")
    }

    fn focus(&self, host: u64, session: String) {
        let mut inner = self.lock();
        inner.next_focus += 1;
        let order = inner.next_focus;
        if !inner.focus.contains_key(&session) && inner.focus.len() >= MAX_SESSIONS
            && let Some(old) = inner.focus.iter().min_by_key(|(_, hosts)| hosts.values().max().copied()).map(|(k, _)| k.clone())
        {
            inner.focus.remove(&old);
        }
        inner.focus.entry(session).or_default().insert(host, order);
    }

    fn set_state(&self, session: String, url: Option<String>, aba: Option<Value>) {
        let mut inner = self.lock();
        let entry = inner.state_entry(session);
        if let Some(url) = url {
            entry.url = url;
        }
        if let Some(aba) = aba {
            entry.aba = aba;
        }
        entry.ts = now_ms();
    }

    fn resolve(&self, host: u64, id: u64, result: Result<Reply, Refusal>) {
        let mut inner = self.lock();
        let Some(pending) = inner.hosts.get_mut(&host).and_then(|h| h.pending.remove(&id)) else { return };
        if let (Ok(Reply::Text(text)), Some(url)) = (&result, &pending.url)
            && !text.starts_with("erro:")
        {
            let entry = inner.state_entry(pending.session);
            entry.url = url.clone();
            entry.ts = now_ms();
        }
        let _ = pending.reply.send(result);
    }

    fn detach(&self, host: u64) {
        let mut inner = self.lock();
        // Soltar os `Pending` fecha cada espera com `Gone`.
        inner.hosts.remove(&host);
        inner.focus.retain(|_, hosts| {
            hosts.remove(&host);
            !hosts.is_empty()
        });
    }
}

struct Forget<'a> {
    registry: &'a Registry,
    host: u64,
    id: u64,
}

impl Drop for Forget<'_> {
    fn drop(&mut self) {
        if let Some(entry) = self.registry.lock().hosts.get_mut(&self.host) {
            entry.pending.remove(&self.id);
        }
    }
}

/// Um app ligado pelo canal. O servidor entrega cada mensagem recebida; ao soltar, o anfitrião sai.
pub struct Connection {
    registry: Arc<Registry>,
    id: u64,
    /// Pedido cujo PNG vem no próximo quadro binário.
    awaiting_png: Option<u64>,
}

impl Connection {
    pub fn text(&mut self, raw: &str) {
        self.awaiting_png = None;
        let Ok(Value::Object(msg)) = serde_json::from_str::<Value>(raw) else { return };
        let session = msg.get("session").and_then(Value::as_str).map(str::to_owned);
        match (msg.get("type").and_then(Value::as_str), session) {
            (Some("focus"), Some(session)) => return self.registry.focus(self.id, session),
            (Some("state"), Some(session)) => {
                let url = msg.get("url").and_then(Value::as_str).map(str::to_owned);
                return self.registry.set_state(session, url, msg.get("aba").cloned());
            }
            (Some(_), _) => return,
            (None, _) => {}
        }
        let Some(id) = msg.get("id").and_then(Value::as_u64) else { return };
        if msg.get("bin").and_then(Value::as_str) == Some("png") {
            self.awaiting_png = Some(id);
            return;
        }
        let result = match msg.get("ok").and_then(Value::as_bool) {
            Some(true) => Ok(Reply::Text(msg.get("text").and_then(Value::as_str).unwrap_or_default().to_owned())),
            Some(false) => {
                let erro = msg.get("erro").and_then(Value::as_str).unwrap_or_default();
                if erro == NOT_ALLOWED {
                    Err(Refusal::NotAllowed)
                } else if erro.starts_with("erro:") {
                    Ok(Reply::Text(erro.to_owned()))
                } else {
                    Ok(Reply::Text(format!("erro: {erro}")))
                }
            }
            None => return,
        };
        self.registry.resolve(self.id, id, result);
    }

    pub fn binary(&mut self, png: Vec<u8>) {
        if let Some(id) = self.awaiting_png.take() {
            self.registry.resolve(self.id, id, Ok(Reply::Png(png)));
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.registry.detach(self.id);
    }
}

#[cfg(test)]
mod tests;
