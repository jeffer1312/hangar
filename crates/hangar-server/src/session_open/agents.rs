//! Plano de lançamento de Pi, omp e Kimi com terminal a partir do spec: o comando do agente, o `-e`
//! da sessão e a pasta pré-confiada do Kimi. Port dos ramos Pi/omp/Kimi de `registry.create` com os
//! `spawn_command`/`resume_command` dos adapters e `kimi_sessions.pretrust_cwd`.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use super::terminal::PaneSpec;
use super::{Account, AgentProvider, OpenError, SessionSpec};

/// A conversa do pane: nova (o id que o Hangar escolheu) ou a retomada de uma que já existe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conversation<'a> { New(&'a str), Resume(&'a str) }

pub struct AgentContext<'a> {
    pub home: &'a Path,
    /// Ambiente do servidor (`PI_CONFIG_DIR`, `PI_CODING_AGENT_DIR`, `KIMI_CODE_HOME`).
    pub server_env: &'a BTreeMap<String, String>,
    pub runtime_config: &'a Value,
    /// O envelope `bwrap … --` do só leitura.
    pub read_only: Option<&'a [String]>,
    pub windows: bool,
}

fn invalid_id() -> OpenError { OpenError::new(409, "erro_criacao_sessao", "session_id invalido") }

static UUID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\z").unwrap());
static KIMI_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\Asession_[0-9a-fA-F-]{36}\z").unwrap());

/// O id vai direto ao comando do shell: só a forma que o agente grava passa.
// ponytail: só o uuid com hífens (o dos nomes de arquivo); o `uuid.UUID` do Python aceitava chaves e `urn:`.
fn checked<'a>(provider: AgentProvider, id: &'a str) -> Result<&'a str, OpenError> {
    let ok = if provider == AgentProvider::Kimi { KIMI_ID.is_match(id) } else { UUID.is_match(id) };
    ok.then_some(id).ok_or_else(invalid_id)
}

fn text<'a>(env: &'a BTreeMap<String, String>, key: &str) -> Option<&'a str> { env.get(key).map(String::as_str).filter(|v| !v.is_empty()) }

/// `omp_dirs.agent_dir`: com perfil, `~/<PI_CONFIG_DIR ou .omp>/profiles/<perfil>/agent`; sem ele, o
/// `PI_CODING_AGENT_DIR` do servidor ou `~/<…>/agent`. `default` é a ausência de perfil.
// ponytail: o OMP_PROFILE/PI_PROFILE do ambiente do servidor não vale para a sessão: o pane só tem o perfil que o spec pôs.
pub fn omp_agent_dir(home: &Path, env: &BTreeMap<String, String>, profile: Option<&str>) -> PathBuf {
    let base = home.join(text(env, "PI_CONFIG_DIR").unwrap_or(".omp"));
    match profile.map(str::trim).filter(|p| !p.is_empty() && *p != "default") {
        Some(p) => base.join("profiles").join(p).join("agent"),
        None => text(env, "PI_CODING_AGENT_DIR").map_or_else(|| base.join("agent"), PathBuf::from),
    }
}

fn mtime(path: &Path) -> std::time::SystemTime {
    path.metadata().and_then(|m| m.modified()).unwrap_or(std::time::SystemTime::UNIX_EPOCH)
}

fn newest(paths: impl Iterator<Item = PathBuf>) -> Option<PathBuf> { paths.filter(|p| p.is_file()).max_by_key(|p| mtime(p)) }

fn ending_in(dir: &Path, suffix: &str) -> impl Iterator<Item = PathBuf> {
    let suffix = suffix.to_owned();
    std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path())
        .filter(move |p| p.file_name().is_some_and(|n| n.to_string_lossy().ends_with(&suffix)))
}

/// `pi_sessions.transcript_path` do omp: o mais novo `*_<id>.jsonl` da pasta do cwd; senão em
/// qualquer pasta da raiz (o omp grava o principal em `sessions/-/`).
fn omp_transcript(sessions: &Path, cwd: &str, id: &str) -> Option<PathBuf> {
    let suffix = format!("_{id}.jsonl");
    newest(ending_in(&sessions.join(crate::list::discover_other::pi_cwd_slug(cwd)), &suffix)).or_else(|| {
        let dirs = std::fs::read_dir(sessions).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir());
        newest(dirs.flat_map(|d| ending_in(&d, &suffix).collect::<Vec<_>>()))
    })
}

/// `model_args.args_de` de Pi, omp e Kimi: `--model`, e `--thinking` no Pi e no omp (o Kimi não tem
/// flag de esforço; a resolução já o recusou).
fn choice(spec: &SessionSpec) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(model) = spec.model.as_deref().filter(|m| !m.is_empty()) { out.extend(["--model".into(), model.into()]); }
    if let Some(effort) = spec.effort.as_deref().filter(|e| !e.is_empty()) { out.extend(["--thinking".into(), effort.into()]); }
    out
}

/// O pane do agente. O omp retomado precisa do transcript no disco (retoma por caminho).
pub fn pane(spec: &SessionSpec, conversation: Conversation, ctx: &AgentContext) -> Result<PaneSpec, OpenError> {
    let id = match conversation { Conversation::New(id) => id, Conversation::Resume(id) => checked(spec.provider, id)? };
    let profile = match &spec.account { Account::Omp { profile, .. } => Some(profile.as_str()), _ => None };
    let mut command: Vec<String> = match (spec.provider, conversation) {
        // `pi --session-id` retoma quando o id já existe ("creating it if missing").
        (AgentProvider::Pi, _) => vec!["pi".into(), "--session-id".into(), id.into()],
        (AgentProvider::Omp, Conversation::New(_)) => {
            let sessions = omp_agent_dir(ctx.home, ctx.server_env, profile).join("sessions");
            let stamp = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S-%3fZ");
            let target = sessions.join(crate::list::discover_other::pi_cwd_slug(&spec.cwd)).join(format!("{stamp}_{id}.jsonl"));
            vec!["omp".into(), "--session".into(), target.to_string_lossy().into_owned()]
        }
        (AgentProvider::Omp, Conversation::Resume(_)) => {
            let sessions = omp_agent_dir(ctx.home, ctx.server_env, profile).join("sessions");
            let found = omp_transcript(&sessions, &spec.cwd, id)
                .ok_or_else(|| OpenError::new(409, "erro_criacao_sessao", "transcript do omp nao encontrado para retomar"))?;
            vec!["omp".into(), "-r".into(), found.to_string_lossy().into_owned()]
        }
        // O Kimi não aceita id escolhido: o da conversa nova nasce no 1º prompt e chega pelo bilhete do hook.
        (AgentProvider::Kimi, Conversation::New(_)) => vec!["kimi".into()],
        (AgentProvider::Kimi, Conversation::Resume(_)) => vec!["kimi".into(), "--session".into(), id.into()],
        (other, _) => return Err(OpenError::new(500, "erro_criacao_sessao", format!("{} não abre por este plano", other.name()))),
    };
    command.extend(choice(spec));
    if let Some(prefix) = ctx.read_only {
        command = super::terminal::sandboxed(prefix, &command, ctx.windows);
    }
    let mut env = super::claude::jev_pairs(spec.jev, ctx.runtime_config);
    env.push(("HANGAR_CLAUDE_SETTINGS".into(), String::new()));
    // Por `-e`, como o resto da sessão: o pane recebe no Windows também, e o Pi aberto
    // pelo app ganha a reserva do bilhete que só o omp tinha.
    if spec.provider != AgentProvider::Kimi { env.push(("CP_PI_SESSION".into(), id.into())); }
    if let Some(profile) = profile { env.push(("OMP_PROFILE".into(), profile.into())); }
    Ok(PaneSpec { name: spec.name.clone(), cwd: spec.cwd.clone(), provider: spec.provider, key: spec.key.clone(), env,
        config_dir: spec.account.config_dir().map(str::to_owned), command })
}

/// O prompt inicial de Pi, omp e Kimi vai à fila durável do nome (`PromptQueue`, que entrega
/// quando o agente fica pronto), carimbado antes da conversa existir. Grava antes do pane: depois
/// dele o Python já pode estar drenando o arquivo.
pub fn queue_initial_prompt(claude_base: &Path, name: &str, text: &str, now: f64) -> std::io::Result<PathBuf> {
    let path = crate::locate::queue_path(claude_base, name);
    let row = json!({"id": crate::mods::state::random_hex(16), "text": text, "ts": now, "delivered": false, "pre_transcript": true});
    std::fs::create_dir_all(path.parent().unwrap())?;
    crate::runtime::queue::atomic_write(&path, format!("{row}\n").as_bytes())?;
    Ok(path)
}

/// `skill_bridge.py --quiet` do backend com o Python da raiz da instalação (`install_root::python_exe`:
/// o do pacote ou o venv); sem um dos dois, nada.
pub fn skill_bridge_argv(backend: &Path, windows: bool) -> Option<Vec<String>> {
    let python = crate::install_root::python_exe(backend.parent()?, windows);
    let script = backend.join("app").join("skill_bridge.py");
    (python.is_file() && script.is_file()).then(|| vec![python.to_string_lossy().into_owned(), script.to_string_lossy().into_owned(), "--quiet".into()])
}

/// `KIMI_CODE_HOME` do servidor ou `~/.kimi-code`.
pub fn kimi_home(home: &Path, env: &BTreeMap<String, String>) -> PathBuf {
    text(env, "KIMI_CODE_HOME").map_or_else(|| home.join(".kimi-code"), PathBuf::from)
}

/// `kimi_sessions.pretrust_cwd`: sem o `workspace-trust` a TUI nasce presa no "Trust this folder?".
/// Já confiada não reescreve; falha só vai ao log.
pub fn kimi_pretrust(kimi_home: &Path, cwd: &str, now_ms: u64) {
    let file = kimi_home.join("workspace-trust").join(crate::list::discover_other::kimi_workdir_key(cwd));
    if file.exists() { return; }
    let body = json!({"root": cwd, "trustedAt": now_ms}).to_string();
    if let Err(error) = std::fs::create_dir_all(file.parent().unwrap()).and_then(|()| crate::runtime::queue::atomic_write(&file, body.as_bytes())) {
        tracing::warn!(file = %file.display(), %error, "pré-confiar a pasta no Kimi falhou; o diálogo aparece na sessão");
    }
}
