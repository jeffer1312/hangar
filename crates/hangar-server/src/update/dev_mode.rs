//! Liga e desliga o modo desenvolvedor numa máquina que já roda pelo `~/.hangar/current`.
//!
//!   hangar-server dev-mode on <checkout>   o `current` passa a apontar para o checkout
//!   hangar-server dev-mode off             o `current` volta para a versão instalada mais nova
//!
//! A troca é o `swap` do atualizador; o reinício, o `SystemRestarter`; a prova, a mesma. Servidor que
//! não volta faz o `current` voltar e reiniciar de novo.
//! Saída: 0 feito ou nada a fazer, 1 recusado ou falhou, 2 uso errado.
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use hangar_package::{Manifest, Version};

use super::http::{mode, Mode};
use super::restart::{SystemRestarter, Topology};
use super::{HealthProbe, Layout, Reading, Restarter, Settings, StateFile, StepRun, StepRunner, UpdateError};

/// Teto do download do Rust do checkout, o mesmo dos comandos da migração.
pub const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// O checkout não tem `bin/`, e o `rust_bins` dele acha o Rust em `~/.hangar/bin`: o `app.rust_release`
/// do checkout o baixa da release da branch. Devolve o aviso quando não deu; quem chama segue.
pub async fn fetch_rust(runner: Arc<dyn StepRunner + Send + Sync>, checkout: &Path) -> Option<String> {
    let checkout = checkout.to_path_buf();
    let failed = tokio::task::spawn_blocking(move || {
        let windows = cfg!(windows);
        let python = crate::install_root::python_exe(&checkout, windows);
        if !python.is_file() { return Some(format!("sem o Python do checkout em {}", python.display())); }
        // O runner marca o passo como dono do download (`HANGAR_UPDATE_OWNS_RUST=1`), e com ele o `rust_release` não baixa.
        let command = if windows {
            format!("set HANGAR_UPDATE_OWNS_RUST=&& \"{}\" -m app.rust_release", python.display())
        } else {
            format!("HANGAR_UPDATE_OWNS_RUST= '{}' -m app.rust_release", python.display().to_string().replace('\'', "'\\''"))
        };
        let StepRun { code, output } = runner.run(&command, &checkout.join("backend"));
        (code != Some(0)).then(|| format!("o app.rust_release saiu com {code:?}: {}", output.trim()))
    }).await.unwrap_or_else(|e| Some(e.to_string()))?;
    let warning = format!("o Rust do modo desenvolvedor não foi atualizado ({failed}); o servidor pode subir com o binário antigo de ~/.hangar/bin");
    tracing::warn!(code = "update_dev_rust_fetch_failed", "update: {warning}");
    Some(warning)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    On(PathBuf),
    Off,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// O `current` aponta para o caminho real dado, e o servidor novo provou estar de pé.
    Switched(PathBuf),
    /// Trocado sem reiniciar (instalação na mão): vale no próximo reinício.
    SwitchedManual(PathBuf),
    /// Já estava no modo pedido; nada foi tocado.
    Already(String),
}

/// A versão instalada mais nova que o atualizador aceitaria: pasta com nome de versão, manifesto
/// válido deste sistema e com a mesma versão do nome.
pub fn newest_package(layout: &Layout, platform: &str) -> Option<(PathBuf, Manifest)> {
    std::fs::read_dir(layout.versions()).ok()?.flatten().filter_map(|entry| {
        let version: Version = entry.file_name().to_str()?.parse().ok()?;
        let dir = entry.path();
        let manifest = super::read_manifest(&dir)?;
        let valid = manifest.os == platform && manifest.version.parse::<Version>().ok() == Some(version) && super::is_package(&dir);
        valid.then_some((version, dir, manifest))
    }).max_by_key(|(version, ..)| *version).map(|(_, dir, manifest)| (dir, manifest))
}

/// Recusa ou falha, com o código que a rota devolve à tela.
#[derive(Debug, PartialEq, Eq)]
pub struct Failure {
    pub code: &'static str,
    pub message: String,
}

impl Failure {
    fn new(code: &'static str, message: impl Into<String>) -> Failure { Failure { code, message: message.into() } }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.message) }
}

/// A trava ocupada: o estado é de quem a segura.
pub const BUSY: &str = "erro_atualizacao_ja_rodando";
/// O servidor novo não provou estar de pé, o `current` voltou para onde estava e o de antes provou estar de pé.
pub const ROLLED_BACK: &str = "erro_dev_mode_voltou";

/// O que o pedido faria, conferido sem tocar em nada.
#[derive(Debug)]
pub enum Checked {
    Switch { target: PathBuf, manifest: Option<Manifest> },
    Already(String),
}

/// As recusas que não dependem de trava nem de processo: a rota responde com elas antes de lançar.
pub fn check(home: &Path, request: &Request) -> Result<Checked, Failure> {
    let layout = Layout::new(home);
    // Sem `current` o serviço não passa pelo lançador: apontá-lo não mudaria o que roda.
    if std::fs::symlink_metadata(layout.current()).is_err() && std::fs::symlink_metadata(layout.aside()).is_err() {
        return Err(Failure::new("erro_dev_mode_nao_migrado",
            format!("esta máquina não roda pelo {}; migre antes com `hangar-server migrate-package`", layout.current().display())));
    }
    match request {
        Request::On(dir) => {
            let missing = |e: String| Failure::new("erro_dev_mode_pasta_inexistente", e);
            if !dir.exists() { return Err(missing(format!("{} não existe", dir.display()))); }
            let dir = super::real(dir).map_err(|e| missing(format!("{}: {e}", dir.display())))?;
            if !super::migrate::is_checkout(&dir) {
                return Err(Failure::new("erro_dev_mode_nao_e_checkout",
                    format!("{} não é um checkout do Hangar (precisa de .git e backend/app)", dir.display())));
            }
            if mode(home) == Mode::Developer(dir.clone()) {
                return Ok(Checked::Already(format!("o modo desenvolvedor já está ligado em {}; nada a fazer", dir.display())));
            }
            Ok(Checked::Switch { target: dir, manifest: None })
        }
        Request::Off => {
            if let Mode::Package(_) = mode(home) {
                let version = super::installed(&layout).map_or_else(String::new, |m| m.version);
                return Ok(Checked::Already(format!("o modo desenvolvedor já está desligado (pacote {version}); nada a fazer")));
            }
            let platform = super::platform();
            let none = || Failure::new("erro_dev_mode_sem_pacote", format!(
                "nenhuma versão do pacote para {platform} em {}, e não há para onde voltar; no modo desenvolvedor o Atualizar só faz \
                 `git pull`: para ter o pacote, desfaça a migração (`hangar-server migrate-package --undo`) e migre de novo com o \
                 checkout limpo na main", layout.versions().display()));
            let (dir, manifest) = newest_package(&layout, &platform).ok_or_else(none)?;
            let target = super::real(&dir).map_err(|e| Failure::new("erro_dev_mode_sem_pacote", format!("{}: {e}", dir.display())))?;
            Ok(Checked::Switch { target, manifest: Some(manifest) })
        }
    }
}

/// A pasta que a tela oferece: o checkout no ar no modo desenvolvedor; senão o que a migração gravou.
pub fn suggested_checkout(home: &Path) -> Option<PathBuf> {
    if let Mode::Developer(root) = mode(home) { return Some(root); }
    // Instalação nova pelo pacote grava a marca sem checkout.
    super::migrate::read_record(&home.join(".hangar").join(super::migrate::MARKER)).map(|r| r.checkout).filter(|c| !c.as_os_str().is_empty())
}

/// Troca o modo. `in_service_group`: este processo morre junto com o backend (cgroup da unit, Job do
/// lançador), e o reinício o mataria no meio. `on_step` recebe `trocar` e `reiniciar`, só depois de
/// todas as recusas. `on_warning` recebe o que não impede a troca (o Rust do checkout não baixou).
#[allow(clippy::too_many_arguments)]
pub async fn switch<R: Restarter, H: HealthProbe>(home: &Path, state_dir: &Path, request: &Request, in_service_group: bool,
    restarter: &R, health: &H, settings: &Settings, runner: Arc<dyn StepRunner + Send + Sync>, on_step: &(dyn Fn(&str) + Sync),
    on_warning: &(dyn Fn(&str) + Sync)) -> Result<Outcome, Failure> {
    let layout = Layout::new(home);
    let (target, manifest) = match check(home, request)? {
        Checked::Already(text) => return Ok(Outcome::Already(text)),
        Checked::Switch { target, manifest } => (target, manifest),
    };
    if in_service_group {
        return Err(Failure::new("erro_dev_mode_no_servico",
            "o dev-mode está rodando dentro do serviço do backend, e o reinício o mataria no meio; rode-o num terminal fora dele"));
    }
    let _lock = super::UpdateLock::try_acquire(&layout).map_err(|e| match e {
        UpdateError::Locked => Failure::new(BUSY, "uma atualização está rodando nesta máquina; tente de novo depois"),
        e => Failure::new("erro_dev_mode_falhou", e.to_string()),
    })?;
    if let Some(pid) = super::other_engine(&StateFile::new(state_dir), state_dir) {
        return Err(Failure::new("erro_dev_mode_motor_git", format!("o motor git do Python está atualizando esta máquina (pid {pid}); tente de novo depois")));
    }
    on_step("trocar");
    if matches!(request, Request::On(_)) {
        if let Some(warning) = fetch_rust(runner, &target).await { on_warning(&warning); }
    }
    let before = match health.read().await { Reading::Rust { pid, .. } => pid, _ => None };
    let (swap_layout, swap_target) = (layout.clone(), target.clone());
    let previous = super::blocking(move || super::swap(&swap_layout, &swap_target)).await
        .map_err(|e| Failure::new("erro_dev_mode_falhou", e.to_string()))?;
    if restarter.manual() { return Ok(Outcome::SwitchedManual(target)); }
    on_step("reiniciar");
    let proved = match restarter.restart().await {
        Err(e) => Err(format!("o servidor não reiniciou: {e}")),
        Ok(()) => match &manifest {
            Some(manifest) => super::wait_rust(health, manifest, before, settings).await,
            None => super::wait_new(health, before, settings).await,
        },
    };
    match proved {
        Ok(()) => Ok(Outcome::Switched(target)),
        Err(why) => Err(roll_back(&layout, previous, why, restarter, health, settings).await),
    }
}

/// O `current` volta para onde apontava e o servidor reinicia de novo; devolve o motivo com o
/// desfecho da volta. `ROLLED_BACK` só com o servidor de antes provado de pé, como na atualização.
async fn roll_back<R: Restarter, H: HealthProbe>(layout: &Layout, previous: Option<PathBuf>, why: String, restarter: &R, health: &H,
    settings: &Settings) -> Failure {
    let failed = |text: String| Failure::new("erro_dev_mode_falhou", text);
    let Some(previous) = previous else { return failed(format!("{why}; o current de antes estava quebrado e não há para onde voltar")) };
    // O checkout não tem manifesto: a prova da volta para ele é o Rust com outro pid.
    let expected = super::read_manifest(&previous);
    let failed_pid = super::failed_pid(health, expected.as_ref()).await;
    let layout = layout.clone();
    if let Err(e) = super::blocking(move || super::swap(&layout, &previous)).await {
        return failed(format!("{why}; e o current não voltou para onde estava: {e}"));
    }
    if let Err(e) = restarter.restart().await {
        return failed(format!("{why}; o current voltou para onde estava, mas o reinício falhou: {e}"));
    }
    if let Err(e) = super::wait_previous(health, expected.as_ref(), failed_pid, settings).await {
        return failed(format!("{why}; o current voltou para onde estava e o servidor reiniciou, mas o de antes não se confirmou no ar: {e}"));
    }
    Failure::new(ROLLED_BACK, format!("{why}; o current voltou para onde estava e o servidor reiniciou"))
}

/// `on <checkout>` ou `off`; o resto é uso errado.
pub fn parse(args: &[String]) -> Option<Request> {
    match args {
        [on, dir] if on == "on" => Some(Request::On(PathBuf::from(dir))),
        [off] if off == "off" => Some(Request::Off),
        _ => None,
    }
}

/// `hangar-server dev-mode …`: `args` sem o nome do programa nem o subcomando.
pub fn cli(args: &[String]) -> ExitCode {
    let Some(request) = parse(args) else {
        eprintln!("uso: hangar-server dev-mode on <checkout> | off");
        return ExitCode::from(2);
    };
    let env = crate::accounts::environment::AccountEnvironment::capture();
    let home = env.home.clone();
    let current = crate::install_root::current(&home);
    let port = match super::restart::env_port(&home, &current, "CP_PORT") {
        Ok(port) => port.unwrap_or(8765),
        Err(e) => { println!("##HANGAR-FALHA## {e}"); return ExitCode::from(1); }
    };
    let runner = Arc::new(super::ShellRunner { timeout: super::steps::STEP_TIMEOUT });
    let topology = Topology::detect(&*runner, &home, cfg!(windows));
    let restarter = SystemRestarter { topology, runner, home: home.clone(), port };
    let health = super::HttpHealth::new(&format!("http://127.0.0.1:{port}"));
    let runtime = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
        Ok(runtime) => runtime,
        Err(e) => { println!("##HANGAR-FALHA## runtime: {e}"); return ExitCode::from(1); }
    };
    let result = runtime.block_on(switch(&home, &super::state_dir(&env.claude_base), &request, super::migrate::in_service_group(),
        &restarter, &health, &Settings::default(), Arc::new(super::ShellRunner { timeout: FETCH_TIMEOUT }), &|_| (),
        &|warning| println!("aviso: {warning}")));
    let label = if request == Request::Off { "desligado" } else { "ligado" };
    match result {
        Ok(Outcome::Switched(dir)) => println!("modo desenvolvedor {label}: {} aponta para {}; servidor reiniciado", current.display(), dir.display()),
        Ok(Outcome::SwitchedManual(dir)) => println!("modo desenvolvedor {label}: {} aponta para {}; sem serviço para reiniciar, \
            vale no próximo reinício do servidor", current.display(), dir.display()),
        Ok(Outcome::Already(text)) => println!("{text}"),
        Err(e) => { println!("##HANGAR-FALHA## {e}"); return ExitCode::from(1); }
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::update::{Layout, Reading};

    struct FakeRestart(Arc<AtomicUsize>);
    impl Restarter for FakeRestart {
        async fn restart(&self) -> Result<(), String> { self.0.fetch_add(1, Ordering::SeqCst); Ok(()) }
    }

    /// Pid 10 antes de reiniciar; depois, 10 + reinícios (ou 10 sempre, com `proves` falso). `commit`
    /// é o pacote de onde o servidor diz ter subido.
    struct FakeHealth { restarts: Arc<AtomicUsize>, proves: bool, commit: Option<String> }
    impl HealthProbe for FakeHealth {
        async fn read(&self) -> Reading {
            let n = if self.proves { self.restarts.load(Ordering::SeqCst) as u32 } else { 0 };
            Reading::Rust { protocol: 1, pid: Some(10 + n), commit: self.commit.clone() }
        }
    }

    fn fast() -> Settings { Settings { health_deadline: Duration::from_millis(100), health_interval: Duration::from_millis(5), ..Settings::default() } }

    fn real(path: &Path) -> PathBuf { crate::list::links::without_verbatim(std::fs::canonicalize(path).unwrap()) }

    /// Uma máquina em pacote (`0.1.0.5`, commit `c5`) e um checkout ao lado.
    struct Machine { _dir: tempfile::TempDir, home: PathBuf, checkout: PathBuf, restarts: Arc<AtomicUsize> }

    impl Machine {
        fn new() -> Machine {
            let dir = tempfile::tempdir().unwrap();
            let home = dir.path().join("home");
            let checkout = dir.path().join("hangar");
            std::fs::create_dir_all(checkout.join(".git")).unwrap();
            std::fs::create_dir_all(checkout.join("backend/app")).unwrap();
            let m = Machine { _dir: dir, home, checkout, restarts: Arc::default() };
            let version = m.version("0.1.0.5", &crate::update::platform());
            crate::update::swap(&m.layout(), &version).unwrap();
            m
        }
        fn layout(&self) -> Layout { Layout::new(&self.home) }
        fn state_dir(&self) -> PathBuf { self.home.join(".claude/.hangar-update") }
        fn version(&self, name: &str, os: &str) -> PathBuf {
            let dir = self.layout().versions().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            let commit = format!("c{}", name.rsplit('.').next().unwrap());
            std::fs::write(dir.join("manifest.json"), json!({"version": name, "commit": commit, "channel": "latest", "protocol": 1,
                "api": 1, "os": os, "files": {}}).to_string()).unwrap();
            dir
        }
        fn current(&self) -> PathBuf { real(&self.layout().current()) }
        fn restart(&self) -> FakeRestart { FakeRestart(self.restarts.clone()) }
        fn health(&self, proves: bool, commit: Option<&str>) -> FakeHealth {
            FakeHealth { restarts: self.restarts.clone(), proves, commit: commit.map(str::to_owned) }
        }
        async fn switch(&self, request: Request, inside: bool, health: &FakeHealth) -> Result<Outcome, String> {
            switch(&self.home, &self.state_dir(), &request, inside, &self.restart(), health, &fast(), self.runner(Some(0)), &|_| (), &|_| ())
                .await.map_err(|f| f.message)
        }
        fn runner(&self, code: Option<i32>) -> Arc<Recorder> {
            Arc::new(Recorder { home: self.home.clone(), code, calls: std::sync::Mutex::default() })
        }
        /// O Python do venv do checkout, onde o `fetch_rust` o procura.
        fn venv(&self) -> PathBuf {
            let python = crate::install_root::python_exe(&real(&self.checkout), cfg!(windows));
            std::fs::create_dir_all(python.parent().unwrap()).unwrap();
            std::fs::write(&python, "").unwrap();
            python
        }
    }

    /// Grava o comando, a pasta e para onde o `current` apontava quando rodou.
    struct Recorder { home: PathBuf, code: Option<i32>, calls: std::sync::Mutex<Vec<(String, PathBuf, PathBuf)>> }
    impl StepRunner for Recorder {
        fn run(&self, command: &str, cwd: &Path) -> StepRun {
            self.calls.lock().unwrap().push((command.to_owned(), cwd.to_owned(), real(&Layout::new(&self.home).current())));
            StepRun { code: self.code, output: "sem rede".into() }
        }
    }

    /// O checkout não tem `bin/`: o Rust dele vem do `~/.hangar/bin`, que o `rust_release` do checkout
    /// atualiza antes da troca. Falha vira aviso e a troca segue; o `off` não baixa nada.
    #[tokio::test]
    async fn dev_mode_on_fetches_the_checkout_rust_before_switching_and_a_failure_is_only_a_warning() {
        let m = Machine::new();
        let package = m.current();
        let health = m.health(true, Some("c5"));
        let warnings = std::sync::Mutex::new(Vec::<String>::new());
        let warn = |w: &str| warnings.lock().unwrap().push(w.to_owned());
        let (state_dir, request, restart, settings) = (m.state_dir(), Request::On(m.checkout.clone()), m.restart(), fast());
        let on = |runner: Arc<Recorder>| switch(&m.home, &state_dir, &request, false, &restart, &health, &settings, runner, &|_| (), &warn);

        let runner = m.runner(Some(0));
        assert_eq!(on(runner.clone()).await, Ok(Outcome::Switched(real(&m.checkout))));
        assert!(runner.calls.lock().unwrap().is_empty(), "sem venv não há o que rodar");
        let missing = warnings.lock().unwrap().clone();
        assert!(missing.len() == 1 && missing[0].contains("Python"), "{missing:?}");

        let off = m.runner(Some(0));
        switch(&m.home, &m.state_dir(), &Request::Off, false, &m.restart(), &health, &fast(), off.clone(), &|_| (), &warn).await.unwrap();
        assert!(off.calls.lock().unwrap().is_empty(), "o off volta ao bin/ do pacote");

        let python = m.venv();
        warnings.lock().unwrap().clear();
        let failing = m.runner(None);
        assert_eq!(on(failing.clone()).await, Ok(Outcome::Switched(real(&m.checkout))), "a falha não impede a troca");
        let calls = failing.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        let (command, cwd, current) = &calls[0];
        assert!(command.contains(&python.display().to_string()) && command.ends_with("-m app.rust_release"), "{command}");
        assert!(command.contains("HANGAR_UPDATE_OWNS_RUST="), "o runner marca o passo como dono do download: {command}");
        assert_eq!(cwd, &real(&m.checkout).join("backend"));
        assert_eq!(current, &package, "roda antes da troca e do reinício");
        let failed = warnings.lock().unwrap().clone();
        assert!(failed.len() == 1 && failed[0].contains("sem rede"), "{failed:?}");
    }

    /// O comando de verdade, pelo shell: o venv numa pasta com espaço e aspa, e o
    /// `HANGAR_UPDATE_OWNS_RUST=1` do `ShellRunner` não chega ao `rust_release`.
    #[cfg(unix)]
    #[tokio::test]
    async fn dev_mode_fetch_rust_runs_the_checkout_venv_from_backend_without_the_update_marker() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let checkout = dir.path().join("che ck'out");
        let python = crate::install_root::python_exe(&checkout, false);
        std::fs::create_dir_all(python.parent().unwrap()).unwrap();
        std::fs::write(&python, "#!/bin/sh\n[ \"$HANGAR_UPDATE_OWNS_RUST\" = 1 ] && exit 3\necho \"$PWD $*\" > \"$0.ran\"\n").unwrap();
        std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
        let runner = Arc::new(super::super::ShellRunner { timeout: Duration::from_secs(20) });
        assert_eq!(fetch_rust(runner, &checkout).await, None);
        let ran = std::fs::read_to_string(python.with_extension("ran")).unwrap();
        // Canônico: no macOS o temporário fica sob /var, link para /private/var, e o $PWD vem resolvido.
        assert_eq!(ran.trim(), format!("{} -m app.rust_release", checkout.join("backend").canonicalize().unwrap().display()));
    }

    #[tokio::test]
    async fn dev_mode_on_points_current_to_the_checkout_restarts_and_proves() {
        let m = Machine::new();
        let outcome = m.switch(Request::On(m.checkout.clone()), false, &m.health(true, None)).await;
        assert_eq!(outcome, Ok(Outcome::Switched(real(&m.checkout))));
        assert_eq!(m.current(), real(&m.checkout));
        assert_eq!(m.restarts.load(Ordering::SeqCst), 1);
        // De novo: já está lá, nada se reinicia.
        let again = m.switch(Request::On(m.checkout.clone()), false, &m.health(true, None)).await;
        assert!(matches!(again, Ok(Outcome::Already(_))), "{again:?}");
        assert_eq!(m.restarts.load(Ordering::SeqCst), 1);
    }

    /// Cada recusa devolve o motivo sem trocar o `current` nem reiniciar.
    #[tokio::test]
    async fn dev_mode_refusals_touch_nothing() {
        let m = Machine::new();
        let before = m.current();
        let health = m.health(true, None);
        let refused = |result: Result<Outcome, String>, needle: &str| {
            let error = result.expect_err(needle);
            assert!(error.contains(needle), "{needle}: {error}");
            assert_eq!(m.current(), before, "{needle}");
            assert_eq!(m.restarts.load(Ordering::SeqCst), 0, "{needle}");
        };
        refused(m.switch(Request::On(m.checkout.join("nada")), false, &health).await, "não existe");
        let plain = m.checkout.join("backend");
        refused(m.switch(Request::On(plain), false, &health).await, "não é um checkout do Hangar");
        refused(m.switch(Request::On(m.checkout.clone()), true, &health).await, "dentro do serviço do backend");
        {
            let _held = crate::update::UpdateLock::try_acquire(&m.layout()).unwrap();
            refused(m.switch(Request::On(m.checkout.clone()), false, &health).await, "atualização está rodando");
        }
        crate::update::StateFile::new(&m.state_dir()).write(json!({"fase": "rodando", "pid": std::process::id()}));
        refused(m.switch(Request::On(m.checkout.clone()), false, &health).await, "motor git");
        std::fs::remove_dir_all(m.state_dir()).unwrap();

        // Sem `current` a máquina não foi migrada: quem decide é a migração.
        crate::update::remove_link(&m.layout().current()).unwrap();
        let error = m.switch(Request::On(m.checkout.clone()), false, &health).await.unwrap_err();
        assert!(error.contains("migrate-package"), "{error}");
        assert!(std::fs::symlink_metadata(m.layout().current()).is_err(), "nada foi criado");
    }

    #[tokio::test]
    async fn dev_mode_off_goes_back_to_the_newest_valid_package_of_this_system() {
        let m = Machine::new();
        let newest = m.version("0.1.0.9", &crate::update::platform());
        m.version("0.1.0.12", "plan9-mips");
        let broken = m.version("0.1.0.11", &crate::update::platform());
        std::fs::write(broken.join("manifest.json"), "{\"version\": \"0.1.0.11\"}").unwrap();
        // Prova pelo commit do manifesto que volta.
        let health = m.health(true, Some("c9"));
        m.switch(Request::On(m.checkout.clone()), false, &health).await.unwrap();

        assert_eq!(m.switch(Request::Off, true, &health).await.map_err(|e| e.contains("dentro do serviço")), Err(true));
        assert_eq!(m.current(), real(&m.checkout));

        assert_eq!(m.switch(Request::Off, false, &health).await, Ok(Outcome::Switched(real(&newest))));
        assert_eq!(m.current(), real(&newest));
        assert_eq!(m.restarts.load(Ordering::SeqCst), 2);
        let again = m.switch(Request::Off, false, &health).await;
        assert!(matches!(again, Ok(Outcome::Already(_))), "{again:?}");
        assert_eq!(m.restarts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn dev_mode_off_without_an_installed_package_refuses() {
        let m = Machine::new();
        let health = m.health(true, None);
        m.switch(Request::On(m.checkout.clone()), false, &health).await.unwrap();
        std::fs::remove_dir_all(m.layout().versions()).unwrap();
        let error = m.switch(Request::Off, false, &health).await.unwrap_err();
        assert!(error.contains("nenhuma versão do pacote"), "{error}");
        assert_eq!(m.current(), real(&m.checkout));
        assert_eq!(m.restarts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn dev_mode_server_that_does_not_come_back_points_current_back_and_restarts() {
        let m = Machine::new();
        let before = m.current();
        let error = m.switch(Request::On(m.checkout.clone()), false, &m.health(false, None)).await.unwrap_err();
        assert!(error.contains("não voltou depois de reiniciar") && error.contains("voltou para onde estava"), "{error}");
        assert_eq!(m.current(), before);
        assert_eq!(m.restarts.load(Ordering::SeqCst), 2, "o reinício da troca e o da volta");
    }

    /// O reinício de número `fails` (1 = o da troca) falha.
    struct FailingRestart { count: Arc<AtomicUsize>, fails: usize }
    impl Restarter for FailingRestart {
        async fn restart(&self) -> Result<(), String> {
            let n = self.count.fetch_add(1, Ordering::SeqCst) + 1;
            if n == self.fails { Err("systemctl saiu com 1".into()) } else { Ok(()) }
        }
    }

    /// A leitura depende de quantos reinícios já houve.
    struct ByRestart { restarts: Arc<AtomicUsize>, reading: fn(usize) -> Reading }
    impl HealthProbe for ByRestart {
        async fn read(&self) -> Reading { (self.reading)(self.restarts.load(Ordering::SeqCst)) }
    }

    /// Antes: o pacote `0.1.0.5` (pid 10); depois do 1º reinício, ninguém; depois do 2º, o pacote de novo.
    fn old_comes_back(n: usize) -> Reading {
        match n {
            1 => Reading::Down("connection refused".into()),
            n => Reading::Rust { protocol: 1, pid: Some(10 + n as u32), commit: Some("c5".into()) },
        }
    }

    /// Depois do 1º reinício, ninguém responde mais.
    fn never_back(n: usize) -> Reading {
        if n == 0 { Reading::Rust { protocol: 1, pid: Some(10), commit: Some("c5".into()) } } else { Reading::Down("connection refused".into()) }
    }

    async fn failed_on(m: &Machine, restart: &impl Restarter, reading: fn(usize) -> Reading) -> Failure {
        let health = ByRestart { restarts: m.restarts.clone(), reading };
        switch(&m.home, &m.state_dir(), &Request::On(m.checkout.clone()), false, restart, &health, &fast(), m.runner(Some(0)), &|_| (), &|_| ())
            .await.unwrap_err()
    }

    /// `ROLLED_BACK` (e o `voltou: true` da tela) só com a versão anterior provada de pé de novo.
    #[tokio::test]
    async fn dev_mode_rolled_back_only_when_the_previous_server_is_proven_up() {
        let m = Machine::new();
        let back = failed_on(&m, &m.restart(), old_comes_back).await;
        assert_eq!(back.code, ROLLED_BACK, "{back}");
        assert_eq!(m.restarts.load(Ordering::SeqCst), 2);

        let m = Machine::new();
        let restart = FailingRestart { count: m.restarts.clone(), fails: 2 };
        let refused = failed_on(&m, &restart, old_comes_back).await;
        assert_ne!(refused.code, ROLLED_BACK, "o reinício da volta falhou: {refused}");
        assert!(refused.message.contains("systemctl saiu com 1"), "{refused}");

        let m = Machine::new();
        let down = failed_on(&m, &m.restart(), never_back).await;
        assert_ne!(down.code, ROLLED_BACK, "a versão anterior não voltou: {down}");
        assert_eq!(m.current(), real(&m.layout().versions().join("0.1.0.5")), "o current voltou mesmo assim");
    }

    struct Manual;
    impl Restarter for Manual {
        async fn restart(&self) -> Result<(), String> { Err("na mão não reinicia".into()) }
        fn manual(&self) -> bool { true }
    }

    #[tokio::test]
    async fn dev_mode_by_hand_switches_without_restarting() {
        let m = Machine::new();
        let outcome = switch(&m.home, &m.state_dir(), &Request::On(m.checkout.clone()), false, &Manual, &m.health(false, None), &fast(), m.runner(Some(0)), &|_| (), &|_| ()).await;
        assert_eq!(outcome, Ok(Outcome::SwitchedManual(real(&m.checkout))));
        assert_eq!(m.current(), real(&m.checkout));
    }

    /// A rota confere antes de lançar o executor: cada recusa tem o código que a tela recebe.
    #[tokio::test]
    async fn dev_mode_check_answers_each_refusal_with_its_code() {
        let m = Machine::new();
        let code = |request: Request| check(&m.home, &request).map(|_| ()).unwrap_err().code;
        assert_eq!(code(Request::On(m.checkout.join("nada"))), "erro_dev_mode_pasta_inexistente");
        assert_eq!(code(Request::On(m.checkout.join("backend"))), "erro_dev_mode_nao_e_checkout");
        assert!(matches!(check(&m.home, &Request::Off), Ok(Checked::Already(_))));
        assert!(matches!(check(&m.home, &Request::On(m.checkout.clone())), Ok(Checked::Switch { .. })));

        m.switch(Request::On(m.checkout.clone()), false, &m.health(true, None)).await.unwrap();
        assert!(matches!(check(&m.home, &Request::On(m.checkout.clone())), Ok(Checked::Already(_))));
        std::fs::remove_dir_all(m.layout().versions()).unwrap();
        assert_eq!(code(Request::Off), "erro_dev_mode_sem_pacote");

        crate::update::remove_link(&m.layout().current()).unwrap();
        assert_eq!(code(Request::On(m.checkout.clone())), "erro_dev_mode_nao_migrado");
    }

    /// A pasta que a tela preenche: o checkout no ar no modo desenvolvedor; senão o da migração.
    #[tokio::test]
    async fn dev_mode_suggested_checkout_is_the_running_one_or_the_migrated_one() {
        let m = Machine::new();
        assert_eq!(suggested_checkout(&m.home), None, "pacote sem registro da migração");
        std::fs::write(m.home.join(".hangar/migration.json"), json!({"mode": "package", "date": "d", "checkout": "/x/hangar",
            "version": null, "reason": null, "notes": [], "done": []}).to_string()).unwrap();
        assert_eq!(suggested_checkout(&m.home), Some(PathBuf::from("/x/hangar")));
        m.switch(Request::On(m.checkout.clone()), false, &m.health(true, None)).await.unwrap();
        assert_eq!(suggested_checkout(&m.home), Some(real(&m.checkout)));
    }

    /// Os passos só são anunciados depois das recusas: quem anuncia grava o pid no estado.
    #[tokio::test]
    async fn dev_mode_steps_are_announced_only_once_the_switch_is_going_ahead() {
        let m = Machine::new();
        let seen = std::sync::Mutex::new(Vec::new());
        let note = |key: &str| seen.lock().unwrap().push(key.to_owned());
        let health = m.health(true, None);
        let refused = switch(&m.home, &m.state_dir(), &Request::On(m.checkout.join("nada")), false, &m.restart(), &health, &fast(),
            m.runner(Some(0)), &note, &|_| ()).await;
        assert!(refused.is_err());
        assert!(seen.lock().unwrap().is_empty());
        switch(&m.home, &m.state_dir(), &Request::On(m.checkout.clone()), false, &m.restart(), &health, &fast(), m.runner(Some(0)), &note, &|_| ())
            .await.unwrap();
        assert_eq!(*seen.lock().unwrap(), ["trocar", "reiniciar"]);
    }

    #[test]
    fn dev_mode_cli_reads_on_and_off_and_exits_2_on_anything_else() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(parse(&args(&["on", "/x/hangar"])), Some(Request::On(PathBuf::from("/x/hangar"))));
        assert_eq!(parse(&args(&["off"])), Some(Request::Off));
        for bad in [&[][..], &["on"], &["off", "/x"], &["liga", "/x"], &["on", "/x", "/y"]] {
            assert_eq!(parse(&args(bad)), None, "{bad:?}");
            assert_eq!(cli(&args(bad)), ExitCode::from(2), "{bad:?}");
        }
    }
}
