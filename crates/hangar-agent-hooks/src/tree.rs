//! A subida da árvore de processos até o `claude` que rodou o hook: dá o `--session-id`/`--resume`
//! do cmdline (o `boot_id` que o servidor conhece) e o pid do REPL, que casa marcador e pane.
use regex::Regex;
use std::sync::OnceLock;

/// Teto de saltos: nunca enrosca numa árvore estranha.
pub const MAX_HOPS: usize = 12;

/// O que a subida precisa saber de cada processo.
pub trait Processes {
    /// O argv; `None` = ilegível (processo sumiu, sem permissão), e a subida desiste.
    fn argv(&self, pid: u32) -> Option<Vec<String>>;
    /// O pai; `None` = ilegível ou sem pai.
    fn parent(&self, pid: u32) -> Option<u32>;
}

/// Só o argv[0] decide: o nome `claude` (sem `.exe`, com a caixa exata), ou `node` rodando algo de
/// uma pasta `claude-code`. Procurar no cmdline inteiro casava o `sh -c` que roda o próprio hook
/// quando o caminho dele tem "claude".
pub fn is_claude(argv: &[String]) -> bool {
    let Some(first) = argv.first() else { return false };
    let base = crate::input::basename(first);
    let cut = base.len().saturating_sub(4);
    let base = match base.get(cut..) {
        Some(tail) if tail.eq_ignore_ascii_case(".exe") => &base[..cut],
        _ => base,
    };
    match base {
        "claude" => true,
        "node" => argv[1..].iter().any(|arg| arg.replace('\\', "/").split('/').any(|part| part == "claude-code")),
        _ => false,
    }
}

fn session_flag() -> &'static Regex {
    static FLAG: OnceLock<Regex> = OnceLock::new();
    FLAG.get_or_init(|| {
        Regex::new(r"--(?:session-id|resume)[ =]([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})").unwrap()
    })
}

/// `(boot_id, pid do claude)` subindo de `start`; `(None, None)` sem ancestral claude achado.
/// Para no pai ≤ 1, como o Python (um claude que é o pid 1 não é reconhecido).
pub fn boot_claude(start: u32, processes: &impl Processes) -> (Option<String>, Option<u32>) {
    let mut pid = start;
    for _ in 0..MAX_HOPS {
        let Some(argv) = processes.argv(pid) else { return (None, None) };
        if is_claude(&argv) {
            let joined = argv.join(" ");
            let boot = session_flag().captures(&joined).map(|caps| caps[1].to_owned());
            return (boot, Some(pid));
        }
        match processes.parent(pid) {
            Some(parent) if parent > 1 => pid = parent,
            _ => return (None, None),
        }
    }
    (None, None)
}

/// O pai deste processo (o shell que o agente abriu para o hook).
pub fn parent_pid() -> Option<u32> {
    #[cfg(unix)]
    {
        Some(std::os::unix::process::parent_id())
    }
    #[cfg(not(unix))]
    {
        System.parent(std::process::id())
    }
}

/// Os processos da máquina. Decide por capacidade (existe `/proc`?), não pelo nome do sistema.
pub struct System;

impl Processes for System {
    fn argv(&self, pid: u32) -> Option<Vec<String>> {
        if std::path::Path::new("/proc").is_dir() {
            let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
            return Some(String::from_utf8_lossy(&raw).split('\0').map(str::to_owned).collect());
        }
        without_proc::argv(pid)
    }

    fn parent(&self, pid: u32) -> Option<u32> {
        if std::path::Path::new("/proc").is_dir() {
            let stat = std::fs::read(format!("/proc/{pid}/stat")).ok()?;
            let stat = String::from_utf8_lossy(&stat);
            // O nome do processo (2º campo) pode ter espaço e parêntese: o ppid vem depois do último `)`.
            return stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok();
        }
        without_proc::parent(pid)
    }
}

#[cfg(not(target_os = "linux"))]
mod without_proc {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    fn read<T>(pid: u32, kind: ProcessRefreshKind, take: impl FnOnce(&sysinfo::Process) -> T) -> Option<T> {
        let pid = Pid::from_u32(pid);
        let mut system = System::new();
        system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, kind);
        system.process(pid).map(take)
    }

    pub fn argv(pid: u32) -> Option<Vec<String>> {
        let argv = read(pid, ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always), |process| {
            process.cmd().iter().map(|arg| arg.to_string_lossy().into_owned()).collect::<Vec<_>>()
        })?;
        // Sem argv legível (outro usuário, processo de sistema) o psutil levantava, e a subida parava.
        (!argv.is_empty()).then_some(argv)
    }

    pub fn parent(pid: u32) -> Option<u32> {
        read(pid, ProcessRefreshKind::nothing(), |process| process.parent().map(|parent| parent.as_u32())).flatten()
    }
}

/// Linux sem `/proc` montado: o Python caía no psutil, que lá não existe, e não achava nada.
#[cfg(target_os = "linux")]
mod without_proc {
    pub fn argv(_pid: u32) -> Option<Vec<String>> { None }
    pub fn parent(_pid: u32) -> Option<u32> { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn only_the_program_name_makes_a_claude() {
        assert!(is_claude(&argv(&["claude", "--resume"])));
        assert!(is_claude(&argv(&["/usr/bin/claude"])));
        assert!(is_claude(&argv(&["claude.exe"])));
        assert!(!is_claude(&argv(&["Claude"])));
        assert!(is_claude(&argv(&["node", "/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js"])));
        assert!(is_claude(&argv(&["node.exe", r"C:\npm\node_modules\@anthropic-ai\claude-code\cli.js"])));
        assert!(!is_claude(&argv(&["node", "/srv/claude-code-x/cli.js"])));
        assert!(!is_claude(&argv(&["sh", "-c", "/home/u/.hangar/worktrees/claude-headless/x"])));
        assert!(!is_claude(&[]));
    }
}
