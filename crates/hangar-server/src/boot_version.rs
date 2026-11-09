//! Versão do código carregado, congelada antes de servir pedidos.
use std::{collections::{BTreeMap,HashMap},path::{Path,PathBuf},sync::{Arc,LazyLock,Mutex,OnceLock},time::Duration};

#[derive(Clone,Debug,PartialEq,Eq,serde::Serialize,serde::Deserialize)]
pub struct Snapshot{pub describe:String,pub readable:Option<String>}
static SNAPSHOTS:LazyLock<Mutex<HashMap<PathBuf,Arc<OnceLock<Snapshot>>>>>=LazyLock::new(Default::default);
fn slot(root:&Path)->Arc<OnceLock<Snapshot>>{
    SNAPSHOTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).entry(root.to_owned()).or_default().clone()
}
pub fn get(root:&Path)->Option<Snapshot>{
    let slot=SNAPSHOTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(root).cloned()?;
    slot.get().cloned()
}
/// A primeira publicação vence; a raiz permite injetar uma versão isolada em provas.
pub fn publish(root:&Path,snapshot:Snapshot)->Snapshot{slot(root).get_or_init(||snapshot).clone()}
/// O git pode esperar pelo disco; a saúde do Supervisor não deve esperar por ele.
pub async fn ensure(root:PathBuf,environment:BTreeMap<String,String>)->Result<Snapshot,tokio::task::JoinError>{
    if let Some(snapshot)=get(&root){return Ok(snapshot);}
    tokio::task::spawn_blocking(move||capture(&root,&environment)).await
}
pub fn capture(root:&Path,environment:&BTreeMap<String,String>)->Snapshot{
    capture_with(root,|args,timeout|{
        let mut command=vec!["git".to_owned()];command.extend(args.iter().map(|v|(*v).to_owned()));
        crate::update::process::run_capture_with_env(&command,root,timeout,environment).map(|output|output.stdout).map_err(|_|())
    })
}
/// A fonte injetada conserva os argumentos e prazos do subprocesso, sem exigir git nas provas.
#[doc(hidden)]
pub fn capture_with(root:&Path,mut git:impl FnMut(&[&str],Duration)->Result<String,()>)->Snapshot{
    slot(root).get_or_init(||{
        let manifest=crate::update::package_manifest(root);
        if let Some(manifest)=manifest{return Snapshot{describe:format!("{}-g{}",manifest["version"].as_str().unwrap(),manifest["commit"].as_str().unwrap().chars().take(12).collect::<String>()),readable:Some(manifest["version"].as_str().unwrap().into())};}
        let describe=git(&["describe","--tags","--match","dist-latest","--long","--always","--dirty"],Duration::from_secs(5)).ok().map(|s|crate::transcript::py::strip(&s).to_owned()).filter(|s|!s.is_empty()).unwrap_or_else(||"indisponivel".into());
        let readable=(||{
            let count=git(&["rev-list","--count","HEAD"],Duration::from_secs(5)).ok().map(|s|crate::transcript::py::strip(&s).to_owned())?;
            if count.is_empty()||!count.chars().all(crate::accounts::claude_config::py_isdigit){return None;}
            let mut version=git(&["show","HEAD:VERSION"],Duration::from_secs(5)).ok().map(|s|crate::transcript::py::strip(&s).to_owned())?;
            if version.is_empty(){version=std::fs::read_to_string(root.join("VERSION")).ok().map(|s|crate::transcript::py::strip(&s).to_owned())?;}
            (!version.is_empty()).then(||format!("{version}.{count}"))
        })();
        Snapshot{describe,readable}
    }).clone()
}

/// Só identidade e versão: a receita com argv/ambiente nunca sai da memória.
#[derive(Clone,serde::Serialize,serde::Deserialize)]
struct PythonLife {
    pid: u32,
    birth: f64,
    root: PathBuf,
    executable: PathBuf,
    version: Snapshot,
    #[serde(default)]
    started: Option<String>,
}

pub(crate) struct PythonLaunch { base: PathBuf, root: PathBuf, executable: PathBuf, version: Snapshot }
impl PythonLaunch {
    pub(crate) fn capture(command: &std::process::Command) -> std::io::Result<Self> {
        let env = crate::accounts::environment::AccountEnvironment::capture();
        // O Command já fixou a versão real. Reler `current` aqui escolheria outro código.
        let root = command.get_current_dir().and_then(Path::parent).ok_or(std::io::ErrorKind::InvalidInput)?.to_owned();
        let executable = std::fs::canonicalize(command.get_program())?;
        let mut environment = env.base;
        for (key, value) in command.get_envs() {
            if let Some(value) = value { environment.insert(key.to_string_lossy().into_owned(), value.to_string_lossy().into_owned()); }
            else { environment.remove(key.to_string_lossy().as_ref()); }
        }
        let env = crate::accounts::environment::AccountEnvironment::from_map(environment);
        let base = crate::runtime::local_policy::log_base_in(&env).ok_or(std::io::ErrorKind::NotFound)?;
        let version = capture(&root, &env.base);
        Ok(Self { base, root, executable, version })
    }

    pub(crate) fn record(&self, pid: u32, birth: f64, executable: PathBuf) -> std::io::Result<()> {
        PythonLife { pid, birth, root: self.root.clone(), executable, version: self.version.clone(), started: None }.save(&self.base)
    }

    pub(crate) fn record_exec(&self) -> std::io::Result<()> {
        let pid = std::process::id();
        let birth = crate::terminal_facts::agent_birth(pid).ok_or(std::io::ErrorKind::NotFound)?;
        self.record(pid, birth, self.executable.clone())
    }
}

fn python_dir(base: &Path) -> PathBuf { base.join("privado/python-versions") }
fn python_path(base: &Path, pid: u32, birth: f64) -> PathBuf { python_dir(base).join(format!("{pid}-{:016x}.json", birth.to_bits())) }
impl PythonLife {
    fn save(&self, base: &Path) -> std::io::Result<()> {
        let dir = python_dir(base);
        std::fs::create_dir_all(&dir)?;
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?; }
        crate::runtime::queue::atomic_write(&python_path(base, self.pid, self.birth), &serde_json::to_vec(self)?)
    }

    fn confirmed(&self, procs: &dyn crate::list::procs::ProcessView) -> bool {
        let Ok(recipe) = procs.recipe(i64::from(self.pid)) else { return false };
        crate::terminal_facts::same_birth(recipe.birth, self.birth)
            && recipe.executable == self.executable && recipe.cwd == self.root.join("backend")
            && backend_module(&recipe)
            && (!cfg!(windows) || python_runtime(self.pid))
            && crate::terminal_facts::agent_birth(self.pid).is_some_and(|birth| crate::terminal_facts::same_birth(birth, self.birth))
    }
}

fn backend_module(recipe: &crate::list::procs::ProcessRecipe) -> bool {
    recipe.argv.get(1).is_some_and(|arg| arg == "-m") && recipe.argv.get(2).is_some_and(|arg| arg == "app.main")
}

fn current_python(base: &Path, pid: u32) -> Option<PythonLife> {
    let birth = crate::terminal_facts::agent_birth(pid)?;
    let life: PythonLife = serde_json::from_slice(&crate::runtime::queue::read_replaced(&python_path(base, pid, birth)).ok()?).ok()?;
    (life.pid == pid && crate::terminal_facts::same_birth(life.birth, birth)
        && life.confirmed(&crate::list::procs::SystemProcs::default())).then_some(life)
}

/// Não guarda ausência: o Python do Windows pode publicar depois do primeiro pedido.
pub(crate) fn python_version(env: &crate::accounts::environment::AccountEnvironment, pid: u32) -> Option<Snapshot> {
    current_python(&crate::runtime::local_policy::log_base_in(env)?, pid).map(|life| life.version)
}

/// As associações já confirmadas sobrevivem ao processo. Sem associação, histórico não herda a vida atual.
pub(crate) struct PythonDiary { base: Option<PathBuf>, lives: HashMap<(u32, String), Snapshot> }
impl PythonDiary {
    pub(crate) fn load(base: Option<PathBuf>) -> Self {
        let mut lives = HashMap::new();
        if let Some(base) = &base {
            for entry in std::fs::read_dir(python_dir(base)).into_iter().flatten().flatten() {
                if entry.path().extension().is_none_or(|ext| ext != "json") { continue; }
                let life = crate::runtime::queue::read_replaced(&entry.path()).ok()
                    .and_then(|raw| serde_json::from_slice::<PythonLife>(&raw).ok());
                if let Some(PythonLife { pid, started: Some(started), version, .. }) = life { lives.insert((pid, started), version); }
            }
        }
        Self { base, lives }
    }

    pub(crate) fn for_paths(paths: &[(String, PathBuf)]) -> Self {
        let base = paths.iter().find_map(|(_, path)| path.ancestors()
            .find(|dir| dir.file_name().is_some_and(|name| name == "diario")).and_then(Path::parent).map(Path::to_owned));
        Self::load(base)
    }

    pub(crate) fn bind(&mut self, value: &serde_json::Value) -> std::io::Result<()> {
        if value["evento"] != "diag.formato" && value["origem"] != "servidor" { return Ok(()); }
        let Some(base) = &self.base else { return Ok(()) };
        let Some(pid) = value["pid_backend"].as_u64().and_then(|pid| u32::try_from(pid).ok()) else { return Ok(()) };
        let Some(started) = value["inicio_backend"].as_str() else { return Ok(()) };
        if self.lives.contains_key(&(pid, started.to_owned())) { return Ok(()); }
        let Some(at) = chrono::DateTime::parse_from_rfc3339(started).ok().map(|at| at.timestamp() as f64 + f64::from(at.timestamp_subsec_nanos()) / 1e9) else { return Ok(()) };
        let Some(mut life) = current_python(base, pid) else { return Ok(()) };
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(std::io::Error::other)?.as_secs_f64();
        // INICIO_EM_EXECUCAO é o instante de importação, não o nascimento do sistema operacional.
        if at < life.birth || at > now || life.started.as_deref().is_some_and(|old| old != started) { return Ok(()); }
        life.started = Some(started.to_owned());
        life.save(base)?;
        self.lives.insert((pid, started.to_owned()), life.version);
        Ok(())
    }

    pub(crate) fn enrich(&self, value: &mut serde_json::Value) {
        let header = value["evento"] == "diag.formato";
        if !header && (value["origem"] != "servidor" || value["pid_backend"].as_u64().is_none()) { return; }
        let identity = value["pid_backend"].as_u64().and_then(|pid| u32::try_from(pid).ok()).zip(value["inicio_backend"].as_str());
        let live = if header {
            identity.and_then(|(pid, started)| current_python(self.base.as_deref()?, pid)
                .filter(|life| life.started.as_deref() == Some(started)).map(|life| life.version))
        } else { None };
        let version = if header { live.as_ref() } else {
            identity.and_then(|(pid, started)| self.lives.get(&(pid, started.to_owned())))
        };
        if let Some(version) = version { value["backend"] = serde_json::json!(version.describe); }
        else if header || value.get("backend").is_none() { value["backend"] = serde_json::json!("indisponivel"); }
    }

    pub(crate) fn enrich_summary(&mut self, text: &str) -> Result<String, serde_json::Error> {
        let fields: HashMap<String, &serde_json::value::RawValue> = serde_json::from_str(text)?;
        let Some(rows) = fields.get("ultimas") else { return Ok(text.to_owned()) };
        let rows: Vec<&serde_json::value::RawValue> = serde_json::from_str(rows.get())?;
        let mut edits = Vec::new();
        for row in rows {
            let value = serde_json::from_str(row.get())?;
            if let Err(error) = self.bind(&value) { tracing::warn!(kind = ?error.kind(), "diário: identidade Python não persistida"); }
            let start = row.get().as_ptr() as usize - text.as_ptr() as usize;
            edits.push((start..start + row.get().len(), self.enrich_line(row.get())));
        }
        let mut output = text.to_owned();
        for (range, replacement) in edits.into_iter().rev() { output.replace_range(range, &replacement); }
        Ok(output)
    }

    pub(crate) fn enrich_line(&self, line: &str) -> String {
        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(line) else { return line.to_owned() };
        let before = value["backend"].clone();
        self.enrich(&mut value);
        if value["backend"] == before { return line.to_owned(); }
        put_backend(line, &value["backend"])
    }
}

/// Troca só o campo de versão; RawValue conserva os bytes dos outros campos.
fn put_backend(line: &str, version: &serde_json::Value) -> String {
    let Ok(fields) = serde_json::from_str::<HashMap<String, &serde_json::value::RawValue>>(line) else { return line.to_owned() };
    if let Some(old) = fields.get("backend") {
        let start = old.get().as_ptr() as usize - line.as_ptr() as usize;
        return format!("{}{}{}", &line[..start], version, &line[start + old.get().len()..]);
    }
    let Some(end) = line.rfind('}') else { return line.to_owned() };
    format!("{},\"backend\":{}{}", &line[..end], version, &line[end..])
}

#[cfg(not(windows))]
fn python_runtime(_: u32) -> bool { true }

/// O redirector do venv tem nome e argv de Python, mas não carrega o interpretador.
#[cfg(windows)]
fn python_runtime(pid: u32) -> bool {
    type Handle = *mut std::ffi::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
        fn CloseHandle(handle: Handle) -> i32;
        fn K32EnumProcessModules(process: Handle, modules: *mut Handle, size: u32, needed: *mut u32) -> i32;
        fn K32GetModuleBaseNameW(process: Handle, module: Handle, name: *mut u16, size: u32) -> u32;
    }
    let process = unsafe { OpenProcess(0x0410, 0, pid) };
    if process.is_null() { return false; }
    let mut modules = [std::ptr::null_mut(); 1024];
    let mut needed = 0;
    let ok = unsafe { K32EnumProcessModules(process, modules.as_mut_ptr(), std::mem::size_of_val(&modules) as u32, &mut needed) } != 0;
    let found = ok && needed as usize <= std::mem::size_of_val(&modules) && modules.iter().take(needed as usize / std::mem::size_of::<Handle>()).any(|module| {
        let mut name = [0u16; 260];
        let len = unsafe { K32GetModuleBaseNameW(process, *module, name.as_mut_ptr(), name.len() as u32) } as usize;
        let name = String::from_utf16_lossy(&name[..len]).to_ascii_lowercase();
        name.strip_prefix("python").and_then(|n| n.strip_suffix(".dll")).is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
    });
    unsafe { CloseHandle(process); }
    found
}

#[cfg(any(windows, test))]
fn effective_python(procs: &dyn crate::list::procs::ProcessView, root: &Path, pid: u32, birth: f64,
    is_runtime: impl Fn(u32) -> bool, current_birth: impl Fn(u32) -> Option<f64>) -> std::io::Result<Option<crate::list::procs::ProcessRecipe>> {
    if !current_birth(pid).is_some_and(|now| crate::terminal_facts::same_birth(now, birth)) { return Ok(None); }
    let children = procs.children(Duration::ZERO)?;
    let mut pending = vec![i64::from(pid)];
    let mut found = Vec::new();
    let mut seen = std::collections::HashSet::new();
    while let Some(candidate) = pending.pop() {
        if !seen.insert(candidate) { continue; }
        if let Some(children) = children.get(&candidate) { pending.extend(children); }
        let Ok(recipe) = procs.recipe(candidate) else { continue };
        if recipe.birth >= birth && recipe.cwd == root.join("backend") && backend_module(&recipe)
            && u32::try_from(candidate).ok().is_some_and(&is_runtime) { found.push(recipe); }
    }
    let [recipe] = found.as_slice() else { return Ok(None) };
    let effective = u32::try_from(recipe.pid).map_err(|_| std::io::ErrorKind::InvalidData)?;
    if !current_birth(pid).is_some_and(|now| crate::terminal_facts::same_birth(now, birth))
        || !current_birth(effective).is_some_and(|now| crate::terminal_facts::same_birth(now, recipe.birth)) { return Ok(None); }
    Ok(Some(recipe.clone()))
}

#[cfg(windows)]
impl PythonLaunch {
    pub(crate) fn record_windows(&self, pid: u32, birth: f64) -> std::io::Result<bool> {
        let procs = crate::list::procs::SystemProcs::default();
        let Some(recipe) = effective_python(&procs, &self.root, pid, birth, python_runtime, crate::terminal_facts::agent_birth)? else { return Ok(false) };
        let effective = u32::try_from(recipe.pid).map_err(|_| std::io::ErrorKind::InvalidData)?;
        self.record(effective, recipe.birth, recipe.executable)?;
        Ok(true)
    }
}

#[cfg(test)]
mod python_tests {
    use super::*;
    use crate::list::procs::{ChildrenMap, ProcessRecipe, ProcessView};
    use serde_json::json;

    #[cfg(unix)]
    #[test]
    fn command_root_is_captured_even_after_current_points_elsewhere() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let first = home.join(".hangar/versions/first");
        let second = home.join(".hangar/versions/second");
        for root in [&first, &second] {
            std::fs::create_dir_all(root.join("python/bin")).unwrap();
            std::fs::create_dir_all(root.join("backend")).unwrap();
            std::fs::write(root.join("python/bin/python3"), "").unwrap();
            std::fs::write(root.join("manifest.json"), json!({"version":"0.1.0.8207","commit":"abc1234def567890","channel":"latest"}).to_string()).unwrap();
        }
        std::os::unix::fs::symlink(&first, crate::install_root::current(home)).unwrap();
        let mut command = crate::install_root::backend_command(home, &[], false).unwrap();
        command.env("HOME", home).env("USERPROFILE", home);
        std::fs::remove_file(crate::install_root::current(home)).unwrap();
        std::os::unix::fs::symlink(&second, crate::install_root::current(home)).unwrap();
        let launch = PythonLaunch::capture(&command).unwrap();
        assert_eq!(launch.root, first);
        assert_eq!(launch.version, Snapshot { describe:"0.1.0.8207-gabc1234def56".into(), readable:Some("0.1.0.8207".into()) });
        assert_eq!(capture_with(&second, |_, _| panic!("manifesto prioritário não executa Git")).readable.as_deref(), Some("0.1.0.8207"));
    }

    #[test]
    fn history_joins_exact_startup_identity_and_never_relabels_a_reused_pid() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();
        let started = "2026-11-06T09:00:00.123-03:00";
        let newer = "2026-11-06T10:00:00.456-03:00";
        let root = base.join("root");
        for (birth, start, describe) in [(1.0, started, "first"), (2.0, newer, "second")] {
            PythonLife { pid:42, birth, root:root.clone(), executable:root.join("python"),
                version:Snapshot { describe:describe.into(), readable:None }, started:Some(start.into()) }.save(base).unwrap();
        }
        let mut versions = PythonDiary::load(Some(base.to_owned()));
        for (start, expected) in [(started, "first"), (newer, "second"), ("2026-11-06T11:00:00-03:00", "indisponivel")] {
            let mut row = json!({"evento":"backend.inicio","origem":"servidor","pid_backend":42,"inicio_backend":start});
            versions.enrich(&mut row);
            assert_eq!(row["backend"], expected);
        }
        let mut header = json!({"evento":"diag.formato","pid_backend":42,"inicio_backend":started,"backend":"must-not-reuse-history"});
        versions.enrich(&mut header);
        assert_eq!(header["backend"], "indisponivel", "histórico não confirma uma vida atualmente ausente");
        let raw = format!("{{\"unknown\":{{\"literal\":1.00,\"s\":\"backend\"}}, \"backend\":\"old\",\"evento\":\"backend.inicio\",\"origem\":\"servidor\",\"pid_backend\":42,\"inicio_backend\":{started:?}}}\n");
        assert_eq!(versions.enrich_line(&raw), raw.replace("\"backend\":\"old\"", "\"backend\":\"first\""));
        let summary = format!("{{\"bytes\":5.00, \"ultimas\":[{}],\"unknown\":1.00}}", raw.trim());
        assert_eq!(versions.enrich_summary(&summary).unwrap(), summary.replace("\"backend\":\"old\"", "\"backend\":\"first\""));
        let old = "{\"evento\":\"backend.inicio\",\"origem\":\"servidor\",\"pid_backend\":99,\"backend\":\"legacy-confirmed\"}";
        assert_eq!(versions.enrich_line(old), old, "versões históricas já registradas são conservadas");
    }

    struct Processes { recipes: HashMap<i64, ProcessRecipe>, children: ChildrenMap }
    impl ProcessView for Processes {
        fn children(&self, _: Duration) -> std::io::Result<Arc<ChildrenMap>> { Ok(Arc::new(self.children.clone())) }
        fn cwd(&self, pid: i64) -> Option<PathBuf> { self.recipes.get(&pid).map(|r| r.cwd.clone()) }
        fn env_var(&self, _: i64, _: &str) -> std::io::Result<Option<std::ffi::OsString>> { Ok(None) }
        fn start_time(&self, pid: i64) -> Option<f64> { self.recipes.get(&pid).map(|r| r.birth) }
        fn recipe(&self, pid: i64) -> std::io::Result<ProcessRecipe> { self.recipes.get(&pid).cloned().ok_or(std::io::ErrorKind::NotFound.into()) }
    }
    fn recipe(pid: i64, birth: f64, root: &Path) -> ProcessRecipe {
        ProcessRecipe { pid, birth, executable:root.join("python.exe"), cwd:root.join("backend"),
            argv:["python.exe", "-m", "app.main"].map(Into::into).to_vec(), environ:vec!["SECRET=never-persist".into()], cgroup:None }
    }

    #[test]
    fn windows_wrapper_requires_a_loaded_python_runtime_and_retries_absence() {
        let root = Path::new("fixture");
        let mut procs = Processes { recipes:HashMap::from([(10, recipe(10, 1.0, root))]), children:HashMap::new() };
        let birth = |pid| procs.start_time(i64::from(pid));
        assert!(effective_python(&procs, root, 10, 1.0, |_| false, birth).unwrap().is_none(), "nome e argv do redirector não provam Python");
        procs.children.insert(10, vec![20]);
        procs.recipes.insert(20, recipe(20, 1.5, root));
        let birth = |pid| procs.start_time(i64::from(pid));
        let actual = effective_python(&procs, root, 10, 1.0, |pid| pid == 20, birth).unwrap().unwrap();
        assert_eq!(actual.pid, 20);
        assert_eq!(actual.birth, 1.5);
        assert!(effective_python(&procs, root, 10, 0.5, |_| true, birth).unwrap().is_none(), "wrapper com PID reutilizado");
        assert!(effective_python(&procs, root, 10, 1.0, |_| true, birth).unwrap().is_none(), "dois interpretadores não escolhem um por conveniência");
        assert!(effective_python(&procs, root, 10, 1.0, |pid| pid == 20, |pid| if pid == 20 {Some(2.0)} else {Some(1.0)}).unwrap().is_none());
    }
}
