// crates/hangar-server/src/transcript/peer.rs
//! Nome tmux da sessão dona de um pid (`registry.name_of_pid`, backend/app/registry.py): é o nome
//! que o recado nativo entre sessões mostra no lugar do título.

use std::collections::{HashMap, HashSet};

/// Pares (sessão, pid do pane) na ordem da saída de
/// `tmux list-panes -a -F '#{session_name}\t#{pane_pid}'`; linha sem pid numérico é ignorada.
pub(crate) fn parse_panes(output: &str) -> Vec<(String, i64)> {
    output
        .lines()
        .filter_map(|line| {
            let (name, pid) = line.rsplit_once('\t')?;
            Some((name.to_string(), pid.trim().parse::<i64>().ok().filter(|p| *p > 0)?))
        })
        .collect()
}

/// `pid` é o pid do pane ou descende dele; o conjunto de visitados protege de anel no mapa.
fn is_descendant(root: i64, pid: i64, children: &HashMap<i64, Vec<i64>>) -> bool {
    let mut seen = HashSet::new();
    let mut stack = vec![root];
    while let Some(p) = stack.pop() {
        if !seen.insert(p) {
            continue;
        }
        if p == pid {
            return true;
        }
        stack.extend(children.get(&p).into_iter().flatten().copied());
    }
    false
}

/// Primeira sessão que tem um pane cujo pid é `pid` ou ancestral dele.
pub(crate) fn session_of(
    panes: &[(String, i64)],
    pid: i64,
    children: &HashMap<i64, Vec<i64>>,
) -> Option<String> {
    panes.iter().find(|(_, pane)| is_descendant(*pane, pid, children)).map(|(name, _)| name.clone())
}

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::HashMap;
    use std::process::{Command, Stdio};
    use std::sync::{LazyLock, Mutex};
    use std::time::{Duration, Instant};

    use super::{parse_panes, session_of};

    /// (ppid, instante de nascimento em ticks) do `/proc/<pid>/stat`. O comm (campo 2) pode ter
    /// espaço e parêntese, então só o último ')' delimita.
    fn read_stat(pid: i64) -> Option<(i64, u64)> {
        let raw = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let mut fields = raw[raw.rfind(')')? + 1..].split_whitespace();
        let ppid = fields.nth(1)?.parse().ok()?;
        let start = fields.nth(17)?.parse().ok()?;
        Some((ppid, start))
    }

    fn children_map() -> HashMap<i64, Vec<i64>> {
        let mut map: HashMap<i64, Vec<i64>> = HashMap::new();
        let Ok(entries) = std::fs::read_dir("/proc") else { return map };
        for entry in entries.flatten() {
            let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<i64>().ok()) else { continue };
            if let Some((ppid, _)) = read_stat(pid) {
                map.entry(ppid).or_default().push(pid);
            }
        }
        map
    }

    /// Panes do tmux; None quando o tmux não roda ou trava (nunca cacheado). Comando recusado
    /// (sem servidor) dá lista vazia, que o Python cacheia como "sem dono".
    fn list_panes() -> Option<Vec<(String, i64)>> {
        let mut child = Command::new("tmux")
            .args(["list-panes", "-a", "-F", "#{session_name}\t#{pane_pid}"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match child.try_wait().ok()? {
                Some(status) if !status.success() => return Some(Vec::new()),
                Some(_) => break,
                None if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        let out = child.wait_with_output().ok()?;
        Some(parse_panes(&String::from_utf8_lossy(&out.stdout)))
    }

    // pid -> (nascimento, sessão). O pid sozinho não identifica o processo (reuso depois que o
    // dono morreu), então a entrada só vale com o mesmo nascimento; sem /proc legível não há cache.
    static CACHE: LazyLock<Mutex<HashMap<i64, (u64, Option<String>)>>> = LazyLock::new(Mutex::default);

    pub fn name_of_pid(pid: i64) -> Option<String> {
        let born = read_stat(pid).map(|(_, start)| start);
        if let Some(born) = born {
            if let Some((cached, name)) = CACHE.lock().ok()?.get(&pid) {
                if *cached == born {
                    return name.clone();
                }
            }
        }
        let panes = list_panes()?;
        let found = session_of(&panes, pid, &children_map());
        if let Some(born) = born {
            CACHE.lock().ok()?.insert(pid, (born, found.clone()));
        }
        found
    }
}

#[cfg(target_os = "linux")]
pub(crate) use linux::name_of_pid;

// Fora do Linux não há /proc: devolve None e o recado cai no título, como no Python quando o nome
// não resolve.
#[cfg(not(target_os = "linux"))]
pub(crate) fn name_of_pid(_pid: i64) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_list_panes_output() {
        let out = "main\t100\nwork space\t200\nbad\tabc\n\nsolo\n";
        assert_eq!(parse_panes(out), vec![("main".to_string(), 100), ("work space".to_string(), 200)]);
    }

    #[test]
    fn matches_pane_pid_and_its_descendants() {
        let panes = parse_panes("a\t10\nb\t20\n");
        let children = HashMap::from([(10, vec![11]), (11, vec![12]), (20, vec![21])]);
        assert_eq!(session_of(&panes, 10, &children).as_deref(), Some("a"));
        assert_eq!(session_of(&panes, 12, &children).as_deref(), Some("a"));
        assert_eq!(session_of(&panes, 21, &children).as_deref(), Some("b"));
        assert_eq!(session_of(&panes, 99, &children), None);
    }

    #[test]
    fn ring_in_the_children_map_does_not_loop() {
        let panes = parse_panes("a\t1\n");
        let children = HashMap::from([(1, vec![2]), (2, vec![1])]);
        assert_eq!(session_of(&panes, 3, &children), None);
    }
}
