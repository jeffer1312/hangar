//! Canal anfitrião: o app desktop de outra máquina empresta o navegador dele às sessões deste servidor.
//! Aqui ficam os anfitriões ligados, o foco por sessão, os pedidos em curso e o estado do navegador
//! de cada sessão; o transporte (WebSocket, autenticação, gravação do print) é do servidor.
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, oneshot};
use zeroize::Zeroizing;

/// O mesmo teto do `/cmd` local quando o CLI não manda prazo.
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(45);
/// O `login` espera a pessoa no painel; mais que isso é pedido esquecido.
pub const MAX_DEADLINE: Duration = Duration::from_secs(600);
const UNSUPPORTED: [&str; 2] = ["objetivo", "confere"];
const FILL_SECRET: &str = "fill-secret";
/// Capacidade que o app anuncia no `focus`: app sem ela não conhece o `fill-secret` e não pode receber o valor.
pub const FILL_SECRET_CAP: &str = "fill_secret";
const NOT_ALLOWED: &str = "browser_host_not_allowed";
/// A causa vem do app e vai num cabeçalho: só nome curto em minúsculas, dígitos e `_`.
fn is_code(code: &str) -> bool {
    !code.is_empty() && code.len() <= 64 && code.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Os nomes de sessão vêm do app: sem teto, um anfitrião encheria a memória.
const MAX_SESSIONS: usize = 1024;

/// Resposta do anfitrião a um verbo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// Texto do `/cmd` (`ok: …` ou `erro: …`).
    Text(String),
    /// O `erro: …` com a causa fechada que o app deu (`page_gone`, `ref_invalid`…), já no formato de nome.
    Error(String, String),
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
    /// `fill-secret` ou o campo `valor` vindos de fora do servidor: o valor do cofre só sai pelo `fill_secret`.
    Secret,
    /// O app com a sessão não anunciou `FILL_SECRET_CAP`: o valor do cofre não sai para ele.
    Outdated,
}

impl Refusal {
    pub fn code(self) -> &'static str {
        match self {
            Refusal::NoHost => "browser_host_none",
            Refusal::NotAllowed => NOT_ALLOWED,
            Refusal::Gone => "browser_host_gone",
            Refusal::Timeout => "browser_host_timeout",
            Refusal::Unsupported => "browser_host_verb_unsupported",
            Refusal::Secret => "browser_host_secret_refused",
            Refusal::Outdated => "browser_host_app_outdated",
        }
    }

    pub fn status(self) -> u16 {
        match self {
            Refusal::NoHost | Refusal::Outdated => 409,
            Refusal::NotAllowed => 403,
            Refusal::Gone => 502,
            Refusal::Timeout => 504,
            Refusal::Unsupported | Refusal::Secret => 422,
        }
    }

    pub fn message(self, verb: &str) -> String {
        match self {
            Refusal::NoHost => "nenhum app desktop do Hangar está com esta sessão aberta".into(),
            Refusal::NotAllowed => "o app desktop recusou usar o navegador dele".into(),
            Refusal::Gone => format!("o app desktop desligou no meio do comando; o {verb} pode ter sido executado"),
            Refusal::Timeout => format!("o app desktop não respondeu o {verb} no prazo"),
            Refusal::Unsupported => format!("{verb} não funciona no navegador de outra máquina"),
            Refusal::Secret => "o valor do cofre só vai ao navegador pela tool vault_fill".into(),
            Refusal::Outdated => format!("o app desktop com esta sessão é de uma versão que não faz o {verb}: atualize o app desktop do Hangar"),
        }
    }
}

/// O que se sabe do navegador de uma sessão, para o `list` e a espera do `open`.
#[derive(Debug, Clone, PartialEq)]
pub struct BrowserState {
    pub url: String,
    /// A `aba` que o app informou.
    pub tab: Value,
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
    /// Sessão → hora (ms) do último uso (`seen`) num app de outra máquina. Fica depois que o app desliga: o navegador da
    /// sessão continua sendo o de lá até um app mostrar a sessão de novo, e o verbo recusa em vez de cair noutro.
    seen: HashMap<String, u64>,
    state: BTreeMap<String, BrowserState>,
}

struct Host {
    /// Zerado ao sair: o quadro do `fill_secret` leva o valor do cofre.
    out: mpsc::UnboundedSender<Zeroizing<String>>,
    pending: HashMap<u64, Pending>,
    /// Anunciou `FILL_SECRET_CAP` no último `focus`.
    fill_secret: bool,
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
        self.state.entry(session).or_insert(BrowserState { url: String::new(), tab: Value::Null, ts: 0 })
    }

    fn chosen(&self, session: &str) -> Option<u64> {
        let hosts = self.focus.get(session)?;
        hosts.iter().filter(|(id, _)| self.hosts.contains_key(id)).max_by_key(|(_, order)| **order).map(|(id, _)| *id)
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// O texto do quadro, com o `valor` fora do `Value`, que não zera o que guarda.
fn frame(body: Map<String, Value>, secret: Option<&str>) -> Zeroizing<String> {
    let base = Zeroizing::new(Value::Object(body).to_string());
    let Some(secret) = secret else { return base };
    // shortcut: a cópia que o WebSocket leva do quadro não é zerada; trocar se o transporte passar a guardar quadros.
    // O teto do valor em JSON (`\u00XX` por byte, mais as aspas) reservado antes: crescer no meio largaria cópias sem zerar.
    let mut out = Zeroizing::new(Vec::with_capacity(frame_capacity(base.len(), secret)));
    out.extend_from_slice(&base.as_bytes()[..base.len() - 1]);
    out.extend_from_slice(b",\"valor\":");
    // Escrita num `Vec` não falha.
    let _ = serde_json::to_writer(&mut *out, secret);
    out.push(b'}');
    // O `String` fica com o mesmo buffer; JSON é sempre UTF-8.
    match String::from_utf8(std::mem::take(&mut *out)) {
        Ok(text) => Zeroizing::new(text),
        Err(e) => { drop(Zeroizing::new(e.into_bytes())); Zeroizing::new(String::new()) }
    }
}

/// O quadro sem o `}` final, a chave `,"valor":`, o valor escapado no pior caso e o `}`.
fn frame_capacity(base: usize, secret: &str) -> usize { base + 10 + secret.len() * 6 + 2 }

fn deadline_of(body: &Map<String, Value>) -> Duration {
    body.get("deadline_ms").and_then(Value::as_u64).map_or(DEFAULT_DEADLINE, |ms| Duration::from_millis(ms.max(1)).min(MAX_DEADLINE))
}

impl Registry {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Um anfitrião novo. O receptor leva os pedidos em texto para o socket; soltar a `Connection` o remove.
    pub fn attach(self: &Arc<Self>) -> (Connection, mpsc::UnboundedReceiver<Zeroizing<String>>) {
        let (out, rx) = mpsc::unbounded_channel();
        let mut inner = self.lock();
        inner.next_host += 1;
        let id = inner.next_host;
        inner.hosts.insert(id, Host { out, pending: HashMap::new(), fill_secret: false });
        (Connection { registry: self.clone(), id, awaiting_png: None }, rx)
    }

    /// Repassa um verbo do corpo do `/cmd` ao anfitrião com foco na sessão e espera até o `deadline_ms`.
    pub async fn command(&self, session: &str, body: Map<String, Value>) -> Result<Reply, Refusal> {
        let verb = body.get("verbo").and_then(Value::as_str).unwrap_or_default().to_owned();
        if verb == FILL_SECRET || body.contains_key("valor") {
            return Err(Refusal::Secret);
        }
        // `--cdp` é página da máquina do app: o agente daqui não a alcança.
        if UNSUPPORTED.contains(&verb.as_str()) || body.contains_key("cdp") {
            return Err(Refusal::Unsupported);
        }
        if verb == "list" {
            return Ok(Reply::Text(self.list()));
        }
        // Só a pergunta do CLI: o navegador vai para o app de outra máquina antes do app desta.
        if verb == "host" {
            let inner = self.lock();
            // Sem uso registrado, o app de lá com a sessão em foco ainda serve a quem perdeu o app daqui; a hora 0
            // perde para qualquer foco gravado por um app vivo desta máquina. Sem app ligado a hora segue, marcada
            // `detached`: ela ainda vence o foco daqui, mas o app daqui caído não passa a sessão a ninguém.
            return match (inner.seen.get(session), inner.chosen(session).is_some()) {
                (Some(ms), true) => Ok(Reply::Text(format!("ok: host {ms}"))),
                (Some(ms), false) => Ok(Reply::Text(format!("ok: host {ms} detached"))),
                (None, true) => Ok(Reply::Text("ok: host 0".into())),
                (None, false) => Err(Refusal::NoHost),
            };
        }
        let url = matches!(verb.as_str(), "open" | "goto")
            .then(|| body.get("args").and_then(|a| a.get(0)).and_then(Value::as_str).map(str::to_owned))
            .flatten();
        self.dispatch(session, body, url, None).await
    }

    /// O `fill-secret` do `vault_fill`, só de dentro do servidor. O valor vai no quadro ao anfitrião e em nenhum outro
    /// lugar: nem no pedido em curso, nem no estado, nem no `list`; `expected_host` é o domínio aprovado, que o app
    /// confere na página antes de digitar.
    pub async fn fill_secret(&self, session: &str, args: &[String], expected_host: &str, value: Zeroizing<String>)
        -> Result<Reply, Refusal> {
        let body = json!({"verbo": FILL_SECRET, "args": args, "expected_host": expected_host});
        let Value::Object(body) = body else { return Err(Refusal::NoHost) };
        self.dispatch(session, body, None, Some(value)).await
    }

    /// Manda o pedido ao anfitrião com foco na sessão e espera a resposta. `secret` entra só no texto do quadro.
    async fn dispatch(&self, session: &str, mut body: Map<String, Value>, url: Option<String>, secret: Option<Zeroizing<String>>)
        -> Result<Reply, Refusal> {
        let deadline = deadline_of(&body);
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
            // Antes de montar o quadro: app antigo trataria o verbo como desconhecido, com o valor já na mão dele.
            if secret.is_some() && !entry.fill_secret { return Err(Refusal::Outdated); }
            let frame = frame(body, secret.as_deref().map(String::as_str));
            // Canal já fechando: o verbo não saiu, então não "pode ter sido executado".
            entry.out.send(frame).map_err(|_| Refusal::NoHost)?;
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

    /// Quando (ms) um app de outra máquina mostrou esta sessão por último; quem a mostrou depois, ele ou o app desta
    /// máquina, recebe o navegador.
    pub fn last_seen(&self, session: &str) -> Option<u64> { self.lock().seen.get(session).copied() }

    /// Algum app de outra máquina ligado está com a sessão em foco.
    pub fn has_host(&self, session: &str) -> bool { self.lock().chosen(session).is_some() }

    /// O app que receberia o `fill_secret` agora o aceita. Conferido antes de ler o cofre: a recusa depois da aprovação
    /// faria a pessoa aprovar à toa.
    pub fn can_fill_secret(&self, session: &str) -> Result<(), Refusal> {
        let inner = self.lock();
        let host = inner.chosen(session).ok_or(Refusal::NoHost)?;
        if inner.hosts.get(&host).is_some_and(|h| h.fill_secret) { Ok(()) } else { Err(Refusal::Outdated) }
    }

    /// O `list` do CLI: as sessões com navegador conhecido; vivo = algum anfitrião ligado com foco nela.
    pub fn list(&self) -> String {
        let inner = self.lock();
        if inner.state.is_empty() {
            return "nenhum navegador registrado".into();
        }
        inner.state.iter().map(|(session, st)| {
            let live = if inner.chosen(session).is_some() { "vivo".to_owned() } else { format!("MORTO (reabra com: hangar-preview open {})", st.url) };
            let tab = st.tab.as_i64().map(|n| format!("  (aba {n})")).unwrap_or_default();
            format!("{session}  {}  {live}{tab}", st.url)
        }).collect::<Vec<_>>().join("\n")
    }

    fn set_caps(&self, host: u64, caps: Option<&Value>) {
        let fills = caps.and_then(Value::as_array).is_some_and(|caps| caps.iter().any(|c| c.as_str() == Some(FILL_SECRET_CAP)));
        if let Some(entry) = self.lock().hosts.get_mut(&host) { entry.fill_secret = fills; }
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

    /// A pessoa usou a sessão no app de lá (clique ou tecla): ele passa a receber o navegador. Reconectar só manda
    /// `focus`, que não mexe nesta hora.
    fn seen(&self, host: u64, session: String) {
        self.focus(host, session.clone());
        let mut inner = self.lock();
        if !inner.seen.contains_key(&session) && inner.seen.len() >= MAX_SESSIONS
            && let Some(old) = inner.seen.iter().min_by_key(|(_, ms)| **ms).map(|(k, _)| k.clone())
        {
            inner.seen.remove(&old);
        }
        inner.seen.insert(session, now_ms());
    }

    fn set_state(&self, session: String, url: Option<String>, tab: Option<Value>) {
        let mut inner = self.lock();
        let entry = inner.state_entry(session);
        if let Some(url) = url {
            entry.url = url;
        }
        if let Some(tab) = tab {
            entry.tab = tab;
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
            (Some("focus"), Some(session)) => {
                self.registry.set_caps(self.id, msg.get("caps"));
                return self.registry.focus(self.id, session);
            }
            (Some("seen"), Some(session)) => return self.registry.seen(self.id, session),
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
            Some(true) => {
                let text = msg.get("text").and_then(Value::as_str).unwrap_or_default().to_owned();
                match msg.get("code").and_then(Value::as_str).filter(|code| is_code(code)) {
                    Some(code) if text.starts_with("erro:") => Ok(Reply::Error(code.to_owned(), text)),
                    _ => Ok(Reply::Text(text)),
                }
            }
            Some(false) => {
                let error = msg.get("error").and_then(Value::as_str).unwrap_or_default();
                if error == NOT_ALLOWED {
                    Err(Refusal::NotAllowed)
                } else if error.starts_with("erro:") {
                    Ok(Reply::Text(error.to_owned()))
                } else {
                    Ok(Reply::Text(format!("erro: {error}")))
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
