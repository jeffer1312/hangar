//! Telemetria anônima no servidor: as rotas do dono, o envio diário e a convergência do dono comum
//! entre as máquinas. Contagem, fila e envio são do crate `hangar-telemetry`.
use std::collections::{BTreeMap, HashSet};
use std::io::{self, BufRead, BufReader};
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::body::to_bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};

use hangar_telemetry::diary::{self, DayDetail};
use hangar_telemetry::events::{self, ClientKind, Code, Dims, Event, PeerKind};
use hangar_telemetry::wire::{Counter, Refusal};
use hangar_telemetry::{CONSENT_SCOPE, Collection, Disabled, Failure, Telemetry};

use crate::groups::peers::PeerClient;
use crate::routes::AppState;
use crate::session_write::{detail, json_response};

const OWNER_ONLY: (&str, &str) = ("telemetry_owner_only", "só o dono pode ler ou mudar a telemetria");
const OWNER_PATH: &str = "/api/telemetry/owner";
const CONVERGE_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// O retrato de peers sai uma vez por dia (marca no arquivo do dia); a hora só decide quando ele é tirado.
const SNAPSHOT_EVERY: Duration = Duration::from_secs(60 * 60);
/// O cliente manda `app.visible_time` neste compasso: mais que isso num evento é relógio torto ou repetição.
const VISIBLE_CAP_MS: u64 = 5 * 60 * 1000;
/// `detalhe` de `rota.escolhida` (`packages/core/src/rota.ts`).
const ROUTES: &[&str] = &["principal", "rede_local", "tailscale", "connect"];
/// `codigo` de `rota.falhou`, em minúsculas: nome do erro do JS ou o código do Expo.
const ROUTE_FAILURES: &[&str] = &["error", "timeouterror", "aborterror", "typeerror", "route_timeout", "erro"];
/// `codigo` de `app.crash`, em minúsculas: nome do erro do JS, ou `panic` do nativo.
const FATAL_CODES: &[&str] = &["error", "typeerror", "referenceerror", "rangeerror", "syntaxerror", "panic"];
/// As CLIs da descoberta do provedor padrão; presas, o retrato do dia sai sem ele.
const DISCOVER_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Choice { consent: bool, consent_scope: Option<u32> }

/// O motor desta vida. `None` quando `telemetry.json` não se lê nem se grava: o servidor sobe sem
/// telemetria, e as rotas respondem 503. Bloqueante.
pub fn open(home: &Path, diag: &crate::diag::DiagClient) -> Option<Arc<Telemetry>> {
    let package = crate::update::running_package();
    let config = hangar_telemetry::Config {
        home: home.to_path_buf(),
        backend: crate::session_open::cli::backend_dir_for_home(home),
        version: package.map_or(env!("CARGO_PKG_VERSION"), |p| p.version.as_str()).to_owned(),
        // Fora de pacote o servidor saiu de um checkout: o fio leva `dev`.
        release_commit: package.map(|p| p.commit.clone()),
        channel: crate::update::channel(&crate::update::Layout::new(home)),
    };
    match Telemetry::new(config) {
        Ok(telemetry) => {
            match telemetry.http_refusal_post() {
                Ok(post) => telemetry.set_refusal_post(post),
                Err(_) => diag.report("telemetry.unavailable", "", "telemetry_client_failed", "telemetria sem cliente HTTP: o aviso de recusa não sai"),
            }
            Some(Arc::new(telemetry))
        }
        Err(e) => {
            tracing::warn!(code = "telemetry_unavailable", kind = ?e.kind(), "telemetria desligada nesta vida: estado ilegível");
            diag.report("telemetry.unavailable", "", "telemetry_unavailable", "telemetria desligada nesta vida");
            None
        }
    }
}

/// O envio diário e a convergência do dono; nada sem telemetria nesta vida.
pub fn spawn(st: &Arc<AppState>) -> Vec<tokio::task::JoinHandle<()>> {
    let Some(telemetry) = st.telemetry.clone() else { return Vec::new() };
    let diag = st.diag.clone();
    if let Some(base) = crate::diag::diary_base(st) {
        telemetry.set_detail(Arc::new(move |day, period, now| diary_detail(&base, day, period, now)));
    }
    let mut tasks = Vec::new();
    match telemetry.http_post() {
        Ok(post) => {
            let diag = diag.clone();
            tasks.push(tokio::spawn(telemetry.clone().run(post, move |failure| report(&diag, failure))));
        }
        Err(_) => st.diag.report("telemetry.unavailable", "", "telemetry_client_failed", "telemetria sem cliente HTTP: nada é enviado"),
    }
    let (snapshot, book, snapshot_diag, snapshot_st) = (telemetry.clone(), st.peers.clone(), diag.clone(), st.clone());
    tasks.push(tokio::spawn(async move {
        let mut done: Option<(String, u64)> = None;
        loop {
            let (today, epoch) = (snapshot.today(), snapshot.consent_epoch());
            if snapshot_due(done.as_ref(), &today, epoch) {
                let (st, read_st, diag) = (snapshot_st.clone(), snapshot_st.clone(), snapshot_diag.clone());
                let discover = move || async move { default_provider(&st).await };
                let read = move |provider: Option<String>| {
                    let runtime = crate::config_sync::runtime::RuntimeConfig::from_state(&read_st).ok()
                        .and_then(|config| effective_runtime(&config, crate::voice::settings::env_jev_key()).ok());
                    server_settings(&read_st.accounts.env.home, runtime.as_ref(), provider.as_deref(), &diag)
                };
                match daily_snapshot(snapshot.clone(), book.clone(), snapshot_diag.clone(), today.clone(), discover, read).await {
                    Ok(Ok(true)) => done = Some((today, epoch)),
                    Ok(Ok(false)) => {}
                    Ok(Err(e)) => report(&snapshot_diag, Failure::Disk(e)),
                    Err(_) => task_failed(&snapshot_diag),
                }
            }
            tokio::time::sleep(SNAPSHOT_EVERY).await;
        }
    }));
    let peers = st.peers.clone();
    tasks.push(tokio::spawn(async move {
        loop {
            converge(&telemetry, &peers, &diag).await;
            tokio::time::sleep(CONVERGE_EVERY).await;
        }
    }));
    tasks
}

/// O dia novo e o aceite novo do mesmo dia (a época sobe a cada aceite) tiram o retrato de novo.
fn snapshot_due(done: Option<&(String, u64)>, today: &str, epoch: u64) -> bool {
    done.is_none_or(|(day, at)| day != today || *at != epoch)
}

pub(crate) fn diary_detail(base: &Path, day: &str, period: &Collection, now: DateTime<Utc>) -> io::Result<DayDetail> {
    match std::fs::symlink_metadata(base) {
        Ok(meta) if meta.file_type().is_dir() => {}
        Ok(_) => return Err(io::Error::other("pasta do diário inválida")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(DayDetail::default()),
        Err(e) => return Err(e),
    }
    // A data no nome é local; só o timestamp completo da linha decide o dia UTC.
    let mut entries = Vec::new();
    for (_, path) in hangar_usage_diary::dated_files(base) {
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        for line in BufReader::new(file).lines() {
            let line = line?;
            let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
            if let Some(entry) = diary::project(&value, day, period, now, crate::mcp::tool_names()) { entries.push(entry); }
        }
    }
    Ok(diary::build(entries))
}

/// Marcas dos retratos do servidor no arquivo do dia, separadas das `settings:<cliente>:<aparelho>` dos apps.
const PEERS_MARK: &str = "peers";
const SERVER_SETTINGS_MARK: &str = "settings:server";

/// Quantas máquinas do `peers.json` há de cada tipo, uma vez por dia (UTC). `Ok(true)` quando o dia já
/// está contado; desligada ou fora do dia e do aceite, `Ok(false)`; dia que não se grava, `Err`. Nos dois
/// últimos a próxima hora tenta de novo. Bloqueante.
fn peers_snapshot(telemetry: &Telemetry, peers: &PeerClient, day: &str, epoch: u64) -> std::io::Result<bool> {
    if !telemetry.enabled() { return Ok(false); }
    let enabled: HashSet<String> = peers.enabled_ids().into_iter().collect();
    let mut kinds: BTreeMap<PeerKind, u64> = BTreeMap::new();
    for id in peers.ids() {
        // Só entrada de máquina (endereço e token), como a varredura do `PeerBook`.
        let Some(base_url) = peers.base_url(&id) else { continue };
        let kind = if enabled.contains(&id) { classify(&base_url) } else { Some(PeerKind::Disabled) };
        if let Some(kind) = kind { *kinds.entry(kind).or_default() += 1; }
    }
    let counters = kinds.into_iter().map(|(kind, n)| Counter::new(Event::Peers, Dims::new().with("kind", kind), None, n)).collect();
    telemetry.record_day(day, epoch, PEERS_MARK, counters)
}

/// O retrato do dia: máquinas e, se o do servidor ainda não saiu hoje, as preferências dele. `discover` roda as
/// CLIs, por isso só depois de o dia das máquinas gravar e só enquanto o dia não tem a marca das preferências,
/// gravada junto com elas. `Ok(Ok(true))` = dia feito.
/// Descoberta que passa de `DISCOVER_TIMEOUT` sai sem provedor, com o código no diário.
async fn daily_snapshot<Fut: std::future::Future<Output = Option<String>>>(
    telemetry: Arc<Telemetry>,
    peers: Arc<PeerClient>,
    diag: crate::diag::DiagClient,
    day: String,
    discover: impl FnOnce() -> Fut,
    read: impl FnOnce(Option<String>) -> Vec<(&'static str, String)> + Send + 'static,
) -> Result<std::io::Result<bool>, tokio::task::JoinError> {
    let epoch = telemetry.consent_epoch();
    let (marks, mark_day) = (telemetry.clone(), day.clone());
    let step = tokio::task::spawn_blocking(move || -> std::io::Result<(bool, bool)> {
        let done = peers_snapshot(&marks, &peers, &mark_day, epoch)?;
        Ok((done, done && !marks.recorded(&mark_day, SERVER_SETTINGS_MARK)))
    }).await?;
    let (done, due) = match step { Ok(step) => step, Err(e) => return Ok(Err(e)) };
    if due {
        let provider = tokio::time::timeout(DISCOVER_TIMEOUT, discover()).await.unwrap_or_else(|_| {
            diag.report("telemetry.settings", "", "telemetry_settings_provider_timeout", "telemetria: descoberta do provedor padrão passou do prazo");
            None
        });
        let recorded = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            if telemetry.today() != day || !telemetry.enabled() || telemetry.consent_epoch() != epoch { return Ok(()); }
            let values = read(provider);
            let (counters, dropped) = settings_counters(ClientKind::Server, values.iter().map(|(name, value)| (*name, Some(value.as_str()))));
            if telemetry.record_day(&day, epoch, SERVER_SETTINGS_MARK, counters)? { report_dropped(&diag, dropped); }
            Ok(())
        }).await?;
        if let Err(e) = recorded { return Ok(Err(e)); }
    }
    Ok(Ok(done))
}

/// O provedor que a abertura escolhe quando o pedido não diz; `None` (e o código no diário) quando a descoberta falha.
async fn default_provider(st: &AppState) -> Option<String> {
    match crate::engines::providers::read(&st.accounts).await {
        Ok(snapshot) => Some(crate::session_open::resolve::choose_provider(&snapshot.selection).name().to_owned()),
        Err(_) => {
            st.diag.report("telemetry.settings", "", "telemetry_settings_provider_failed", "telemetria: provedor padrão não descoberto");
            None
        }
    }
}

/// O ditado e o Jev como os leitores deles resolvem: o `runtime-config.json`, senão o ambiente (`.env`,
/// `TYPESAFE_API_KEY`, `settings.json`). `Err` quando o arquivo existe e não se lê. Bloqueante.
pub(crate) fn effective_runtime(config: &crate::config_sync::runtime::RuntimeConfig, env_jev_key: Option<String>)
    -> Result<Value, crate::config_sync::runtime_error::RuntimeError> {
    let get = |key: &str| config.get_checked(key);
    let jev = crate::voice::settings::read_gate(&config.env.home, &config.env.claude_base, env_jev_key).jev.is_some();
    Ok(json!({
        "dictation_organization_mode": get("dictation_organization_mode")?,
        "transcription_providers": get("transcription_providers")?,
        "groq_api_key": get("groq_api_key")?,
        "jev": jev,
    }))
}

/// O que o servidor guarda das escolhas do dono, já nos nomes de `events::SETTINGS`. Arquivo que não se lê deixa a
/// preferência dele de fora, com o código no diário (`runtime` `None`: o ditado e o Jev); sem provedor, saem de fora
/// ele e o esforço. Bloqueante.
pub(crate) fn server_settings(home: &Path, runtime: Option<&Value>, provider: Option<&str>, diag: &crate::diag::DiagClient) -> Vec<(&'static str, String)> {
    let on = |on: bool| if on { "on" } else { "off" }.to_owned();
    let unreadable = |code: &str| diag.report("telemetry.settings", "", code, "telemetria: preferência do servidor ilegível, fora do retrato");
    let mut values = Vec::new();
    match crate::session_defaults::options(home) {
        Ok(options) => {
            let options = options.unwrap_or_default();
            values.push(("conversation_beta", on(options.conversation_beta)));
            values.push(("code_mode", on(options.code_mode)));
        }
        Err(_) => unreadable("telemetry_settings_options_unreadable"),
    }
    if let Some(provider) = provider {
        values.push(("default_provider", provider.to_owned()));
        match crate::session_defaults::saved(home, provider, "", "") {
            Ok(saved) => {
                let effort = saved.map(|d| d.effort).filter(|e| !e.is_empty());
                values.push(("default_effort", effort.unwrap_or_else(|| "unset".to_owned())));
            }
            Err(_) => unreadable("telemetry_settings_defaults_unreadable"),
        }
    }
    match crate::diag_report::auto::consent(home) {
        Ok(consent) => values.push(("auto_report", consent.map_or("unanswered".to_owned(), on))),
        Err(_) => unreadable("telemetry_settings_auto_report_unreadable"),
    }
    match runtime {
        Some(runtime) => {
            let text = |key: &str| runtime[key].as_str().map(str::trim).unwrap_or_default().to_owned();
            let organization = text("dictation_organization_mode");
            values.push(("dictation_org", if organization.is_empty() { "none".to_owned() } else { organization }));
            // O primeiro serviço da lista é o que transcreve; sem lista, a chave Groq antiga.
            let provider = runtime["transcription_providers"][0]["kind"].as_str().map(str::to_owned)
                .or_else(|| (!text("groq_api_key").is_empty()).then(|| "groq".to_owned()));
            values.push(("dictation_provider", provider.unwrap_or_else(|| "none".to_owned())));
            values.push(("jev", if runtime["jev"].as_bool() == Some(true) { "configured" } else { "no" }.to_owned()));
        }
        None => unreadable("telemetry_settings_runtime_unreadable"),
    }
    match crate::connect::state::read(&home.join(".hangar").join("connect")) {
        Ok(linked) => values.push(("connect", on(linked.is_some()))),
        Err(_) => unreadable("telemetry_settings_connect_unreadable"),
    }
    let computer = hangar_computer_control_setup::Paths::new(home.to_path_buf(), Vec::new());
    match hangar_computer_control_setup::read_entry(computer.main_file(), false) {
        Ok(main) => values.push(("computer_use", on(main.entry.is_some()))),
        Err(_) => unreadable("telemetry_settings_computer_use_unreadable"),
    }
    match hangar_whatsapp::install::current_checked(&hangar_whatsapp::install::Paths::new(home)) {
        Ok(installed) => values.push(("whatsapp", if installed.is_some() { "installed" } else { "no" }.to_owned())),
        Err(_) => unreadable("telemetry_settings_whatsapp_unreadable"),
    }
    values
}

/// Projeta os pares conhecidos e os nomes seguros das preferências descartadas.
fn settings_counters<'a>(client: ClientKind, values: impl IntoIterator<Item = (&'a str, Option<&'a str>)>) -> (Vec<Counter>, Vec<&'static str>) {
    let (mut counters, mut dropped) = (Vec::new(), Vec::new());
    for (name, value) in values {
        let Some((setting, value)) = value.and_then(|value| events::setting(name, value)) else { dropped.push(known_setting(name)); continue };
        let dims = Dims::new().with("client", client).with("setting", setting).with("value", value);
        counters.push(Counter::new(Event::Settings, dims, None, 1));
    }
    (counters, dropped)
}

fn known_setting(name: &str) -> &'static str {
    events::SETTINGS.iter().find(|(known, _)| *known == name).map_or("other", |(known, _)| *known)
}

/// Uma linha por retrato, com até 8 nomes distintos da lista fechada.
fn report_dropped(diag: &crate::diag::DiagClient, mut dropped: Vec<&'static str>) {
    if dropped.is_empty() { return; }
    let count = dropped.len();
    dropped.sort_unstable();
    dropped.dedup();
    dropped.truncate(8);
    diag.report_with("telemetry.settings", "", "telemetry_settings_dropped", "telemetria: preferência do retrato fora da lista ou sem texto",
        json!({"settings": dropped.join(","), "count": count}));
}

/// O retrato do app tem rota própria: não depende de confirmação do diário e não pode cair nele num rollback.
pub async fn snapshot(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    if let Err(response) = admit(&st, peer, &req).await { return response; }
    let Some(telemetry) = st.telemetry.clone() else { return unavailable() };
    let item: Value = match to_bytes(req.into_body(), 16 * 1024).await.ok().and_then(|raw| serde_json::from_slice(&raw).ok()) {
        Some(item) => item,
        None => return detail(StatusCode::UNPROCESSABLE_ENTITY, "erro_corpo_invalido", "retrato inválido", json!({})),
    };
    let diag = st.diag.clone();
    match tokio::task::spawn_blocking(move || settings_snapshot(&telemetry, &diag, &item)).await {
        Ok(Ok(Some(period))) => private_view(json!({"snapshot_period": period})),
        Ok(Ok(None)) => detail(StatusCode::CONFLICT, "telemetry_snapshot_not_current", "retrato fora do período de coleta ou do dia UTC", json!({})),
        Ok(Err(e)) => { report(&st.diag, Failure::Disk(e)); unavailable() }
        Err(_) => { task_failed(&st.diag); unavailable() }
    }
}

fn settings_snapshot(telemetry: &Telemetry, diag: &crate::diag::DiagClient, item: &Value) -> std::io::Result<Option<String>> {
    let Some(period) = item["snapshot_period"].as_str() else { return Ok(None) };
    let Some(captured) = item["ts"].as_str().and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok()) else { return Ok(None) };
    let Some(values) = item["values"].as_object() else { return Ok(None) };
    let client = screen_client(item);
    let device = item["device"].as_str()
        .filter(|id| (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
    let key = format!("settings:{}:{}", client.as_str(), device.unwrap_or_default());
    let (counters, dropped) = settings_counters(client, values.iter().map(|(name, value)| (name.as_str(), value.as_str())));
    if !telemetry.record_snapshot(period, captured.to_utc(), &key, counters)? { return Ok(None); }
    if device.is_none() {
        diag.report("telemetry.settings", "", "telemetry_settings_device_invalid", "telemetria: retrato sem id de aparelho válido, contado como um só");
    }
    report_dropped(diag, dropped);
    Ok(Some(period.to_owned()))
}

/// O tipo de ligação de um endereço; o endereço em si nunca sai daqui. `None` quando não é URL.
pub(crate) fn classify(base_url: &str) -> Option<PeerKind> {
    let url = reqwest::Url::parse(base_url).ok()?;
    let host = url.host_str()?.trim_start_matches('[').trim_end_matches(']').trim_end_matches('.').to_ascii_lowercase();
    if let Ok(ip) = host.parse::<IpAddr>() { return Some(ip_kind(ip)); }
    // ponytail: nome sem ponto (hosts, DNS da casa) conta como rede local; nome curto do MagicDNS cai aqui também.
    let local = !host.contains('.') || [".local", ".lan", ".home.arpa", ".internal"].iter().any(|suffix| host.ends_with(suffix));
    Some(if host.ends_with(".ts.net") { PeerKind::Tailscale }
        else if host.ends_with(".hangar.dev.br") { PeerKind::Connect }
        else if local { PeerKind::Lan }
        else { PeerKind::Public })
}

/// 100.64.0.0/10 e fd7a:115c:a1e0::/48 são as faixas que o Tailscale distribui.
fn ip_kind(ip: IpAddr) -> PeerKind {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            if a == 100 && (64..128).contains(&b) { PeerKind::Tailscale }
            else if v4.is_private() || v4.is_loopback() || v4.is_link_local() { PeerKind::Lan }
            else { PeerKind::Public }
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            if s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0 { PeerKind::Tailscale }
            else if v6.is_loopback() || s[0] & 0xfe00 == 0xfc00 || s[0] & 0xffc0 == 0xfe80 { PeerKind::Lan }
            else { PeerKind::Public }
        }
    }
}

/// Conta as linhas da tela que a telemetria conhece, das `accepted` primeiras que o diário gravou
/// (o resto a tela reenvia). Campo fora da lista vira `other`; texto nunca sai.
pub(crate) fn count_screen_in_period(telemetry: &Telemetry, period: &Collection, epoch: u64, now: DateTime<Utc>, body: &[u8], accepted: usize) {
    let Ok(batch) = serde_json::from_slice::<Value>(body.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(body)) else { return };
    let items = match &batch { Value::Object(fields) => fields.get("eventos"), other => Some(other) };
    let Some(Value::Array(items)) = items else { return };
    let day = now.format("%Y-%m-%d").to_string();
    for item in items.iter().take(accepted) {
        let Some(at) = item["ts"].as_str().and_then(|ts| DateTime::parse_from_rfc3339(ts).ok()).map(|at| at.to_utc()) else { continue };
        if at < period.since || at > now || at.date_naive() != now.date_naive() || telemetry.today() != day { continue; }
        if item["evento"] == "app.visible_time" {
            let started = item["started_at"].as_str().and_then(|ts| DateTime::parse_from_rfc3339(ts).ok()).map(|ts| ts.to_utc());
            if started.is_none_or(|started| started < period.since || started > at || started.date_naive() != at.date_naive()) { continue; }
        }
        screen_event(telemetry, epoch, item);
    }
}

fn screen_client(item: &Value) -> ClientKind {
    item["cliente"].as_str().and_then(ClientKind::parse).filter(|c| *c != ClientKind::Server).unwrap_or(ClientKind::Other)
}

fn screen_event(telemetry: &Telemetry, epoch: u64, item: &Value) {
    let text = |key: &str| item[key].as_str().unwrap_or_default();
    let client = screen_client(item);
    let took = item["ms"].as_u64();
    let dims = Dims::new().with("client", client);
    match text("evento") {
        "app.visible_time" => {
            let minutes = took.map_or(0, |ms| (ms.min(VISIBLE_CAP_MS) + 30_000) / 60_000);
            if minutes > 0 { telemetry.count_in(epoch, Event::AppVisibleMinutes, dims, minutes); }
        }
        "rota.escolhida" => {
            let dims = dims.with("failed", false).with("route", Code::known(text("detalhe"), ROUTES));
            match took {
                Some(ms) => telemetry.duration_in(epoch, Event::ClientRoute, dims, Duration::from_millis(ms)),
                None => telemetry.count_in(epoch, Event::ClientRoute, dims, 1),
            }
        }
        "rota.falhou" => {
            let code = Code::known(&text("codigo").to_ascii_lowercase(), ROUTE_FAILURES);
            telemetry.count_in(epoch, Event::ClientRoute, dims.with("failed", true).with("code", code), 1);
        }
        "chat.opened" => if let Some(ms) = took { telemetry.duration_in(epoch, Event::ChatOpen, dims, Duration::from_millis(ms)); },
        "app.crash" => {
            let code = Code::known(&text("codigo").to_ascii_lowercase(), FATAL_CODES);
            telemetry.count_in(epoch, Event::ClientFatal, dims.with("code", code), 1);
        }
        _ => {}
    }
}

/// O endereço do receptor nunca vai ao diário: só o status.
pub(crate) fn report(diag: &crate::diag::DiagClient, failure: Failure) {
    match failure {
        Failure::Refused(status) => {
            let message = if status == 413 { "pacote de telemetria recusado por tamanho; fica na fila" } else { "pacote de telemetria recusado" };
            diag.report("telemetry.rejected", "", &format!("http_{status}"), message);
        }
        Failure::Unreachable { status } => {
            let code = status.map_or_else(|| "telemetry_unreachable".to_owned(), |status| format!("telemetry_unreachable_http_{status}"));
            diag.report("telemetry.unreachable", "", &code, "receptor da telemetria sem resposta 2xx; o dia fica na fila");
        }
        Failure::Pruned(days) => diag.report_with("telemetry.pruned", "", "telemetry_pruned",
            "fila da telemetria acima do teto: dias mais antigos apagados sem envio", json!({"days": days})),
        Failure::Disk(e) => {
            tracing::warn!(code = "telemetry_disk_failed", kind = ?e.kind(), "telemetria: fila não lida ou não gravada");
            diag.report("telemetry.disk", "", "telemetry_disk_failed", "fila da telemetria não lida ou não gravada");
        }
    }
}

/// Tarefa bloqueante da telemetria que entrou em pânico ou foi cancelada.
pub fn task_failed(diag: &crate::diag::DiagClient) {
    tracing::warn!(code = "telemetry_task_failed", "telemetria: tarefa bloqueante interrompida");
    diag.report("telemetry.task", "", "telemetry_task_failed", "tarefa da telemetria interrompida");
}

/// Uma passada: cada máquina habilitada do `peers.json` diz o dono dela, juntas, e o mais antigo é
/// adotado (empate: menor id). Máquina fora do ar, sem a rota (versão antiga) ou que recusa não muda nada.
pub(crate) async fn converge(telemetry: &Arc<Telemetry>, peers: &Arc<PeerClient>, diag: &crate::diag::DiagClient) {
    let ids = { let peers = peers.clone(); tokio::task::spawn_blocking(move || peers.enabled_ids()).await };
    let Ok(ids) = ids else { return task_failed(diag) };
    let answers = futures_util::future::join_all(ids.iter().map(|id| peers.call(id, reqwest::Method::GET, OWNER_PATH, None))).await;
    let oldest = answers.into_iter().filter_map(|answer| {
        let answer = answer.ok()??;
        Some((answer["owner_created"].as_i64()?, answer["owner_id"].as_str()?.to_owned()))
    }).min();
    let Some((created, owner_id)) = oldest else { return };
    let telemetry = telemetry.clone();
    match tokio::task::spawn_blocking(move || telemetry.adopt_owner(&owner_id, created)).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => {
            tracing::warn!(code = "telemetry_owner_unsaved", kind = ?e.kind(), "telemetria: dono comum não gravado");
            diag.report("telemetry.owner", "", "telemetry_owner_unsaved", "dono comum da telemetria não gravado");
        }
        Err(_) => task_failed(diag),
    }
}

/// Pedido de convidado por qualquer porta: convite, par, login ou a 8766.
pub(crate) fn guest(req: &Request) -> bool {
    crate::routes::on_guest_port(req) || req.headers().contains_key("x-hangar-guest")
        || req.extensions().get::<crate::login_guests::gate::Login>().is_some()
        || matches!(crate::share::caller::caller(req), Some(crate::share::caller::Caller::Guest(_)))
}

fn refused() -> Response { detail(StatusCode::FORBIDDEN, OWNER_ONLY.0, OWNER_ONLY.1, json!({})) }

fn unavailable() -> Response {
    detail(StatusCode::SERVICE_UNAVAILABLE, "telemetry_unavailable", "não foi possível ler ou guardar a telemetria", json!({}))
}

/// Só o dono. Convidado (convite, par, login, porta 8766) leva 403 sem contar falha de senha; token
/// errado, a recusa contada. O pedido é lido antes da espera: `&Request` atravessando um `await` tira o `Send`.
fn admit(st: &AppState, peer: SocketAddr, req: &Request) -> impl Future<Output = Result<(), Response>> + Send + use<> {
    let guest = guest(req);
    let owner = st.edge().owner_only(peer, req, OWNER_ONLY);
    async move {
        if guest { return Err(refused()); }
        owner.await
    }
}

fn private_view(value: Value) -> Response {
    let mut response = json_response(StatusCode::OK, value);
    response.headers_mut().insert(axum::http::header::CACHE_CONTROL, axum::http::HeaderValue::from_static("no-store"));
    response
}

fn view(telemetry: &Telemetry) -> Value {
    let period = telemetry.snapshot_period();
    let mut view = json!({"consent": telemetry.consent(), "consent_scope": CONSENT_SCOPE,
        "disabled_by": telemetry.disabled_by().map(Disabled::as_str), "enabled": period.is_some()});
    if let Some(period) = period { view["snapshot_period"] = json!(period); }
    view
}

/// `GET` anuncia o escopo e, ligada, o `snapshot_period` que os apps devolvem com o retrato. `PUT` só aceita o
/// "sim" acompanhado desse escopo; `false` revoga em qualquer versão.
pub async fn setting(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    if let Err(response) = admit(&st, peer, &req).await { return response; }
    let Some(telemetry) = st.telemetry.clone() else { return unavailable() };
    let choice = if req.method() == Method::PUT {
        let parsed = to_bytes(req.into_body(), 1024).await.ok().and_then(|raw| serde_json::from_slice::<Choice>(&raw).ok());
        let Some(choice) = parsed else {
            return detail(StatusCode::UNPROCESSABLE_ENTITY, "erro_corpo_invalido", "esperado {\"consent\": true ou false}", json!({}));
        };
        if choice.consent && choice.consent_scope != Some(CONSENT_SCOPE) {
            return detail(StatusCode::UNPROCESSABLE_ENTITY, "telemetry_consent_scope_required", "confirme o aviso de telemetria deste servidor", json!({"consent_scope": CONSENT_SCOPE}));
        }
        Some(choice.consent)
    } else {
        None
    };
    let (diag, worker) = (st.diag.clone(), telemetry.clone());
    let done = tokio::task::spawn_blocking(move || {
        // Gravada, a escolha vale; o que não se aplicou agora vai ao diário e a passada seguinte refaz.
        let (applied, refusal) = match choice {
            Some(true) => (worker.set_consent(true)?, None),
            Some(false) => worker.refuse()?,
            None => (None, None),
        };
        if let Some(e) = applied { report(&diag, Failure::Disk(e)); }
        Ok::<_, std::io::Error>((view(&worker), refusal))
    }).await;
    match done {
        Ok(Ok((body, refusal))) => {
            if let Some(refusal) = refusal { send_refusal(&telemetry, refusal, st.diag.clone()); }
            private_view(body)
        }
        Ok(Err(e)) => {
            tracing::warn!(code = "telemetry_unavailable", kind = ?e.kind(), "telemetria: escolha não guardada");
            unavailable()
        }
        Err(_) => {
            task_failed(&st.diag);
            unavailable()
        }
    }
}

/// O aviso único de "Não enviar", em segundo plano; o que não chegou vai ao diário e não se tenta de novo.
fn send_refusal(telemetry: &Telemetry, refusal: Refusal, diag: crate::diag::DiagClient) {
    let Some(post) = telemetry.refusal_post() else {
        diag.report("telemetry.refusal", "", "telemetry_refusal_no_client", "aviso de recusa da telemetria sem cliente HTTP: não sai");
        return;
    };
    tokio::spawn(async move {
        if let Err(status) = hangar_telemetry::send_refusal(refusal, post).await {
            let code = status.map_or_else(|| "telemetry_refusal_unreachable".to_owned(), |status| format!("telemetry_refusal_http_{status}"));
            diag.report("telemetry.refusal", "", &code, "aviso de recusa da telemetria não chegou ao receptor; não volta a ser mandado");
        }
    });
}

/// `GET /api/telemetry/owner`: o dono desta máquina, para as outras do mesmo dono convergirem.
pub async fn owner(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    if let Err(response) = admit(&st, peer, &req).await { return response; }
    let Some(telemetry) = st.telemetry.clone() else { return unavailable() };
    match telemetry.owner() {
        Ok((owner_id, owner_created)) => json_response(StatusCode::OK, json!({"owner_id": owner_id, "owner_created": owner_created})),
        Err(_) => unavailable(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::groups::peers::PeerBook;
    use crate::telemetry_hooks::{test_state, test_telemetry};
    use axum::body::Body;

    const TOKEN: &str = "owner";

    async fn put(st: &Arc<AppState>, body: &str, guest: bool) -> (StatusCode, Value) {
        request(st, Method::PUT, body, guest).await
    }

    async fn request(st: &Arc<AppState>, method: Method, body: &str, guest: bool) -> (StatusCode, Value) {
        let mut req = axum::http::Request::builder().method(method.clone()).uri("/api/telemetry")
            .header("authorization", format!("Bearer {TOKEN}")).header("content-type", "application/json");
        if guest { req = req.header("x-hangar-guest", "1"); }
        let (state, peer, req) = (State(st.clone()), ConnectInfo("127.0.0.1:5000".parse().unwrap()), req.body(Body::from(body.to_owned())).unwrap());
        let response = if method == Method::POST { snapshot(state, peer, req).await } else { setting(state, peer, req).await };
        let status = response.status();
        let raw = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn guest_put_is_refused_and_changes_nothing() {
        let home = tempfile::tempdir().unwrap();
        let st = test_state(home.path(), false);
        let (status, body) = put(&st, r#"{"consent": true, "consent_scope": 3}"#, true).await;
        assert_eq!((status, &body["detail"]["code"]), (StatusCode::FORBIDDEN, &json!("telemetry_owner_only")));
        assert_eq!(st.telemetry.as_ref().unwrap().consent(), None);
    }

    /// O aviso de recusa que sairia, num canal em vez da rede.
    fn refusal_receiver(telemetry: &Telemetry) -> tokio::sync::mpsc::UnboundedReceiver<Value> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        telemetry.set_refusal_post(Arc::new(move |body: Vec<u8>| -> hangar_telemetry::PostFuture {
            let _ = tx.send(serde_json::from_slice(&body).unwrap());
            Box::pin(async { Some(200) })
        }));
        rx
    }

    #[tokio::test]
    async fn telemetry_no_from_the_owner_sends_one_anonymous_refusal_and_a_guest_none() {
        let home = tempfile::tempdir().unwrap();
        let st = test_state(home.path(), true);
        let mut sent = refusal_receiver(st.telemetry.as_ref().unwrap());
        assert_eq!(put(&st, r#"{"consent": false}"#, true).await.0, StatusCode::FORBIDDEN);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(sent.try_recv().is_err(), "convidado não manda aviso");
        assert_eq!(put(&st, r#"{"consent": false}"#, false).await.0, StatusCode::OK);
        let ping = tokio::time::timeout(Duration::from_secs(5), sent.recv()).await.unwrap().unwrap();
        assert_eq!(ping, json!({"kind": "revoked", "version": "0.42.0", "os": std::env::consts::OS, "channel": "latest"}));
        put(&st, r#"{"consent": true, "consent_scope": 3}"#, false).await;
        put(&st, r#"{"consent": false}"#, false).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(sent.try_recv().is_err(), "uma vez por máquina");
    }

    #[tokio::test]
    async fn telemetry_consent_get_announces_scope_and_legacy_false_revokes() {
        let home = tempfile::tempdir().unwrap();
        let st = test_state(home.path(), false);
        assert_eq!(request(&st, Method::GET, "", false).await,
            (StatusCode::OK, json!({"consent": null, "consent_scope": 3, "disabled_by": null, "enabled": false})));
        for body in [r#"{"consent": "sim"}"#, r#"{"consent": null}"#, r#"{"consent": true, "x": 1}"#, "{}", "nada"] {
            let (status, reply) = put(&st, body, false).await;
            assert_eq!((status, &reply["detail"]["code"]), (StatusCode::UNPROCESSABLE_ENTITY, &json!("erro_corpo_invalido")), "{body}");
        }
        assert_eq!(st.telemetry.as_ref().unwrap().consent(), None);
        for body in [r#"{"consent": true}"#, r#"{"consent": true, "consent_scope": 2}"#, r#"{"consent": true, "consent_scope": 4}"#] {
            let (status, reply) = put(&st, body, false).await;
            assert_eq!((status, &reply["detail"]["code"]), (StatusCode::UNPROCESSABLE_ENTITY, &json!("telemetry_consent_scope_required")), "{body}");
        }
        let (status, active) = put(&st, r#"{"consent": true, "consent_scope": 3}"#, false).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!((active["consent"].as_bool(), active["consent_scope"].as_u64(), active["enabled"].as_bool()), (Some(true), Some(3), Some(true)));
        assert!(active["snapshot_period"].as_str().is_some());
        assert_eq!(put(&st, r#"{"consent": false}"#, false).await,
            (StatusCode::OK, json!({"consent": false, "consent_scope": 3, "disabled_by": null, "enabled": false})));
        assert!(!st.telemetry.as_ref().unwrap().enabled());
    }

    #[tokio::test]
    async fn snapshot_post_confirms_only_after_saving_and_deduplicates_after_restart() {
        let home = tempfile::tempdir().unwrap();
        let ts = "2026-11-27T12:00:00Z";
        let clock = chrono::DateTime::parse_from_rfc3339(ts).unwrap().to_utc();
        let telemetry = Arc::new(Telemetry::for_test(home.path(), Arc::new(move || clock)));
        let mut st = test_state(home.path(), false);
        Arc::get_mut(&mut st).unwrap().telemetry = Some(telemetry.clone());
        let yes = r#"{"consent":true,"consent_scope":3}"#;
        let (_, first) = put(&st, yes, false).await;
        let (_, unchanged) = put(&st, yes, false).await;
        assert_eq!(first["snapshot_period"], unchanged["snapshot_period"]);
        let (_, off) = put(&st, r#"{"consent":false}"#, false).await;
        assert!(off.get("snapshot_period").is_none());
        let (_, new) = put(&st, yes, false).await;
        assert_ne!(first["snapshot_period"], new["snapshot_period"]);
        let body = |period: &Value, ts: &str| json!({"snapshot_period": period, "ts": ts, "cliente": "ios",
            "device": "fictional-device", "values": {"theme": "dark", "palette": "classic"}}).to_string();
        let current = body(&new["snapshot_period"], ts);
        assert_eq!(request(&st, Method::POST, &current, true).await.0, StatusCode::FORBIDDEN);
        for bad in [body(&first["snapshot_period"], ts), body(&Value::Null, ts), body(&new["snapshot_period"], "2026-11-27T12:01:01Z")] {
            assert_eq!(request(&st, Method::POST, &bad, false).await.0, StatusCode::CONFLICT);
        }
        let day = home.path().join(".hangar/telemetry-v3/queue/2026-11-27.json");
        assert!(!day.exists());
        std::fs::create_dir_all(&day).unwrap();
        assert_eq!(request(&st, Method::POST, &current, false).await.0, StatusCode::SERVICE_UNAVAILABLE);
        assert!(day.is_dir(), "sem arquivo confirmado, o cliente ainda pode repetir");
        std::fs::remove_dir(&day).unwrap();
        assert_eq!(request(&st, Method::POST, &current, false).await, (StatusCode::OK, json!({"snapshot_period": new["snapshot_period"]})));
        assert!(day.is_file(), "a confirmação já implica gravação");
        drop(telemetry);

        let restarted = Arc::new(Telemetry::for_test(home.path(), Arc::new(move || clock)));
        Arc::get_mut(&mut st).unwrap().telemetry = Some(restarted.clone());
        let (_, after_restart) = request(&st, Method::GET, "", false).await;
        assert_eq!(request(&st, Method::POST, &current, false).await.0, StatusCode::CONFLICT);
        let retry = body(&after_restart["snapshot_period"], ts);
        assert_eq!(request(&st, Method::POST, &retry, false).await,
            (StatusCode::OK, json!({"snapshot_period": after_restart["snapshot_period"]})));
        assert_eq!(restarted.counters().iter().map(|c| (c.dims["setting"].as_str(), c.count)).collect::<Vec<_>>(), [("palette", 1), ("theme", 1)]);
        assert!(!home.path().join(".hangar/telemetry-v3/once.json").exists());
    }

    /// Uma máquina do dono que responde `status` com `body` em `/api/telemetry/owner`.
    async fn peer(dir: &Path, status: StatusCode, body: Value) -> (Arc<PeerClient>, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let answer = (status, body.to_string());
        let router = axum::Router::new().route(OWNER_PATH, axum::routing::get(move || { let answer = answer.clone(); async move { answer } }));
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap(); });
        let file = dir.join("peers.json");
        std::fs::write(&file, json!({"lab": {"base_url": format!("http://{address}"), "token": TOKEN}}).to_string()).unwrap();
        (Arc::new(PeerClient::new(PeerBook::new(Some(file)))), task)
    }

    #[tokio::test]
    async fn older_owner_of_a_peer_is_adopted() {
        let home = tempfile::tempdir().unwrap();
        let telemetry = test_telemetry(home.path(), false);
        let older = "0".repeat(32);
        let (peers, task) = peer(home.path(), StatusCode::OK, json!({"owner_id": older, "owner_created": 1})).await;
        converge(&telemetry, &peers, &crate::diag::DiagClient::new()).await;
        task.abort();
        assert_eq!(telemetry.owner().unwrap(), (older, 1));
    }

    #[tokio::test]
    async fn peer_without_the_route_changes_nothing() {
        let home = tempfile::tempdir().unwrap();
        let telemetry = test_telemetry(home.path(), false);
        let before = telemetry.owner().unwrap();
        let (peers, task) = peer(home.path(), StatusCode::NOT_FOUND, json!({"detail": "Not Found"})).await;
        converge(&telemetry, &peers, &crate::diag::DiagClient::new()).await;
        task.abort();
        assert_eq!(telemetry.owner().unwrap(), before);
    }

    fn has(telemetry: &Telemetry, event: Event, dims: Dims, bucket: Option<hangar_telemetry::buckets::Bucket>, count: u64) -> bool {
        telemetry.counters().contains(&hangar_telemetry::wire::Counter::new(event, dims, bucket, count))
    }

    fn count_screen(telemetry: &Telemetry, body: &[u8], accepted: usize, now: DateTime<Utc>) {
        let period = telemetry.collection().unwrap();
        let mut body: Value = serde_json::from_slice(body).unwrap();
        for item in body["eventos"].as_array_mut().unwrap() {
            item["ts"] = json!(now);
            if item["evento"] == "app.visible_time" { item["started_at"] = json!(period.since); }
        }
        count_screen_in_period(telemetry, &period, telemetry.consent_epoch(), now, body.to_string().as_bytes(), accepted);
    }

    #[test]
    fn screen_events_count_only_known_values() {
        use hangar_telemetry::buckets::Bucket;
        let home = tempfile::tempdir().unwrap();
        let now: DateTime<Utc> = "2026-11-27T20:00:00Z".parse().unwrap();
        let telemetry = Telemetry::for_test(home.path(), Arc::new(move || now));
        telemetry.set_consent(true).unwrap();
        let body = json!({"eventos": [
            {"evento": "app.visible_time", "ms": 900_000, "cliente": "ios"},
            {"evento": "rota.escolhida", "detalhe": "tailscale", "ms": 250, "cliente": "android"},
            {"evento": "rota.falhou", "codigo": "TimeoutError", "cliente": "android"},
            {"evento": "rota.falhou", "codigo": "https://casa.exemplo", "cliente": "android"},
            {"evento": "chat.opened", "ms": 1_200, "cliente": "native"},
            {"evento": "app.crash", "codigo": "TypeError", "cliente": "celular-do-jeff"},
            {"evento": "sse.conectou", "ms": 10, "cliente": "ios"},
            // Fora das linhas que o diário gravou: a tela reenvia, e aqui não conta.
            {"evento": "app.visible_time", "ms": 900_000, "cliente": "ios"},
        ]}).to_string();
        count_screen(&telemetry, body.as_bytes(), 7, now);
        let android = || Dims::new().with("client", ClientKind::Android);
        assert!(has(&telemetry, Event::AppVisibleMinutes, Dims::new().with("client", ClientKind::Ios), None, 5), "teto de 5 min");
        assert!(has(&telemetry, Event::ClientRoute, android().with("failed", false).with("route", Code::known("tailscale", ROUTES)), Some(Bucket::Lt300ms), 1));
        assert!(has(&telemetry, Event::ClientRoute, android().with("failed", true).with("code", Code::known("timeouterror", ROUTE_FAILURES)), None, 1));
        assert!(has(&telemetry, Event::ClientRoute, android().with("failed", true).with("code", Code::OTHER), None, 1));
        assert!(has(&telemetry, Event::ChatOpen, Dims::new().with("client", ClientKind::Native), Some(Bucket::Lt3s), 1));
        assert!(has(&telemetry, Event::ClientFatal, Dims::new().with("client", ClientKind::Other).with("code", Code::known("typeerror", FATAL_CODES)), None, 1));
        assert_eq!(telemetry.counters().len(), 6);
        let wire = serde_json::to_string(&telemetry.counters()).unwrap();
        assert!(!wire.contains("exemplo") && !wire.contains("celular"), "{wire}");
    }

    #[test]
    fn settings_snapshot_counts_once_a_day_per_client_and_device() {
        use std::sync::atomic::{AtomicI64, Ordering};
        let home = tempfile::tempdir().unwrap();
        let first_day = "2026-11-27T12:00:00Z";
        let now = Arc::new(AtomicI64::new(chrono::DateTime::parse_from_rfc3339(first_day).unwrap().timestamp()));
        let clock = now.clone();
        let telemetry = Telemetry::for_test(home.path(), Arc::new(move || chrono::DateTime::from_timestamp(clock.load(Ordering::SeqCst), 0).unwrap()));
        telemetry.set_consent(true).unwrap();
        let period = telemetry.snapshot_period().unwrap();
        let snapshot = |client: &str, device: &str, ts: &str| json!({"snapshot_period": period, "cliente": client, "device": device,
            "ts": ts, "values": {"theme": "dark", "font": "Fira Code", "modelo": "opus", "tool_look": 3}});
        let diag = crate::diag::DiagClient::new();
        let send = |item: Value| settings_snapshot(&telemetry, &diag, &item).unwrap();
        for (client, device) in [("native", "a-1"), ("native", "a-1"), ("native", "b-2"), ("ios", "a-1"), ("server", "c-3")] {
            assert_eq!(send(snapshot(client, device, first_day)).as_deref(), Some(period.as_str()));
        }
        let count = |client: &str, setting: &str, value: &str| telemetry.counters().into_iter()
            .find(|c| c.event == "settings" && c.dims.get("client").map(String::as_str) == Some(client)
                && c.dims["setting"] == setting && c.dims["value"] == value)
            .map_or(0, |c| c.count);
        assert_eq!(count("native", "theme", "dark"), 2, "dois aparelhos, cada um uma vez");
        assert_eq!(count("ios", "theme", "dark"), 1);
        assert_eq!(count("other", "theme", "dark"), 1, "app não fala como o servidor");
        assert_eq!(count("native", "font", "other"), 2, "nome de fonte sai other");
        assert_eq!(telemetry.counters().len(), 6, "preferência desconhecida e valor que não é texto ficam fora");
        telemetry.save().unwrap();
        let day_file = home.path().join(".hangar/telemetry-v3/queue/2026-11-27.json");
        // O fechamento do dia muda o lote (selo, detalhe); os contadores do retrato não mudam.
        let counters = || serde_json::from_slice::<Value>(&std::fs::read(&day_file).unwrap()).unwrap()["counters"].clone();
        let saved = counters();
        now.fetch_add(24 * 60 * 60, Ordering::SeqCst);
        let mut late = snapshot("native", "a-1", "2026-11-28T01:59:59+02:00");
        late["values"]["theme"] = json!("light");
        let mut missing = late.clone();
        missing.as_object_mut().unwrap().remove("ts");
        let mut invalid = late.clone();
        invalid["ts"] = json!("2026-11-28");
        for bad in [late, missing, invalid] { assert!(send(bad).is_none()); }
        assert!(send(snapshot("native", "a-1", "2026-11-28T00:00:00Z")).is_some());
        assert_eq!(count("native", "theme", "dark"), 1, "só a captura do dia UTC pode consumir a marca");
        assert_eq!(count("native", "theme", "light"), 0);
        let wire = serde_json::to_string(&telemetry.counters()).unwrap();
        assert!(!wire.contains("Fira") && !wire.contains("a-1"), "{wire}");
        assert_eq!(telemetry.counters().len(), 2, "o dia novo é outro pacote, e o aparelho conta de novo");
        telemetry.save().unwrap();
        assert_eq!(counters(), saved, "a captura atrasada não reescreve o dia fechado");
    }

    #[test]
    fn app_settings_snapshot_counts_under_scope_3_and_not_before_acceptance() {
        let home = tempfile::tempdir().unwrap();
        let now: DateTime<Utc> = "2026-11-27T12:00:00Z".parse().unwrap();
        let telemetry = Telemetry::for_test(home.path(), Arc::new(move || now));
        let diag = crate::diag::DiagClient::new();
        let item = |period: Value| json!({"snapshot_period": period, "cliente": "native", "device": "a-1", "ts": now,
            "values": {"theme": "dark"}});
        assert!(telemetry.snapshot_period().is_none(), "sem aceite não há período para o app devolver");
        assert!(settings_snapshot(&telemetry, &diag, &item(json!("0".repeat(32)))).unwrap().is_none());
        assert!(telemetry.counters().is_empty());

        telemetry.set_consent(true).unwrap();
        let period = telemetry.snapshot_period().unwrap();
        assert_eq!(settings_snapshot(&telemetry, &diag, &item(json!(period))).unwrap().as_deref(), Some(period.as_str()));
        let counted = || telemetry.counters().into_iter()
            .map(|c| (c.event, c.dims["client"].clone(), c.dims["setting"].clone(), c.dims["value"].clone(), c.count)).collect::<Vec<_>>();
        assert_eq!(counted(), [("settings".into(), "native".into(), "theme".into(), "dark".into(), 1)]);
        // O formato antigo, dentro do lote do diário, continua fora da contagem.
        let legacy = json!({"eventos": [{"evento": "settings.snapshot", "cliente": "native", "aparelho": "b-2",
            "valores": {"theme": "light"}}]}).to_string();
        count_screen(&telemetry, legacy.as_bytes(), 1, now);
        assert_eq!(counted().len(), 1);
    }

    #[test]
    fn server_settings_read_the_choices_without_free_text() {
        let home = tempfile::tempdir().unwrap();
        let hangar = home.path().join(".hangar");
        std::fs::create_dir_all(&hangar).unwrap();
        std::fs::write(hangar.join("session-options.json"), r#"{"conversation_beta": true, "code_mode": false}"#).unwrap();
        std::fs::write(hangar.join("auto-report.json"), r#"{"auto_report": false}"#).unwrap();
        let key = crate::session_defaults::scope_key("codex", "", "").unwrap();
        std::fs::write(hangar.join("session-defaults.json"), json!({key: {"model": "gpt-6.1-sol", "effort": "xhigh", "permission": ""}}).to_string()).unwrap();
        let runtime = json!({"dictation_organization_mode": "harness", "groq_api_key": "gsk-segredo", "jev": true});
        let diag = crate::diag::DiagClient::new();
        let values: BTreeMap<&str, String> = server_settings(home.path(), Some(&runtime), Some("codex"), &diag).into_iter().collect();
        let expected: BTreeMap<&str, String> = [("conversation_beta", "on"), ("code_mode", "off"), ("default_provider", "codex"),
            ("default_effort", "xhigh"), ("auto_report", "off"), ("dictation_org", "harness"), ("dictation_provider", "groq"),
            ("connect", "off"), ("computer_use", "off"), ("whatsapp", "no"), ("jev", "configured")]
            .into_iter().map(|(k, v)| (k, v.to_owned())).collect();
        assert_eq!(values, expected);
        let bare: BTreeMap<&str, String> = server_settings(home.path(), Some(&json!({})), None, &diag).into_iter().collect();
        assert_eq!((bare.get("default_provider"), bare["dictation_org"].as_str(), bare["dictation_provider"].as_str(), bare["jev"].as_str()),
            (None, "none", "none", "no"));

        let telemetry = test_telemetry(home.path(), true);
        let values = server_settings(home.path(), Some(&runtime), Some("codex"), &diag);
        let (counters, _) = settings_counters(ClientKind::Server, values.iter().map(|(name, value)| (*name, Some(value.as_str()))));
        assert!(telemetry.record_day(&telemetry.today(), telemetry.consent_epoch(), SERVER_SETTINGS_MARK, counters).unwrap());
        assert_eq!(telemetry.counters().len(), values.len(), "nenhum valor sai other");
        assert!(telemetry.counters().iter().any(|c| c.dims["setting"] == "dictation_provider" && c.dims["value"] == "groq"));
        assert!(telemetry.counters().iter().all(|c| c.dims.get("client").map(String::as_str) == Some("server")));
        assert!(!serde_json::to_string(&telemetry.counters()).unwrap().contains("segredo"));
    }

    /// Os códigos que `diag` gravou em `dir`.
    async fn diary_codes(diag: &crate::diag::DiagClient, dir: &Path) -> Vec<Value> {
        diag.flush().await;
        crate::diag::test_support::rows(dir).into_iter().filter(|row| row["codigo"].as_str().is_some_and(|c| c.starts_with("telemetry_"))).collect()
    }

    #[tokio::test]
    async fn settings_snapshot_reports_what_it_drops_and_a_missing_device() {
        let (home, diary) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ts = "2026-11-27T12:00:00Z";
        let now = chrono::DateTime::parse_from_rfc3339(ts).unwrap().to_utc();
        let telemetry = Telemetry::for_test(home.path(), Arc::new(move || now));
        telemetry.set_consent(true).unwrap();
        let diag = crate::diag::DiagClient::new().with_diary(Some(diary.path().to_owned()));
        let period = telemetry.snapshot_period().unwrap();
        let body = json!({"snapshot_period": period, "cliente": "native", "ts": ts,
            "values": {"theme": "dark", "modelo": "opus", "tool_look": 3}});
        assert!(settings_snapshot(&telemetry, &diag, &body).unwrap().is_some());
        let rows = diary_codes(&diag, diary.path()).await;
        let row = |code: &str| rows.iter().find(|row| row["codigo"] == code).cloned();
        assert!(row("telemetry_settings_device_invalid").is_some(), "{rows:?}");
        let dropped = row("telemetry_settings_dropped").expect("o que saiu do retrato vai ao diário");
        assert_eq!((dropped["settings"].as_str(), dropped["count"].as_u64()), (Some("other,tool_look"), Some(2)), "nome fora da lista sai other");
        assert!(!rows.iter().any(|row| row.to_string().contains("modelo") || row.to_string().contains("opus")));

        assert!(!rows.iter().any(|row| row.to_string().contains(&period)));
    }

    #[tokio::test]
    async fn runtime_settings_come_from_the_environment_and_an_unreadable_file_leaves_them_out() {
        let (home, diary) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        // Impede que a descoberta da instalação leia o .env do checkout da máquina.
        std::fs::create_dir_all(home.path().join(".hangar/current")).unwrap();
        std::fs::create_dir_all(home.path().join(".claude")).unwrap();
        std::fs::write(home.path().join(".claude/settings.json"), r#"{"env": {"TYPESAFE_API_KEY": "tsk-segredo"}}"#).unwrap();
        let env = crate::accounts::environment::AccountEnvironment::from_map([
            ("HOME".into(), home.path().display().to_string()), ("USERPROFILE".into(), home.path().display().to_string()),
            ("CP_GROQ_API_KEY".into(), "gsk-segredo".into()), ("CP_DICTATION_ORGANIZATION_MODE".into(), "harness".into()),
        ].into());
        let config = crate::config_sync::runtime::RuntimeConfig::new(env).unwrap();
        let diag = crate::diag::DiagClient::new().with_diary(Some(diary.path().to_owned()));
        let runtime = effective_runtime(&config, None).unwrap();
        let values: BTreeMap<&str, String> = server_settings(home.path(), Some(&runtime), None, &diag).into_iter().collect();
        assert_eq!((values["dictation_org"].as_str(), values["dictation_provider"].as_str(), values["jev"].as_str()),
            ("harness", "groq", "configured"));

        std::fs::write(&config.path, "{").unwrap();
        assert!(effective_runtime(&config, None).is_err());
        let values: BTreeMap<&str, String> = server_settings(home.path(), None, None, &diag).into_iter().collect();
        assert!(!values.contains_key("dictation_org") && !values.contains_key("dictation_provider") && !values.contains_key("jev"));
        assert!(values.contains_key("connect"), "o resto do retrato sai");
        assert!(diary_codes(&diag, diary.path()).await.iter().any(|row| row["codigo"] == "telemetry_settings_runtime_unreadable"));
    }

    #[tokio::test]
    async fn unreadable_whatsapp_install_is_omitted_with_a_diagnostic() {
        let (home, diary) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let paths = hangar_whatsapp::install::Paths::new(home.path());
        let record = paths.record();
        std::fs::create_dir_all(record.parent().unwrap()).unwrap();
        std::fs::write(&record, r#"{"tag": "private-fixture", "binary": ""}"#).unwrap();
        let diag = crate::diag::DiagClient::new().with_diary(Some(diary.path().to_owned()));
        let values = server_settings(home.path(), Some(&json!({})), None, &diag);
        assert!(!values.iter().any(|(key, _)| *key == "whatsapp"));
        let rows = diary_codes(&diag, diary.path()).await;
        assert!(rows.iter().any(|row| row["codigo"] == "telemetry_settings_whatsapp_unreadable"));
        assert!(!rows.iter().any(|row| row.to_string().contains("private-fixture")));
        assert!(hangar_whatsapp::install::current(&paths).is_none(), "a leitura antiga não muda");
    }

    #[tokio::test(start_paused = true)]
    async fn provider_discovery_that_hangs_times_out_without_provider() {
        let (home, diary) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let telemetry = test_telemetry(home.path(), true);
        let peers = Arc::new(PeerClient::new(PeerBook::new(None)));
        let diag = crate::diag::DiagClient::new().with_diary(Some(diary.path().to_owned()));
        let seen = Arc::new(std::sync::Mutex::new(Some("não lido".to_owned())));
        let read = { let seen = seen.clone(); move |p: Option<String>| { *seen.lock().unwrap() = p; Vec::new() } };
        let day = telemetry.today();
        assert!(daily_snapshot(telemetry, peers, diag.clone(), day, std::future::pending::<Option<String>>, read).await.unwrap().unwrap());
        assert_eq!(*seen.lock().unwrap(), None, "o retrato sai sem o provedor");
        assert!(diary_codes(&diag, diary.path()).await.iter().any(|row| row["codigo"] == "telemetry_settings_provider_timeout"));
    }



    #[tokio::test]
    async fn provider_is_discovered_only_after_the_day_mark() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let home = tempfile::tempdir().unwrap();
        let ts = chrono::DateTime::parse_from_rfc3339("2026-11-27T12:00:00Z").unwrap().to_utc();
        let start = || { let t = Arc::new(Telemetry::for_test(home.path(), Arc::new(move || ts))); t.set_consent(true).unwrap(); t };
        let mut telemetry = start();
        let peers = Arc::new(PeerClient::new(PeerBook::new(None)));
        let calls = Arc::new(AtomicUsize::new(0));
        let round = |telemetry: &Arc<Telemetry>| {
            let calls = calls.clone();
            let discover = move || { calls.fetch_add(1, Ordering::SeqCst); async { Some("codex".to_owned()) } };
            daily_snapshot(telemetry.clone(), peers.clone(), crate::diag::DiagClient::new(), telemetry.today(),
                discover, |p: Option<String>| vec![("default_provider", p.unwrap_or_default())])
        };
        let day = home.path().join(".hangar/telemetry-v3/queue/2026-11-27.json");
        std::fs::create_dir_all(&day).unwrap();
        assert!(round(&telemetry).await.unwrap().is_err());
        assert!(round(&telemetry).await.unwrap().is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0, "dia que não se grava não roda as CLIs");
        std::fs::remove_dir(&day).unwrap();
        assert!(round(&telemetry).await.unwrap().unwrap());
        let saved = std::fs::read_to_string(&day).unwrap();
        assert!(saved.contains(SERVER_SETTINGS_MARK) && saved.contains("codex"), "marca e contagem gravadas juntas: {saved}");
        drop(telemetry);
        telemetry = start();
        assert!(round(&telemetry).await.unwrap().unwrap());
        assert_eq!(calls.load(Ordering::SeqCst), 1, "o dia já contado não redescobre, nem depois de reiniciar");
        assert_eq!(telemetry.counters().iter().filter(|c| c.dims["setting"] == "default_provider" && c.dims["value"] == "codex").map(|c| c.count).sum::<u64>(), 1);
    }

    #[tokio::test]
    async fn new_consent_on_the_same_day_takes_the_server_snapshot_again() {
        let home = tempfile::tempdir().unwrap();
        let ts = chrono::DateTime::parse_from_rfc3339("2026-11-27T12:00:00Z").unwrap().to_utc();
        let telemetry = Arc::new(Telemetry::for_test(home.path(), Arc::new(move || ts)));
        telemetry.set_consent(true).unwrap();
        let peers = Arc::new(PeerClient::new(PeerBook::new(None)));
        let round = || daily_snapshot(telemetry.clone(), peers.clone(), crate::diag::DiagClient::new(), telemetry.today(),
            || async { Some("codex".to_owned()) }, |p: Option<String>| vec![("default_provider", p.unwrap_or_default())]);
        let provider = || telemetry.counters().iter().any(|c| c.dims["setting"] == "default_provider");
        let day = telemetry.today();
        assert!(snapshot_due(None, &day, telemetry.consent_epoch()));
        assert!(round().await.unwrap().unwrap());
        let done = (day.clone(), telemetry.consent_epoch());
        assert!(!snapshot_due(Some(&done), &day, telemetry.consent_epoch()), "o mesmo dia e aceite não repetem");
        telemetry.set_consent(false).unwrap();
        telemetry.set_consent(true).unwrap();
        assert!(!provider(), "a recusa apagou o retrato do período anterior");
        assert!(snapshot_due(Some(&done), &day, telemetry.consent_epoch()), "aceite novo no mesmo dia tira o retrato de novo");
        assert!(round().await.unwrap().unwrap());
        assert!(provider());
    }

    #[tokio::test(start_paused = true)]
    async fn daily_snapshot_crossing_utc_midnight_does_not_count_on_the_next_day() {
        use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
        let home = tempfile::tempdir().unwrap();
        let start = chrono::DateTime::parse_from_rfc3339("2026-11-27T23:59:59Z").unwrap().timestamp();
        let clock = Arc::new(AtomicI64::new(start));
        let now = clock.clone();
        let telemetry = Arc::new(Telemetry::for_test(home.path(), Arc::new(move || {
            chrono::DateTime::from_timestamp(now.load(Ordering::SeqCst), 0).unwrap()
        })));
        telemetry.set_consent(true).unwrap();
        let peers = Arc::new(PeerClient::new(PeerBook::new(None)));
        let diag = crate::diag::DiagClient::new();
        let read = |provider: Option<String>| vec![("default_provider", provider.unwrap())];
        let cross_midnight = || async {
            tokio::time::sleep(Duration::from_secs(2)).await;
            clock.fetch_add(2, Ordering::SeqCst);
            Some("claude".to_owned())
        };
        assert!(daily_snapshot(telemetry.clone(), peers.clone(), diag.clone(), "2026-11-27".into(), cross_midnight, read).await.unwrap().unwrap());
        assert!(telemetry.counters().is_empty(), "o retrato antigo não entrou no dia novo");
        telemetry.save().unwrap();
        let closed = std::fs::read_to_string(home.path().join(".hangar/telemetry-v3/queue/2026-11-27.json")).unwrap_or_default();
        assert!(!closed.contains("default_provider") && !closed.contains(SERVER_SETTINGS_MARK), "dia fechado não foi reaberto: {closed}");

        let calls = AtomicUsize::new(0);
        for _ in 0..2 {
            let discover = || async { calls.fetch_add(1, Ordering::SeqCst); Some("codex".to_owned()) };
            assert!(daily_snapshot(telemetry.clone(), peers.clone(), diag.clone(), "2026-11-28".into(), discover, read).await.unwrap().unwrap());
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(telemetry.counters().iter().map(|c| (c.dims["value"].as_str(), c.count)).collect::<Vec<_>>(), [("codex", 1)]);
    }

    #[test]
    fn peers_snapshot_whose_mark_cannot_be_written_is_not_done() {
        let home = tempfile::tempdir().unwrap();
        let telemetry = test_telemetry(home.path(), true);
        let file = home.path().join("peers.json");
        std::fs::write(&file, json!({"a": {"base_url": "http://192.168.0.10:8765", "token": "t"}}).to_string()).unwrap();
        let peers = PeerClient::new(PeerBook::new(Some(file)));
        let collect = || peers_snapshot(&telemetry, &peers, &telemetry.today(), telemetry.consent_epoch());
        let day = home.path().join(format!(".hangar/telemetry-v3/queue/{}.json", telemetry.today()));
        std::fs::create_dir_all(&day).unwrap();
        assert!(collect().is_err());
        assert!(collect().is_err(), "a falha continua até gravar");
        std::fs::remove_dir(&day).unwrap();
        assert!(collect().unwrap());
        assert!(collect().unwrap());
        assert_eq!(telemetry.counters(), vec![Counter::new(Event::Peers, Dims::new().with("kind", PeerKind::Lan), None, 1)], "contado uma vez só");
        assert!(day.is_file());
    }

    #[test]
    fn tailscale_address_is_counted_by_kind_only() {
        assert_eq!(classify("https://x.tail1.ts.net:8765"), Some(PeerKind::Tailscale));
        assert_eq!(classify("http://100.64.0.2:8765"), Some(PeerKind::Tailscale));
        assert_eq!(classify("https://casa.exemplo.hangar.dev.br"), Some(PeerKind::Connect));
        assert_eq!(classify("http://192.168.0.10:8765"), Some(PeerKind::Lan));
        assert_eq!(classify("http://[fe80::1]:8765"), Some(PeerKind::Lan));
        assert_eq!(classify("http://notebook-tunel:8765"), Some(PeerKind::Lan));
        assert_eq!(classify("https://exemplo.com.br"), Some(PeerKind::Public));
        assert_eq!(classify("não é url"), None);

        let home = tempfile::tempdir().unwrap();
        let telemetry = test_telemetry(home.path(), true);
        let file = home.path().join("peers.json");
        std::fs::write(&file, json!({
            "a": {"base_url": "https://x.tail1.ts.net:8765", "token": "t"},
            "b": {"base_url": "http://100.64.0.2:8765", "token": "t"},
            "c": {"base_url": "https://casa.exemplo.hangar.dev.br", "token": "t"},
            "d": {"base_url": "http://192.168.0.10:8765", "token": "t"},
            "e": {"base_url": "https://exemplo.com.br", "token": "t"},
            "f": {"base_url": "http://10.0.0.2:8765", "token": "t", "enabled": false},
            "g": {"app": {"nome": "celular"}},
            "h": {"base_url": "https://y.ts.net", "token": ""},
        }).to_string()).unwrap();
        let peers = PeerClient::new(PeerBook::new(Some(file)));
        let collect = || peers_snapshot(&telemetry, &peers, &telemetry.today(), telemetry.consent_epoch());
        assert!(collect().unwrap());
        assert!(collect().unwrap(), "o dia já contado também é feito");
        let kind = |kind: PeerKind, n: u64| has(&telemetry, Event::Peers, Dims::new().with("kind", kind), None, n);
        assert!(kind(PeerKind::Tailscale, 2), "um retrato por dia");
        assert!(kind(PeerKind::Connect, 1) && kind(PeerKind::Lan, 1) && kind(PeerKind::Public, 1) && kind(PeerKind::Disabled, 1));
        assert_eq!(telemetry.counters().len(), 5);
        let wire = serde_json::to_string(&telemetry.counters()).unwrap();
        assert!(!wire.contains("tail1") && !wire.contains("192.168") && !wire.contains("exemplo"), "{wire}");
    }
}
