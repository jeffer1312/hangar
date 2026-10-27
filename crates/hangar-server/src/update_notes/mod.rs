//! `GET /api/update/notes`: avisos destrutivos e novidades que a próxima atualização traz, lidos
//! direto do ref alvo no git (sem fetch: o cliente chama depois do `procurar=1`).
use crate::routes::{AppState, gate};
use crate::session_write::{detail_body, json_response};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const GIT_TIMEOUT: Duration = Duration::from_secs(5);

pub async fn route(State(state): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Response {
    if !gate(&state, peer, &request).1 {
        return json_response(StatusCode::FORBIDDEN, detail_body("update_notes_owner_only", "só o dono pode ler as novidades da atualização", json!({})));
    }
    let home = state.accounts.env.home.clone();
    if let crate::update::http::Mode::Package(_) = crate::update::http::mode(&home) {
        return match package(&state).await {
            Ok(notes) => json_response(StatusCode::OK, with_acceptance(notes, &home)),
            Err(why) => { tracing::warn!("leitura dos avisos da atualização falhou no índice do pacote: {why}"); unavailable() }
        };
    }
    // Mesmo registro que o `atualizacoes.py` grava.
    let base = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| state.accounts.env.home.join(".claude"));
    let cwd = std::env::current_dir().unwrap_or_default();
    let branch = update_branch(std::env::var("CP_UPDATE_BRANCH").ok(), tokio::fs::read_to_string(cwd.join(".env")).await.ok().as_deref());
    match collect(&cwd, &format!("origin/{branch}"), &crate::update::state_dir(&base)).await {
        Ok(notes) => json_response(StatusCode::OK, with_acceptance(notes, &home)),
        Err(()) => { tracing::warn!(branch = %branch, cwd = %cwd.display(), "leitura dos avisos da atualização falhou no git"); unavailable() }
    }
}

/// Aviso já aceito sai; o da migração leva a escolha do modo e volta para quem migrou sem vê-lo.
fn with_acceptance(mut notes: Value, home: &Path) -> Value {
    let layout = crate::update::Layout::new(home);
    let accepted = crate::update::accepted(&layout);
    let step = crate::update::migrate::STEP_ID;
    let Some(warnings) = notes.get_mut("warnings").and_then(Value::as_array_mut) else { return notes };
    warnings.retain(|w| !w["id"].as_str().is_some_and(|id| accepted.contains(id)));
    for warning in warnings.iter_mut().filter(|w| w["id"] == step) {
        warning["choice"] = "mode".into();
        warning["mode"] = Value::Null;
    }
    if !warnings.iter().any(|w| w["id"] == step) && let Some(warning) = crate::update::migrate::pending_warning(home, &notes_root(home)) {
        warnings.insert(0, warning);
    }
    notes
}

/// Onde estão os `docs/atualizacoes` que esta máquina roda: o `current`, senão o checkout do servidor.
pub(crate) fn notes_root(home: &Path) -> PathBuf {
    let current = crate::install_root::current(home);
    if std::fs::symlink_metadata(&current).is_ok() { return current; }
    let cwd = std::env::current_dir().unwrap_or_default();
    if cwd.file_name().is_some_and(|n| n == "backend") { cwd.parent().map(Path::to_path_buf).unwrap_or(cwd) } else { cwd }
}

fn unavailable() -> Response {
    json_response(StatusCode::SERVICE_UNAVAILABLE, detail_body("update_notes_unavailable", "não deu para ler os avisos desta atualização", json!({})))
}

/// No pacote: as notas da versão publicada no canal instalado (o índice as leva), menos os passos
/// já aplicados nesta máquina e as novidades que a versão em uso já traz.
async fn package(state: &AppState) -> Result<Value, String> {
    let layout = crate::update::Layout::new(&state.accounts.env.home);
    let installed = crate::update::installed(&layout).ok_or_else(|| "manifest.json da versão em uso ilegível".to_owned())?;
    let channel = crate::update::channel(&layout);
    // Trocar de canal instala o publicado no outro, mesmo com número menor.
    let switching = channel != installed.channel;
    let release = crate::update::Release { base_url: state.update_release_base.clone(), channel };
    let published = crate::update::http::published(&release, false).await;
    if let Some(error) = published.error { return Err(error); }
    let entry = published.entry.ok_or_else(|| "índice sem entrada para este sistema".to_owned())?;
    let newer = match (entry.version.parse::<hangar_package::Version>(), installed.version.parse::<hangar_package::Version>()) {
        (Ok(remote), Ok(here)) => remote > here || (switching && remote != here),
        _ => return Err(format!("versão ilegível: publicada {}, em uso {}", entry.version, installed.version)),
    };
    if !newer { return Ok(json!({"target": entry.version, "warnings": [], "news": []})); }
    let notes = entry.notes.ok_or_else(|| format!("o índice não traz os avisos de {}", entry.version))?;
    let done = crate::update::steps::applied(&crate::update::state_dir(&state.accounts.env.claude_base))?;
    let shipped = layout.current().join("docs").join("novidades");
    let id = |v: &Value| v["id"].as_str().unwrap_or_default().to_owned();
    let warnings: Vec<Value> = notes.warnings.into_iter().filter(|w| !done.contains(&id(w))).collect();
    let news: Vec<Value> = notes.news.into_iter().filter(|n| !shipped.join(format!("{}.md", id(n))).exists()).collect();
    Ok(json!({"target": entry.version, "warnings": warnings, "news": news}))
}

async fn git(dir: &Path, args: &[&str]) -> Result<String, ()> {
    let (dir, args): (PathBuf, Vec<String>) = (dir.to_owned(), args.iter().map(|a| (*a).to_owned()).collect());
    let out = tokio::task::spawn_blocking(move || {
        hangar_workspace::process::run(&dir, &args.iter().map(String::as_str).collect::<Vec<_>>(), GIT_TIMEOUT)
    }).await;
    match out {
        Ok(Ok(out)) if out.code == 0 => Ok(out.stdout),
        _ => Err(()),
    }
}

async fn show(repo: &Path, target: &str, path: &str) -> Result<String, ()> {
    git(repo, &["show", &format!("{target}:{path}")]).await
}

/// Nomes `*.md` (sem README) de uma pasta no ref; pasta ausente no ref = lista vazia.
async fn list_md(repo: &Path, rev: &str, dir: &str) -> Result<Vec<String>, ()> {
    let out = git(repo, &["ls-tree", "-z", "--name-only", &format!("{rev}:{dir}")]).await;
    let out = match out {
        Ok(out) => out,
        // `rev:dir` falha tanto com a pasta ausente quanto com o ref ausente: separa pelo ref.
        Err(()) => { git(repo, &["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")]).await?; return Ok(Vec::new()); }
    };
    let mut names: Vec<String> = out.split('\0').filter(|n| is_note_file(n)).map(str::to_owned).collect();
    names.sort();
    Ok(names)
}

fn is_note_file(name: &str) -> bool { name.ends_with(".md") && !name.to_uppercase().starts_with("README") }

/// Espelho do `_frontmatter` de `backend/app/atualizacoes.py`: `chave: valor` entre `---`.
fn frontmatter(text: &str) -> (HashMap<String, String>, String) {
    let lines: Vec<&str> = text.lines().collect();
    if lines.first().is_none_or(|l| l.trim() != "---") { return (HashMap::new(), text.to_owned()); }
    let Some(end) = lines.iter().skip(1).position(|l| *l == "---").map(|i| i + 1) else { return (HashMap::new(), text.to_owned()) };
    let fields = lines[1..end].iter()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    (fields, lines[end + 1..].join("\n").trim().to_owned())
}

/// A branch que o Atualizar segue, como o `alvo()` do Python: `CP_UPDATE_BRANCH` do ambiente, senão
/// do `backend/.env` (o pydantic lê de lá e o filho não herda), senão a main. Nome inválido = main.
pub(crate) fn update_branch(env: Option<String>, dotenv: Option<&str>) -> String {
    let from_file = || dotenv?.lines().find_map(|l| l.trim().strip_prefix("CP_UPDATE_BRANCH=")).map(|v| v.trim().trim_matches(['"', '\'']).to_owned());
    let value = env.filter(|v| !v.trim().is_empty()).or_else(from_file).unwrap_or_default();
    let value = value.trim();
    let valid = !value.is_empty() && !value.starts_with('-') && !value.contains("..")
        && value.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'));
    if valid { value.to_owned() } else { "main".to_owned() }
}


fn news_item(id: &str, text: &str) -> Option<(u8, Value)> {
    let (f, body) = frontmatter(text);
    let title = f.get("titulo").filter(|t| !t.is_empty())?;
    let (rank, kind) = match f.get("tipo").map(String::as_str)? { "novo" => (0, "new"), "melhoria" => (1, "improvement"), "correcao" => (2, "fix"), _ => return None };
    let platforms = match f.get("onde").map(String::as_str)? { "desktop" => "desktop", "celular" => "mobile", "todos" => "all", _ => return None };
    let opt = |k: &str| f.get(k).filter(|v| !v.is_empty()).cloned();
    // Correção não ganha animação: não há gesto novo a mostrar.
    let animation = if kind == "fix" { None } else { opt("animacao") };
    Some((rank, json!({"id": id, "title": title, "kind": kind, "text": body, "platforms": platforms, "try_it": opt("experimente"), "animation": animation})))
}

/// `state_dir`: a pasta do `aplicados.json`; ilegível é erro, ausente é nada aplicado.
async fn warnings(repo: &Path, target: &str, state_dir: &Path) -> Result<Vec<Value>, ()> {
    let dir = state_dir.to_owned();
    let done = tokio::task::spawn_blocking(move || crate::update::steps::applied(&dir)).await.map_err(|_| ())?
        .map_err(|e| tracing::warn!("avisos da atualização: {e}"))?;
    let mut warnings = Vec::new();
    // Um `git grep` acha os candidatos de uma vez: um `git show` por passo custa caro no Windows.
    // Sem resultado (ou se ele falhar), lê todos, como antes.
    let marked = git(repo, &["grep", "-l", "-i", "-E", "^destrutivo:[[:space:]]*(true|sim|1)[[:space:]]*$", target, "--", "docs/atualizacoes"]).await
        .map(|out| out.lines().filter_map(|l| l.rsplit('/').next()).map(str::to_owned).collect::<HashSet<_>>()).ok();
    for name in list_md(repo, target, "docs/atualizacoes").await? {
        if marked.as_ref().is_some_and(|m| !m.contains(&name)) { continue; }
        if let Some((id, item)) = warning_item(&name, &show(repo, target, &format!("docs/atualizacoes/{name}")).await?)
            && !done.contains(&id) {
            warnings.push(item);
        }
    }
    Ok(warnings)
}

/// O aviso de um passo destrutivo com título, e o id dele (motor `git`).
fn warning_item(name: &str, text: &str) -> Option<(String, Value)> {
    let (f, body) = frontmatter(text);
    let id = f.get("id").filter(|i| !i.is_empty()).cloned().unwrap_or_else(|| name.trim_end_matches(".md").to_owned());
    let destructive = f.get("destrutivo").is_some_and(|v| matches!(v.to_lowercase().as_str(), "true" | "sim" | "1"));
    let title = f.get("titulo").filter(|t| !t.is_empty() && destructive)?;
    Some((id.clone(), json!({"id": id, "title": title, "text": body})))
}

/// O aviso no índice do pacote: só passo destrutivo de máquina que o `steps.rs` roda neste sistema.
/// A regra que o `scripts/montar-pacote.py` repete (casos em `fixtures.json`).
#[cfg(test)]
fn package_warning(name: &str, text: &str, windows: bool) -> Option<(String, Value)> {
    let (fields, body) = crate::update::steps::frontmatter(text);
    let step = crate::update::steps::step(&fields, name.strip_suffix(".md").unwrap_or(name), windows)?;
    (step.machine && step.destructive && !step.command.is_empty())
        .then(|| (step.id.clone(), json!({"id": step.id, "title": step.title, "text": body})))
}

/// Na ordem da tela: novo, melhoria, correção; dentro de cada, pelo id.
fn order_news(mut news: Vec<(u8, String, Value)>) -> Vec<Value> {
    news.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    news.into_iter().map(|n| n.2).collect()
}

/// Novidades que o alvo tem e o instalado não: `*.md` direto em `docs/novidades`, sem README.
async fn news(repo: &Path, target: &str) -> Result<Vec<Value>, ()> {
    // `--no-renames`: arquivo renomeado conta como novo, como na comparação de nomes.
    let added = git(repo, &["diff", "-z", "--no-renames", "--name-only", "--diff-filter=A", "HEAD", target, "--", "docs/novidades"]).await?;
    let mut names: Vec<&str> = added.split('\0').filter_map(|p| p.strip_prefix("docs/novidades/"))
        .filter(|n| !n.contains('/') && is_note_file(n)).collect();
    names.sort();
    let mut news = Vec::new();
    for name in names {
        let id = name.trim_end_matches(".md").to_owned();
        if let Some(item) = news_item(&id, &show(repo, target, &format!("docs/novidades/{name}")).await?) { news.push((item.0, id, item.1)); }
    }
    Ok(order_news(news))
}

pub(crate) async fn collect(cwd: &Path, target: &str, state_dir: &Path) -> Result<Value, ()> {
    let repo = PathBuf::from(git(cwd, &["rev-parse", "--show-toplevel"]).await?.trim());
    let (warnings, news) = tokio::join!(warnings(&repo, target, state_dir), news(&repo, target));
    Ok(json!({"target": target, "warnings": warnings?, "news": news?}))
}

#[cfg(test)]
mod tests;
