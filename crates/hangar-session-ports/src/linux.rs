use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use std::os::unix::fs::MetadataExt;

use crate::{Listener, Outside, ProcessInfo, Scan, is_devtools, reachable_locally, sorted};

const LISTEN: u8 = 0x0A;

pub(crate) fn scan(candidates: &[u32]) -> io::Result<Scan> {
    let mut sockets: HashMap<u64, (u16, IpAddr)> = listen_sockets(&fs::read_to_string("/proc/net/tcp")?).collect();
    // Sem IPv6 no kernel o arquivo não existe.
    match fs::read_to_string("/proc/net/tcp6") {
        Ok(table) => sockets.extend(listen_sockets(&table)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    Ok(split(&sockets, candidates, fd_inodes, comm))
}

/// Os sockets dos candidatos viram `listeners`; o resto, `outside`, sem procurar dono.
fn split(
    sockets: &HashMap<u64, (u16, IpAddr)>,
    candidates: &[u32],
    fd_inodes: impl FnMut(u32) -> Option<Vec<u64>>,
    name: impl FnMut(u32) -> String,
) -> Scan {
    let (listeners, held) = owners(sockets, candidates, fd_inodes, name);
    let mut outside: Vec<(u16, u64, IpAddr)> = sockets.iter().filter(|(inode, _)| !held.contains(inode)).map(|(inode, (port, addr))| (*port, *inode, *addr)).collect();
    outside.sort_unstable();
    Scan { listeners: sorted(listeners), outside: outside.into_iter().map(|(port, inode, addr)| Outside { port, inode: Some(inode), addr }).collect() }
}

/// Varre os fds dos processos deste usuário até achar todos os inodes ou gastar o teto.
pub(crate) fn holders(inodes: &HashSet<u64>, fd_budget: usize) -> HashMap<u64, u32> {
    let (Ok(me), Ok(entries)) = (fs::metadata("/proc/self"), fs::read_dir("/proc")) else { return HashMap::new() };
    let pids = entries.flatten().filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok()).collect();
    walk(pids, inodes, fd_budget, |pid| {
        let dir = std::path::Path::new("/proc").join(pid.to_string());
        // O fd de outro usuário não se deixa ler: nem tenta.
        if !fs::metadata(&dir).is_ok_and(|m| m.uid() == me.uid()) { return None; }
        Some(fs::read_dir(dir.join("fd")).ok()?.flatten().map(|fd| socket_inode(&fd.path())).collect())
    })
}

/// Do pid maior para o menor: o servidor novo tem pid alto, e o teto não se gasta antes nos processos
/// antigos da sessão gráfica. `fds`: o inode de socket de cada fd do pid (`None` = outro tipo de fd).
// ponytail: depois que o pid_max dá a volta, o novo tem pid baixo e volta a ficar por último.
fn walk(mut pids: Vec<u32>, inodes: &HashSet<u64>, fd_budget: usize, mut fds: impl FnMut(u32) -> Option<Vec<Option<u64>>>) -> HashMap<u64, u32> {
    pids.sort_unstable_by(|a, b| b.cmp(a));
    let mut found = HashMap::new();
    let mut budget = fd_budget;
    for pid in pids {
        if found.len() == inodes.len() { break; }
        let Some(fds) = fds(pid) else { continue };
        for inode in fds {
            if budget == 0 { return found; }
            budget -= 1;
            if let Some(inode) = inode && inodes.contains(&inode) {
                found.entry(inode).or_insert(pid);
            }
        }
    }
    found
}

pub(crate) fn process_info(pid: u32) -> ProcessInfo {
    // Processo de outro usuário ou que acabou de sair não se lê: fica sem chave e sem cmdline.
    let read = |what: &str| fs::read(format!("/proc/{pid}/{what}")).unwrap_or_else(|e| {
        tracing::debug!(pid, what, kind = ?e.kind(), "session ports: process info unreadable");
        Vec::new()
    });
    let environ = read("environ");
    let cmdline = read("cmdline");
    ProcessInfo {
        session_key: environ_var(&environ, b"CP_SESSION_KEY"),
        devtools: is_devtools(cmdline.split(|b| *b == 0).map(String::from_utf8_lossy)),
        process: comm(pid),
    }
}

fn environ_var(environ: &[u8], name: &[u8]) -> Option<String> {
    environ.split(|b| *b == 0)
        .find_map(|item| item.strip_prefix(name)?.strip_prefix(b"="))
        .filter(|value| !value.is_empty())
        .map(|value| String::from_utf8_lossy(value).into_owned())
}

fn socket_inode(fd: &std::path::Path) -> Option<u64> {
    fs::read_link(fd).ok()?.to_str()?.strip_prefix("socket:[")?.strip_suffix(']')?.parse().ok()
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

/// `(inode, (porta, endereço))` de cada socket em LISTEN alcançável pelo loopback; o cabeçalho não casa.
fn listen_sockets(table: &str) -> impl Iterator<Item = (u64, (u16, IpAddr))> + '_ {
    table.lines().filter_map(parse_line)
        .filter(|e| e.state == LISTEN && reachable_locally(e.addr))
        .map(|e| (e.inode, (e.port, e.addr)))
}

/// Procura os sockets só nos fds dos candidatos; pid que não se deixa ler fica de fora. Devolve
/// também os inodes que algum candidato segura.
fn owners(
    sockets: &HashMap<u64, (u16, IpAddr)>,
    candidates: &[u32],
    mut fd_inodes: impl FnMut(u32) -> Option<Vec<u64>>,
    mut name: impl FnMut(u32) -> String,
) -> (Vec<Listener>, HashSet<u64>) {
    let (mut found, mut held) = (Vec::new(), HashSet::new());
    if sockets.is_empty() {
        return (found, held);
    }
    let mut seen = HashSet::new();
    for &pid in candidates {
        if !seen.insert(pid) {
            continue;
        }
        let Some(inodes) = fd_inodes(pid) else { continue };
        let ports: Vec<(u16, IpAddr)> = inodes.iter().filter_map(|inode| sockets.get(inode).copied()).collect();
        if ports.is_empty() {
            continue;
        }
        held.extend(inodes.iter().filter(|inode| sockets.contains_key(inode)));
        let process = name(pid);
        found.extend(ports.into_iter().map(|(port, addr)| Listener { port, pid, process: process.clone(), addr }));
    }
    (found, held)
}

fn fd_inodes(pid: u32) -> Option<Vec<u64>> {
    let entries = fs::read_dir(format!("/proc/{pid}/fd")).ok()?;
    Some(entries.flatten().filter_map(|entry| socket_inode(&entry.path())).collect())
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
    fn only_listen_on_loopback_or_any_is_kept_with_its_address() {
        let ip = |text: &str| text.parse::<IpAddr>().unwrap();
        assert_eq!(listen_sockets(TCP).collect::<Vec<_>>(), vec![(10673, (8317, ip("127.0.0.1"))), (26902, (5432, ip("0.0.0.0")))]);
        assert_eq!(listen_sockets(TCP6).collect::<Vec<_>>(), vec![(18355, (7331, ip("::1"))), (26903, (5432, ip("::")))]);
    }

    #[test]
    fn each_address_of_a_port_is_kept_and_outsiders_are_never_scanned() {
        let sockets: HashMap<u64, (u16, IpAddr)> = listen_sockets(TCP).chain(listen_sockets(TCP6)).collect();
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
        let found = split(&sockets, &[10, 40, 20, 10], fds, names);
        // O 5432 do pid 10 fica em v4 e v6: o túnel escolhe a família pelo endereço.
        assert_eq!(found.listeners, vec![
            Listener { port: 5432, pid: 10, process: "postgres".into(), addr: Ipv4Addr::UNSPECIFIED.into() },
            Listener { port: 5432, pid: 10, process: "postgres".into(), addr: Ipv6Addr::UNSPECIFIED.into() },
            Listener { port: 7331, pid: 20, process: "node".into(), addr: Ipv6Addr::LOCALHOST.into() },
        ]);
        assert_eq!(scanned, vec![10, 40, 20]);
        // O 8317 (inode 10673) é de um processo que não foi lido.
        assert_eq!(found.outside, vec![Outside { port: 8317, inode: Some(10673), addr: Ipv4Addr::LOCALHOST.into() }]);
    }

    #[test]
    fn the_other_half_of_a_divided_port_stays_outside() {
        // 5432 em v4 (26902) é do candidato; o v6 (26903) é de outro processo.
        let sockets: HashMap<u64, (u16, IpAddr)> = listen_sockets(TCP).chain(listen_sockets(TCP6)).collect();
        let found = split(&sockets, &[10], |_| Some(vec![26902]), |_| "node".to_owned());
        assert_eq!(found.listeners, vec![Listener { port: 5432, pid: 10, process: "node".into(), addr: Ipv4Addr::UNSPECIFIED.into() }]);
        assert_eq!(found.outside.iter().map(|o| (o.port, o.inode, o.addr)).collect::<Vec<_>>(),
            vec![(5432, Some(26903), IpAddr::from(Ipv6Addr::UNSPECIFIED)), (7331, Some(18355), IpAddr::from(Ipv6Addr::LOCALHOST)),
                (8317, Some(10673), IpAddr::from(Ipv4Addr::LOCALHOST))]);
    }

    #[test]
    fn the_session_key_is_read_whole_and_only_by_its_exact_name() {
        let environ = b"HOME=/h\0CP_SESSION_KEY_OLD=x\0CP_SESSION_KEY=abc=d\0PATH=/bin\0";
        assert_eq!(environ_var(environ, b"CP_SESSION_KEY").as_deref(), Some("abc=d"));
        assert_eq!(environ_var(b"CP_SESSION_KEY=\0", b"CP_SESSION_KEY"), None);
        assert_eq!(environ_var(b"", b"CP_SESSION_KEY"), None);
    }

    #[test]
    fn the_walk_starts_at_the_highest_pid_so_the_budget_reaches_a_new_server() {
        // 10 e 20 são processos antigos com muitos fds; o servidor novo 9000 segura o inode 77.
        let fds = |pid: u32| match pid {
            9000 => Some(vec![None, Some(77)]),
            40 => None,
            _ => Some(vec![None; 5]),
        };
        let wanted = HashSet::from([77]);
        assert_eq!(walk(vec![10, 20, 40, 9000], &wanted, 2, fds), HashMap::from([(77, 9000)]));
        // O teto vale: com um fd só, o 77 (o segundo fd do 9000) não é lido.
        assert!(walk(vec![10, 9000], &wanted, 1, fds).is_empty());
    }

    #[test]
    fn this_process_holds_its_own_listener_and_the_walk_stops_at_the_budget() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let me = std::process::id();
        let found = scan(&[me]).unwrap();
        assert!(found.listeners.iter().any(|l| l.port == port && l.pid == me && l.addr == IpAddr::V4(Ipv4Addr::LOCALHOST)), "{found:?}");
        let inode = fd_inodes(me).unwrap().into_iter().find(|inode| {
            fs::read_to_string("/proc/net/tcp").unwrap().lines().filter_map(parse_line).any(|e| e.inode == *inode && e.port == port)
        }).unwrap();
        let wanted = HashSet::from([inode]);
        assert_eq!(holders(&wanted, usize::MAX).get(&inode), Some(&me));
        assert!(holders(&wanted, 0).is_empty());
    }
}
