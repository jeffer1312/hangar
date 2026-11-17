//! Vigia do servidor para o systemd: com `WatchdogSec`, quem parar de avisar é religado.
use std::net::SocketAddr;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

type ProbeFuture = std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>>;
pub type Probe = Box<dyn Fn() -> ProbeFuture + Send + Sync>;

pub fn probe<F: std::future::Future<Output = bool> + Send + 'static>(make: impl Fn() -> F + Send + Sync + 'static) -> Probe {
    Box::new(move || -> ProbeFuture { Box::pin(make()) })
}

/// Carência depois do pronto: a subida numa máquina cheia ainda prepara as sessões, e nada é julgado.
pub const GRACE: Duration = Duration::from_secs(180);
/// Rodadas falhas seguidas antes de reter o aviso: uma rodada lenta sob carga (build saturando a
/// máquina, pool bloqueante ocupado) não vira um laço de reinício.
pub const FAILURES: u32 = 4;

/// Quando reter o aviso ao systemd. O antigo vigia do Python também esperava 15 min entre dois
/// religamentos; aqui quem espaça é a espera crescente da unit (`RestartSteps`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy { pub grace: Duration, pub failures: u32 }

impl Policy {
    pub const SERVER: Policy = Policy { grace: GRACE, failures: FAILURES };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Ainda na carência: avisa sem provar.
    Grace,
    Passed,
    /// A rodada falhou, mas ainda não são `failures` seguidas: avisa.
    Tolerated(u32),
    /// `failures` rodadas seguidas falharam: o aviso não sai, e o systemd religa pelo prazo.
    Withheld(u32),
}

/// As rodadas desde o pronto, com as falhas seguidas; a que passa zera a conta.
#[derive(Debug)]
pub struct Rounds { policy: Policy, ready: std::time::Instant, failed: u32 }

impl Rounds {
    pub fn new(policy: Policy, ready: std::time::Instant) -> Self { Self { policy, ready, failed: 0 } }

    pub fn in_grace(&self, now: std::time::Instant) -> bool { now.saturating_duration_since(self.ready) < self.policy.grace }

    /// O veredito da rodada de `now`; `passed` é `None` na carência, quando nada é provado.
    pub fn round(&mut self, now: std::time::Instant, passed: Option<bool>) -> Verdict {
        if self.in_grace(now) { return Verdict::Grace; }
        match passed {
            None | Some(true) => { self.failed = 0; Verdict::Passed }
            Some(false) => {
                self.failed = self.failed.saturating_add(1);
                if self.failed < self.policy.failures { Verdict::Tolerated(self.failed) } else { Verdict::Withheld(self.failed) }
            }
        }
    }
}

impl Verdict {
    pub fn sends(self) -> bool { !matches!(self, Verdict::Withheld(_)) }
}

/// Com `WatchdogSec`, o systemd religa quem parar de avisar. Depois da carência o aviso só sai
/// enquanto as provas respondem no prazo, tolerando `failures - 1` rodadas falhas seguidas: um
/// relógio solto seguiria avisando com o servidor travado.
pub async fn notify_alive(notifier: hangar_boot::notify::Notifier, every: Duration, probes: Vec<(&'static str, Probe)>, policy: Policy) {
    let limit = TIMEOUT.min(every);
    let mut rounds = Rounds::new(policy, std::time::Instant::now());
    loop {
        let round = std::time::Instant::now();
        let late = if rounds.in_grace(round) { None } else { Some(late_probe(&probes, limit).await) };
        let ms = u64::try_from(round.elapsed().as_millis()).unwrap_or(u64::MAX);
        let verdict = rounds.round(std::time::Instant::now(), late.map(|late| late.is_none()));
        let probe = late.flatten();
        match verdict {
            Verdict::Tolerated(failed) => tracing::warn!(code = "systemd_watchdog_late", probe, failed, limit = policy.failures,
                "prova de vida atrasada; o aviso ao systemd ainda sai"),
            Verdict::Withheld(failed) => tracing::error!(code = "systemd_watchdog_withheld", probe, failed,
                "provas de vida atrasadas em rodadas seguidas; o aviso ao systemd não sai e ele religa pelo prazo"),
            Verdict::Grace | Verdict::Passed => {}
        }
        if verdict.sends() && let Err(error) = notifier.send("WATCHDOG=1") {
            tracing::warn!(code = "systemd_watchdog_send_failed", kind = ?error.kind(), "aviso de vida ao systemd não saiu");
        }
        // Depois do aviso, e numa thread própria: disco lento não atrasa o aviso nem a rodada, e o pool
        // bloqueante pode ser justamente a prova que travou.
        if let Some(line) = diary_line(verdict, probe, ms) {
            let spawned = std::thread::Builder::new().name("watchdog-diary".into())
                .spawn(move || crate::diag::record_blocking(crate::diag::diary_dir().as_deref(), &[line], "watchdog"));
            if let Err(error) = spawned {
                tracing::warn!(code = "systemd_watchdog_diary_failed", kind = ?error.kind(), "rodada atrasada do watchdog não foi ao diário");
            }
        }
        tokio::time::sleep(every).await;
    }
}

/// A rodada atrasada ou retida no diário, para a tela de status contar. `ms`: quanto a rodada levou.
pub fn diary_line(verdict: Verdict, probe: Option<&str>, ms: u64) -> Option<serde_json::Value> {
    let kind = match verdict {
        Verdict::Tolerated(_) => "atrasado",
        Verdict::Withheld(_) => "retido",
        Verdict::Grace | Verdict::Passed => return None,
    };
    let mut line = serde_json::json!({"evento": "systemd_watchdog", "nivel": "aviso", "tipo": kind, "ms": ms});
    if let Some(probe) = probe { line["prova"] = serde_json::json!(probe); }
    Some(line)
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

/// Até a porta pública atender a saúde. É o "pronto" da vida: a preparação das sessões segue
/// depois, e as ações esperam por ela (`startup`), não o aviso ao systemd.
pub async fn wait_serving(listen: SocketAddr) {
    let address = probe_address(listen);
    let started = std::time::Instant::now();
    let mut warned = false;
    while !health_answers(address, TIMEOUT).await {
        if !warned && started.elapsed() >= Duration::from_secs(60) {
            tracing::warn!(code = "server_ready_late", %address, "a saúde não responde há 60 s: sem READY=1 nem marca de pé");
            warned = true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
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

    #[test]
    fn exit_watchdog_round_goes_to_the_diary_only_when_late() {
        assert_eq!(diary_line(Verdict::Tolerated(1), Some("health"), 5003),
            Some(serde_json::json!({"evento": "systemd_watchdog", "nivel": "aviso", "tipo": "atrasado", "ms": 5003, "prova": "health"})));
        assert_eq!(diary_line(Verdict::Withheld(4), None, 12),
            Some(serde_json::json!({"evento": "systemd_watchdog", "nivel": "aviso", "tipo": "retido", "ms": 12})));
        assert_eq!(diary_line(Verdict::Passed, None, 3), None);
        assert_eq!(diary_line(Verdict::Grace, Some("health"), 3), None);
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

    /// P2 com a carência e a tolerância do antigo vigia do Python: nos primeiros 180 s o aviso sai
    /// sempre; depois, só a quarta rodada falha seguida o retém, e uma rodada boa zera a conta.
    #[test]
    fn p2_grace_and_four_failed_rounds_before_withholding() {
        let ready = std::time::Instant::now();
        let at = |secs: u64| ready + Duration::from_secs(secs);
        let mut rounds = Rounds::new(Policy::SERVER, ready);
        assert_eq!(rounds.round(at(0), Some(false)), Verdict::Grace);
        assert_eq!(rounds.round(at(179), None), Verdict::Grace, "na carência nada é provado nem contado");
        assert_eq!(rounds.round(at(180), Some(false)), Verdict::Tolerated(1));
        assert_eq!(rounds.round(at(210), Some(false)), Verdict::Tolerated(2));
        assert_eq!(rounds.round(at(240), Some(true)), Verdict::Passed, "a rodada boa zera a conta");
        let verdicts: Vec<_> = (0..5).map(|n| rounds.round(at(270 + 30 * n), Some(false))).collect();
        assert_eq!(verdicts, [Verdict::Tolerated(1), Verdict::Tolerated(2), Verdict::Tolerated(3), Verdict::Withheld(4), Verdict::Withheld(5)]);
        assert_eq!(verdicts.iter().map(|v| v.sends()).collect::<Vec<_>>(), [true, true, true, false, false]);
        assert_eq!(rounds.round(at(500), Some(true)), Verdict::Passed);
        assert!(Verdict::Grace.sends());
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
        // Sem carência e retendo na primeira falha: o componente preso nunca avisa.
        let strict = Policy { grace: Duration::ZERO, failures: 1 };
        let wedged = tokio::spawn(notify_alive(notifier.clone(), every, vec![("preso", fixed(None))], strict));
        tokio::time::sleep(Duration::from_millis(300)).await;
        wedged.abort();
        let mut buffer = [0u8; 64];
        assert!(socket.recv(&mut buffer).is_err(), "componente preso não pode mandar WATCHDOG=1");
        // Na carência o aviso sai mesmo com a prova presa.
        let early = tokio::spawn(notify_alive(notifier.clone(), every, vec![("preso", fixed(None))], Policy::SERVER));
        let reader = socket.try_clone().unwrap();
        reader.set_nonblocking(false).unwrap();
        reader.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let read = tokio::task::spawn_blocking(move || { let mut buffer = [0u8; 64]; let n = reader.recv(&mut buffer).unwrap(); buffer[..n].to_vec() }).await.unwrap();
        early.abort();
        let _ = early.await;
        assert_eq!(read, b"WATCHDOG=1");
        socket.set_nonblocking(true).unwrap();
        while socket.recv(&mut buffer).is_ok() {}
        let healthy = tokio::spawn(notify_alive(notifier, every, vec![("ok", fixed(Some(true)))], strict));
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
}
