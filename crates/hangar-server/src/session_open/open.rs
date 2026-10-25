//! Abrir uma sessão Claude com terminal pelo spec: o que o `create_session` e o ramo Claude do
//! `registry.create` do Python faziam em volta do pane, na mesma ordem, com os consertos da Task 4.
//! Os efeitos de fora (processos, pontes, diário) passam pelo `OpenHost`: o servidor, o comando de
//! terminal com o servidor fora e o harness do contrato implementam cada um o seu.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures_util::future::BoxFuture;
use serde_json::{Value, json};

use super::agents::{self, Conversation};
use super::claude::{self, PluginBridge};
use super::terminal::{self, Mux, MuxOutput, PaneSpec, TerminalEnv};
use super::{Account, AgentProvider, OpenCaller, OpenError, PermissionMode, Resolved, SessionSpec, Surface};


/// O que a sessão que acabou de nascer avisa a quem guarda estado do nome.
#[derive(Clone, Debug)]
/// `codex_terminal`: a pasta da conta do terminal Codex que nasceu (o nascimento conta como uso dela).
/// `restored`: a mesma sessão voltando depois do reboot; o par e o grupo do nome são dela.
pub struct Born { pub name: String, pub provider: AgentProvider, pub jsonl: Option<String>, pub remember_provider: bool, pub codex_terminal: Option<String>,
    pub restored: bool }

pub trait OpenHost: Send + Sync {
    /// A pasta pessoal do dono (registros, `~/.claude.json`, `~/.hangar`).
    fn home(&self) -> &Path;
    /// O ambiente do servidor (PATH, `CP_*`, `CLAUDE_CONFIG_DIR`).
    fn env(&self) -> &BTreeMap<String, String>;
    fn terminal(&self) -> &TerminalEnv;
    fn mux(&self) -> &dyn Mux;
    fn bridge(&self) -> Option<PluginBridge>;
    /// `projects` da conta do servidor.
    fn server_projects(&self) -> PathBuf;
    /// A pasta `backend/` do Hangar (a venv e o `skill_bridge.py`).
    fn backend_dir(&self) -> Option<PathBuf>;
    fn run(&self, argv: Vec<String>, timeout: Duration) -> BoxFuture<'_, MuxOutput>;
    /// A operação `prepare` das escolhas de plugins e skills; devolve o `{"settings": …}`.
    fn customizations(&self, args: Value) -> BoxFuture<'_, Result<Value, OpenError>>;
    /// O grupo da sessão antiga de mesmo nome sai antes de qualquer efeito.
    /// Os avisos de saída que não chegaram ao grupo antigo vão aos `avisos` da criação.
    fn leave_old_group<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Vec<String>, OpenError>>;
    /// O nome reusado não herda nada da morta (fila, `then`, caches); antes do pane, porque depois
    /// dele o runtime já adota a sessão.
    fn release_name<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), OpenError>>;
    /// Depois do pane: grupo e par externo do nome antigo, transcript semeado, lista. Devolve avisos.
    fn born<'a>(&'a self, born: &'a Born) -> BoxFuture<'a, Vec<String>>;
    /// O Codex sem terminal que acabou de ganhar sidecar sobe agora (falha só avisa: o próximo envio
    /// abre) e recebe a primeira mensagem. Devolve avisos.
    fn warm_codex<'a>(&'a self, name: &'a str, initial_prompt: Option<&'a str>) -> BoxFuture<'a, Vec<String>>;
    fn diary(&self, events: Vec<Value>);
    /// Os eventos `conta.reconciliar.*`: o servidor grava no diário dele; o comando de terminal, que não
    /// grava os da criação, no arquivo dos comandos de conta.
    fn account_diary(&self, events: Vec<Value>);
    /// Quanto a abertura espera a reconciliação da conta; os testes encurtam.
    fn reconcile_limit(&self) -> Duration { RECONCILE_LIMIT }
    /// Id da conversa (`--session-id`) e chave da ponte do plugin, novos a cada vida.
    fn new_session_id(&self) -> String;
    fn new_bridge_key(&self) -> String;
    /// As sessões vivas e o transcript de cada uma (nome, caminho): retomar uma conversa que já roda
    /// poria dois processos no mesmo arquivo. Lista indisponível é erro, nunca "nenhuma".
    fn live_sessions(&self) -> BoxFuture<'_, Result<Vec<(String, String)>, OpenError>>;
    /// A sessão sem terminal recém-criada sobe em segundo plano (os hooks de SessionStart rodam enquanto
    /// a pessoa digita) e entrega o que está na fila. Não espera.
    fn wake(&self, name: &str);
    /// As reservas pré-aquecidas deste servidor; `None` onde não há (comando de terminal, harness).
    fn prewarm(&self) -> Option<&crate::prewarm::Store>;
}

/// Leitura presa (FIFO, montagem travada) ou trava que não solta recusa a abertura em vez de segurá-la
/// para sempre.
pub(crate) const RECONCILE_LIMIT: Duration = Duration::from_secs(120);

/// UUID v4 de 16 bytes aleatórios em hexadecimal.
pub fn uuid4(hex: &str) -> String {
    let mut h: Vec<char> = hex.chars().take(32).collect();
    h[12] = '4';
    h[16] = ['8', '9', 'a', 'b'][h[16].to_digit(16).unwrap_or(0) as usize % 4];
    let s: String = h.into_iter().collect();
    format!("{}-{}-{}-{}-{}", &s[..8], &s[8..12], &s[12..16], &s[16..20], &s[20..32])
}

/// Um id de conversa novo.
pub(crate) fn new_session_id() -> String { uuid4(&crate::mods::state::random_hex(16)) }

/// Os eventos do diário de uma criação, na ordem do `registry.create` (`@diag.rastrear`).
struct Diary { events: Vec<Value>, op: String, started: Instant, name: String, provider: &'static str }

impl Diary {
    fn new(op: String, name: &str, provider: &'static str) -> Self {
        Self { events: vec![json!({"evento": "sessao.criar.inicio", "nivel": "ok", "operacao": op})], op, started: Instant::now(), name: name.into(), provider }
    }
    fn session(&self) -> Value { if self.name.is_empty() { Value::Null } else { Value::from(self.name.clone()) } }
    fn step(&mut self, etapa: &str) {
        let mut e = json!({"evento": "sessao.criar_etapa", "nivel": "ok", "sessao": self.session(), "provider": self.provider, "etapa": etapa, "operacao": self.op});
        if e["sessao"].is_null() { e.as_object_mut().unwrap().remove("sessao"); }
        self.events.push(e);
    }
    fn refused(&mut self, level: &str, code: &str, etapa: Option<&str>, detail: Option<&str>) {
        let mut e = json!({"evento": "sessao.criar_recusada", "nivel": level, "sessao": self.session(), "provider": self.provider, "operacao": self.op});
        match (etapa, detail) {
            // Falha de lançamento leva a etapa, o código e o stderr resumido.
            (Some(etapa), detail) => { e["etapa"] = etapa.into(); e["codigo"] = code.into(); e["detalhe"] = detail.unwrap_or("").into(); }
            (None, _) => e["detalhe"] = code.into(),
        }
        self.events.push(e);
    }
    /// A pasta não confiada deixa a sessão presa no "trust this folder?": a criação segue, e o diário conta.
    fn trust_failed(&mut self, detail: &str) {
        self.events.push(json!({"evento": "sessao.confiar_pasta_falhou", "nivel": "aviso", "sessao": self.session(), "provider": self.provider,
            "etapa": "confiar_pasta", "detalhe": detail, "operacao": self.op}));
    }
    fn end(mut self, error: Option<&OpenError>) -> Vec<Value> {
        let ms = self.started.elapsed().as_millis() as u64;
        self.events.push(match error {
            None => json!({"evento": "sessao.criar.fim", "nivel": "ok", "codigo": "retornou", "ms": ms, "operacao": self.op}),
            Some(e) => json!({"evento": "sessao.criar.fim", "nivel": "erro", "codigo": "excecao", "ms": ms,
                "erro_tipo": e.code.unwrap_or("recusa"), "operacao": self.op}),
        });
        self.events
    }
}

/// Recusas que o Python dava dentro do `registry.create` (depois do `sessao.criar.inicio`): o
/// diário as registra igual. As de antes (validação da API) não deixam evento de criação.
pub fn refusal_diary(op: String, name: &str, provider: &'static str, error: &OpenError) -> Option<Vec<Value>> {
    let in_use = error.code == Some("erro_nome_em_uso");
    let invalid = error.code == Some("erro_criacao_sessao") && error.msg.starts_with("nome invalido");
    let agent = error.code == Some("erro_criacao_sessao")
        && ["sessao sem terminal so vale para provider claude ou codex", "Nome de perfil OMP inválido", "sessao sem terminal nao aceita read_only",
            "permission_mode: use um de Ask for approval, Approve for me, Full Access"].contains(&error.msg.as_str());
    if !(in_use || invalid || agent) { return None; }
    let mut diary = Diary::new(op, if invalid { "" } else { name }, provider);
    diary.step("validar");
    if in_use { diary.refused("aviso", "nome_ja_em_uso", None, None); }
    Some(diary.end(Some(error)))
}

/// `resolve_scan_roots`: o `scan_roots` do `runtime-config.json`, senão `CP_SCAN_ROOTS`, senão o
/// padrão do sistema; cada um com `~` expandido e o caminho real, só as pastas que existem.
pub fn scan_roots(home: &Path, env: &BTreeMap<String, String>, runtime: &Value) -> Vec<PathBuf> {
    let raw = runtime["scan_roots"].as_str().filter(|r| !r.trim().is_empty()).map(str::to_owned)
        .or_else(|| env.get("CP_SCAN_ROOTS").cloned())
        .unwrap_or_else(|| (if cfg!(windows) { "~,~/Documents,~/Desktop" } else { "~/pessoal,~/sistemas" }).into());
    let mut out: Vec<PathBuf> = Vec::new();
    for entry in raw.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let path = match entry.strip_prefix('~') {
            Some("") => home.to_owned(),
            Some(rest) if rest.starts_with(['/', '\\']) => home.join(&rest[1..]),
            _ => PathBuf::from(entry),
        };
        let Ok(real) = path.canonicalize() else { continue };
        if real.is_dir() && !out.contains(&real) { out.push(real); }
    }
    out
}

/// `_allowed_scan_root`: a pasta da worktree mora numa raiz liberada (a do convidado, para ele).
fn allowed_root(host: &dyn OpenHost, caller: &OpenCaller, cwd: &str, runtime: &Value) -> Result<PathBuf, OpenError> {
    let roots = match caller {
        OpenCaller::Guest { root } => vec![root.clone()],
        OpenCaller::Owner => scan_roots(host.home(), host.env(), runtime),
    };
    let target = Path::new(cwd).canonicalize().unwrap_or_else(|_| PathBuf::from(cwd));
    roots.into_iter().find(|root| target.starts_with(root))
        .ok_or_else(|| OpenError::new(403, "erro_criacao_sessao", "root not allowed"))
}

fn creation(msg: impl Into<String>) -> OpenError { OpenError::new(409, "erro_criacao_sessao", msg) }

fn name_in_use() -> OpenError { OpenError::new(409, "erro_nome_em_uso", "ja existe uma sessao com esse nome") }

/// O tmux que não respondeu: não dá para saber se o nome está livre.
pub(crate) fn mux_unavailable(detail: &str) -> OpenError {
    OpenError::new(503, "erro_mux_indisponivel", "o tmux não respondeu — a lista de sessões está indisponível")
        .with_params(json!({"detalhe": detail}))
}

/// O pane pode ter nascido sem o multiplexador confirmar: nada em volta dele é desfeito.
const UNCERTAIN: &str = "session_open_uncertain";

/// O passo de cada criação em curso, pelo nome sanitizado da sessão nova (`GET
/// /api/sessions/creation-progress`): a tela de criar consulta enquanto espera e sabe que não travou.
static PROGRESS: std::sync::Mutex<BTreeMap<String, Value>> = std::sync::Mutex::new(BTreeMap::new());

fn progress_map() -> std::sync::MutexGuard<'static, BTreeMap<String, Value>> {
    PROGRESS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// O registro do passo de uma criação: só quem o abriu o apaga (o bastão acompanha a criação por dentro dele).
pub struct Progress(Option<String>);

impl Progress {
    pub fn track(name: &str) -> Self {
        let key = crate::list::discover_other::sanitize_session_name(name);
        let mut map = progress_map();
        if key.is_empty() || map.contains_key(&key) { return Self(None); }
        map.insert(key.clone(), json!({"step": "preparando", "params": {}}));
        Self(Some(key))
    }
}

impl Drop for Progress {
    fn drop(&mut self) { if let Some(key) = &self.0 { progress_map().remove(key); } }
}

/// O passo de agora, se há criação em curso com esse nome.
pub fn progress_step(name: &str, step: &str, params: Value) {
    if let Some(entry) = progress_map().get_mut(&crate::list::discover_other::sanitize_session_name(name)) {
        *entry = json!({"step": step, "params": params});
    }
}

pub fn progress(name: &str) -> Value {
    progress_map().get(&crate::list::discover_other::sanitize_session_name(name)).cloned().unwrap_or_else(|| json!({"step": null, "params": {}}))
}

/// Nomes com abertura em curso neste processo, por casa (`home`); `true` = quem segura é uma reserva
/// pré-aquecida.
static OPENING: std::sync::Mutex<BTreeMap<(PathBuf, String), bool>> = std::sync::Mutex::new(BTreeMap::new());

fn opening_names() -> std::sync::MutexGuard<'static, BTreeMap<(PathBuf, String), bool>> {
    OPENING.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A reserva do nome enquanto a abertura corre: a segunda abertura do mesmo nome é recusada. A vazia
/// (`Default`) não segura nada.
#[derive(Default)]
pub(crate) struct NameClaim(Option<(PathBuf, String)>);

impl NameClaim {
    pub(crate) fn take(home: &Path, name: &str) -> Result<Self, OpenError> { Self::hold(home, name, false) }

    /// O nome da reserva pré-aquecida: a derivação do dono o trata como livre e a abertura descarta a reserva.
    pub(crate) fn reserve(home: &Path, name: &str) -> Result<Self, OpenError> { Self::hold(home, name, true) }

    fn hold(home: &Path, name: &str, reserve: bool) -> Result<Self, OpenError> {
        let key = (home.to_owned(), name.to_owned());
        let mut names = opening_names();
        if names.contains_key(&key) { return Err(name_in_use()); }
        names.insert(key.clone(), reserve);
        Ok(Self(Some(key)))
    }

    /// O nome da reserva adotada (ou descartada no meio da abertura) passa a ser de uma abertura comum.
    pub(crate) fn promote(&self) {
        if let Some(key) = &self.0 && let Some(reserve) = opening_names().get_mut(key) { *reserve = false; }
    }

    /// O nome segurado, se há.
    pub(crate) fn name(&self) -> Option<&str> { self.0.as_ref().map(|(_, name)| name.as_str()) }

    /// Há abertura (ou reserva) em curso com esse nome, em qualquer casa deste processo.
    pub(crate) fn opening(name: &str) -> bool { opening_names().keys().any(|(_, n)| n == name) }

    /// Como `opening`, sem contar as reservas pré-aquecidas.
    pub(crate) fn opening_without_reserves(name: &str) -> bool {
        opening_names().iter().any(|((_, n), reserve)| n == name && !reserve)
    }
}

impl Drop for NameClaim {
    fn drop(&mut self) { if let Some(key) = self.0.take() { opening_names().remove(&key); } }
}

/// `orq_readonly.prepare`: o envelope `bwrap` que deixa o código só leitura, provado com `/bin/true`.
async fn read_only_prefix(host: &dyn OpenHost, cwd: &str, config_dir: Option<&str>) -> Result<Vec<String>, OpenError> {
    read_only_envelope(host.env(), host.home(), cwd, config_dir, &|argv, timeout| host.run(argv, timeout)).await
}

/// O envelope do só leitura com o ambiente, a casa e quem roda programas: a abertura e o relançamento do
/// Codex (que o reaplica) usam o mesmo.
pub(crate) async fn read_only_envelope<'a>(env: &BTreeMap<String, String>, home: &Path, cwd: &str, config_dir: Option<&str>,
    run: &'a (dyn Fn(Vec<String>, Duration) -> BoxFuture<'a, MuxOutput> + Sync)) -> Result<Vec<String>, OpenError> {
    let refuse = |msg: String| OpenError::new(400, "erro_criacao_sessao", msg);
    if !cfg!(target_os = "linux") { return Err(refuse("read-only exige Linux com bubblewrap (bwrap)".into())); }
    let bwrap = crate::runtime::process::resolve_program("bwrap", env)
        .ok_or_else(|| refuse("read-only exige bubblewrap: instale bwrap antes de criar a sessão".into()))?;
    let root = Path::new(cwd).canonicalize().map_err(|e| refuse(format!("read-only não pôde preparar a proteção: {e}")))?;
    let mut paths = vec![root.clone()];
    let git = run(["git", "-C", &root.to_string_lossy(), "rev-parse", "--path-format=absolute", "--git-dir", "--git-common-dir",
        "--show-toplevel"].map(str::to_owned).into(), Duration::from_secs(5)).await;
    if git.code == Some(0) {
        for line in git.stdout.lines().filter(|l| !l.is_empty()) {
            let real = Path::new(line).canonicalize().map_err(|e| refuse(format!("read-only não pôde preparar a proteção: {e}")))?;
            if !paths.contains(&real) { paths.push(real); }
        }
    } else if root.ancestors().any(|p| p.join(".git").exists()) {
        return Err(refuse("read-only não conseguiu localizar os metadados Git".into()));
    }
    let home_real = home.canonicalize().unwrap_or_else(|_| home.to_owned());
    if paths.iter().any(|p| home_real.starts_with(p)) {
        return Err(refuse("read-only não pode proteger a HOME inteira ou um diretório acima dela".into()));
    }
    let mut runtimes: Vec<PathBuf> = [".claude", ".codex", ".pi", ".omp", ".kimi-code", ".hangar"].iter().map(|n| home.join(n)).collect();
    runtimes.extend(config_dir.map(PathBuf::from));
    runtimes.extend(["CLAUDE_CONFIG_DIR", "CODEX_HOME", "PI_CODING_AGENT_DIR", "PI_CODING_AGENT_SESSION_DIR", "PI_CONFIG_DIR", "KIMI_CODE_HOME"]
        .iter().filter_map(|k| env.get(*k).filter(|v| !v.is_empty())).map(PathBuf::from));
    let resolved = |p: &PathBuf| p.canonicalize().unwrap_or_else(|_| p.clone());
    if runtimes.iter().any(|r| paths.iter().any(|p| resolved(r).starts_with(p))) {
        return Err(refuse("read-only recusado: o código protegido contém o runtime de um agente".into()));
    }
    paths.sort_by_key(|p| (p.components().count(), p.to_string_lossy().into_owned()));
    let mut argv: Vec<String> = [bwrap.to_string_lossy().as_ref(), "--bind", "/", "/", "--dev-bind", "/dev", "/dev", "--setenv", "HANGAR_ORQ_READ_ONLY", "1"]
        .map(str::to_owned).into();
    for p in &paths { argv.extend(["--ro-bind".into(), p.to_string_lossy().into_owned(), p.to_string_lossy().into_owned()]); }
    argv.push("--".into());
    let mut probe = argv.clone();
    probe.push("/bin/true".into());
    let out = run(probe, Duration::from_secs(5)).await;
    match out.code {
        Some(0) => Ok(argv),
        None => Err(refuse(format!("read-only não pôde preparar a proteção: {}", out.stderr.trim()))),
        Some(_) => Err(refuse(format!("read-only recusado pelo bubblewrap: {}", out.stderr.trim()))),
    }
}

/// O corpo de `CreatedSessionInfo` da sessão que acabou de nascer.
fn created_body(spec: &SessionSpec, jsonl: Option<&str>, warnings: Vec<String>, route: Option<&claude::EngineRoute>, codex_home: Option<&str>, provider_source: Option<&str>) -> Value {
    let engine = match &spec.account { Account::Engine { name, .. } => Value::from(name.clone()), _ => Value::Null };
    let headless = spec.surface == super::Surface::Headless;
    // O Codex sem terminal é rastreado desde o sidecar; o com terminal só depois de a TUI abrir a thread.
    let conta = match (route, codex_home) {
        (Some(route), _) => Some(route.credential_id.clone()),
        (None, Some(home)) if headless => Some(format!("codex:{home}")),
        _ => None,
    };
    // Worktree ligada: o `.git` dela é arquivo. A branch que já era a da pasta fica na pasta.
    let linked = spec.worktree.is_some() && Path::new(&spec.cwd).join(".git").is_file();
    let head = json!({
        "name": spec.name, "lifecycle_id": null, "transfer_id": null, "transfer_phase": null, "cwd": spec.cwd,
        "jsonl": jsonl, "provider": spec.provider.name(), "headless": headless, "engine": engine, "engine_account": route.map(|r| r.account.clone()),
        "codex_home": codex_home, "codex_service_tier": null, "conta": conta, "state": "idle", "last_activity": null,
        "last_reply": null, "last_reply_at": null, "tracked": jsonl.is_some() || (headless && codex_home.is_some()),
        "branch": spec.worktree.as_ref().map(|w| w.branch.clone()),
        "worktree": linked,
        "worktree_path": linked.then(|| spec.cwd.clone()),
        "worktree_gone": false, "git_cwd": null, "git_dirty": null, "git_ahead": null, "git_behind": null,
    });
    let rest = json!({
        "git_added": null, "git_removed": null, "avisos": warnings, "label": null, "startup_steps": [], "question": null,
        "pending_questions": 0, "options": null, "stalled": false, "problema": null, "limited": false, "limit_reset": null,
        "shared": false, "guest_kind": null, "owner": null, "then_target": null, "status_line": null, "context": null,
        "model": null, "pair_peers": null, "pair_external": null, "pair_gid": null, "pair_task": null, "orq_arbiter": null,
        "loop_status": null, "loop_iter": null, "loop_max": null, "plan_name": null, "plan_task": null,
        "plan_task_total": null, "plan_done": null, "plan_total": null, "plan_complete": null, "plan_tasks": null,
        "plan_hidden": null, "config_dir": spec.account.config_dir(), "account_source": spec.account_source.wire(),
        "provider_source": provider_source,
    });
    let mut body = head;
    body.as_object_mut().unwrap().extend(rest.as_object().unwrap().clone());
    body
}

/// A worktree que esta abertura criou, para desfazer se ela falhar (ou se o registro do convidado falhar).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorktreeUndo { pub source: String, pub path: String, pub branch: String, pub branch_created: bool }

impl WorktreeUndo {
    /// O `git` roda fora do runtime.
    pub async fn undo(self) -> Result<(), OpenError> {
        let failed = |detail: String| OpenError::new(500, "erro_criacao_sessao", format!("falha ao desfazer a worktree: {detail}"));
        tokio::task::spawn_blocking(move || {
            let made = hangar_workspace::git::Worktree { path: self.path.clone(), created: true, branch_created: self.branch_created };
            hangar_workspace::git::undo_worktree(Path::new(&self.source), &made, &self.branch)
                .map_err(|e| e.detail.as_str().map_or_else(|| e.detail.to_string(), str::to_owned))
        }).await.map_err(|e| failed(e.to_string()))?.map_err(failed)
    }
}

/// O que a abertura devolve: o corpo da resposta e a worktree criada (quem registra o convidado a
/// desfaz se o registro falhar).
pub struct Opened { pub body: Value, pub undo: Option<WorktreeUndo>, pub key: String }

/// O provedor pedido, para o diário das recusas de antes da resolução.
pub fn requested_provider(request: &super::SessionOpenRequest) -> &'static str {
    request.provider.as_deref().and_then(AgentProvider::parse).map_or("claude", AgentProvider::name)
}

/// O que esta abertura atende: sessão com terminal de todo provedor, e o Claude e o Codex sem terminal.
pub fn opens_here(spec: &SessionSpec) -> bool {
    spec.surface == Surface::Terminal || matches!(spec.provider, AgentProvider::Claude | AgentProvider::Codex)
}

/// Abre a sessão do spec resolvido: com terminal de todo provedor, ou o Claude e o Codex sem terminal.
pub async fn open_session(host: &dyn OpenHost, resolved: Resolved, caller: &OpenCaller) -> Result<Opened, OpenError> {
    open_adopting(host, resolved, caller, None).await
}

/// A reserva pré-aquecida que segura o nome sai antes: a pessoa não a vê, e ela nunca vira "nome em uso".
async fn claim_name(host: &dyn OpenHost, name: &str) -> Result<NameClaim, OpenError> {
    if let Some(store) = host.prewarm() { store.discard_name(name, "nome_pedido").await; }
    NameClaim::take(host.home(), name)
}

/// `open_session` com a reserva pré-aquecida tirada para este pedido: o Claude sem terminal a adota se
/// ela ainda cabe; senão ela sai e a abertura segue como sempre. Quem chama fecha com `Adopting::finish`.
pub async fn open_adopting(host: &dyn OpenHost, resolved: Resolved, caller: &OpenCaller, mut adopting: Option<&mut super::adopt::Adopting>)
    -> Result<Opened, OpenError> {
    let Resolved { mut spec, mut warnings, provider_source, initial_prompt, remember_provider, previous_non_plan, resume, restore } = resolved;
    let _claim = match adopting.as_deref_mut().and_then(|a| a.claim_for(&spec.name)) {
        Some(claim) => claim,
        None => claim_name(host, &spec.name).await?,
    };
    let runtime =crate::runtime::codex_launch::runtime_config(&crate::accounts::environment::AccountEnvironment::from_map(host.env().clone()));
    let codex_home = match &spec.account {
        Account::Codex { account_id } => Some(super::codex::home(host.env(), account_id)?),
        _ => None,
    };
    // Antes de qualquer efeito; a worktree troca a pasta e o envelope é refeito sobre ela.
    let read_only_home = spec.account.config_dir().map(str::to_owned).or_else(|| codex_home.clone());
    let mut read_only = match spec.read_only {
        true => Some(read_only_prefix(host, &spec.cwd, read_only_home.as_deref()).await?),
        false => None,
    };
    let engine = matches!(spec.account, Account::Engine { .. });
    // Modelo de motor deixado no `settings.json` faria a sessão Anthropic nascer nele.
    if !engine && spec.provider == AgentProvider::Claude { warnings.extend(claude::drop_foreign_model(host.home(), spec.account.config_dir())); }
    let mut undo = None;
    if let Some(wt) = spec.worktree.clone() {
        let root = allowed_root(host, caller, &spec.cwd, &runtime)?;
        let stem = crate::list::discover_other::sanitize_session_name(&wt.branch);
        let stem = if stem.is_empty() { "worktree".to_owned() } else { stem };
        let source = spec.cwd.clone();
        let home = host.home().to_owned();
        // A pasta das worktrees é a configurada neste servidor, como na rota de worktrees.
        let made = tokio::task::spawn_blocking(move || {
            let location = hangar_workspace::worktree_settings::WorktreeSettings::load_at(&home.join(".hangar/worktree-settings.json"))?
                .location_path(&home)?;
            hangar_workspace::git::create_branch_worktree(Path::new(&source), &wt.branch, &stem, &root, wt.new_branch, wt.base.as_deref(), &location)
        }).await.map_err(|_| creation("a criação da worktree não terminou"))?
            .map_err(|e| OpenError::new(e.status, "erro_criacao_sessao", e.detail.as_str().map_or_else(|| e.detail.to_string(), str::to_owned)))?;
        if made.created {
            undo = Some(WorktreeUndo { source: spec.cwd.clone(), path: made.path.clone(), branch: spec.worktree.as_ref().unwrap().branch.clone(),
                branch_created: made.branch_created });
        }
        spec.cwd = made.path.clone();
        if let Some(w) = spec.worktree.as_mut() { w.path = Some(made.path); }
        if read_only.is_some() {
            match read_only_prefix(host, &spec.cwd, read_only_home.as_deref()).await {
                Ok(prefix) => read_only = Some(prefix),
                Err(error) => { if let Some(undo) = undo { undo.undo().await?; } return Err(error); }
            }
        }
    }
    match launch(host, &mut spec, read_only, warnings, initial_prompt.as_deref(), remember_provider, previous_non_plan, &runtime, codex_home.as_deref(),
                 resume.as_deref(), restore, adopting).await {
        Ok((jsonl, warnings, route)) => {
            // O Codex sem terminal já subiu dentro da abertura (`warm_codex`).
            if spec.surface == Surface::Headless && spec.provider == AgentProvider::Claude { host.wake(&spec.name); }
            Ok(Opened { body: created_body(&spec, jsonl.as_deref(), warnings, route.as_ref(), codex_home.as_deref(), provider_source), undo, key: spec.key.clone() })
        }
        // Incerta, a sessão pode estar de pé na worktree: desfazer apagaria a pasta debaixo dela.
        Err(error) => {
            if let Some(undo) = undo && error.code != Some(UNCERTAIN) { undo.undo().await?; }
            Err(error)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn launch(host: &dyn OpenHost, spec: &mut SessionSpec, read_only: Option<Vec<String>>, warnings: Vec<String>, initial_prompt: Option<&str>,
                remember_provider: bool, previous_non_plan: Option<PermissionMode>, runtime: &Value, codex_home: Option<&str>,
                resume: Option<&str>, restore: bool, adopting: Option<&mut super::adopt::Adopting>)
                -> Result<(Option<String>, Vec<String>, Option<claude::EngineRoute>), OpenError> {
    // `ciclo_conta`: a guarda compartilhada da conta vale da reconciliação ao pane, e a exclusão dela
    // (exclusiva) espera a sessão que está subindo. Também na adoção: o `--prep` pode mudar a
    // configuração que a reserva leu, e a guarda segura a conta até o sidecar existir.
    let (_guard, mut avisos) = prepare_account(host, spec, codex_home).await?;
    avisos.extend(warnings);
    progress_step(&spec.name, "criando", json!({}));
    let mut diary = Diary::new(crate::mods::state::random_hex(8), &spec.name, spec.provider.name());
    let result = match (spec.surface, codex_home) {
        (Surface::Terminal, _) => terminal_life(host, spec, read_only.as_deref(), initial_prompt, &mut diary, remember_provider, runtime, codex_home, resume, restore).await,
        (Surface::Headless, Some(home)) =>
            codex_headless_life(host, spec, home, initial_prompt, &mut diary, remember_provider, resume).await.map(|w| (None, w, None)),
        (Surface::Headless, None) => headless_life(host, spec, previous_non_plan, initial_prompt, &mut diary, remember_provider, resume, adopting).await,
    };
    let events = diary.end(result.as_ref().err());
    host.diary(events);
    let (jsonl, mut born_warnings, route) = result?;
    avisos.append(&mut born_warnings);
    Ok((jsonl, avisos, route))
}

/// A conta que a sessão lê, como a trava das contas a identifica.
pub(crate) fn account_key(host: &dyn OpenHost, spec: &SessionSpec, codex_home: Option<&str>) -> Result<crate::accounts::AccountKey, OpenError> {
    let account_home = spec.account.config_dir().map(PathBuf::from)
        .or_else(|| host.env().get("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()).map(PathBuf::from))
        .unwrap_or_else(|| host.home().join(".claude"));
    match codex_home {
        Some(home) => crate::accounts::AccountKey::new(crate::accounts::Provider::Codex, Path::new(home)),
        None => crate::accounts::AccountKey::new(crate::accounts::Provider::Claude, &account_home),
    }.map_err(|e| creation(format!("conta inválida: {e}")))
}

/// O começo de toda abertura e da reserva pré-aquecida: login da conta do Hangar, guarda compartilhada
/// da conta (vale até quem chamou soltá-la) e a reconciliação da conta, aqui mesmo. Devolve a guarda
/// e os avisos da reconciliação.
pub(crate) async fn prepare_account(host: &dyn OpenHost, spec: &SessionSpec, codex_home: Option<&str>)
    -> Result<(Option<crate::accounts::AccountGuard>, Vec<String>), OpenError> {
    let config_dir = spec.account.config_dir().map(str::to_owned);
    let locks = crate::accounts::AccountLocks::new(host.home().join(".hangar").join("account-locks"));
    let key = account_key(host, spec, codex_home)?;
    let hangar_account = config_dir.as_deref().filter(|dir| claude::hangar_account(Path::new(dir)).is_some());
    // O Kimi não lê a conta Claude: nem trava nem reconciliação.
    let reads_account = spec.provider != AgentProvider::Kimi;
    let hangar_account = hangar_account.filter(|_| reads_account);
    // Sem login o Claude sobe e fica mudo: recusa antes de subir qualquer coisa.
    if spec.provider == AgentProvider::Claude && let Some(dir) = hangar_account
        && !crate::accounts::claude_refresh::has_login(Path::new(dir)) {
        let label = claude::hangar_account(Path::new(dir)).unwrap_or_default();
        return Err(OpenError { params: json!({"conta": label}),
            ..OpenError::new(409, "erro_conta_sem_login", format!("a conta {label} não está logada; faça login nela antes de abrir uma sessão")) });
    }
    let mut guard = match reads_account {
        true => Some(crate::accounts::claude_refresh::acquire_for_session(&locks, &key, Instant::now() + Duration::from_secs(5)).await
            .map_err(|e| match e {
                crate::accounts::LockError::Busy => OpenError::new(409, "account_busy", "a conta está ocupada por outra operação"),
                crate::accounts::LockError::Io(e) => creation(format!("trava da conta: {e}")),
            })?),
        false => None,
    };
    // A conta do Hangar se reconcilia antes de abrir; os avisos dela vêm antes dos da abertura.
    let mut avisos = Vec::new();
    if let Some(dir) = hangar_account {
        // Conferida de novo sob a guarda: a exclusão pode ter passado antes dela.
        let account = claude::hangar_account(Path::new(dir))
            .ok_or_else(|| OpenError::plain(404, format!("{dir} não é uma conta criada pelo hangar")))?;
        progress_step(&spec.name, "conta", json!({"conta": account}));
        use crate::accounts::claude_config::{Outcome, Wait, late_outcome, reconcile_dir, refused};
        // A pasta conferida acima, não `~/.claude-<nome>`: a conta pode morar fora do HOME.
        let (home, dir, project) = (host.home().to_owned(), PathBuf::from(dir), hangar_workspace::worktrees::sanitize_cwd(&spec.cwd));
        let limit = host.reconcile_limit();
        let wait = Wait::within(limit);
        // As travas esperam só até o prazo, e a guarda vai com a tarefa: a recusa no limite não deixa
        // reconciliação rodando sem ela (a exclusão da conta esperaria) nem segurando o runtime.
        let held = guard.take();
        let task = tokio::task::spawn_blocking(move || (held, reconcile_dir(&home, &dir, Some(&project), wait)));
        let outcome = match tokio::time::timeout(limit, task).await {
            Ok(Ok((held, outcome))) => {
                guard = held;
                outcome
            }
            Ok(Err(e)) => Outcome::failed(refused(format!("a reconciliação parou no meio: {e}"))),
            Err(_) => late_outcome(limit),
        };
        host.account_diary(outcome.diary);
        avisos = claude::account_result(&account, outcome.result)?;
    }
    Ok((guard, avisos))
}

/// Com motor: o `hangar-engine` no PATH (sem ele a sessão nasce e morre calada) e a conta ChatGPT fixa,
/// a mesma rota que a abertura conferiu, relida agora.
fn engine_route(host: &dyn OpenHost, spec: &SessionSpec) -> Result<Option<claude::EngineRoute>, OpenError> {
    if let Account::Engine { .. } = spec.account {
        if crate::runtime::process::resolve_program("hangar-engine", host.env()).is_none() {
            return Err(creation("hangar-engine nao esta no PATH deste servidor — sem ele a sessao com motor nasce e morre na \
                hora, sem erro. Instale o lancador: no Linux, scripts/install-claude-wrapper.sh; no Windows, install.ps1."));
        }
    }
    match &spec.account {
        Account::Engine { name, account: Some(account), .. } => Ok(Some(super::facts::fixed_account(
            &crate::accounts::AccountService::new(crate::accounts::environment::AccountEnvironment::from_map(host.env().clone())), name, account)?)),
        _ => Ok(None),
    }
}

/// O pane do Claude, o transcript da conversa nova e a rota da conta ChatGPT fixa do motor.
/// As escolhas de plugins e skills da conversa: as pedidas e, na retomada, as que ela já tinha guardado.
pub(crate) async fn conversation_settings(host: &dyn OpenHost, spec: &SessionSpec, sid: &str, resume: bool) -> Result<Option<Value>, OpenError> {
    if spec.customizations.is_none() && !resume { return Ok(None); }
    let effective_dir = spec.account.config_dir().map(str::to_owned).or_else(|| host.env().get("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()).cloned())
        .unwrap_or_else(|| host.home().join(".claude").to_string_lossy().into_owned());
    let settings = host.customizations(json!({"session_id": sid, "cwd": spec.cwd, "config_dir": effective_dir,
        "selection": spec.customizations, "resume": resume})).await?["settings"].clone();
    Ok(Some(settings).filter(|s| s.as_object().is_some_and(|o| !o.is_empty()) || spec.customizations.is_some()))
}

async fn claude_plan(host: &dyn OpenHost, spec: &SessionSpec, sid: &str, read_only: Option<&[String]>, initial_prompt: Option<&str>,
                     runtime: &Value, resume: bool) -> Result<(PaneSpec, String, Option<claude::EngineRoute>), OpenError> {
    let settings = conversation_settings(host, spec, sid, resume).await?;
    let route = engine_route(host, spec)?;
    let roots = claude::plugin_roots(host.home());
    let bridge = host.bridge();
    let key = host.new_bridge_key();
    let ctx = claude::ClaudeContext { plugin_roots: &roots, bridge: bridge.as_ref(), bridge_key: &key, runtime_config: runtime,
        server_env: host.env(), settings: settings.as_ref(), read_only, windows: host.terminal().windows, initial_prompt, engine_route: route.as_ref(),
        resume, allow_bypass: false };
    let pane = claude::pane(spec, sid, &ctx);
    let jsonl = claude::transcript(spec, sid, &host.server_projects()).to_string_lossy().into_owned();
    Ok((pane, jsonl, route))
}

/// A ponte de skills (`skill_bridge.py`): a skill instalada no Claude vale já nesta sessão de Pi ou
/// Kimi. Nunca impede a abertura.
async fn sync_skills(host: &dyn OpenHost) {
    let Some(argv) = host.backend_dir().and_then(|dir| agents::skill_bridge_argv(&dir, host.terminal().windows)) else {
        tracing::warn!(backend = ?host.backend_dir(), "ponte de skills sem a venv do backend; não refeita antes de abrir a sessão");
        return;
    };
    let out = host.run(argv, Duration::from_secs(30)).await;
    if out.code != Some(0) {
        tracing::warn!(code = ?out.code, stderr = %out.stderr.trim(), "ponte de skills não refeita antes de abrir a sessão");
    }
}

/// O sidecar do Claude sem terminal com os campos que o Python grava (`sessions.save`): os dois lados
/// ainda o leem, e um revert não pode reabrir a sessão em modo errado.
pub(crate) fn headless_sidecar(spec: &SessionSpec, sid: &str, previous_non_plan: Option<PermissionMode>, settings: Option<&Value>,
                    route: Option<&claude::EngineRoute>) -> Value {
    let (engine, context_window) = match &spec.account {
        Account::Engine { name, context_window, .. } => (Some(name.clone()), *context_window),
        _ => (None, None),
    };
    let mut meta = json!({
        "name": spec.name, "provider": "claude", "headless": true, "key": spec.key, "cwd": spec.cwd, "session_id": sid,
        "config_dir": spec.account.config_dir(), "engine": engine, "model": spec.model, "effort": spec.effort,
        "context_window": context_window, "permission_mode": spec.permission_mode.map(PermissionMode::name),
        "previous_non_plan": previous_non_plan.map(PermissionMode::name), "subagent_model": spec.subagent_model, "jev": spec.jev,
    });
    if let Some(tier) = spec.service_tier { meta["service_tier"] = json!(tier.name()); }
    if let Some(settings) = settings { meta["claude_settings"] = settings.clone(); }
    if let Some(route) = route {
        meta["engine_account"] = json!(route.account);
        meta["engine_credential_id"] = json!(route.credential_id);
        meta["engine_account_base_url"] = json!(route.base_url);
    }
    meta
}

/// A linha da primeira mensagem na fila da sessão nova (`runtime/<key>.json`), carimbada antes da
/// conversa existir (`pre_transcript`, como o kick-off do bastão). Bloqueante.
fn enqueue_first(queue_dir: &Path, key: &str, name: &str, text: &str) -> std::io::Result<()> {
    use crate::runtime::queue::{Action, State, Store, acquire_lease_within};
    let runtime = queue_dir.join("runtime");
    let _lease = acquire_lease_within(&runtime.join(format!("{key}.lock")), Duration::from_secs(3))?;
    let mut store = Store::open(&runtime.join(format!("{key}.json")), queue_dir, State::new(key, 1, name, Vec::new()))?;
    let epoch_s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    let id = crate::mods::state::random_hex(16);
    store.exec(1, &format!("append:{id}"), crate::runtime::protocol::ClockSample { monotonic_s: 0.0, epoch_s },
        Action::Append { text: text.into(), delivered: false, ts: Some(epoch_s), pre_transcript: true, entry_id: Some(id) })?;
    Ok(())
}

/// Do `validar` ao `sessao.criada` do Claude sem terminal: grava a primeira mensagem, o registro e o
/// sidecar. O processo sobe depois (`OpenHost::wake`), pelo spec; com a reserva adotada, o sidecar já
/// aponta o cano vivo dela e a subida só conecta.
#[allow(clippy::too_many_arguments)]
async fn headless_life(host: &dyn OpenHost, spec: &mut SessionSpec, previous_non_plan: Option<PermissionMode>, initial_prompt: Option<&str>,
                       diary: &mut Diary, remember_provider: bool, resume: Option<&str>, adopting: Option<&mut super::adopt::Adopting>)
                       -> Result<(Option<String>, Vec<String>, Option<claude::EngineRoute>), OpenError> {
    diary.step("validar");
    let sidecars = host.home().join(".hangar");
    let taken = |kind: &str| crate::runtime::codex_sidecar::path(&sidecars.join(kind), &spec.name).exists();
    // A resolução conferiu o nome antes de conta e worktree: outra sessão pode ter nascido desde então.
    // Sem pane, o multiplexador que não responde (ou não existe) não impede, como no `_create_headless`.
    let pane = terminal::presence(host.mux(), host.terminal(), &spec.name).await.0 == terminal::Presence::Present;
    if pane || taken("claude-headless") || taken("codex-sessions") {
        diary.refused("aviso", "nome_ja_em_uso", None, None);
        return Err(name_in_use());
    }
    let left = host.leave_old_group(&spec.name).await?;
    // Depois do `--prep`: a configuração que ele mexeu entra na comparação com a da reserva.
    let mut adopting = match adopting {
        Some(adopting) if resume.is_none() => adopting.confirm(spec).await.then_some(adopting),
        _ => None,
    };
    // A reserva já roda com a chave e a conversa dela (estão no ambiente do processo): a sessão nasce nelas.
    // Retomada: o processo sobe com `--resume`, porque o transcript dessa conversa já existe.
    // Adotada, as escolhas da conversa são as que a reserva gravou ao nascer (o processo subiu com elas);
    // a comparação do spec garantiu a mesma seleção pedida.
    let (sid, settings) = match adopting.as_deref().and_then(super::adopt::Adopting::reserve) {
        Some(reserve) => { spec.key = reserve.key.clone(); (reserve.session_id.clone(), reserve.settings.clone()) }
        None => {
            let sid = resume.map_or_else(|| host.new_session_id(), str::to_owned);
            let settings = conversation_settings(host, spec, &sid, resume.is_some()).await?;
            (sid, settings)
        }
    };
    let route = engine_route(host, spec)?;
    spec.conversation = Some(sid.clone());
    diary.step("confiar_pasta");
    if let Err(error) = claude::pretrust_async(claude::claude_json(host.terminal(), host.home(), spec.account.config_dir()), spec.cwd.clone(), host.terminal().windows).await {
        diary.trust_failed(&error);
    }
    // Antes do sidecar: depois dele o runtime pode adotar a sessão nova no meio da limpeza.
    host.release_name(&spec.name).await?;
    // A primeira mensagem já está na fila durável quando a criação responde; quem sobe a sessão
    // (o acordar, a subida do servidor, a próxima escrita) a entrega, e falhar a subida não a perde.
    if let Some(text) = initial_prompt.filter(|p| !p.is_empty()) {
        progress_step(&spec.name, "recado", json!({}));
        let queue_dir = crate::accounts::environment::AccountEnvironment::from_map(host.env().clone()).claude_base.join(".hangar-queue");
        let (key, name, text) = (spec.key.clone(), spec.name.clone(), text.to_owned());
        let queued = tokio::task::spawn_blocking(move || enqueue_first(&queue_dir, &key, &name, &text)).await
            .unwrap_or_else(|e| Err(std::io::Error::other(e.to_string())));
        if let Err(error) = queued {
            diary.refused("erro", "fila_nao_gravada", Some("sidecar_gravado"), Some(&error.kind().to_string()));
            return Err(OpenError::new(500, "erro_criacao_sessao", format!("a primeira mensagem não entrou na fila: {error}")));
        }
    }
    let mut meta = headless_sidecar(spec, &sid, previous_non_plan, settings.as_ref(), route.as_ref()).as_object().cloned().unwrap_or_default();
    if let Some(reserve) = adopting.as_deref().and_then(super::adopt::Adopting::reserve) {
        // O `cano` inteiro, como a subida grava: o `wake` acha o processo vivo e conecta sem subir outro.
        let cano = serde_json::to_value(&reserve.cano).map_err(|e| OpenError::new(500, "erro_criacao_sessao", format!("o processo da reserva não foi gravado: {e}")))?;
        meta.insert("cano".into(), cano);
    }
    persist(host, spec, diary, "sidecar_gravado", &sidecars.join("claude-headless"), &meta)?;
    if let Some(adopting) = adopting.as_deref_mut() { adopting.adopted().await; }
    let jsonl = claude::transcript(spec, &sid, &host.server_projects()).to_string_lossy().into_owned();
    let born = Born { name: spec.name.clone(), provider: spec.provider, jsonl: Some(jsonl.clone()), remember_provider, codex_terminal: None, restored: false };
    let mut warnings = left;
    warnings.extend(announce(host, spec, diary, &born, "sidecar_gravado").await);
    Ok((Some(jsonl), warnings, route))
}

/// O registro antes do que faz a sessão existir: sem ele ela não é reaberta nem reconhecida pela chave.
fn write_record(host: &dyn OpenHost, spec: &SessionSpec, diary: &mut Diary, stage: &'static str) -> Result<(), OpenError> {
    super::record::write(host.home(), spec).map_err(|error| {
        diary.refused("erro", "registro_nao_gravado", Some(stage), Some(&error.kind().to_string()));
        OpenError::new(500, "erro_criacao_sessao", format!("o registro da sessão não foi gravado: {error}"))
    })
}

fn forget_record(host: &dyn OpenHost, key: &str) {
    if let Err(error) = super::record::remove(host.home(), key) {
        tracing::warn!(kind = ?error.kind(), "registro da abertura que não nasceu ficou para trás");
    }
}

/// Registro e sidecar da sessão sem terminal; o sidecar que não grava leva o registro junto.
fn persist(host: &dyn OpenHost, spec: &SessionSpec, diary: &mut Diary, stage: &'static str, dir: &Path,
           meta: &crate::runtime::codex_sidecar::Meta) -> Result<(), OpenError> {
    write_record(host, spec, diary, stage)?;
    crate::runtime::codex_sidecar::put(dir, &spec.name, meta).map_err(|error| {
        diary.refused("erro", "sidecar_nao_gravado", Some(stage), Some(&error.kind().to_string()));
        forget_record(host, &spec.key);
        OpenError::new(500, "erro_criacao_sessao", format!("o arquivo da sessão não foi gravado: {error}"))
    })
}

async fn announce(host: &dyn OpenHost, spec: &SessionSpec, diary: &mut Diary, born: &Born, stage: &str) -> Vec<String> {
    let warnings = host.born(born).await;
    diary.events.push(json!({"evento": "sessao.criada", "nivel": "ok", "sessao": spec.name, "provider": spec.provider.name(),
        "etapa": stage, "operacao": diary.op}));
    warnings
}

/// O nome que a resolução conferiu pode ter ganhado um pane desde então.
async fn name_still_free(host: &dyn OpenHost, spec: &SessionSpec, diary: &mut Diary) -> Result<(), OpenError> {
    match terminal::presence(host.mux(), host.terminal(), &spec.name).await {
        (terminal::Presence::Absent, _) => Ok(()),
        (terminal::Presence::Present, _) => {
            diary.refused("aviso", "nome_ja_em_uso", None, None);
            Err(name_in_use())
        }
        (terminal::Presence::Unknown, stderr) => Err(mux_unavailable(&stderr)),
    }
}

/// `_create_codex_headless`: o sidecar sem terminal (com a chave do spec) e o registro; o processo
/// sobe logo depois pelo runtime (`warm_codex`), como o aquecimento da criação no Python.
async fn codex_headless_life(host: &dyn OpenHost, spec: &mut SessionSpec, codex_home: &str, initial_prompt: Option<&str>, diary: &mut Diary,
                             remember_provider: bool, resume: Option<&str>) -> Result<Vec<String>, OpenError> {
    spec.conversation = resume.map(str::to_owned);
    diary.step("validar");
    name_still_free(host, spec, diary).await?;
    let mut warnings = host.leave_old_group(&spec.name).await?;
    diary.step("confiar_pasta");
    super::codex::pretrust(Path::new(codex_home), &spec.cwd);
    host.release_name(&spec.name).await?;
    let mut meta = super::codex::headless_sidecar(spec, codex_home);
    // A thread que já existe: a subida a retoma (`thread/resume`) em vez de abrir outra.
    if let Some(thread) = resume { meta.insert("thread_id".into(), json!(thread)); }
    persist(host, spec, diary, "confiar_pasta", &crate::runtime::codex_sidecar::dir(host.home()), &meta)?;
    let born = Born { name: spec.name.clone(), provider: spec.provider, jsonl: None, remember_provider, codex_terminal: None, restored: false };
    warnings.extend(announce(host, spec, diary, &born, "sidecar_gravado").await);
    if initial_prompt.is_some_and(|p| !p.is_empty()) { progress_step(&spec.name, "recado", json!({})); }
    warnings.extend(host.warm_codex(&spec.name, initial_prompt).await);
    Ok(warnings)
}

/// Do `validar` ao `sessao.criada`: o pane e o que muda em volta dele.
#[allow(clippy::too_many_arguments)]
async fn terminal_life(host: &dyn OpenHost, spec: &mut SessionSpec, read_only: Option<&[String]>, initial_prompt: Option<&str>, diary: &mut Diary,
                       remember_provider: bool, runtime: &Value, codex_home: Option<&str>, resume: Option<&str>, restore: bool)
                       -> Result<(Option<String>, Vec<String>, Option<claude::EngineRoute>), OpenError> {
    diary.step("validar");
    let claude = spec.provider == AgentProvider::Claude;
    if matches!(spec.provider, AgentProvider::Pi | AgentProvider::Kimi) { sync_skills(host).await; }
    // A resolução conferiu o nome antes de conta e worktree: outro pane pode ter nascido desde então.
    name_still_free(host, spec, diary).await?;
    let mut warnings = if restore { Vec::new() } else { host.leave_old_group(&spec.name).await? };
    diary.step("preparar_comando");
    let sid = resume.map_or_else(|| host.new_session_id(), str::to_owned);
    let (pane, jsonl, route) = if claude {
        let (pane, jsonl, route) = claude_plan(host, spec, &sid, read_only, initial_prompt, runtime, resume.is_some()).await?;
        (pane, Some(jsonl), route)
    } else if let Some(home) = codex_home {
        super::codex::require_launcher(host.env())?;
        // A TUI abre a thread: o prompt inicial vai no comando do pane.
        let mut launch = super::codex::creation(spec, home, initial_prompt);
        launch.resume = resume.map(str::to_owned);
        // O rollout só nasce com a TUI, num caminho que não se deriva do cwd: nada a semear.
        (super::codex::pane(&spec.name, &spec.key, &launch, spec.jev, runtime, read_only, host.terminal().windows)?, None, None)
    } else {
        let ctx = agents::AgentContext { home: host.home(), server_env: host.env(), runtime_config: runtime, read_only,
            windows: host.terminal().windows };
        let conversation = resume.map_or(Conversation::New(&sid), Conversation::Resume);
        // O transcript nasce no 1º turno, num layout próprio: nada a semear.
        (agents::pane(spec, conversation, &ctx)?, None, None)
    };
    // O Kimi novo e o Codex só sabem a conversa depois da TUI (bilhete, sidecar).
    spec.conversation = match spec.provider {
        AgentProvider::Kimi | AgentProvider::Codex => resume.map(str::to_owned),
        _ => Some(sid.clone()),
    };
    diary.step("confiar_pasta");
    match spec.provider {
        AgentProvider::Claude => if let Err(error) = claude::pretrust_async(claude::claude_json(host.terminal(), host.home(), spec.account.config_dir()),
            spec.cwd.clone(), host.terminal().windows).await { diary.trust_failed(&error); },
        AgentProvider::Kimi => {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
            agents::kimi_pretrust(&agents::kimi_home(host.home(), host.env()), &spec.cwd, now);
        }
        // A conta adicional confia pelo preparo que o lançador pede antes da TUI.
        AgentProvider::Codex if matches!(&spec.account, Account::Codex { account_id } if account_id == "default") => {
            super::codex::pretrust(Path::new(codex_home.unwrap_or_default()), &spec.cwd);
        }
        _ => {}
    }
    diary.step("criar_terminal");
    if !restore { host.release_name(&spec.name).await?; }
    // A sessão que volta já tem registro (plugins, worktree, criadora): a falha do pane o devolve como estava.
    let previous = restore.then(|| super::record::read(host.home(), &spec.key).ok().flatten()).flatten();
    write_record(host, spec, diary, "criar_terminal")?;
    // O Claude e o Codex levam o prompt no comando; os outros, na fila.
    let queued = match initial_prompt.filter(|p| !claude && codex_home.is_none() && !p.is_empty()) {
        None => None,
        Some(text) => {
            progress_step(&spec.name, "recado", json!({}));
            let base = host.server_projects().parent().map(Path::to_path_buf).unwrap_or_default();
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
            match agents::queue_initial_prompt(&base, &spec.name, text, now) {
                Ok(path) => Some(path),
                Err(error) => {
                    diary.refused("erro", "prompt_inicial_nao_gravado", Some("criar_terminal"), Some(&error.kind().to_string()));
                    forget_record(host, &spec.key);
                    return Err(OpenError::new(500, "erro_criacao_sessao", format!("o prompt inicial não entrou na fila: {error}")));
                }
            }
        }
    };
    if let Err(error) = terminal::spawn(host.mux(), host.terminal(), &pane).await {
        diary.refused("erro", error.code, Some("criar_terminal"), Some(&error.stderr));
        if error.code == terminal::UNCERTAIN {
            return Err(OpenError::new(504, UNCERTAIN, "o tmux não respondeu e a sessão pode ter nascido; confira a lista antes de tentar de novo"));
        }
        let undone = match &previous {
            Some(previous) => super::record::write(host.home(), previous),
            None => super::record::remove(host.home(), &spec.key),
        };
        if let Err(error) = undone {
            tracing::warn!(kind = ?error.kind(), "registro da abertura que não nasceu não voltou ao de antes");
        }
        if let Some(path) = &queued && let Err(error) = std::fs::remove_file(path) {
            tracing::warn!(kind = ?error.kind(), "prompt inicial da abertura que não nasceu ficou na fila");
        }
        return Err(creation(match error.code {
            "terminal_morreu_ao_nascer" => format!("o terminal fechou logo depois de abrir: {}", error.stderr),
            _ => "falha ao criar sessao no tmux".to_owned(),
        }));
    }
    diary.step("limpar_estado_anterior");
    let born = Born { name: spec.name.clone(), provider: spec.provider, jsonl: jsonl.clone(), remember_provider, codex_terminal: codex_home.map(str::to_owned), restored: restore };
    warnings.extend(announce(host, spec, diary, &born, "terminal_criado").await);
    Ok((jsonl, warnings, route))
}
