//! O lado Rust do harness de contrato da abertura (`session_open_contract.py diff --side rust`): a
//! abertura do Rust sobre o cenário montado em disco, com o multiplexador, os programas e as pontes
//! gravados na forma em que o lado Python os grava. Só para o harness e os testes.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use futures_util::future::BoxFuture;
use serde_json::{Map, Value, json};

use super::claude::PluginBridge;
use super::open::{Born, OpenHost};
use super::resolve::{CodexCatalogError, CodexLogin, CreatorFacts, OpenFacts, ProviderCatalog, ServerDefaults};
use super::terminal::{Mux, MuxOutput, TerminalEnv};
use super::{AgentProvider, OpenCaller, OpenError, ServiceTier, SessionOpenRequest};

const SCOPE: [&str; 6] = ["systemd-run", "--user", "--scope", "--collect", "-q", "--"];

/// `<ROOT>` do cenário vira a raiz; `<ROOT_SANITIZED>`, a pasta de projeto dela.
pub fn place(text: &str, root: &Path) -> String {
    let root = root.to_string_lossy();
    text.replace("<ROOT_SANITIZED>", &hangar_workspace::worktrees::sanitize_cwd(&root)).replace("<ROOT>", &root)
}

/// O `/proc` dos panes do cenário, fora do instantâneo de arquivos do harness.
fn proc_root(root: &Path) -> PathBuf { root.join(".harness").join("proc-rust") }

/// A `backend/` do cenário: a venv e o `skill_bridge.py` existem sempre, tenha ou não a árvore uma venv.
fn fake_backend(root: &Path) -> PathBuf { root.join(".harness").join("backend") }

/// O que o lado Python não monta e o Rust lê do disco: os panes do cenário (um `tmux` que só lista e
/// o `/proc` deles) e o `plugin-dir` que o servidor publica ao subir.
pub fn prepare(scenario: &Value, root: &Path, repo: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let bin = root.join("bin");
    let write = |path: &Path, bytes: &[u8]| -> std::io::Result<()> {
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(path, bytes)
    };
    let mut panes = String::new();
    for (index, session) in scenario["state"]["tmux_sessions"].as_array().into_iter().flatten().enumerate() {
        let pid = 9_000_001 + index;
        let name = session["name"].as_str().unwrap_or_default();
        let env = session["env"].as_object().cloned().unwrap_or_default();
        let provider = env.get("CP_PROVIDER").and_then(Value::as_str).unwrap_or("");
        panes += &format!("{name}\t1\t{pid}\t{}\t%{index}\t\t{provider}\t\t0\t0\n", place(session["cwd"].as_str().unwrap_or_default(), root));
        let argv: Vec<String> = session["command"].as_str().unwrap_or_default().trim_start_matches("exec ").split_whitespace().map(str::to_owned).collect();
        let proc = proc_root(root).join(pid.to_string());
        write(&proc.join("stat"), format!("{pid} ({}) S 1 {}", argv.first().map_or("sh", String::as_str), "0 ".repeat(20)).as_bytes())?;
        write(&proc.join("cmdline"), (argv.join("\0") + "\0").as_bytes())?;
        let environ: String = env.iter().map(|(k, v)| format!("{k}={}\0", place(v.as_str().unwrap_or_default(), root))).collect();
        write(&proc.join("environ"), environ.as_bytes())?;
        if session["environ_unreadable"] == true { std::fs::set_permissions(proc.join("environ"), std::fs::Permissions::from_mode(0o000))?; }
    }
    write(&fake_backend(root).join(".venv/bin/python"), b"")?;
    write(&fake_backend(root).join("app/skill_bridge.py"), b"")?;
    write(&bin.join("panes.txt"), panes.as_bytes())?;
    crate::write_test_executable(&bin.join("tmux"), b"#!/bin/sh\n[ \"$1\" = list-panes ] || exit 1\ncat \"$(dirname \"$0\")/panes.txt\"\n");
    let roots: String = ["hangar", "github-actions"].iter().map(|m| format!("{}\n", repo.join("plugins").join(m).display())).collect();
    write(&root.join("home/.hangar/plugin-dir"), roots.as_bytes())?;
    Ok(())
}

/// O que o harness grava, na forma do lado Python.
#[derive(Default)]
struct Record { subprocesses: Vec<Value>, probes: Vec<Value>, rust_bridge: Vec<Value>, runtime_requests: Vec<Value>, diary: Vec<Value>,
    /// Os nomes que a abertura limpou (grupo antigo, fila, `then`): só o restore mostra, porque o Python não grava.
    cleaned: Vec<String>,
    /// O passo da criação visto pelo aquecimento do Codex; só aparece com a criação acompanhada.
    progress_seen: Vec<Value> }

/// O tmux em memória do harness: o estado inicial do cenário, os `new-session` gravados e as consultas
/// respondidas pelo estado.
struct FakeTmux { sessions: Mutex<BTreeMap<String, Value>>, force: Value, record: std::sync::Arc<Mutex<Record>>,
    silenced: std::sync::atomic::AtomicBool }

impl Mux for FakeTmux {
    fn scope(&self) -> BoxFuture<'_, Vec<String>> { Box::pin(async { SCOPE.map(str::to_owned).into() }) }

    fn diary(&self, line: Value) { self.record.lock().unwrap().diary.push(line); }

    fn run(&self, argv: Vec<String>) -> BoxFuture<'_, MuxOutput> {
        let scoped = argv.first().is_some_and(|a| a == "systemd-run");
        let inner: Vec<String> = if scoped { argv.iter().skip_while(|a| *a != "--").skip(1).cloned().collect() } else { argv };
        let args = &inner[1..];
        let opt = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
        let target = opt("-t").map(|t| t.trim_start_matches('=').split(':').next().unwrap_or_default().to_owned());
        // Um tmux que deixa de responder no prazo (só os testes do Rust o pedem): `"all"` desde o começo,
        // `true` a partir do `new-session`.
        use std::sync::atomic::Ordering::SeqCst;
        if self.force["silent"] == true && args.first().is_some_and(|a| a == "new-session") { self.silenced.store(true, SeqCst); }
        if self.force["silent"] == "all" || self.silenced.load(SeqCst) {
            return Box::pin(async { MuxOutput { code: None, stdout: String::new(), stderr: "o multiplexador não respondeu no prazo".into() } });
        }
        let mut sessions = self.sessions.lock().unwrap();
        let (code, stdout, stderr) = match args.first().map(String::as_str) {
            Some("new-session") => {
                let name = opt("-s").unwrap_or_default();
                let (code, stderr) = if !self.force.is_null() {
                    (self.force["rc"].as_i64().unwrap_or(1) as i32, self.force["stderr"].as_str().unwrap_or("").to_owned())
                } else if sessions.contains_key(&name) {
                    (1, format!("duplicate session: {name}\n"))
                } else {
                    let env: Map<String, Value> = args.windows(2).filter(|w| w[0] == "-e").filter_map(|w| w[1].split_once('='))
                        .map(|(k, v)| (k.to_owned(), Value::from(v))).collect();
                    sessions.insert(name, json!({"cwd": opt("-c"), "env": env, "command": args.last()}));
                    (0, String::new())
                };
                let mut entry = json!({"prog": "tmux", "argv": args, "cwd": null, "rc": code});
                if scoped { entry["scope"] = "systemd-run --user --scope --collect -q".into(); }
                self.record.lock().unwrap().subprocesses.push(entry);
                (code, String::new(), stderr)
            }
            Some("has-session") => match target.filter(|t| sessions.contains_key(t)) {
                Some(_) => (0, String::new(), String::new()),
                None => (1, String::new(), "can't find session\n".into()),
            },
            Some("show-environment") => match target.and_then(|t| sessions.get(&t).cloned()) {
                Some(s) => (0, format!("{}={}\n", args.last().unwrap(), s["env"][args.last().unwrap().as_str()].as_str().unwrap_or("")), String::new()),
                None => (1, String::new(), "can't find session\n".into()),
            },
            _ => (0, String::new(), String::new()),
        };
        Box::pin(async move { MuxOutput { code: Some(code), stdout, stderr } })
    }
}

/// Os fatos de disco (`LocalFacts`), com as cotas e o catálogo do Codex que o harness deu à ponte do
/// Python, e gravados como ela os grava.
struct Facts { local: super::facts::LocalFacts, quotas: Vec<Value>, fakes: Value, record: std::sync::Arc<Mutex<Record>> }

impl Facts {
    /// `codex_models._call` sobre a resposta falsa da ponte `dictation`; sem resposta, a ponte fora do ar.
    fn dictation(&self, op: &str, args: Value) -> Result<Value, CodexCatalogError> {
        self.record.lock().unwrap().rust_bridge.push(json!({"bridge": "dictation", "op": op, "args": args}));
        let reply = &self.fakes["rust_bridge"][format!("dictation.{op}")];
        if reply.is_null() { return Err(CodexCatalogError::Unavailable("O servidor Rust de organização está indisponível.".into())); }
        let error = &reply["error"];
        if error.is_null() { return Ok(reply["result"].clone()); }
        let detail = error["detail"].as_str().unwrap_or_default().to_owned();
        Err(match error["code"].as_str() {
            Some("dictation_catalog_refused") => CodexCatalogError::Refused(detail),
            _ if error["status"] == 400 => CodexCatalogError::Invalid(detail),
            _ => CodexCatalogError::Unavailable(detail),
        })
    }
}

impl OpenFacts for Facts {
    fn providers(&self) -> Result<ProviderCatalog, OpenError> { self.local.providers() }
    fn creator(&self, name: &str) -> Result<Option<CreatorFacts>, OpenError> { self.local.creator(name) }
    fn defaults(&self) -> ServerDefaults { self.local.defaults() }
    fn saved_defaults(&self, provider: AgentProvider, engine: &str, engine_account: &str) -> Result<Option<crate::session_defaults::Defaults>, String> {
        self.local.saved_defaults(provider, engine, engine_account)
    }
    fn codex_logins(&self) -> Result<Vec<CodexLogin>, OpenError> { self.local.codex_logins() }
    fn claude_quotas(&self) -> Vec<Value> {
        self.record.lock().unwrap().rust_bridge.push(json!({"bridge": "accounts", "op": "quotas", "args": {"cached_only": true}}));
        self.quotas.iter().filter(|row| row["provedor"] == "claude").cloned().collect()
    }
    fn existing_dir(&self, cwd: &str) -> Option<String> { self.local.existing_dir(cwd) }
    fn config_dirs(&self) -> Result<Vec<String>, OpenError> { self.local.config_dirs() }
    fn engines(&self) -> Vec<String> { self.local.engines() }
    fn supports_fast(&self, engine: Option<&str>, model: Option<&str>) -> bool { self.local.supports_fast(engine, model) }
    fn fixed_engine_model(&self, engine: &str, account: &str, model: Option<&str>) -> Result<String, OpenError> {
        self.local.fixed_engine_model(engine, account, model)
    }
    // O harness recusa a rede: o catálogo do motor não responde.
    fn engine_window(&self, engine: &str, _model: &str, _account: Option<&str>) -> Result<Option<u64>, OpenError> {
        Err(OpenError::new(503, "session_open_fact_unavailable", format!("o catálogo do motor '{engine}' não respondeu: rede recusada")))
    }
    fn codex_home(&self, account_id: &str) -> Result<String, OpenError> { self.local.codex_home(account_id) }
    /// `codex_models.checar_escolha`: o catálogo da conta só quando há modelo, depois a conferência.
    fn codex_choice(&self, home: Option<&str>, model: Option<&str>, effort: Option<&str>, tier: Option<ServiceTier>)
        -> Result<(), CodexCatalogError> {
        let models = match model {
            Some(_) => self.dictation("catalog_models", json!({"provider": "codex", "home": home, "fresh": false}))?["models"].clone(),
            None => json!([]),
        };
        self.dictation("validate_model", json!({"models": models, "model": model, "effort": effort, "service_tier": tier.map(ServiceTier::name)}))?;
        Ok(())
    }
    fn account_mode(&self, config_dir: Option<&str>) -> Result<Option<String>, String> { self.local.account_mode(config_dir) }
    fn name_in_use(&self, name: &str) -> Result<bool, OpenError> { self.local.name_in_use(name) }
    fn name_has_leftovers(&self, name: &str) -> Result<bool, OpenError> { self.local.name_has_leftovers(name) }
    fn transfer_busy(&self, name: &str) -> Result<bool, OpenError> { self.local.transfer_busy(name) }
    fn real_path(&self, path: &str) -> PathBuf { self.local.real_path(path) }
    fn new_key(&self) -> String { self.local.new_key() }
    fn now(&self) -> f64 { self.local.now() }
}

struct Host { home: PathBuf, env: BTreeMap<String, String>, terminal: TerminalEnv, mux: FakeTmux, fakes: Value,
    record: std::sync::Arc<Mutex<Record>>, prewarm: Option<std::sync::Arc<crate::prewarm::Store>> }

/// As regras dos programas falsos do harness (`DEFAULT_PROGRAM_RULES` e `fakes.programs`).
fn rule<'a>(fakes: &'a Value, prog: &str, args: &[String]) -> Option<&'a Value> {
    fakes["programs"].as_array().into_iter().flatten().find(|r| {
        r["prog"] == prog && r["args"].as_array().is_none_or(|want| want.iter().zip(args).all(|(w, a)| w == a.as_str()) && want.len() <= args.len())
    })
}

impl OpenHost for Host {
    fn home(&self) -> &Path { &self.home }
    fn env(&self) -> &BTreeMap<String, String> { &self.env }
    fn terminal(&self) -> &TerminalEnv { &self.terminal }
    fn mux(&self) -> &dyn Mux { &self.mux }
    fn bridge(&self) -> Option<PluginBridge> { Some(PluginBridge { port: 8765, secret: self.env.get("CP_AUTH_TOKEN").cloned().unwrap_or_default() }) }
    fn server_projects(&self) -> PathBuf { self.home.join(".claude/projects") }
    fn backend_dir(&self) -> Option<PathBuf> { self.home.parent().map(fake_backend) }

    fn run(&self, argv: Vec<String>, timeout: Duration) -> BoxFuture<'_, MuxOutput> {
        Box::pin(async move {
            let prog = Path::new(&argv[0]).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            // `git` roda de verdade, dentro da raiz, como no lado Python.
            if prog == "git" { return super::server::run_program(&self.env, argv, timeout).await; }
            let args = &argv[1..];
            let found = rule(&self.fakes, &prog, args);
            let code = found.map_or(0, |r| r["rc"].as_i64().unwrap_or(0) as i32);
            let entry = json!({"prog": prog, "argv": args, "rc": code});
            let mut record = self.record.lock().unwrap();
            if found.is_some() { record.probes.push(entry) } else { record.subprocesses.push(entry) }
            MuxOutput { code: Some(code), stdout: found.and_then(|r| r["stdout"].as_str()).unwrap_or("").into(),
                stderr: found.and_then(|r| r["stderr"].as_str()).unwrap_or("").into() }
        })
    }

    fn customizations(&self, args: Value) -> BoxFuture<'_, Result<Value, OpenError>> {
        Box::pin(async move {
            self.record.lock().unwrap().rust_bridge.push(json!({"bridge": "claude_customizations", "op": "prepare", "args": args}));
            let reply = &self.fakes["rust_bridge"]["claude_customizations.prepare"];
            match &reply["error"] {
                Value::Null => Ok(if reply["result"].is_null() { json!({}) } else { reply["result"].clone() }),
                e => Err(OpenError::new(e["status"].as_u64().unwrap_or(503) as u16, "claude_customizations_unavailable",
                    e["detail"].as_str().unwrap_or_default().to_owned())),
            }
        })
    }

    fn leave_old_group<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Vec<String>, OpenError>> {
        Box::pin(async move { self.record.lock().unwrap().cleaned.push(format!("grupo:{name}")); Ok(Vec::new()) })
    }
    fn release_name<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), OpenError>> {
        Box::pin(async move { self.record.lock().unwrap().cleaned.push(format!("nome:{name}")); Ok(()) })
    }

    fn born<'a>(&'a self, born: &'a Born) -> BoxFuture<'a, Vec<String>> {
        Box::pin(async move {
            self.record.lock().unwrap().rust_bridge.push(json!({"bridge": "share", "op": "session_died", "args": {"name": born.name}}));
            if born.remember_provider {
                let base = crate::accounts::environment::AccountEnvironment::from_map(self.env.clone()).claude_base;
                super::server::remember_provider(&base, born.provider.name()).expect("provedor lembrado");
            }
            Vec::new()
        })
    }

    fn warm_codex<'a>(&'a self, name: &'a str, initial_prompt: Option<&'a str>) -> BoxFuture<'a, Vec<String>> {
        Box::pin(async move {
            // A primeira mensagem só existe no Rust (o Python a recusava sem terminal): vai junto quando há.
            let mut request = json!({"op": "codex.aquecer_sem_terminal", "name": name});
            if let Some(text) = initial_prompt { request["initial_prompt"] = json!(text); }
            let step = super::open::progress(name)["step"].clone();
            let mut record = self.record.lock().unwrap();
            if !step.is_null() { record.progress_seen.push(step); }
            record.runtime_requests.push(request);
            Vec::new()
        })
    }

    fn diary(&self, events: Vec<Value>) { self.record.lock().unwrap().diary.extend(events); }
    fn account_diary(&self, events: Vec<Value>) { self.record.lock().unwrap().diary.extend(events); }
    fn new_session_id(&self) -> String { super::open::uuid4(&crate::mods::state::random_hex(16)) }
    fn new_bridge_key(&self) -> String { crate::mods::state::random_hex(16) }
    fn wake(&self, name: &str) {
        self.record.lock().unwrap().runtime_requests.push(json!({"op": "claude_headless.acordar", "name": name}));
    }
    fn prewarm(&self) -> Option<&crate::prewarm::Store> { self.prewarm.as_deref() }
    /// O transcript que a lista do Python dava aos panes Claude do cenário: o id do comando, na conta do pane.
    fn live_sessions(&self) -> BoxFuture<'_, Result<Vec<(String, String)>, OpenError>> {
        Box::pin(async move {
            let sessions = self.mux.sessions.lock().unwrap().clone();
            Ok(sessions.into_iter().filter_map(|(name, s)| {
                let words: Vec<&str> = s["command"].as_str()?.split_whitespace().collect();
                let sid = words.windows(2).find(|w| matches!(w[0], "--session-id" | "--resume")).map(|w| w[1])?;
                let config = s["env"]["CLAUDE_CONFIG_DIR"].as_str().map_or_else(|| self.home.join(".claude"), PathBuf::from);
                let project = hangar_workspace::worktrees::sanitize_cwd(s["cwd"].as_str()?);
                Some((name, config.join("projects").join(project).join(format!("{sid}.jsonl")).to_string_lossy().into_owned()))
            }).collect())
        })
    }
}

/// Roda os passos do cenário (só `POST /api/sessions`) pela abertura do Rust. `{"not_ours": true}`
/// quando algum passo não é do Rust (o Python o atende): o harness diz a rota que falta.
pub async fn run(scenario: &Value, root: &Path, env: BTreeMap<String, String>) -> Value { run_with(scenario, root, env, None).await }

/// A resolução de `request` com os fatos do cenário, como a do passo `POST /api/sessions`, sem gravar
/// nada: o lançador falso da reserva monta o spec dela por aqui.
pub async fn resolve_request(scenario: &Value, root: &Path, env: BTreeMap<String, String>, request: SessionOpenRequest)
    -> Result<super::Resolved, OpenError> {
    let substituted: Value = serde_json::from_str(&place(&scenario.to_string(), root)).unwrap_or(Value::Null);
    resolve_step(substituted["fakes"].clone(), root, env, Default::default(), request).await
}

async fn resolve_step(fakes: Value, root: &Path, env: BTreeMap<String, String>, record: std::sync::Arc<Mutex<Record>>, request: SessionOpenRequest)
    -> Result<super::Resolved, OpenError> {
    let (runtime, root, quotas) = (tokio::runtime::Handle::current(), root.to_owned(), fakes["quotas"].as_array().cloned().unwrap_or_default());
    tokio::task::spawn_blocking(move || {
        let local = super::facts::LocalFacts::new(crate::accounts::environment::AccountEnvironment::from_map(env), runtime)
            .with_terminal(crate::list::mux::Mux::with_program(root.join("bin/tmux"), Duration::from_secs(2)),
                std::sync::Arc::new(crate::list::procs::ProcFs::with_root(proc_root(&root))));
        super::resolve_open(&request, &OpenCaller::Owner, &Facts { local, quotas, fakes, record })
    }).await.unwrap()
}

/// `run` com as reservas pré-aquecidas do teste: o passo com `prewarm` adota como o `decide` do servidor,
/// comparando o spec resolvido com o da reserva.
pub async fn run_with(scenario: &Value, root: &Path, env: BTreeMap<String, String>, prewarm: Option<std::sync::Arc<crate::prewarm::Store>>) -> Value {
    let record = std::sync::Arc::new(Mutex::new(Record::default()));
    let substituted: Value = serde_json::from_str(&place(&scenario.to_string(), root)).unwrap_or(Value::Null);
    let state = &substituted["state"];
    let fakes = substituted["fakes"].clone();
    // `tmux_late_sessions`: nascidas depois da resolução (o tmux as tem, a lista que a resolução leu não).
    let late = fakes["tmux_late_sessions"].as_array().cloned().unwrap_or_default().into_iter().map(|name| json!({"name": name, "env": {}}));
    let sessions: BTreeMap<String, Value> = state["tmux_sessions"].as_array().cloned().unwrap_or_default().into_iter().chain(late)
        .map(|s| (s["name"].as_str().unwrap_or_default().to_owned(), json!({"cwd": s["cwd"], "env": s["env"], "command": s["command"]}))).collect();
    let home = root.join("home");
    let terminal = TerminalEnv { birth: Duration::from_millis(100), ..TerminalEnv::local(&home, &env) };
    let host = Host { home: home.clone(), env: env.clone(), terminal,
        mux: FakeTmux { sessions: Mutex::new(sessions), force: fakes["tmux_new_session"].clone(), record: record.clone(), silenced: Default::default() },
        fakes: fakes.clone(), record: record.clone(), prewarm };
    let mut steps = Vec::new();
    let mut restored = false;
    for step in substituted["steps"].as_array().into_iter().flatten() {
        // `{"call": "restore", "key", "entry"}`: a sessão da fotografia volta pelo spec, como na subida.
        if step["call"] == "restore" {
            restored = true;
            let entry: super::restore::Entry = serde_json::from_value(step["entry"].clone()).expect("entrada da fotografia");
            let (status, body) = match super::restore::resolved_for(&home, &host.server_projects(), step["key"].as_str().unwrap_or_default(), &entry) {
                Ok(resolved) => match super::open::open_session(&host, resolved, &OpenCaller::Owner).await {
                    Ok(opened) => (200, opened.body),
                    Err(error) => (error.status, error.body()),
                },
                Err(code) => (409, json!({"detail": code})),
            };
            steps.push(json!({"call": "restore", "status": status, "body": body}));
            continue;
        }
        let path = step["path"].as_str().unwrap_or_default().to_owned();
        let archive = path.strip_prefix("/api/archive/").and_then(|r| r.strip_suffix("/resume")).and_then(|r| r.split_once('/'))
            .map(|(p, s)| (p.to_owned(), s.to_owned()));
        if let (true, Some((project, sid))) = (step["method"] == "POST", archive) {
            let live = host.live_sessions().await.unwrap_or_default();
            let (facts_env, facts_root, facts_record, quotas, facts_fakes) = (env.clone(), root.to_owned(), record.clone(),
                fakes["quotas"].as_array().cloned().unwrap_or_default(), fakes.clone());
            let (runtime, server_projects, home) = (tokio::runtime::Handle::current(), host.server_projects(), home.clone());
            let body = if step["json"].is_null() { String::new() } else { step["json"].to_string() };
            let (p, s) = (project.clone(), sid.clone());
            let planned = tokio::task::spawn_blocking(move || {
                let local = super::facts::LocalFacts::new(crate::accounts::environment::AccountEnvironment::from_map(facts_env.clone()), runtime)
                    .with_terminal(crate::list::mux::Mux::with_program(facts_root.join("bin/tmux"), Duration::from_secs(2)),
                        std::sync::Arc::new(crate::list::procs::ProcFs::with_root(proc_root(&facts_root))));
                let facts = Facts { local, quotas, fakes: facts_fakes, record: facts_record };
                let config_dirs = facts.config_dirs()?;
                let w = super::archive::Where { home: &home, env: &facts_env, server_projects: &server_projects, config_dirs: &config_dirs, live: &live };
                let plan = super::archive::plan(&w, &facts, &p, &s, super::archive::ArchiveBody::parse(body.as_bytes())?)?;
                let resolved = super::resolve_open(&plan.request, &OpenCaller::Owner, &facts)?;
                Ok::<_, OpenError>((plan, resolved))
            }).await.unwrap();
            let (status, body) = match planned {
                Ok((plan, resolved)) => match { let project = plan.project.clone(); super::archive::open(&host, plan, resolved, &project, &sid).await } {
                    Ok(body) => (200, body),
                    Err(error) => (error.status, error.body()),
                },
                Err(error) => (error.status, error.body()),
            };
            steps.push(json!({"method": "POST", "path": path, "status": status, "body": body}));
            continue;
        }
        if step["method"] != "POST" || step["path"] != "/api/sessions" { return json!({"not_ours": true}); }
        let Ok(mut request) = SessionOpenRequest::parse(step["json"].to_string().as_bytes()) else { return json!({"not_ours": true}) };
        request.take_reserved_name = host.prewarm.is_some();
        let mut adopting = match (request.prewarm.take(), host.prewarm.clone()) {
            (Some(id), Some(store)) => Some(super::adopt::Adopting::take(store, &id, &mut request).await),
            _ => None,
        };
        let resolved = resolve_step(fakes.clone(), root, env.clone(), record.clone(), request.clone()).await;
        // Como o `decide` do servidor: o spec resolvido contra o da reserva.
        if let (Some(adopting), Ok(resolved)) = (adopting.as_mut(), &resolved) { adopting.compare(resolved).await; }
        let (status, body) = match resolved {
            Ok(resolved) if !super::open::opens_here(&resolved.spec) => return json!({"not_ours": true}),
            Ok(resolved) => match super::open::open_adopting(&host, resolved, &OpenCaller::Owner, adopting.as_mut()).await {
                Ok(opened) => (200, opened.body),
                Err(error) => (error.status, error.body()),
            },
            Err(error) => {
                if let Some(events) = super::open::refusal_diary(crate::mods::state::random_hex(8), request.name.as_deref().unwrap_or(""), super::open::requested_provider(&request), &error) { host.diary(events); }
                (error.status, error.body())
            }
        };
        if let Some(adopting) = adopting { host.diary(vec![adopting.finish(body["name"].as_str().unwrap_or_default(), None).await]); }
        steps.push(json!({"method": "POST", "path": "/api/sessions", "status": status, "body": body}));
    }
    let record = std::mem::take(&mut *record.lock().unwrap());
    let tmux_sessions: Map<String, Value> = host.mux.sessions.lock().unwrap().clone().into_iter().collect();
    let mut out = json!({"steps": steps, "subprocesses": record.subprocesses, "probes": record.probes, "rust_bridge": record.rust_bridge,
        "runtime_requests": record.runtime_requests, "signals": [], "network": [], "tmux_sessions": tmux_sessions, "diary": record.diary});
    if restored { out["cleaned"] = json!(record.cleaned); }
    if !record.progress_seen.is_empty() { out["progress_seen"] = json!(record.progress_seen); }
    out
}
