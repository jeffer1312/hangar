//! Leitura de processos (`backend/app/procinfo.py`).
use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// ppid → filhos, na ordem da varredura.
pub type ChildrenMap = HashMap<i64, Vec<i64>>;

/// Acima da cadência do poll da lista (1,5 s): abaixo dela cada tique varreria de novo.
pub const CHILDREN_TTL: Duration = Duration::from_secs(3);

/// Receita efêmera: nunca serializar nem imprimir argv/ambiente em diário.
#[derive(Clone, PartialEq)]
pub struct ProcessRecipe {
    pub pid:i64,
    pub birth:f64,
    pub executable:PathBuf,
    pub cwd:PathBuf,
    pub argv:Vec<OsString>,
    pub environ:Vec<OsString>,
    pub cgroup:Option<Vec<u8>>,
}
impl std::fmt::Debug for ProcessRecipe {
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {
        f.debug_struct("ProcessRecipe").field("pid",&self.pid).field("birth",&self.birth).finish_non_exhaustive()
    }
}

/// O que a lista precisa saber de um processo. Processo que morreu ou é de outro dono responde
/// vazio/`None`; só a varredura inteira falhar é erro. E/S bloqueante: em contexto async, chamar
/// por `spawn_blocking`.
pub trait ProcessView: Send + Sync {
    /// Mapa reusado por até `max_age`; `Duration::ZERO` lê de novo (sessão criada há menos de 1 s).
    fn children(&self, max_age: Duration) -> io::Result<Arc<ChildrenMap>>;
    /// Quem implementa escolhe um dos dois pares (`argv`/`argv_checked`, `fds`/`fds_checked`).
    fn argv(&self, pid: i64) -> Vec<String> { self.argv_checked(pid).unwrap_or_default() }
    /// O argv, com o erro que o separa de um processo sem argv (`vanished` = morreu no caminho).
    fn argv_checked(&self, pid: i64) -> io::Result<Vec<String>> { Ok(self.argv(pid)) }
    /// Os pids que a rodada vai ler por inteiro (argv e ambiente), numa leitura só onde reler um
    /// pid custa o retrato de todos (`sysinfo`).
    fn prefetch(&self, _pids: &[i64]) {}
    fn cwd(&self, pid: i64) -> Option<PathBuf>;
    /// `Ok(None)` = variável ausente; `Err` = ambiente ilegível (a exclusão de conta separa os dois).
    fn env_var(&self, pid: i64, name: &str) -> io::Result<Option<OsString>>;
    /// Nascimento em segundos desde a época.
    fn start_time(&self, pid: i64) -> Option<f64>;
    /// Destinos dos descritores abertos.
    fn fds(&self, pid: i64) -> Vec<PathBuf> { self.fds_checked(pid).unwrap_or_default() }
    fn fds_checked(&self, pid: i64) -> io::Result<Vec<PathBuf>> { Ok(self.fds(pid)) }
    /// Leitura fresca completa; leitores sem essa prova não podem relançar um processo.
    fn recipe(&self, _pid:i64)->io::Result<ProcessRecipe> { Err(io::ErrorKind::Unsupported.into()) }
    /// Nome do executável (`comm` no Linux). Só a varredura de uso de conta pergunta.
    fn name(&self, _pid: i64) -> io::Result<String> { Err(io::ErrorKind::Unsupported.into()) }
    /// Ambiente coberto pelo título que o processo escreveu (Chromium/Electron): só zeros, ou pedaços
    /// da própria linha de comando. Só a varredura de uso de conta pergunta.
    fn environ_erased(&self, _pid: i64) -> bool { false }
}

#[derive(Debug)]
struct ProcessExited;
impl std::fmt::Display for ProcessExited {fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {f.write_str("processo terminou")}}
impl std::error::Error for ProcessExited {}
pub fn exited(error:&io::Error)->bool {error.get_ref().is_some_and(|cause|cause.is::<ProcessExited>())}

pub use hangar_core::process::vanished;

#[cfg(target_os = "linux")]
pub use linux::ProcFs;
#[cfg(not(target_os = "linux"))]
pub use other::SysInfo;

/// O leitor desta plataforma.
#[cfg(target_os = "linux")]
pub type SystemProcs = ProcFs;
#[cfg(not(target_os = "linux"))]
pub type SystemProcs = SysInfo;

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::sync::Mutex;
    use std::time::Instant;

    /// O ambiente lido de um pid: a rodada pergunta 5 a 8 variáveis do mesmo processo.
    /// O erro guarda o código do sistema: o `vanished` reconhece o ESRCH por ele.
    type Environ = (Instant, Result<Arc<[u8]>, (io::ErrorKind, Option<i32>)>);

    fn rebuild((kind, raw): (io::ErrorKind, Option<i32>)) -> io::Error {
        raw.map_or_else(|| kind.into(), io::Error::from_raw_os_error)
    }

    /// Abaixo da cadência do poll (1,5 s): serve uma rodada, nunca a seguinte.
    const ENVIRON_TTL: Duration = Duration::from_secs(1);

    pub struct ProcFs {
        root: PathBuf,
        cache: Mutex<Option<(Instant, Arc<ChildrenMap>)>>,
        /// O boot não muda enquanto o processo vive.
        btime: std::sync::OnceLock<f64>,
        environ: Mutex<crate::list::capped::Capped<i64, Environ>>,
    }

    impl Default for ProcFs {
        fn default() -> Self { Self::with_root("/proc") }
    }

    impl ProcFs {
        /// Raiz trocável só para o teste apontar para um /proc de mentira.
        pub fn with_root(root: impl Into<PathBuf>) -> Self {
            Self { root: root.into(), cache: Mutex::new(None), btime: std::sync::OnceLock::new(),
                environ: Mutex::new(crate::list::capped::Capped::new(crate::list::capped::SESSION_CAP)) }
        }

        /// Para varrer a máquina inteira (uso de conta): o ambiente de cada processo é lido uma vez,
        /// e o teto da lista despejaria entradas e anotaria o despejo no diário a cada varredura.
        pub fn whole_machine() -> Self {
            let procs = Self::default();
            *procs.environ.lock().unwrap_or_else(|e| e.into_inner()) = crate::list::capped::Capped::new(usize::MAX);
            procs
        }

        fn environ(&self, pid: i64) -> io::Result<Arc<[u8]>> {
            let mut cache = self.environ.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((at, read)) = cache.get(&pid)
                && at.elapsed() < ENVIRON_TTL
            {
                return read.clone().map_err(rebuild);
            }
            let read = std::fs::read(self.path(pid, "environ")).map(Arc::from).map_err(|e| (e.kind(), e.raw_os_error()));
            cache.insert(pid, (Instant::now(), read.clone()));
            read.map_err(rebuild)
        }

        fn path(&self, pid: i64, leaf: &str) -> PathBuf { self.root.join(pid.to_string()).join(leaf) }

        /// (ppid, nascimento em ticks). O comm (campo 2) pode ter espaço e parêntese: só o último
        /// ')' delimita.
        fn stat(&self, pid: i64) -> Option<(i64, u64)> {
            // `comm` aceita byte arbitrário (`prctl`): não pode tirar o processo do mapa.
            let raw = String::from_utf8_lossy(&std::fs::read(self.path(pid, "stat")).ok()?).into_owned();
            let mut fields = raw[raw.rfind(')')? + 1..].split_whitespace();
            let ppid = fields.nth(1)?.parse().ok()?;
            let start = fields.nth(17)?.parse().ok()?;
            Some((ppid, start))
        }

        /// Processo que some no meio da varredura fica de fora; raiz ilegível ou nenhum `stat` lido
        /// (`hidepid`) é erro, nunca um mapa vazio.
        fn scan(&self) -> io::Result<ChildrenMap> {
            let mut map = ChildrenMap::new();
            let mut seen = 0usize;
            for entry in std::fs::read_dir(&self.root)? {
                let Ok(entry) = entry else { continue };
                let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<i64>().ok()) else { continue };
                seen += 1;
                if let Some((ppid, _)) = self.stat(pid) {
                    map.entry(ppid).or_default().push(pid);
                }
            }
            if seen > 0 && map.is_empty() {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "nenhum stat legível"));
            }
            Ok(map)
        }

        fn btime(&self) -> Option<f64> {
            if let Some(b) = self.btime.get() {
                return Some(*b);
            }
            // Só o valor lido fica: uma falha guardada deixaria todo nascimento em branco.
            let raw = std::fs::read_to_string(self.root.join("stat")).ok()?;
            let b = raw.lines().find_map(|l| l.strip_prefix("btime ")).and_then(|v| v.trim().parse().ok())?;
            Some(*self.btime.get_or_init(|| b))
        }
    }

    /// `None` quando o sistema não responde: um nascimento inventado erraria todos os processos.
    fn clock_ticks() -> Option<f64> {
        static TICKS: std::sync::OnceLock<Option<f64>> = std::sync::OnceLock::new();
        *TICKS.get_or_init(|| {
            unsafe extern "C" { fn sysconf(name: i32) -> i64; }
            const SC_CLK_TCK: i32 = 2;
            // SAFETY: sysconf só lê um valor de configuração; 2 é _SC_CLK_TCK em glibc e musl.
            let ticks = unsafe { sysconf(SC_CLK_TCK) };
            (ticks > 0).then_some(ticks as f64)
        })
    }

    impl ProcessView for ProcFs {
        fn children(&self, max_age: Duration) -> io::Result<Arc<ChildrenMap>> {
            // Trava durante a varredura: N leitores que erram juntos varrem uma vez só.
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((at, map)) = cache.as_ref() {
                if at.elapsed() < max_age {
                    return Ok(map.clone());
                }
            }
            let map = Arc::new(self.scan()?);
            *cache = Some((Instant::now(), map.clone()));
            Ok(map)
        }

        fn recipe(&self,pid:i64)->io::Result<ProcessRecipe> {
            use std::os::unix::ffi::OsStringExt;
            let unreadable=||io::Error::new(io::ErrorKind::PermissionDenied,"receita do processo ilegível");
            let stat=std::fs::read(self.path(pid,"stat"))?;
            let birth=self.start_time(pid).ok_or_else(unreadable)?;
            let end=stat.iter().rposition(|b|*b==b')').ok_or_else(unreadable)?;
            if stat[end+1..].split(|b|b.is_ascii_whitespace()).find(|s|!s.is_empty())==Some(b"Z".as_slice()) {
                return Err(io::Error::new(io::ErrorKind::NotFound,ProcessExited));
            }
            let split=|mut raw:Vec<u8>| {
                if raw.last()==Some(&0) { raw.pop(); }
                raw.split(|b|*b==0).map(|s|OsString::from_vec(s.to_vec())).collect::<Vec<_>>()
            };
            let argv=std::fs::read(self.path(pid,"cmdline"))?;
            let environ=std::fs::read(self.path(pid,"environ"))?;
            if argv.is_empty() || environ.is_empty() { return Err(unreadable()); }
            let recipe=ProcessRecipe {pid,birth,executable:std::fs::read_link(self.path(pid,"exe"))?,
                cwd:std::fs::read_link(self.path(pid,"cwd"))?,argv:split(argv),environ:split(environ),
                cgroup:Some(std::fs::read(self.path(pid,"cgroup"))?)};
            if self.start_time(pid)!=Some(birth) { return Err(io::Error::other("identidade do processo mudou")); }
            Ok(recipe)
        }

        fn argv_checked(&self, pid: i64) -> io::Result<Vec<String>> {
            let raw = std::fs::read(self.path(pid, "cmdline"))?;
            Ok(raw.split(|b| *b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect())
        }

        fn cwd(&self, pid: i64) -> Option<PathBuf> { std::fs::read_link(self.path(pid, "cwd")).ok() }

        fn env_var(&self, pid: i64, name: &str) -> io::Result<Option<OsString>> {
            use std::os::unix::ffi::OsStringExt;
            let raw = self.environ(pid)?;
            let prefix = [name.as_bytes(), b"="].concat();
            Ok(raw.split(|b| *b == 0).find_map(|kv| kv.strip_prefix(prefix.as_slice()))
                .map(|v| OsString::from_vec(v.to_vec())))
        }

        fn start_time(&self, pid: i64) -> Option<f64> {
            let (_, ticks) = self.stat(pid)?;
            Some(self.btime()? + ticks as f64 / clock_ticks()?)
        }

        fn fds_checked(&self, pid: i64) -> io::Result<Vec<PathBuf>> {
            // Um fd fechado entre a listagem e o `readlink` é corrida, não falha.
            Ok(std::fs::read_dir(self.path(pid, "fd"))?.flatten().filter_map(|e| std::fs::read_link(e.path()).ok()).collect())
        }

        fn name(&self, pid: i64) -> io::Result<String> {
            Ok(String::from_utf8_lossy(&std::fs::read(self.path(pid, "comm"))?).trim().to_owned())
        }

        fn environ_erased(&self, pid: i64) -> bool {
            let (Ok(raw), Ok(title)) = (self.environ(pid), std::fs::read(self.path(pid, "cmdline"))) else { return false };
            // Um ambiente de verdade tem entradas que o argv não tem (`PATH`, `HOME`).
            !raw.is_empty() && raw.split(|b| *b == 0).filter(|chunk| !chunk.is_empty())
                .all(|chunk| title.windows(chunk.len()).any(|window| window == chunk))
        }
    }
}

// Compilado também no teste do Linux: é a única plataforma onde o CI local roda.
#[cfg(any(not(target_os = "linux"), test))]
mod other {
    use super::*;
    use std::sync::Mutex;
    use std::time::Instant;
    use sysinfo::{Pid, Process, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    struct Snapshot {
        system: System,
        /// Quando a tabela inteira foi relida (pai e nascimento) e o mapa que saiu dela.
        cache: Option<(Instant, Arc<ChildrenMap>)>,
        /// Pids desse retrato: fora dele, com o retrato fresco, o processo nasceu depois ou morreu.
        listed: std::collections::HashSet<Pid>,
        /// Pids com argv e ambiente lidos (`prefetch`); um relido sozinho só tem o que aquela
        /// leitura pediu.
        full: std::collections::HashSet<Pid>,
    }

    /// O ramo `psutil` do `procinfo.py`. No Windows toda releitura, mesmo de um pid, tira o retrato
    /// de todos os processos (`CreateToolhelp32Snapshot`): a tabela sai no modo mínimo e argv e
    /// ambiente só dos pids das sessões, numa leitura por tique.
    pub struct SysInfo {
        state: Mutex<Snapshot>,
        #[cfg(test)]
        pub(super) refreshes: std::sync::atomic::AtomicUsize,
    }

    impl SysInfo {
        /// O mesmo leitor: sem cache de ambiente com teto, nada a trocar.
        pub fn whole_machine() -> Self { Self::default() }
    }

    impl Default for SysInfo {
        fn default() -> Self {
            Self {
                state: Mutex::new(Snapshot { system: System::new(), cache: None, listed: Default::default(), full: Default::default() }),
                #[cfg(test)]
                refreshes: Default::default(),
            }
        }
    }

    impl SysInfo {
        fn counted(&self) {
            #[cfg(test)]
            self.refreshes.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }

        /// Do retrato da rodada quando `from_snapshot` e o pid já tem o que `kind` pede (`full`, ou
        /// só o básico com `kind` vazio); senão relê só `pid`, com só o que `kind` pede. Retrato
        /// fresco sem o pid responde `None` sem reler: no Windows cada releitura é um retrato inteiro,
        /// e um pid morto custaria um por tique. `None` = morto, nascido depois ou fora do alcance.
        fn with<T>(&self, pid: i64, from_snapshot: bool, kind: ProcessRefreshKind, read: impl FnOnce(&Process) -> T) -> Option<T> {
            let pid = Pid::from_u32(u32::try_from(pid).ok()?);
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let fresh = state.cache.as_ref().is_some_and(|(at, _)| at.elapsed() < CHILDREN_TTL);
            if fresh && !state.listed.contains(&pid) {
                return None;
            }
            let have = if kind == ProcessRefreshKind::nothing() { &state.listed } else { &state.full };
            if !(from_snapshot && fresh && have.contains(&pid)) {
                self.counted();
                state.system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, kind);
            }
            state.system.process(pid).map(read)
        }
    }

    fn as_i64(pid: Pid) -> i64 { i64::from(pid.as_u32()) }

    impl ProcessView for SysInfo {
        fn children(&self, max_age: Duration) -> io::Result<Arc<ChildrenMap>> {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((at, map)) = state.cache.as_ref() {
                if at.elapsed() < max_age {
                    return Ok(map.clone());
                }
            }
            self.counted();
            // Modo mínimo: pai e nascimento. Argv e ambiente de todos custavam uma leitura de
            // memória por processo da máquina; só os das sessões vêm, pelo `prefetch`.
            state.system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
            state.listed = state.system.processes().keys().copied().collect();
            state.full.clear();
            if state.listed.is_empty() {
                state.cache = None;
                return Err(io::Error::other("nenhum processo listado"));
            }
            // Em ordem de pid, como o `psutil.process_iter`; sem pai fica sob 0, como lá.
            let mut pids: Vec<(i64, i64)> = state.system.processes().iter()
                .map(|(pid, p)| (as_i64(*pid), p.parent().map_or(0, as_i64))).collect();
            pids.sort_unstable();
            let mut map = ChildrenMap::new();
            for (pid, ppid) in pids {
                map.entry(ppid).or_default().push(pid);
            }
            let map = Arc::new(map);
            state.cache = Some((Instant::now(), map.clone()));
            Ok(map)
        }

        fn prefetch(&self, pids: &[i64]) {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let fresh = state.cache.as_ref().is_some_and(|(at, _)| at.elapsed() < CHILDREN_TTL);
            let want: Vec<Pid> = pids.iter().filter_map(|p| u32::try_from(*p).ok()).map(Pid::from_u32)
                .filter(|p| !fresh || (state.listed.contains(p) && !state.full.contains(p))).collect();
            if want.is_empty() {
                return;
            }
            self.counted();
            // `Always`: no macOS o pid sobrevive ao exec, e o argv guardado seria o do shell.
            let kind = ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always).with_environ(UpdateKind::Always);
            state.system.refresh_processes_specifics(ProcessesToUpdate::Some(&want), true, kind);
            // Fora de um retrato fresco o `full` valeria até o próximo e o pid sairia sem reler.
            if fresh {
                let state = &mut *state;
                state.full.extend(want.into_iter().filter(|p| state.system.process(*p).is_some()));
            }
        }

        fn recipe(&self,pid:i64)->io::Result<ProcessRecipe> {
            let pid=Pid::from_u32(u32::try_from(pid).map_err(|_|io::ErrorKind::InvalidInput)?);
            // Retrato próprio: ausência no cache da lista não prova ausência de um relançamento.
            let mut system=System::new();
            let kind=ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always).with_exe(UpdateKind::Always)
                .with_environ(UpdateKind::Always).with_cwd(UpdateKind::Always);
            system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]),true,kind);
            let p=system.process(pid).ok_or(io::ErrorKind::NotFound)?;
            let unreadable=||io::Error::new(io::ErrorKind::PermissionDenied,"receita do processo ilegível");
            if p.cmd().is_empty() || p.environ().is_empty() || p.start_time()==0 { return Err(unreadable()); }
            #[cfg(windows)] let birth=crate::terminal_facts::agent_birth(pid.as_u32()).ok_or_else(unreadable)?;
            #[cfg(not(windows))] let birth=p.start_time() as f64;
            let recipe=ProcessRecipe {pid:as_i64(pid),birth,
                executable:p.exe().filter(|p|!p.as_os_str().is_empty()).ok_or_else(unreadable)?.to_owned(),
                cwd:p.cwd().filter(|p|!p.as_os_str().is_empty()).ok_or_else(unreadable)?.to_owned(),
                argv:p.cmd().to_vec(),environ:p.environ().to_vec(),cgroup:None};
            system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]),true,ProcessRefreshKind::nothing());
            #[cfg(windows)] let current_birth=crate::terminal_facts::agent_birth(pid.as_u32());
            #[cfg(not(windows))] let current_birth=system.process(pid).map(|p|p.start_time() as f64);
            if current_birth!=Some(recipe.birth) {return Err(io::Error::other("identidade do processo mudou"));}
            Ok(recipe)
        }

        /// Sumido do retrato é `NotFound` (corrida); vivo com argv vazio é o que o `sysinfo` devolve
        /// quando não consegue ler (outro dono, processo protegido).
        fn argv_checked(&self, pid: i64) -> io::Result<Vec<String>> {
            let argv = self.with(pid, true, ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
                |p| p.cmd().iter().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>());
            match argv {
                None => Err(io::ErrorKind::NotFound.into()),
                Some(argv) if argv.is_empty() => Err(io::Error::new(io::ErrorKind::PermissionDenied, "argv ilegível")),
                Some(argv) => Ok(argv),
            }
        }

        /// Fora do retrato: só a sessão sem terminal com a pasta renomeada pergunta.
        fn cwd(&self, pid: i64) -> Option<PathBuf> {
            self.with(pid, false, ProcessRefreshKind::nothing().with_cwd(UpdateKind::Always),
                |p| p.cwd().map(|c| c.to_path_buf())).flatten()
        }

        fn env_var(&self, pid: i64, name: &str) -> io::Result<Option<OsString>> {
            let prefix = [name.as_bytes(), b"="].concat();
            let found = self.with(pid, true, ProcessRefreshKind::nothing().with_environ(UpdateKind::Always), |p| {
                // Ambiente vazio é o que o `sysinfo` devolve quando não consegue ler (outro dono).
                (!p.environ().is_empty()).then(|| p.environ().iter().find_map(|kv| {
                    let value = kv.as_encoded_bytes().strip_prefix(prefix.as_slice())?;
                    // SAFETY: corte logo após o `=` ASCII, fronteira válida do encoding do OsStr.
                    Some(unsafe { std::ffi::OsStr::from_encoded_bytes_unchecked(value) }.to_os_string())
                }))
            });
            match found {
                None => Err(io::Error::new(io::ErrorKind::NotFound, "processo ausente")),
                Some(None) => Err(io::Error::new(io::ErrorKind::PermissionDenied, "ambiente ilegível")),
                Some(Some(value)) => Ok(value),
            }
        }

        fn start_time(&self, pid: i64) -> Option<f64> {
            // Segundos inteiros: o `psutil` dava fração, o `sysinfo` não tem.
            self.with(pid, true, ProcessRefreshKind::nothing(), |p| p.start_time())
                .filter(|t| *t > 0).map(|t| t as f64)
        }

        /// Fora do Linux o transcript não sai do fd aberto (`procinfo._open_jsonl`): enumerar
        /// handles leva segundos.
        fn fds(&self, _pid: i64) -> Vec<PathBuf> { Vec::new() }

        fn name(&self, pid: i64) -> io::Result<String> {
            self.with(pid, true, ProcessRefreshKind::nothing(), |p| p.name().to_string_lossy().into_owned())
                .ok_or_else(|| io::ErrorKind::NotFound.into())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::atomic::Ordering;

        fn sleeper(env: &str) -> std::process::Child {
            let mut c = if cfg!(windows) {
                let mut c = std::process::Command::new("ping");
                c.args(["-n", "30", "127.0.0.1"]);
                c
            } else {
                let mut c = std::process::Command::new("sleep");
                c.arg("30");
                c
            };
            c.env("HANGAR_PROCS_MARK", env).stdout(std::process::Stdio::null()).spawn().unwrap()
        }

        /// Depois do exec o argv é o do programa; antes dele, o do pai.
        fn wait_exec(pid: i64) {
            let deadline = Instant::now() + Duration::from_secs(5);
            let probe = SysInfo::default();
            while !probe.argv(pid).last().is_some_and(|a| a == "30" || a == "127.0.0.1") && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        #[test]
        fn one_snapshot_serves_the_round() {
            let mut a = sleeper("a");
            let pid_a = i64::from(a.id());
            wait_exec(pid_a);
            let procs = SysInfo::default();
            assert!(procs.children(Duration::ZERO).unwrap().get(&i64::from(std::process::id())).is_some_and(|k| k.contains(&pid_a)));
            assert!(procs.start_time(pid_a).is_some(), "o nascimento sai do retrato mínimo");
            procs.prefetch(&[pid_a, 1 << 30]);
            for _ in 0..3 {
                assert!(procs.argv(pid_a).last().is_some_and(|x| x == "30" || x == "127.0.0.1"));
                assert_eq!(procs.env_var(pid_a, "HANGAR_PROCS_MARK").unwrap(), Some(OsString::from("a")));
                assert!(procs.start_time(pid_a).is_some());
            }
            procs.prefetch(&[pid_a]);
            assert_eq!(procs.refreshes.load(Ordering::Relaxed), 2, "tabela mínima + argv e ambiente dos pids pedidos");
            // Nascido depois do retrato fresco: nem relido sozinho (no Windows seria outro retrato).
            let mut b = sleeper("b");
            let pid_b = i64::from(b.id());
            wait_exec(pid_b);
            assert!(procs.argv(pid_b).is_empty() && procs.start_time(pid_b).is_none());
            assert_eq!(procs.refreshes.load(Ordering::Relaxed), 2);
            procs.children(Duration::ZERO).unwrap();
            // Sem `prefetch`: relido sozinho só com o argv; o ambiente dele não pode sair vazio.
            assert!(!procs.argv(pid_b).is_empty());
            assert_eq!(procs.env_var(pid_b, "HANGAR_PROCS_MARK").unwrap(), Some(OsString::from("b")));
            assert_eq!(procs.refreshes.load(Ordering::Relaxed), 5);
            for c in [&mut a, &mut b] {
                c.kill().unwrap();
                c.wait().unwrap();
            }
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn add(root: &Path, pid: i64, ppid: i64, comm: &str, argv: &[&str]) {
        let dir = root.join(pid.to_string());
        fs::create_dir_all(dir.join("fd")).unwrap();
        // comm com espaço e parêntese: só o último ')' delimita os campos.
        let mut stat = format!("{pid} ({comm}) S {ppid}");
        for _ in 0..17 { stat.push_str(" 0"); }
        stat.push_str(" 500 0 0\n");
        fs::write(dir.join("stat"), stat).unwrap();
        let mut cmd = Vec::new();
        for a in argv { cmd.extend_from_slice(a.as_bytes()); cmd.push(0); }
        fs::write(dir.join("cmdline"), cmd).unwrap();
    }

    fn fake_proc() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("stat"), "cpu 1 2 3\nbtime 1000\n").unwrap();
        add(root.path(), 10, 1, "fish", &["fish"]);
        add(root.path(), 11, 10, "node (x)", &["/usr/bin/node", "claude", "--session-id", "abc"]);
        add(root.path(), 12, 10, "tmux", &["tmux"]);
        root
    }

    #[test]
    fn absent_pid_and_definite_exit_are_not_unreadable_environment() {
        let root=fake_proc();let procs=ProcFs::with_root(root.path());
        assert_eq!(procs.recipe(99).unwrap_err().kind(),io::ErrorKind::NotFound);
        let path=root.path().join("11/stat");
        let stat=fs::read_to_string(&path).unwrap().replace(") S ",") Z ");fs::write(path,stat).unwrap();
        assert!(exited(&procs.recipe(11).unwrap_err()));
        assert!(crate::runtime::lifecycle::process_absent(&procs,11,1005.0).unwrap());
    }
    #[test]
    fn recipe_preserves_os_bytes_and_refuses_unreadable_environment() {
        use std::os::unix::ffi::OsStrExt;
        let root = fake_proc();
        let dir = root.path().join("11");
        fs::write(dir.join("cmdline"), b"/usr/bin/claude\0--settings\0/x/\xff.json\0\0").unwrap();
        fs::write(dir.join("environ"), b"A=\xff\0EMPTY=\0").unwrap();
        std::os::unix::fs::symlink("/usr/bin/claude", dir.join("exe")).unwrap();
        std::os::unix::fs::symlink("/work/dir", dir.join("cwd")).unwrap();
        fs::write(dir.join("cgroup"), b"0::/user.slice/session.scope\n").unwrap();
        let procs = ProcFs::with_root(root.path());
        let recipe = procs.recipe(11).unwrap();
        assert_eq!(recipe.argv[2].as_bytes(), b"/x/\xff.json");
        assert_eq!(recipe.argv[3].as_bytes(), b"");
        assert_eq!(recipe.environ[0].as_bytes(), b"A=\xff");
        assert_eq!(recipe.birth, 1005.0);
        assert_eq!(recipe.cgroup.as_deref(), Some(b"0::/user.slice/session.scope\n".as_slice()));
        fs::write(dir.join("environ"), b"").unwrap();
        assert_eq!(procs.recipe(11).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        fs::remove_file(dir.join("environ")).unwrap();
        assert!(procs.recipe(11).is_err());
    }

    #[test]
    fn reads_children_and_argv() {
        let root = fake_proc();
        let dir = root.path().join("11");
        fs::write(dir.join("environ"), b"A=1\0CLAUDE_CONFIG_DIR=/x/\xffy\0").unwrap();
        std::os::unix::fs::symlink("/work/dir", dir.join("cwd")).unwrap();
        std::os::unix::fs::symlink("/p/s.jsonl", dir.join("fd").join("3")).unwrap();
        fs::create_dir(root.path().join("self")).unwrap();
        let procs = ProcFs::with_root(root.path());

        let map = procs.children(CHILDREN_TTL).unwrap();
        let mut kids = map.get(&10).cloned().unwrap_or_default();
        kids.sort();
        assert_eq!(kids, vec![11, 12]);
        assert_eq!(map.get(&1), Some(&vec![10]));
        assert_eq!(procs.argv(11), ["/usr/bin/node", "claude", "--session-id", "abc"]);
        assert_eq!(procs.cwd(11), Some(PathBuf::from("/work/dir")));
        use std::os::unix::ffi::OsStrExt;
        assert_eq!(procs.env_var(11, "CLAUDE_CONFIG_DIR").unwrap().unwrap().as_bytes(), b"/x/\xffy");
        assert_eq!(procs.env_var(11, "CP_ENGINE").unwrap(), None);
        assert!(procs.env_var(12, "CP_ENGINE").is_err(), "sem environ legível é erro, não ausência");
        assert_eq!(procs.fds(11), [PathBuf::from("/p/s.jsonl")]);
        // 500 ticks / 100 Hz depois do boot em 1000.
        assert_eq!(procs.start_time(11), Some(1005.0));
        fs::write(root.path().join("stat"), "btime 2000\n").unwrap();
        assert_eq!(procs.start_time(11), Some(1005.0), "o boot é lido uma vez");
        assert_eq!(procs.argv(99), Vec::<String>::new());
        assert_eq!(procs.start_time(99), None);
    }

    #[test]
    fn environ_overwritten_by_the_process_title_is_erased() {
        let root = fake_proc();
        let procs = ProcFs::with_root(root.path());
        fs::write(root.path().join("11/environ"), vec![0u8; 3120]).unwrap();
        fs::write(root.path().join("12/environ"), b"A=1\0").unwrap();
        assert!(procs.environ_erased(11), "só zeros: o título foi escrito por cima");
        assert!(!procs.environ_erased(12));
        assert!(!procs.environ_erased(10), "ilegível não é apagado");
        // Título maior que o argv original: o resto dele transborda para a área do ambiente.
        add(root.path(), 13, 10, "claude", &["/usr/lib/app/claude --type=gpu-process --ozone-platform=wayland --render=x"]);
        let mut spilled = b"m=wayland --render=x\0".to_vec();
        spilled.resize(3120, 0);
        fs::write(root.path().join("13/environ"), spilled).unwrap();
        assert!(procs.environ_erased(13), "o que há no ambiente é pedaço do título");
        fs::write(root.path().join("10/environ"), b"").unwrap();
        assert!(!ProcFs::with_root(root.path()).environ_erased(10), "vazio não é apagado");
    }

    #[test]
    fn environ_is_read_once_per_round_and_errors_are_kept() {
        let root = fake_proc();
        let dir = root.path().join("11");
        fs::write(dir.join("environ"), b"A=1\0B=2\0").unwrap();
        let procs = ProcFs::with_root(root.path());
        assert_eq!(procs.env_var(11, "A").unwrap(), Some(OsString::from("1")));
        fs::write(dir.join("environ"), b"A=9\0").unwrap();
        assert_eq!(procs.env_var(11, "B").unwrap(), Some(OsString::from("2")), "a rodada lê o environ uma vez");
        assert_eq!(procs.env_var(12, "A").unwrap_err().kind(), io::ErrorKind::NotFound, "o erro também fica");
        assert!(procs.argv_checked(99).is_err_and(|e| vanished(&e)), "morto é corrida, não argv vazio");
        assert!(procs.fds_checked(99).is_err_and(|e| vanished(&e)));
        assert_eq!(procs.argv_checked(11).unwrap().len(), 4);
    }

    #[test]
    fn fresh_read_sees_new_child() {
        let root = fake_proc();
        let procs = ProcFs::with_root(root.path());
        assert!(procs.children(CHILDREN_TTL).unwrap().get(&11).is_none());
        add(root.path(), 13, 11, "claude", &["claude"]);
        assert!(procs.children(CHILDREN_TTL).unwrap().get(&11).is_none(), "dentro do prazo reusa o mapa");
        assert_eq!(procs.children(Duration::ZERO).unwrap().get(&11), Some(&vec![13]));
    }

    #[test]
    fn unreadable_root_is_error() {
        let procs = ProcFs::with_root("/nonexistent/proc");
        assert!(procs.children(Duration::ZERO).is_err());
        let hidden = tempfile::tempdir().unwrap();
        fs::create_dir(hidden.path().join("10")).unwrap();
        assert!(ProcFs::with_root(hidden.path()).children(Duration::ZERO).is_err(), "nenhum stat legível");
    }

    #[test]
    fn invalid_utf8_comm_keeps_process() {
        let root = fake_proc();
        let dir = root.path().join("20");
        fs::create_dir(&dir).unwrap();
        let mut stat = b"20 (a\xff b) S 10".to_vec();
        stat.extend(" 0".repeat(17).bytes());
        stat.extend(b" 7 0 0\n");
        fs::write(dir.join("stat"), stat).unwrap();
        let procs = ProcFs::with_root(root.path());
        assert!(procs.children(Duration::ZERO).unwrap()[&10].contains(&20));
        assert_eq!(procs.start_time(20), Some(1000.07));
    }
}

#[cfg(test)]
mod platform_tests {
    use super::*;
    use std::time::Instant;

    /// Filho real lido pelo leitor da plataforma (`/proc` no Linux, `sysinfo` nos outros).
    #[test]
    fn sysinfo_reads_argv_and_env() {
        let dir = tempfile::tempdir().unwrap();
        let mut command = if cfg!(windows) {
            let mut c = std::process::Command::new("ping");
            c.args(["-n", "30", "127.0.0.1"]);
            c
        } else {
            let mut c = std::process::Command::new("sleep");
            c.arg("30");
            c
        };
        let mut child = command.current_dir(dir.path()).env("PSMUX_SESSION", "sim")
            .stdout(std::process::Stdio::null()).spawn().unwrap();
        let pid = i64::from(child.id());
        let procs = SystemProcs::default();
        // Logo após o spawn o processo ainda pode ter o argv do pai (antes do exec).
        let deadline = Instant::now() + Duration::from_secs(5);
        let argv = loop {
            let argv = procs.argv(pid);
            if argv.last().is_some_and(|a| a == "30" || a == "127.0.0.1") || Instant::now() > deadline { break argv; }
            std::thread::sleep(Duration::from_millis(50));
        };
        let env = procs.env_var(pid, "PSMUX_SESSION");
        let absent = procs.env_var(pid, "HANGAR_PROCS_ABSENT");
        let cwd = procs.cwd(pid);
        let start = procs.start_time(pid);
        let kids = procs.children(Duration::ZERO).unwrap().get(&i64::from(std::process::id())).cloned();
        child.kill().unwrap();
        child.wait().unwrap();

        assert!(argv.last().is_some_and(|a| a == "30" || a == "127.0.0.1"), "argv: {argv:?}");
        assert_eq!(env.unwrap(), Some(OsString::from("sim")));
        assert_eq!(absent.unwrap(), None);
        let same = |p: &std::path::Path| std::fs::canonicalize(p).unwrap();
        assert_eq!(cwd.as_deref().map(same), Some(same(dir.path())));
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
        assert!(start.is_some_and(|s| (now - s).abs() < 120.0), "start: {start:?}, now: {now}");
        assert!(kids.is_some_and(|k| k.contains(&pid)), "o filho aparece no mapa sob o pai");
        let gone = SystemProcs::default();
        assert!(gone.argv(pid).is_empty() && gone.cwd(pid).is_none(), "processo morto responde vazio");
        assert!(gone.env_var(pid, "PSMUX_SESSION").is_err(), "morto é ilegível, não ausência");
    }
}
