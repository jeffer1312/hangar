//! Retomar uma conversa do Arquivo (`POST /api/archive/{project}/{session_id}/resume`): acha a conversa
//! (a conta dona, a pasta gravada no cabeçalho, a worktree apagada que volta para a principal) e a abre
//! pela mesma resolução da criação, retomando a conversa em vez de abrir outra. Port do
//! `resume_archived` com `archive.py` e `archive_providers.py`.
use std::collections::BTreeMap;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{AgentProvider, OpenError, OpenFacts, SessionOpenRequest};

/// O corpo do Python (`ResumeArchivedBody`): estrito e todo opcional.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveBody {
    #[serde(default)]
    pub engine: Option<String>,
    #[serde(default)]
    pub engine_account: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub config_dir: Option<String>,
    #[serde(default = "claude")]
    pub provider: String,
    #[serde(default)]
    pub codex_account: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// Com ou sem terminal acima do padrão do servidor (`None`: o padrão).
    #[serde(default)]
    pub headless: Option<bool>,
}

fn claude() -> String { "claude".into() }

impl ArchiveBody {
    /// Corpo ausente vale o padrão.
    pub fn parse(bytes: &[u8]) -> Result<Self, OpenError> {
        if bytes.iter().all(u8::is_ascii_whitespace) { return Ok(Self { provider: claude(), ..Default::default() }); }
        serde_json::from_slice(bytes).map_err(|e| OpenError::invalid_body(&e.to_string()))
    }
}

static PROJECT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A[A-Za-z0-9-]+\z").unwrap());
static UUID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\z").unwrap());
static KIMI_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\Asession_[0-9a-fA-F-]{36}\z").unwrap());

/// Como os `ValueError`/`FileNotFoundError` do Python viram resposta na rota.
#[derive(Debug)]
enum Miss { Invalid, Absent, Coded(OpenError) }

impl From<Miss> for OpenError {
    fn from(miss: Miss) -> Self {
        match miss {
            Miss::Invalid => OpenError::new(400, "erro_path_invalido", "invalid path"),
            Miss::Absent => OpenError::new(404, "erro_transcript_nao_encontrado", "transcript not found"),
            Miss::Coded(error) => error,
        }
    }
}

/// O que a abertura precisa saber da máquina para achar a conversa.
pub struct Where<'a> {
    pub home: &'a Path,
    pub env: &'a BTreeMap<String, String>,
    /// `projects` da conta do servidor (a do processo, `None` no `_contas`).
    pub server_projects: &'a Path,
    /// As contas Claude do catálogo, pela ordem dele.
    pub config_dirs: &'a [String],
    /// As sessões vivas e o transcript de cada uma.
    pub live: &'a [(String, String)],
}

/// `_contas`: (conta, `projects`) das contas Claude a varrer; a do servidor por último, se ainda não veio.
fn claude_accounts(w: &Where, only: Option<&str>) -> Vec<(Option<String>, PathBuf)> {
    if let Some(dir) = only { return vec![(Some(dir.to_owned()), Path::new(dir).join("projects"))]; }
    let real = |p: &Path| p.canonicalize().ok();
    let mut out: Vec<(Option<String>, PathBuf)> = Vec::new();
    let mut seen = Vec::new();
    for dir in w.config_dirs {
        let projects = Path::new(dir).join("projects");
        let Some(r) = real(&projects) else { continue };
        if seen.contains(&r) { continue; }
        seen.push(r);
        out.push((Some(dir.clone()), projects));
    }
    if real(w.server_projects).is_some_and(|r| !seen.contains(&r)) { out.push((None, w.server_projects.to_owned())); }
    out
}

/// `conta_de`: a conta que guarda a conversa (`None` = a do servidor).
fn claude_owner(w: &Where, project: &str, sid: &str) -> Result<Option<String>, Miss> {
    if !PROJECT.is_match(project) || !UUID.is_match(sid) { return Err(Miss::Invalid); }
    claude_accounts(w, None).into_iter().find(|(_, base)| base.join(project).join(format!("{sid}.jsonl")).is_file())
        .map(|(cfg, _)| cfg).ok_or(Miss::Absent)
}

/// `archive_jsonl` do Claude: sem conta, a primeira que tiver o arquivo.
fn claude_jsonl(w: &Where, project: &str, sid: &str, config_dir: Option<&str>) -> Result<PathBuf, Miss> {
    if !PROJECT.is_match(project) || !UUID.is_match(sid) { return Err(Miss::Invalid); }
    claude_accounts(w, config_dir).into_iter().map(|(_, base)| base.join(project).join(format!("{sid}.jsonl")))
        .find(|p| p.is_file()).ok_or(Miss::Absent)
}

/// O primeiro valor que `field` acha nas `lines` primeiras linhas JSON do arquivo.
fn header(path: &Path, lines: usize, field: impl Fn(&Value) -> Option<String>) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    std::io::BufReader::new(file).split(b'\n').take(lines).flatten()
        .filter_map(|line| serde_json::from_slice::<Value>(&line).ok()).find_map(|obj| field(&obj))
}

fn mtime(path: &Path) -> std::time::SystemTime { path.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH) }

fn text(value: &Value) -> Option<String> { value.as_str().filter(|s| !s.is_empty()).map(str::to_owned) }

/// `_pi_jsonl`: o mais novo `*/*_<id>.jsonl` da raiz do agente.
fn pi_jsonl(root: &Path, sid: &str) -> Result<PathBuf, Miss> {
    if !UUID.is_match(sid) { return Err(Miss::Invalid); }
    let suffix = format!("_{sid}.jsonl");
    std::fs::read_dir(root).into_iter().flatten().flatten().map(|d| d.path()).filter(|d| d.is_dir())
        .flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten().map(|f| f.path()).collect::<Vec<_>>())
        .filter(|f| f.is_file() && f.file_name().is_some_and(|n| n.to_string_lossy().ends_with(&suffix)))
        .max_by_key(|f| mtime(f)).ok_or(Miss::Absent)
}

/// `_kimi_conversas`: a entrada mais recente do índice com o wire gravado (o wire e a pasta da sessão).
fn kimi_jsonl(kimi_home: &Path, sid: &str) -> Result<(PathBuf, Option<String>), Miss> {
    if !KIMI_ID.is_match(sid) { return Err(Miss::Invalid); }
    let index = std::fs::read(kimi_home.join("session_index.jsonl")).map_err(|_| Miss::Absent)?;
    index.split(|b| *b == b'\n').rev().filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .find(|o| o["sessionId"] == sid && o["sessionDir"].as_str().is_some_and(|d| !d.is_empty()))
        .and_then(|o| {
            let wire = Path::new(o["sessionDir"].as_str()?).join("agents").join("main").join("wire.jsonl");
            wire.is_file().then(|| (wire, text(&o["workDir"])))
        }).ok_or(Miss::Absent)
}

/// `_under` do `codex_contas`: o caminho real dentro da raiz.
fn under(path: &Path, root: &Path) -> bool {
    let real = |p: &Path| p.canonicalize().unwrap_or_else(|_| std::path::absolute(p).unwrap_or_else(|_| p.to_owned()));
    real(path).starts_with(real(root))
}

/// `account_for_rollout`: a conta Codex cuja `sessions`/`archived_sessions` guarda o rollout.
fn codex_owner<'a>(accounts: &'a [crate::accounts::catalog::Account], rollout: &Path) -> Result<Option<&'a crate::accounts::catalog::Account>, Miss> {
    let found: Vec<&crate::accounts::catalog::Account> = accounts.iter()
        .filter(|a| under(rollout, &a.home.join("sessions")) || under(rollout, &a.home.join("archived_sessions"))).collect();
    if found.len() > 1 {
        return Err(Miss::Coded(OpenError::new(409, "codex_account_ambiguous_rollout", "o rollout está em mais de uma conta Codex")
            .with_params(json!({"path": rollout, "accounts": found.iter().map(|a| a.id.clone()).collect::<Vec<_>>()}))));
    }
    Ok(found.into_iter().next())
}

/// Os rollouts `rollout-*-<id>.jsonl` da conta, nas duas raízes.
fn codex_rollouts(home: &Path, sid: &str) -> Vec<PathBuf> {
    fn walk(dir: &Path, suffix: &str, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() { walk(&path, suffix, out); }
            else if path.file_name().is_some_and(|n| { let n = n.to_string_lossy(); n.starts_with("rollout-") && n.ends_with(suffix) }) { out.push(path); }
        }
    }
    let mut out = Vec::new();
    let suffix = format!("-{sid}.jsonl");
    for root in [home.join("sessions"), home.join("archived_sessions")] { walk(&root, &suffix, &mut out); }
    out
}

/// `_codex_jsonl`: o rollout da conversa e a conta dona; conta pedida que não é a dona é conflito.
fn codex_jsonl(accounts: &[crate::accounts::catalog::Account], sid: &str, asked: Option<&str>) -> Result<(PathBuf, String), Miss> {
    let mismatch = |origin: &str| Miss::Coded(OpenError::new(409, "codex_account_archive_mismatch", "a conversa é de outra conta Codex")
        .with_params(json!({"account_id": asked, "origin_account": origin})));
    let mut found: Vec<(PathBuf, String)> = Vec::new();
    for account in accounts.iter().filter(|a| asked.is_none_or(|id| a.id == id)) {
        for path in codex_rollouts(&account.home, sid) {
            if let Some(owner) = codex_owner(accounts, &path)? && owner.id == account.id { found.push((path, owner.id.clone())); }
        }
    }
    if let Some(asked) = asked && found.is_empty() {
        for account in accounts.iter().filter(|a| a.id != asked) {
            for path in codex_rollouts(&account.home, sid) {
                if let Some(owner) = codex_owner(accounts, &path)? && owner.id != asked { return Err(mismatch(&owner.id)); }
            }
        }
    }
    let mut ids: Vec<&String> = found.iter().map(|(_, id)| id).collect();
    ids.sort();
    ids.dedup();
    if asked.is_none() && ids.len() > 1 {
        return Err(Miss::Coded(OpenError::new(409, "codex_account_ambiguous_rollout", "a conversa está em mais de uma conta Codex")
            .with_params(json!({"session_id": sid, "accounts": ids}))));
    }
    found.into_iter().max_by_key(|(p, _)| mtime(p)).ok_or(Miss::Absent)
}

/// O rollout que veio de uma transferência Claude→Codex: retomá-lo exige conferir a fronteira dela, que
/// é da transferência (no Python). Índice ilegível também recusa: não se sabe se é dela.
fn from_transfer(home: &Path, codex_home: &Path, thread: &str) -> bool {
    let home_text = codex_home.canonicalize().unwrap_or_else(|_| codex_home.to_owned()).to_string_lossy().into_owned();
    let key = format!("[{}, {}]", serde_json::to_string(&home_text).unwrap_or_default(), serde_json::to_string(thread).unwrap_or_default());
    let digest: String = ring::digest::digest(&ring::digest::SHA256, key.as_bytes()).as_ref().iter().map(|b| format!("{b:02x}")).collect();
    match std::fs::read(home.join(".hangar").join("conversation-transfers").join("indexes").join(format!("thread-{digest}.json"))) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => true,
        Ok(raw) => serde_json::from_slice::<Value>(&raw).ok().and_then(|v| v.as_array().map(|a| !a.is_empty())).unwrap_or(true),
    }
}

/// `worktrees.redirect`: a worktree apagada pelo Hangar retoma na pasta principal.
fn redirect(home: &Path, cwd: String) -> String {
    if Path::new(&cwd).is_dir() { return cwd; }
    let removed = hangar_workspace::worktrees::removed_at(&home.join(".hangar").join("worktrees-removidas.json"));
    removed.into_iter().find(|(path, _)| Path::new(&cwd).starts_with(path)).map_or(cwd, |(_, main)| main)
}

/// A conversa achada: o transcript, a pasta dela e, no Claude, a conta dona; no Codex, a conta.
pub struct Found { pub transcript: PathBuf, pub cwd: String, pub owner: Option<String>, pub codex_account: Option<String> }

/// O pedido de abertura da retomada e o que a rota faz em volta dele.
pub struct Plan { pub request: SessionOpenRequest, pub found: Found, pub provider: AgentProvider, pub project: String }

/// `-` no lugar do projeto: a pasta que guarda a conversa Claude, em qualquer conta (o id é único).
fn claude_project(w: &Where, project: &str, sid: &str) -> Result<String, Miss> {
    if project != "-" { return Ok(project.to_owned()); }
    if !UUID.is_match(sid) { return Err(Miss::Invalid); }
    let file = format!("{sid}.jsonl");
    claude_accounts(w, None).into_iter().find_map(|(_, base)| std::fs::read_dir(base).ok()?.flatten()
        .find(|e| e.path().join(&file).is_file())?.file_name().into_string().ok()).ok_or(Miss::Absent)
}

/// As validações e a busca do `resume_archived`, na ordem dele; o nome livre vem por último.
pub fn plan(w: &Where, facts: &dyn OpenFacts, project: &str, sid: &str, body: ArchiveBody) -> Result<Plan, OpenError> {
    if body.config_dir.as_ref().is_some_and(|dir| !w.config_dirs.contains(dir)) {
        return Err(OpenError::new(400, "erro_config_dir_invalido", "config_dir invalido"));
    }
    let provider = AgentProvider::parse(&body.provider).ok_or_else(|| OpenError::new(400, "erro_provider_invalido", "provider invalido"))?;
    if body.codex_account.is_some() && provider != AgentProvider::Codex {
        return Err(OpenError::new(400, "codex_account_so_codex", "codex_account só vale para provider codex"));
    }
    if provider == AgentProvider::Codex && !UUID.is_match(sid) {
        return Err(OpenError::new(400, "erro_rollout_sem_id", "nome de rollout sem id de conversa"));
    }
    let project = if provider == AgentProvider::Claude { claude_project(w, project, sid)? } else { project.to_owned() };
    let project = project.as_str();
    let found = match provider {
        AgentProvider::Claude => {
            // A conta pedida vence; sem ela, a dona (a conversa noutra conta morre com "No conversation found").
            // Pedida outra conta, a conversa é procurada onde está e muda de conta depois da resolução.
            let lookup = match claude_owner(w, project, sid) {
                Ok(owner) => owner,
                Err(_) => body.config_dir.clone(),
            };
            let transcript = claude_jsonl(w, project, sid, lookup.as_deref())?;
            let cwd = header(&transcript, 60, |o| text(&o["cwd"]));
            Found { transcript, cwd: cwd.unwrap_or_default(), owner: lookup, codex_account: None }
        }
        AgentProvider::Pi | AgentProvider::Omp => {
            let root = match provider {
                AgentProvider::Pi => w.env.get("PI_CODING_AGENT_SESSION_DIR").filter(|v| !v.is_empty()).map(PathBuf::from)
                    .unwrap_or_else(|| w.home.join(".pi").join("agent").join("sessions")),
                _ => super::agents::omp_agent_dir(w.home, w.env, None).join("sessions"),
            };
            let transcript = pi_jsonl(&root, sid)?;
            let cwd = header(&transcript, 5, |o| (o["type"] == "session").then(|| text(&o["cwd"])).flatten());
            Found { transcript, cwd: cwd.unwrap_or_default(), owner: None, codex_account: None }
        }
        AgentProvider::Kimi => {
            let (transcript, cwd) = kimi_jsonl(&super::agents::kimi_home(w.home, w.env), sid)?;
            Found { transcript, cwd: cwd.unwrap_or_default(), owner: None, codex_account: None }
        }
        AgentProvider::Codex => {
            let service = crate::accounts::AccountService::new(crate::accounts::environment::AccountEnvironment::from_map(w.env.clone()));
            let accounts = match body.codex_account.as_deref() {
                Some(id) => service.resolve(crate::accounts::Provider::Codex, id).map(|a| vec![a]),
                None => service.snapshot(crate::accounts::Provider::Codex),
            }.map_err(|e| OpenError::new(e.status, e.code, e.message.clone()).with_params(e.params.clone()))?;
            let all = service.snapshot(crate::accounts::Provider::Codex).map_err(|e| OpenError::new(e.status, e.code, e.message))?;
            let (transcript, owner) = match codex_jsonl(&all, sid, body.codex_account.as_deref()) {
                Ok(found) if accounts.iter().any(|a| a.id == found.1) => found,
                Ok(_) => return Err(Miss::Absent.into()),
                Err(miss) => return Err(miss.into()),
            };
            let home = all.iter().find(|a| a.id == owner).map(|a| a.home.clone()).unwrap_or_default();
            if from_transfer(w.home, &home, sid) {
                return Err(OpenError::new(409, "session_transfer_resume_unsupported",
                    "esta conversa veio de uma transferência do Claude para o Codex; retomá-la pelo Arquivo ainda não é possível"));
            }
            let cwd = header(&transcript, 5, |o| (o["type"] == "session_meta").then(|| text(&o["payload"]["cwd"])).flatten());
            Found { transcript, cwd: cwd.unwrap_or_default(), owner: None, codex_account: Some(owner) }
        }
    };
    if found.cwd.is_empty() { return Err(OpenError::new(422, "erro_cwd_ausente", "cwd not found in transcript")); }
    let found = Found { cwd: redirect(w.home, found.cwd.clone()), ..found };
    // Um segundo processo no mesmo arquivo corromperia a conversa.
    let real = |p: &str| std::fs::canonicalize(p).unwrap_or_else(|_| PathBuf::from(p));
    let target = real(&found.transcript.to_string_lossy());
    if let Some((name, _)) = w.live.iter().find(|(_, jsonl)| real(jsonl) == target) {
        return Err(OpenError::new(409, "erro_conversa_viva", "conversa já está aberta").with_params(json!({"sessao": name})));
    }
    let base = crate::list::discover_other::sanitize_session_name(body.name.as_deref()
        .unwrap_or_else(|| Path::new(&found.cwd).file_name().and_then(|n| n.to_str()).unwrap_or("")));
    let base = if base.is_empty() { "sessao".to_owned() } else { base };
    let mut name = base.clone();
    for i in 2.. {
        if !facts.name_in_use(&name)? { break; }
        name = format!("{base}-{i}");
    }
    let config_dir = body.config_dir.clone().or_else(|| found.owner.clone());
    let request = SessionOpenRequest { name: Some(name), cwd: found.cwd.clone(), config_dir, provider: Some(provider.name().into()),
        codex_account: found.codex_account.clone(), account_bound: provider == AgentProvider::Codex,
        engine: body.engine, engine_account: body.engine_account, model: body.model,
        headless: body.headless, ..Default::default() };
    Ok(Plan { request, found, provider, project: project.to_owned() })
}

/// `move_conversation`: a conversa vai inteira para a conta `to` (o `<id>.jsonl` e a pasta irmã), sem
/// cópia na origem. Já estava lá: `false`.
pub fn move_conversation(server_projects: &Path, project: &str, sid: &str, from: Option<&str>, to: Option<&str>) -> Result<bool, OpenError> {
    move_with(&|a, b| std::fs::rename(a, b), server_projects, project, sid, from, to)
}

fn move_with(rename: &dyn Fn(&Path, &Path) -> std::io::Result<()>, server_projects: &Path, project: &str, sid: &str, from: Option<&str>, to: Option<&str>)
    -> Result<bool, OpenError> {
    let projects = |dir: Option<&str>| dir.map_or_else(|| server_projects.to_owned(), |d| Path::new(d).join("projects"));
    let (src, dst) = (projects(from).join(project), projects(to).join(project));
    // A conta padrão pode vir pelo nome da pasta e pela ausência: a mesma pasta não é mudança.
    let real = |p: &Path| p.parent().and_then(|p| p.canonicalize().ok());
    if from == to || real(&src).is_some_and(|s| real(&dst) == Some(s)) { return Ok(false); }
    let file = format!("{sid}.jsonl");
    if dst.join(&file).exists() || dst.join(sid).exists() {
        return Err(OpenError::new(409, "erro_conversa_ja_na_conta", "a conta destino ja tem esta conversa"));
    }
    let failed = |e: std::io::Error| OpenError::new(500, "erro_mover_conversa", format!("nao consegui mover a conversa de conta: {e}"))
        .with_params(json!({"erro": e.to_string()}));
    std::fs::create_dir_all(&dst).map_err(failed)?;
    rename(&src.join(&file), &dst.join(&file)).map_err(failed)?;
    if src.join(sid).is_dir() && let Err(error) = rename(&src.join(sid), &dst.join(sid)) {
        // Sem a pasta irmã a conversa perde tool-results e subagentes: melhor inteira na origem.
        if let Err(back) = rename(&dst.join(&file), &src.join(&file)) {
            let message = format!("nao consegui mover a conversa de conta: {error}; a conversa ficou partida entre as contas: {} e {}",
                dst.join(&file).display(), src.join(sid).display());
            tracing::error!(back = %back, "{message}");
            let line = json!({"evento": "sessao.conversa_partida", "nivel": "erro", "codigo": "erro_mover_conversa", "detalhe": message});
            crate::diag::record_blocking(crate::diag::diary_dir().as_deref(), &[line], "conversa partida");
            return Err(OpenError::new(500, "erro_mover_conversa", message).with_params(json!({"erro": error.to_string()})));
        }
        return Err(failed(error));
    }
    Ok(true)
}

/// Abre a retomada resolvida: a conversa muda para a conta em que a sessão vai nascer (a pedida, ou a de
/// mais folga pela regra dos 95%) e volta para a de antes se a abertura falhar.
pub async fn open(host: &dyn super::open::OpenHost, plan: Plan, mut resolved: super::Resolved, project: &str, sid: &str)
    -> Result<Value, OpenError> {
    resolved.resume = Some(sid.to_owned());
    let server_projects = host.server_projects();
    let to = resolved.spec.account.config_dir().map(str::to_owned);
    let from = plan.found.owner.clone();
    let moved = plan.provider == AgentProvider::Claude && move_conversation(&server_projects, project, sid, from.as_deref(), to.as_deref())?;
    match super::open::open_session(host, resolved, &super::OpenCaller::Owner).await {
        Ok(opened) => {
            let mut body = opened.body;
            // A resposta é a `SessionInfo` do Python; o aviso de conta trocada vai em `avisos`.
            if let Some(map) = body.as_object_mut() { map.remove("config_dir"); map.remove("account_source"); }
            Ok(body)
        }
        Err(error) => {
            // Incerta, a sessão pode estar de pé na conta nova: devolver partiria o transcript em dois.
            if moved && error.code != Some("session_open_uncertain") && let Err(undo) = move_conversation(&server_projects, project, sid, to.as_deref(), from.as_deref()) {
                tracing::error!(code = undo.code.unwrap_or("-"), "a conversa da retomada que falhou ficou na conta nova");
            }
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    /// SF8: a pasta irmã que não muda e o arquivo que não volta deixam a conversa partida entre as
    /// contas, e o erro diz isso.
    #[test]
    fn a_move_that_cannot_be_undone_says_the_conversation_is_split() {
        let root = tempfile::tempdir().unwrap();
        let (a, b) = (root.path().join("a"), root.path().join("b"));
        std::fs::create_dir_all(a.join("projects/p/s1")).unwrap();
        std::fs::write(a.join("projects/p/s1.jsonl"), "{}").unwrap();
        let calls = std::cell::Cell::new(0);
        let rename = |_: &std::path::Path, _: &std::path::Path| {
            calls.set(calls.get() + 1);
            if calls.get() == 1 { Ok(()) } else { Err(std::io::Error::other("ocupado")) }
        };
        let error = super::move_with(&rename, root.path(), "p", "s1", a.to_str(), b.to_str()).unwrap_err();
        assert_eq!(calls.get(), 3, "a volta foi tentada");
        assert!(error.msg.contains("partida"), "{}", error.msg);
    }
}
