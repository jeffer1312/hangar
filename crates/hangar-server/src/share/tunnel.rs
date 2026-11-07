//! Funnel (internet) só para a porta do convidado. A 443 é o `serve` do dono e a 10000 a prévia
//! de porta; das portas que o Funnel aceita, a 8443 é a que sobra.
use futures_util::future::BoxFuture;
use serde_json::Value;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

pub const FUNNEL_PORT: u16 = 8443;
pub use crate::connect::GUEST_PORT;
pub const FIX_OPERATOR: &str = "sudo tailscale set --operator=$USER";
pub const FIX_FUNNEL: &str = "libere o Funnel na política da tailnet (nodeAttrs \"funnel\"): https://login.tailscale.com/admin/acls/file";
const TIMEOUT: Duration = Duration::from_secs(20);
const HOST_TTL: Duration = Duration::from_secs(300);
const FAILURE_TTL: Duration = Duration::from_secs(15);
const NOT_INSTALLED: &str = "tailscale nao encontrado";
const NOT_LOGGED_IN: &str = "tailscale sem DNSName (logado?)";
static LOGIN_URL: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"https://login\.tailscale\.com/\S+").expect("regex do link"));

pub struct Output { pub success: bool, pub stdout: String, pub stderr: String }

pub trait Tailscale: Send + Sync {
    fn run<'a>(&'a self, args: &'a [&'a str]) -> BoxFuture<'a, Result<Output, String>>;
    // Operador é coisa do tailscaled do Linux; no Windows e no macOS o app do Tailscale não o usa.
    fn needs_operator(&self) -> bool { cfg!(target_os = "linux") }
    // ponytail: só o ambiente (como o getpass do Python antes do pwd); sem USER/LOGNAME o operador acusa falta.
    fn user(&self) -> String {
        ["LOGNAME", "USER", "LNAME", "USERNAME"].iter()
            .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty())).unwrap_or_default()
    }
}

/// O `tailscale` de verdade, sem herdar a autoridade do servidor.
pub struct Real;
impl Tailscale for Real {
    fn run<'a>(&'a self, args: &'a [&'a str]) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            let mut command = tokio::process::Command::new("tailscale");
            command.args(args).stdin(std::process::Stdio::null()).kill_on_drop(true);
            for key in crate::terminal_process::PRIVATE_ENV_KEYS { command.env_remove(key); }
            #[cfg(windows)] { command.creation_flags(0x08000000); } // CREATE_NO_WINDOW: sem console piscando.
            let out = match tokio::time::timeout(TIMEOUT, command.output()).await {
                Err(_) => return Err("tailscale timeout".into()),
                Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => return Err(NOT_INSTALLED.into()),
                Ok(Err(e)) => return Err(format!("tailscale falhou: {e}")),
                Ok(Ok(out)) => out,
            };
            Ok(Output {
                success: out.status.success(),
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            })
        })
    }
}

/// O que já se sabe do túnel sem perguntar de novo ao `tailscale`.
#[derive(Default)]
pub struct Memo {
    host: Mutex<Option<(Instant, String)>>,
    failure: Mutex<Option<(Instant, String)>>,
    /// Último estado do Funnel confirmado ou aplicado; `None` = não se sabe.
    applied: Mutex<Option<bool>>,
}

impl Memo {
    pub fn applied(&self) -> Option<bool> { *self.applied.lock().unwrap_or_else(|e| e.into_inner()) }
    pub fn set_applied(&self, on: Option<bool>) { *self.applied.lock().unwrap_or_else(|e| e.into_inner()) = on; }
    pub fn forget_failure(&self) { *self.failure.lock().unwrap_or_else(|e| e.into_inner()) = None; }
}

/// Só a resposta do `tailscale` confirma: timeout ou daemon reiniciando não dizem que ele não existe aqui.
pub fn confirmed_absent(fix: &str) -> bool { fix == NOT_INSTALLED || fix == NOT_LOGGED_IN }

/// `host` lembrado por `HOST_TTL` e a falha por `FAILURE_TTL`: a página e o resgate vêm da internet e não abrem
/// um processo por pedido.
pub async fn cached_host(ts: &dyn Tailscale, memo: &Memo) -> Result<String, String> {
    if let Some((at, h)) = &*memo.host.lock().unwrap_or_else(|e| e.into_inner()) && at.elapsed() < HOST_TTL {
        return Ok(h.clone());
    }
    if let Some((at, fix)) = &*memo.failure.lock().unwrap_or_else(|e| e.into_inner()) && at.elapsed() < FAILURE_TTL {
        return Err(fix.clone());
    }
    match host(ts).await {
        Ok(h) => {
            memo.forget_failure();
            *memo.host.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), h.clone()));
            Ok(h)
        }
        Err(fix) => {
            *memo.failure.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), fix.clone()));
            Err(fix)
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Prereqs { pub missing: Vec<String>, pub fix: String, pub enable_url: Option<String> }

#[derive(Debug, Clone, PartialEq)]
pub struct TunnelError { pub missing: Vec<String>, pub fix: String, pub enable_url: Option<String> }

impl TunnelError {
    pub(super) fn plain(fix: impl Into<String>) -> Self { Self { missing: vec![], fix: fix.into(), enable_url: None } }
}

fn or_default(text: &str, default: &str) -> String {
    if text.is_empty() { default } else { text }.trim().to_owned()
}

async fn call(ts: &dyn Tailscale, args: &[&str]) -> Result<Output, TunnelError> {
    ts.run(args).await.map_err(TunnelError::plain)
}

async fn status(ts: &dyn Tailscale) -> Result<Value, TunnelError> {
    let p = call(ts, &["status", "--json"]).await?;
    match serde_json::from_str::<Value>(&p.stdout).ok().and_then(|v| v.get("Self").cloned()) {
        Some(s @ Value::Object(_)) => Ok(s),
        _ => Err(TunnelError::plain(or_default(&p.stderr, "tailscale status falhou"))),
    }
}

fn dns_name(s: &Value) -> String {
    s.get("DNSName").and_then(Value::as_str).unwrap_or("").trim_end_matches('.').to_owned()
}

pub async fn host(ts: &dyn Tailscale) -> Result<String, String> {
    let name = dns_name(&status(ts).await.map_err(|e| e.fix)?);
    if name.is_empty() { return Err(NOT_LOGGED_IN.into()); }
    Ok(name)
}

async fn missing(ts: &dyn Tailscale, s: &Value) -> Result<Vec<String>, TunnelError> {
    let mut missing = vec![];
    if ts.needs_operator() {
        let p = call(ts, &["debug", "prefs"]).await?;
        let operator = serde_json::from_str::<Value>(&p.stdout).ok()
            .and_then(|v| v.get("OperatorUser").and_then(Value::as_str).map(str::to_owned)).unwrap_or_default();
        if operator != ts.user() { missing.push("operator".to_owned()); }
    }
    let funnel = s.get("CapMap").and_then(Value::as_object)
        .is_some_and(|caps| caps.keys().any(|k| k == "funnel" || k.starts_with("https://tailscale.com/cap/funnel")));
    if !funnel { missing.push("funnel".to_owned()); }
    Ok(missing)
}

async fn prereqs_of(ts: &dyn Tailscale, s: &Value) -> Result<Prereqs, TunnelError> {
    let missing = missing(ts, s).await?;
    let fix = missing.iter().map(|m| if m == "operator" { FIX_OPERATOR } else { FIX_FUNNEL }).collect::<Vec<_>>().join("\n");
    let node = s.get("ID").and_then(Value::as_str).unwrap_or("");
    let enable_url = (!node.is_empty() && missing.iter().any(|m| m == "funnel"))
        .then(|| format!("https://login.tailscale.com/f/funnel?node={node}"));
    Ok(Prereqs { missing, fix, enable_url })
}

/// O que falta para o Funnel subir, sem ligar nada.
pub async fn prereqs(ts: &dyn Tailscale) -> Result<Prereqs, String> {
    let s = status(ts).await.map_err(|e| e.fix)?;
    prereqs_of(ts, &s).await.map_err(|e| e.fix)
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

async fn is_on(ts: &dyn Tailscale) -> Result<bool, TunnelError> {
    let p = call(ts, &["funnel", "status", "--json"]).await?;
    let text = if p.stdout.is_empty() { "{}" } else { &p.stdout };
    let Ok(v) = serde_json::from_str::<Value>(text) else { return Ok(false) };
    let suffix = format!(":{FUNNEL_PORT}");
    Ok(v.get("AllowFunnel").and_then(Value::as_object)
        .is_some_and(|allow| allow.iter().any(|(k, on)| k.ends_with(&suffix) && truthy(on))))
}

/// Endereço do Funnel desta máquina, conferidos os pré-requisitos; não liga nada.
async fn ready(ts: &dyn Tailscale) -> Result<String, TunnelError> {
    let s = status(ts).await?;
    let pre = prereqs_of(ts, &s).await?;
    if !pre.missing.is_empty() {
        return Err(TunnelError { missing: pre.missing, fix: pre.fix, enable_url: pre.enable_url });
    }
    Ok(format!("https://{}:{FUNNEL_PORT}", dns_name(&s)))
}

pub async fn ensure_on(ts: &dyn Tailscale) -> Result<String, TunnelError> {
    let base = ready(ts).await?;
    if !is_on(ts).await? { open(ts).await?; }
    Ok(base)
}

async fn open(ts: &dyn Tailscale) -> Result<(), TunnelError> {
    let https = format!("--https={FUNNEL_PORT}");
    let target = format!("http://127.0.0.1:{GUEST_PORT}");
    let p = call(ts, &["funnel", "--bg", "--yes", &https, &target]).await?;
    if !p.success {
        // A tailnet recusou o Funnel e o próprio tailscale imprimiu o link para liberar.
        if let Some(hit) = LOGIN_URL.find(&format!("{}{}", p.stdout, p.stderr)) {
            let url = hit.as_str().trim_end_matches(['.', ',', ';', ')']).to_owned();
            return Err(TunnelError { missing: vec!["funnel".into()], fix: FIX_FUNNEL.into(), enable_url: Some(url) });
        }
        let text = if !p.stderr.is_empty() { &p.stderr } else { &p.stdout };
        return Err(TunnelError::plain(or_default(text, "tailscale funnel falhou")));
    }
    Ok(())
}

pub async fn ensure_off(ts: &dyn Tailscale) -> Result<(), TunnelError> {
    let https = format!("--https={FUNNEL_PORT}");
    let p = call(ts, &["funnel", &https, "off"]).await?;
    if !p.success && !format!("{}{}", p.stdout, p.stderr).contains("does not exist") {
        return Err(TunnelError::plain(or_default(&p.stderr, "falha ao desligar o funnel")));
    }
    Ok(())
}

/// `Ok(true)` = o Funnel ficou como `active` pede; `Ok(false)` = o `tailscale` não respondeu e
/// não havia nada a ligar.
pub async fn sync(ts: &dyn Tailscale, active: bool) -> Result<bool, TunnelError> {
    let on = match is_on(ts).await {
        Ok(on) => on,
        // Sem tailscale e sem convite: nada a desligar, e o laço periódico não vira log por minuto.
        Err(e) if active => return Err(e),
        Err(_) => return Ok(false),
    };
    if active && !on {
        ready(ts).await?;
        open(ts).await?;
    } else if on && !active {
        ensure_off(ts).await?;
    }
    Ok(true)
}
