use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hangar_package::{Manifest, PackageIndex, PlatformEntry, Version};
use serde_json::Value;

use super::*;

const PLATFORM: &str = "test-os";
const OLD: &str = "0.1.0.10";
const NEW: &str = "0.1.0.11";
const PROTOCOL: u32 = 89;

fn sha(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// `.tar.gz` com `manifest.json` e os arquivos dados; devolve os bytes.
fn package(version: &str, commit: &str, files: &[(&str, &str)]) -> Vec<u8> { package_keyed(version, commit, None, files) }

fn package_keyed(version: &str, commit: &str, dist_key: Option<&str>, files: &[(&str, &str)]) -> Vec<u8> {
    let manifest = Manifest {
        version: version.into(), commit: commit.into(), channel: "latest".into(), protocol: PROTOCOL, api: 1,
        os: PLATFORM.into(), dist_key: dist_key.map(str::to_owned),
        files: files.iter().map(|(p, c)| (p.to_string(), sha(c.as_bytes()))).collect(),
    };
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
    let manifest = serde_json::to_vec(&manifest).unwrap();
    for (path, content) in std::iter::once(("manifest.json", manifest.as_slice())).chain(files.iter().map(|(p, c)| (*p, c.as_bytes()))) {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, path, content).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

/// Release falsa servida por um servidor HTTP em processo, a partir de uma pasta.
struct FakeRelease {
    root: tempfile::TempDir,
    base: String,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for FakeRelease {
    fn drop(&mut self) { self.server.abort(); }
}

async fn serve(root: PathBuf) -> (String, tokio::task::JoinHandle<()>) {
    use axum::extract::{Path as UrlPath, State};
    use axum::http::StatusCode;
    let app = axum::Router::new().route("/{*path}", axum::routing::get(|State(root): State<PathBuf>, UrlPath(path): UrlPath<String>| async move {
        match std::fs::read(root.join(path)) {
            Ok(bytes) => Ok(bytes),
            Err(_) => Err(StatusCode::NOT_FOUND),
        }
    })).with_state(root);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    (base, tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); }))
}

impl FakeRelease {
    async fn new() -> FakeRelease {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("package-latest")).unwrap();
        let (base, server) = serve(root.path().to_path_buf()).await;
        FakeRelease { root, base, server }
    }

    fn release(&self) -> Release { Release { base_url: self.base.clone(), channel: "latest".into() } }

    /// Publica o pacote e a entrada do índice; `sha` troca o sha256 anunciado.
    fn publish(&self, version: &str, bytes: &[u8], sha_override: Option<&str>) -> PlatformEntry {
        let asset = format!("hangar-{version}-{PLATFORM}.tar.gz");
        std::fs::write(self.root.path().join("package-latest").join(&asset), bytes).unwrap();
        let entry = PlatformEntry { version: version.into(), asset, sha256: sha_override.map_or_else(|| sha(bytes), str::to_owned),
            protocol: PROTOCOL, api: 1, commit: format!("commit-{version}"), dist_key: None, notes: None };
        let index = PackageIndex { platforms: BTreeMap::from([(PLATFORM.to_owned(), entry.clone())]) };
        std::fs::write(self.root.path().join("package-latest/package-latest.json"), serde_json::to_vec(&index).unwrap()).unwrap();
        entry
    }
}

/// Uma pasta de versão já extraída, só com o manifesto.
fn version_dir(layout: &Layout, version: &str) -> PathBuf {
    let dir = layout.versions().join(version);
    std::fs::create_dir_all(&dir).unwrap();
    let manifest = Manifest { version: version.into(), commit: format!("commit-{version}"), channel: "latest".into(), protocol: PROTOCOL,
        api: 1, os: PLATFORM.into(), dist_key: None, files: BTreeMap::new() };
    std::fs::write(dir.join("manifest.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();
    dir
}

fn real(path: &Path) -> PathBuf { crate::list::links::without_verbatim(std::fs::canonicalize(path).unwrap()) }

#[derive(Default)]
struct FakeSteps {
    ran: Mutex<Vec<String>>,
}

/// `touch <caminho>` cria o arquivo; `fail` sai com 1 e a marca do instalador.
impl StepRunner for FakeSteps {
    fn run(&self, command: &str, _cwd: &Path) -> StepRun {
        self.ran.lock().unwrap().push(command.to_owned());
        if let Some(path) = command.strip_prefix("touch ") {
            std::fs::write(path, "").unwrap();
            return StepRun { code: Some(0), output: String::new() };
        }
        StepRun { code: Some(1), output: "linha qualquer\n##HANGAR-FALHA## o instalador quebrou\nfim".into() }
    }
}

#[derive(Clone, Default)]
struct FakeRestart {
    count: Arc<AtomicUsize>,
    /// O primeiro reinício falha.
    fail_first: bool,
    /// Instalação na mão: ninguém reinicia.
    manual: bool,
}

impl Restarter for FakeRestart {
    async fn restart(&self) -> Result<(), String> {
        let n = self.count.fetch_add(1, Ordering::SeqCst) + 1;
        if self.fail_first && n == 1 { return Err("systemctl saiu com 1".into()); }
        Ok(())
    }
    fn manual(&self) -> bool { self.manual }
}

/// A leitura depende de quantos reinícios já houve.
struct FakeHealth {
    restarts: Arc<AtomicUsize>,
    reading: Box<dyn Fn(usize) -> Reading + Send + Sync>,
}

impl HealthProbe for FakeHealth {
    async fn read(&self) -> Reading { (self.reading)(self.restarts.load(Ordering::SeqCst)) }
}

fn rust(pid: u32, version: &str) -> Reading {
    Reading::Rust { protocol: PROTOCOL, pid: Some(pid), commit: Some(format!("commit-{version}")) }
}

/// Antes do reinício e depois da volta, o servidor de antes; depois do 1º reinício, o da versão nova.
fn served(restarts: usize) -> Reading { rust(100 + restarts as u32, if restarts == 1 { NEW } else { OLD }) }

struct NeverInUse;
impl InUse for NeverInUse {
    fn in_use(&self, _dir: &Path) -> bool { false }
}

struct Machine {
    home: tempfile::TempDir,
    layout: Layout,
    state_dir: PathBuf,
    client: reqwest::Client,
}

impl Machine {
    /// `~/.hangar/current` apontando para a versão antiga, mais uma versão ainda mais velha.
    fn new() -> Machine {
        let home = tempfile::tempdir().unwrap();
        let layout = Layout::new(home.path());
        version_dir(&layout, "0.1.0.9");
        let old = version_dir(&layout, OLD);
        swap(&layout, &old).unwrap();
        let state_dir = state_dir(&home.path().join(".claude"));
        Machine { home, layout, state_dir, client: crate::http_client().build().unwrap() }
    }

    fn job<'a>(&'a self, release: &'a Release) -> Job<'a> {
        Job { client: &self.client, release, platform: PLATFORM, layout: &self.layout, home: self.home.path(), state_dir: &self.state_dir, diary_dir: None, ui: None, notes: vec![],
            settings: Settings { download_deadline: Duration::from_secs(30), health_deadline: Duration::from_millis(200), health_interval: Duration::from_millis(5) } }
    }

    fn current(&self) -> PathBuf { real(&self.layout.current()) }
    fn state(&self) -> serde_json::Map<String, Value> { StateFile::new(&self.state_dir).read() }
    fn applied(&self) -> Vec<String> {
        std::fs::read(self.state_dir.join("aplicados.json")).ok().map(|b| serde_json::from_slice(&b).unwrap()).unwrap_or_default()
    }
}

fn step_doc(id: &str, tipo: &str, command: &str, proof: &str) -> String {
    format!("---\nid: {id}\ntitulo: Passo {id}\ncomando: {command}\nprova: {proof}\ndestrutivo: false\ntipo: {tipo}\n---\n\nTexto.\n")
}

/// Pacote novo com um passo de máquina (`command`) e um de conteúdo, que nunca roda no pacote.
fn new_package(machine: &Machine, command: &str) -> (Vec<u8>, PathBuf) {
    let proof = machine.home.path().join("prova-do-passo");
    let command = command.replace("{proof}", &proof.display().to_string());
    let content = step_doc("2026-10-02-conteudo", "conteudo", &format!("touch {}", machine.home.path().join("nunca").display()),
        &machine.home.path().join("nunca").display().to_string());
    let maquina = step_doc("2026-10-01-maquina", "maquina", &command, &proof.display().to_string());
    let bytes = package(NEW, &format!("commit-{NEW}"), &[("VERSION", "0.1.0\n"),
        ("docs/atualizacoes/2026-10-01-maquina.md", &maquina), ("docs/atualizacoes/2026-10-02-conteudo.md", &content)]);
    (bytes, proof)
}

#[tokio::test]
async fn update_check_returns_the_entry_only_when_newer() {
    let fake = FakeRelease::new().await;
    let client = crate::http_client().build().unwrap();
    let entry = fake.publish(NEW, b"qualquer", None);
    let release = fake.release();
    let older: Version = OLD.parse().unwrap();
    assert_eq!(check(&client, &release, PLATFORM, None).await.unwrap(), Some(entry.clone()), "nada instalado");
    assert_eq!(check(&client, &release, PLATFORM, Some(older)).await.unwrap(), Some(entry));
    assert_eq!(check(&client, &release, PLATFORM, Some(NEW.parse().unwrap())).await.unwrap(), None, "mesma versão");
    assert_eq!(check(&client, &release, PLATFORM, Some("0.1.0.12".parse().unwrap())).await.unwrap(), None, "instalada mais nova");
    assert_eq!(check(&client, &release, "outro-os", None).await.unwrap_err().code(), "update_no_platform");
    let missing = Release { channel: "beta".into(), ..release };
    assert_eq!(check(&client, &missing, PLATFORM, None).await.unwrap_err().code(), "update_index_unavailable");
}

#[tokio::test]
async fn update_happy_path_swaps_restarts_proves_and_prunes() {
    let fake = FakeRelease::new().await;
    let machine = Machine::new();
    let old = machine.current();
    let (bytes, proof) = new_package(&machine, "touch {proof}");
    fake.publish(NEW, &bytes, None);
    let release = fake.release();
    let restart = FakeRestart::default();
    let health = FakeHealth { restarts: restart.count.clone(), reading: Box::new(served) };
    let steps = Arc::new(FakeSteps::default());

    let outcome = run(&machine.job(&release), steps.clone(), &restart, &health, Arc::new(NeverInUse)).await;

    assert_eq!(outcome, Outcome::Updated { version: NEW.into() });
    assert_eq!(machine.current(), real(&machine.layout.versions().join(NEW)));
    assert_eq!(restart.count.load(Ordering::SeqCst), 1);
    assert!(proof.exists());
    assert_eq!(steps.ran.lock().unwrap().len(), 1, "só o passo de máquina roda: {:?}", steps.ran.lock().unwrap());
    assert_eq!(machine.applied(), vec!["2026-10-01-maquina".to_owned()]);
    assert!(old.exists(), "a anterior fica para a volta");
    assert!(!machine.layout.versions().join("0.1.0.9").exists(), "a mais velha é podada");
    let state = machine.state();
    assert_eq!(state["fase"], "pronto");
    assert_eq!(state["ok"], true);
    assert_eq!(state["commit_para"], format!("commit-{NEW}"));
    assert_eq!(state["commit_de"], format!("commit-{OLD}"));
    assert!(!machine.layout.downloads().join(format!("hangar-{NEW}-{PLATFORM}.tar.gz")).exists(), "o download não fica");
}

#[tokio::test]
async fn update_sha_mismatch_changes_nothing() {
    let fake = FakeRelease::new().await;
    let machine = Machine::new();
    let old = machine.current();
    let (bytes, _) = new_package(&machine, "touch {proof}");
    fake.publish(NEW, &bytes, Some(&"0".repeat(64)));
    let release = fake.release();
    let restart = FakeRestart::default();
    let health = FakeHealth { restarts: restart.count.clone(), reading: Box::new(|_| rust(7, OLD)) };

    let outcome = run(&machine.job(&release), Arc::new(FakeSteps::default()), &restart, &health, Arc::new(NeverInUse)).await;

    let Outcome::Failed { voltou, no_ar, erro } = outcome else { panic!("{outcome:?}") };
    assert!(!voltou && no_ar, "nada mudou e o servidor de antes segue no ar");
    assert!(erro.contains("sha256"), "{erro}");
    assert_eq!(machine.current(), old);
    assert!(!machine.layout.versions().join(NEW).exists());
    assert_eq!(restart.count.load(Ordering::SeqCst), 0);
    let state = machine.state();
    assert_eq!((state["fase"].clone(), state["ok"].clone(), state["voltou"].clone(), state["no_ar"].clone()),
        (Value::from("pronto"), Value::from(false), Value::from(false), Value::from(true)));
}

#[tokio::test]
async fn update_machine_step_failing_changes_nothing() {
    let fake = FakeRelease::new().await;
    let machine = Machine::new();
    let old = machine.current();
    let (bytes, _) = new_package(&machine, "fail");
    fake.publish(NEW, &bytes, None);
    let release = fake.release();
    let restart = FakeRestart::default();
    let health = FakeHealth { restarts: restart.count.clone(), reading: Box::new(|_| rust(7, OLD)) };

    let outcome = run(&machine.job(&release), Arc::new(FakeSteps::default()), &restart, &health, Arc::new(NeverInUse)).await;

    let Outcome::Failed { voltou, no_ar, erro } = outcome else { panic!("{outcome:?}") };
    assert!(!voltou && no_ar);
    assert!(erro.contains("o instalador quebrou"), "a marca do instalador vai ao erro: {erro}");
    assert_eq!(machine.current(), old);
    assert_eq!(restart.count.load(Ordering::SeqCst), 0);
    assert!(machine.applied().is_empty(), "passo que falhou não entra no registro");
}

#[tokio::test]
async fn update_health_failing_after_swap_swaps_back() {
    let fake = FakeRelease::new().await;
    let machine = Machine::new();
    let old = machine.current();
    let (bytes, _) = new_package(&machine, "touch {proof}");
    fake.publish(NEW, &bytes, None);
    let release = fake.release();
    let restart = FakeRestart::default();
    // Antes: o servidor de antes; 1º reinício: ninguém; 2º (a volta): o de antes de novo.
    let health = FakeHealth { restarts: restart.count.clone(), reading: Box::new(|n| match n {
        1 => Reading::Down("connection refused".into()),
        n => served(n),
    }) };

    let outcome = run(&machine.job(&release), Arc::new(FakeSteps::default()), &restart, &health, Arc::new(NeverInUse)).await;

    let Outcome::Failed { voltou, no_ar, .. } = outcome else { panic!("{outcome:?}") };
    assert!(voltou && no_ar);
    assert_eq!(machine.current(), old, "o current volta");
    assert_eq!(restart.count.load(Ordering::SeqCst), 2, "reinicia de novo na volta");
    let state = machine.state();
    assert_eq!((state["voltou"].clone(), state["no_ar"].clone(), state["ok"].clone()), (Value::from(true), Value::from(true), Value::from(false)));
    // O passo de máquina desta rodada continua registrado, e o aviso diz qual.
    assert_eq!(machine.applied(), vec!["2026-10-01-maquina".to_owned()]);
    assert!(state["avisos"].as_array().unwrap().iter().any(|a| a.as_str().is_some_and(|a| a.contains("2026-10-01-maquina"))), "{state:?}");
}

#[tokio::test]
async fn update_old_server_answering_after_restart_fails_even_without_a_known_pid() {
    let fake = FakeRelease::new().await;
    let machine = Machine::new();
    let old = machine.current();
    let (bytes, _) = new_package(&machine, "touch {proof}");
    fake.publish(NEW, &bytes, None);
    let release = fake.release();
    let restart = FakeRestart::default();
    // Antes do reinício ninguém diz o pid; depois, um processo novo, mas da versão de antes.
    let health = FakeHealth { restarts: restart.count.clone(), reading: Box::new(|n| match n {
        0 => Reading::NotRust("sem health".into()),
        n => rust(100 + n as u32, OLD),
    }) };

    let outcome = run(&machine.job(&release), Arc::new(FakeSteps::default()), &restart, &health, Arc::new(NeverInUse)).await;

    let Outcome::Failed { voltou, erro, .. } = outcome else { panic!("{outcome:?}") };
    assert!(voltou && erro.contains(&format!("commit-{OLD}")), "{erro}");
    assert_eq!(machine.current(), old);
}

/// Troca do Windows interrompida entre os dois `rename`: sem `current`, com a versão boa ao lado.
#[tokio::test]
async fn update_recovers_the_current_left_aside_by_an_interrupted_swap() {
    let fake = FakeRelease::new().await;
    let machine = Machine::new();
    let old = machine.current();
    std::fs::rename(machine.layout.current(), machine.layout.hangar.join("current.old")).unwrap();
    let bytes = package(OLD, &format!("commit-{OLD}"), &[]);
    fake.publish(OLD, &bytes, None);
    let release = fake.release();
    let restart = FakeRestart::default();
    let health = FakeHealth { restarts: restart.count.clone(), reading: Box::new(served) };

    let outcome = run(&machine.job(&release), Arc::new(FakeSteps::default()), &restart, &health, Arc::new(NeverInUse)).await;

    assert_eq!(outcome, Outcome::UpToDate, "o current voltou antes de ler a versão instalada");
    assert_eq!(machine.current(), old);
    assert!(std::fs::symlink_metadata(machine.layout.hangar.join("current.old")).is_err(), "o link ao lado virou o current");
    assert_eq!(restart.count.load(Ordering::SeqCst), 0);
}

/// O motor `git` do Python rodando nesta máquina: recusa sem tocar em nada.
#[tokio::test]
async fn update_refuses_while_the_python_engine_runs() {
    let machine = Machine::new();
    let old = machine.current();
    let release = Release { base_url: "http://127.0.0.1:9".into(), channel: "latest".into() };
    std::fs::create_dir_all(&machine.state_dir).unwrap();
    let python = format!(r#"{{"fase": "rodando", "pid": {}, "etapa": "codigo"}}"#, std::process::id());
    std::fs::write(machine.state_dir.join("estado.json"), &python).unwrap();
    let restart = FakeRestart::default();
    let health = FakeHealth { restarts: restart.count.clone(), reading: Box::new(served) };

    let outcome = run(&machine.job(&release), Arc::new(FakeSteps::default()), &restart, &health, Arc::new(NeverInUse)).await;

    assert_eq!(outcome, Outcome::OtherEngine { pid: std::process::id() });
    assert_eq!(std::fs::read_to_string(machine.state_dir.join("estado.json")).unwrap(), python);
    assert_eq!(machine.current(), old);

    // Dono morto: a rodada segue (aqui até a falha de rede do índice).
    std::fs::write(machine.state_dir.join("estado.json"), r#"{"fase": "rodando", "pid": null}"#).unwrap();
    std::fs::write(machine.state_dir.join("rodando.lock"), "2147483000").unwrap();
    let outcome = run(&machine.job(&release), Arc::new(FakeSteps::default()), &restart, &health, Arc::new(NeverInUse)).await;
    assert!(matches!(outcome, Outcome::Failed { voltou: false, .. }), "{outcome:?}");
}

#[test]
fn update_running_package_comes_from_the_root_above_the_binary() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("bin").join("hangar-server");
    assert_eq!(package_of(&exe), None, "sem manifesto = modo desenvolvedor");
    let root = version_dir(&Layout::new(dir.path()), NEW);
    assert_eq!(package_of(&root.join("bin").join("hangar-server")),
        Some(PackageId { version: NEW.into(), commit: format!("commit-{NEW}") }));
}

#[tokio::test]
async fn update_python_only_health_is_a_failure() {
    let fake = FakeRelease::new().await;
    let machine = Machine::new();
    let old = machine.current();
    let (bytes, _) = new_package(&machine, "touch {proof}");
    fake.publish(NEW, &bytes, None);
    let release = fake.release();
    let restart = FakeRestart::default();
    let health = FakeHealth { restarts: restart.count.clone(), reading: Box::new(|n| match n {
        1 => Reading::NotRust("404 em /__hangar_server/health".into()),
        n => served(n),
    }) };

    let outcome = run(&machine.job(&release), Arc::new(FakeSteps::default()), &restart, &health, Arc::new(NeverInUse)).await;

    let Outcome::Failed { voltou, erro, .. } = outcome else { panic!("{outcome:?}") };
    assert!(voltou, "{erro}");
    assert!(erro.contains("rust"), "{erro}");
    assert_eq!(machine.current(), old);
}

#[tokio::test]
async fn update_restart_error_swaps_back() {
    let fake = FakeRelease::new().await;
    let machine = Machine::new();
    let old = machine.current();
    let (bytes, _) = new_package(&machine, "touch {proof}");
    fake.publish(NEW, &bytes, None);
    let release = fake.release();
    let restart = FakeRestart { fail_first: true, ..FakeRestart::default() };
    let health = FakeHealth { restarts: restart.count.clone(), reading: Box::new(served) };

    let outcome = run(&machine.job(&release), Arc::new(FakeSteps::default()), &restart, &health, Arc::new(NeverInUse)).await;

    let Outcome::Failed { voltou, no_ar, erro } = outcome else { panic!("{outcome:?}") };
    assert!(voltou && no_ar && erro.contains("systemctl saiu com 1"), "{erro}");
    assert_eq!(machine.current(), old);
}

/// Download que falha e pacote que não abre: nada muda, como no sha errado.
#[tokio::test]
async fn update_download_and_extraction_failures_change_nothing() {
    for (case, publish) in [("download", "asset ausente"), ("extração", "não é gzip")] {
        let fake = FakeRelease::new().await;
        let machine = Machine::new();
        let old = machine.current();
        let entry = fake.publish(NEW, publish.as_bytes(), None);
        if case == "download" {
            std::fs::remove_file(fake.root.path().join("package-latest").join(&entry.asset)).unwrap();
        }
        let release = fake.release();
        let restart = FakeRestart::default();
        let health = FakeHealth { restarts: restart.count.clone(), reading: Box::new(|_| rust(7, OLD)) };

        let outcome = run(&machine.job(&release), Arc::new(FakeSteps::default()), &restart, &health, Arc::new(NeverInUse)).await;

        assert!(matches!(outcome, Outcome::Failed { voltou: false, no_ar: true, .. }), "{case}: {outcome:?}");
        assert_eq!(machine.current(), old, "{case}");
        assert!(!machine.layout.versions().join(NEW).exists(), "{case}");
        assert_eq!(restart.count.load(Ordering::SeqCst), 0, "{case}");
    }
}

#[tokio::test]
async fn update_same_pid_after_restart_is_a_failure() {
    let fake = FakeRelease::new().await;
    let machine = Machine::new();
    let (bytes, _) = new_package(&machine, "touch {proof}");
    fake.publish(NEW, &bytes, None);
    let release = fake.release();
    let restart = FakeRestart::default();
    let health = FakeHealth { restarts: restart.count.clone(), reading: Box::new(|_| rust(42, NEW)) };

    let outcome = run(&machine.job(&release), Arc::new(FakeSteps::default()), &restart, &health, Arc::new(NeverInUse)).await;

    // A volta também só ouve a versão nova: não há como dizer que a anterior voltou.
    assert!(matches!(outcome, Outcome::Failed { voltou: false, no_ar: true, .. }), "{outcome:?}");
}

#[tokio::test]
async fn update_roll_back_says_voltou_only_with_the_previous_version_answering() {
    let fake = FakeRelease::new().await;
    let machine = Machine::new();
    let old = machine.current();
    let (bytes, _) = new_package(&machine, "touch {proof}");
    fake.publish(NEW, &bytes, None);
    let release = fake.release();
    let restart = FakeRestart::default();
    // 1º reinício: ninguém; depois da volta, outro processo, mas ainda da versão nova.
    let health = FakeHealth { restarts: restart.count.clone(), reading: Box::new(|n| match n {
        0 => rust(100, OLD),
        1 => Reading::Down("connection refused".into()),
        n => rust(100 + n as u32, NEW),
    }) };

    let outcome = run(&machine.job(&release), Arc::new(FakeSteps::default()), &restart, &health, Arc::new(NeverInUse)).await;

    let Outcome::Failed { voltou, no_ar, erro } = outcome else { panic!("{outcome:?}") };
    assert!(!voltou && no_ar, "{erro}");
    assert!(erro.contains("versão anterior") && erro.contains(&format!("commit-{NEW}")), "{erro}");
    assert_eq!(machine.current(), old, "o current voltou mesmo assim");
    assert_eq!(machine.state()["voltou"], false);
}

#[tokio::test]
async fn update_second_run_is_refused_by_the_lock() {
    let machine = Machine::new();
    let release = Release { base_url: "http://127.0.0.1:9".into(), channel: "latest".into() };
    let _held = UpdateLock::try_acquire(&machine.layout).unwrap();
    let restart = FakeRestart::default();
    let health = FakeHealth { restarts: restart.count.clone(), reading: Box::new(|_| rust(1, OLD)) };

    let outcome = run(&machine.job(&release), Arc::new(FakeSteps::default()), &restart, &health, Arc::new(NeverInUse)).await;

    assert_eq!(outcome, Outcome::Locked);
    assert!(machine.state().is_empty(), "a recusa não mexe no estado da rodada que está de pé");
    drop(_held);
    assert!(UpdateLock::try_acquire(&machine.layout).is_ok(), "solta ao sair");
}

/// Um filho no meio do fork leva a trava junto até o exec: trava que solta logo não é "outra rodada".
#[test]
fn update_lock_held_for_an_instant_is_not_busy() {
    let machine = Machine::new();
    let held = UpdateLock::try_acquire(&machine.layout).unwrap();
    let release = std::thread::spawn(move || { std::thread::sleep(Duration::from_millis(100)); drop(held); });
    assert!(UpdateLock::try_acquire(&machine.layout).is_ok());
    release.join().unwrap();
}

#[tokio::test]
async fn update_state_fields_match_the_python_engine() {
    let fake = FakeRelease::new().await;
    let machine = Machine::new();
    let (bytes, _) = new_package(&machine, "fail");
    fake.publish(NEW, &bytes, None);
    let release = fake.release();
    let restart = FakeRestart::default();
    let health = FakeHealth { restarts: restart.count.clone(), reading: Box::new(|_| rust(1, OLD)) };

    run(&machine.job(&release), Arc::new(FakeSteps::default()), &restart, &health, Arc::new(NeverInUse)).await;

    // `_base()` do Python: `<CLAUDE_CONFIG_DIR>/.hangar-update/estado.json`.
    let path = machine.home.path().join(".claude/.hangar-update/estado.json");
    let state: serde_json::Map<String, Value> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    for key in ["fase", "ok", "erro", "voltou", "no_ar", "avisos", "etapa", "passo", "total", "texto", "etapa_inicio", "ts", "pid",
        "commit_de", "commit_para", "log", "reiniciar_manual"] {
        assert!(state.contains_key(key), "falta {key}: {state:?}");
    }
    assert_eq!(state["pid"], std::process::id());
    assert_eq!(state["etapa"], "passos");
    assert!(state["avisos"].is_array() && state["log"].is_array());
    let ts = state["ts"].as_str().unwrap();
    assert!(chrono::DateTime::parse_from_rfc3339(ts).is_ok() && ts.len() == 25, "isoformat com segundos e fuso: {ts}");
}

#[test]
fn update_prune_keeps_current_and_previous_and_skips_in_use() {
    let home = tempfile::tempdir().unwrap();
    let layout = Layout::new(home.path());
    let busy = version_dir(&layout, "0.1.0.7");
    let stale = version_dir(&layout, "0.1.0.8");
    let previous = version_dir(&layout, "0.1.0.9");
    let current = version_dir(&layout, "0.1.0.10");
    swap(&layout, &current).unwrap();
    struct Busy(PathBuf);
    impl InUse for Busy {
        fn in_use(&self, dir: &Path) -> bool { real(dir) == real(&self.0) }
    }

    let pruned = prune(&layout, Some(&previous), &Busy(busy.clone()));

    assert!(current.exists() && previous.exists() && busy.exists());
    assert!(!stale.exists());
    assert_eq!(pruned.in_use.len(), 1);
    assert_eq!(pruned.removed.len(), 1);
    assert!(pruned.failed.is_empty(), "{:?}", pruned.failed);
    let leftovers: Vec<_> = std::fs::read_dir(layout.versions()).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(leftovers.len(), 3, "sem lixeira sobrando: {leftovers:?}");
}

#[cfg(target_os = "linux")]
#[test]
fn update_proc_in_use_sees_a_process_working_in_the_folder() {
    let home = tempfile::tempdir().unwrap();
    let layout = Layout::new(home.path());
    let busy = version_dir(&layout, "0.1.0.7");
    let idle = version_dir(&layout, "0.1.0.8");
    let mut child = std::process::Command::new("sleep").arg("30").current_dir(&busy).spawn().unwrap();
    let seen = (ProcInUse.in_use(&busy), ProcInUse.in_use(&idle));
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(seen, (true, false));
}

#[test]
fn update_swap_returns_the_previous_target() {
    let home = tempfile::tempdir().unwrap();
    let layout = Layout::new(home.path());
    let a = version_dir(&layout, "0.1.0.1");
    let b = version_dir(&layout, "0.1.0.2");
    assert_eq!(swap(&layout, &a).unwrap(), None);
    assert_eq!(swap(&layout, &b).unwrap(), Some(real(&a)));
    assert_eq!(real(&layout.current()), real(&b));
    assert_eq!(installed(&layout).map(|m| m.version), Some("0.1.0.2".to_owned()));
    let names: Vec<_> = std::fs::read_dir(&layout.hangar).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(names.len(), 2, "só versions e current: {names:?}");
}

#[tokio::test]
async fn update_prepare_redoes_a_differing_folder_but_never_the_current_one() {
    let fake = FakeRelease::new().await;
    let machine = Machine::new();
    let bytes = package(NEW, "c", &[("VERSION", "novo\n")]);
    let entry = fake.publish(NEW, &bytes, None);
    let release = fake.release();
    let stale = machine.layout.versions().join(NEW);
    std::fs::create_dir_all(&stale).unwrap();
    std::fs::write(stale.join("lixo"), "x").unwrap();

    let dir = prepare(&machine.client, &release, &entry, &machine.layout, Duration::from_secs(30)).await.unwrap();
    assert_eq!(std::fs::read_to_string(dir.join("VERSION")).unwrap(), "novo\n");
    assert!(!dir.join("lixo").exists());

    std::fs::write(dir.join("lixo"), "x").unwrap();
    swap(&machine.layout, &dir).unwrap();
    let error = prepare(&machine.client, &release, &entry, &machine.layout, Duration::from_secs(30)).await.unwrap_err();
    assert_eq!(error.code(), "package_existing_differs");
    assert!(dir.join("lixo").exists(), "a pasta do current não é apagada");
}

#[test]
fn update_steps_read_like_python_and_register_after_the_proof() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("versao");
    let docs = root.join("docs/atualizacoes");
    std::fs::create_dir_all(&docs).unwrap();
    let state_dir = home.path().join(".hangar-update");
    // Dois seguidos com o mesmo comando rodam uma vez; prova com `~` e relativa à raiz da versão.
    std::fs::write(docs.join("2026-01-01-a.md"), step_doc("2026-01-01-a", "maquina", "touch ARQ", "~/a-prova")).unwrap();
    std::fs::write(docs.join("2026-01-02-b.md"), step_doc("2026-01-02-b", "maquina", "touch ARQ", "rel-prova")).unwrap();
    std::fs::write(docs.join("2026-01-03-texto.md"), "---\ntitulo: Só texto\ntipo: maquina\n---\ncorpo\n").unwrap();
    std::fs::write(docs.join("2026-01-04-sem-prova.md"), "---\ntitulo: X\ncomando: touch y\ntipo: maquina\n---\n").unwrap();
    std::fs::write(docs.join("2026-01-05-sem-titulo.md"), "---\nid: z\n---\n").unwrap();
    std::fs::write(docs.join("2026-01-06-conteudo.md"), step_doc("2026-01-06-conteudo", "conteudo", "touch ARQ", "x")).unwrap();
    std::fs::write(docs.join("README.md"), "# leia\n").unwrap();
    std::fs::write(home.path().join("a-prova"), "").unwrap();
    std::fs::write(root.join("rel-prova"), "").unwrap();
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(state_dir.join("aplicados.json"), r#"["2026-01-06-conteudo", "antigo"]"#).unwrap();

    let (steps, invalid) = steps::read_steps(&docs, false);
    assert_eq!(steps.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["2026-01-01-a", "2026-01-02-b", "2026-01-03-texto", "2026-01-06-conteudo"]);
    assert_eq!(invalid, ["2026-01-04-sem-prova.md", "2026-01-05-sem-titulo.md"]);
    assert!(steps[2].machine && !steps[3].machine && steps[2].command.is_empty());

    struct Count(Mutex<usize>);
    impl StepRunner for Count {
        fn run(&self, _command: &str, _cwd: &Path) -> StepRun { *self.0.lock().unwrap() += 1; StepRun { code: Some(0), output: String::new() } }
    }
    let runner = Count(Mutex::new(0));
    let state = StateFile::new(&state_dir);
    let done = apply_machine_steps(&root, home.path(), &state_dir, &runner, &state).unwrap();
    assert_eq!(done, ["2026-01-01-a", "2026-01-02-b", "2026-01-03-texto"]);
    assert_eq!(*runner.0.lock().unwrap(), 1, "mesmo comando do vizinho roda uma vez");
    let registry: Value = serde_json::from_slice(&std::fs::read(state_dir.join("aplicados.json")).unwrap()).unwrap();
    assert_eq!(registry, serde_json::json!(["2026-01-01-a", "2026-01-02-b", "2026-01-03-texto", "2026-01-06-conteudo", "antigo"]));
    assert_eq!(state.read()["passos_invalidos"], serde_json::json!(["2026-01-04-sem-prova.md", "2026-01-05-sem-titulo.md"]));

    // Prova que falta depois do comando: falha e não registra.
    std::fs::write(docs.join("2026-01-07-c.md"), step_doc("2026-01-07-c", "maquina", "touch ARQ", "nao-existe")).unwrap();
    let error = apply_machine_steps(&root, home.path(), &state_dir, &runner, &state).unwrap_err();
    assert!(error.to_string().contains("nao-existe"), "{error}");
    assert!(!steps::applied(&state_dir).unwrap().contains("2026-01-07-c"));
}

/// Registro ilegível é erro, nunca "nada aplicado": nem some o histórico nem se roda tudo de novo.
#[test]
fn update_steps_unreadable_registry_is_an_error_and_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let state_dir = dir.path().join("state");
    assert!(steps::applied(&state_dir).unwrap().is_empty(), "arquivo ausente = nada aplicado");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(state_dir.join("aplicados.json"), "[\"antigo\", ").unwrap();
    assert!(steps::applied(&state_dir).is_err());
    assert!(steps::mark(&state_dir, "novo").is_err());
    let root = dir.path().join("root");
    std::fs::create_dir_all(root.join("docs/atualizacoes")).unwrap();
    struct Never;
    impl StepRunner for Never { fn run(&self, _: &str, _: &Path) -> StepRun { panic!("não roda passo sem saber o que já rodou") } }
    assert!(apply_machine_steps(&root, dir.path(), &state_dir, &Never, &StateFile::new(&state_dir)).is_err());
    assert!(steps::mark_all(&root, &state_dir).is_err());
    assert_eq!(std::fs::read_to_string(state_dir.join("aplicados.json")).unwrap(), "[\"antigo\", ");
}

#[test]
fn update_steps_pick_the_command_of_this_system() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("p.md"), "---\ntitulo: T\ncomando_posix: so-posix\nprova: x\ntipo: maquina\n---\n").unwrap();
    assert_eq!(steps::read_steps(dir.path(), false).0[0].command, "so-posix");
    let windows = &steps::read_steps(dir.path(), true).0[0];
    assert_eq!((windows.command.as_str(), windows.proof.len()), ("", 0), "só do outro sistema: texto, sem prova");
}

#[cfg(unix)]
#[test]
fn update_shell_runner_runs_in_the_version_root() {
    let dir = tempfile::tempdir().unwrap();
    let runner = ShellRunner { timeout: Duration::from_secs(10) };
    let ok = runner.run("echo oi > saida && echo $LC_ALL $HANGAR_UPDATE_OWNS_RUST", dir.path());
    assert_eq!(ok, StepRun { code: Some(0), output: "C 1\n".into() });
    assert!(dir.path().join("saida").exists());
    assert_eq!(runner.run("exit 3", dir.path()).code, Some(3));
    let slow = ShellRunner { timeout: Duration::from_millis(200) }.run("sleep 5", dir.path());
    assert_eq!(slow.code, None, "estourou o prazo");
}

#[cfg(unix)]
fn alive(pid: i32) -> bool {
    // Zumbi conta como morto: o processo já saiu, só falta quem o recolha.
    std::fs::read_to_string(format!("/proc/{pid}/stat")).map_or_else(
        |_| unsafe { libc::kill(pid, 0) == 0 },
        |stat| stat.rsplit(')').next().is_some_and(|rest| !rest.trim_start().starts_with('Z')))
}

/// Neto que herda o stdout: o passo que terminou não espera por ele, e o que estourou o prazo leva
/// a árvore inteira junto.
#[cfg(unix)]
#[test]
fn update_shell_runner_does_not_wait_for_a_grandchild_and_kills_the_tree_on_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let runner = ShellRunner { timeout: Duration::from_secs(20) };
    let started = std::time::Instant::now();
    let done = runner.run("sleep 30 & echo $! > neto; echo oi", dir.path());
    let grandchild: i32 = std::fs::read_to_string(dir.path().join("neto")).unwrap().trim().parse().unwrap();
    unsafe { libc::kill(grandchild, libc::SIGKILL) };
    assert_eq!(done, StepRun { code: Some(0), output: "oi\n".into() });
    assert!(started.elapsed() < Duration::from_secs(10), "esperou o neto: {:?}", started.elapsed());

    let started = std::time::Instant::now();
    let slow = ShellRunner { timeout: Duration::from_millis(300) }.run("sleep 30 & echo $! > neto2; sleep 30", dir.path());
    assert_eq!(slow.code, None);
    assert!(started.elapsed() < Duration::from_secs(10), "esperou o neto: {:?}", started.elapsed());
    let grandchild: i32 = std::fs::read_to_string(dir.path().join("neto2")).unwrap().trim().parse().unwrap();
    let gone = (0..100).any(|_| { std::thread::sleep(Duration::from_millis(20)); !alive(grandchild) });
    if !gone { unsafe { libc::kill(grandchild, libc::SIGKILL) }; }
    assert!(gone, "o neto {grandchild} sobreviveu ao prazo");
}

#[tokio::test]
async fn update_http_health_tells_rust_from_python_and_down() {
    let root = tempfile::tempdir().unwrap();
    let (python, server) = serve(root.path().to_path_buf()).await;
    assert!(matches!(HttpHealth::new(&python).read().await, Reading::NotRust(_)), "404 no health = só o Python");
    std::fs::create_dir_all(root.path().join("__hangar_server")).unwrap();
    std::fs::write(root.path().join("__hangar_server/health"), r#"{"ok":true,"protocol":89,"pid":4321}"#).unwrap();
    assert_eq!(HttpHealth::new(&python).read().await, Reading::Rust { protocol: 89, pid: Some(4321), commit: None }, "modo desenvolvedor");
    std::fs::write(root.path().join("__hangar_server/health"),
        r#"{"ok":true,"protocol":89,"pid":4321,"package_version":"0.1.0.11","package_commit":"abc"}"#).unwrap();
    assert_eq!(HttpHealth::new(&python).read().await, Reading::Rust { protocol: 89, pid: Some(4321), commit: Some("abc".into()) });
    server.abort();
    let _ = server.await;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let closed = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    assert!(matches!(HttpHealth::new(&closed).read().await, Reading::Down(_)));
}

const KEY: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// Tela web falsa: anota a chave pedida e se o passo de máquina já tinha rodado.
struct FakeUi {
    asked: Mutex<Vec<(String, bool)>>,
    proof: PathBuf,
    fail: bool,
}

impl UiFetcher for FakeUi {
    fn fetch<'a>(&'a self, key: &'a str, ui_dir: &'a Path) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.asked.lock().unwrap().push((key.to_owned(), self.proof.exists()));
            if self.fail { return Err("ui_download_failed: sem rede".into()); }
            std::fs::create_dir_all(ui_dir.join(key)).unwrap();
            Ok(())
        })
    }
}

async fn run_with_ui(web_ui: Option<bool>, fail: bool) -> (Outcome, Machine, Vec<(String, bool)>) {
    let fake = FakeRelease::new().await;
    let machine = Machine::new();
    let proof = machine.home.path().join("prova-do-passo");
    let maquina = step_doc("2026-10-01-maquina", "maquina", &format!("touch {}", proof.display()), &proof.display().to_string());
    fake.publish(NEW, &package_keyed(NEW, &format!("commit-{NEW}"), Some(KEY), &[("docs/atualizacoes/2026-10-01-maquina.md", &maquina)]), None);
    if let Some(on) = web_ui {
        std::fs::write(machine.layout.hangar.join("web-ui.json"), serde_json::json!({"web_ui": on}).to_string()).unwrap();
    }
    let release = fake.release();
    let restart = FakeRestart::default();
    let health = FakeHealth { restarts: restart.count.clone(), reading: Box::new(served) };
    let ui = FakeUi { asked: Mutex::default(), proof, fail };
    let mut job = machine.job(&release);
    job.ui = Some(&ui);
    let outcome = run(&job, Arc::new(FakeSteps::default()), &restart, &health, Arc::new(NeverInUse)).await;
    drop(job);
    let asked = ui.asked.into_inner().unwrap();
    (outcome, machine, asked)
}

#[tokio::test]
async fn update_dist_is_fetched_after_prepare_and_before_the_machine_steps() {
    let (outcome, machine, asked) = run_with_ui(None, false).await;
    assert_eq!(outcome, Outcome::Updated { version: NEW.into() });
    assert_eq!(asked, vec![(KEY.to_owned(), false)], "a chave do manifesto novo, antes do passo de máquina");
    assert!(machine.layout.hangar.join("ui").join(KEY).is_dir(), "baixada em ~/.hangar/ui");
}

#[tokio::test]
async fn update_dist_is_not_fetched_with_the_web_ui_off() {
    let (outcome, _machine, asked) = run_with_ui(Some(false), false).await;
    assert_eq!(outcome, Outcome::Updated { version: NEW.into() });
    assert!(asked.is_empty(), "{asked:?}");
}

#[tokio::test]
async fn update_dist_failure_is_a_warning_not_a_failed_update() {
    let (outcome, machine, asked) = run_with_ui(Some(true), true).await;
    assert_eq!(asked.len(), 1);
    assert_eq!(outcome, Outcome::Updated { version: NEW.into() });
    let state = machine.state();
    assert_eq!(state["ok"], true);
    let warnings = state["avisos"].as_array().unwrap();
    assert!(warnings.iter().any(|w| w.as_str().unwrap().contains("ui_download_failed")), "{warnings:?}");
}

#[tokio::test]
async fn update_manual_install_swaps_without_restart_and_says_so() {
    let fake = FakeRelease::new().await;
    let machine = Machine::new();
    let (bytes, _) = new_package(&machine, "touch {proof}");
    fake.publish(NEW, &bytes, None);
    let release = fake.release();
    let restart = FakeRestart { manual: true, ..FakeRestart::default() };
    // O servidor de antes segue no ar: a prova de vida reprovaria.
    let health = FakeHealth { restarts: restart.count.clone(), reading: Box::new(|_| rust(7, OLD)) };

    let outcome = run(&machine.job(&release), Arc::new(FakeSteps::default()), &restart, &health, Arc::new(NeverInUse)).await;

    assert_eq!(outcome, Outcome::UpdatedManual { version: NEW.into() });
    assert_eq!(restart.count.load(Ordering::SeqCst), 0);
    assert_eq!(machine.current(), real(&machine.layout.versions().join(NEW)));
    let state = machine.state();
    assert_eq!((state["fase"].clone(), state["ok"].clone(), state["reiniciar_manual"].clone()), ("pronto".into(), true.into(), true.into()));
}

/// pid que não existe (acima do teto de pids do Linux e do Windows usual).
const DEAD: u64 = 999_999_999;

#[test]
fn update_screen_state_turns_a_dead_owner_into_a_failure() {
    let machine = Machine::new();
    let state = StateFile::new(&machine.state_dir);
    let old_ts = "2026-01-01T00:00:00+00:00";
    std::fs::create_dir_all(&machine.state_dir).unwrap();
    let write = |value: Value| std::fs::write(state.path(), value.to_string()).unwrap();

    write(serde_json::json!({"fase": "rodando", "pid": DEAD, "etapa": "baixar", "ts": old_ts}));
    let screen = for_screen(&machine.state_dir, &machine.layout, Some(OLD));
    assert_eq!((screen["fase"].clone(), screen["ok"].clone(), screen["voltou"].clone()), ("pronto".into(), false.into(), false.into()));
    assert!(screen["erro"].as_str().unwrap().contains("interrompida"));
    assert_eq!(state.read()["fase"], "pronto", "gravado, para não repetir a conclusão");

    // Sem pid e recém-escrito: o executor ainda está subindo.
    write(serde_json::json!({"fase": "rodando", "pid": null}));
    StateFile::new(&machine.state_dir).write(serde_json::json!({}));
    assert_eq!(for_screen(&machine.state_dir, &machine.layout, Some(OLD))["fase"], "rodando");

    // Vivo: nada muda.
    write(serde_json::json!({"fase": "rodando", "pid": std::process::id(), "ts": old_ts}));
    assert_eq!(for_screen(&machine.state_dir, &machine.layout, Some(OLD))["fase"], "rodando");

    // Morreu no reinício que pediu, e a versão nova está instalada e no ar: concluída.
    write(serde_json::json!({"fase": "rodando", "pid": DEAD, "etapa": "reiniciar", "versao_para": OLD, "ts": old_ts}));
    let screen = for_screen(&machine.state_dir, &machine.layout, Some(OLD));
    assert_eq!((screen["fase"].clone(), screen["ok"].clone()), ("pronto".into(), true.into()));
    // A mesma morte com o servidor ainda na versão de antes é falha, e o texto diz onde a máquina ficou.
    write(serde_json::json!({"fase": "rodando", "pid": DEAD, "etapa": "reiniciar", "versao_para": OLD, "ts": old_ts}));
    let screen = for_screen(&machine.state_dir, &machine.layout, Some("0.1.0.9"));
    assert_eq!(screen["ok"], false);
    let erro = screen["erro"].as_str().unwrap();
    assert!(erro.contains(&format!("aponta para a versão {OLD}")) && erro.contains("servidor de pé é o da versão 0.1.0.9"), "{erro}");
}

#[tokio::test]
async fn update_unreadable_web_ui_choice_goes_to_the_diary_and_keeps_the_default() {
    let fake = FakeRelease::new().await;
    let machine = Machine::new();
    fake.publish(NEW, &package_keyed(NEW, &format!("commit-{NEW}"), Some(KEY), &[]), None);
    std::fs::write(machine.layout.hangar.join("web-ui.json"), "{ torto").unwrap();
    let release = fake.release();
    let restart = FakeRestart::default();
    let health = FakeHealth { restarts: restart.count.clone(), reading: Box::new(served) };
    let ui = FakeUi { asked: Mutex::default(), proof: machine.home.path().join("x"), fail: false };
    let diary = machine.home.path().join("diario");
    let mut job = machine.job(&release);
    job.ui = Some(&ui);
    job.diary_dir = Some(diary.clone());
    assert_eq!(run(&job, Arc::new(FakeSteps::default()), &restart, &health, Arc::new(NeverInUse)).await, Outcome::Updated { version: NEW.into() });
    assert_eq!(ui.asked.lock().unwrap().len(), 1, "o padrão é ligada");
    let lines: String = std::fs::read_dir(&diary).unwrap().flatten().map(|e| std::fs::read_to_string(e.path()).unwrap()).collect();
    assert!(lines.contains("update_web_ui_unreadable"), "{lines}");
}

#[tokio::test]
async fn update_launch_notes_survive_the_reset_of_the_warnings() {
    let fake = FakeRelease::new().await;
    let machine = Machine::new();
    let (bytes, _) = new_package(&machine, "touch {proof}");
    fake.publish(NEW, &bytes, None);
    let release = fake.release();
    let restart = FakeRestart::default();
    let health = FakeHealth { restarts: restart.count.clone(), reading: Box::new(served) };
    let mut job = machine.job(&release);
    job.notes = vec!["o Job recusou a saída".into()];
    assert_eq!(run(&job, Arc::new(FakeSteps::default()), &restart, &health, Arc::new(NeverInUse)).await, Outcome::Updated { version: NEW.into() });
    assert_eq!(machine.state()["avisos"], serde_json::json!(["o Job recusou a saída"]));
}

#[tokio::test]
async fn update_incomplete_manifest_in_use_fails_with_the_reason_and_changes_nothing() {
    let machine = Machine::new();
    let old = machine.current();
    // Pacote pelo critério do Python, mas sem o que o atualizador precisa.
    std::fs::write(old.join("manifest.json"), r#"{"version": "0.1.0.10", "commit": "c", "channel": "latest"}"#).unwrap();
    let release = Release { base_url: "http://127.0.0.1:9".into(), channel: "latest".into() };
    let restart = FakeRestart::default();
    let health = FakeHealth { restarts: restart.count.clone(), reading: Box::new(|_| rust(7, OLD)) };
    let outcome = run(&machine.job(&release), Arc::new(FakeSteps::default()), &restart, &health, Arc::new(NeverInUse)).await;
    let Outcome::Failed { voltou: false, erro, .. } = outcome else { panic!("{outcome:?}") };
    assert!(erro.contains("manifest.json"), "{erro}");
    assert_eq!(machine.state()["erro"].as_str(), Some(erro.as_str()));
    assert_eq!(machine.current(), old);
}
