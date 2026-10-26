use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hangar_package::Manifest;

use super::*;
use crate::update::dev::tests::{behind_clone, sh};

const VERSION: &str = "0.1.0.5";

const UNIT: &str = "[Unit]\nDescription=hangar backend\n\n[Service]\nWorkingDirectory=CHECKOUT/backend\nEnvironment=MALLOC_ARENA_MAX=2\nExecStart=/usr/bin/uv run python -m app.main\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n";

/// Para e sobe anotando; `fail_start` faz a primeira subida falhar, `fail_restop` a segunda parada.
#[derive(Default)]
struct FakeService {
    calls: Mutex<Vec<&'static str>>,
    starts: Arc<AtomicUsize>,
    fail_start: bool,
    fail_restop: bool,
    /// Roda na primeira subida: o que o backend novo mexe antes de a prova falhar.
    on_start: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// Avisa a cada subida (o "sinal" chega durante a prova).
    started: tokio::sync::Notify,
}

impl Service for FakeService {
    fn stop(&self) -> Result<(), String> {
        let mut calls = self.calls.lock().unwrap();
        calls.push("stop");
        if self.fail_restop && calls.iter().filter(|c| **c == "stop").count() == 2 { return Err("systemctl stop saiu com 1".into()); }
        Ok(())
    }
    fn start(&self) -> Result<(), String> {
        self.calls.lock().unwrap().push("start");
        if let Some(touch) = self.on_start.lock().unwrap().take() { touch(); }
        let n = self.starts.fetch_add(1, Ordering::SeqCst) + 1;
        self.started.notify_one();
        if self.fail_start && n == 1 { return Err("systemctl start saiu com 1".into()); }
        Ok(())
    }
}

/// O pacote "baixado": uma pasta de versão com o manifesto, como o `prepare` deixaria.
#[derive(Default)]
struct FakeFetcher {
    calls: AtomicUsize,
    channels: Mutex<Vec<String>>,
}

impl Fetcher for FakeFetcher {
    fn fetch<'a>(&'a self, layout: &'a Layout, channel: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Fetched, String>> + Send + 'a>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.channels.lock().unwrap().push(channel.to_owned());
            let dir = layout.versions().join(VERSION);
            std::fs::create_dir_all(dir.join("backend/hooks")).unwrap();
            std::fs::create_dir_all(dir.join("scripts")).unwrap();
            let manifest = Manifest { version: VERSION.into(), commit: format!("commit-{VERSION}"), channel: "latest".into(), protocol: 7,
                api: 1, os: "test".into(), dist_key: None, files: BTreeMap::new() };
            std::fs::write(dir.join("manifest.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();
            Ok(Fetched::Version { dir, manifest, notes: Vec::new() })
        })
    }
}

/// Antes da primeira subida, o servidor de antes (pid 1); depois, o novo (pid 2) com o commit do
/// pacote. `proves = false`: o novo nunca responde.
struct FakeHealth {
    starts: Arc<AtomicUsize>,
    proves: bool,
}

impl HealthProbe for FakeHealth {
    async fn read(&self) -> Reading {
        // Cada subida é um processo novo.
        match self.starts.load(Ordering::SeqCst) {
            0 => Reading::Rust { protocol: 7, pid: Some(1), commit: None },
            n if self.proves => Reading::Rust { protocol: 7, pid: Some(n as u32 + 1), commit: Some(format!("commit-{VERSION}")) },
            _ => Reading::Down("connection refused".into()),
        }
    }
}

/// Uma máquina Linux em teste: home, checkout limpo na `main` com upstream, unit do backend, dados do
/// usuário, hooks do Kimi e do Codex apontando para o checkout e atalhos em `~/.local/bin`.
struct Box_ {
    _dir: tempfile::TempDir,
    home: PathBuf,
    checkout: PathBuf,
    launcher: PathBuf,
}

impl Box_ {
    fn new() -> Box_ {
        let dir = tempfile::tempdir().unwrap();
        let checkout = behind_clone(dir.path());
        let home = dir.path().join("home");
        let backend = checkout.join("backend");
        std::fs::create_dir_all(backend.join("launchers")).unwrap();
        std::fs::create_dir_all(checkout.join("scripts")).unwrap();
        std::fs::write(backend.join(".env"), "CP_AUTH_TOKEN=segredo\nCP_SERVER_ID=caixa\n").unwrap();
        std::fs::write(backend.join("peers.json"), "{\"peers\": []}\n").unwrap();
        std::fs::write(backend.join("launchers/x.sh"), "echo x\n").unwrap();
        std::fs::write(checkout.join("scripts/hangar-send"), "#!/bin/sh\n").unwrap();
        let unit_dir = home.join(".config/systemd/user");
        std::fs::create_dir_all(&unit_dir).unwrap();
        std::fs::write(unit_dir.join("hangar-backend.service"), UNIT.replace("CHECKOUT", &checkout.display().to_string())).unwrap();
        let hooks = backend.join("hooks").display().to_string();
        std::fs::create_dir_all(home.join(".kimi-code")).unwrap();
        std::fs::write(home.join(".kimi-code/config.toml"), format!(
            "default_model = \"k2\"\n\n[providers.x]\napi_key = \"chave\"\n\n# hangar: marcadores de estado + bilhete pane->sessao (app/kimi_hook_installer).\n\
             # Remova este bloco para desligar o rastreio de sessoes Kimi pelo app.\n[[hooks]]\nevent = \"Stop\"\ncommand = '\"/py\" \"{hooks}/kimi_state_hook.py\"'\ntimeout = 5\n\n\
             [[hooks]]\nevent = \"Stop\"\ncommand = 'alheio.sh'\ntimeout = 5\n")).unwrap();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::write(home.join(".codex/hooks.json"), serde_json::to_vec_pretty(&serde_json::json!({"hooks": {
            "Stop": [{"hooks": [{"type": "command", "command": format!("\"/py\" \"{hooks}/state_hook.py\" || exit 0")}]},
                     {"hooks": [{"type": "command", "command": "alheio.sh"}]}],
        }})).unwrap()).unwrap();
        std::fs::create_dir_all(home.join(".local/bin")).unwrap();
        std::os::unix::fs::symlink(checkout.join("scripts/hangar-send"), home.join(".local/bin/hangar-send")).unwrap();
        std::os::unix::fs::symlink("/usr/bin/true", home.join(".local/bin/alheio")).unwrap();
        std::fs::create_dir_all(home.join(".hangar")).unwrap();
        std::fs::write(home.join(".hangar/backend-dir"), format!("{}\n", backend.display())).unwrap();
        std::fs::write(home.join(".hangar/plugin-dir"), format!("{}/plugins/hangar\n", checkout.display())).unwrap();
        // Como no repositório de verdade, os dados do usuário não aparecem no `git status`.
        std::fs::write(checkout.join(".git/info/exclude"), "backend/\nscripts/\n").unwrap();
        let launcher = dir.path().join("hangar-server-novo");
        std::fs::write(&launcher, "binário").unwrap();
        Box_ { _dir: dir, home, checkout, launcher }
    }

    fn machine<'a>(&self, service: &'a FakeService, fetcher: &'a FakeFetcher) -> Machine<'a> {
        Machine {
            home: self.home.clone(), checkout: self.checkout.clone(), windows: false,
            systemd_dir: self.home.join(".config/systemd/user"), tasks_dir: None,
            kimi_home: self.home.join(".kimi-code"), codex_homes: vec![self.home.join(".codex")],
            launcher_source: self.launcher.clone(), in_service_group: false, state_dir: self.state_dir(), service, fetcher,
            settings: Settings { download_deadline: Duration::from_secs(5), health_deadline: Duration::from_millis(300), health_interval: Duration::from_millis(10) },
        }
    }

    fn unit(&self) -> String { std::fs::read_to_string(self.home.join(".config/systemd/user/hangar-backend.service")).unwrap() }
    fn current(&self) -> PathBuf { self.home.join(".hangar/current") }
    fn state_dir(&self) -> PathBuf { self.home.join(".claude/.hangar-update") }
    fn step_applied(&self) -> bool { crate::update::steps::applied(&self.state_dir()).unwrap().contains(STEP_ID) }
    fn marker(&self) -> Option<serde_json::Value> {
        std::fs::read(self.home.join(".hangar/migration.json")).ok().map(|b| serde_json::from_slice(&b).unwrap())
    }
}

fn health(service: &FakeService, proves: bool) -> FakeHealth { FakeHealth { starts: service.starts.clone(), proves } }

/// Toda a árvore: caminho → alvo do link ou conteúdo.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            let value = if kind.is_symlink() { format!("-> {}", std::fs::read_link(&path).unwrap().display()) }
                else if kind.is_dir() { stack.push(path.clone()); "dir".into() }
                else { String::from_utf8_lossy(&std::fs::read(&path).unwrap()).into_owned() };
            out.insert(path, value);
        }
    }
    out
}

#[test]
fn migrate_step_id_is_the_step_file() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/atualizacoes").join(format!("{STEP_ID}.md"));
    let (fields, _) = crate::update::steps::frontmatter(&std::fs::read_to_string(&file).unwrap());
    assert_eq!(fields.get("id").map(String::as_str), Some(STEP_ID));
}

#[test]
fn migrate_decides_developer_by_edits_ahead_branch_and_preview() {
    let clean = Facts { dirty: 0, untracked: 0, ahead: 0, branch: "main".into(), upstream: true, preview: false, dev_requested: false };
    assert_eq!(decide(&clean).0, Mode::Package);
    assert_eq!(decide(&Facts { branch: "beta".into(), ..clean.clone() }), (Mode::Package, None), "beta é o canal de teste");
    for facts in [Facts { dirty: 2, ..clean.clone() }, Facts { untracked: 1, ..clean.clone() },Facts { ahead: 1, ..clean.clone() }, Facts { branch: "PM-1".into(), ..clean.clone() },
                  Facts { preview: true, ..clean.clone() }, Facts { upstream: false, ..clean.clone() }, Facts { dev_requested: true, ..clean.clone() }] {
        let (mode, why) = decide(&facts);
        assert_eq!(mode, Mode::Developer, "{facts:?}");
        assert!(why.is_some(), "{facts:?}");
    }
}

#[tokio::test]
async fn migrate_clean_checkout_becomes_package() {
    let b = Box_::new();
    let (service, fetcher) = (FakeService::default(), FakeFetcher::default());
    let outcome = migrate(&b.machine(&service, &fetcher), &health(&service, true), false).await.unwrap();
    assert_eq!(outcome, Outcome::Migrated(Mode::Package));
    assert!(b.step_applied(), "o passo que roda a migração fica registrado: a tela não o anuncia de novo");
    assert_eq!(fetcher.calls.load(Ordering::SeqCst), 1);
    assert_eq!(*fetcher.channels.lock().unwrap(), ["latest"]);
    assert_eq!(*service.calls.lock().unwrap(), ["stop", "start"]);

    // Dados do usuário no lugar novo, com um link no antigo.
    let config = b.home.join(".hangar/config");
    assert_eq!(std::fs::read_to_string(config.join(".env")).unwrap(), "CP_AUTH_TOKEN=segredo\nCP_SERVER_ID=caixa\n");
    assert_eq!(std::fs::read_link(b.checkout.join("backend/.env")).unwrap(), config.join(".env"));
    assert_eq!(std::fs::read_link(b.checkout.join("backend/peers.json")).unwrap(), config.join("peers.json"));
    assert_eq!(std::fs::read_to_string(b.checkout.join("backend/launchers/x.sh")).unwrap(), "echo x\n", "pelo link");
    assert_eq!(std::fs::read_to_string(config.join("launchers/x.sh")).unwrap(), "echo x\n");
    assert!(!config.join("projects.json").exists(), "o que não existia não nasce");

    assert_eq!(std::fs::canonicalize(b.current()).unwrap(), std::fs::canonicalize(b.home.join(".hangar/versions").join(VERSION)).unwrap());
    let launcher = b.home.join(".hangar/bin/hangar-launcher");
    assert_eq!(std::fs::read_to_string(&launcher).unwrap(), "binário");
    let unit = b.unit();
    assert!(unit.contains(&format!("ExecStart=\"{}\" launch backend\n", launcher.display())), "{unit}");
    assert!(!unit.contains("uv run"), "{unit}");
    assert!(unit.contains("MALLOC_ARENA_MAX=2"), "o resto da unit fica");

    // Uso avulso pelo `current`, nunca pelo checkout.
    assert_eq!(std::fs::read_link(b.home.join(".local/bin/hangar-send")).unwrap(), b.current().join("scripts/hangar-send"));
    assert_eq!(std::fs::read_link(b.home.join(".local/bin/alheio")).unwrap(), Path::new("/usr/bin/true"));
    assert_eq!(std::fs::read_to_string(b.home.join(".hangar/backend-dir")).unwrap(), format!("{}\n", b.current().join("backend").display()));
    assert_eq!(std::fs::read_to_string(b.home.join(".hangar/plugin-dir")).unwrap(), format!("{}\n", b.current().join("plugins/hangar").display()));

    let marker = b.marker().expect("marca");
    assert_eq!(marker["mode"], "package");
    assert_eq!(marker["version"], VERSION);
    assert_eq!(marker["checkout"], b.checkout.display().to_string());
    assert!(marker["date"].as_str().is_some_and(|d| !d.is_empty()));
    assert!(!b.home.join(".hangar/migration.partial.json").exists());

    // De novo: já migrada, nada muda.
    let before = snapshot(&b.home);
    assert_eq!(migrate(&b.machine(&service, &fetcher), &health(&service, true), false).await.unwrap(), Outcome::AlreadyMigrated(Mode::Package));
    assert_eq!(snapshot(&b.home), before);

    // Registro do passo perdido: a próxima rodada o refaz; ilegível, a rodada falha e não o apaga.
    std::fs::remove_file(b.state_dir().join("aplicados.json")).unwrap();
    assert_eq!(migrate(&b.machine(&service, &fetcher), &health(&service, true), false).await.unwrap(), Outcome::AlreadyMigrated(Mode::Package));
    assert!(b.step_applied());
    std::fs::write(b.state_dir().join("aplicados.json"), "{").unwrap();
    let error = migrate(&b.machine(&service, &fetcher), &health(&service, true), false).await.unwrap_err();
    assert!(error.contains("aplicados.json"), "{error}");
    assert_eq!(std::fs::read_to_string(b.state_dir().join("aplicados.json")).unwrap(), "{");
}

#[tokio::test]
async fn migrate_drops_old_kimi_and_codex_hook_entries() {
    let b = Box_::new();
    let (service, fetcher) = (FakeService::default(), FakeFetcher::default());
    migrate(&b.machine(&service, &fetcher), &health(&service, true), false).await.unwrap();
    let kimi = std::fs::read_to_string(b.home.join(".kimi-code/config.toml")).unwrap();
    assert!(!kimi.contains("kimi_state_hook.py"), "{kimi}");
    assert!(kimi.contains("alheio.sh") && kimi.contains("api_key = \"chave\""), "o resto fica: {kimi}");
    assert!(toml::from_str::<toml::Table>(&kimi).is_ok(), "continua TOML válido: {kimi}");
    let codex: serde_json::Value = serde_json::from_slice(&std::fs::read(b.home.join(".codex/hooks.json")).unwrap()).unwrap();
    assert_eq!(codex["hooks"]["Stop"], serde_json::json!([{"hooks": [{"type": "command", "command": "alheio.sh"}]}]));
}

#[tokio::test]
async fn migrate_clean_beta_checkout_becomes_package_of_the_beta_channel() {
    let b = Box_::new();
    sh(&b.checkout, "git checkout -q -b beta && git branch -q -u origin/main");
    let (service, fetcher) = (FakeService::default(), FakeFetcher::default());
    let machine = b.machine(&service, &fetcher);
    let Outcome::DryRun(lines) = migrate(&machine, &health(&service, true), true).await.unwrap() else { panic!("dry-run") };
    assert!(lines[0].contains("canal beta"), "{lines:?}");
    let outcome = migrate(&machine, &health(&service, true), false).await.unwrap();
    assert_eq!(outcome, Outcome::Migrated(Mode::Package));
    assert_eq!(*fetcher.channels.lock().unwrap(), ["beta"]);
}

#[tokio::test]
async fn migrate_dirty_branch_or_preview_checkout_is_developer_mode_without_download() {
    for case in ["dirty", "untracked", "branch", "preview", "dev-requested"] {
        let b = Box_::new();
        match case {
            "dirty" => sh(&b.checkout, "echo mudou > a"),
            "untracked" => sh(&b.checkout, "echo novo > novo.txt"),
            "branch" => sh(&b.checkout, "git checkout -q -b PM-1"),
            "preview" => std::fs::write(b.home.join(".config/systemd/user/hangar-frontend.service"), "[Service]\n").unwrap(),
            _ => std::fs::write(b.home.join(".hangar").join(DEV_REQUESTED), "").unwrap(),
        }
        let (service, fetcher) = (FakeService::default(), FakeFetcher::default());
        let outcome = migrate(&b.machine(&service, &fetcher), &health(&service, true), false).await.unwrap();
        assert_eq!(outcome, Outcome::Migrated(Mode::Developer), "{case}");
        assert!(b.step_applied(), "{case}");
        assert_eq!(fetcher.calls.load(Ordering::SeqCst), 0, "{case}: nada baixado");
        assert!(!b.home.join(".hangar/versions").exists(), "{case}");
        assert_eq!(std::fs::canonicalize(b.current()).unwrap(), std::fs::canonicalize(&b.checkout).unwrap(), "{case}");
        assert!(b.unit().contains("launch backend"), "{case}");
        assert_eq!(b.marker().unwrap()["mode"], "developer", "{case}");
        assert!(!b.home.join(".hangar").join(DEV_REQUESTED).exists(), "{case}: a marca do --dev some depois de usada");
    }
}

#[tokio::test]
async fn migrate_failure_after_data_copy_and_before_start_undoes_everything() {
    let b = Box_::new();
    let before = (snapshot(&b.home), snapshot(&b.checkout.join("backend")));
    // O lançador some depois da conferência: falha logo depois de copiar os dados.
    let (service, fetcher) = (FakeService::default(), FakeFetcher::default());
    let mut machine = b.machine(&service, &fetcher);
    machine.launcher_source = b.home.join("nao-existe");
    let error = migrate(&machine, &health(&service, true), false).await.unwrap_err();
    assert!(error.contains("nao-existe"), "{error}");
    assert_eq!(*service.calls.lock().unwrap(), ["stop", "start"], "o backend de antes volta");
    let after = (snapshot(&b.home), snapshot(&b.checkout.join("backend")));
    // O pacote baixado fica para a próxima tentativa; o resto volta como estava.
    let mut home_after = after.0.clone();
    home_after.retain(|path, _| !path.starts_with(b.home.join(".hangar/versions")) && path != &b.home.join(".hangar/update.lock")
        && path != &b.home.join(".hangar/downloads"));
    assert_eq!(home_after, before.0);
    assert_eq!(after.1, before.1);
}

#[tokio::test]
async fn migrate_server_that_does_not_come_back_undoes_everything() {
    let b = Box_::new();
    let unit = b.unit();
    let (service, fetcher) = (FakeService::default(), FakeFetcher::default());
    let error = migrate(&b.machine(&service, &fetcher), &health(&service, false), false).await.unwrap_err();
    assert!(error.contains("não voltou"), "{error}");
    assert_eq!(*service.calls.lock().unwrap(), ["stop", "start", "stop", "start"]);
    assert_eq!(b.unit(), unit);
    assert!(std::fs::symlink_metadata(b.current()).is_err());
    assert_eq!(std::fs::read_to_string(b.checkout.join("backend/.env")).unwrap(), "CP_AUTH_TOKEN=segredo\nCP_SERVER_ID=caixa\n");
    assert!(!b.checkout.join("backend/.env").is_symlink());
    assert!(!b.home.join(".hangar/config").join(".env").exists());
    assert!(b.marker().is_none() && !b.home.join(".hangar/migration.partial.json").exists());
}

#[tokio::test]
async fn migrate_undo_points_everything_back_to_the_checkout() {
    let b = Box_::new();
    let (unit, kimi) = (b.unit(), std::fs::read_to_string(b.home.join(".kimi-code/config.toml")).unwrap());
    let (service, fetcher) = (FakeService::default(), FakeFetcher::default());
    migrate(&b.machine(&service, &fetcher), &health(&service, true), false).await.unwrap();
    // Depois da migração o backend novo grava hooks e dados pelo `current`.
    std::fs::write(b.home.join(".hangar/config/.env"), "CP_AUTH_TOKEN=segredo\nCP_SERVER_ID=caixa\nCP_NOVO=1\n").unwrap();
    let current_hook = format!("\n[[hooks]]\nevent = \"Stop\"\ncommand = '\"/py\" \"{}/backend/hooks/kimi_state_hook.py\"'\ntimeout = 5\n", b.current().display());
    std::fs::write(b.home.join(".kimi-code/config.toml"), std::fs::read_to_string(b.home.join(".kimi-code/config.toml")).unwrap() + &current_hook).unwrap();

    let service = FakeService::default();
    undo(&b.machine(&service, &fetcher), &health(&service, true)).await.unwrap();
    assert_eq!(*service.calls.lock().unwrap(), ["stop", "start"]);
    assert_eq!(b.unit(), unit);
    assert!(std::fs::symlink_metadata(b.current()).is_err());
    assert!(!b.checkout.join("backend/.env").is_symlink());
    assert_eq!(std::fs::read_to_string(b.checkout.join("backend/.env")).unwrap(), "CP_AUTH_TOKEN=segredo\nCP_SERVER_ID=caixa\nCP_NOVO=1\n", "o dado mais novo volta");
    assert!(!b.home.join(".hangar/config/.env").exists());
    assert!(b.checkout.join("backend/launchers").is_dir() && !b.checkout.join("backend/launchers").is_symlink());
    assert_eq!(std::fs::read_link(b.home.join(".local/bin/hangar-send")).unwrap(), b.checkout.join("scripts/hangar-send"));
    assert_eq!(std::fs::read_to_string(b.home.join(".hangar/backend-dir")).unwrap(), format!("{}\n", b.checkout.join("backend").display()));
    let kimi_after = std::fs::read_to_string(b.home.join(".kimi-code/config.toml")).unwrap();
    assert!(!kimi_after.contains(&b.current().display().to_string()), "{kimi_after}");
    assert!(kimi_after.contains(&format!("{}/backend/hooks/kimi_state_hook.py", b.checkout.display())), "{kimi_after}");
    assert!(toml::from_str::<toml::Table>(&kimi_after).is_ok());
    assert_eq!(kimi_after.matches("[[hooks]]").count(), kimi.matches("[[hooks]]").count());
    assert!(!b.home.join(".hangar/bin/hangar-launcher").exists());
    assert!(b.marker().is_none());
}

#[tokio::test]
async fn migrate_rollback_leaves_everything_when_the_new_backend_does_not_stop() {
    let b = Box_::new();
    let service = FakeService { fail_restop: true, ..FakeService::default() };
    let fetcher = FakeFetcher::default();
    let error = migrate(&b.machine(&service, &fetcher), &health(&service, false), false).await.unwrap_err();
    assert!(error.contains("não parou") && error.contains("migration.partial.json"), "{error}");
    assert_eq!(*service.calls.lock().unwrap(), ["stop", "start", "stop"], "nada sobe por cima do backend que não parou");
    assert!(std::fs::symlink_metadata(b.current()).is_ok(), "a volta não mexeu em nada");
    assert!(b.checkout.join("backend/.env").is_symlink() && b.home.join(".hangar/config/.env").exists());
    assert!(b.home.join(".hangar/migration.partial.json").exists());
}

#[tokio::test]
async fn migrate_rollback_failures_keep_the_record_and_leave_unreadable_files_alone() {
    let b = Box_::new();
    let (home, codex, kimi) = (b.home.clone(), b.home.join(".codex/hooks.json"), b.home.join(".kimi-code/config.toml"));
    let service = FakeService::default();
    // O backend novo mexe antes de a prova falhar: arquivo a mais nos dados, hooks.json torto, config do Kimi sem UTF-8.
    *service.on_start.lock().unwrap() = Some(Box::new(move || {
        std::fs::write(home.join(".hangar/config/novo.txt"), "x").unwrap();
        std::fs::write(&codex, "{ torto").unwrap();
        std::fs::write(&kimi, b"\xff\xfe[[hooks]]").unwrap();
    }));
    let fetcher = FakeFetcher::default();
    let error = migrate(&b.machine(&service, &fetcher), &health(&service, false), false).await.unwrap_err();
    for what in ["hooks.json", "config.toml", ".hangar/config"] { assert!(error.contains(what), "{what}: {error}"); }
    assert!(b.home.join(".hangar/migration.partial.json").exists(), "o registro fica para conferir à mão");
    assert_eq!(std::fs::read(b.home.join(".codex/hooks.json")).unwrap(), b"{ torto", "ilegível não é sobrescrito");
    assert_eq!(std::fs::read(b.home.join(".kimi-code/config.toml")).unwrap(), b"\xff\xfe[[hooks]]");
}

#[tokio::test]
async fn migrate_notes_hook_files_it_could_not_read() {
    let b = Box_::new();
    std::fs::write(b.home.join(".codex/hooks.json"), "{ torto").unwrap();
    std::fs::write(b.home.join(".kimi-code/config.toml"), b"\xff\xfe").unwrap();
    let (service, fetcher) = (FakeService::default(), FakeFetcher::default());
    assert_eq!(migrate(&b.machine(&service, &fetcher), &health(&service, true), false).await.unwrap(), Outcome::Migrated(Mode::Package));
    let notes = b.marker().unwrap()["notes"].to_string();
    assert!(notes.contains("hooks.json") && notes.contains("config.toml"), "{notes}");
}

#[tokio::test]
async fn migrate_dry_run_changes_nothing() {
    let b = Box_::new();
    // O `.git` fica fora: o `git status` da decisão pode só renovar o índice.
    let tree = |b: &Box_| { let mut t = snapshot(&b.checkout); t.retain(|p, _| !p.starts_with(b.checkout.join(".git"))); (snapshot(&b.home), t) };
    let before = tree(&b);
    let (service, fetcher) = (FakeService::default(), FakeFetcher::default());
    let Outcome::DryRun(plan) = migrate(&b.machine(&service, &fetcher), &health(&service, true), true).await.unwrap() else { panic!("sem plano") };
    assert!(plan.iter().any(|l| l.contains("pacote")), "{plan:?}");
    assert!(plan.iter().any(|l| l.contains(".env")), "{plan:?}");
    assert!(service.calls.lock().unwrap().is_empty());
    assert_eq!(fetcher.calls.load(Ordering::SeqCst), 0);
    assert_eq!(tree(&b), before);
}

#[tokio::test]
async fn migrate_without_a_backend_service_is_not_migrated() {
    let b = Box_::new();
    std::fs::remove_file(b.home.join(".config/systemd/user/hangar-backend.service")).unwrap();
    let (service, fetcher) = (FakeService::default(), FakeFetcher::default());
    let outcome = migrate(&b.machine(&service, &fetcher), &health(&service, true), false).await.unwrap();
    assert!(matches!(outcome, Outcome::NotMigrated(_)), "{outcome:?}");
    assert!(service.calls.lock().unwrap().is_empty());
    assert_eq!(b.marker().unwrap()["mode"], "not_migrated");
    assert!(std::fs::symlink_metadata(b.current()).is_err());
}

/// A árvore que a volta tem que devolver: a home sem o que fica de propósito (pacote baixado, trava)
/// e a `backend/` do checkout.
fn restorable(b: &Box_) -> (BTreeMap<PathBuf, String>, BTreeMap<PathBuf, String>) {
    let mut home = snapshot(&b.home);
    home.retain(|path, _| !path.starts_with(b.home.join(".hangar/versions")) && path != &b.home.join(".hangar/update.lock"));
    (home, snapshot(&b.checkout.join("backend")))
}

#[tokio::test]
async fn migrate_refuses_inside_the_backend_service_before_stopping_it() {
    let b = Box_::new();
    let before = restorable(&b);
    let (service, fetcher) = (FakeService::default(), FakeFetcher::default());
    let mut machine = b.machine(&service, &fetcher);
    machine.in_service_group = true;
    let error = migrate(&machine, &health(&service, true), false).await.unwrap_err();
    assert!(error.contains("serviço do backend"), "{error}");
    assert!(service.calls.lock().unwrap().is_empty());
    assert_eq!(fetcher.calls.load(Ordering::SeqCst), 0);
    assert_eq!(restorable(&b), before);
}

#[tokio::test]
async fn migrate_after_a_killed_run_undoes_it_and_migrates_again() {
    let b = Box_::new();
    let (service, fetcher) = (FakeService::default(), FakeFetcher::default());
    // Morto (SIGKILL) esperando a prova: o futuro some sem volta nenhuma.
    let (machine, probe) = (b.machine(&service, &fetcher), health(&service, false));
    tokio::select! {
        _ = migrate(&machine, &probe, false) => panic!("a prova não devia passar"),
        () = service.started.notified() => {}
    }
    assert!(b.home.join(".hangar/migration.partial.json").exists() && std::fs::symlink_metadata(b.current()).is_ok());

    let service = FakeService::default();
    let outcome = migrate(&b.machine(&service, &fetcher), &health(&service, true), false).await.unwrap();
    assert_eq!(outcome, Outcome::Migrated(Mode::Package));
    assert_eq!(*service.calls.lock().unwrap(), ["stop", "start", "stop", "start"], "desfaz a anterior e migra de novo");
    assert!(!b.home.join(".hangar/migration.partial.json").exists());
    assert_eq!(b.marker().unwrap()["mode"], "package");
    assert_eq!(std::fs::read_to_string(b.home.join(".hangar/config/.env")).unwrap(), "CP_AUTH_TOKEN=segredo\nCP_SERVER_ID=caixa\n");
    assert_eq!(std::fs::read_link(b.checkout.join("backend/.env")).unwrap(), b.home.join(".hangar/config/.env"));
    let kimi = std::fs::read_to_string(b.home.join(".kimi-code/config.toml")).unwrap();
    assert_eq!(kimi.matches("[[hooks]]").count(), 1, "o hook do checkout sai uma vez só: {kimi}");
}

#[tokio::test]
async fn migrate_interrupted_while_proving_rolls_back_to_the_checkout() {
    let b = Box_::new();
    let before = restorable(&b);
    let (service, fetcher) = (FakeService::default(), FakeFetcher::default());
    // O "sinal" chega quando o backend novo sobe, durante a espera da prova.
    let error = migrate_until(&b.machine(&service, &fetcher), &health(&service, false), false, service.started.notified()).await.unwrap_err();
    assert!(error.contains("interrompida") && error.contains("desfeita"), "{error}");
    assert_eq!(*service.calls.lock().unwrap(), ["stop", "start", "stop", "start"]);
    // Os hooks do Kimi e do Codex voltam, no fim da lista (a volta deles não é byte a byte).
    let hooks = [b.home.join(".kimi-code/config.toml"), b.home.join(".codex/hooks.json")];
    let without_hooks = |mut tree: (BTreeMap<PathBuf, String>, BTreeMap<PathBuf, String>)| { tree.0.retain(|p, _| !hooks.contains(p)); tree };
    assert_eq!(without_hooks(restorable(&b)), without_hooks(before));
    let checkout_hooks = b.checkout.join("backend/hooks").display().to_string();
    for file in &hooks {
        let text = std::fs::read_to_string(file).unwrap();
        assert!(text.contains(&checkout_hooks) && text.contains("alheio.sh"), "{text}");
    }
    assert!(b.marker().is_none());
}

#[tokio::test]
async fn migrate_partial_copy_never_becomes_the_config_entry() {
    use std::os::unix::fs::PermissionsExt;
    let b = Box_::new();
    // Um arquivo ilegível no meio da pasta: a cópia dela falha depois de começar.
    let blocked = b.checkout.join("backend/launchers/y.sh");
    std::fs::write(&blocked, "segredo").unwrap();
    let before = restorable(&b);
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(&blocked).is_ok() { return; } // root lê tudo: o caso não se monta
    let (service, fetcher) = (FakeService::default(), FakeFetcher::default());
    let error = migrate(&b.machine(&service, &fetcher), &health(&service, true), false).await;
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o644)).unwrap();
    let error = error.unwrap_err();
    assert!(error.contains("launchers"), "{error}");
    let config = b.home.join(".hangar/config");
    assert!(!config.join("launchers").exists(), "cópia parcial com o nome final");
    assert!(std::fs::read_dir(&config).map_or(true, |mut d| d.next().is_none()), "sobrou temporário em {}", config.display());
    assert_eq!(restorable(&b), before);
}

#[tokio::test]
async fn migrate_reads_the_not_migrated_mark_written_by_the_step_script() {
    let b = Box_::new();
    // A linha que scripts/migrar-pacote.sh grava quando não há binário para o sistema.
    std::fs::write(b.home.join(".hangar/migration.json"), format!(
        "{{\"mode\":\"not_migrated\",\"date\":\"2026-10-24T10:00:00-0300\",\"checkout\":\"{}\",\"version\":null,\"reason\":\"sem binário\",\"notes\":[],\"done\":[]}}\n",
        b.checkout.display())).unwrap();
    let (service, fetcher) = (FakeService::default(), FakeFetcher::default());
    let outcome = migrate(&b.machine(&service, &fetcher), &health(&service, true), false).await.unwrap();
    assert_eq!(outcome, Outcome::AlreadyMigrated(Mode::NotMigrated));
    assert!(service.calls.lock().unwrap().is_empty());
}

#[test]
fn migrate_windows_task_runs_the_launcher_resolved_and_quoted() {
    let old = "WScript.Quit CreateObject(\"WScript.Shell\").Run(\"\"\"C:\\h\\backend\\.venv\\Scripts\\python.exe\"\" -m app.main --log \"\"C:\\Users\\Zé\\AppData\\Local\\hangar\\logs\\privado\\hangar-backend.log\"\"\", 0, True)\r\n";
    let launcher = Path::new(r"C:\Users\Zé Silva\.hangar\bin\hangar-launcher.exe");
    let new = vbs_with_launcher(old, launcher, Path::new(r"C:\x.log"));
    assert_eq!(new, "WScript.Quit CreateObject(\"WScript.Shell\").Run(\"\"\"C:\\Users\\Zé Silva\\.hangar\\bin\\hangar-launcher.exe\"\" launch backend --log \"\"C:\\Users\\Zé\\AppData\\Local\\hangar\\logs\\privado\\hangar-backend.log\"\"\", 0, True)\r\n");
    let bytes = encode_vbs(&new);
    assert_eq!(&bytes[..2], &[0xFF, 0xFE], "UTF-16LE com BOM, como o instalador");
    assert_eq!(decode_vbs(&bytes), new);
}

#[test]
fn migrate_windows_shortcuts_go_through_the_current_of_the_profile() {
    let cmd = "@echo off\r\ncall \"%~dp0hangar-python.cmd\" \"C:\\Users\\ze\\hangar\\scripts\\hangar-conta\" %*\r\n";
    let new = retarget(cmd, r"C:\Users\ze\hangar\scripts\", r"%USERPROFILE%\.hangar\current\scripts\", true).unwrap();
    assert_eq!(new, "@echo off\r\ncall \"%~dp0hangar-python.cmd\" \"%USERPROFILE%\\.hangar\\current\\scripts\\hangar-conta\" %*\r\n");
    assert_eq!(retarget(cmd, r"c:\users\ZE\hangar\scripts\", "X\\", true).as_deref(), Some("@echo off\r\ncall \"%~dp0hangar-python.cmd\" \"X\\hangar-conta\" %*\r\n"),
        "o Windows não diferencia caixa");
    assert_eq!(retarget(cmd, r"D:\outro\scripts\", "X", true), None);
}
