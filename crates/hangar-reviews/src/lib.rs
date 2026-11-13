//! Revisões (pull requests e merge requests) dos repositórios que o Hangar conhece, por `gh` e `glab`, sem configuração:
//! os hosts são os dos remotes e o login é o que cada CLI já tem. O servidor só liga as rotas e o cache.
mod cli;
mod github;
mod gitlab;
mod hosts;
pub mod model;

use cli::Cli;
use hosts::{Access, Remote};
use model::{Detail, Summary};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub struct Failure {
    /// Código estável para a tela; o texto do provedor ou do CLI fica no `message`.
    pub code: &'static str,
    pub message: String,
}

impl Failure {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self { Self { code, message: message.into() } }

    /// O status HTTP que a rota responde para esta falha.
    pub fn status(&self) -> u16 {
        match self.code {
            "cli_missing" | "cli_unavailable" | "context_unavailable" => 503,
            "not_found" => 404,
            "bad_repo" | "bad_request" => 422,
            "no_access" => 403,
            _ => 502,
        }
    }
}

/// Host e repositório viram argumento de CLI e caminho de API: só os caracteres que um host e um caminho de repositório
/// têm.
pub fn valid_host(host: &str) -> bool {
    !host.is_empty() && host.len() <= 255 && host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'))
        && !host.starts_with(['-', '.'])
}

pub fn valid_repo(repo: &str) -> bool {
    !repo.is_empty() && repo.len() <= 512 && repo.contains('/') && !repo.starts_with(['/', '-', '.'])
        && !repo.split('/').any(|part| part.is_empty() || part == "." || part == "..")
        && repo.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '/'))
}

/// O que se sabe dos repositórios: os hosts com quantos repositórios cada, e as sessões por (host, repo, branch).
pub struct Known {
    hosts: BTreeMap<String, usize>,
    sessions: HashMap<(String, String, String), Vec<String>>,
    /// Pastas de sessão, para o `branch` aceitar caminho de sessão fora das raízes.
    pub session_dirs: Vec<PathBuf>,
}

/// Monta o `Known` a partir das fontes das worktrees: raízes, pastas recentes e as sessões `(nome, pasta)`. Lê disco.
pub fn known(roots: &[String], cwds: &[String], sessions: Vec<(String, String)>, with_hosts: bool) -> Known {
    let mut hosts: BTreeMap<String, usize> = BTreeMap::new();
    if with_hosts {
        let mut repos = hangar_workspace::worktrees::main_repos(cwds.iter().map(String::as_str));
        repos.extend(hangar_workspace::worktrees::discover_repositories(roots).repos);
        repos.sort();
        repos.dedup();
        for repo in repos {
            for host in hosts::repo_hosts(Path::new(&repo)) { *hosts.entry(host).or_default() += 1; }
        }
    }
    let mut by_branch: HashMap<(String, String, String), Vec<String>> = HashMap::new();
    let mut session_dirs = Vec::new();
    for (name, dir) in sessions {
        session_dirs.push(hangar_workspace::real(Path::new(&dir)));
        if let Some(found) = hosts::branch_remote(Path::new(&dir)) {
            let names = by_branch.entry((found.head.host, found.head.repo.to_ascii_lowercase(), found.branch)).or_default();
            if !names.contains(&name) { names.push(name); }
        }
    }
    Known { hosts, sessions: by_branch, session_dirs }
}

impl Known {
    /// As sessões na branch de origem, no repositório de onde ela vem (o fork, quando é de fork).
    fn attach(&self, item: &mut Summary) {
        item.sessions = self.sessions.get(&(item.host.clone(), item.source_repo.to_ascii_lowercase(), item.source.clone())).cloned().unwrap_or_default();
    }
}

/// Um host achado nos repositórios e quem o atende.
#[derive(Serialize)]
pub struct HostRow {
    pub host: String,
    pub repos: usize,
    /// `ok`, `error`, `no_login`, `cli_missing` ou `unknown`.
    pub state: &'static str,
    pub cli: Option<&'static str>,
    pub user: Option<String>,
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct Listing { pub hosts: Vec<HostRow>, pub reviews: Vec<Summary> }

/// Os PRs e MRs abertos de cada host com login, com as sessões da branch.
pub async fn list(known: &Known) -> Listing {
    let mut tasks = tokio::task::JoinSet::new();
    for (host, repos) in known.hosts.clone() {
        tasks.spawn(async move {
            let access = hosts::access(&host).await;
            let result = match &access {
                Access::Ok { cli: Cli::Gh, user } => Some(github::list(&host, user).await),
                Access::Ok { cli: Cli::Glab, user } => Some(gitlab::list(&host, user).await),
                _ => None,
            };
            (host, repos, access, result)
        });
    }
    let mut rows = Vec::new();
    let mut reviews: Vec<Summary> = Vec::new();
    while let Some(done) = tasks.join_next().await {
        let Ok((host, repos, access, result)) = done else { continue };
        let row = |state, cli: Option<Cli>, user: Option<String>, error| HostRow { host: host.clone(), repos, state, cli: cli.map(Cli::program), user, error };
        rows.push(match (access, result) {
            (Access::Ok { cli, user }, Some(Ok(items))) => { reviews.extend(items); row("ok", Some(cli), Some(user), None) }
            (Access::Ok { cli, user }, Some(Err(e))) => {
                tracing::warn!(host = %host, code = e.code, "revisões: lista falhou");
                row("error", Some(cli), Some(user), Some(e.message))
            }
            (Access::NoLogin { cli, installed }, _) => row(if installed { "no_login" } else { "cli_missing" }, Some(cli), None, None),
            _ => row("unknown", None, None, None),
        });
    }
    rows.sort_by(|a, b| a.host.cmp(&b.host));
    for item in &mut reviews { known.attach(item); }
    reviews.sort_by(|a, b| a.host.cmp(&b.host).then(a.repo.cmp(&b.repo)).then(b.number.cmp(&a.number)));
    Listing { hosts: rows, reviews }
}

/// A revisão da branch de uma pasta: o estado da busca e, quando há, o resumo e (sem `summary_only`) o detalhe.
#[derive(Default, Serialize)]
pub struct BranchState {
    /// `ok`, `none`, `no_login`, `cli_missing`, `unsupported` ou `no_remote`.
    pub state: &'static str,
    pub host: String,
    pub repo: String,
    pub branch: String,
    pub cli: Option<&'static str>,
    pub summary: Option<Summary>,
    pub review: Option<Detail>,
}

pub async fn branch(known: &Known, dir: &Path, summary_only: bool) -> Result<BranchState, Failure> {
    let probe = dir.to_path_buf();
    let found = tokio::task::spawn_blocking(move || hosts::branch_remote(&probe)).await.ok().flatten();
    let Some(hosts::BranchRemote { head: Remote { host, repo: head_repo }, branch, targets }) = found
        else { return Ok(BranchState { state: "no_remote", ..Default::default() }) };
    let mut out = BranchState { host, repo: head_repo.clone(), branch, ..Default::default() };
    let (cli, user) = match hosts::access(&out.host).await {
        Access::Unknown => { out.state = "unsupported"; return Ok(out); }
        Access::NoLogin { cli, installed } => {
            (out.state, out.cli) = (if installed { "no_login" } else { "cli_missing" }, Some(cli.program()));
            return Ok(out);
        }
        Access::Ok { cli, user } => (cli, user),
    };
    out.cli = Some(cli.program());
    let mut summary = None;
    for (index, target) in targets.iter().enumerate() {
        let found = match cli {
            Cli::Gh => github::branch(&out.host, &target.repo, &out.branch, &head_repo).await,
            Cli::Glab => gitlab::branch(&out.host, &target.repo, &out.branch, &head_repo).await,
        };
        summary = match found {
            Ok(found) => found,
            // O repositório da branch responde pela busca; outro remote do host (um `upstream` antigo, sem acesso) só se
            // procura de passagem.
            Err(e) if index > 0 => { tracing::warn!(host = %out.host, repo = %target.repo, code = e.code, "revisões: remote extra sem leitura"); None }
            Err(e) => return Err(e),
        };
        if summary.is_some() { out.repo = target.repo.clone(); break; }
    }
    let Some(mut summary) = summary else { out.state = "none"; return Ok(out) };
    out.state = "ok";
    if !summary_only {
        let mut detail = provider_detail(cli, &out.host, &summary.repo, summary.number, &user).await?;
        known.attach(&mut detail.summary);
        out.review = Some(detail);
    }
    known.attach(&mut summary);
    out.summary = Some(summary);
    Ok(out)
}

async fn provider_detail(cli: Cli, host: &str, repo: &str, number: u64, user: &str) -> Result<Detail, Failure> {
    match cli { Cli::Gh => github::detail(host, repo, number).await, Cli::Glab => gitlab::detail(host, repo, number, user).await }
}

async fn logged(host: &str) -> Result<(Cli, String), Failure> {
    match hosts::access(host).await {
        Access::Ok { cli, user } => Ok((cli, user)),
        _ => Err(Failure::new("no_access", "nenhum CLI com login neste host")),
    }
}

pub async fn detail(known: &Known, host: &str, repo: &str, number: u64) -> Result<Detail, Failure> {
    let (cli, user) = logged(host).await?;
    let mut detail = provider_detail(cli, host, repo, number, &user).await?;
    known.attach(&mut detail.summary);
    Ok(detail)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    pub host: String,
    pub repo: String,
    pub number: u64,
    /// `reply`, `resolve`, `approve`, `merge` ou `auto_merge`.
    pub action: String,
    #[serde(default)]
    pub thread: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
}

/// Faz a ação no provedor. Pedido malformado volta `Err` com `bad_request` antes de qualquer chamada.
pub async fn act(mut a: Action) -> Result<(), Failure> {
    a.host = a.host.to_ascii_lowercase();
    if !valid_host(&a.host) || !valid_repo(&a.repo) { return Err(Failure::new("bad_request", "host ou repositório inválido")); }
    let thread = a.thread.as_deref().filter(|t| !t.is_empty() && t.len() <= 200 && t.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '=')));
    let text = a.body.as_deref().map(str::trim).filter(|t| !t.is_empty());
    let need_thread = || thread.ok_or_else(|| Failure::new("bad_request", "falta a thread"));
    let (cli, _) = logged(&a.host).await?;
    let (host, repo, number) = (a.host.as_str(), a.repo.as_str(), a.number);
    let done = match (a.action.as_str(), cli) {
        ("reply", _) if text.is_none() => return Err(Failure::new("bad_request", "resposta sem texto")),
        ("reply", Cli::Gh) => github::reply(host, need_thread()?, text.unwrap_or_default()).await,
        ("reply", Cli::Glab) => gitlab::reply(host, repo, number, need_thread()?, text.unwrap_or_default()).await,
        ("resolve", Cli::Gh) => github::resolve(host, need_thread()?).await,
        ("resolve", Cli::Glab) => gitlab::resolve(host, repo, number, need_thread()?).await,
        ("approve", Cli::Gh) => github::approve(host, repo, number).await,
        ("approve", Cli::Glab) => gitlab::approve(host, repo, number).await,
        ("merge" | "auto_merge", Cli::Gh) => github::merge(host, repo, number, a.action == "auto_merge").await,
        ("merge" | "auto_merge", Cli::Glab) => gitlab::merge(host, repo, number, a.action == "auto_merge").await,
        _ => return Err(Failure::new("bad_request", "ação desconhecida")),
    };
    done.map_err(|e| {
        if e.message.contains("401") || e.message.to_ascii_lowercase().contains("auth") { hosts::forget(host); }
        Failure::new("action_failed", e.message)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_and_repo_never_become_options_or_traversal() {
        assert!(valid_host("github.com") && valid_host("git.example.dev:8443"));
        assert!(!valid_host("-oProxyCommand=x") && !valid_host("a b") && !valid_host(""));
        assert!(valid_repo("grupo/sub/projeto") && valid_repo("dono/repo.js"));
        assert!(!valid_repo("../etc") && !valid_repo("a//b") && !valid_repo("-x/y") && !valid_repo("semdono") && !valid_repo("a/../b"));
    }

    #[tokio::test]
    async fn malformed_actions_are_refused_before_any_call() {
        let action = |action: &str, thread: Option<&str>, body: Option<&str>| Action { host: "-x".into(), repo: "a/b".into(), number: 1,
            action: action.into(), thread: thread.map(str::to_owned), body: body.map(str::to_owned) };
        assert_eq!(act(action("approve", None, None)).await.unwrap_err().code, "bad_request");
    }
}
