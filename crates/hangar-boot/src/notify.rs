//! Aviso ao systemd numa unit `Type=notify`: pronto, vivo e parando. Fora do systemd, nada.
use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct Notifier {
    socket: Option<String>,
    /// Metade do `WatchdogSec`: o intervalo entre dois `WATCHDOG=1`.
    pub watchdog: Option<Duration>,
}

impl Notifier {
    /// `NOTIFY_SOCKET` e `WATCHDOG_USEC` (que só valem para `WATCHDOG_PID`, quando ele vem).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>, pid: u32) -> Notifier {
        let socket = get("NOTIFY_SOCKET").filter(|value| !value.is_empty());
        let ours = get("WATCHDOG_PID").is_none_or(|value| value.trim().parse::<u32>() == Ok(pid));
        let watchdog = get("WATCHDOG_USEC").and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|usec| ours && *usec > 0 && socket.is_some()).map(|usec| Duration::from_micros(usec / 2));
        Notifier { socket, watchdog }
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
