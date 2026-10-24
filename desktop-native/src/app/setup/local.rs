//! Entrada (spec "Entrada"): acha a pasta do Hangar pelo `WorkingDirectory` da unit `hangar-backend` (Linux) ou da ação da
//! tarefa `hangar-backend` (Windows), sem nada tenta `~/hangar`, lê porta e token do `.env` (`env_file`) e testa se responde.
//! Não existe `/api/health`: `GET /api/sessions` com 401 já prova que é um Hangar.
use std::path::{Path, PathBuf};
use crate::api::Api;

pub(crate) const DEFAULT_PORT: u16 = 8765;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LocalInstall { pub dir: PathBuf, pub port: u16, pub token: Option<String> }

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

/// A pasta do serviço vale se ainda for um clone; senão `~/hangar`, se for. Nunca inventa uma pasta.
pub(crate) fn pick_dir(service: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    service.filter(|d| d.join("backend").is_dir()).or_else(|| home.filter(|d| d.join("backend").is_dir()))
}

pub(crate) fn default_dir() -> Option<PathBuf> { std::env::home_dir().map(|home| home.join("hangar")) }

/// Pasta do Hangar deste computador. `HANGAR_SETUP_DEST` é só para provar telas sem tocar na instalação real.
pub(crate) fn find_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("HANGAR_SETUP_DEST").map(PathBuf::from) { return Some(dir); }
    pick_dir(service_dir(), default_dir())
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
/// A mesma regra do `user_data::path` do `hangar-server`, que este crate não importa.
pub(crate) fn env_file(home: Option<&Path>, dir: &Path) -> PathBuf {
    home.map(|home| home.join(".hangar").join("config").join(".env")).filter(|new| new.exists())
        .unwrap_or_else(|| dir.join("backend").join(".env"))
}

pub(crate) fn read_install(dir: &Path) -> LocalInstall { read_install_in(std::env::home_dir().as_deref(), dir) }

fn read_install_in(home: Option<&Path>, dir: &Path) -> LocalInstall {
    let (port, token) = std::fs::read_to_string(env_file(home, dir)).map(|text| parse_env(&text)).unwrap_or((None, None));
    LocalInstall { dir: dir.to_owned(), port: port.unwrap_or(DEFAULT_PORT), token }
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
        LocalInstall { dir: PathBuf::from("/home/dev/hangar"), port: 8765, token: token.map(str::to_owned) }
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
    fn read_install_defaults_the_port() {
        let root = std::env::temp_dir().join(format!("hangar-local-env-{}", std::process::id()));
        let (home, dir) = (root.join("home"), root.join("hangar"));
        std::fs::create_dir_all(dir.join("backend")).unwrap();
        assert_eq!(read_install_in(Some(&home), &dir), LocalInstall { dir: dir.clone(), port: DEFAULT_PORT, token: None });
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
        assert_eq!(read_install_in(Some(&home), &dir), LocalInstall { dir: dir.clone(), port: 9100, token: Some("novo".into()) });
        assert_eq!(env_file(Some(&home), &dir), new);
        // Os dois: vale o novo.
        std::fs::write(dir.join("backend/.env"), "CP_PORT=9000\nCP_AUTH_TOKEN=velho\n").unwrap();
        assert_eq!(read_install_in(Some(&home), &dir), LocalInstall { dir: dir.clone(), port: 9100, token: Some("novo".into()) });
        // Só o antigo: como hoje.
        std::fs::remove_file(&new).unwrap();
        assert_eq!(read_install_in(Some(&home), &dir), LocalInstall { dir: dir.clone(), port: 9000, token: Some("velho".into()) });
        assert_eq!(env_file(Some(&home), &dir), dir.join("backend/.env"));
        let _ = std::fs::remove_dir_all(root);
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
