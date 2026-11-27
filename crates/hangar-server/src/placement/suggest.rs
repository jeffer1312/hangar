//! A escolha da máquina antes da abertura (`machine` no pedido) e `POST /api/placement/suggest`, a mesma
//! escolha sem abrir nada.
use std::{net::SocketAddr, sync::Arc, time::Duration};

use axum::{body::to_bytes, extract::{ConnectInfo, Request, State}, http::StatusCode, response::Response};
use chrono::Utc;
use futures_util::future::join_all;
use reqwest::Method;
use serde_json::{Value, json};

use hangar_core::project::ProjectId;
use hangar_placement::{AgentMode, Exclusion, MachineFacts, OpenContext, Placement, PlacementConfig, ReasonCode, UiMode, Unpublished, Warning, choose};

use super::{context, facts, forward};
use crate::groups::peers::PeerError;
use crate::routes::{AppState, owner_gate};
use crate::session_open::{AgentProvider, OpenCaller, OpenError, SessionOpenRequest, Surface};
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
    There { server: String, body: Value, placement: Option<Value> },
    Refused(OpenError),
}

/// A escolha do `POST /api/sessions` com `machine`. `auto` fora do balanceamento abre aqui com o motivo;
/// máquina explícita com campo que só vale aqui é recusada, nunca aberta lá sem ele.
pub async fn step(st: &Arc<AppState>, request: &SessionOpenRequest, caller: &OpenCaller) -> Step {
    let local = st.current_server_id();
    let Some(machine) = request.machine.as_deref() else { return Step::Here(None) };
    let auto = machine == AUTO;
    if !auto && machine == local { return Step::Here(None) }
    let kept = local_only(st, request, caller).await;
    let here = |why: &str| Step::Here(Some(wire(&not_balanced(&local, why))));
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
    let ctx = match context::build(st, &st.placement_reservations, Some(&request.cwd), request.branch.as_deref(), true).await {
        Ok(ctx) => ctx,
        Err(code) if auto => { skipped(st, code); return here(code) }
        Err(code) => return Step::Refused(OpenError::new(503, "placement_context_unavailable",
            "não consegui ler a pasta pedida nesta máquina para abri-la na outra").with_params(json!({"detalhe": code}))),
    };
    let (placement, dest) = if auto {
        let (placement, facts) = place(st, &ctx, &config).await;
        if matches!(placement.reason, ReasonCode::NotBalanced { .. }) { return Step::Here(Some(wire(&placement))) }
        if config.agent_mode == AgentMode::Ask { return Step::Refused(ask(&placement)) }
        if let Some(root) = &ctx.cwd_root { st.placement_reservations.reserve(root, &placement.server); }
        // O clone ainda não abre por aqui: a escolha que pede clone é recusada, à vista.
        if placement.clone.is_some() { return Step::Refused(clone_unavailable(&placement.server, Some(&placement))) }
        if placement.server == ctx.local_id { return Step::Here(Some(wire(&placement))) }
        let dest = facts.into_iter().find(|f| f.server_id == placement.server);
        (Some(placement), dest)
    } else {
        (None, peer_facts(st, machine.to_owned(), &facts_path(ctx.cwd_project.as_ref())).await)
    };
    let server = placement.as_ref().map_or(machine, |p| p.server.as_str()).to_owned();
    let dest = match dest {
        Some(dest) if dest.capable => dest,
        Some(_) => return Step::Refused(OpenError::new(409, "placement_peer_old",
            format!("{server} está numa versão sem a escolha de máquina: abra com `{server}::<nome>`")).with_params(json!({"server": server}))),
        None => return Step::Refused(OpenError::new(503, "placement_peer_unavailable",
            format!("{server} não respondeu aos fatos do balanceamento; confira se ela está ligada")).with_params(json!({"server": server}))),
    };
    let inherited = match inherited(st, request).await {
        Ok(inherited) => inherited,
        Err(error) => return Step::Refused(error),
    };
    match forward::body(request, &ctx, &dest, &inherited) {
        Some(body) => Step::There { server, body, placement: placement.as_ref().map(wire) },
        None => Step::Refused(clone_unavailable(&server, placement.as_ref())),
    }
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
    match context::build(st, &st.placement_reservations, Some(&request.cwd), request.branch.as_deref(), false).await {
        Ok(ctx) => place(st, &ctx, &config).await.0,
        Err(code) => { skipped(st, code); not_balanced(&local, code) }
    }
}

/// A escolha com os fatos desta máquina e, só se ela passou do limite ou uma regra aponta outra, os das
/// outras, em paralelo. Sem os fatos daqui não balanceia.
async fn place(st: &Arc<AppState>, ctx: &OpenContext, config: &PlacementConfig) -> (Placement, Vec<MachineFacts>) {
    let now = Utc::now();
    // Mesma pasta, trabalho não publicado e pasta sem git decidem sem fato nenhum.
    let pinned = choose(ctx, &[], config, now);
    let is_pinned = matches!(pinned.reason, ReasonCode::SameFolder | ReasonCode::Unpublished { .. } | ReasonCode::PlainDir);
    if is_pinned && pinned.server == ctx.local_id { return (pinned, Vec::new()) }
    let mut facts = match facts::local(st, ctx.cwd_project.as_ref()).await {
        Ok(local) => vec![local],
        Err(code) => { skipped(st, code); return (not_balanced(&ctx.local_id, code), Vec::new()) }
    };
    if !is_pinned {
        let first = choose(ctx, &facts, config, now);
        if first.server == ctx.local_id && first.warnings.is_empty() { return (first, facts) }
    }
    facts.extend(gather(st, ctx.cwd_project.as_ref(), &ctx.local_id).await);
    (choose(ctx, &facts, config, now), facts)
}

/// Os fatos das máquinas ligadas do `peers.json`; a que não responde em 2 s fica fora.
async fn gather(st: &Arc<AppState>, project: Option<&ProjectId>, local: &str) -> Vec<MachineFacts> {
    let peers = st.peers.clone();
    let ids = tokio::task::spawn_blocking(move || peers.enabled_ids()).await.unwrap_or_default();
    let path = facts_path(project);
    join_all(ids.into_iter().filter(|id| id != local).map(|id| peer_facts(st, id, &path))).await.into_iter().flatten().collect()
}

async fn peer_facts(st: &AppState, id: String, path: &str) -> Option<MachineFacts> {
    let answer = tokio::time::timeout(FACTS_WAIT, st.peers.call(&id, Method::GET, path, None)).await.ok()?;
    read_facts(id, answer)
}

/// A resposta de uma máquina à rota de fatos, pelo id do `peers.json` (o endereço que alcança ela). A rota
/// inexistente lá (404 sem código) é versão sem a escolha: entra como incapaz, para o aviso dizer isso.
fn read_facts(id: String, answer: Result<Option<Value>, PeerError>) -> Option<MachineFacts> {
    match answer {
        Ok(Some(value)) => serde_json::from_value::<MachineFacts>(value).ok().map(|f| MachineFacts { server_id: id, ..f }),
        Err(PeerError::Refused { status: 404, detail }) if detail.get("code").is_none() => Some(MachineFacts {
            server_id: id, version: String::new(), capable: false, mem_total: 0, mem_available: 0, working: 0, cpu_pct: None,
            repos: Vec::new(), clone_dir: None,
        }),
        _ => None,
    }
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
    // O grupo `orq` recusa membro de fora: a orquestração abre sempre aqui.
    match st.groups.as_ref()?.link(creator).await {
        Ok(Some(sidecar)) if sidecar.orq => Some("orq"),
        Ok(_) => None,
        Err(_) => Some("groups_unreadable"),
    }
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
    // A ligação ainda só vale entre sessões desta máquina.
    if request.link { return Some("link") }
    None
}

async fn load_config(st: &AppState) -> Result<PlacementConfig, &'static str> {
    let home = st.accounts.env.home.clone();
    match tokio::task::spawn_blocking(move || hangar_placement::store::load(&home)).await {
        Ok(Ok(config)) => Ok(config),
        _ => Err("placement_config_unreadable"),
    }
}

/// O que viria da criadora, resolvido aqui pela mesma resolução da abertura: lá ela não existe.
async fn inherited(st: &Arc<AppState>, request: &SessionOpenRequest) -> Result<forward::Inherited, OpenError> {
    if request.creator.is_none() || request.inherit == Some(false) { return Ok(forward::Inherited::default()) }
    let spec = crate::session_open::server::resolve_here(st, request).await?.spec;
    // O Codex com terminal recusa modo pedido: lá ele fica com o padrão de lá.
    let terminal_codex = spec.provider == AgentProvider::Codex && spec.surface == Surface::Terminal;
    let surfaces = matches!(spec.provider, AgentProvider::Claude | AgentProvider::Codex | AgentProvider::Antigravity);
    Ok(forward::Inherited {
        provider: Some(spec.provider.name().to_owned()),
        model: spec.model,
        effort: spec.effort,
        permission_mode: spec.permission_mode.filter(|_| !terminal_codex).map(|mode| mode.name().to_owned()),
        headless: surfaces.then_some(spec.surface == Surface::Headless),
    })
}

/// Leitura que falhou tira o pedido do balanceamento: vai ao diário, a sessão abre aqui.
fn skipped(st: &AppState, code: &str) {
    st.diag.report("rust.placement_skipped", "", code, "balanceamento fora: a abertura ficou nesta máquina");
}

fn ask(placement: &Placement) -> OpenError {
    OpenError::new(409, "placement_ask", format!("o balanceamento pede confirmação: a sugestão é abrir em {} ({}). Pergunte à \
        pessoa e abra de novo com a máquina escolhida (`maquina` no new_session, `--machine` no hangar-send)", placement.server, text(placement)))
        .with_params(json!({"placement": wire(placement)}))
}

fn clone_unavailable(server: &str, placement: Option<&Placement>) -> OpenError {
    OpenError::new(409, "placement_clone_unavailable", format!("{server} não tem o repositório desta pasta e o clone ainda não \
        está disponível: escolha outra máquina (`maquina` no new_session, `--machine` no hangar-send)"))
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

pub fn text(p: &Placement) -> String {
    let mut text = match &p.reason {
        ReasonCode::SameFolder => format!("mesma pasta de uma sessão aberta em {}", p.server),
        ReasonCode::Unpublished { why } => format!("{}: fica nesta máquina", match why {
            Unpublished::Ahead => "commit não enviado",
            Unpublished::Dirty => "mudança não commitada",
            Unpublished::NoUpstream => "branch sem upstream",
        }),
        ReasonCode::PlainDir => "pasta sem git que só existe nesta máquina".to_owned(),
        ReasonCode::ProjectRule { project } => format!("a regra do projeto {} manda para {}", project.0, p.server),
        ReasonCode::LocalWithinLimits => "esta máquina está dentro dos limites".to_owned(),
        ReasonCode::LeastLoaded { local_out } => format!("esta máquina saiu da escolha ({}); {} tem menos sessões trabalhando",
            exclusion(*local_out), p.server),
        ReasonCode::NoCandidate => "nenhuma máquina dentro dos limites: fica nesta".to_owned(),
        ReasonCode::NotBalanced { why } => format!("fora do balanceamento ({})", why_text(why)),
    };
    for warning in &p.warnings {
        text.push_str("; ");
        text.push_str(&match warning {
            Warning::RuleServerOut { server, why } => format!("a máquina da regra, {server}, ficou fora ({})", exclusion(*why)),
            Warning::RuleOverridden { server } => format!("a regra do projeto mandaria para {server}"),
            Warning::AllOverLimit => "todas as máquinas passaram do limite".to_owned(),
        });
    }
    if let Some(clone) = &p.clone { text.push_str(&format!("; {} não tem o repositório: clonar em {}", p.server, clone.dir)); }
    text
}

fn exclusion(why: Exclusion) -> &'static str {
    match why {
        Exclusion::Missing => "desligada ou sem resposta",
        Exclusion::NotCapable => "versão sem a escolha de máquina",
        Exclusion::Schedule => "regra de horário",
        Exclusion::Memory => "pouca memória livre",
        Exclusion::Working => "muitas sessões trabalhando",
        Exclusion::Cpu => "processador acima do limite",
        Exclusion::NoCloneDir => "sem o repositório e sem pasta de clone",
    }
}

fn why_text(why: &str) -> &str {
    match why {
        "guest" => "pedido de convidado",
        "prewarm" => "reserva pré-aquecida",
        "local_field" => "conta, motor ou perfil desta máquina",
        "link" => "a ligação só vale nesta máquina",
        "orq" => "orquestração",
        "mode_off" => "balanceamento desligado",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_only_exists_here_keeps_the_request_here() {
        let owner = OpenCaller::Owner;
        let plain = SessionOpenRequest { cwd: "/x".into(), machine: Some(AUTO.into()), ..Default::default() };
        assert_eq!(kept_by_request(&plain, &owner), None);
        let guest = OpenCaller::Guest { root: "/g".into() };
        assert_eq!(kept_by_request(&plain, &guest), Some("guest"));
        let cases: [(&str, fn(&mut SessionOpenRequest)); 7] = [
            ("prewarm", |r| r.prewarm = Some("p".into())),
            ("local_field", |r| r.config_dir = Some("/h/.claude-x".into())),
            ("local_field", |r| r.engine = Some("kimi".into())),
            ("local_field", |r| r.codex_account = Some("a1".into())),
            ("local_field", |r| r.omp_profile = Some("p1".into())),
            ("local_field", |r| r.engine_account = Some("e1".into())),
            ("link", |r| r.link = true),
        ];
        for (why, change) in cases {
            let mut request = plain.clone();
            change(&mut request);
            assert_eq!(kept_by_request(&request, &owner), Some(why), "{request:?}");
        }
        // Motor vazio é o mesmo que nenhum: a tela manda `engine: null`.
        assert_eq!(kept_by_request(&SessionOpenRequest { engine: Some(String::new()), ..plain }, &owner), None);
    }

    #[test]
    fn an_old_machine_is_incapable_and_a_silent_one_is_out() {
        let old = read_facts("vm".into(), Err(PeerError::Refused { status: 404, detail: json!("Not Found") })).unwrap();
        assert!(!old.capable && old.server_id == "vm");
        // 404 com código é recusa da rota de lá, não versão antiga.
        assert_eq!(read_facts("vm".into(), Err(PeerError::Refused { status: 404, detail: json!({"code": "x"}) })), None);
        assert_eq!(read_facts("vm".into(), Err(PeerError::Transport("prazo".into()))), None);
        let facts = json!({"server_id": "outro-nome", "version": "1", "capable": true, "mem_total": 1, "mem_available": 1,
            "working": 0, "cpu_pct": null, "repos": [], "clone_dir": null});
        assert_eq!(read_facts("nb".into(), Ok(Some(facts))).unwrap().server_id, "nb", "vale o id que alcança a máquina");
    }

    #[test]
    fn the_answer_says_where_and_why_and_a_refused_request_too() {
        let p = Placement { server: "nb".into(), reason: ReasonCode::LeastLoaded { local_out: Exclusion::Working }, clone: None,
            warnings: vec![Warning::RuleServerOut { server: "vm".into(), why: Exclusion::Missing }] };
        let value = wire(&p);
        assert_eq!(value["reason"], json!({"code": "least_loaded", "local_out": "working"}));
        assert_eq!(value["text"], "esta máquina saiu da escolha (muitas sessões trabalhando); nb tem menos sessões trabalhando; \
            a máquina da regra, vm, ficou fora (desligada ou sem resposta)");
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
        assert!(error.msg.contains("--machine"), "{}", error.msg);
        let error = clone_unavailable("vm", None);
        assert_eq!((error.status, error.code), (409, Some("placement_clone_unavailable")));
    }

    #[test]
    fn the_facts_path_carries_the_project_encoded() {
        assert_eq!(facts_path(Some(&ProjectId("git:github.com/a/b".into()))), "/api/placement/facts?project=git%3Agithub.com%2Fa%2Fb");
        assert_eq!(facts_path(None), "/api/placement/facts");
    }
}
