//! Vigia do Python: com o laço dele parado o processo segue vivo, e o systemd (Restart=on-failure)
//! só religa quem morre. Sem resposta por ~1 min, o Rust pede ao systemd que religue o serviço.
use crate::proxy::HttpClient;
use axum::body::Body;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const EVERY: Duration = Duration::from_secs(15);
const TIMEOUT: Duration = Duration::from_secs(5);
const FAILS: u32 = 4;
/// A subida do Python já ficou 117 s sem responder: antes disso o vigia não julga nada.
const GRACE: Duration = Duration::from_secs(180);
/// Entre dois religamentos: uma máquina que trava sempre não vira um laço de reinício.
const MIN_GAP: Duration = Duration::from_secs(15 * 60);

/// O serviço do systemd em que este processo roda (`.../app.slice/hangar-backend.service`); fora dele, nada.
fn unit_from(cgroup: &str) -> Option<String> {
    cgroup.lines().filter_map(|line| line.rsplit('/').next())
        .find(|last| last.ends_with(".service") && !last.starts_with("user@"))
        .map(str::to_owned)
}

/// Marca no futuro (relógio que voltou) conta como recente: na dúvida, não religa.
fn restarted_recently(mark: &Path) -> bool {
    let Ok(at) = std::fs::metadata(mark).and_then(|m| m.modified()) else { return false };
    SystemTime::now().duration_since(at).map_or(true, |age| age < MIN_GAP)
}

async fn ping(http: &HttpClient, upstream: SocketAddr, secret: &str) -> bool {
    let Ok(req) = axum::http::Request::get(format!("http://{upstream}/internal/ping"))
        .header("x-hangar-internal", secret).body(Body::empty()) else { return false };
    matches!(tokio::time::timeout(TIMEOUT, http.request(req)).await, Ok(Ok(resp)) if resp.status().is_success())
}

pub async fn run(http: HttpClient, upstream: SocketAddr, secret: String, home: PathBuf) {
    let Some(unit) = std::fs::read_to_string("/proc/self/cgroup").ok().as_deref().and_then(unit_from) else { return };
    tokio::time::sleep(GRACE).await;
    let mark = home.join(".hangar").join("vigia-religou");
    let mut fails = 0;
    loop {
        tokio::time::sleep(EVERY).await;
        if ping(&http, upstream, &secret).await {
            fails = 0;
            continue;
        }
        fails += 1;
        tracing::warn!(code = "python_ping_failed", fails, "o Python não respondeu ao vigia");
        if fails < FAILS {
            continue;
        }
        if restarted_recently(&mark) {
            tracing::warn!(code = "python_restart_skipped", "Python parado, mas o vigia já religou há menos de 15 min");
            fails = 0;
            continue;
        }
        // Sem a marca não há o intervalo de 15 min: religar assim viraria um laço de reinício.
        if let Err(error) = std::fs::create_dir_all(mark.parent().unwrap_or(&home)).and_then(|_| std::fs::write(&mark, b"")) {
            tracing::error!(code = "python_restart_unguarded", error = %error.kind(), "Python parado, mas sem gravar a marca o vigia não religa");
            fails = 0;
            continue;
        }
        tracing::error!(code = "python_restart", unit = %unit, "Python parado há ~1 min: o vigia pede para religar o serviço");
        // `--no-block`: o próprio vigia mora no serviço que vai cair; esperar o fim seria morrer no meio.
        match tokio::process::Command::new("systemctl").args(["--user", "--no-block", "restart", &unit]).status().await {
            Ok(status) if status.success() => return,
            other => tracing::error!(code = "python_restart_failed", detail = ?other.map(|s| s.code()), "o systemctl não aceitou religar; o vigia segue"),
        }
        fails = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_comes_from_the_service_cgroup_only() {
        assert_eq!(unit_from("0::/user.slice/user-1000.slice/user@1000.service/app.slice/hangar-backend.service\n").as_deref(),
            Some("hangar-backend.service"));
        assert_eq!(unit_from("0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-kitty-123.scope\n"), None);
        assert_eq!(unit_from("0::/user.slice/user-1000.slice/user@1000.service\n"), None);
    }

    #[test]
    fn recent_restart_mark_blocks_another() {
        let dir = tempfile::tempdir().unwrap();
        let mark = dir.path().join("vigia-religou");
        assert!(!restarted_recently(&mark));
        std::fs::write(&mark, b"").unwrap();
        assert!(restarted_recently(&mark));
    }
}
