//! Portas TCP em LISTEN dos processos candidatos: quem escuta e em qual porta. Dar a porta à
//! sessão é de quem chama, que já sabe quais processos são dela.
#![deny(unsafe_code)]
#![cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]

use std::collections::{HashMap, HashSet};
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

/// Socket em LISTEN alcançável pelo loopback que nenhum candidato segura.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outside {
    pub port: u16,
    /// O inode do socket no Linux; `None` onde o dono não se procura (Windows).
    pub inode: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scan {
    /// Uma por (porta, pid), em ordem de porta e pid.
    pub listeners: Vec<Listener>,
    /// Em ordem de porta.
    pub outside: Vec<Outside>,
}

/// Uma leitura da tabela: o que os candidatos seguram e o que fica de fora. Só os fds dos
/// candidatos são examinados: isto roda a cada tique da lista.
pub fn scan(candidates: &[u32]) -> io::Result<Scan> {
    #[cfg(target_os = "linux")]
    return linux::scan(candidates);
    #[cfg(windows)]
    return windows::scan(candidates);
    // shortcut: no macOS volta vazio; o teto é não mostrar portas lá. Ler por libproc
    // (proc_pidinfo/PROC_PIDFDSOCKETINFO) quando houver um Mac para provar o layout das structs.
    #[cfg(not(any(target_os = "linux", windows)))]
    { let _ = candidates; Ok(Scan::default()) }
}

/// Quem segura cada inode, entre os processos deste usuário, lendo no máximo `fd_budget` fds.
pub fn holders(inodes: &HashSet<u64>, fd_budget: usize) -> HashMap<u64, u32> {
    #[cfg(target_os = "linux")]
    return linux::holders(inodes, fd_budget);
    #[cfg(not(target_os = "linux"))]
    { let _ = (inodes, fd_budget); HashMap::new() }
}

/// O que o dono da porta herdou e como foi lançado, lido uma vez enquanto ele escuta.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessInfo {
    /// `CP_SESSION_KEY` do ambiente: o pane e o cano a põem, e os filhos do agente a herdam.
    pub session_key: Option<String>,
    /// `--remote-debugging-port` no `cmdline`: porta de DevTools de navegador, não servidor da pessoa.
    pub devtools: bool,
    pub process: String,
}

pub fn process_info(pid: u32) -> ProcessInfo {
    #[cfg(target_os = "linux")]
    return linux::process_info(pid);
    // ponytail: no Windows ambiente e cmdline exigem ler o PEB de outro processo; sem isso o
    // processo fica sem chave e sem marca de DevTools.
    #[cfg(not(target_os = "linux"))]
    { let _ = pid; ProcessInfo::default() }
}

/// O processo ainda existe (o pid pode ter sido reusado; o teto é esse).
pub fn alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    return std::path::Path::new(&format!("/proc/{pid}")).exists();
    #[cfg(not(target_os = "linux"))]
    { let _ = pid; true }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn is_devtools(args: impl IntoIterator<Item = impl AsRef<str>>) -> bool {
    const FLAG: &str = "--remote-debugging-port";
    args.into_iter().any(|arg| arg.as_ref().strip_prefix(FLAG).is_some_and(|rest| rest.is_empty() || rest.starts_with('=')))
}

/// Só o que o túnel alcança por 127.0.0.1 ou ::1; escuta presa a outro IP da máquina não serve.
fn reachable_locally(ip: IpAddr) -> bool {
    let v4 = |a: Ipv4Addr| a.is_loopback() || a.is_unspecified();
    match ip {
        IpAddr::V4(a) => v4(a),
        IpAddr::V6(a) => a.is_loopback() || a.is_unspecified() || a.to_ipv4_mapped().is_some_and(v4),
    }
}

/// A mesma porta do mesmo pid em v4 e v6 vira uma entrada; em ordem de porta e pid.
fn sorted(found: impl IntoIterator<Item = Listener>) -> Vec<Listener> {
    let mut list: Vec<Listener> = found.into_iter().collect();
    list.sort_by_key(|l| (l.port, l.pid));
    list.dedup_by_key(|l| (l.port, l.pid));
    list
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
    fn same_port_and_pid_twice_comes_out_once_and_another_pid_stays() {
        let l = |port, pid| Listener { port, pid, process: "p".into() };
        assert_eq!(sorted([l(5432, 8), l(3000, 9), l(5432, 7), l(5432, 8)]), vec![l(3000, 9), l(5432, 7), l(5432, 8)]);
    }

    #[test]
    fn only_the_debugging_port_flag_marks_devtools() {
        assert!(is_devtools(["chrome", "--remote-debugging-port=9333"]));
        assert!(is_devtools(["chrome", "--remote-debugging-port", "9333"]));
        for args in [&["chrome", "--remote-debugging-pipe"][..], &["node", "server.js", "--port=9333"], &["chrome", "--remote-debugging-portal"]] {
            assert!(!is_devtools(args), "{args:?}");
        }
    }
}
