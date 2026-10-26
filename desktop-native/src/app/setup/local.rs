//! Entrada (spec "Entrada"): acha a pasta do Hangar pelo `WorkingDirectory` da unit `hangar-backend` (Linux) ou da ação da
//! tarefa `hangar-backend` (Windows), sem nada tenta `~/hangar`, lê porta e token do `.env` (`env_file`) e testa se responde.
//! Não existe `/api/health`: `GET /api/sessions` com 401 já prova que é um Hangar.
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use crate::api::Api;

pub(crate) const DEFAULT_PORT: u16 = 8765;

/// Como o assistente instala: clone do git (`dir` = o clone) ou o pacote por versão (`dir` = `~/.hangar`).
/// `state.json` sem o campo é de antes do pacote: clone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Mode { #[default] Git, Package }

/// Onde o CI publica o pacote; nos demais sistemas (macOS) só existe o clone.
pub(crate) const PACKAGE_OS: bool = cfg!(all(target_arch = "x86_64", any(target_os = "linux", windows)));

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LocalInstall { pub mode: Mode, pub dir: PathBuf, pub port: u16, pub token: Option<String> }

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Found {
    /// Respondeu com o token do `.env`: conectar sem digitar nada.
    Ready { install: LocalInstall, address: String, token: String },
    /// É um Hangar, mas o token não está legível ou não vale: o cartão de sempre com o endereço preenchido.
    NeedsToken { install: Option<LocalInstall>, address: String },
    /// Nada respondeu; `install` é a pasta achada, onde o assistente roda.
    Silent { install: Option<LocalInstall> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Answer { Accepted, Refused, Silent }

pub(crate) fn address(port: u16) -> String { format!("http://127.0.0.1:{port}") }

/// O mesmo que o `install-native` lê (`grep … | tail -1`): a última linha de cada chave vale.
pub(crate) fn parse_env(text: &str) -> (Option<u16>, Option<String>) {
    let (mut port, mut token) = (None, None);
    for line in text.lines() {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
        match key.trim() {
            "CP_PORT" => if let Ok(p) = value.parse() { port = Some(p) },
            "CP_AUTH_TOKEN" if !value.is_empty() => token = Some(value.to_owned()),
            _ => {}
        }
    }
    (port, token)
}

/// O `WorkingDirectory` é `<pasta>/backend`; a pasta do Hangar é a de cima.
pub(crate) fn repo_from_backend_dir(raw: &str) -> Option<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() { return None; }
    let dir = PathBuf::from(raw);
    match dir.file_name() {
        Some(name) if name == "backend" => dir.parent().map(Path::to_path_buf),
        _ => Some(dir),
    }
}

/// Versão extraída do pacote: `manifest.json` sem `.git` (a regra do `install.sh`).
fn is_package(dir: &Path) -> bool { dir.join("manifest.json").is_file() && !dir.join(".git").exists() }

/// A pasta do serviço vale se ainda for um clone; senão `~/hangar`, se for. Nunca inventa uma pasta.
pub(crate) fn pick_dir(service: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    // A tarefa do Windows no pacote aponta para `current/backend`, que também tem `backend/`.
    let checkout = |d: &PathBuf| d.join("backend").is_dir() && !is_package(d);
    service.filter(checkout).or_else(|| home.filter(checkout))
}

pub(crate) fn default_dir() -> Option<PathBuf> { std::env::home_dir().map(|home| home.join("hangar")) }

/// Instalação existente e como ela é. `forced` (`HANGAR_SETUP_DEST`) é sempre clone. O pacote vem antes de `~/hangar`:
/// o clone que sobrou da migração não é o que o serviço roda.
pub(crate) fn pick(forced: Option<PathBuf>, service: Option<PathBuf>, home: Option<&Path>) -> Option<(Mode, PathBuf)> {
    if let Some(dir) = forced { return Some((Mode::Git, dir)); }
    let package = home.map(|h| h.join(".hangar")).filter(|d| is_package(&d.join("current")));
    // Modo desenvolvedor: o `current` aponta para o checkout que o serviço roda (a unit fica em `~/.hangar`).
    let developer = || home.and_then(|h| std::fs::canonicalize(h.join(".hangar").join("current")).ok())
        .map(|d| PathBuf::from(crate::app::disk::plain_path(&d.to_string_lossy())));
    pick_dir(service, None).map(|d| (Mode::Git, d))
        .or_else(|| package.map(|d| (Mode::Package, d)))
        .or_else(|| pick_dir(developer(), None).map(|d| (Mode::Git, d)))
        .or_else(|| pick_dir(None, home.map(|h| h.join("hangar"))).map(|d| (Mode::Git, d)))
}

/// Máquina sem Hangar: o pacote onde ele é publicado; nos demais, o clone em `~/hangar`.
pub(crate) fn fresh(home: &Path, package_os: bool) -> (Mode, PathBuf) {
    if package_os { (Mode::Package, home.join(".hangar")) } else { (Mode::Git, home.join("hangar")) }
}

/// Depois de uma execução no pacote: canal sem pacote para este sistema faz o bootstrap cair no clone em `~/hangar`, e vale
/// o que ficou instalado. Nada achado mantém a decisão; o clone nunca muda.
pub(crate) fn settle(launched: (Mode, PathBuf), found: Option<(Mode, PathBuf)>) -> (Mode, PathBuf) {
    if launched.0 == Mode::Package { found.unwrap_or(launched) } else { launched }
}

/// Só o disco, sem perguntar ao serviço: roda na thread da janela, e a queda do bootstrap é sempre para `~/hangar`.
pub(crate) fn installed_now() -> Option<(Mode, PathBuf)> { pick(None, None, std::env::home_dir().as_deref()) }

/// Onde o assistente instala quando nada foi achado.
pub(crate) fn fresh_here() -> (Mode, PathBuf) { std::env::home_dir().map_or((Mode::Git, PathBuf::new()), |home| fresh(&home, PACKAGE_OS)) }

/// Instalação deste computador. `HANGAR_SETUP_DEST` é só para provar telas sem tocar na instalação real.
pub(crate) fn find_install() -> Option<LocalInstall> {
    pick(std::env::var_os("HANGAR_SETUP_DEST").map(PathBuf::from), service_dir(), std::env::home_dir().as_deref())
        .map(|(mode, dir)| LocalInstall { mode, ..read_install(&dir) })
}

#[cfg(target_os = "linux")]
fn service_dir() -> Option<PathBuf> {
    let out = super::system::hidden(&mut std::process::Command::new("systemctl"))
        .args(["--user", "show", "hangar-backend.service", "-p", "WorkingDirectory", "--value"]).output().ok()?;
    repo_from_backend_dir(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(windows)]
fn service_dir() -> Option<PathBuf> {
    let out = super::system::powershell("(Get-ScheduledTask -TaskName 'hangar-backend' -ErrorAction SilentlyContinue).Actions[0].WorkingDirectory").ok()?;
    repo_from_backend_dir(&out)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn service_dir() -> Option<PathBuf> { None }

/// O `.env` do servidor: `~/.hangar/config/.env` quando existe (máquina migrada), senão o da `backend/`.
/// A mesma regra do `user_data::path` do `hangar-server`, que este crate não importa. Disco que não deixa
/// conferir o lugar novo é erro, com o caminho: cair no da `backend/` leria o token errado.
pub(crate) fn env_file(home: Option<&Path>, dir: &Path) -> Result<PathBuf, String> {
    if let Some(new) = home.map(|home| home.join(".hangar").join("config").join(".env")) {
        match new.try_exists() {
            Ok(true) => return Ok(new),
            Ok(false) => {}
            Err(e) => return Err(format!("{}: {e}", new.display())),
        }
    }
    Ok(dir.join("backend").join(".env"))
}

/// Porta e token; o modo é de quem achou a instalação (`find_install`).
pub(crate) fn read_install(dir: &Path) -> LocalInstall { read_install_in(std::env::home_dir().as_deref(), dir) }

/// `.env` que não se confere fica sem porta nem token; quem mostra o motivo é o `env_file` da tela.
fn read_install_in(home: Option<&Path>, dir: &Path) -> LocalInstall {
    let (port, token) = env_file(home, dir).ok().and_then(|env| std::fs::read_to_string(env).ok()).map(|text| parse_env(&text)).unwrap_or((None, None));
    LocalInstall { mode: Mode::default(), dir: dir.to_owned(), port: port.unwrap_or(DEFAULT_PORT), token }
}

pub(crate) fn classify(install: Option<LocalInstall>, answer: Answer) -> Found {
    let address = address(install.as_ref().map_or(DEFAULT_PORT, |i| i.port));
    match (answer, install) {
        (Answer::Accepted, Some(install)) if install.token.is_some() => {
            let token = install.token.clone().unwrap_or_default();
            Found::Ready { install, address, token }
        }
        (Answer::Accepted | Answer::Refused, install) => Found::NeedsToken { install, address },
        (Answer::Silent, install) => Found::Silent { install },
    }
}

/// Sem pasta achada ainda testa a porta padrão: um Hangar rodando de outro jeito também conta.
pub(crate) async fn probe(install: Option<LocalInstall>) -> Found {
    let port = install.as_ref().map_or(DEFAULT_PORT, |i| i.port);
    let token = install.as_ref().and_then(|i| i.token.clone()).unwrap_or_default();
    let answer = match Api::new(&address(port), &token) {
        Err(_) => Answer::Silent,
        Ok(api) => match api.sessions().await {
            Ok(_) => Answer::Accepted,
            Err(error) if matches!(error.status, Some(401 | 403)) => Answer::Refused,
            Err(_) => Answer::Silent,
        },
    };
    classify(install, answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install(token: Option<&str>) -> LocalInstall {
        LocalInstall { mode: Mode::Git, dir: PathBuf::from("/home/dev/hangar"), port: 8765, token: token.map(str::to_owned) }
    }

    #[test]
    fn env_reads_port_and_token_like_the_install_scripts() {
        let text = "# CP_PORT=1\nCP_PORT=9000\nexport CP_AUTH_TOKEN=\"abc 1\"\nCP_AUTH_TOKEN=def\nOUTRA=x\n";
        assert_eq!(parse_env(text), (Some(9000), Some("def".to_owned())));
        assert_eq!(parse_env("CP_AUTH_TOKEN=\nCP_PORT=porta\n"), (None, None));
    }

    #[test]
    fn unit_working_directory_points_at_backend() {
        assert_eq!(repo_from_backend_dir("/home/dev/Projetos/hangar/backend\n"), Some(PathBuf::from("/home/dev/Projetos/hangar")));
        assert_eq!(repo_from_backend_dir("C:\\Users\\dev\\hangar\\backend"), Some(PathBuf::from("C:\\Users\\dev\\hangar")).filter(|_| cfg!(windows))
            .or(Some(PathBuf::from("C:\\Users\\dev\\hangar\\backend"))));
        assert_eq!(repo_from_backend_dir("  \n"), None);
    }

    #[test]
    fn service_dir_wins_over_home_hangar() {
        let root = std::env::temp_dir().join(format!("hangar-local-{}", std::process::id()));
        let (service, home) = (root.join("Projetos/hangar"), root.join("hangar"));
        std::fs::create_dir_all(service.join("backend")).unwrap();
        std::fs::create_dir_all(home.join("backend")).unwrap();
        assert_eq!(pick_dir(Some(service.clone()), Some(home.clone())), Some(service.clone()));
        // Unit que aponta para pasta apagada não vale: cai em ~/hangar se ele for um clone.
        assert_eq!(pick_dir(Some(root.join("sumiu")), Some(home.clone())), Some(home.clone()));
        std::fs::remove_dir_all(home.join("backend")).unwrap();
        assert_eq!(pick_dir(None, Some(home)), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn mode_follows_what_is_installed() {
        let root = std::env::temp_dir().join(format!("hangar-local-mode-{}", std::process::id()));
        let home = root.join("home");
        let (checkout, package) = (home.join("hangar"), home.join(".hangar"));
        std::fs::create_dir_all(&home).unwrap();
        // Nada instalado: pacote onde ele é publicado, clone em `~/hangar` nos demais.
        assert_eq!(pick(None, None, Some(&home)), None);
        assert_eq!(fresh(&home, true), (Mode::Package, package.clone()));
        assert_eq!(fresh(&home, false), (Mode::Git, checkout.clone()));
        // Pacote: `current` com `manifest.json` e sem `.git`.
        std::fs::create_dir_all(package.join("current/backend")).unwrap();
        std::fs::write(package.join("current/manifest.json"), "{}").unwrap();
        assert_eq!(pick(None, None, Some(&home)), Some((Mode::Package, package.clone())));
        // A tarefa do Windows aponta para `current/backend`: a versão extraída nunca vira clone.
        assert_eq!(pick(None, Some(package.join("current")), Some(&home)), Some((Mode::Package, package.clone())));
        // Sobra de `~/hangar` numa máquina migrada não tira do pacote: o serviço não aponta para ela.
        std::fs::create_dir_all(checkout.join("backend")).unwrap();
        assert_eq!(pick(None, None, Some(&home)), Some((Mode::Package, package.clone())));
        // Serviço num clone vence; `HANGAR_SETUP_DEST` é sempre clone.
        let service = root.join("Projetos/hangar");
        std::fs::create_dir_all(service.join("backend")).unwrap();
        assert_eq!(pick(None, Some(service.clone()), Some(&home)), Some((Mode::Git, service)));
        assert_eq!(pick(Some(root.join("prova")), None, Some(&home)), Some((Mode::Git, root.join("prova"))));
        // Modo desenvolvedor: `current` num checkout (`.git`) não é pacote; é o clone, e vence a sobra de `~/hangar`.
        std::fs::create_dir_all(package.join("current/.git")).unwrap();
        assert_eq!(pick(None, None, Some(&home)), Some((Mode::Git, package.join("current"))));
        std::fs::remove_dir_all(package.join("current/backend")).unwrap();
        assert_eq!(pick(None, None, Some(&home)), Some((Mode::Git, checkout.clone())));
        std::fs::remove_dir_all(checkout.join("backend")).unwrap();
        assert_eq!(pick(None, None, Some(&home)), None);
        // O `.env` do pacote é o de `~/.hangar/config`.
        std::fs::create_dir_all(package.join("config")).unwrap();
        std::fs::write(package.join("config/.env"), "CP_AUTH_TOKEN=pac\n").unwrap();
        assert_eq!(read_install_in(Some(&home), &package).token.as_deref(), Some("pac"));
        let _ = std::fs::remove_dir_all(root);
    }

    /// Modo desenvolvedor migrado: a unit roda em `~/.hangar` e o `current` aponta para um checkout fora de
    /// `~/hangar`. A instalação é esse checkout, e o token vem de `~/.hangar/config/.env`.
    #[cfg(unix)]
    #[test]
    fn developer_mode_current_is_the_checkout_it_points_to() {
        let root = std::env::temp_dir().join(format!("hangar-local-dev-{}", std::process::id()));
        let (home, checkout) = (root.join("home"), root.join("Projetos/hangar"));
        let hangar = home.join(".hangar");
        std::fs::create_dir_all(checkout.join(".git")).unwrap();
        std::fs::create_dir_all(checkout.join("backend")).unwrap();
        std::fs::create_dir_all(hangar.join("config")).unwrap();
        std::fs::write(hangar.join("config/.env"), "CP_AUTH_TOKEN=dev\n").unwrap();
        std::os::unix::fs::symlink(&checkout, hangar.join("current")).unwrap();
        let picked = pick(None, repo_from_backend_dir(&hangar.to_string_lossy()), Some(&home));
        let token = read_install_in(Some(&home), &checkout).token;
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(picked, Some((Mode::Git, checkout)));
        assert_eq!(token.as_deref(), Some("dev"));
    }

    #[test]
    fn after_a_run_what_got_installed_wins_over_the_package_decision() {
        let (package, clone) = ((Mode::Package, PathBuf::from("/h/.hangar")), (Mode::Git, PathBuf::from("/h/hangar")));
        // Canal sem pacote: o bootstrap instalou o clone.
        assert_eq!(settle(package.clone(), Some(clone.clone())), clone);
        assert_eq!(settle(package.clone(), Some(package.clone())), package);
        // Nada instalado ainda: a decisão fica.
        assert_eq!(settle(package.clone(), None), package);
        // O clone nunca muda.
        assert_eq!(settle(clone.clone(), Some(package.clone())), clone);
        assert_eq!(settle(clone.clone(), None), clone);
    }

    #[test]
    fn read_install_defaults_the_port() {
        let root = std::env::temp_dir().join(format!("hangar-local-env-{}", std::process::id()));
        let (home, dir) = (root.join("home"), root.join("hangar"));
        std::fs::create_dir_all(dir.join("backend")).unwrap();
        assert_eq!(read_install_in(Some(&home), &dir), LocalInstall { mode: Mode::Git, dir: dir.clone(), port: DEFAULT_PORT, token: None });
        std::fs::write(dir.join("backend/.env"), "CP_AUTH_TOKEN=t0k\n").unwrap();
        assert_eq!(read_install_in(Some(&home), &dir).token.as_deref(), Some("t0k"));
        assert_eq!(read_install_in(None, &dir).token.as_deref(), Some("t0k"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn read_install_prefers_the_user_data_env() {
        let root = std::env::temp_dir().join(format!("hangar-local-userdata-{}", std::process::id()));
        let (home, dir) = (root.join("home"), root.join("hangar"));
        let new = home.join(".hangar/config/.env");
        std::fs::create_dir_all(new.parent().unwrap()).unwrap();
        std::fs::create_dir_all(dir.join("backend")).unwrap();
        // Só no lugar novo: máquina migrada, sem `.env` na `backend/`.
        std::fs::write(&new, "CP_PORT=9100\nCP_AUTH_TOKEN=novo\n").unwrap();
        let git = |port, token: &str| LocalInstall { mode: Mode::Git, dir: dir.clone(), port, token: Some(token.into()) };
        assert_eq!(read_install_in(Some(&home), &dir), git(9100, "novo"));
        assert_eq!(env_file(Some(&home), &dir), Ok(new.clone()));
        // Os dois: vale o novo.
        std::fs::write(dir.join("backend/.env"), "CP_PORT=9000\nCP_AUTH_TOKEN=velho\n").unwrap();
        assert_eq!(read_install_in(Some(&home), &dir), git(9100, "novo"));
        // Só o antigo: como hoje.
        std::fs::remove_file(&new).unwrap();
        assert_eq!(read_install_in(Some(&home), &dir), git(9000, "velho"));
        assert_eq!(env_file(Some(&home), &dir), Ok(dir.join("backend/.env")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn env_file_that_cannot_be_checked_is_an_error_not_the_old_place() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("hangar-local-stat-{}", std::process::id()));
        let (home, dir) = (root.join("home"), root.join("hangar"));
        let config = home.join(".hangar/config");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o000)).unwrap();
        let (read, is_root) = (env_file(Some(&home), &dir), std::fs::read_dir(&config).is_ok());
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o700)).unwrap();
        let _ = std::fs::remove_dir_all(&root);
        if is_root { return; } // root lê tudo: o caso não se monta
        let error = read.unwrap_err();
        assert!(error.contains(".hangar/config/.env"), "{error}");
    }

    #[test]
    fn classify_follows_the_spec_entry() {
        let address = "http://127.0.0.1:8765".to_owned();
        assert_eq!(classify(Some(install(Some("t"))), Answer::Accepted),
            Found::Ready { install: install(Some("t")), address: address.clone(), token: "t".into() });
        // 401 prova que é um Hangar, mas sem token aceito o cartão abre com o endereço preenchido.
        assert_eq!(classify(Some(install(Some("velho"))), Answer::Refused), Found::NeedsToken { install: Some(install(Some("velho"))), address: address.clone() });
        assert_eq!(classify(None, Answer::Refused), Found::NeedsToken { install: None, address: address.clone() });
        assert_eq!(classify(Some(install(None)), Answer::Accepted), Found::NeedsToken { install: Some(install(None)), address });
        assert_eq!(classify(Some(install(Some("t"))), Answer::Silent), Found::Silent { install: Some(install(Some("t"))) });
    }
}
