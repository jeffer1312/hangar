//! Decisão e envio pelos dois canais, Web Push e Expo:
//! mudo por sessão, horário de silêncio, convidado escondido e dono no PC seguram todo tipo de aviso;
//! "aguardando" quase juntos viram um aviso só.
use super::{store, webpush};
use serde_json::{Value, json};
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
        "aguardando" => {
            if en {
                "Awaiting your reply"
            } else {
                "Aguardando sua resposta"
            }
        }
        "terminou" => {
            if en {
                "Finished"
            } else {
                "Terminou"
            }
        }
        "caiu" => {
            if en {
                "Died"
            } else {
                "Caiu"
            }
        }
        "travada" => {
            if en {
                "May be stuck"
            } else {
                "Pode estar travada"
            }
        }
        "limite" => {
            if en {
                "Usage limit reached"
            } else {
                "Limite de uso atingido"
            }
        }
        "volta" => {
            if en {
                "resets {reset}"
            } else {
                "volta {reset}"
            }
        }
        "sessoes" => {
            if en {
                "{n} sessions waiting"
            } else {
                "{n} sessões aguardando"
            }
        }
        "loop_done" => {
            if en {
                "loop finished: {reason}"
            } else {
                "loop concluído: {reason}"
            }
        }
        "loop_done_claimed" => {
            if en {
                "Claude declared done — confirm?"
            } else {
                "Claude declarou pronto — confirmar?"
            }
        }
        "loop_stopped" => {
            if en {
                "loop stopped: {reason}"
            } else {
                "loop parado: {reason}"
            }
        }
        "loop_exhausted" => {
            if en {
                "loop exhausted its iterations: {reason}"
            } else {
                "loop esgotou as iterações: {reason}"
            }
        }
        "loop_failed" => {
            if en {
                "loop failed: {reason}"
            } else {
                "loop falhou: {reason}"
            }
        }
        _ => "",
    }
}

/// Pergunta longa estoura o payload do Web Push (~4 KB) e o aviso some: corta como o `summary`.
fn clip(text: &str) -> String {
    if text.chars().count() <= 160 {
        return text.to_owned();
    }
    format!("{}…", text.chars().take(159).collect::<String>().trim_end())
}

#[derive(Clone, Debug, PartialEq)]
pub enum Notice {
    Awaiting(Option<String>),
    Finished(Option<String>),
    Dead,
    Stalled,
    Limited(Option<String>),
    /// Desfecho do loop: a frase fixa no idioma da inscrição, o motivo como veio.
    Loop {
        status: String,
        reason: String,
    },
}

impl Notice {
    pub fn body(&self, locale: &str) -> String {
        match self {
            Self::Awaiting(Some(q)) => clip(q),
            Self::Awaiting(None) => msg(locale, "aguardando").to_owned(),
            Self::Finished(Some(r)) => format!("{}: {r}", msg(locale, "terminou")),
            Self::Finished(None) => msg(locale, "terminou").to_owned(),
            Self::Dead => msg(locale, "caiu").to_owned(),
            Self::Stalled => msg(locale, "travada").to_owned(),
            Self::Limited(Some(reset)) => format!(
                "{} · {}",
                msg(locale, "limite"),
                msg(locale, "volta").replace("{reset}", reset)
            ),
            Self::Limited(None) => msg(locale, "limite").to_owned(),
            Self::Loop { status, reason } => match msg(locale, &format!("loop_{status}")) {
                "" => reason.clone(),
                text => text.replace("{reason}", reason),
            },
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
        Self {
            session: session.to_owned(),
            body: Box::new(move |l| notice.body(l)),
            title: None,
            tag: None,
            url: None,
        }
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
            tag: Some(COALESCE_TAG),
            url: Some("/".to_owned()),
        }
    }

    pub fn title_suffix(&self, locale: &str) -> String {
        self.title
            .as_ref()
            .map_or_else(|| self.session.clone(), |t| t(locale))
    }

    /// O JSON que o service worker lê.
    pub fn payload(&self, entry: &Value) -> Result<Value, &'static str> {
        use crate::{config_sync::apply::truthy, orchestration::codec::python_string};
        let object = entry.as_object().ok_or("push_subscription_invalid")?;
        let locale_value = object.get("locale").filter(|value| truthy(value));
        if locale_value.is_some_and(|value| value.is_array() || value.is_object()) {
            return Err("push_subscription_invalid");
        }
        let label = object
            .get("label")
            .filter(|value| truthy(value))
            .map(python_string)
            .unwrap_or_else(|| "claude".into());
        let locale = locale_value.and_then(Value::as_str).unwrap_or("pt");
        let url = self.url.clone().unwrap_or_else(|| {
            format!(
                "/?server={}&session={}",
                object
                    .get("serverId")
                    .map(python_string)
                    .unwrap_or_default(),
                self.session
            )
        });
        let mut p = json!({"title": format!("{label} · {}", self.title_suffix(locale)), "body": (self.body)(locale), "session": self.session, "url": url});
        if let Some(tag) = self.tag {
            p["tag"] = json!(tag);
        }
        Ok(p)
    }
}

/// O que segura um aviso, lido na hora (arquivo de preferências, convidados e presença).
pub struct Gate {
    pub muted: bool,
    pub quiet: bool,
    pub hidden_guest: bool,
    pub owner_at_pc: bool,
}

impl Gate {
    pub fn suppressed(&self) -> bool {
        self.muted || self.quiet || self.hidden_guest || self.owner_at_pc
    }
}

pub struct Sender {
    st: Arc<crate::routes::AppState>,
    env: BTreeMap<String, String>,
    dir: PathBuf,
    pending: Mutex<Option<BTreeMap<String, Option<String>>>>,
}

impl Sender {
    pub fn from_state(st: Arc<crate::routes::AppState>) -> Result<Arc<Self>, &'static str> {
        let config = super::config::Frozen::from_state(&st)?;
        Ok(Arc::new(Self {
            st,
            env: config.env,
            dir: config.dir,
            pending: Mutex::new(None),
        }))
    }
    #[cfg(test)]
    pub fn new(st: Arc<crate::routes::AppState>, env: BTreeMap<String, String>) -> Arc<Self> {
        let config = super::config::Frozen::from_state(&st).expect("configuração isolada de prova");
        Arc::new(Self {
            st,
            env,
            dir: config.dir,
            pending: Mutex::new(None),
        })
    }
    fn dir(&self) -> PathBuf {
        self.dir.clone()
    }

    /// O convidado escondido é decidido pela vida da sessão, como na lista: o pedido velho de um
    /// convidado não cala a sessão nova do dono com o mesmo nome.
    pub async fn gate(&self, session: &str) -> Gate {
        let blocked = || {
            self.st.diag.report(
                "rust.push_gate_failed",
                session,
                "push_gate_invalid",
                "preferências incompatíveis; aviso segurado",
            );
            Gate {
                muted: true,
                quiet: false,
                hidden_guest: false,
                owner_at_pc: false,
            }
        };
        let dir = self.dir();
        let prefs = match tokio::task::spawn_blocking(move || store::prefs(&dir)).await {
            Ok(Ok(value)) => value,
            _ => return blocked(),
        };
        let muted = store::is_muted(&prefs, session);
        if !matches!(muted, Ok(false)) {
            crate::orchestration::codec::discard(prefs);
            return if muted.is_err() {
                blocked()
            } else {
                Gate {
                    muted: true,
                    quiet: false,
                    hidden_guest: false,
                    owner_at_pc: false,
                }
            };
        }
        let quiet = store::in_quiet_hours(&prefs, chrono::Local::now().time());
        crate::orchestration::codec::discard(prefs);
        if !matches!(quiet, Ok(false)) {
            return if quiet.is_err() {
                blocked()
            } else {
                Gate {
                    muted: false,
                    quiet: true,
                    hidden_guest: false,
                    owner_at_pc: false,
                }
            };
        }
        let service = self.st.login_guests.clone();
        let name = session.to_owned();
        let claimed = tokio::task::spawn_blocking(move || {
            service
                .store
                .snapshot(true)
                .map(|doc| {
                    let claimed = doc["sessions"].get(&name).is_some();
                    crate::orchestration::codec::discard(doc);
                    claimed
                })
                .unwrap_or(false)
        })
        .await
        .unwrap_or(false);
        let life = if claimed {
            crate::login_guests::gate::session_life(&self.st, session).await
        } else {
            None
        };
        let service = self.st.login_guests.clone();
        let name = session.to_owned();
        let hidden_guest = tokio::task::spawn_blocking(move || {
            service
                .store
                .owner(&name, life.as_deref(), true)
                .is_some_and(|guest| !guest.owner_sees)
        })
        .await
        .unwrap_or(true);
        Gate {
            muted: false,
            quiet: false,
            hidden_guest,
            owner_at_pc: !hidden_guest && crate::presence::present(&self.st),
        }
    }

    /// Todo aviso que não é "aguardando".
    pub fn notify(self: &Arc<Self>, session: &str, notice: Notice) {
        let (me, session) = (self.clone(), session.to_owned());
        tokio::spawn(async move {
            if me.gate(&session).await.suppressed() {
                return;
            }
            me.broadcast(Outgoing::single(&session, notice)).await
        });
    }

    /// "Aguardando": entra na janela de 2 s; quem chega nela sai junto.
    pub fn awaiting(self: &Arc<Self>, session: &str, question: Option<String>) {
        let (me, session) = (self.clone(), session.to_owned());
        tokio::spawn(async move {
            if me.gate(&session).await.suppressed() {
                return;
            }
            let first = {
                let mut pending = me.pending.lock().unwrap_or_else(|e| e.into_inner());
                let first = pending.is_none();
                pending
                    .get_or_insert_with(BTreeMap::new)
                    .insert(session, question);
                first
            };
            if first {
                tokio::time::sleep(COALESCE).await;
                me.flush().await;
            }
        });
    }

    pub async fn flush(&self) {
        let Some(buf) = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        else {
            return;
        };
        let out = match buf.len() {
            0 => return,
            1 => {
                let (name, body) = buf.into_iter().next().unwrap();
                Outgoing::single(&name, Notice::Awaiting(body))
            }
            _ => Outgoing::coalesced(&buf.into_keys().collect()),
        };
        self.broadcast(out).await;
    }

    /// Expo primeiro e por conta própria (não depende do VAPID); depois cada inscrição do Web Push,
    /// podando as mortas.
    pub async fn broadcast(&self, out: Outgoing) {
        let pick = |f: &dyn Fn(&str) -> String| (f("pt"), f("en"));
        crate::push_expo::send(
            &self.st,
            &out.session,
            pick(&|l| out.title_suffix(l)),
            pick(&*out.body),
            out.url.as_deref() == Some("/"),
        );
        let Some(vapid) = webpush::vapid(&self.env) else {
            // Sem chave o Web Push está desligado; chave pública presente e par inválido é falha.
            if !webpush::public_key(&self.env).is_empty() {
                self.st.diag.report(
                    "rust.push_web_failed",
                    "push",
                    "push_vapid_invalid",
                    "chave VAPID inválida; Web Push não saiu",
                );
            }
            return;
        };
        let _delivery = store::delivery_lock().await;
        let dir = self.dir();
        let read_dir = dir.clone();
        let subs = match tokio::task::spawn_blocking(move || store::subs(&read_dir))
            .await
            .unwrap_or(Err("push_store_unreadable"))
        {
            Ok(subs) => subs,
            Err(code) => {
                self.st.diag.report(
                    "rust.push_web_failed",
                    "push",
                    code,
                    "inscrições ilegíveis; Web Push não saiu",
                );
                return;
            }
        };
        if subs.is_empty() {
            return;
        }
        static CLIENT: std::sync::OnceLock<Option<reqwest::Client>> = std::sync::OnceLock::new();
        let Some(client) = CLIENT.get_or_init(|| {
            crate::http_client()
                .timeout(Duration::from_secs(10))
                .build()
                .ok()
        }) else {
            crate::orchestration::codec::discard(Value::Array(subs));
            self.st.diag.report(
                "rust.push_web_failed",
                "push",
                "push_client_invalid",
                "cliente Web Push indisponível; inscrições preservadas",
            );
            return;
        };
        let mut dead = Vec::new();
        let mut invalid = None;
        for entry in &subs {
            let value = match out.payload(entry) {
                Ok(value) => value,
                Err(code) => {
                    invalid = Some(code);
                    break;
                }
            };
            let payload = crate::orchestration::codec::dumps_utf8(&value);
            crate::orchestration::codec::discard(value);
            let payload = match payload {
                Ok(payload) => payload,
                Err(_) => {
                    self.st.diag.report(
                        "rust.push_web_failed",
                        "push",
                        "push_encoding_invalid",
                        "aviso incompatível com UTF-8; inscrição preservada",
                    );
                    continue;
                }
            };
            match webpush::send(client, &vapid, &entry["subscription"], payload.as_bytes()).await {
                webpush::Sent::Ok => {}
                webpush::Sent::Gone => dead.extend(
                    entry["subscription"]["endpoint"]
                        .as_str()
                        .map(str::to_owned),
                ),
                webpush::Sent::Failed(code) => self.st.diag.report(
                    "rust.push_web_failed",
                    "push",
                    code,
                    "aviso Web Push não saiu; a inscrição fica",
                ),
            }
        }
        crate::orchestration::codec::discard(Value::Array(subs));
        if let Some(code) = invalid {
            self.st.diag.report(
                "rust.push_web_failed",
                "push",
                code,
                "inscrição incompatível; poda não aplicada",
            );
            return;
        }

        if !dead.is_empty() {
            let dir2 = dir.clone();
            if !matches!(
                tokio::task::spawn_blocking(move || store::prune_locked(&dir2, &dead)).await,
                Ok(Ok(()))
            ) {
                self.st.diag.report(
                    "rust.push_web_failed",
                    "push",
                    "push_store_unwritable",
                    "inscrição morta não saiu do arquivo",
                );
            }
        }
    }
}

#[cfg(test)]
#[path = "send_tests.rs"]
mod tests;
