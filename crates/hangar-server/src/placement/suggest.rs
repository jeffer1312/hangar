//! A escolha da máquina antes da abertura (`machine` no pedido) e `POST /api/placement/suggest`, a mesma
//! escolha sem abrir nada.
use std::{net::SocketAddr, sync::Arc, time::Duration};

use axum::{body::to_bytes, extract::{ConnectInfo, Request, State}, http::StatusCode, response::Response};
use chrono::Utc;
use futures_util::future::join_all;
use reqwest::Method;
use serde_json::{Value, json};

use hangar_core::project::ProjectId;
use hangar_placement::{AgentMode, KnownRepo, MachineFacts, OpenContext, Placement, PlacementConfig, ReasonCode, UiMode, choose, text, why_text};

use super::{clone, context, facts, forward};
use crate::groups::peers::PeerError;
use crate::routes::{AppState, owner_gate};
use crate::session_open::resolve::{CreatorFacts, creator_codex_mode, creator_mode};
use crate::session_open::{AgentProvider, OpenCaller, OpenError, SessionOpenRequest};
use crate::session_write::{detail_body, json_response};

/// O valor de `machine` que pede a escolha.
pub const AUTO: &str = "auto";
/// Quanto cada máquina tem para responder os fatos: a que passa disso sai da escolha.
const FACTS_WAIT: Duration = Duration::from_secs(2);

/// O que a abertura faz depois da escolha.
pub enum Step {
    /// Abre nesta máquina; `placement` vai na resposta quando o pedido trouxe `auto`.
    Here(Option<Value>),
    /// Abre em `server` com este corpo (`forward::open`); `placement` é `None` com máquina explícita.
    /// `cloned`: o repositório foi clonado lá agora (o aviso vai aos `avisos` da resposta). `reserve`: a raiz daqui e o
    /// id de lá, reservados só quando lá confirmar a abertura.
    There { server: String, body: Value, placement: Option<Value>, cloned: bool, reserve: Option<(String, String)> },
    Refused(OpenError),
}

/// A escolha do `POST /api/sessions` com `machine`. `auto` fora do balanceamento abre aqui com o motivo;
/// máquina explícita com campo que só vale aqui é recusada, nunca aberta lá sem ele.
pub async fn step(st: &Arc<AppState>, request: &SessionOpenRequest, caller: &OpenCaller) -> Step {
    let mut chosen = None;
    let step = choose_step(st, request, caller, &mut chosen).await;
    if request.machine.as_deref() == Some(AUTO) { count_placed(st.counting(), caller, chosen.as_ref(), &step); }
    step
}

/// `session_placed` da abertura com `auto`, só do dono. `chosen`: o motivo da escolha; `None` é o pedido que
/// ficou fora do balanceamento antes dela. A sugestão da tela (`suggest`) não passa aqui.
fn count_placed(telemetry: Option<&hangar_telemetry::Telemetry>, caller: &OpenCaller, chosen: Option<&ReasonCode>, step: &Step) {
    use hangar_telemetry::events::{Code, Dims, Event, PLACEMENT_REASONS, PlacementOutcome};
    let Some(telemetry) = telemetry.filter(|_| matches!(caller, OpenCaller::Owner)) else { return };
    let (outcome, clone) = match step {
        Step::Here(_) => (PlacementOutcome::Here, false),
        Step::There { cloned, .. } => (PlacementOutcome::Elsewhere, *cloned),
        Step::Refused(_) => (PlacementOutcome::Refused, false),
    };
    // Só a etiqueta `code` do motivo: projeto e `why` ficam de fora.
    let code = chosen.map_or_else(|| "not_balanced".to_owned(),
        |reason| serde_json::to_value(reason).ok().and_then(|v| v["code"].as_str().map(str::to_owned)).unwrap_or_default());
    let dims = Dims::new().with("outcome", outcome).with("reason", Code::known(&code, PLACEMENT_REASONS)).with("clone", clone);
    telemetry.count(Event::SessionPlaced, dims, 1);
}

async fn choose_step(st: &Arc<AppState>, request: &SessionOpenRequest, caller: &OpenCaller, chosen: &mut Option<ReasonCode>) -> Step {
    let local = st.current_server_id();
    let Some(machine) = request.machine.as_deref() else { return Step::Here(None) };
    let auto = machine == AUTO;
    if !auto && machine == local { return Step::Here(None) }
    let kept = local_only(st, request, caller).await;
    let here = |why: &str| Step::Here(Some(wire(&not_balanced(&local, why))));
    // Daqui em diante `machine` explícita é a chave do `peers.json`, que alcança a máquina.
    let key;
    let machine = if !auto && kept.is_none() {
        match machine_key(st, machine).await {
            Some(found) => { key = found; key.as_str() }
            None => {
                let mut error = forward::machine_unknown(machine);
                error.msg.push_str(" nem é o id informado por uma delas");
                return Step::Refused(error);
            }
        }
    } else { machine };
    match (auto, kept) {
        (true, Some(why)) => return here(why),
        (false, Some(why)) => return Step::Refused(OpenError::new(400, "placement_not_here",
            format!("este pedido só abre nesta máquina ({})", why_text(why))).with_params(json!({"why": why, "server": machine}))),
        _ => {}
    }
    let config = if auto {
        match load_config(st).await {
            Ok(config) if config.agent_mode == AgentMode::Off => return here("mode_off"),
            Ok(config) => config,
            Err(code) => { skipped(st, code); return here(code) }
        }
    } else { PlacementConfig::default() };
    let ctx = match context::build(st, &st.placement_reservations, Some(&request.cwd), true).await {
        Ok(ctx) => ctx,
        Err(code) if auto => { skipped(st, code); return here(code) }
        Err(code) => return Step::Refused(OpenError::new(503, "placement_context_unavailable",
            "não consegui ler a pasta pedida nesta máquina para abri-la na outra").with_params(json!({"detalhe": code}))),
    };
    // Repositório sem `origin` de projeto só existe nesta pasta: lá não há o que abrir nem clonar. No `auto` ele
    // já ficou aqui.
    if !auto && ctx.cwd_root.is_some() && ctx.cwd_project.is_none() {
        return Step::Refused(OpenError::new(409, "placement_repo_unpublished", format!("a pasta é um repositório sem `origin` \
            publicado: ele só existe nesta máquina e não abre em {machine}")).with_params(json!({"server": machine})));
    }
    let (placement, dest) = if auto {
        let (placement, facts) = place(st, &ctx, &config).await;
        *chosen = Some(placement.reason.clone());
        match chosen_step(&placement, &ctx.local_id, config.agent_mode) {
            Chosen::Kept => return Step::Here(Some(wire(&placement))),
            Chosen::Here => {
                if let Some(root) = &ctx.cwd_root { st.placement_reservations.reserve(root, &placement.server); }
                return Step::Here(Some(wire(&placement)));
            }
            Chosen::Ask => return Step::Refused(ask(&placement)),
            Chosen::There => {}
        }
        let dest = facts.into_iter().find(|(_, f)| f.server_id == placement.server).ok_or(None);
        (Some(placement), dest)
    } else {
        (None, peer_facts(st, machine.to_owned(), &facts_path(ctx.cwd_project.as_ref())).await.map_err(Some))
    };
    // `server` é a chave do `peers.json`, que alcança a máquina; `dest.server_id`, o id que ela informou.
    let fallback = placement.as_ref().map_or(machine, |p| p.server.as_str()).to_owned();
    let (server, mut dest) = match dest {
        Ok((key, dest)) if dest.capable => (key, dest),
        Ok((server, _)) => return Step::Refused(OpenError::new(409, "placement_peer_old",
            format!("{server} está numa versão sem a escolha de máquina: abra com `{server}::<nome>`")).with_params(json!({"server": server}))),
        Err(dropped) => return Step::Refused(peer_unavailable(&fallback, dropped.as_ref())),
    };
    let reserve = ctx.cwd_root.clone().map(|root| (root, dest.server_id.clone()));
    let inherited = match inherited(st, request).await {
        Ok(inherited) => inherited,
        Err(error) => return Step::Refused(error),
    };
    let missing = ctx.cwd_project.as_ref().filter(|p| !dest.repos.iter().any(|r| &r.project == *p));
    let cloned = missing.is_some();
    if let Some(project) = missing {
        let root = match clone_there(st, &server, &ctx, &dest, placement.as_ref()).await {
            Ok(root) => root,
            Err(error) => return Step::Refused(error),
        };
        dest.repos.push(KnownRepo { project: project.clone(), root });
    }
    match forward::body(request, &ctx, &dest, &inherited) {
        Some(mut body) => {
            // A sessão do clone novo instala o que o projeto pede antes do resto.
            if cloned {
                body["initial_prompt"] = json!(crate::session_open::baton::fresh_clone_prompt(body["initial_prompt"].as_str()));
            }
            Step::There { server, body, placement: placement.as_ref().map(wire), cloned, reserve }
        }
        None => Step::Refused(clone_unavailable(&server, placement.as_ref())),
    }
}

/// Clona em `dest` o repositório da pasta do pedido e devolve a raiz lá. A pasta é a da escolha; com a
/// máquina pedida, a da tela para ela, senão a que ela calcula.
async fn clone_there(st: &AppState, server: &str, ctx: &OpenContext, dest: &MachineFacts, placement: Option<&Placement>)
    -> Result<String, OpenError> {
    let dir = match placement.and_then(|p| p.clone.as_ref()) {
        Some(target) => Some(target.dir.clone()),
        None => match load_config(st).await {
            Ok(config) => config.clone_dirs.get(&dest.server_id).cloned(),
            Err(code) => { skipped(st, code); None }
        }.or_else(|| dest.clone_dir.clone()),
    };
    let root_here = ctx.cwd_root.clone();
    let origin = tokio::task::spawn_blocking(move || root_here.and_then(|r| hangar_workspace::worktrees::origin_url(&r))).await.ok().flatten();
    let (Some(dir), Some(origin)) = (dir, origin) else { return Err(clone_unavailable(server, placement)) };
    clone::remote(&st.peers, server, &origin, &dir).await
}

/// `POST /api/placement/suggest`: o corpo de abertura → a máquina que a tela marca e o motivo, sem abrir.
pub async fn suggest(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Response {
    if let Err(response) = owner_gate(&st, peer, &request).await { return response; }
    let parsed = to_bytes(request.into_body(), crate::session_write::BODY_LIMIT).await.ok()
        .and_then(|bytes| SessionOpenRequest::parse(&bytes).ok());
    let Some(open) = parsed else {
        return json_response(StatusCode::UNPROCESSABLE_ENTITY, detail_body("erro_corpo_invalido", "pedido de abertura inválido", json!({})));
    };
    json_response(StatusCode::OK, wire(&suggestion(&st, &open).await))
}

async fn suggestion(st: &Arc<AppState>, request: &SessionOpenRequest) -> Placement {
    let local = st.current_server_id();
    if let Some(why) = local_only(st, request, &OpenCaller::Owner).await { return not_balanced(&local, why) }
    let config = match load_config(st).await {
        Ok(config) if config.ui_mode == UiMode::Off => return not_balanced(&local, "mode_off"),
        Ok(config) => config,
        Err(code) => { skipped(st, code); return not_balanced(&local, code) }
    };
    match context::build(st, &st.placement_reservations, Some(&request.cwd), false).await {
        Ok(ctx) => place(st, &ctx, &config).await.0,
        Err(code) => { skipped(st, code); not_balanced(&local, code) }
    }
}

/// O que a abertura `auto` faz com a escolha.
#[derive(Debug, PartialEq)]
enum Chosen {
    /// Fora do balanceamento: abre aqui sem reservar a pasta.
    Kept,
    Here,
    /// O Ask só pergunta quando a escolha sai desta máquina: aqui não há o que confirmar.
    Ask,
    There,
}

fn chosen_step(placement: &Placement, local: &str, mode: AgentMode) -> Chosen {
    if matches!(placement.reason, ReasonCode::NotBalanced { .. }) { Chosen::Kept }
    else if placement.server == local { Chosen::Here }
    else if mode == AgentMode::Ask { Chosen::Ask }
    else { Chosen::There }
}

/// A escolha com os fatos lidos agora. Sem os fatos daqui não balanceia (vai ao diário e abre aqui).
async fn place(st: &Arc<AppState>, ctx: &OpenContext, config: &PlacementConfig) -> (Placement, Vec<(String, MachineFacts)>) {
    let local = || async { facts::local(st, None, false).await };
    let others = || gather(st, ctx.cwd_project.as_ref(), &ctx.local_id);
    match place_with(ctx, config, Utc::now(), local, others).await {
        Ok(chosen) => chosen,
        Err(code) => { skipped(st, code); (not_balanced(&ctx.local_id, code), Vec::new()) }
    }
}

/// A escolha com os fatos desta máquina (`local`) e, só se ela passou do limite ou uma regra aponta outra, os das
/// outras (`others`, em paralelo). Cada fato vai com a chave do `peers.json` que alcança a máquina (a daqui, com o id
/// daqui). A pasta reservada para outra máquina só lê os das outras; com a reservada fora deles (desligou), a
/// reserva cai e a pasta volta à escolha. `Err`: os fatos daqui, quando precisos, não vieram.
async fn place_with<L, O>(ctx: &OpenContext, config: &PlacementConfig, now: chrono::DateTime<Utc>, local: impl FnOnce() -> L,
    others: impl FnOnce() -> O) -> Result<(Placement, Vec<(String, MachineFacts)>), &'static str>
where L: std::future::Future<Output = Result<MachineFacts, &'static str>>, O: std::future::Future<Output = Vec<(String, MachineFacts)>> {
    // Lidos uma vez só, no primeiro caminho que precisar deles.
    let mut others = Some(others);
    // Mesma pasta, trabalho não publicado e pasta sem git decidem sem fato nenhum.
    let free;
    let (ctx, gathered) = match hangar_placement::pinned(ctx, config) {
        Some(pinned) if pinned.server == ctx.local_id => return Ok((pinned, Vec::new())),
        // Só a reserva prende noutra máquina (sessão daqui na pasta prende aqui).
        Some(pinned) => {
            let seen = match others.take() { Some(others) => others().await, None => Vec::new() };
            if seen.iter().any(|(_, f)| f.server_id == pinned.server) { return Ok((pinned, seen)) }
            free = OpenContext { same_folder_server: None, ..ctx.clone() };
            if let Some(pinned) = hangar_placement::pinned(&free, config) { return Ok((pinned, Vec::new())) }
            (&free, Some(seen))
        }
        None => (ctx, None),
    };
    // A escolha não olha os repositórios daqui: a pasta do pedido é desta máquina.
    let mut seen = vec![(ctx.local_id.clone(), local().await?)];
    let only = |seen: &[(String, MachineFacts)]| seen.iter().map(|(_, f)| f.clone()).collect::<Vec<_>>();
    let rest = match gathered {
        Some(rest) => rest,
        None => {
            let first = choose(ctx, &only(&seen), config, now);
            if first.server == ctx.local_id && first.warnings.is_empty() { return Ok((first, seen)) }
            match others.take() { Some(others) => others().await, None => Vec::new() }
        }
    };
    seen.extend(rest);
    Ok((choose(ctx, &only(&seen), config, now), seen))
}

/// Os fatos das máquinas ligadas do `peers.json`; a que não responde em 2 s fica fora, e a que se diz esta também.
async fn gather(st: &Arc<AppState>, project: Option<&ProjectId>, local: &str) -> Vec<(String, MachineFacts)> {
    let peers = st.peers.clone();
    let keys = tokio::task::spawn_blocking(move || peers.enabled_ids()).await.unwrap_or_default();
    let path = facts_path(project);
    join_all(keys.into_iter().filter(|key| key != local).map(|key| peer_facts(st, key, &path))).await.into_iter().flatten()
        .filter(|(_, f)| f.server_id != local).collect()
}

/// A chave do `peers.json` de `machine`: ela mesma, ou a da máquina cujo id informado (o `CP_SERVER_ID` de lá, nos
/// fatos) é esse; a escolha e as mensagens dão o id, que pode diferir da chave daqui.
async fn machine_key(st: &AppState, machine: &str) -> Option<String> {
    let keys = st.peers.addressable().await;
    if keys.contains(machine) { return Some(machine.to_owned()) }
    let path = facts_path(None);
    join_all(keys.into_iter().map(|key| peer_facts(st, key, &path))).await.into_iter().flatten()
        .find_map(|(key, f)| (f.server_id == machine).then_some(key))
}

/// Por que uma máquina ficou fora da escolha: o código do diário e a causa. `answered`: ela respondeu, recusando
/// ou com corpo que não serve.
#[derive(Debug)]
struct Dropped { code: &'static str, cause: String, answered: bool }

/// Os fatos de `key`; a máquina descartada vai ao diário com o código e a causa, nunca à tela.
async fn peer_facts(st: &AppState, key: String, path: &str) -> Result<(String, MachineFacts), Dropped> {
    let read = match tokio::time::timeout(FACTS_WAIT, st.peers.call(&key, Method::GET, path, None)).await {
        Ok(answer) => read_facts(key.clone(), answer),
        Err(_) => Err(Dropped { code: "placement_peer_timeout", cause: format!("{key} não respondeu em {} s", FACTS_WAIT.as_secs()),
            answered: false }),
    };
    if let Err(dropped) = &read {
        let cause = dropped.cause.chars().take(300).collect::<String>();
        st.diag.report_with("rust.placement_peer_dropped", "", dropped.code, "máquina fora da escolha do balanceamento",
            json!({"server": key, "cause": cause}));
    }
    read
}

/// A máquina pedida que não deu os fatos. A que respondeu recusando não é "confira se está ligada".
fn peer_unavailable(server: &str, dropped: Option<&Dropped>) -> OpenError {
    // A causa vem crua de lá (corpo, erro do serde): curta, como no diário.
    let cause = dropped.map(|d| d.cause.chars().take(300).collect::<String>());
    let msg = match (dropped, &cause) {
        (Some(d), Some(cause)) if d.answered => format!("{server} não deu os fatos do balanceamento: {cause}"),
        _ => format!("{server} não respondeu aos fatos do balanceamento; confira se ela está ligada"),
    };
    OpenError::new(503, "placement_peer_unavailable", msg)
        .with_params(json!({"server": server, "cause": cause, "detalhe": dropped.map(|d| d.code)}))
}

/// A resposta de uma máquina à rota de fatos, com a chave do `peers.json` que a alcança. O id que vale nas
/// regras, nas reservas e no `machine` encaminhado é o que ela informa (o `CP_SERVER_ID` de lá); sem ele ela entra
/// como incapaz, com a chave no lugar do id: as regras são gravadas pelo id e a chave daqui pode não ser a dela.
/// A rota inexistente lá (404 sem código) é versão sem a escolha: entra como incapaz, para o aviso dizer isso.
fn read_facts(key: String, answer: Result<Option<Value>, PeerError>) -> Result<(String, MachineFacts), Dropped> {
    let unreadable = |cause: String| Dropped { code: "placement_peer_unreadable", cause, answered: true };
    let facts = match answer {
        Ok(Some(value)) => serde_json::from_value::<MachineFacts>(value).map_err(|e| unreadable(format!("{key}: fatos ilegíveis ({e})")))
            .map(|f| if f.server_id.is_empty() { MachineFacts { server_id: key.clone(), capable: false, ..f } } else { f })?,
        Ok(None) => return Err(unreadable(format!("{key}: resposta dos fatos vazia"))),
        Err(error) if error.missing_route() => MachineFacts { server_id: key.clone(), ..Default::default() },
        Err(error) => {
            let (code, answered) = match &error {
                PeerError::Refused { .. } => ("placement_peer_refused", true),
                PeerError::IdentityMismatch { .. } => ("placement_peer_identity", true),
                PeerError::Unknown => ("placement_machine_unknown", false),
                _ => ("placement_peer_unreachable", false),
            };
            return Err(Dropped { code, cause: error.text(&key), answered });
        }
    };
    Ok((key, facts))
}

fn facts_path(project: Option<&ProjectId>) -> String {
    match project {
        Some(project) => format!("/api/placement/facts?project={}", form_urlencoded::byte_serialize(project.0.as_bytes()).collect::<String>()),
        None => "/api/placement/facts".into(),
    }
}

/// Por que o pedido só abre nesta máquina; `None`: pode ir a outra.
async fn local_only(st: &AppState, request: &SessionOpenRequest, caller: &OpenCaller) -> Option<&'static str> {
    if let Some(why) = kept_by_request(request, caller) { return Some(why) }
    let creator = request.creator.as_deref()?;
    // A sessão de convidado abre filha pelo token do dono: a filha segue as regras dele.
    match crate::session_open::server::read_guests(&st.accounts.env.claude_base) {
        Ok(Some(raw)) if raw["sessions"].get(creator).is_some() => return Some("guest"),
        Ok(_) => {}
        Err(_) => return Some("guests_unreadable"),
    }
    // O árbitro abre executor e revisor antes de juntá-los ao grupo `orq`: quem conduz uma execução viva fica aqui.
    match orchestrates(st, creator).await {
        Ok(true) => return Some("orq"),
        Ok(false) => {}
        Err(()) => return Some("orq_unreadable"),
    }
    // O grupo `orq` recusa membro de fora: a orquestração abre sempre aqui.
    match st.groups.as_ref()?.link(creator).await {
        Ok(Some(sidecar)) if sidecar.orq => Some("orq"),
        Ok(_) => None,
        Err(_) => Some("groups_unreadable"),
    }
}

/// `creator` é o condutor (`<gid>-orq`) ou o árbitro de uma orquestração viva, pela mesma leitura de
/// `groups::orq::is_orchestrator`.
async fn orchestrates(st: &AppState, creator: &str) -> Result<bool, ()> {
    use crate::orchestration::runs;
    let (home, creator) = (st.accounts.env.home.clone(), creator.to_owned());
    tokio::task::spawn_blocking(move || runs::active(&home, runs::now()).map(|rows| leads(&rows, &creator)))
        .await.map_err(|_| ())?.map_err(|_| ())
}

fn leads(rows: &[Value], name: &str) -> bool {
    rows.iter().any(|row| row["name"] == name || row["arbiter"] == name)
}

fn kept_by_request(request: &SessionOpenRequest, caller: &OpenCaller) -> Option<&'static str> {
    if !matches!(caller, OpenCaller::Owner) { return Some("guest") }
    if request.prewarm.is_some() { return Some("prewarm") }
    let named = |value: &Option<String>| value.as_deref().is_some_and(|v| !v.is_empty());
    // Conta, motor e perfil são pastas e cadastros desta máquina.
    if request.config_dir.is_some() || named(&request.engine) || request.engine_account.is_some() || request.engine_binding.is_some()
        || request.codex_account.is_some() || named(&request.omp_profile) {
        return Some("local_field");
    }
    None
}

pub(super) async fn load_config(st: &AppState) -> Result<PlacementConfig, &'static str> {
    let home = st.accounts.env.home.clone();
    match tokio::task::spawn_blocking(move || hangar_placement::store::load(&home)).await {
        Ok(Ok(config)) => Ok(config),
        _ => Err("placement_config_unreadable"),
    }
}

/// O que vem da criadora: lá ela não existe. Só a criadora é lida aqui, sem a resolução da abertura (nome em uso,
/// cota, conta), que é de lá. Modelo e esforço nunca vêm dela; o padrão salvo daqui fica de fora, e lá vale o de lá.
async fn inherited(st: &Arc<AppState>, request: &SessionOpenRequest) -> Result<forward::Inherited, OpenError> {
    let Some(name) = request.creator.as_deref().filter(|_| request.inherit != Some(false)) else { return Ok(forward::Inherited::default()) };
    let creator = crate::session_open::server::creator_here(st, name).await?;
    Ok(creator.map(|creator| from_creator(request, &creator)).unwrap_or_default())
}

fn from_creator(request: &SessionOpenRequest, creator: &CreatorFacts) -> forward::Inherited {
    // Conta, motor e perfil ficaram aqui (`kept_by_request`); o modelo de subagente é do Claude, como na abertura.
    let provider = match request.provider.as_deref() {
        Some(name) => AgentProvider::parse(name),
        None if request.subagent_model.is_some() => Some(AgentProvider::Claude),
        None => Some(creator.provider),
    };
    let surfaces = matches!(provider, Some(AgentProvider::Claude | AgentProvider::Codex | AgentProvider::Antigravity));
    let headless = (request.headless.is_none() && surfaces && !request.read_only).then_some(creator.headless);
    // Com `inherit: true` o modo da criadora vem antes do padrão salvo; sem ele vale o padrão de lá.
    let asked = request.inherit == Some(true) && matches!(creator.provider, AgentProvider::Claude | AgentProvider::Codex);
    let permission_mode = match provider {
        Some(AgentProvider::Claude) if asked => creator_mode(creator),
        // O Codex com terminal recusa modo pedido: lá ele fica com o padrão de lá, e sem superfície sabida também.
        Some(AgentProvider::Codex) if asked && request.headless.or(headless) == Some(true) => creator_codex_mode(creator),
        _ => None,
    };
    forward::Inherited {
        provider: (request.provider.is_none() && request.subagent_model.is_none()).then(|| creator.provider.name().to_owned()),
        permission_mode: permission_mode.map(|mode| mode.name().to_owned()),
        headless,
    }
}

/// Leitura que falhou tira o pedido do balanceamento: vai ao diário, a sessão abre aqui.
fn skipped(st: &AppState, code: &str) {
    st.diag.report("rust.placement_skipped", "", code, "balanceamento fora: a abertura ficou nesta máquina");
}

fn ask(placement: &Placement) -> OpenError {
    let server = &placement.server;
    OpenError::new(409, "placement_ask", format!("o balanceamento pede confirmação: a sugestão é abrir em {server} ({}). Pergunte à \
        pessoa e abra de novo com a máquina escolhida (`machine: \"{server}\"` no new_session, `--machine {server}` no hangar-send)",
        text(placement)))
        .with_params(json!({"placement": wire(placement)}))
}

fn clone_unavailable(server: &str, placement: Option<&Placement>) -> OpenError {
    OpenError::new(409, "placement_clone_unavailable", format!("{server} não tem o repositório desta pasta nem pasta de clone \
        conhecida: escolha outra máquina (`machine` no new_session, `--machine` no hangar-send) ou a pasta de clone dela na tela"))
        .with_params(json!({"server": server, "placement": placement.map(wire)}))
}

pub fn not_balanced(local: &str, why: &str) -> Placement {
    Placement { server: local.to_owned(), reason: ReasonCode::NotBalanced { why: why.to_owned() }, clone: None, warnings: Vec::new() }
}

/// `placement` da resposta: a escolha com o texto de onde e por quê.
pub fn wire(placement: &Placement) -> Value {
    let mut value = json!(placement);
    value["text"] = json!(text(placement));
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use hangar_placement::{Exclusion, Warning};

    #[test]
    fn what_only_exists_here_keeps_the_request_here() {
        let owner = OpenCaller::Owner;
        let plain = SessionOpenRequest { cwd: "/x".into(), machine: Some(AUTO.into()), ..Default::default() };
        assert_eq!(kept_by_request(&plain, &owner), None);
        let guest = OpenCaller::Guest { root: "/g".into() };
        assert_eq!(kept_by_request(&plain, &guest), Some("guest"));
        let cases: [(&str, fn(&mut SessionOpenRequest)); 6] = [
            ("prewarm", |r| r.prewarm = Some("p".into())),
            ("local_field", |r| r.config_dir = Some("/h/.claude-x".into())),
            ("local_field", |r| r.engine = Some("kimi".into())),
            ("local_field", |r| r.codex_account = Some("a1".into())),
            ("local_field", |r| r.omp_profile = Some("p1".into())),
            ("local_field", |r| r.engine_account = Some("e1".into())),
        ];
        for (why, change) in cases {
            let mut request = plain.clone();
            change(&mut request);
            assert_eq!(kept_by_request(&request, &owner), Some(why), "{request:?}");
        }
        // Motor vazio é o mesmo que nenhum: a tela manda `engine: null`.
        assert_eq!(kept_by_request(&SessionOpenRequest { engine: Some(String::new()), ..plain.clone() }, &owner), None);
        // A filha ligada vai a outra máquina: a ligação é feita pela federação dos grupos.
        assert_eq!(kept_by_request(&SessionOpenRequest { link: true, creator: Some("pai".into()), ..plain }, &owner), None);
    }

    #[test]
    fn an_old_machine_is_incapable_and_a_silent_one_is_out() {
        let (key, old) = read_facts("vm".into(), Err(PeerError::Refused { status: 404, detail: json!("Not Found") })).unwrap();
        assert!(!old.capable && old.server_id == "vm" && key == "vm");
        // 404 com código é recusa da rota de lá, não versão antiga.
        let refused = read_facts("vm".into(), Err(PeerError::Refused { status: 404, detail: json!({"code": "x"}) })).unwrap_err();
        assert_eq!((refused.code, refused.answered), ("placement_peer_refused", true));
        let silent = read_facts("vm".into(), Err(PeerError::Transport("prazo".into()))).unwrap_err();
        assert_eq!((silent.code, silent.answered, silent.cause.as_str()), ("placement_peer_unreachable", false, "prazo"));
    }

    #[test]
    fn a_dropped_machine_keeps_the_cause_and_one_that_answered_is_not_told_to_be_on() {
        for (status, detail) in [(401, json!("token")), (403, json!("proibido")), (503, json!({"code": "placement_facts_list_unavailable"}))] {
            let dropped = read_facts("vm".into(), Err(PeerError::Refused { status, detail })).unwrap_err();
            assert_eq!(dropped.code, "placement_peer_refused");
            assert!(dropped.cause.contains(&format!("HTTP {status}")), "{}", dropped.cause);
            let error = peer_unavailable("vm", Some(&dropped));
            assert_eq!((error.status, error.code), (503, Some("placement_peer_unavailable")));
            assert_eq!(error.params["cause"], json!(dropped.cause));
            assert!(!error.msg.contains("ligada") && error.msg.contains(&format!("HTTP {status}")), "{}", error.msg);
        }
        let garbled = read_facts("vm".into(), Ok(Some(json!({"server_id": 3})))).unwrap_err();
        assert_eq!((garbled.code, garbled.answered), ("placement_peer_unreadable", true));
        assert_eq!(read_facts("vm".into(), Ok(None)).unwrap_err().code, "placement_peer_unreadable");
        let silent = Dropped { code: "placement_peer_timeout", cause: "vm não respondeu em 2 s".into(), answered: false };
        let error = peer_unavailable("vm", Some(&silent));
        assert!(error.msg.contains("confira se ela está ligada"), "{}", error.msg);
        assert_eq!((&error.params["cause"], &error.params["detalhe"]), (&json!("vm não respondeu em 2 s"), &json!("placement_peer_timeout")));
    }

    #[test]
    fn the_id_is_the_one_the_machine_reports_and_the_key_still_reaches_it() {
        let facts = |id: &str| json!({"server_id": id, "version": "1", "capable": true, "mem_total": 1, "mem_available": 1,
            "working": 0, "cpu_pct": null, "repos": [], "clone_dir": null});
        let (key, nb) = read_facts("notebook".into(), Ok(Some(facts("nb")))).unwrap();
        assert_eq!((key.as_str(), nb.server_id.as_str()), ("notebook", "nb"));
        // Sem `CP_SERVER_ID` lá, a chave, e incapaz: não entra na escolha.
        let (_, blank) = read_facts("notebook".into(), Ok(Some(facts("")))).unwrap();
        assert_eq!((blank.server_id.as_str(), blank.capable), ("notebook", false));
    }

    #[test]
    fn the_conductor_and_the_arbiter_of_a_live_run_stay_here() {
        let rows = [json!({"name": "g1-orq", "gid": "g1", "arbiter": "arb"})];
        assert!(leads(&rows, "g1-orq") && leads(&rows, "arb"));
        assert!(!leads(&rows, "executor") && !leads(&[], "arb"));
    }

    fn creator(provider: AgentProvider, headless: bool, mode: &str) -> CreatorFacts {
        CreatorFacts { provider, headless, config_dir: Ok(None), permission_mode: Some(mode.into()), codex_account: None }
    }

    #[test]
    fn only_what_came_from_the_creator_goes_explicit() {
        let asked = SessionOpenRequest { creator: Some("pai".into()), ..Default::default() };
        let claude = creator(AgentProvider::Claude, true, "bypassPermissions");
        // Sem `inherit: true` o modo é o padrão salvo de lá; o provedor e a superfície são da criadora.
        let plain = from_creator(&asked, &claude);
        assert_eq!(plain, forward::Inherited { provider: Some("claude".into()), headless: Some(true), ..Default::default() });
        let with = from_creator(&SessionOpenRequest { inherit: Some(true), ..asked.clone() }, &claude);
        assert_eq!(with.permission_mode.as_deref(), Some("bypassPermissions"));
        // Provedor e superfície pedidos não vêm dela.
        let none = from_creator(&SessionOpenRequest { provider: Some("claude".into()), headless: Some(false), ..asked.clone() }, &claude);
        assert_eq!(none, forward::Inherited::default());
        // Codex com terminal: lá fica com o modo de lá; sem terminal, o modo Codex da criadora.
        let terminal = from_creator(&SessionOpenRequest { inherit: Some(true), ..asked.clone() }, &creator(AgentProvider::Codex, false, "Full Access"));
        assert_eq!((terminal.permission_mode, terminal.headless), (None, Some(false)));
        let headless = from_creator(&SessionOpenRequest { inherit: Some(true), ..asked }, &creator(AgentProvider::Codex, true, "Full Access"));
        assert_eq!(headless.permission_mode.as_deref(), Some("Full Access"));
    }

    #[test]
    fn ask_only_asks_when_the_choice_leaves_this_machine() {
        let at = |server: &str| Placement { server: server.into(), reason: ReasonCode::LocalWithinLimits, clone: None, warnings: Vec::new() };
        assert_eq!(chosen_step(&at("pc"), "pc", AgentMode::Ask), Chosen::Here);
        assert_eq!(chosen_step(&at("nb"), "pc", AgentMode::Ask), Chosen::Ask);
        assert_eq!(chosen_step(&at("nb"), "pc", AgentMode::Auto), Chosen::There);
        assert_eq!(chosen_step(&not_balanced("pc", "orq"), "pc", AgentMode::Ask), Chosen::Kept);
    }

    fn machine(id: &str, working: u32) -> MachineFacts {
        MachineFacts { server_id: id.into(), version: "1".into(), capable: true, mem_total: 0, mem_available: 0, working, cpu_pct: None,
            repos: Vec::new(), clone_dir: None }
    }

    fn reserved_for(server: &str) -> OpenContext {
        OpenContext { local_id: "pc".into(), cwd_root: Some("/w/repo".into()), same_folder_server: Some(server.into()), ..Default::default() }
    }

    fn now() -> chrono::DateTime<Utc> { chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 11, 27, 15, 0, 0).unwrap() }

    #[tokio::test]
    async fn a_folder_reserved_elsewhere_does_not_read_the_facts_here() {
        let config = PlacementConfig::default();
        let local = || async { Err("placement_facts_list_unavailable") };
        let others = || async { vec![("notebook".to_owned(), machine("nb", 9))] };
        let (placement, seen) = place_with(&reserved_for("nb"), &config, now(), local, others).await.unwrap();
        assert_eq!((placement.server.as_str(), placement.reason), ("nb", ReasonCode::SameFolder));
        assert_eq!(seen.len(), 1, "os fatos de lá seguem para o encaminhamento");
    }

    #[tokio::test]
    async fn a_reservation_for_a_machine_that_left_falls_back_to_the_choice() {
        let config = PlacementConfig::default();
        let local = || async { Ok(machine("pc", 0)) };
        let others = || async { vec![("vm".to_owned(), machine("vm", 0))] };
        let (placement, _) = place_with(&reserved_for("nb"), &config, now(), local, others).await.unwrap();
        assert_eq!((placement.server.as_str(), placement.reason), ("pc", ReasonCode::LocalWithinLimits));
        // Daqui fora do limite, a carga escolhe entre as que responderam, com os fatos já lidos.
        let busy = || async { Ok(machine("pc", 9)) };
        let others = || async { vec![("vm".to_owned(), machine("vm", 0))] };
        let (placement, _) = place_with(&reserved_for("nb"), &config, now(), busy, others).await.unwrap();
        assert_eq!(placement.server, "vm");
    }

    #[test]
    fn the_mcp_and_cli_wait_longer_than_the_worst_open_elsewhere() {
        use crate::groups::peers::{CLONE_TIMEOUT, OPEN_TIMEOUT};
        let worst = FACTS_WAIT * 2 + CLONE_TIMEOUT + OPEN_TIMEOUT;
        assert!(Duration::from_secs(hangar_registration::mcp_register::TOOL_TIMEOUT_S) > worst);
        // O `hangar-send --new` só espera tanto com máquina pedida (`auto` ou id); sem ela, o prazo curto de sempre.
        for machine in ["auto", "nb"] { assert!(Duration::from_secs_f64(hangar_send::create_timeout(machine)) > worst, "{machine}"); }
        assert!(Duration::from_secs_f64(hangar_send::create_timeout("")) < worst);
    }

    #[test]
    fn the_answer_says_where_and_why_and_a_refused_request_too() {
        let p = Placement { server: "nb".into(), reason: ReasonCode::LeastLoaded { local_out: Exclusion::Working }, clone: None,
            warnings: vec![Warning::RuleServerOut { server: "vm".into(), why: Exclusion::Missing }] };
        let value = wire(&p);
        assert_eq!(value["reason"], json!({"code": "least_loaded", "local_out": "working"}));
        assert_eq!(value["text"], text(&p));
        let kept = wire(&not_balanced("pc", "orq"));
        assert_eq!((&kept["server"], &kept["reason"]), (&json!("pc"), &json!({"code": "not_balanced", "why": "orq"})));
        assert_eq!(kept["text"], "fora do balanceamento (orquestração)");
    }

    #[test]
    fn ask_and_clone_refuse_with_the_suggestion() {
        let p = Placement { server: "nb".into(), reason: ReasonCode::LocalWithinLimits, clone: None, warnings: Vec::new() };
        let error = ask(&p);
        assert_eq!((error.status, error.code), (409, Some("placement_ask")));
        assert_eq!(error.params["placement"]["server"], "nb");
        // O id da sugestão é aceito pelo `--machine`/`machine` (`machine_key`): a mensagem o cita pronto.
        assert!(error.msg.contains("--machine nb") && error.msg.contains(r#"machine: "nb""#), "{}", error.msg);
        let error = clone_unavailable("vm", None);
        assert_eq!((error.status, error.code), (409, Some("placement_clone_unavailable")));
        assert!(error.msg.starts_with("vm "), "{}", error.msg);
    }

    #[test]
    fn telemetry_session_placed_counts_the_owner_with_closed_dims() {
        use hangar_telemetry::events::Event;
        let home = tempfile::tempdir().unwrap();
        let telemetry = crate::telemetry_hooks::test_telemetry(home.path(), true);
        let rule = ReasonCode::ProjectRule { project: ProjectId("git:github.com/a/segredo".into()) };
        count_placed(Some(&telemetry), &OpenCaller::Guest { root: "/g".into() }, Some(&rule), &Step::Here(None));
        assert!(telemetry.counters().is_empty(), "convidado não conta");
        count_placed(Some(&telemetry), &OpenCaller::Owner, None, &Step::Here(None));
        count_placed(Some(&telemetry), &OpenCaller::Owner, Some(&ReasonCode::NotBalanced { why: "orq".into() }), &Step::Here(None));
        let there = Step::There { server: "notebook".into(), body: json!({}), placement: None, cloned: true, reserve: None };
        count_placed(Some(&telemetry), &OpenCaller::Owner, Some(&ReasonCode::LeastLoaded { local_out: Exclusion::Working }), &there);
        count_placed(Some(&telemetry), &OpenCaller::Owner, Some(&rule), &Step::Refused(ask(&Placement { server: "nb".into(),
            reason: rule.clone(), clone: None, warnings: Vec::new() })));
        let mut seen: Vec<_> = telemetry.counters().into_iter().filter(|c| c.event == Event::SessionPlaced.as_str())
            .map(|c| (c.dims["outcome"].clone(), c.dims["reason"].clone(), c.dims["clone"].clone(), c.count)).collect();
        seen.sort();
        let row = |o: &str, r: &str, c: &str, n| (o.to_owned(), r.to_owned(), c.to_owned(), n);
        assert_eq!(seen, [row("elsewhere", "least_loaded", "true", 1), row("here", "not_balanced", "false", 2),
            row("refused", "project_rule", "false", 1)]);
        assert!(telemetry.counters().iter().all(|c| c.dims.values().all(|v| !["segredo", "notebook", "nb", "orq"].iter().any(|s| v.contains(s)))),
            "projeto, máquina e `why` nunca saem");
    }

    #[test]
    fn the_facts_path_carries_the_project_encoded() {
        assert_eq!(facts_path(Some(&ProjectId("git:github.com/a/b".into()))), "/api/placement/facts?project=git%3Agithub.com%2Fa%2Fb");
        assert_eq!(facts_path(None), "/api/placement/facts");
    }
}
