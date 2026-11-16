//! Vigia do Python: com o laço dele parado o processo segue vivo, e o systemd (Restart=on-failure)
//! só religa quem morre. Sem resposta por ~1 min, o Rust pede ao systemd que religue o serviço.
//! Sem o Python, quem vigia é o próprio systemd pelo `WatchdogSec` (`notify_alive`).
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

type ProbeFuture = std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>>;
pub type Probe = Box<dyn Fn() -> ProbeFuture + Send + Sync>;

pub fn probe<F: std::future::Future<Output = bool> + Send + 'static>(make: impl Fn() -> F + Send + Sync + 'static) -> Probe {
    Box::new(move || -> ProbeFuture { Box::pin(make()) })
}

/// Com `WatchdogSec`, o systemd religa quem parar de avisar. O aviso só sai quando todas as provas
/// responderam no prazo: um relógio solto seguiria avisando com o servidor travado.
pub async fn notify_alive(notifier: hangar_boot::notify::Notifier, every: Duration, probes: Vec<(&'static str, Probe)>) {
    let limit = TIMEOUT.min(every);
    loop {
        match late_probe(&probes, limit).await {
            None => if let Err(error) = notifier.send("WATCHDOG=1") {
                tracing::warn!(code = "systemd_watchdog_send_failed", kind = ?error.kind(), "aviso de vida ao systemd não saiu");
            },
            Some(probe) => tracing::warn!(code = "systemd_watchdog_late", probe, "prova de vida atrasada; o aviso ao systemd não sai"),
        }
        tokio::time::sleep(every).await;
    }
}

/// A primeira prova que não respondeu `true` dentro de `limit`.
pub async fn late_probe(probes: &[(&'static str, Probe)], limit: Duration) -> Option<&'static str> {
    for (name, probe) in probes {
        if !matches!(tokio::time::timeout(limit, probe()).await, Ok(true)) { return Some(name); }
    }
    None
}

/// As provas do servidor: a saúde pela porta pública (aceitar e rotear), uma volta pelo pool
/// bloqueante (preso, nenhuma leitura de disco anda) e o registro de sessões livre. A lista ao vivo
/// fica de fora: ela só gira com alguém olhando.
pub fn server_probes(listen: SocketAddr, state: std::sync::Arc<crate::state::live::StateEnv>) -> Vec<(&'static str, Probe)> {
    let address = probe_address(listen);
    vec![
        ("health", probe(move || health_answers(address, TIMEOUT))),
        ("blocking_pool", probe(|| async { tokio::task::spawn_blocking(|| ()).await.is_ok() })),
        ("runtime_registry", probe(move || {
            let state = state.clone();
            async move {
                if let Some(registry) = state.runtime.get() { registry.responsive().await; }
                true
            }
        })),
    ]
}

/// Quem escuta em todas as interfaces responde no loopback da mesma família.
pub fn probe_address(listen: SocketAddr) -> SocketAddr {
    match listen.ip() {
        std::net::IpAddr::V4(ip) if ip.is_unspecified() => SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, listen.port())),
        std::net::IpAddr::V6(ip) if ip.is_unspecified() => SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, listen.port())),
        _ => listen,
    }
}

async fn health_answers(addr: SocketAddr, limit: Duration) -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let exchange = async {
        let mut stream = tokio::net::TcpStream::connect(addr).await.ok()?;
        stream.write_all(b"GET /__hangar_server/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").await.ok()?;
        let mut head = [0u8; 12];
        stream.read_exact(&mut head).await.ok()?;
        Some(head.starts_with(b"HTTP/1.1 200"))
    };
    matches!(tokio::time::timeout(limit, exchange).await, Ok(Some(true)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_goes_to_loopback_of_the_same_family() {
        assert_eq!(probe_address("0.0.0.0:8765".parse().unwrap()), "127.0.0.1:8765".parse().unwrap());
        assert_eq!(probe_address("[::]:8765".parse().unwrap()), "[::1]:8765".parse().unwrap());
        assert_eq!(probe_address("192.168.0.10:8765".parse().unwrap()), "192.168.0.10:8765".parse().unwrap());
    }

    fn fixed(answer: Option<bool>) -> Probe {
        probe(move || async move {
            match answer { Some(answer) => answer, None => std::future::pending().await }
        })
    }

    #[tokio::test]
    async fn p2_a_wedged_or_failing_probe_withholds_the_watchdog() {
        let limit = Duration::from_millis(50);
        assert_eq!(late_probe(&[("a", fixed(Some(true))), ("b", fixed(Some(true)))], limit).await, None);
        assert_eq!(late_probe(&[("a", fixed(Some(true))), ("preso", fixed(None))], limit).await, Some("preso"));
        assert_eq!(late_probe(&[("falhou", fixed(Some(false))), ("b", fixed(Some(true)))], limit).await, Some("falhou"));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn p2_wedged_component_sends_no_watchdog_and_a_healthy_one_does() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notify.sock");
        let socket = std::os::unix::net::UnixDatagram::bind(&path).unwrap();
        socket.set_nonblocking(true).unwrap();
        let text = path.to_string_lossy().into_owned();
        let notifier = hangar_boot::notify::Notifier::from_lookup(move |key| (key == "NOTIFY_SOCKET").then(|| text.clone()), 1);
        let every = Duration::from_millis(20);
        let wedged = tokio::spawn(notify_alive(notifier.clone(), every, vec![("preso", fixed(None))]));
        tokio::time::sleep(Duration::from_millis(300)).await;
        wedged.abort();
        let mut buffer = [0u8; 64];
        assert!(socket.recv(&mut buffer).is_err(), "componente preso não pode mandar WATCHDOG=1");
        let healthy = tokio::spawn(notify_alive(notifier, every, vec![("ok", fixed(Some(true)))]));
        socket.set_nonblocking(false).unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let read = tokio::task::spawn_blocking(move || { let n = socket.recv(&mut buffer).unwrap(); buffer[..n].to_vec() }).await.unwrap();
        healthy.abort();
        assert_eq!(read, b"WATCHDOG=1");
    }

    #[tokio::test]
    async fn health_probe_needs_a_200() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for answer in [&b"HTTP/1.1 200 OK\r\n\r\n"[..], &b"HTTP/1.1 503 Service Unavailable\r\n\r\n"[..]] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 256];
                let _ = socket.read(&mut request).await;
                socket.write_all(answer).await.unwrap();
            }
        });
        assert!(health_answers(addr, Duration::from_secs(5)).await);
        assert!(!health_answers(addr, Duration::from_secs(5)).await);
        server.await.unwrap();
        // Ninguém escutando: não há aviso.
        assert!(!health_answers(addr, Duration::from_secs(5)).await);
    }

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
