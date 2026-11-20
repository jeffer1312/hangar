//! Bloco Sessões: quantas por provedor, quanto os processos delas consomem, caídas, adormecidas e fila.
//! Recebe tudo já coletado (linhas da lista, pids de cada sessão, mapa de filhos, leitura de processo):
//! nenhuma leitura de multiplexador ou de registro mora aqui.
use std::collections::{BTreeMap, HashMap, HashSet};

use hangar_api::session::SessionRow;
use hangar_api::status::{DeadSession, SessionsBlock, Usage};

/// ppid → filhos, no formato do `ChildrenMap` do servidor (`list/procs.rs`).
pub type Children = HashMap<i64, Vec<i64>>;

const PROVIDERS: [&str; 5] = ["claude", "codex", "pi", "omp", "kimi"];

/// Grupo de processo de `SessionsBlock.by_process`, na ordem em que sai.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ProcessGroup {
    Claude,
    CodexAppServer,
    PiOmpKimi,
    Cano,
    Tmux,
}

impl ProcessGroup {
    pub fn key(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::CodexAppServer => "codex_app_server",
            Self::PiOmpKimi => "pi_omp_kimi",
            Self::Cano => "cano",
            Self::Tmux => "tmux",
        }
    }

    /// Pelo nome do executável. O Claude instalado pelo instalador nativo roda com o nome da versão
    /// (`2.1.40`), e o Pi/omp como `node`: por isso a raiz da sessão traz o grupo dela (`Root::group`).
    /// A TUI do Codex também se chama `codex` e entra junto com o app-server.
    pub fn from_executable(name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        let base = lower.strip_suffix(".exe").unwrap_or(&lower);
        match base {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::CodexAppServer),
            "pi" | "omp" | "kimi" => Some(Self::PiOmpKimi),
            "hangar-cano" => Some(Self::Cano),
            "tmux" | "psmux" => Some(Self::Tmux),
            // O servidor do tmux troca o título para `tmux: server`.
            _ if base.starts_with("tmux:") => Some(Self::Tmux),
            _ => None,
        }
    }

    /// O grupo do agente de cada provedor, para a raiz cujo nome não diz quem é.
    pub fn of_provider(provider: &str) -> Option<Self> {
        match provider {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::CodexAppServer),
            "pi" | "omp" | "kimi" => Some(Self::PiOmpKimi),
            _ => None,
        }
    }
}

/// Uma leitura de processo. `cpu_pct` em porcentagem de um núcleo; ausente antes da segunda amostra.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcStat {
    pub name: String,
    pub rss_bytes: u64,
    pub cpu_pct: Option<f64>,
}

/// Quem lê memória e CPU por pid. `None` = processo que sumiu ou não pôde ser lido.
pub trait ProcessReading {
    fn read(&self, pid: u32) -> Option<ProcStat>;
}

/// A CPU do `sysinfo` só vale depois de duas atualizações do mesmo `System`: quem chama garante isso.
impl ProcessReading for sysinfo::System {
    fn read(&self, pid: u32) -> Option<ProcStat> {
        let process = self.process(sysinfo::Pid::from_u32(pid))?;
        Some(ProcStat {
            name: process.name().to_string_lossy().into_owned(),
            rss_bytes: process.memory(),
            cpu_pct: Some(f64::from(process.cpu_usage())),
        })
    }
}

/// Um processo de onde a árvore da sessão parte (pane, agente, app-server do Codex, cano). `group` é o
/// que a fonte do pid já sabe; vale quando o nome do executável não diz.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Root {
    pub pid: u32,
    pub group: Option<ProcessGroup>,
}

/// A fila de uma sessão: quantas esperam e o relógio da mais antiga (`queue::oldest_waiting`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct QueueWait {
    pub waiting: u32,
    pub oldest_ts: Option<f64>,
}

/// O que o bloco precisa, já coletado. `pids` por nome de sessão; `tmux_server` é o pid do servidor do
/// multiplexador, contado uma vez; `now` em segundos desde a época.
pub struct Facts<'a> {
    pub rows: &'a [SessionRow],
    pub pids: &'a HashMap<String, Vec<Root>>,
    pub tmux_server: Option<u32>,
    pub children: &'a Children,
    pub queues: &'a [QueueWait],
    pub now: f64,
}

#[derive(Default)]
struct Acc {
    count: u32,
    rss: Option<u64>,
    cpu: Option<f64>,
}

impl Acc {
    fn add(&mut self, stat: &ProcStat) {
        self.rss = Some(self.rss.unwrap_or(0) + stat.rss_bytes);
        if let Some(cpu) = stat.cpu_pct { self.cpu = Some(self.cpu.unwrap_or(0.0) + cpu); }
    }

    fn usage(self, key: &str) -> Usage {
        Usage { key: key.to_owned(), count: self.count, rss_bytes: self.rss, cpu_pct: self.cpu }
    }
}

fn is_dead(row: &SessionRow) -> bool { row.state == "dead" }

/// Consumo por provedor e por grupo de processo. Cada sessão soma a árvore inteira das raízes dela; um
/// processo entra uma vez só, mesmo alcançável por duas sessões. No grupo, o filho herda o grupo do
/// ancestral mais próximo (o MCP do Claude conta no Claude); processo sem grupo nenhum na linha (o shell
/// do pane) conta no provedor e fica fora de `by_process`. `count` é sessões no provedor e processos
/// no grupo; linha `dead` fica de fora.
pub fn usage(rows: &[SessionRow], pids: &HashMap<String, Vec<Root>>, tmux_server: Option<u32>, children: &Children,
    procs: &dyn ProcessReading) -> (Vec<Usage>, Vec<Usage>) {
    let mut seen = HashSet::new();
    let mut providers: BTreeMap<&str, Acc> = BTreeMap::new();
    let mut groups: BTreeMap<ProcessGroup, Acc> = BTreeMap::new();
    if let Some(pid) = tmux_server && seen.insert(pid) && let Some(stat) = procs.read(pid) {
        let acc = groups.entry(ProcessGroup::Tmux).or_default();
        acc.count += 1;
        acc.add(&stat);
    }
    for row in rows.iter().filter(|row| !is_dead(row)) {
        let provider = providers.entry(row.provider.as_str()).or_default();
        provider.count += 1;
        let mut stack: Vec<(u32, Option<ProcessGroup>)> =
            pids.get(&row.name).into_iter().flatten().map(|root| (root.pid, root.group)).collect();
        while let Some((pid, inherited)) = stack.pop() {
            if !seen.insert(pid) { continue; }
            // Sumiu entre o mapa e a leitura: os filhos dele foram herdados por outro processo.
            let Some(stat) = procs.read(pid) else { continue };
            provider.add(&stat);
            let group = ProcessGroup::from_executable(&stat.name).or(inherited);
            if let Some(group) = group {
                let acc = groups.entry(group).or_default();
                acc.count += 1;
                acc.add(&stat);
            }
            for child in children.get(&i64::from(pid)).into_iter().flatten() {
                if let Ok(child) = u32::try_from(*child) { stack.push((child, group)); }
            }
        }
    }
    let rank = |key: &str| PROVIDERS.iter().position(|p| *p == key).unwrap_or(PROVIDERS.len());
    let mut by_provider: Vec<(&str, Acc)> = providers.into_iter().collect();
    by_provider.sort_by_key(|entry| rank(entry.0));
    (by_provider.into_iter().map(|(key, acc)| acc.usage(key)).collect(),
        groups.into_iter().map(|(group, acc)| acc.usage(group.key())).collect())
}

/// O bloco inteiro. `open` não conta as caídas; as adormecidas contam como abertas e também em `asleep`.
/// Caída sem código na linha sai com a causa `unknown`.
pub fn sessions_block(facts: &Facts, procs: &dyn ProcessReading) -> SessionsBlock {
    let (by_provider, by_process) = usage(facts.rows, facts.pids, facts.tmux_server, facts.children, procs);
    let dead: Vec<DeadSession> = facts.rows.iter().filter(|row| is_dead(row))
        .map(|row| DeadSession { name: row.name.clone(), cause: row.problema.clone().unwrap_or_else(|| "unknown".into()) })
        .collect();
    let live = facts.rows.iter().filter(|row| !is_dead(row));
    let open = u32::try_from(live.clone().count()).unwrap_or(u32::MAX);
    let asleep = u32::try_from(live.filter(|row| row.asleep).count()).unwrap_or(u32::MAX);
    let queued = facts.queues.iter().map(|q| q.waiting).fold(0u32, u32::saturating_add);
    let oldest = facts.queues.iter().filter(|q| q.waiting > 0).filter_map(|q| q.oldest_ts).reduce(f64::min);
    // Relógio da fila à frente do nosso (outra máquina gravou) vale zero, nunca negativo.
    let oldest_queued_secs = oldest.map(|ts| (facts.now - ts).max(0.0) as u64);
    SessionsBlock { open, by_provider, by_process, dead, asleep, queued, oldest_queued_secs }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(HashMap<u32, ProcStat>);

    impl Fake {
        fn new(items: &[(u32, &str, u64, f64)]) -> Self {
            Self(items.iter().map(|(pid, name, rss, cpu)| (*pid, ProcStat { name: (*name).into(), rss_bytes: *rss, cpu_pct: Some(*cpu) })).collect())
        }
    }

    impl ProcessReading for Fake {
        fn read(&self, pid: u32) -> Option<ProcStat> { self.0.get(&pid).cloned() }
    }

    fn row(name: &str, provider: &str) -> SessionRow {
        serde_json::from_value(serde_json::json!({"name": name, "provider": provider})).unwrap()
    }

    fn find<'a>(list: &'a [Usage], key: &str) -> &'a Usage { list.iter().find(|u| u.key == key).unwrap_or_else(|| panic!("{key} ausente: {list:?}")) }

    fn roots(items: Vec<(&str, Vec<Root>)>) -> HashMap<String, Vec<Root>> {
        items.into_iter().map(|(name, list)| (name.to_owned(), list)).collect()
    }

    #[test]
    fn sums_by_provider_counts_tmux_once_and_adds_the_claude_child_to_claude() {
        // tmux(1) → bash(10) → claude "2.1.40"(11) → node MCP(12); tmux(1) → bash(20) → codex TUI(21);
        // app-server do Codex(30) fora do pane; segunda sessão Claude: bash(40) → claude(41).
        let procs = Fake::new(&[
            (1, "tmux: server", 8, 0.5),
            (10, "bash", 4, 0.0), (11, "2.1.40", 300, 10.0), (12, "node", 100, 2.0),
            (20, "bash", 4, 0.0), (21, "codex", 50, 1.0), (30, "codex", 70, 3.0),
            (40, "bash", 4, 0.0), (41, "claude", 200, 5.0),
        ]);
        let children: Children = HashMap::from([(1, vec![10, 20, 40]), (10, vec![11]), (11, vec![12]), (20, vec![21]), (40, vec![41])]);
        let rows = vec![row("a", "claude"), row("b", "codex"), row("c", "claude")];
        let claude = Some(ProcessGroup::Claude);
        let pids = roots(vec![
            ("a", vec![Root { pid: 10, group: None }, Root { pid: 11, group: claude }]),
            ("b", vec![Root { pid: 20, group: None }, Root { pid: 30, group: Some(ProcessGroup::CodexAppServer) }]),
            ("c", vec![Root { pid: 40, group: None }, Root { pid: 41, group: claude }]),
        ]);
        let (by_provider, by_process) = usage(&rows, &pids, Some(1), &children, &procs);

        assert_eq!(by_provider.iter().map(|u| u.key.as_str()).collect::<Vec<_>>(), ["claude", "codex"]);
        let c = find(&by_provider, "claude");
        assert_eq!((c.count, c.rss_bytes, c.cpu_pct), (2, Some(4 + 300 + 100 + 4 + 200), Some(17.0)), "o pid 11 alcançado duas vezes conta uma");
        let x = find(&by_provider, "codex");
        assert_eq!((x.count, x.rss_bytes), (1, Some(4 + 50 + 70)));

        let claude = find(&by_process, "claude");
        assert_eq!((claude.count, claude.rss_bytes), (3, Some(300 + 100 + 200)), "o node filho do Claude soma no Claude");
        let tmux = find(&by_process, "tmux");
        assert_eq!((tmux.count, tmux.rss_bytes), (1, Some(8)), "o servidor do tmux conta uma vez, sem os panes");
        assert_eq!(find(&by_process, "codex_app_server").count, 2);
        assert!(by_process.iter().all(|u| u.key != "cano" && u.key != "pi_omp_kimi"), "grupo sem processo não sai");
    }

    #[test]
    fn a_vanished_process_and_its_subtree_are_skipped() {
        let procs = Fake::new(&[(11, "claude", 300, 1.0)]);
        let children: Children = HashMap::from([(10, vec![11])]);
        let pids = roots(vec![("a", vec![Root { pid: 10, group: None }])]);
        let (by_provider, by_process) = usage(&[row("a", "claude")], &pids, None, &children, &procs);
        let a = find(&by_provider, "claude");
        assert_eq!((a.count, a.rss_bytes), (1, None), "a sessão conta, a memória fica sem fonte");
        assert!(by_process.is_empty());
    }

    #[test]
    fn the_block_counts_dead_asleep_and_the_oldest_queue() {
        let mut dead = row("morta", "claude");
        dead.state = "dead".into();
        dead.problema = Some("terminal_out_of_memory".into());
        let mut gone = row("sem-codigo", "codex");
        gone.state = "dead".into();
        let mut sleeping = row("dorme", "claude");
        sleeping.asleep = true;
        let rows = vec![dead, gone, sleeping, row("viva", "pi")];
        let pids = HashMap::new();
        let children = Children::new();
        let queues = [QueueWait { waiting: 2, oldest_ts: Some(900.0) }, QueueWait { waiting: 1, oldest_ts: Some(700.0) },
            QueueWait { waiting: 0, oldest_ts: Some(100.0) }];
        let facts = Facts { rows: &rows, pids: &pids, tmux_server: None, children: &children, queues: &queues, now: 1000.0 };
        let block = sessions_block(&facts, &Fake::new(&[]));
        assert_eq!((block.open, block.asleep, block.queued, block.oldest_queued_secs), (2, 1, 3, Some(300)));
        assert_eq!(block.dead, vec![
            DeadSession { name: "morta".into(), cause: "terminal_out_of_memory".into() },
            DeadSession { name: "sem-codigo".into(), cause: "unknown".into() },
        ]);
        assert_eq!(block.by_provider.iter().map(|u| (u.key.as_str(), u.count)).collect::<Vec<_>>(), [("claude", 1), ("pi", 1)]);
    }

    #[test]
    fn executable_names_map_to_groups() {
        assert_eq!(ProcessGroup::from_executable("hangar-cano.exe"), Some(ProcessGroup::Cano));
        assert_eq!(ProcessGroup::from_executable("Claude.exe"), Some(ProcessGroup::Claude));
        assert_eq!(ProcessGroup::from_executable("psmux.exe"), Some(ProcessGroup::Tmux));
        assert_eq!(ProcessGroup::from_executable("kimi"), Some(ProcessGroup::PiOmpKimi));
        assert_eq!(ProcessGroup::from_executable("bash"), None);
    }
}
