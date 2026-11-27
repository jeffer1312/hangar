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
use hangar_telemetry::{CONSENT_SCOPE, Collection, Disabled, Failure, Telemetry};

use crate::groups::peers::PeerClient;
use crate::routes::AppState;
use crate::session_write::{detail, json_response};

const OWNER_ONLY: (&str, &str) = ("telemetry_owner_only", "só o dono pode ler ou mudar a telemetria");
const OWNER_PATH: &str = "/api/telemetry/owner";
const CONVERGE_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// O retrato de peers sai uma vez por dia (`once`); a hora só decide quando ele é tirado.
const SNAPSHOT_EVERY: Duration = Duration::from_secs(60 * 60);
/// O cliente manda `app.visible_time` neste compasso: mais que isso num evento é relógio torto ou repetição.
const VISIBLE_CAP_MS: u64 = 5 * 60 * 1000;
/// `detalhe` de `rota.escolhida` (`packages/core/src/rota.ts`).
const ROUTES: &[&str] = &["principal", "rede_local", "tailscale", "connect"];
/// `codigo` de `rota.falhou`, em minúsculas: nome do erro do JS ou o código do Expo.
const ROUTE_FAILURES: &[&str] = &["error", "timeouterror", "aborterror", "typeerror", "route_timeout", "erro"];
/// `codigo` de `app.crash`, em minúsculas: nome do erro do JS, ou `panic` do nativo.
const FATAL_CODES: &[&str] = &["error", "typeerror", "referenceerror", "rangeerror", "syntaxerror", "panic"];

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
        Ok(telemetry) => Some(Arc::new(telemetry)),
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
        let mut done: Option<String> = None;
        loop {
            let today = snapshot.today();
            if done.as_deref() != Some(today.as_str()) {
                let (st, env, diag) = (snapshot_st.clone(), snapshot_st.accounts.env.clone(), snapshot_diag.clone());
                let discover = move || async move { default_provider(&st).await };
                let read = move |provider: Option<String>| {
                    server_settings(&env.home, &crate::runtime::codex_launch::runtime_config(&env), provider.as_deref(), &diag)
                };
                match daily_snapshot(snapshot.clone(), book.clone(), discover, read).await {
                    Ok(Ok(true)) => done = Some(today),
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

/// Quantas máquinas do `peers.json` há de cada tipo, uma vez por dia (UTC). `Ok(true)` quando o dia já
/// está contado; desligada, `Ok(false)`; marca que não se lê nem se grava, `Err`. Nos dois últimos a
/// próxima hora tenta de novo. Bloqueante.
pub(crate) fn peers_snapshot(telemetry: &Telemetry, peers: &PeerClient) -> std::io::Result<bool> {
    if !telemetry.enabled() { return Ok(false); }
    let enabled: HashSet<String> = peers.enabled_ids().into_iter().collect();
    let mut kinds: BTreeMap<PeerKind, u64> = BTreeMap::new();
    for id in peers.ids() {
        // Só entrada de máquina (endereço e token), como a varredura do `PeerBook`.
        let Some(base_url) = peers.base_url(&id) else { continue };
        let kind = if enabled.contains(&id) { classify(&base_url) } else { Some(PeerKind::Disabled) };
        if let Some(kind) = kind { *kinds.entry(kind).or_default() += 1; }
    }
    if telemetry.once(&format!("peers:{}", chrono::Utc::now().format("%Y-%m-%d")))? {
        for (kind, n) in kinds { telemetry.count(Event::Peers, Dims::new().with("kind", kind), n); }
    }
    Ok(true)
}

/// O retrato do dia: máquinas e, se o do servidor ainda não saiu hoje, as preferências dele. `discover` roda as
/// CLIs, por isso só depois da marca do dia gravada: marca que não se grava não descobre nada. `Ok(Ok(true))` = dia feito.
async fn daily_snapshot<Fut: std::future::Future<Output = Option<String>>>(
    telemetry: Arc<Telemetry>,
    peers: Arc<PeerClient>,
    discover: impl FnOnce() -> Fut,
    read: impl FnOnce(Option<String>) -> Vec<(&'static str, String)> + Send + 'static,
) -> Result<std::io::Result<bool>, tokio::task::JoinError> {
    let marks = telemetry.clone();
    let epoch = marks.consent_epoch();
    let day = marks.today();
    let mark_day = day.clone();
    let step = tokio::task::spawn_blocking(move || -> std::io::Result<(bool, bool)> {
        let done = peers_snapshot(&marks, &peers)?;
        let stable = done && marks.today() == mark_day && marks.consent_epoch() == epoch;
        Ok((stable, stable && marks.once_in_epoch(epoch, &format!("settings:server:{mark_day}"))?))
    }).await?;
    let (done, due) = match step { Ok(step) => step, Err(e) => return Ok(Err(e)) };
    if due {
        let provider = discover().await;
        tokio::task::spawn_blocking(move || {
            let values = read(provider);
            if telemetry.today() == day {
                count_settings(&telemetry, epoch, ClientKind::Server, values.iter().map(|(name, value)| (*name, value.as_str())));
            }
        }).await?;
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

/// O que o servidor guarda das escolhas do dono, já nos nomes de `events::SETTINGS`. Arquivo que não se lê deixa a
/// preferência dele de fora, com o código no diário; sem provedor, saem de fora ele e o esforço. Bloqueante.
pub(crate) fn server_settings(home: &Path, runtime: &Value, provider: Option<&str>, diag: &crate::diag::DiagClient) -> Vec<(&'static str, String)> {
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
    let text = |key: &str| runtime[key].as_str().map(str::trim).unwrap_or_default().to_owned();
    let organization = text("dictation_organization_mode");
    values.push(("dictation_org", if organization.is_empty() { "none".to_owned() } else { organization }));
    // O primeiro serviço da lista é o que transcreve; sem lista, a chave Groq antiga.
    let provider = runtime["transcription_providers"][0]["kind"].as_str().map(str::to_owned)
        .or_else(|| (!text("groq_api_key").is_empty()).then(|| "groq".to_owned()));
    values.push(("dictation_provider", provider.unwrap_or_else(|| "none".to_owned())));
    match crate::connect::state::read(&home.join(".hangar").join("connect")) {
        Ok(linked) => values.push(("connect", on(linked.is_some()))),
        Err(_) => unreadable("telemetry_settings_connect_unreadable"),
    }
    let computer = hangar_computer_control_setup::Paths::new(home.to_path_buf(), Vec::new());
    match hangar_computer_control_setup::read_entry(computer.main_file(), false) {
        Ok(main) => values.push(("computer_use", on(main.entry.is_some()))),
        Err(_) => unreadable("telemetry_settings_computer_use_unreadable"),
    }
    let whatsapp = hangar_whatsapp::install::current(&hangar_whatsapp::install::Paths::new(home)).is_some();
    values.push(("whatsapp", if whatsapp { "installed" } else { "no" }.to_owned()));
    values.push(("jev", if text("jev_api_key").is_empty() { "no" } else { "configured" }.to_owned()));
    values
}

fn count_settings<'a>(telemetry: &Telemetry, epoch: u64, client: ClientKind, values: impl IntoIterator<Item = (&'a str, &'a str)>) {
    for (name, value) in values {
        let Some((setting, value)) = events::setting(name, value) else { continue };
        telemetry.count_in_epoch(epoch, Event::Settings, Dims::new().with("client", client).with("setting", setting).with("value", value), 1);
    }
}

/// `settings.snapshot` de um app: um por dia por cliente e aparelho (`aparelho`, o id que o app já guarda); sem id
/// válido, um por dia por cliente. O id fica só no `once.json` desta máquina.
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
        screen_event(telemetry, epoch, &day, item);
    }
}

fn screen_event(telemetry: &Telemetry, epoch: u64, day: &str, item: &Value) {
    let text = |key: &str| item[key].as_str().unwrap_or_default();
    let client = ClientKind::parse(text("cliente")).filter(|c| *c != ClientKind::Server).unwrap_or(ClientKind::Other);
    let took = item["ms"].as_u64();
    let dims = Dims::new().with("client", client);
    match text("evento") {
        "app.visible_time" => {
            let minutes = took.map_or(0, |ms| (ms.min(VISIBLE_CAP_MS) + 30_000) / 60_000);
            if minutes > 0 { telemetry.count_in_epoch(epoch, Event::AppVisibleMinutes, dims, minutes); }
        }
        "rota.escolhida" => {
            let dims = dims.with("failed", false).with("route", Code::known(text("detalhe"), ROUTES));
            match took {
                Some(ms) => telemetry.duration_in_epoch(epoch, Event::ClientRoute, dims, Duration::from_millis(ms)),
                None => telemetry.count_in_epoch(epoch, Event::ClientRoute, dims, 1),
            }
        }
        "rota.falhou" => {
            let code = Code::known(&text("codigo").to_ascii_lowercase(), ROUTE_FAILURES);
            telemetry.count_in_epoch(epoch, Event::ClientRoute, dims.with("failed", true).with("code", code), 1);
        }
        "chat.opened" => if let Some(ms) = took { telemetry.duration_in_epoch(epoch, Event::ChatOpen, dims, Duration::from_millis(ms)); },
        "app.crash" => {
            let code = Code::known(&text("codigo").to_ascii_lowercase(), FATAL_CODES);
            telemetry.count_in_epoch(epoch, Event::ClientFatal, dims.with("code", code), 1);
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

fn view(telemetry: &Telemetry) -> Value {
    json!({"consent": telemetry.consent(), "consent_scope": CONSENT_SCOPE, "disabled_by": telemetry.disabled_by().map(Disabled::as_str)})
}

/// `GET` anuncia o escopo. `PUT` só aceita o "sim" acompanhado desse escopo; `false` legado revoga.
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
    let diag = st.diag.clone();
    let done = tokio::task::spawn_blocking(move || {
        // Gravada, a escolha vale; o que não se aplicou agora vai ao diário e a passada seguinte refaz.
        if let Some(consent) = choice && let Some(e) = telemetry.set_consent(consent)? { report(&diag, Failure::Disk(e)); }
        Ok::<Value, std::io::Error>(view(&telemetry))
    }).await;
    match done {
        Ok(Ok(body)) => json_response(StatusCode::OK, body),
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
        let mut req = axum::http::Request::builder().method(Method::PUT).uri("/api/telemetry")
            .header("authorization", format!("Bearer {TOKEN}")).header("content-type", "application/json");
        if guest { req = req.header("x-hangar-guest", "1"); }
        let response = setting(State(st.clone()), ConnectInfo("127.0.0.1:5000".parse().unwrap()), req.body(Body::from(body.to_owned())).unwrap()).await;
        let status = response.status();
        let raw = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn guest_put_is_refused_and_changes_nothing() {
        let home = tempfile::tempdir().unwrap();
        let st = test_state(home.path(), false);
        let (status, body) = put(&st, r#"{"consent": true}"#, true).await;
        assert_eq!((status, &body["detail"]["code"]), (StatusCode::FORBIDDEN, &json!("telemetry_owner_only")));
        assert_eq!(st.telemetry.as_ref().unwrap().consent(), None);
    }

    #[tokio::test]
    async fn telemetry_consent_get_announces_scope_and_legacy_false_revokes() {
        let home = tempfile::tempdir().unwrap();
        let st = test_state(home.path(), false);
        let get = axum::http::Request::get("/api/telemetry").header("authorization", "Bearer owner").body(Body::empty()).unwrap();
        let get = setting(State(st.clone()), ConnectInfo("127.0.0.1:5000".parse().unwrap()), get).await;
        assert_eq!(get.status(), StatusCode::OK);
        let get: Value = serde_json::from_slice(&to_bytes(get.into_body(), 1024).await.unwrap()).unwrap();
        assert_eq!(get, json!({"consent": null, "consent_scope": 3, "disabled_by": null}));
        for body in [r#"{"consent": "sim"}"#, r#"{"consent": null}"#, r#"{"consent": true, "x": 1}"#, "{}", "nada"] {
            let (status, reply) = put(&st, body, false).await;
            assert_eq!((status, &reply["detail"]["code"]), (StatusCode::UNPROCESSABLE_ENTITY, &json!("erro_corpo_invalido")), "{body}");
        }
        assert_eq!(st.telemetry.as_ref().unwrap().consent(), None);
        for body in [r#"{"consent": true}"#, r#"{"consent": true, "consent_scope": 2}"#, r#"{"consent": true, "consent_scope": 4}"#] {
            let (status, reply) = put(&st, body, false).await;
            assert_eq!((status, &reply["detail"]["code"]), (StatusCode::UNPROCESSABLE_ENTITY, &json!("telemetry_consent_scope_required")), "{body}");
        }
        assert_eq!(put(&st, r#"{"consent": true, "consent_scope": 3}"#, false).await,
            (StatusCode::OK, json!({"consent": true, "consent_scope": 3, "disabled_by": null})));
        assert_eq!(put(&st, r#"{"consent": false}"#, false).await,
            (StatusCode::OK, json!({"consent": false, "consent_scope": 3, "disabled_by": null})));
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
    fn telemetry_client_settings_snapshot_is_not_counted_under_option_b() {
        let home = tempfile::tempdir().unwrap();
        let now: DateTime<Utc> = "2026-11-27T12:00:00Z".parse().unwrap();
        let telemetry = Telemetry::for_test(home.path(), Arc::new(move || now));
        telemetry.set_consent(true).unwrap();
        let body = json!({"eventos": [{"evento": "settings.snapshot", "cliente": "native", "aparelho": "a-1",
            "valores": {"theme": "dark"}}]}).to_string();
        count_screen(&telemetry, body.as_bytes(), 1, now);
        assert!(telemetry.counters().is_empty());
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
        let runtime = json!({"dictation_organization_mode": "harness", "groq_api_key": "gsk-segredo", "jev_api_key": "sk-segredo"});
        let diag = crate::diag::DiagClient::new();
        let values: BTreeMap<&str, String> = server_settings(home.path(), &runtime, Some("codex"), &diag).into_iter().collect();
        let expected: BTreeMap<&str, String> = [("conversation_beta", "on"), ("code_mode", "off"), ("default_provider", "codex"),
            ("default_effort", "xhigh"), ("auto_report", "off"), ("dictation_org", "harness"), ("dictation_provider", "groq"),
            ("connect", "off"), ("computer_use", "off"), ("whatsapp", "no"), ("jev", "configured")]
            .into_iter().map(|(k, v)| (k, v.to_owned())).collect();
        assert_eq!(values, expected);
        let bare: BTreeMap<&str, String> = server_settings(home.path(), &json!({}), None, &diag).into_iter().collect();
        assert_eq!((bare.get("default_provider"), bare["dictation_org"].as_str(), bare["dictation_provider"].as_str(), bare["jev"].as_str()),
            (None, "none", "none", "no"));

        let telemetry = test_telemetry(home.path(), true);
        let values = server_settings(home.path(), &runtime, Some("codex"), &diag);
        count_settings(&telemetry, telemetry.consent_epoch(), ClientKind::Server, values.iter().map(|(name, value)| (*name, value.as_str())));
        assert_eq!(telemetry.counters().len(), values.len(), "nenhum valor sai other");
        assert!(telemetry.counters().iter().any(|c| c.dims["setting"] == "dictation_provider" && c.dims["value"] == "groq"));
        assert!(telemetry.counters().iter().all(|c| c.dims.get("client").map(String::as_str) == Some("server")));
        assert!(!serde_json::to_string(&telemetry.counters()).unwrap().contains("segredo"));
    }

    #[tokio::test]
    async fn provider_is_discovered_only_after_the_day_mark() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let home = tempfile::tempdir().unwrap();
        let telemetry = test_telemetry(home.path(), true);
        let peers = Arc::new(PeerClient::new(PeerBook::new(None)));
        let calls = Arc::new(AtomicUsize::new(0));
        let round = || {
            let calls = calls.clone();
            let discover = move || { calls.fetch_add(1, Ordering::SeqCst); async { Some("codex".to_owned()) } };
            daily_snapshot(telemetry.clone(), peers.clone(), discover, |p: Option<String>| vec![("default_provider", p.unwrap_or_default())])
        };
        let marks = home.path().join(".hangar/telemetry-v3/once.json");
        std::fs::create_dir_all(&marks).unwrap();
        assert!(round().await.unwrap().is_err());
        assert!(round().await.unwrap().is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0, "marca que não se grava não roda as CLIs");
        std::fs::remove_dir(&marks).unwrap();
        assert!(round().await.unwrap().unwrap());
        assert!(round().await.unwrap().unwrap());
        assert_eq!(calls.load(Ordering::SeqCst), 1, "o dia já contado não redescobre");
        assert!(telemetry.counters().iter().any(|c| c.dims["setting"] == "default_provider" && c.dims["value"] == "codex"));
    }

    #[test]
    fn peers_snapshot_whose_mark_cannot_be_written_is_not_done() {
        let home = tempfile::tempdir().unwrap();
        let telemetry = test_telemetry(home.path(), true);
        let file = home.path().join("peers.json");
        std::fs::write(&file, json!({"a": {"base_url": "http://192.168.0.10:8765", "token": "t"}}).to_string()).unwrap();
        let peers = PeerClient::new(PeerBook::new(Some(file)));
        let marks = home.path().join(".hangar/telemetry-v3/once.json");
        std::fs::create_dir_all(&marks).unwrap();
        assert!(peers_snapshot(&telemetry, &peers).is_err());
        assert!(telemetry.counters().is_empty());
        std::fs::remove_dir(&marks).unwrap();
        assert!(peers_snapshot(&telemetry, &peers).unwrap());
        assert_eq!(telemetry.counters().len(), 1);
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
        assert!(peers_snapshot(&telemetry, &peers).unwrap());
        assert!(peers_snapshot(&telemetry, &peers).unwrap(), "o dia já contado também é feito");
        let kind = |kind: PeerKind, n: u64| has(&telemetry, Event::Peers, Dims::new().with("kind", kind), None, n);
        assert!(kind(PeerKind::Tailscale, 2), "um retrato por dia");
        assert!(kind(PeerKind::Connect, 1) && kind(PeerKind::Lan, 1) && kind(PeerKind::Public, 1) && kind(PeerKind::Disabled, 1));
        assert_eq!(telemetry.counters().len(), 5);
        let wire = serde_json::to_string(&telemetry.counters()).unwrap();
        assert!(!wire.contains("tail1") && !wire.contains("192.168") && !wire.contains("exemplo"), "{wire}");
    }
}
