//! Aviso ao systemd numa unit `Type=notify`: pronto, vivo e parando. Fora do systemd, nada.
use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct Notifier {
    socket: Option<String>,
    /// Metade do `WatchdogSec`: o intervalo entre dois `WATCHDOG=1`.
    pub watchdog: Option<Duration>,
    /// Código de por que o watchdog pedido ficou de fora, para o log depois que ele abrir.
    pub rejected: Option<&'static str>,
}

impl Notifier {
    /// `NOTIFY_SOCKET` e `WATCHDOG_USEC` (que só valem para `WATCHDOG_PID`, quando ele vem).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>, pid: u32) -> Notifier {
        let socket = get("NOTIFY_SOCKET").filter(|value| !value.is_empty());
        let Some(raw) = get("WATCHDOG_USEC") else { return Notifier { socket, watchdog: None, rejected: None } };
        let rejected = |code| Notifier { socket: socket.clone(), watchdog: None, rejected: Some(code) };
        let Ok(usec) = raw.trim().parse::<u64>() else { return rejected("watchdog_usec_invalid") };
        match get("WATCHDOG_PID").map(|value| value.trim().parse::<u32>()) {
            Some(Err(_)) => return rejected("watchdog_pid_invalid"),
            // O watchdog é de outro processo (o lançador, ou quem nos abriu): não é nosso para avisar.
            Some(Ok(other)) if other != pid => return rejected("watchdog_pid_other"),
            _ => {}
        }
        if usec == 0 { return Notifier { socket, watchdog: None, rejected: None } }
        if socket.is_none() { return rejected("watchdog_without_socket"); }
        Notifier { socket, watchdog: Some(Duration::from_micros(usec / 2)), rejected: None }
    }

    pub fn send(&self, state: &str) -> std::io::Result<()> {
        let Some(socket) = &self.socket else { return Ok(()) };
        send_to(socket, state)
    }
}

#[cfg(target_os = "linux")]
fn send_to(socket: &str, state: &str) -> std::io::Result<()> {
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::{SocketAddr, UnixDatagram};
    let datagram = UnixDatagram::unbound()?;
    let sent = match socket.strip_prefix('@') {
        Some(name) => datagram.send_to_addr(state.as_bytes(), &SocketAddr::from_abstract_name(name)?)?,
        None => datagram.send_to(state.as_bytes(), socket)?,
    };
    if sent != state.len() { return Err(std::io::Error::other("aviso ao systemd cortado")); }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn send_to(_socket: &str, _state: &str) -> std::io::Result<()> {
    Ok(())
}
