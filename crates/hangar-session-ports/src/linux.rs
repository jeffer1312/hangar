use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::{Listener, by_port, reachable_locally};

const LISTEN: u8 = 0x0A;

pub(crate) fn listening(candidates: &[u32]) -> io::Result<Vec<Listener>> {
    let mut sockets: HashMap<u64, u16> = listen_sockets(&fs::read_to_string("/proc/net/tcp")?).collect();
    // Sem IPv6 no kernel o arquivo não existe.
    match fs::read_to_string("/proc/net/tcp6") {
        Ok(table) => sockets.extend(listen_sockets(&table)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    Ok(by_port(owners(&sockets, candidates, fd_inodes, comm)))
}

#[derive(Debug, PartialEq, Eq)]
struct Entry {
    addr: IpAddr,
    port: u16,
    state: u8,
    inode: u64,
}

/// Uma linha de `/proc/net/tcp{,6}`: `sl local rem st tx:rx tr:tm retrnsmt uid timeout inode …`.
fn parse_line(line: &str) -> Option<Entry> {
    let fields: Vec<&str> = line.split_whitespace().take(10).collect();
    let [_, local, _, state, _, _, _, _, _, inode] = fields[..] else { return None };
    let (addr, port) = local.split_once(':')?;
    Some(Entry {
        addr: parse_addr(addr)?,
        port: u16::from_str_radix(port, 16).ok()?,
        state: u8::from_str_radix(state, 16).ok()?,
        inode: inode.parse().ok()?,
    })
}

/// O kernel imprime cada palavra de 32 bits do endereço na ordem de bytes da máquina.
fn parse_addr(hex: &str) -> Option<IpAddr> {
    if hex.len() % 8 != 0 {
        return None;
    }
    let words: Vec<[u8; 4]> = (0..hex.len() / 8)
        .map(|i| hex.get(i * 8..i * 8 + 8).and_then(|w| u32::from_str_radix(w, 16).ok()).map(u32::to_ne_bytes))
        .collect::<Option<_>>()?;
    match words[..] {
        [a] => Some(IpAddr::V4(Ipv4Addr::from(a))),
        [a, b, c, d] => {
            let mut bytes = [0u8; 16];
            for (chunk, word) in bytes.chunks_exact_mut(4).zip([a, b, c, d]) {
                chunk.copy_from_slice(&word);
            }
            Some(IpAddr::V6(Ipv6Addr::from(bytes)))
        }
        _ => None,
    }
}

/// `(inode, porta)` de cada socket em LISTEN alcançável pelo loopback; o cabeçalho não casa.
fn listen_sockets(table: &str) -> impl Iterator<Item = (u64, u16)> + '_ {
    table.lines().filter_map(parse_line)
        .filter(|e| e.state == LISTEN && reachable_locally(e.addr))
        .map(|e| (e.inode, e.port))
}

/// Procura os sockets só nos fds dos candidatos; pid que não se deixa ler fica de fora.
fn owners(
    sockets: &HashMap<u64, u16>,
    candidates: &[u32],
    mut fd_inodes: impl FnMut(u32) -> Option<Vec<u64>>,
    mut name: impl FnMut(u32) -> String,
) -> Vec<Listener> {
    let mut found = Vec::new();
    if sockets.is_empty() {
        return found;
    }
    let mut seen = HashSet::new();
    for &pid in candidates {
        if !seen.insert(pid) {
            continue;
        }
        let Some(inodes) = fd_inodes(pid) else { continue };
        let ports: Vec<u16> = inodes.iter().filter_map(|inode| sockets.get(inode).copied()).collect();
        if ports.is_empty() {
            continue;
        }
        let process = name(pid);
        found.extend(ports.into_iter().map(|port| Listener { port, pid, process: process.clone() }));
    }
    found
}

fn fd_inodes(pid: u32) -> Option<Vec<u64>> {
    let entries = fs::read_dir(format!("/proc/{pid}/fd")).ok()?;
    Some(entries.flatten()
        .filter_map(|entry| fs::read_link(entry.path()).ok())
        .filter_map(|target| target.to_str()?.strip_prefix("socket:[")?.strip_suffix(']')?.parse().ok())
        .collect())
}

fn comm(pid: u32) -> String {
    fs::read_to_string(format!("/proc/{pid}/comm")).map(|name| name.trim_end().to_owned()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TCP: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:207D 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 10673 2 0000000000000000 100 0 0 10 0
   5: 02004064:01BB 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 36322 2 0000000000000000 100 0 0 10 0
  20: 00000000:1538 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 26902 1 0000000000000000 100 0 0 10 0
  26: 3200A8C0:223D 1C00A8C0:9664 01 00000000:00000000 00:00000000 00000000  1000        0 1674674 1 0000000000000000 21 0 0 10 -1
";
    const TCP6: &str = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 5C117AFD0000E0A10000000002000000:F59B 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 23512 2 0000000000000000 100 0 0 10 0
   2: 00000000000000000000000001000000:1CA3 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 18355 1 0000000000000000 100 0 0 10 0
   4: 00000000000000000000000000000000:1538 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 26903 1 0000000000000000 100 0 0 10 0
";

    #[test]
    fn a_real_tcp_line_gives_address_port_state_and_inode() {
        let line = "   0: 0100007F:207D 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 10673 2 0000000000000000 100 0 0 10 0";
        assert_eq!(parse_line(line), Some(Entry { addr: "127.0.0.1".parse().unwrap(), port: 8317, state: 0x0A, inode: 10673 }));
    }

    #[test]
    fn a_real_tcp6_line_gives_address_port_state_and_inode() {
        let line = "   2: 00000000000000000000000001000000:1CA3 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 18355 1 0000000000000000 100 0 0 10 0";
        assert_eq!(parse_line(line), Some(Entry { addr: "::1".parse().unwrap(), port: 7331, state: 0x0A, inode: 18355 }));
        let tailscale = "   0: 5C117AFD0000E0A10000000002000000:F59B 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 23512 2 0000000000000000 100 0 0 10 0";
        assert_eq!(parse_line(tailscale).unwrap().addr, "fd7a:115c:a1e0::2".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn only_listen_on_loopback_or_any_is_kept() {
        assert_eq!(listen_sockets(TCP).collect::<Vec<_>>(), vec![(10673, 8317), (26902, 5432)]);
        assert_eq!(listen_sockets(TCP6).collect::<Vec<_>>(), vec![(18355, 7331), (26903, 5432)]);
    }

    #[test]
    fn same_port_in_v4_and_v6_comes_out_once_and_outsiders_are_never_scanned() {
        let sockets: HashMap<u64, u16> = listen_sockets(TCP).chain(listen_sockets(TCP6)).collect();
        let mut scanned = Vec::new();
        let fds = |pid: u32| {
            scanned.push(pid);
            match pid {
                10 => Some(vec![3, 26902, 26903]),
                20 => Some(vec![18355, 99]),
                40 => None,
                _ => Some(vec![10673]),
            }
        };
        let names = |pid: u32| if pid == 10 { "postgres".to_owned() } else { "node".to_owned() };
        let found = by_port(owners(&sockets, &[10, 40, 20, 10], fds, names));
        assert_eq!(found, vec![
            Listener { port: 5432, pid: 10, process: "postgres".into() },
            Listener { port: 7331, pid: 20, process: "node".into() },
        ]);
        assert_eq!(scanned, vec![10, 40, 20]);
    }
}
