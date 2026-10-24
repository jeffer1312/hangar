//! A abertura no servidor: o `POST /api/sessions` do dono e do convidado com login, o bastão do dono,
//! a ponte privada por onde o Python repassa o que entrou direto nele (Connect) e o `OpenHost`
//! com os serviços do Rust.
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::{Body, to_bytes};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};

use super::claude::PluginBridge;
use super::open::{Born, OpenHost};
use super::terminal::{Mux, MuxOutput, ProcessMux, TerminalEnv};
use super::{OpenCaller, OpenError, SessionOpenRequest};
use crate::accounts::environment::AccountEnvironment;
use crate::routes::AppState;

/// Marca do bastão que o Rust repassa ao Python (`rust`): a sucessora com terminal (Claude, Pi, omp, Kimi) volta como
/// pedido de abertura, com a mesma marca, e o Rust a abre. Vinda de fora, sai antes.
pub const DECLINED: &str = "x-hangar-open";

pub struct ServerHost {
    st: Arc<AppState>,
    env: AccountEnvironment,
    terminal: TerminalEnv,
    mux: ProcessMux,
}

/// O ambiente do servidor para a abertura: o do processo e as opções `CP_*` do `.env` do servidor
/// (`user_data::env_file`), que o pydantic do Python lê sem exportar.
/// Como lá, a variável do processo vence a do arquivo.
pub fn server_env(mut process: BTreeMap<String, String>, dotenv: &Path) -> BTreeMap<String, String> {
    let file = std::fs::read_to_string(dotenv).map(|text| super::local::dotenv(&text)).unwrap_or_default();
    for (key, value) in file.into_iter().filter(|(key, _)| key.starts_with("CP_")) {
        process.entry(key).or_insert(value);
    }
    process
}

/// `runtime_config.aplicar({"last_session_provider": …})`: o mesmo arquivo e a mesma troca atômica do
/// Python (temporário na pasta, troca, 0600), que relê pela data do arquivo. Igual não regrava;
/// ilegível não é sobrescrito.
pub fn remember_provider(claude_base: &Path, provider: &str) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind};
    let path = claude_base.join("runtime-config.json");
    let mut config = match std::fs::read(&path) {
        Ok(raw) => serde_json::from_slice::<Value>(&raw).ok().filter(Value::is_object)
            .ok_or_else(|| Error::new(ErrorKind::InvalidData, "runtime-config.json ilegível"))?,
        Err(error) if error.kind() == ErrorKind::NotFound => json!({}),
        Err(error) => return Err(error),
    };
    if config["last_session_provider"] == provider { return Ok(()); }
    config["last_session_provider"] = json!(provider);
    std::fs::create_dir_all(claude_base)?;
    crate::runtime::queue::atomic_write(&path, &serde_json::to_vec_pretty(&config)?)
}

/// O convidado com login (`guests.json`: o Python grava os convidados, os dois gravam o dono das sessões).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginGuest { pub id: String, pub name: String, pub root: PathBuf }

/// `guest_users.lookup_token`, só lendo: o hash do token contra o `token_hash` de cada um. Ausente,
/// ilegível ou com um convidado torto é "nenhum convidado", como no `_load` de lá.
pub fn login_guest(claude_base: &Path, token: &[u8]) -> Option<LoginGuest> {
    use subtle::ConstantTimeEq;
    let raw: Value = serde_json::from_slice(&std::fs::read(claude_base.join("guests.json")).ok()?).ok()?;
    let hash: String = ring::digest::digest(&ring::digest::SHA256, token).as_ref().iter().map(|b| format!("{b:02x}")).collect();
    let text = |guest: &Value, key: &str| guest[key].as_str().map(str::to_owned);
    let guests: Option<Vec<(String, LoginGuest)>> = raw["guests"].as_array()?.iter().map(|g| Some((text(g, "token_hash")?,
        LoginGuest { id: text(g, "id")?, name: text(g, "name")?, root: text(g, "root")?.into() }))).collect();
    guests?.into_iter().find(|(stored, _)| bool::from(stored.as_bytes().ct_eq(hash.as_bytes()))).map(|(_, guest)| guest)
}

/// `guest_users.claim`: o dono da sessão no `guests.json`, sob a `guests.lock` que o Python também
/// pega (`atomico.locked`), relendo o arquivo dentro da trava. Ilegível não é sobrescrito.
pub fn claim_guest(claude_base: &Path, session: &str, guest: &str, life: Option<&str>) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind};
    std::fs::create_dir_all(claude_base)?;
    let lock = std::fs::OpenOptions::new().read(true).append(true).create(true).open(claude_base.join("guests.lock"))?;
    lock.lock()?;
    let path = claude_base.join("guests.json");
    let mut state = match crate::runtime::queue::read_replaced(&path) {
        Ok(raw) => serde_json::from_slice::<Value>(&raw).ok().filter(Value::is_object)
            .ok_or_else(|| Error::new(ErrorKind::InvalidData, "guests.json ilegível"))?,
        Err(error) if error.kind() == ErrorKind::NotFound => json!({}),
        Err(error) => return Err(error),
    };
    if state.get("guests").is_none() { state["guests"] = json!([]); }
    if state.get("sessions").is_none() { state["sessions"] = json!({}); }
    let sessions = state["sessions"].as_object_mut().ok_or_else(|| Error::new(ErrorKind::InvalidData, "guests.json ilegível"))?;
    sessions.insert(session.into(), json!({"guest": guest, "life": life}));
    crate::runtime::queue::atomic_write(&path, &serde_json::to_vec_pretty(&state)?)
}

/// `guest_users.set_life`: a vida nova da sessão que o convidado criou, sob a mesma `guests.lock`.
/// Sem dono gravado não há o que mudar; ilegível não é sobrescrito.
pub fn guest_set_life(claude_base: &Path, session: &str, life: &str) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind};
    let lock = std::fs::OpenOptions::new().read(true).append(true).create(true).open(claude_base.join("guests.lock"))?;
    lock.lock()?;
    let path = claude_base.join("guests.json");
    let mut state = match crate::runtime::queue::read_replaced(&path) {
        Ok(raw) => serde_json::from_slice::<Value>(&raw).ok().filter(Value::is_object)
            .ok_or_else(|| Error::new(ErrorKind::InvalidData, "guests.json ilegível"))?,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let Some(owner) = state["sessions"].get_mut(session).filter(|o| o.is_object()) else { return Ok(()) };
    if owner["life"] == life { return Ok(()); }
    owner["life"] = json!(life);
    crate::runtime::queue::atomic_write(&path, &serde_json::to_vec_pretty(&state)?)
}

/// O convidado do envelope da ponte: `guest` ausente ou nulo é o dono; o id, o convidado dele.
pub fn bridge_guest(envelope: &Value, claude_base: &Path) -> Result<Option<LoginGuest>, OpenError> {
    let malformed = || OpenError::new(400, "erro_corpo_invalido", "guest é o id do convidado ou null");
    if envelope.get("guest_root").is_some() { return Err(malformed()); }
    match &envelope["guest"] {
        Value::Null => Ok(None),
        Value::String(id) if !id.is_empty() => guest_by_id(claude_base, id).map(Some)
            .ok_or_else(|| OpenError::new(403, "erro_criacao_sessao", "convidado inexistente")),
        _ => Err(malformed()),
    }
}

/// O convidado `id` do `guests.json`, para a ponte do Connect, que chega com o id já autenticado lá.
pub fn guest_by_id(claude_base: &Path, id: &str) -> Option<LoginGuest> {
    let raw: Value = serde_json::from_slice(&std::fs::read(claude_base.join("guests.json")).ok()?).ok()?;
    let guest = raw["guests"].as_array()?.iter().find(|g| g["id"] == id)?;
    Some(LoginGuest { id: id.into(), name: guest["name"].as_str()?.into(), root: guest["root"].as_str()?.into() })
}

/// A fila que a sessão morta deixou no nome (a projeção `.hangar-queue/<nome>.jsonl`, que o Rust lê
/// no histórico), o vínculo `then` dela (`.hangar-chain/<nome>.json`) e o loop: a nova não os herda.
pub fn forget_queue(claude_base: &Path, name: &str) -> std::io::Result<()> {
    // O loop que não saiu não segura a limpeza da fila e do `then`; o erro dele volta no fim.
    let looped = crate::loops::store::Store::new(claude_base).forget(name);
    let projection = crate::locate::queue_path(claude_base, name);
    let chain = claude_base.join(".hangar-chain").join(projection.file_name().unwrap_or_default()).with_extension("json");
    for path in [projection.with_extension("jsonl.tmp"), projection, chain.with_extension("json.tmp"), chain] {
        match std::fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
    }
    looped
}

impl ServerHost {
    pub async fn new(st: Arc<AppState>) -> Self {
        // `TerminalEnv::local` pode esperar o `systemctl --user` (até 3 s): fora das threads do runtime.
        let (env, terminal) = tokio::task::spawn_blocking(|| {
            let dotenv = crate::user_data::env_file(std::env::current_dir().ok().as_deref()).unwrap_or_default();
            let env = AccountEnvironment::from_map(server_env(std::env::vars().collect(), &dotenv));
            let terminal = TerminalEnv::local(&env.home, &env.base);
            (env, terminal)
        }).await.unwrap_or_else(|error| std::panic::resume_unwind(error.into_panic()));
        let mux = ProcessMux { diary: crate::diag::diary_dir_in(&env), ..ProcessMux::default() };
        Self { st, env, terminal, mux }
    }

    /// O dono da sessão que o convidado com login abriu vai ao `guests.json`. Falhou: a sessão sai
    /// com a worktree que ela criou, como no `_kill_unclaimed`.
    async fn claim(&self, mut opened: super::open::Opened, guest: &LoginGuest) -> Result<super::open::Opened, OpenError> {
        let name = opened.body["name"].as_str().unwrap_or_default().to_owned();
        let lives = crate::share::life::Lives::new(Arc::new(crate::share::life::MuxProbe::new(self.terminal.program.clone(), self.env.home.clone())), Arc::default());
        // Logo após criar, o multiplexador pode ainda não responder a vida (`claim` do Python).
        let mut life = None;
        for _ in 0..3 {
            life = lives.life(&name, true).await;
            if life.is_some() { break; }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let (base, session, id) = (self.env.claude_base.clone(), name.clone(), guest.id.clone());
        let written = tokio::task::spawn_blocking(move || claim_guest(&base, &session, &id, life.as_deref())).await;
        let cause = match written {
            Ok(Ok(())) => {
                opened.body["owner"] = json!(guest.name);
                return Ok(opened);
            }
            Ok(Err(error)) => error.to_string(),
            Err(_) => "a gravação não terminou".to_owned(),
        };
        tracing::warn!(cause, "o dono da sessão do convidado não foi registrado; a sessão sai");
        // No psmux o `=` não vale no `kill-session` e o nome cru já é exato.
        self.mux.run(vec![self.terminal.program.clone(), "kill-session".into(), "-t".into(), super::terminal::target(self.terminal.windows, &name)]).await;
        if let Err(error) = super::record::remove(&self.env.home, &opened.key) {
            tracing::warn!(kind = ?error.kind(), "registro da sessão do convidado ficou para trás");
        }
        self.st.list.forget(&name);
        self.st.list.invalidate();
        if let Some(undo) = opened.undo { undo.undo().await?; }
        Err(OpenError::new(500, "erro_criacao_sessao", format!("o dono da sessão não foi registrado: {cause}")))
    }
}

impl ServerHost {
    /// O terminal Codex que acabou de nascer usa a conta antes de o lançador gravar o sidecar: a
    /// exclusão dela o vê pelo nascimento publicado (ainda sob a guarda compartilhada da abertura).
    async fn publish_codex_birth(&self, name: &str, codex_home: &str) {
        let instances = crate::accounts::usage::terminal_instances().await;
        let Some(instance) = instances.as_ref().ok().and_then(|live| live.get(name)).cloned() else {
            tracing::warn!(code = "codex_birth_unpublished", "terminal Codex sem identidade no multiplexador; a exclusão da conta não o vê até o sidecar");
            return;
        };
        let (home, name, codex_home, live) = (self.env.home.clone(), name.to_owned(), codex_home.to_owned(), instances.unwrap_or_default());
        let written = tokio::task::spawn_blocking(move || {
            let key = crate::accounts::AccountKey::new(crate::accounts::Provider::Codex, Path::new(&codex_home))?;
            crate::accounts::usage::publish_birth(&home, &name, &key, &instance, &live)
        }).await;
        if !matches!(written, Ok(Ok(()))) {
            tracing::warn!(code = "codex_birth_unpublished", "nascimento do terminal Codex não gravado; a exclusão da conta não o vê até o sidecar");
        }
    }
}

/// Programa da abertura, pelo PATH do servidor e sem os segredos dele no ambiente.
pub async fn run_program(env: &BTreeMap<String, String>, argv: Vec<String>, timeout: Duration) -> MuxOutput {
    let failed = |stderr: String| MuxOutput { code: None, stdout: String::new(), stderr };
    let Some((program, args)) = argv.split_first() else { return failed("comando vazio".into()) };
    let Some(exe) = crate::runtime::process::resolve_program(program, env) else { return failed(format!("{program} não está no PATH")) };
    let mut command = tokio::process::Command::new(exe);
    command.args(args).env_clear().stdin(std::process::Stdio::null()).kill_on_drop(true)
        .envs(env.iter().filter(|(k, _)| !matches!(k.as_str(), "CP_AUTH_TOKEN" | "HANGAR_INTERNAL_SECRET" | "HANGAR_RUNTIME_INSTANCE")));
    match tokio::time::timeout(timeout, command.output()).await {
        Ok(Ok(out)) => MuxOutput { code: out.status.code(), stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned() },
        Ok(Err(e)) => failed(e.to_string()),
        Err(_) => failed(format!("{program} não respondeu no prazo")),
    }
}

impl OpenHost for ServerHost {
    fn home(&self) -> &Path { &self.env.home }
    fn env(&self) -> &BTreeMap<String, String> { &self.env.base }
    fn terminal(&self) -> &TerminalEnv { &self.terminal }
    fn mux(&self) -> &dyn Mux { &self.mux }
    fn bridge(&self) -> Option<PluginBridge> { Some(PluginBridge { port: self.st.cfg.listen.port(), secret: self.st.cfg.auth_token.clone() }) }
    fn server_projects(&self) -> PathBuf { self.env.claude_base.join("projects") }
    /// O servidor roda na `backend/` (é de lá que ele lê o `.env`).
    fn backend_dir(&self) -> Option<PathBuf> { std::env::current_dir().ok() }

    fn run(&self, argv: Vec<String>, timeout: Duration) -> BoxFuture<'_, MuxOutput> {
        Box::pin(run_program(&self.env.base, argv, timeout))
    }

    fn customizations(&self, args: Value) -> BoxFuture<'_, Result<Value, OpenError>> {
        Box::pin(async move {
            crate::claude_customizations::prepare_for_open(&self.st, args).await
                .map_err(|(status, code, detail)| OpenError::new(status, code, detail))
        })
    }

    fn leave_old_group<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Vec<String>, OpenError>> {
        Box::pin(async move {
            let Some(groups) = &self.st.groups else { return Ok(Vec::new()) };
            match crate::groups::exit::leave_and_notify(&self.st, groups, name).await {
                Ok((_, failures)) => Ok(exit_warnings(&failures)),
                Err(_) if groups.has_sidecar(name) => Err(OpenError::new(409, "erro_grupo_limpeza_falhou",
                    format!("o grupo da sessão antiga '{name}' não foi desfeito; tente de novo"))),
                Err(_) => { tracing::warn!("criação: grupos falharam sem grupo a desfazer"); Ok(Vec::new()) }
            }
        })
    }

    fn release_name<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), OpenError>> {
        Box::pin(async move {
            self.st.list.forget(name);
            let (base, owned) = (self.env.claude_base.clone(), name.to_owned());
            if let Err(error) = tokio::task::spawn_blocking(move || forget_queue(&base, &owned)).await
                .unwrap_or_else(|e| Err(std::io::Error::other(e.to_string()))) {
                return Err(OpenError::new(503, "erro_lista_indisponivel", format!("a fila da sessão antiga não foi apagada: {error}"))
                    .with_params(json!({"detalhe": "fila_antiga"})));
            }
            // O registro das sessões abertas é do Rust: a vida que acabou com este nome não prende a nova
            // (conversa, pane e fila dela). No Python não há registro em espera a soltar.
            if let Some(runtime) = self.st.state.runtime.get().cloned() && let Err(error) = runtime.release_name(name).await {
                return Err(OpenError::new(503, "erro_sessao_antiga_presa", format!("a sessão antiga com este nome não saiu do registro: {}", error.code)));
            }
            Ok(())
        })
    }

    fn born<'a>(&'a self, born: &'a Born) -> BoxFuture<'a, Vec<String>> {
        Box::pin(async move {
            if let Some(share) = self.st.share.as_ref().filter(|_| !born.restored) {
                crate::share::pair_routes::session_died(self.st.clone(), share.clone(), Arc::new(crate::share::pair_routes::Direct), &born.name).await;
            }
            if let Some(jsonl) = &born.jsonl { self.st.list.seed(&born.name, jsonl); }
            self.st.list.invalidate();
            if let Some(home) = &born.codex_terminal { self.publish_codex_birth(&born.name, home).await; }
            if !born.remember_provider { return Vec::new(); }
            let (base, provider) = (self.env.claude_base.clone(), born.provider.name());
            match tokio::task::spawn_blocking(move || remember_provider(&base, provider)).await {
                Ok(Ok(())) => Vec::new(),
                Ok(Err(error)) => vec![format!("A sessão foi criada, mas não consegui lembrar o provedor: {error}")],
                Err(error) => vec![format!("A sessão foi criada, mas não consegui lembrar o provedor: {error}")],
            }
        })
    }

    fn warm_codex<'a>(&'a self, name: &'a str, initial_prompt: Option<&'a str>) -> BoxFuture<'a, Vec<String>> {
        Box::pin(async move {
            let opened = match self.st.state.runtime.get() {
                Some(runtime) => runtime.open_codex_headless(name).await,
                None => None,
            };
            // Não subiu: a sessão existe e o próximo envio tenta de novo, como no aquecimento do Python.
            match &opened {
                Some(Ok(())) => {}
                Some(Err(error)) => tracing::warn!(code = %error.code, "Codex sem terminal criado não subiu agora; o próximo envio abre"),
                None => tracing::warn!("Codex sem terminal criado sem runtime para subir agora; o próximo envio abre"),
            }
            let Some(text) = initial_prompt.filter(|t| !t.is_empty()) else { return Vec::new() };
            let (status, body) = crate::session_write::input::write_input(&self.st, name, text, false).await;
            if status.is_success() { return Vec::new(); }
            let why = body["detail"]["msg"].as_str().or(body["detail"].as_str()).unwrap_or("o envio falhou").to_owned();
            vec![format!("A sessão foi criada, mas a primeira mensagem não foi entregue: {why}")]
        })
    }

    fn diary(&self, events: Vec<Value>) { crate::diag::record(crate::diag::diary_dir(), events, "criação"); }

    fn new_session_id(&self) -> String { super::open::uuid4(&crate::mods::state::random_hex(16)) }
    fn new_bridge_key(&self) -> String { crate::mods::state::random_hex(16) }

    fn live_sessions(&self) -> BoxFuture<'_, Result<Vec<(String, String)>, OpenError>> {
        Box::pin(async move {
            let produced = self.st.list.snapshot().await.map_err(|e| OpenError::new(503, "erro_lista_indisponivel",
                "a lista de sessões está indisponível").with_params(json!({"detalhe": e.code})))?;
            Ok(produced.rows.iter().filter_map(|row| Some((row.name.clone(), row.jsonl.clone()?))).collect())
        })
    }

    fn wake(&self, name: &str) {
        let Some(runtime) = self.st.state.runtime.get().cloned() else {
            tracing::warn!(session = %name, "sem o registro do runtime: a sessão sem terminal sobe no primeiro envio");
            return;
        };
        let name = name.to_owned();
        // A falha fica no canal da sessão (`open_inner`); a fila espera a próxima subida.
        tokio::spawn(async move {
            match runtime.open_claude_headless(&name, true).await {
                Some(Ok(())) => {}
                Some(Err(error)) => tracing::warn!(session = %name, code = %error.code, "a sessão sem terminal criada não subiu"),
                None => tracing::warn!(session = %name, "a sessão sem terminal criada sumiu antes de subir"),
            }
        });
    }

    fn prewarm(&self) -> Option<&crate::prewarm::Store> { Some(&self.st.prewarm) }
}

pub enum Decision { Opened(super::open::Opened), Refused(OpenError), NotOurs }

/// `decide` numa tarefa própria: cliente que desconecta no meio não interrompe a criação (a worktree
/// criada sem o pane ficaria para trás) nem o registro do convidado (`claim`), como o
/// `complete_on_cancel` do Python.
async fn decide_detached(st: &Arc<AppState>, request: SessionOpenRequest, caller: OpenCaller, claim: Option<LoginGuest>) -> Decision {
    let link = match request.link_plan(&caller) {
        Ok(link) => link,
        Err(error) => return Decision::Refused(error),
    };
    let st = st.clone();
    tokio::spawn(async move {
        let host = ServerHost::new(st.clone()).await;
        let decision = match (decide(&st, &host, request, caller).await, claim) {
            (Decision::Opened(opened), Some(guest)) => match host.claim(opened, &guest).await {
                Ok(opened) => Decision::Opened(opened),
                Err(error) => Decision::Refused(error),
            },
            (decision, _) => decision,
        };
        match decision {
            Decision::Opened(mut opened) => {
                if let Some(error) = link_child(&st, link, &opened.body).await
                    && let Some(body) = opened.body.as_object_mut() { body.insert("link_error".into(), error); }
                Decision::Opened(opened)
            }
            decision => decision,
        }
    }).await.unwrap_or_else(|_| Decision::Refused(OpenError::new(500, "erro_criacao_sessao", "a abertura não terminou")))
}

/// Põe a sessão aberta no grupo da criadora pela rota de grupo. Falhar não fecha a sessão: devolve o
/// `link_error` da resposta.
async fn link_child(st: &Arc<AppState>, link: super::LinkPlan, opened: &Value) -> Option<Value> {
    let parent = match link {
        super::LinkPlan::No => return None,
        super::LinkPlan::Unlinked(code, detail) => return Some(json!({"code": code, "detail": detail})),
        super::LinkPlan::Join(parent) => parent,
    };
    let child = opened["name"].as_str().unwrap_or_default();
    let code = match crate::groups::bridge::route(st.clone(), "POST", &parent, "pair", json!({"peer": child, "link": true})).await {
        Ok(reply) if reply["status"] == 200 => return None,
        Ok(reply) => {
            let detail = &reply["body"]["detail"];
            detail["code"].as_str().or(detail.as_str()).map_or_else(|| format!("http_{}", reply["status"]), str::to_owned)
        }
        Err(failed) => failed.code,
    };
    tracing::warn!(code = "erro_link_grupo", session = %child, parent = %parent, detail = %code, "a sessão abriu sem ligação");
    Some(json!({"code": "erro_link_grupo", "detail": code}))
}

/// Resolve e abre o que é do Rust: com terminal de todo provedor, e o Claude e o Codex sem terminal.
/// Recusa da resolução é do Rust: as dele são as do Python mais os consertos.
pub async fn decide(st: &Arc<AppState>, host: &ServerHost, mut request: SessionOpenRequest, caller: OpenCaller) -> Decision {
    // A reserva pré-aquecida é só do dono: o nome derivado que só ela segura fica para esta abertura.
    let owner = matches!(caller, OpenCaller::Owner);
    request.take_reserved_name = owner;
    let mut adopting = match request.prewarm.take() {
        Some(id) if owner => Some(super::adopt::Adopting::take(st.prewarm.clone(), &id, &mut request).await),
        _ => None,
    };
    let decision = decide_adopting(st, host, &request, &caller, adopting.as_mut()).await;
    if let Some(adopting) = adopting {
        let error = match &decision { Decision::Refused(error) => Some(error), _ => None };
        let name = match &decision { Decision::Opened(opened) => opened.body["name"].as_str().unwrap_or_default(), _ => request.name.as_deref().unwrap_or_default() };
        host.diary(vec![adopting.finish(name, error).await]);
    }
    decision
}

async fn decide_adopting(st: &Arc<AppState>, host: &ServerHost, request: &SessionOpenRequest, caller: &OpenCaller,
                         mut adopting: Option<&mut super::adopt::Adopting>) -> Decision {
    let facts_env = host.env.clone();
    let runtime = tokio::runtime::Handle::current();
    let (request_for_facts, caller_for_facts) = (request.clone(), caller.clone());
    let resolved = tokio::task::spawn_blocking(move || {
        let facts = super::facts::LocalFacts::new(facts_env, runtime);
        super::resolve_open(&request_for_facts, &caller_for_facts, &facts)
    }).await;
    let resolved = match resolved.unwrap_or_else(|_| Err(OpenError::new(500, "erro_criacao_sessao", "a resolução da abertura não terminou"))) {
        Ok(resolved) => resolved,
        Err(error) => {
            if let Some(events) = super::open::refusal_diary(crate::mods::state::random_hex(8), request.name.as_deref().unwrap_or(""), super::open::requested_provider(request), &error) { host.diary(events); }
            return Decision::Refused(error);
        }
    };
    // O spec desta resolução contra o guardado na reserva: nada resolvido nem montado de novo para ela.
    if let Some(adopting) = adopting.as_deref_mut() { adopting.compare(&resolved).await; }
    if !super::open::opens_here(&resolved.spec) { return Decision::NotOurs; }
    if let super::Account::Codex { account_id } = &resolved.spec.account { codex_preparation_warning(st, account_id).await; }
    match super::open::open_adopting(host, resolved, caller, adopting).await {
        Ok(opened) => Decision::Opened(opened),
        Err(error) => Decision::Refused(error),
    }
}

/// `_codex_require_idle_preparation`: conta Codex adicional com a sincronização fora de `ready` abre
/// assim mesmo; o estado fica no log.
async fn codex_preparation_warning(st: &AppState, account_id: &str) {
    let Ok(account) = st.accounts.resolve(crate::accounts::Provider::Codex, account_id) else { return };
    if account.is_default { return; }
    let status = match crate::accounts::http::bridge(st) {
        Ok(bridge) => st.accounts.preparation_status(&account, &bridge).await,
        Err(error) => return tracing::warn!(code = error.code, "conta Codex {account_id}: sincronização não lida antes de abrir"),
    };
    if status["status"] != "ready" {
        let issues: Vec<&str> = status["issues"].as_array().into_iter().flatten().filter_map(|i| i["code"].as_str()).collect();
        tracing::warn!("conta Codex {account_id} com sincronização {}; pendências: {issues:?}", status["status"]);
    }
}

/// A linha `api.servidor` do middleware de acesso do Python, para o pedido que o Rust atendeu
/// (o que segue ao Python ele mesmo registra).
pub(crate) fn access_line(route: &str, status: u16, ms: u64, req: &str) -> Value {
    let level = if status >= 500 { "erro" } else if status >= 400 { "aviso" } else { "ok" };
    json!({"evento": "api.servidor", "nivel": level, "detalhe": route, "codigo": status.to_string(), "ms": ms, "etapa": "cabecalhos", "req": req})
}

fn access_diary(route: &str, response: &Response, started: Instant, req: &str) {
    let line = access_line(route, response.status().as_u16(), started.elapsed().as_millis() as u64, req);
    crate::diag::record(crate::diag::diary_dir(), vec![line], "acesso");
}

/// Os avisos de saída que não chegaram ao grupo antigo, na frase que a criação devolve em `avisos`.
pub(crate) fn exit_warnings(errs: &[Value]) -> Vec<String> {
    if errs.is_empty() { return Vec::new(); }
    vec![format!("A sessão antiga saiu do grupo, mas o aviso de saída falhou: {}", crate::groups::routes::failures_text(errs))]
}

pub(crate) fn reply(error: &OpenError) -> Response {
    let status = StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    crate::session_write::json_response(status, error.body())
}

/// `POST /api/sessions`: o dono e o convidado com login abrem Claude com terminal no Rust (o do
/// convidado na pasta dele, com o dono registrado no `guests.json`); o resto segue ao Python com o
/// corpo intacto (corpo que não é pedido válido também: o 422 é o dele).
pub async fn create(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (started, diag) = (Instant::now(), crate::routes::diag_req(&req));
    let (fwd, owner) = crate::routes::gate(&st, peer, &req);
    let (mut parts, body) = req.into_parts();
    // Só o Rust diz ao Python que já olhou.
    parts.headers.remove(DECLINED);
    let Ok(bytes) = to_bytes(body, crate::session_write::BODY_LIMIT).await else {
        return crate::session_write::too_large(&parts.headers);
    };
    let guest = if owner { None } else {
        crate::auth::presented_token(&parts.headers, parts.uri.query(), &parts.method, fwd.https)
            .and_then(|token| login_guest(&AccountEnvironment::capture().claude_base, &token))
    };
    let pass = |parts: axum::http::request::Parts, bytes: axum::body::Bytes| {
        let st = st.clone();
        async move { crate::routes::pass(&st, Request::from_parts(parts, Body::from(bytes)), &fwd).await }
    };
    if !owner && guest.is_none() { return pass(parts, bytes).await; }
    let Ok(request) = SessionOpenRequest::parse(&bytes) else { return pass(parts, bytes).await };
    // O passo vale também para a criação que segue ao Python: ele não tem mais o dele.
    let _progress = super::open::Progress::track(request.name.as_deref().unwrap_or(""));
    let name = request.name.clone().unwrap_or_default();
    let caller = guest.as_ref().map_or(OpenCaller::Owner, |g| OpenCaller::Guest { root: g.root.clone() });
    match decide_detached(&st, request, caller, guest).await {
        Decision::Opened(opened) => {
            let mut response = crate::session_write::json_response(StatusCode::OK, opened.body);
            crate::routes::cors(&parts.headers, response.headers_mut());
            access_diary("POST /api/sessions", &response, started, &diag);
            response
        }
        Decision::Refused(error) => {
            let mut response = reply(&error);
            crate::routes::cors(&parts.headers, response.headers_mut());
            access_diary("POST /api/sessions", &response, started, &diag);
            response
        }
        Decision::NotOurs => {
            super::open::progress_step(&name, "criando", json!({}));
            pass(parts, bytes).await
        }
    }
}

/// `GET /api/sessions/creation-progress?name=`: o passo da criação em curso, do dono e do convidado com
/// login (as duas pontas que criam). O resto segue ao Python, que recusa.
pub async fn creation_progress(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (fwd, owner) = crate::routes::gate(&st, peer, &req);
    let guest = !owner && crate::auth::presented_token(req.headers(), req.uri().query(), req.method(), fwd.https)
        .and_then(|token| login_guest(&AccountEnvironment::capture().claude_base, &token)).is_some();
    if !owner && !guest { return crate::routes::pass(&st, req, &fwd).await; }
    let name = form_urlencoded::parse(req.uri().query().unwrap_or("").as_bytes()).find(|(k, _)| k == "name").map(|(_, v)| v.into_owned());
    let mut response = crate::session_write::json_response(StatusCode::OK, super::open::progress(name.as_deref().unwrap_or("")));
    crate::routes::cors(req.headers(), response.headers_mut());
    response
}

/// O passo do preparo do bastão: a reescrita pelo modelo (até 180 s) tem passo próprio.
pub(super) fn baton_step(asked: &Value) -> &'static str {
    if asked["resumo_por_modelo"] == true { "resumo_modelo" } else { "resumo" }
}

/// `POST /api/sessions/{name}/bastao` do dono: o Python monta e grava o dossiê (é dele) e devolve o
/// pedido de abertura da sucessora em vez de abrir; o Rust a abre com o kick-off como primeira mensagem.
pub async fn bastao(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (fwd, owner) = crate::routes::gate(&st, peer, &req);
    let (mut parts, body) = req.into_parts();
    parts.headers.remove(DECLINED);
    let Ok(bytes) = to_bytes(body, crate::session_write::BODY_LIMIT).await else {
        return crate::session_write::too_large(&parts.headers);
    };
    let headers = parts.headers.clone();
    if owner { parts.headers.insert(DECLINED, axum::http::HeaderValue::from_static("rust")); }
    let asked = serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null);
    let successor = asked["name"].as_str().unwrap_or_default().to_owned();
    let _progress = super::open::Progress::track(&successor);
    super::open::progress_step(&successor, baton_step(&asked), json!({}));
    let response = crate::routes::pass(&st, Request::from_parts(parts, Body::from(bytes)), &fwd).await;
    if !owner || response.headers().get(DECLINED).is_none_or(|v| v != "rust") { return response; }
    let prepared: Value = match to_bytes(response.into_body(), crate::session_write::BODY_LIMIT).await {
        Ok(raw) => serde_json::from_slice(&raw).unwrap_or(Value::Null),
        Err(_) => Value::Null,
    };
    let mut response = match SessionOpenRequest::parse(prepared["open"].to_string().as_bytes()) {
        Err(_) => reply(&OpenError::new(502, "erro_criacao_sessao", "o preparo do bastão não devolveu um pedido de abertura")),
        Ok(request) => match decide_detached(&st, request, OpenCaller::Owner, None).await {
            Decision::Opened(opened) => crate::session_write::json_response(StatusCode::OK, json!({"name": opened.body["name"],
                "dossie": prepared["dossie"], "texto": prepared["texto"], "kickoff": prepared["kickoff"], "aviso": prepared["aviso"],
                "avisos": opened.body["avisos"]})),
            Decision::Refused(error) => reply(&error),
            Decision::NotOurs => reply(&OpenError::new(500, "erro_criacao_sessao", "a sucessora do bastão não abre com terminal no Rust")),
        },
    };
    crate::routes::cors(&headers, response.headers_mut());
    response
}

/// `/__hangar_server/claude/<nome>/open`: o que entrou direto no Python (Connect, com o convidado e a
/// pasta dele, e o bastão por ele) abre aqui pelo mesmo caminho. `{"request": …, "guest": <id>}`; o
/// que não é do Rust volta 421 e o Python segue. O dono da sessão do convidado é gravado aqui.
pub async fn bridge(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    if !crate::workspace_routes::private_ok(&st, peer, req.headers()) { return StatusCode::NOT_FOUND.into_response(); }
    let Ok(bytes) = to_bytes(req.into_body(), crate::session_write::BODY_LIMIT).await else { return StatusCode::PAYLOAD_TOO_LARGE.into_response() };
    // Pedido torto é recusa de validação; o 421 fica só para o que o Rust não atende.
    let envelope = match serde_json::from_slice::<Value>(&bytes) {
        Ok(envelope) => envelope,
        Err(error) => return reply(&OpenError::invalid_body(&error.to_string())),
    };
    let request = match SessionOpenRequest::parse(envelope["request"].to_string().as_bytes()) {
        Ok(request) => request,
        Err(error) => return reply(&error),
    };
    let guest = match bridge_guest(&envelope, &AccountEnvironment::capture().claude_base) {
        Ok(guest) => guest,
        Err(error) => return reply(&error),
    };
    let caller = guest.as_ref().map_or(OpenCaller::Owner, |g| OpenCaller::Guest { root: g.root.clone() });
    match decide_detached(&st, request, caller, guest).await {
        Decision::Opened(opened) => crate::session_write::json_response(StatusCode::OK, json!({"info": opened.body})),
        Decision::Refused(error) => reply(&error),
        Decision::NotOurs => StatusCode::MISDIRECTED_REQUEST.into_response(),
    }
}

/// `POST /api/archive/{project}/{session_id}/resume`: a conversa do Arquivo volta numa sessão nova, pela
/// resolução da criação. Dono pela porta pública ou pelo repasse do Python (Connect).
pub async fn serve_archive_resume(st: &Arc<AppState>, project: &str, sid: &str, bytes: &[u8]) -> Response {
    let body = match super::archive::ArchiveBody::parse(bytes) {
        Ok(body) => body,
        Err(error) => return reply(&error),
    };
    match resume_archived(st, project, sid, body).await {
        Ok(body) => crate::session_write::json_response(StatusCode::OK, body),
        Err(error) => reply(&error),
    }
}

/// A retomada do Arquivo; `-` no lugar do projeto procura a conversa pelo id.
pub async fn resume_archived(st: &Arc<AppState>, project: &str, sid: &str, body: super::archive::ArchiveBody) -> Result<Value, OpenError> {
    let st = st.clone();
    let (project, sid) = (project.to_owned(), sid.to_owned());
    // Tarefa própria: cliente que desconecta não deixa a conversa movida de conta sem a sessão.
    tokio::spawn(async move {
        let host = ServerHost::new(st.clone()).await;
        let live = host.live_sessions().await?;
        let (env, runtime, server_projects) = (host.env.clone(), tokio::runtime::Handle::current(), host.server_projects());
        let (p, s) = (project.clone(), sid.clone());
        let planned = tokio::task::spawn_blocking(move || {
            let facts = super::facts::LocalFacts::new(env.clone(), runtime);
            let config_dirs = super::OpenFacts::config_dirs(&facts)?;
            let w = super::archive::Where { home: &env.home, env: &env.base, server_projects: &server_projects, config_dirs: &config_dirs, live: &live };
            let plan = super::archive::plan(&w, &facts, &p, &s, body)?;
            let resolved = super::resolve_open(&plan.request, &OpenCaller::Owner, &facts)?;
            Ok::<_, OpenError>((plan, resolved))
        }).await.unwrap_or_else(|_| Err(OpenError::new(500, "erro_criacao_sessao", "a resolução da retomada não terminou")));
        let (plan, resolved) = planned?;
        let _progress = super::open::Progress::track(&resolved.spec.name);
        let project = plan.project.clone();
        super::archive::open(&host, plan, resolved, &project, &sid).await
    }).await.unwrap_or_else(|_| Err(OpenError::new(500, "erro_criacao_sessao", "a retomada não terminou")))
}

/// A rota pública do Arquivo: só o dono; o resto segue ao Python, que recusa.
pub async fn archive_resume(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>,
                            axum::extract::Path((project, sid)): axum::extract::Path<(String, String)>, req: Request) -> Response {
    let (started, diag) = (Instant::now(), crate::routes::diag_req(&req));
    let (fwd, owner) = crate::routes::gate(&st, peer, &req);
    if !owner { return crate::routes::pass(&st, req, &fwd).await; }
    let headers = req.headers().clone();
    let Ok(bytes) = to_bytes(req.into_body(), crate::session_write::BODY_LIMIT).await else {
        return crate::session_write::too_large(&headers);
    };
    let mut response = serve_archive_resume(&st, &project, &sid, &bytes).await;
    crate::routes::cors(&headers, response.headers_mut());
    access_diary("POST /api/archive/{project}/{session_id}/resume", &response, started, &diag);
    response
}

#[cfg(test)]
mod tests {
    /// SF7: o aviso de saída que não chegou ao grupo antigo vira aviso da criação, com quem e por quê.
    #[test]
    fn a_group_exit_that_did_not_reach_everyone_is_a_creation_warning() {
        assert!(super::exit_warnings(&[]).is_empty());
        let errs = [serde_json::json!({"sessao": "par", "erro": {"code": "erro_peer_nao_avisado", "msg": "fora do ar"}})];
        let warnings = super::exit_warnings(&errs);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("par"), "{warnings:?}");
    }
}
