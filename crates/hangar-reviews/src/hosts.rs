//! De onde saem os hosts: os remotes dos repositórios que o Hangar já descobre (as mesmas fontes das worktrees). Quem
//! atende cada host é o CLI que tem login nele; host que nenhum reconhece fica de fora. Branch e remote são lidos dos
//! arquivos do git, sem processo: a lista sonda uma pasta por sessão.
use super::cli::{self, Cli};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

/// Login confirmado é relido de tempos em tempos; a ausência, mais cedo, para um login novo aparecer logo.
const KNOWN_TTL: Duration = Duration::from_secs(600);
const MISSING_TTL: Duration = Duration::from_secs(90);

/// Um remote lido: o host e o caminho do repositório nele.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Remote { pub host: String, pub repo: String }

/// `https://host/a/b.git`, `ssh://git@host:22/a/b`, `git@host:a/b.git`; caminho local e URL sem caminho não são remote.
pub fn parse_remote(url: &str) -> Option<Remote> {
    let url = url.trim();
    let (host, path) = if let Some((scheme, rest)) = url.split_once("://") {
        if !matches!(scheme.to_ascii_lowercase().as_str(), "https" | "http" | "ssh" | "git" | "git+ssh" | "ssh+git") { return None; }
        let (authority, path) = rest.split_once('/')?;
        let authority = authority.rsplit('@').next()?;
        // A porta do SSH não é a da API; a do HTTPS fica, porque é a do host.
        let host = if scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http") { authority.to_owned() }
            else { authority.split(':').next()?.to_owned() };
        (host, path.to_owned())
    } else {
        // scp: `usuario@host:caminho`, sem barra antes dos dois-pontos (senão é caminho local).
        let (left, path) = url.split_once(':')?;
        if left.contains('/') || left.contains('\\') || path.starts_with('\\') || left.len() == 1 { return None; }
        (left.rsplit('@').next()?.to_owned(), path.to_owned())
    };
    let repo = path.trim_matches('/').trim_end_matches(".git").trim_matches('/').to_owned();
    let host = host.to_ascii_lowercase();
    if host.is_empty() || repo.is_empty() || !repo.contains('/') { return None; }
    Some(Remote { host, repo })
}

/// O que interessa do `config` do git: a URL de cada remote e o remote de cada branch.
#[derive(Debug, Default, PartialEq)]
pub struct Config { pub remotes: Vec<(String, String)>, pub branches: HashMap<String, String> }

pub fn parse_config(text: &str) -> Config {
    enum Section { Remote(String), Branch(String), Other }
    let mut config = Config::default();
    let mut section = Section::Other;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            let quoted = |prefix: &str| line.strip_prefix(prefix).and_then(|r| r.strip_suffix("\"]")).map(str::to_owned);
            section = quoted("[remote \"").map(Section::Remote).or_else(|| quoted("[branch \"").map(Section::Branch)).unwrap_or(Section::Other);
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim().trim_matches('"').to_owned();
        match (&section, key.trim().to_ascii_lowercase().as_str()) {
            (Section::Remote(name), "url") if !config.remotes.iter().any(|(n, _)| n == name) => config.remotes.push((name.clone(), value)),
            (Section::Branch(name), "remote") => { config.branches.insert(name.clone(), value); }
            _ => {}
        }
    }
    config
}

impl Config {
    /// O remote de uma branch (o dela, senão `origin`, senão o primeiro).
    fn remote_for(&self, branch: Option<&str>) -> Option<Remote> {
        let wanted = branch.and_then(|b| self.branches.get(b)).filter(|r| *r != ".").map(String::as_str).unwrap_or("origin");
        self.remotes.iter().find(|(n, _)| n == wanted).or_else(|| self.remotes.first()).and_then(|(_, url)| parse_remote(url))
    }
}

/// A pasta do git de uma pasta de trabalho e a pasta comum (a do `config`): iguais no checkout principal, diferentes
/// numa worktree (`.git` é um arquivo com `gitdir:` e a pasta dela tem `commondir`).
fn git_dirs(root: &Path) -> Option<(PathBuf, PathBuf)> {
    let dot = root.join(".git");
    if dot.is_dir() { return Some((dot.clone(), dot)); }
    let target = std::fs::read_to_string(&dot).ok()?;
    let gitdir = hangar_workspace::worktrees::normpath(&root.join(target.trim().strip_prefix("gitdir: ")?));
    let common = std::fs::read_to_string(gitdir.join("commondir")).ok()
        .map(|c| hangar_workspace::worktrees::normpath(&gitdir.join(c.trim()))).unwrap_or_else(|| gitdir.clone());
    Some((gitdir, common))
}

fn read_config(common: &Path) -> Option<Config> { std::fs::read_to_string(common.join("config")).ok().map(|t| parse_config(&t)) }

/// O remote de um repositório (o de `origin`, senão o primeiro).
pub fn repo_remote(repo: &Path) -> Option<Remote> {
    let (_, common) = git_dirs(repo)?;
    read_config(&common)?.remote_for(None)
}

/// A branch e o remote dela numa pasta de trabalho (worktree ou principal, ou subpasta delas).
pub fn branch_remote(dir: &Path) -> Option<(Remote, String)> {
    let root = PathBuf::from(hangar_workspace::worktrees::repo_root_of(&dir.to_string_lossy())?);
    let (gitdir, common) = git_dirs(&root)?;
    let head = std::fs::read_to_string(gitdir.join("HEAD")).ok()?;
    let branch = head.trim().strip_prefix("ref: refs/heads/").filter(|b| !b.is_empty())?.to_owned();
    let remote = read_config(&common)?.remote_for(Some(&branch))?;
    Some((remote, branch))
}

/// Quem atende um host e com que conta.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    Ok { cli: Cli, user: String },
    /// O host é de um provedor conhecido, mas o CLI dele não tem login ou não está instalado.
    NoLogin { cli: Cli, installed: bool },
    /// Nenhum CLI reconhece o host.
    Unknown,
}

static ACCESS: LazyLock<Mutex<HashMap<String, (Instant, Access)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// O provedor que o nome já entrega, para dizer qual login falta quando nenhum CLI reconhece o host.
fn guess(host: &str) -> Option<Cli> {
    if host == "github.com" || host.ends_with(".ghe.com") || host.starts_with("github.") { Some(Cli::Gh) }
    else if host == "gitlab.com" || host.starts_with("gitlab.") || host.contains(".gitlab.") { Some(Cli::Glab) }
    else { None }
}

pub async fn access(host: &str) -> Access {
    if let Some((at, access)) = ACCESS.lock().unwrap_or_else(|e| e.into_inner()).get(host)
        && at.elapsed() < if matches!(access, Access::Ok { .. }) { KNOWN_TTL } else { MISSING_TTL } {
        return access.clone();
    }
    let found = probe(host).await;
    ACCESS.lock().unwrap_or_else(|e| e.into_inner()).insert(host.to_owned(), (Instant::now(), found.clone()));
    found
}

/// Ação que falhou por credencial: a próxima leitura confere o login de novo.
pub fn forget(host: &str) { ACCESS.lock().unwrap_or_else(|e| e.into_inner()).remove(host); }

async fn probe(host: &str) -> Access {
    // O que o nome sugere vai primeiro: um `auth status` a menos no caso comum.
    let order = match guess(host) { Some(Cli::Glab) => [Cli::Glab, Cli::Gh], _ => [Cli::Gh, Cli::Glab] };
    for cli in order {
        if cli::locate(cli).is_none() { continue; }
        let args = vec!["auth".into(), "status".into(), "--hostname".into(), host.to_owned()];
        if !matches!(cli::status(cli, args, cli::READ).await, Ok(true)) { continue; }
        if let Some(user) = login(cli, host).await { return Access::Ok { cli, user }; }
    }
    match guess(host) {
        Some(cli) => Access::NoLogin { cli, installed: cli::locate(cli).is_some() },
        None => Access::Unknown,
    }
}

async fn login(cli: Cli, host: &str) -> Option<String> {
    let key = match cli { Cli::Gh => "login", Cli::Glab => "username" };
    let out = cli::run(cli, vec!["api".into(), "--hostname".into(), host.into(), "user".into()], cli::READ).await.ok()?;
    serde_json::from_str::<serde_json::Value>(&out).ok()?.get(key)?.as_str().map(str::to_owned).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(host: &str, repo: &str) -> Option<Remote> { Some(Remote { host: host.into(), repo: repo.into() }) }

    #[test]
    fn remotes_of_every_shape_give_host_and_repo() {
        assert_eq!(parse_remote("https://github.com/jeffer1312/hangar.git"), r("github.com", "jeffer1312/hangar"));
        assert_eq!(parse_remote("https://user:tok@gitlab.example.com/a/b/c.git"), r("gitlab.example.com", "a/b/c"));
        assert_eq!(parse_remote("ssh://git@git.example.dev:23/grp/proj.git"), r("git.example.dev", "grp/proj"));
        assert_eq!(parse_remote("git@github.com:dono/repo.git"), r("github.com", "dono/repo"));
        assert_eq!(parse_remote("https://Git.Example.com:8443/x/y/"), r("git.example.com:8443", "x/y"));
    }

    #[test]
    fn local_paths_and_bare_hosts_are_not_remotes() {
        assert_eq!(parse_remote("/srv/git/repo.git"), None);
        assert_eq!(parse_remote("C:\\repos\\x"), None);
        assert_eq!(parse_remote("../outro"), None);
        assert_eq!(parse_remote("https://github.com/"), None);
        assert_eq!(parse_remote("file:///tmp/a/b"), None);
    }

    #[test]
    fn config_gives_each_remote_and_the_remote_of_each_branch() {
        let text = "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = https://github.com/a/b.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n[remote \"fork\"]\n\turl = git@github.com:c/d.git\n[branch \"feat\"]\n\tremote = fork\n";
        let config = parse_config(text);
        assert_eq!(config.remotes.len(), 2);
        assert_eq!(config.remote_for(Some("feat")), r("github.com", "c/d"));
        assert_eq!(config.remote_for(Some("main")), r("github.com", "a/b"));
        assert_eq!(config.remote_for(None), r("github.com", "a/b"));
    }

    #[test]
    fn branch_and_remote_come_from_the_files_of_a_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let (main, wt) = (tmp.path().join("main"), tmp.path().join("wt"));
        std::fs::create_dir_all(main.join(".git/worktrees/wt")).unwrap();
        std::fs::create_dir_all(wt.join("sub")).unwrap();
        std::fs::write(main.join(".git/config"), "[remote \"origin\"]\n\turl = https://gitlab.example.com/g/p.git\n").unwrap();
        std::fs::write(main.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(main.join(".git/worktrees/wt/HEAD"), "ref: refs/heads/feat/x\n").unwrap();
        std::fs::write(main.join(".git/worktrees/wt/commondir"), "../..\n").unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", main.join(".git/worktrees/wt").display())).unwrap();
        assert_eq!(branch_remote(&wt.join("sub")), Some((Remote { host: "gitlab.example.com".into(), repo: "g/p".into() }, "feat/x".into())));
        assert_eq!(branch_remote(&main).map(|(_, b)| b), Some("main".into()));
        assert_eq!(repo_remote(&wt), r("gitlab.example.com", "g/p"));
    }

    #[test]
    fn host_names_suggest_the_cli() {
        assert_eq!(guess("github.com"), Some(Cli::Gh));
        assert_eq!(guess("gitlab.example.dev"), Some(Cli::Glab));
        assert_eq!(guess("git.acme.dev"), None);
    }
}
