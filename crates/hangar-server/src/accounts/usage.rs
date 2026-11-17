//! Fatos de uso de uma conta lidos aqui: processos da máquina, nascimentos de terminal publicados
//! e sidecars de sessão. Não adquirem a proteção de existência (o chamador pode já ter
//! exclusividade), e leitura parcial nunca prova que a conta está livre.
use super::{AccountKey, AccountService, Provider, UsageFacts, environment::expand};
use crate::list::procs::{ProcessView, vanished};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    io,
    path::{Path, PathBuf},
    time::Duration,
};

const CLIENTS: [&str; 4] = ["claude", "codex", "pi", "omp"];
const HOSTS: [&str; 13] = ["node", "bun", "python", "python3", "python3.14", "hangar-cano",
    "bash", "sh", "zsh", "fish", "cmd", "powershell", "pwsh"];

/// Nome da sessão do multiplexador → `pid:$id:criação` do servidor que a hospeda.
pub(crate) type Instances = BTreeMap<String, String>;

/// Erro que deixa os fatos incompletos.
struct Unknown;

/// Processo que morreu no meio da leitura fica de fora; o resto ilegível é dúvida.
enum Skip {
    Gone,
    Unknown,
}

impl From<io::Error> for Skip {
    fn from(error: io::Error) -> Self {
        if vanished(&error) { Skip::Gone } else { Skip::Unknown }
    }
}

/// Nome final e componentes de cada argumento; a barra invertida também separa.
fn argv_parts(argv: &[String]) -> (HashSet<String>, HashSet<String>) {
    let (mut names, mut parts) = (HashSet::new(), HashSet::new());
    for item in argv {
        let lower = item.to_lowercase();
        let pieces: Vec<&str> = lower.split(['/', '\\']).collect();
        let last = pieces.last().copied().unwrap_or("");
        let last = last.strip_suffix(".exe").unwrap_or(last);
        names.insert(last.strip_suffix(".js").unwrap_or(last).to_owned());
        parts.extend(pieces.into_iter().map(str::to_owned));
    }
    (names, parts)
}

/// `~` do usuário e caminho relativo à pasta do processo; pasta ilegível não resolve.
fn owner(provider: Provider, value: &str, cwd: impl FnOnce() -> Option<PathBuf>, home: &Path) -> Option<AccountKey> {
    let path = expand(value, home);
    let path = if path.is_absolute() { path } else { cwd()?.join(path) };
    AccountKey::new(provider, &path).ok()
}

fn variable(provider: Provider) -> &'static str {
    if provider == Provider::Codex { "CODEX_HOME" } else { "CLAUDE_CONFIG_DIR" }
}

/// Herdeiro da variável da conta (tmux, MCP, shell): carrega a conta sem usá-la. Ambiente
/// ilegível não carrega.
fn holds(procs: &dyn ProcessView, pid: i64, key: &AccountKey, home: &Path) -> bool {
    let Ok(Some(value)) = procs.env_var(pid, variable(key.provider)) else { return false };
    let value = value.to_string_lossy();
    !value.is_empty() && owner(key.provider, &value, || procs.cwd(pid), home).as_ref() == Some(key)
}

/// `Ok(true)` = o CLI usa a conta. Só o CLI conta: palavra solta no argv de um MCP ou shell, não.
fn uses(procs: &dyn ProcessView, pid: i64, key: &AccountKey, home: &Path) -> Result<bool, Skip> {
    let name = procs.name(pid)?.to_lowercase();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    if !CLIENTS.contains(&name) && !HOSTS.contains(&name) {
        return Ok(false);
    }
    let before = procs.start_time(pid).ok_or(Skip::Gone)?;
    let argv = procs.argv_checked(pid)?;
    // Argv vazio no `/proc` é processo que já saiu (zumbi), como o `ZombieProcess` do psutil.
    if argv.is_empty() {
        return Err(Skip::Gone);
    }
    let (names, parts) = argv_parts(&argv);
    let pertinent = CLIENTS.contains(&name) || CLIENTS.iter().any(|c| names.contains(*c))
        || parts.contains("claude-code") || argv.iter().any(|a| a.contains("cano.py") || a.contains("hangar-cano"));
    if !pertinent {
        return Ok(false);
    }
    let mut value = procs.env_var(pid, variable(key.provider))?.map(|v| v.to_string_lossy().into_owned());
    if key.provider == Provider::Codex {
        for (index, argument) in argv.iter().enumerate() {
            if argument == "--codex-home" {
                value = Some(argv.get(index + 1).ok_or(Skip::Unknown)?.clone());
            } else if let Some(rest) = argument.strip_prefix("--codex-home=") {
                value = Some(rest.to_owned());
            }
        }
        if value.as_deref() == Some("") {
            return Err(Skip::Unknown);
        }
    }
    let value = match value.filter(|v| !v.is_empty()) {
        Some(value) => value,
        None => {
            // O ambiente pode conter as duas contas herdadas; só o provider pertinente usa o padrão.
            let expected: &[&str] = if key.provider == Provider::Codex { &["codex"] } else { &["claude", "pi", "omp"] };
            if !expected.contains(&name) && !expected.iter().any(|e| names.contains(*e)) {
                return Ok(false);
            }
            // Sem variável porque o título cobriu o ambiente: app Chromium/Electron (o Claude Desktop se
            // chama `claude`), não CLI de conta.
            if procs.environ_erased(pid) {
                return Ok(false);
            }
            // O padrão é o da casa do processo: o CLI resolve `~` pelo ambiente dele.
            let own = procs.env_var(pid, if cfg!(windows) { "USERPROFILE" } else { "HOME" })?
                .filter(|h| !h.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.to_owned());
            own.join(if key.provider == Provider::Codex { ".codex" } else { ".claude" }).to_string_lossy().into_owned()
        }
    };
    let owner = owner(key.provider, &value, || procs.cwd(pid), home).ok_or(Skip::Unknown)?;
    // Pid reaproveitado entre as leituras não pode emprestar o argv de outro processo.
    if procs.start_time(pid).ok_or(Skip::Gone)? != before {
        return Err(Skip::Unknown);
    }
    Ok(owner == *key)
}

/// Ambiente ilegível e identidade trocada nunca provam ausência de uso. E/S bloqueante.
pub(crate) fn inspect_processes(procs: &dyn ProcessView, key: &AccountKey, home: &Path) -> UsageFacts {
    let mut facts = UsageFacts { complete: true, ..Default::default() };
    let Ok(map) = procs.children(Duration::ZERO) else {
        facts.complete = false;
        return facts;
    };
    let mut pids: Vec<i64> = map.values().flatten().copied().collect();
    pids.sort_unstable();
    pids.dedup();
    procs.prefetch(&pids);
    for pid in pids {
        let Ok(id) = u32::try_from(pid) else { continue };
        if holds(procs, pid, key, home) {
            facts.holders.push(id);
        }
        match uses(procs, pid, key, home) {
            Ok(true) => facts.pids.push(id),
            Ok(false) | Err(Skip::Gone) => {}
            Err(Skip::Unknown) => facts.complete = false,
        }
    }
    facts
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn json_rows(directory: &Path) -> Result<Vec<Value>, Unknown> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(Unknown),
    };
    let mut rows = Vec::new();
    for entry in entries {
        let path = entry.map_err(|_| Unknown)?.path();
        if path.extension().is_some_and(|e| e == "json") {
            rows.push(serde_json::from_slice(&std::fs::read(&path).map_err(|_| Unknown)?).map_err(|_| Unknown)?);
        }
    }
    Ok(rows)
}

fn text(value: &Value) -> Result<Option<&str>, Unknown> {
    match value {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s.as_str()).filter(|s| !s.is_empty())),
        other if !truthy(other) => Ok(None),
        _ => Err(Unknown),
    }
}

/// Inclui sidecars sem filho iniciado: o nascimento adiado ainda usa a conta.
fn registered(
    service: &AccountService,
    key: &AccountKey,
    terminals: &mut dyn FnMut() -> Result<Instances, ()>,
    facts: &mut UsageFacts,
) -> Result<(), Unknown> {
    let home = &service.env.home;
    let mut cached: Option<Instances> = None;
    let mut instances = |cached: &mut Option<Instances>| -> Result<Instances, Unknown> {
        if cached.is_none() {
            *cached = Some(terminals().map_err(|_| Unknown)?);
        }
        Ok(cached.clone().unwrap_or_default())
    };
    for pending in json_rows(&home.join(".hangar/account-locks/births"))? {
        let provider: Provider = serde_json::from_value(pending["provider"].clone()).map_err(|_| Unknown)?;
        let canonical = pending["canonical_home"].as_str().ok_or(Unknown)?;
        let owner = AccountKey::new(provider, Path::new(canonical)).map_err(|_| Unknown)?;
        if !truthy(&pending["name"]) || pending.get("instance").is_none() {
            return Err(Unknown);
        }
        if owner != *key {
            continue;
        }
        let live = instances(&mut cached)?;
        match &pending["instance"] {
            // Sem identidade, ausência do nome pode ser apenas um rename.
            Value::Null => facts.complete = false,
            instance => facts.sessions.extend(live.iter()
                .filter(|(_, identity)| instance.as_str() == Some(identity.as_str()))
                .map(|(name, _)| name.clone())),
        }
    }
    let directory = home.join(".hangar").join(if key.provider == Provider::Codex { "codex-sessions" } else { "claude-headless" });
    for row in json_rows(&directory)? {
        let Some(name) = row.get("name").filter(|n| truthy(n)) else { return Err(Unknown) };
        let name = name.as_str().ok_or(Unknown)?.to_owned();
        let account_home = if key.provider == Provider::Codex {
            match text(&row["codex_home"])? {
                Some(path) => PathBuf::from(path),
                None => {
                    let id = text(&row["codex_account"])?.unwrap_or("default");
                    service.resolve(Provider::Codex, id).map_err(|_| Unknown)?.home
                }
            }
        } else {
            text(&row["config_dir"])?.map(PathBuf::from).unwrap_or_else(|| home.join(".claude"))
        };
        if AccountKey::new(key.provider, &account_home).map_err(|_| Unknown)? != *key {
            continue;
        }
        if key.provider == Provider::Codex && !truthy(&row["headless"]) && !truthy(&row["launching"])
            && !instances(&mut cached)?.contains_key(&name)
        {
            continue;
        }
        facts.sessions.push(name);
    }
    Ok(())
}

/// Processos, nascimentos e sidecars desta máquina. E/S bloqueante: chamar por `spawn_blocking`.
pub(crate) fn local_usage(
    service: &AccountService,
    key: &AccountKey,
    procs: &dyn ProcessView,
    terminals: &mut dyn FnMut() -> Result<Instances, ()>,
) -> UsageFacts {
    let mut facts = inspect_processes(procs, key, &service.env.home);
    if registered(service, key, terminals, &mut facts).is_err() {
        facts.complete = false;
    }
    facts.merge(UsageFacts { complete: true, ..Default::default() });
    facts
}

/// Valida cada linha: formato que o multiplexador não interpolou é dúvida, não ausência.
fn parse_instances(stdout: &str) -> Result<Instances, ()> {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let mut instances = Instances::new();
    for line in stdout.lines() {
        let (name, identity) = line.split_once('\t').ok_or(())?;
        if identity.contains('\t') {
            return Err(());
        }
        let parts: Vec<&str> = identity.split(':').collect();
        let [pid, id, created] = parts[..] else { return Err(()) };
        if name.is_empty() || !digits(pid) || !id.strip_prefix('$').is_some_and(digits) || !digits(created) {
            return Err(());
        }
        instances.insert(name.to_owned(), identity.to_owned());
    }
    Ok(instances)
}

/// `publish_terminal_birth`: o terminal que acabou de nascer na conta conta como uso dela até o
/// lançador gravar o sidecar. Publicar leva junto os nascimentos cuja instância não vive mais.
/// Chamar sob a guarda compartilhada da conta.
pub(crate) fn publish_birth(home: &Path, name: &str, key: &AccountKey, instance: &str, live: &Instances) -> io::Result<()> {
    let directory = home.join(".hangar/account-locks/births");
    std::fs::create_dir_all(&directory)?;
    for entry in std::fs::read_dir(&directory)?.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") { continue; }
        let gone = std::fs::read(&path).ok().and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
            .and_then(|row| row["instance"].as_str().map(|i| !live.values().any(|v| v == i)));
        if gone == Some(true) { let _ = std::fs::remove_file(&path); }
    }
    let row = serde_json::json!({"name": name, "provider": key.provider, "canonical_home": key.canonical_home, "instance": instance});
    let token = crate::mods::state::random_hex(16);
    crate::runtime::queue::atomic_write(&directory.join(format!("{token}.json")), row.to_string().as_bytes())
}

/// Identidade das sessões vivas do multiplexador (tmux ou psmux).
pub(crate) async fn terminal_instances() -> Result<Instances, ()> {
    let mut command = crate::terminal_input::child_command("tmux");
    command.args(["list-sessions", "-F", "#{session_name}\t#{pid}:#{session_id}:#{session_created}"])
        .stdin(std::process::Stdio::null()).kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(5), command.output()).await.map_err(|_| ())?.map_err(|_| ())?;
    if out.status.success() {
        return parse_instances(std::str::from_utf8(&out.stdout).map_err(|_| ())?);
    }
    let error = String::from_utf8_lossy(&out.stderr);
    let error = error.trim();
    let absent = error.starts_with("no server") || error.starts_with("no sessions")
        || (error.starts_with("error connecting to ") && error.ends_with("(No such file or directory)"));
    if out.status.code() == Some(1) && absent { Ok(Instances::new()) } else { Err(()) }
}

/// Processo nascendo ou saindo (no Windows argv e ambiente ficam ilegíveis por um instante, que
/// sob carga passa de meio segundo) deixa uma leitura incompleta à toa: relê até o prazo. Uma
/// leitura completa posterior prova o mesmo que qualquer outra.
async fn reread_incomplete<F, Fut>(mut read: F) -> UsageFacts
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = UsageFacts>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let facts = read().await;
        if facts.complete || !facts.sessions.is_empty() || tokio::time::Instant::now() >= deadline {
            return facts;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

impl AccountService {
    /// Fatos desta máquina somados aos do runtime Rust. Runtime ausente deixa os fatos
    /// incompletos: nunca prova que a conta está livre.
    pub(crate) async fn usage(
        &self,
        runtime: Option<&crate::runtime::gateway::RuntimeRegistry>,
        key: &AccountKey,
    ) -> UsageFacts {
        let mut facts = reread_incomplete(|| {
            let (service, owned, handle) = (self.clone(), key.clone(), tokio::runtime::Handle::current());
            async move {
                tokio::task::spawn_blocking(move || {
                    let procs = crate::list::procs::SystemProcs::whole_machine();
                    let fixture = service.env.terminal_fixture.clone();
                    local_usage(&service, &owned, &procs, &mut || match &fixture {
                        Some(fixture) => fixture().and_then(|stdout| parse_instances(&stdout)),
                        None => handle.block_on(terminal_instances()),
                    })
                })
                .await
                .unwrap_or_else(|_| {
                    tracing::warn!(code = "account_usage_task_failed", "leitura de uso da conta não terminou");
                    UsageFacts::default()
                })
            }
        })
        .await;
        facts.merge(match runtime {
            Some(runtime) => runtime.account_usage(key).await,
            None => UsageFacts::default(),
        });
        facts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::environment::AccountEnvironment;
    use crate::list::procs::ChildrenMap;
    use std::{collections::HashMap, ffi::OsString, sync::{Arc, Mutex}};

    #[derive(Clone, Default)]
    struct Proc {
        name: String,
        argv: Vec<String>,
        /// `None` = ambiente ilegível.
        env: Option<HashMap<String, String>>,
        starts: Vec<f64>,
        gone: bool,
        /// Título escrito por cima do ambiente (Chromium/Electron).
        erased: bool,
    }

    #[derive(Default)]
    struct Fake {
        procs: BTreeMap<i64, Proc>,
        reads: Mutex<HashMap<i64, usize>>,
    }

    impl Fake {
        fn with(rows: Vec<(i64, Proc)>) -> Self { Self { procs: rows.into_iter().collect(), ..Default::default() } }
        fn get(&self, pid: i64) -> io::Result<&Proc> {
            self.procs.get(&pid).filter(|p| !p.gone).ok_or_else(|| io::ErrorKind::NotFound.into())
        }
    }

    impl ProcessView for Fake {
        fn children(&self, _: Duration) -> io::Result<Arc<ChildrenMap>> {
            Ok(Arc::new(ChildrenMap::from([(1, self.procs.keys().copied().collect())])))
        }
        fn argv_checked(&self, pid: i64) -> io::Result<Vec<String>> { Ok(self.get(pid)?.argv.clone()) }
        fn cwd(&self, _: i64) -> Option<PathBuf> { Some(PathBuf::from("/")) }
        fn env_var(&self, pid: i64, name: &str) -> io::Result<Option<OsString>> {
            let env = self.get(pid)?.env.as_ref().ok_or_else(|| io::Error::from(io::ErrorKind::PermissionDenied))?;
            Ok(env.get(name).map(OsString::from))
        }
        fn start_time(&self, pid: i64) -> Option<f64> {
            let starts = &self.get(pid).ok()?.starts;
            let mut reads = self.reads.lock().unwrap();
            let read = reads.entry(pid).or_default();
            *read += 1;
            starts.get(*read - 1).or(starts.last()).copied()
        }
        fn name(&self, pid: i64) -> io::Result<String> { Ok(self.get(pid)?.name.clone()) }
        fn environ_erased(&self, pid: i64) -> bool { self.get(pid).is_ok_and(|p| p.erased) }
    }

    fn proc(name: &str, argv: &[&str], env: &[(&str, &Path)]) -> Proc {
        Proc {
            name: name.into(),
            argv: argv.iter().map(|a| (*a).to_owned()).collect(),
            env: Some(env.iter().map(|(k, v)| ((*k).to_owned(), v.to_string_lossy().into_owned())).collect()),
            starts: vec![1.0],
            gone: false,
            erased: false,
        }
    }

    fn service(home: &Path) -> AccountService {
        AccountService::new(AccountEnvironment::from_map(
            [("HOME".into(), home.to_string_lossy().into()), ("USERPROFILE".into(), home.to_string_lossy().into())].into()))
    }

    fn none() -> Result<Instances, ()> { Ok(Instances::new()) }

    #[test]
    fn inherited_variable_holds_account_but_only_the_cli_uses_it() {
        let tmp = tempfile::tempdir().unwrap();
        let account = tmp.path().join(".claude-work");
        std::fs::create_dir(&account).unwrap();
        let key = AccountKey::new(Provider::Claude, &account).unwrap();
        let env = [("CLAUDE_CONFIG_DIR", account.as_path())];
        let plugin = tmp.path().join(".claude/plugins/x/server.js");
        let fake = Fake::with(vec![
            (81, proc("node", &["node", plugin.to_str().unwrap()], &env)),
            (82, proc("node", &["node", "mcp.js", "--pi"], &env)),
            (83, proc("tmux: server", &["tmux"], &env)),
            (84, proc("node", &["node", "--max-old-space-size=64", "/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js"], &env)),
            (85, Proc { env: None, ..proc("python3", &["python3"], &[]) }),
        ]);
        let facts = inspect_processes(&fake, &key, tmp.path());
        assert!(facts.complete);
        assert_eq!(facts.pids, [84]);
        assert_eq!(facts.holders, [81, 82, 83, 84]);
    }

    #[test]
    fn unreadable_environment_reused_pid_and_vanished_process() {
        let tmp = tempfile::tempdir().unwrap();
        let key = AccountKey::new(Provider::Codex, tmp.path()).unwrap();
        let codex = proc("codex.exe", &["codex", "app-server"], &[("CODEX_HOME", tmp.path())]);
        let unreadable = Fake::with(vec![(71, Proc { env: None, ..codex.clone() })]);
        assert_eq!(inspect_processes(&unreadable, &key, tmp.path()).ensure_unused(), Err("account_usage_unknown"));
        let reused = Fake::with(vec![(71, Proc { starts: vec![1.0, 2.0], ..codex.clone() })]);
        let facts = inspect_processes(&reused, &key, tmp.path());
        assert!(!facts.complete && facts.pids.is_empty());
        let vanished = Fake::with(vec![(71, Proc { gone: true, ..codex.clone() })]);
        assert!(inspect_processes(&vanished, &key, tmp.path()).complete);
        let zombie = Fake::with(vec![(71, Proc { argv: Vec::new(), env: None, ..codex.clone() })]);
        assert!(inspect_processes(&zombie, &key, tmp.path()).complete, "zumbi já saiu");
        let facts = inspect_processes(&Fake::with(vec![(71, codex)]), &key, tmp.path());
        assert!(facts.complete && facts.pids == [71]);
        assert_eq!(facts.ensure_unused(), Err("account_in_use"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_process_unreadable_for_a_moment_is_read_again_until_the_deadline() {
        // No Windows o `sysinfo` não lê argv nem ambiente de quem está nascendo ou saindo; sob carga
        // isso passa de meio segundo.
        let reads = std::cell::Cell::new(0);
        let incomplete = UsageFacts::default();
        let started = tokio::time::Instant::now();
        let facts = reread_incomplete(|| {
            reads.set(reads.get() + 1);
            let next = if reads.get() < 8 { incomplete.clone() } else { UsageFacts { complete: true, ..Default::default() } };
            async move { next }
        })
        .await;
        assert!(facts.complete, "leu {} vezes em {:?}", reads.get(), started.elapsed());
        // O que nunca se completa termina no prazo, sem reler para sempre.
        let started = tokio::time::Instant::now();
        assert!(!reread_incomplete(|| async { UsageFacts::default() }).await.complete);
        assert!(started.elapsed() <= Duration::from_secs(3));
    }

    #[test]
    fn electron_app_named_claude_with_its_environment_overwritten_is_not_a_client() {
        // O Claude Desktop (Electron) se chama `claude`, junta o argv numa linha e escreve o título por
        // cima do ambiente: sem variável lida, ele passaria por cliente da conta Claude padrão.
        let tmp = tempfile::tempdir().unwrap();
        let default = AccountKey::new(Provider::Claude, &tmp.path().join(".claude")).unwrap();
        let desktop = |argv: &str| Proc { erased: true, ..proc("claude", &[argv], &[]) };
        let fake = Fake::with(vec![
            (90, desktop("/usr/lib/claude-desktop-bin/claude --ozone-platform=wayland")),
            (91, desktop("/usr/lib/claude-desktop-bin/claude --type=zygote --no-sandbox")),
            (92, proc("claude", &["claude", "--session-id", "x"], &[])),
        ]);
        let facts = inspect_processes(&fake, &default, tmp.path());
        assert!(facts.complete);
        assert_eq!(facts.pids, [92], "só o CLI usa a conta padrão");
    }

    #[test]
    fn codex_home_flag_overrides_environment() {
        let tmp = tempfile::tempdir().unwrap();
        let (account, other) = (tmp.path().join("account"), tmp.path().join("other"));
        let key = AccountKey::new(Provider::Codex, &account).unwrap();
        let flag = format!("--codex-home={}", account.display());
        let fake = Fake::with(vec![
            (10, proc("codex", &["codex", "--codex-home", account.to_str().unwrap()], &[("CODEX_HOME", &other)])),
            (11, proc("node", &["node", "/x/codex.js", &flag], &[])),
            (12, proc("codex", &["codex", &format!("--codex-home={}", other.display())], &[("CODEX_HOME", &account)])),
        ]);
        let facts = inspect_processes(&fake, &key, tmp.path());
        assert!(facts.complete);
        assert_eq!(facts.pids, [10, 11]);
        let empty = Fake::with(vec![(13, proc("codex", &["codex", "--codex-home="], &[]))]);
        assert!(!inspect_processes(&empty, &key, tmp.path()).complete);
        // Sem variável, o padrão sai da casa do próprio processo.
        let default = AccountKey::new(Provider::Codex, &tmp.path().join(".codex")).unwrap();
        let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        let homes = Fake::with(vec![
            (14, proc("codex", &["codex"], &[(var, tmp.path())])),
            (15, proc("codex", &["codex"], &[(var, &other)])),
        ]);
        assert_eq!(inspect_processes(&homes, &default, Path::new("/nowhere")).pids, [14]);
    }

    #[test]
    fn births_follow_instance_and_unknown_instance_is_incomplete() {
        let tmp = tempfile::tempdir().unwrap();
        let service = service(tmp.path());
        let account = tmp.path().join("account");
        let key = AccountKey::new(Provider::Codex, &account).unwrap();
        let births = tmp.path().join(".hangar/account-locks/births");
        std::fs::create_dir_all(&births).unwrap();
        let record = |instance: Value| std::fs::write(births.join("t.json"), serde_json::json!({
            "name": "birth", "provider": "codex", "canonical_home": account, "instance": instance}).to_string()).unwrap();
        let empty = Fake::default();
        record("1:$2:3".into());
        let mut renamed = || -> Result<Instances, ()> {
            Ok(Instances::from([("renamed".into(), "1:$2:3".into()), ("birth".into(), "1:$9:3".into())]))
        };
        let facts = local_usage(&service, &key, &empty, &mut renamed);
        assert!(facts.complete);
        assert_eq!(facts.sessions, ["renamed"]);
        assert!(!local_usage(&service, &key, &empty, &mut || Err(())).complete, "multiplexador mudo é dúvida");
        local_usage(&service, &key, &empty, &mut none).ensure_unused().unwrap();
        record(Value::Null);
        assert!(!local_usage(&service, &key, &empty, &mut none).complete);
        std::fs::write(births.join("t.json"), "{").unwrap();
        assert!(!local_usage(&service, &key, &empty, &mut none).complete);
    }

    /// O terminal Codex criado aqui fica em uso pela conta até o sidecar existir; o nascimento de uma
    /// instância que já morreu sai na publicação seguinte.
    #[test]
    fn a_published_birth_keeps_the_account_in_use_and_prunes_dead_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let service = service(tmp.path());
        let account = tmp.path().join("account");
        let key = AccountKey::new(Provider::Codex, &account).unwrap();
        let births = tmp.path().join(".hangar/account-locks/births");
        std::fs::create_dir_all(&births).unwrap();
        std::fs::write(births.join("old.json"), serde_json::json!({"name": "morta", "provider": "codex", "canonical_home": account,
            "instance": "1:$7:1"}).to_string()).unwrap();
        let live = Instances::from([("cx".into(), "1:$2:3".into())]);
        publish_birth(tmp.path(), "cx", &key, "1:$2:3", &live).unwrap();
        assert!(!births.join("old.json").exists(), "a instância morta não fica");
        let facts = local_usage(&service, &key, &Fake::default(), &mut || Ok(live.clone()));
        assert_eq!((facts.complete, facts.sessions.as_slice()), (true, ["cx".to_owned()].as_slice()));
    }

    #[test]
    fn sidecars_count_and_broken_one_is_incomplete() {
        let tmp = tempfile::tempdir().unwrap();
        let service = service(tmp.path());
        let empty = Fake::default();
        let account = tmp.path().join(".claude-work");
        std::fs::create_dir(&account).unwrap();
        let key = AccountKey::new(Provider::Claude, &account).unwrap();
        let headless = tmp.path().join(".hangar/claude-headless");
        std::fs::create_dir_all(&headless).unwrap();
        std::fs::write(headless.join("birth.json"), serde_json::json!({"name":"birth","session_id":"pending","config_dir":account}).to_string()).unwrap();
        std::fs::write(headless.join("main.json"), serde_json::json!({"name":"main"}).to_string()).unwrap();
        let facts = local_usage(&service, &key, &empty, &mut none);
        assert!(facts.complete);
        assert_eq!(facts.sessions, ["birth"]);
        std::fs::write(headless.join("birth.json"), "{").unwrap();
        assert!(!local_usage(&service, &key, &empty, &mut none).complete);

        // Sidecar de terminal Codex só conta com a sessão viva ou ainda nascendo.
        let codex = tmp.path().join(".hangar/codex-sessions");
        std::fs::create_dir_all(&codex).unwrap();
        let key = AccountKey::new(Provider::Codex, &tmp.path().join("account")).unwrap();
        let row = serde_json::json!({"name":"old","codex_home":tmp.path().join("account")});
        std::fs::write(codex.join("old.json"), row.to_string()).unwrap();
        let mut live = || -> Result<Instances, ()> { Ok(Instances::from([("old".into(), "1:$2:3".into())])) };
        assert_eq!(local_usage(&service, &key, &empty, &mut live).sessions, ["old"]);
        local_usage(&service, &key, &empty, &mut none).ensure_unused().unwrap();
        assert!(!local_usage(&service, &key, &empty, &mut || Err(())).complete);
        let mut launching = row.clone();
        launching["launching"] = true.into();
        std::fs::write(codex.join("old.json"), launching.to_string()).unwrap();
        let facts = local_usage(&service, &key, &empty, &mut || Err(()));
        assert!(facts.complete && facts.sessions == ["old"]);
    }

    #[test]
    fn mux_format_is_validated() {
        assert_eq!(parse_instances("birth\t123:$4:567\r\n").unwrap(), Instances::from([("birth".into(), "123:$4:567".into())]));
        assert!(parse_instances("birth\t#{pid}:$4:567\n").is_err());
        assert!(parse_instances("birth\n").is_err());
    }
}
