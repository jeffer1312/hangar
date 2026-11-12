use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub async fn read(ports: HashSet<i64>) -> BTreeMap<i64, (bool, Option<String>)> {
    #[cfg(target_os = "linux")]
    {
        tokio::task::spawn_blocking(move || linux(ports))
            .await
            .unwrap_or_default()
    }
    #[cfg(not(target_os = "linux"))]
    {
        other(ports).await
    }
}
#[cfg(target_os = "linux")]
fn linux(ports: HashSet<i64>) -> BTreeMap<i64, (bool, Option<String>)> {
    static CACHE: OnceLock<Mutex<(HashMap<String, String>, HashMap<String, Instant>)>> =
        OnceLock::new();
    let mut out: BTreeMap<_, _> = ports.iter().map(|p| (*p, (false, None))).collect();
    if ports.is_empty() {
        return out;
    }
    let mut inodes = HashMap::new();
    for file in ["/proc/net/tcp", "/proc/net/tcp6"] {
        if let Ok(text) = std::fs::read_to_string(file) {
            for line in text.lines().skip(1) {
                let f: Vec<_> = line.split_whitespace().collect();
                if f.len() > 9 && f[3] == "0A" {
                    if let Some(p) = f[1]
                        .rsplit(':')
                        .next()
                        .and_then(|p| i64::from_str_radix(p, 16).ok())
                        .filter(|p| ports.contains(p))
                    {
                        out.insert(p, (true, None));
                        inodes.insert(format!("socket:[{}]", f[9]), p);
                    }
                }
            }
        }
    }
    let mut cache = CACHE
        .get_or_init(|| Mutex::new((HashMap::new(), HashMap::new())))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    cache.0.retain(|s, _| inodes.contains_key(s));
    cache.1.retain(|s, _| inodes.contains_key(s));
    let mut pending: HashSet<_> = inodes
        .keys()
        .filter(|s| {
            !cache.0.contains_key(*s)
                && cache
                    .1
                    .get(*s)
                    .is_none_or(|t| t.elapsed() >= Duration::from_secs(60))
        })
        .cloned()
        .collect();
    if let Ok(pids) = std::fs::read_dir("/proc") {
        for pid in pids.filter_map(Result::ok) {
            if pending.is_empty() {
                break;
            }
            if !pid
                .file_name()
                .to_string_lossy()
                .bytes()
                .all(|b| b.is_ascii_digit())
            {
                continue;
            }
            if let Ok(fds) = std::fs::read_dir(pid.path().join("fd")) {
                for fd in fds.filter_map(Result::ok) {
                    if let Ok(link) = std::fs::read_link(fd.path()) {
                        let link = link.to_string_lossy().into_owned();
                        if pending.contains(&link) {
                            if let Ok(cwd) = std::fs::read_link(pid.path().join("cwd")) {
                                cache
                                    .0
                                    .insert(link.clone(), cwd.to_string_lossy().into_owned());
                                pending.remove(&link);
                            }
                        }
                    }
                }
            }
        }
    }
    for p in pending {
        cache.1.insert(p, Instant::now());
    }
    for (socket, port) in inodes {
        if let Some(owner) = cache.0.get(&socket) {
            out.insert(port, (true, Some(owner.clone())));
        }
    }
    out
}
#[cfg(not(target_os = "linux"))]
async fn other(ports: HashSet<i64>) -> BTreeMap<i64, (bool, Option<String>)> {
    let mut out = BTreeMap::new();
    // Sem permissão para consultar o dono, mantém a distinção entre porta fechada e dono desconhecido.
    let mut cmd = tokio::process::Command::new(if cfg!(windows) { "netstat" } else { "lsof" });
    cmd.args(if cfg!(windows) {
        vec!["-ano", "-p", "tcp"]
    } else {
        vec!["-nP", "-iTCP", "-sTCP:LISTEN", "-Fpn"]
    })
    .kill_on_drop(true);
    let text = tokio::time::timeout(Duration::from_secs(5), cmd.output())
        .await
        .ok()
        .and_then(Result::ok)
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::nothing().with_cwd(sysinfo::UpdateKind::Always),
    );
    for port in ports {
        let mut found = None;
        if cfg!(windows) {
            for line in text.lines() {
                let f: Vec<_> = line.split_whitespace().collect();
                if f.len() >= 5
                    && f[3] == "LISTENING"
                    && f[1].rsplit(':').next().and_then(|p| p.parse::<i64>().ok()) == Some(port)
                {
                    let owner = f[4]
                        .parse::<usize>()
                        .ok()
                        .and_then(|p| system.process(sysinfo::Pid::from(p)))
                        .and_then(|p| p.cwd())
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "<indeterminado>".into());
                    found = Some(owner);
                    break;
                }
            }
        }
        if !cfg!(windows) {
            let mut pid = None;
            for line in text.lines() {
                if let Some(p) = line.strip_prefix('p') {
                    pid = p.parse::<usize>().ok();
                }
                if let Some(address) = line.strip_prefix('n') {
                    if address
                        .rsplit(':')
                        .next()
                        .and_then(|p| p.parse::<i64>().ok())
                        == Some(port)
                    {
                        found = Some(
                            pid.and_then(|p| system.process(sysinfo::Pid::from(p)))
                                .and_then(|p| p.cwd())
                                .map(|p| p.to_string_lossy().into_owned())
                                .unwrap_or_else(|| "<indeterminado>".into()),
                        );
                        break;
                    }
                }
            }
        }
        let listening = if found.is_some() {
            true
        } else if let Ok(p) = u16::try_from(port) {
            tokio::time::timeout(
                Duration::from_millis(150),
                tokio::net::TcpStream::connect(("127.0.0.1", p)),
            )
            .await
            .is_ok_and(|r| r.is_ok())
        } else {
            false
        };
        out.insert(
            port,
            (
                listening,
                if listening {
                    Some(found.unwrap_or_else(|| "<indeterminado>".into()))
                } else {
                    None
                },
            ),
        );
    }
    out
}
