//! Quem decide se o aviso sai e o manda pelos dois canais (Web Push e Expo), como o `push.py`:
//! mudo por sessão, horário de silêncio, convidado escondido e dono no PC seguram todo tipo de aviso;
//! "aguardando" quase juntos viram um aviso só.
use super::{store, webpush};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Janela em que vários "aguardando" viram um aviso agregado.
pub const COALESCE: Duration = Duration::from_secs(2);
/// Tag constante: o service worker troca o card agregado em vez de empilhar.
pub const COALESCE_TAG: &str = "cp-awaiting-coalesced";

/// As frases do `push._MSG`; idioma desconhecido cai no pt (inscrição antiga).
pub fn msg(locale: &str, key: &str) -> &'static str {
    let en = locale == "en";
    match key {
        "aguardando" => if en { "Awaiting your reply" } else { "Aguardando sua resposta" },
        "terminou" => if en { "Finished" } else { "Terminou" },
        "caiu" => if en { "Died" } else { "Caiu" },
        "travada" => if en { "May be stuck" } else { "Pode estar travada" },
        "limite" => if en { "Usage limit reached" } else { "Limite de uso atingido" },
        "volta" => if en { "resets {reset}" } else { "volta {reset}" },
        "sessoes" => if en { "{n} sessions waiting" } else { "{n} sessões aguardando" },
        _ => "",
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Notice { Awaiting(Option<String>), Finished(Option<String>), Dead, Stalled, Limited(Option<String>) }

impl Notice {
    pub fn body(&self, locale: &str) -> String {
        match self {
            Self::Awaiting(Some(q)) => q.clone(),
            Self::Awaiting(None) => msg(locale, "aguardando").to_owned(),
            Self::Finished(Some(r)) => format!("{}: {r}", msg(locale, "terminou")),
            Self::Finished(None) => msg(locale, "terminou").to_owned(),
            Self::Dead => msg(locale, "caiu").to_owned(),
            Self::Stalled => msg(locale, "travada").to_owned(),
            Self::Limited(Some(reset)) => format!("{} · {}", msg(locale, "limite"), msg(locale, "volta").replace("{reset}", reset)),
            Self::Limited(None) => msg(locale, "limite").to_owned(),
        }
    }
}

/// Um aviso pronto para os canais: o que muda por idioma é calculado por inscrição.
pub struct Outgoing {
    pub session: String,
    pub body: Box<dyn Fn(&str) -> String + Send + Sync>,
    pub title: Option<Box<dyn Fn(&str) -> String + Send + Sync>>,
    pub tag: Option<&'static str>,
    pub url: Option<String>,
}

impl Outgoing {
    pub fn single(session: &str, notice: Notice) -> Self {
        Self { session: session.to_owned(), body: Box::new(move |l| notice.body(l)), title: None, tag: None, url: None }
    }

    /// Várias sessões no mesmo ciclo: "N sessões aguardando: a, b", abre a lista.
    pub fn coalesced(names: &BTreeSet<String>) -> Self {
        let joined = names.iter().cloned().collect::<Vec<_>>().join(", ");
        let n = names.len().to_string();
        let (n2, j2) = (n.clone(), joined.clone());
        Self {
            session: joined,
            body: Box::new(move |l| format!("{}: {j2}", msg(l, "sessoes").replace("{n}", &n2))),
            title: Some(Box::new(move |l| msg(l, "sessoes").replace("{n}", &n))),
            tag: Some(COALESCE_TAG), url: Some("/".to_owned()),
        }
    }

    pub fn title_suffix(&self, locale: &str) -> String { self.title.as_ref().map_or_else(|| self.session.clone(), |t| t(locale)) }

    /// O JSON que o service worker lê.
    pub fn payload(&self, entry: &Value) -> Value {
        let label = entry["label"].as_str().filter(|l| !l.is_empty()).unwrap_or("claude");
        let locale = entry["locale"].as_str().filter(|l| !l.is_empty()).unwrap_or("pt");
        let url = self.url.clone().unwrap_or_else(|| format!("/?server={}&session={}", entry["serverId"].as_str().unwrap_or(""), self.session));
        let mut p = json!({"title": format!("{label} · {}", self.title_suffix(locale)), "body": (self.body)(locale), "session": self.session, "url": url});
        if let Some(tag) = self.tag { p["tag"] = json!(tag); }
        p
    }
}

/// O que segura um aviso, lido na hora (arquivo de preferências, convidados e presença).
pub struct Gate { pub muted: bool, pub quiet: bool, pub hidden_guest: bool, pub owner_at_pc: bool }

impl Gate {
    pub fn suppressed(&self) -> bool { self.muted || self.quiet || self.hidden_guest || self.owner_at_pc }
}

pub struct Sender {
    st: Arc<crate::routes::AppState>,
    env: BTreeMap<String, String>,
    pending: Mutex<Option<BTreeMap<String, Option<String>>>>,
}

impl Sender {
    pub fn new(st: Arc<crate::routes::AppState>, env: BTreeMap<String, String>) -> Arc<Self> {
        Arc::new(Self { st, env, pending: Mutex::new(None) })
    }

    fn dir(&self) -> PathBuf { store::dir(&self.st.accounts.env.claude_base) }

    pub fn gate(&self, session: &str) -> Gate {
        let prefs = store::prefs(&self.dir());
        let guests = crate::list::local_facts::Guests::read(&self.st.accounts.env.claude_base);
        Gate {
            muted: store::is_muted(&prefs, session),
            quiet: store::in_quiet_hours(&prefs, chrono::Local::now().time()),
            hidden_guest: guests.access(&[(session.to_owned(), None)]).1.iter().any(|h| h == session),
            owner_at_pc: crate::presence::present(&self.st),
        }
    }

    /// Todo aviso que não é "aguardando".
    pub fn notify(self: &Arc<Self>, session: &str, notice: Notice) {
        if self.gate(session).suppressed() { return; }
        let me = self.clone();
        let out = Outgoing::single(session, notice);
        tokio::spawn(async move { me.broadcast(out).await });
    }

    /// "Aguardando": entra na janela de 2 s; quem chega nela sai junto.
    pub fn awaiting(self: &Arc<Self>, session: &str, question: Option<String>) {
        if self.gate(session).suppressed() { return; }
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        let first = pending.is_none();
        pending.get_or_insert_with(BTreeMap::new).insert(session.to_owned(), question);
        drop(pending);
        if first {
            let me = self.clone();
            tokio::spawn(async move {
                tokio::time::sleep(COALESCE).await;
                me.flush().await;
            });
        }
    }

    pub async fn flush(&self) {
        let Some(buf) = self.pending.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
        let out = match buf.len() {
            0 => return,
            1 => { let (name, body) = buf.into_iter().next().unwrap(); Outgoing::single(&name, Notice::Awaiting(body)) }
            _ => Outgoing::coalesced(&buf.into_keys().collect()),
        };
        self.broadcast(out).await;
    }

    /// Expo primeiro e por conta própria (não depende do VAPID); depois cada inscrição do Web Push,
    /// podando as mortas.
    pub async fn broadcast(&self, out: Outgoing) {
        let pick = |f: &dyn Fn(&str) -> String| (f("pt"), f("en"));
        crate::push_expo::send(&self.st, &out.session, pick(&|l| out.title_suffix(l)), pick(&*out.body), out.url.as_deref() == Some("/"));
        let Some(vapid) = webpush::vapid(&self.env) else { return };
        let dir = self.dir();
        let subs = store::subs(&dir);
        if subs.is_empty() { return; }
        static CLIENT: std::sync::OnceLock<Option<reqwest::Client>> = std::sync::OnceLock::new();
        let Some(client) = CLIENT.get_or_init(|| crate::http_client().timeout(Duration::from_secs(10)).build().ok()) else { return };
        let mut dead = Vec::new();
        for entry in &subs {
            let payload = out.payload(entry).to_string();
            match webpush::send(client, &vapid, &entry["subscription"], payload.as_bytes()).await {
                webpush::Sent::Ok => {}
                webpush::Sent::Gone => dead.extend(entry["subscription"]["endpoint"].as_str().map(str::to_owned)),
                webpush::Sent::Failed(code) => self.st.diag.report("rust.push_web_failed", "push", code, "aviso Web Push não saiu; a inscrição fica"),
            }
        }
        if !dead.is_empty() {
            let dir2 = dir.clone();
            if !matches!(tokio::task::spawn_blocking(move || store::prune(&dir2, &dead)).await, Ok(Ok(()))) {
                self.st.diag.report("rust.push_web_failed", "push", "push_store_unwritable", "inscrição morta não saiu do arquivo");
            }
        }
    }
}
