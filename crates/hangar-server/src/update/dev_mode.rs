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
use super::{HealthProbe, Layout, Reading, Restarter, Settings, StateFile, UpdateError};

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

/// Troca o modo. `in_service_group`: este processo morre junto com o backend (cgroup da unit, Job do
/// lançador), e o reinício o mataria no meio.
pub async fn switch<R: Restarter, H: HealthProbe>(home: &Path, state_dir: &Path, request: &Request, in_service_group: bool,
    restarter: &R, health: &H, settings: &Settings) -> Result<Outcome, String> {
    let layout = Layout::new(home);
    // Sem `current` o serviço não passa pelo lançador: apontá-lo não mudaria o que roda.
    if std::fs::symlink_metadata(layout.current()).is_err() && std::fs::symlink_metadata(layout.aside()).is_err() {
        return Err(format!("esta máquina não roda pelo {}; migre antes com `hangar-server migrate-package`", layout.current().display()));
    }
    let (target, manifest) = match request {
        Request::On(dir) => {
            if !dir.exists() { return Err(format!("{} não existe", dir.display())); }
            let dir = super::real(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            if !super::migrate::is_checkout(&dir) {
                return Err(format!("{} não é um checkout do Hangar (precisa de .git e backend/app)", dir.display()));
            }
            if mode(home) == Mode::Developer(dir.clone()) {
                return Ok(Outcome::Already(format!("o modo desenvolvedor já está ligado em {}; nada a fazer", dir.display())));
            }
            (dir, None::<Manifest>)
        }
        Request::Off => {
            if let Mode::Package(_) = mode(home) {
                let version = super::installed(&layout).map_or_else(String::new, |m| m.version);
                return Ok(Outcome::Already(format!("o modo desenvolvedor já está desligado (pacote {version}); nada a fazer")));
            }
            let platform = super::platform();
            let (dir, manifest) = newest_package(&layout, &platform).ok_or_else(|| format!(
                "nenhuma versão do pacote para {platform} em {}, e não há para onde voltar; no modo desenvolvedor o Atualizar só faz \
                 `git pull`: para ter o pacote, desfaça a migração (`hangar-server migrate-package --undo`) e migre de novo com o \
                 checkout limpo na main", layout.versions().display()))?;
            (super::real(&dir).map_err(|e| format!("{}: {e}", dir.display()))?, Some(manifest))
        }
    };
    if in_service_group {
        return Err("o dev-mode está rodando dentro do serviço do backend, e o reinício o mataria no meio; rode-o num terminal fora dele".into());
    }
    let _lock = super::UpdateLock::try_acquire(&layout).map_err(|e| match e {
        UpdateError::Locked => "uma atualização está rodando nesta máquina; tente de novo depois".to_owned(),
        e => e.to_string(),
    })?;
    if let Some(pid) = super::other_engine(&StateFile::new(state_dir), state_dir) {
        return Err(format!("o motor git do Python está atualizando esta máquina (pid {pid}); tente de novo depois"));
    }
    let before = match health.read().await { Reading::Rust { pid, .. } => pid, _ => None };
    let (swap_layout, swap_target) = (layout.clone(), target.clone());
    let previous = super::blocking(move || super::swap(&swap_layout, &swap_target)).await.map_err(|e| e.to_string())?;
    if restarter.manual() { return Ok(Outcome::SwitchedManual(target)); }
    let proved = match restarter.restart().await {
        Err(e) => Err(format!("o servidor não reiniciou: {e}")),
        Ok(()) => match &manifest {
            Some(manifest) => super::wait_rust(health, manifest, before, settings).await,
            None => super::wait_new(health, before, settings).await,
        },
    };
    match proved {
        Ok(()) => Ok(Outcome::Switched(target)),
        Err(why) => Err(roll_back(&layout, previous, why, restarter).await),
    }
}

/// O `current` volta para onde apontava e o servidor reinicia de novo; devolve o motivo com o
/// desfecho da volta.
async fn roll_back<R: Restarter>(layout: &Layout, previous: Option<PathBuf>, why: String, restarter: &R) -> String {
    let Some(previous) = previous else { return format!("{why}; o current de antes estava quebrado e não há para onde voltar") };
    let layout = layout.clone();
    if let Err(e) = super::blocking(move || super::swap(&layout, &previous)).await {
        return format!("{why}; e o current não voltou para onde estava: {e}");
    }
    match restarter.restart().await {
        Ok(()) => format!("{why}; o current voltou para onde estava e o servidor reiniciou"),
        Err(e) => format!("{why}; o current voltou para onde estava, mas o reinício falhou: {e}"),
    }
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
        &restarter, &health, &Settings::default()));
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
            switch(&self.home, &self.state_dir(), &request, inside, &self.restart(), health, &fast()).await
        }
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

    struct Manual;
    impl Restarter for Manual {
        async fn restart(&self) -> Result<(), String> { Err("na mão não reinicia".into()) }
        fn manual(&self) -> bool { true }
    }

    #[tokio::test]
    async fn dev_mode_by_hand_switches_without_restarting() {
        let m = Machine::new();
        let outcome = switch(&m.home, &m.state_dir(), &Request::On(m.checkout.clone()), false, &Manual, &m.health(false, None), &fast()).await;
        assert_eq!(outcome, Ok(Outcome::SwitchedManual(real(&m.checkout))));
        assert_eq!(m.current(), real(&m.checkout));
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
