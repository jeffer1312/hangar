//! Portas TCP em LISTEN dos processos candidatos: quem escuta e em qual porta. Dar a porta à
//! sessão é de quem chama, que já sabe quais processos são dela.
#![deny(unsafe_code)]
#![cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]

use std::collections::BTreeMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(any(windows, test))]
mod win_table;
#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listener {
    pub port: u16,
    pub pid: u32,
    /// Nome curto do executável (o `comm` no Linux).
    pub process: String,
}

/// As portas em LISTEN de `candidates`, uma por porta e em ordem crescente. Só os processos
/// candidatos são examinados: isto roda a cada tique da lista.
pub fn listening(candidates: &[u32]) -> io::Result<Vec<Listener>> {
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    #[cfg(target_os = "linux")]
    return linux::listening(candidates);
    #[cfg(windows)]
    return windows::listening(candidates);
    // shortcut: no macOS volta vazio; o teto é não mostrar portas lá. Ler por libproc
    // (proc_pidinfo/PROC_PIDFDSOCKETINFO) quando houver um Mac para provar o layout das structs.
    #[cfg(not(any(target_os = "linux", windows)))]
    Ok(Vec::new())
}

/// Só o que o túnel alcança por 127.0.0.1 ou ::1; escuta presa a outro IP da máquina não serve.
fn reachable_locally(ip: IpAddr) -> bool {
    let v4 = |a: Ipv4Addr| a.is_loopback() || a.is_unspecified();
    match ip {
        IpAddr::V4(a) => v4(a),
        IpAddr::V6(a) => a.is_loopback() || a.is_unspecified() || a.to_ipv4_mapped().is_some_and(v4),
    }
}

/// A mesma porta em v4 e v6 vira uma entrada; fica a primeira vista.
fn by_port(found: impl IntoIterator<Item = Listener>) -> Vec<Listener> {
    let mut ports = BTreeMap::new();
    for listener in found {
        ports.entry(listener.port).or_insert(listener);
    }
    ports.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_and_any_addresses_are_reachable() {
        for ip in ["127.0.0.1", "127.0.0.53", "0.0.0.0", "::1", "::", "::ffff:127.0.0.1"] {
            assert!(reachable_locally(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["192.168.0.50", "100.64.0.2", "fd7a:115c:a1e0::2", "::ffff:192.168.0.1"] {
            assert!(!reachable_locally(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn same_port_twice_comes_out_once_sorted() {
        let l = |port, pid| Listener { port, pid, process: "p".into() };
        assert_eq!(by_port([l(5432, 7), l(3000, 9), l(5432, 8)]), vec![l(3000, 9), l(5432, 7)]);
    }
}
