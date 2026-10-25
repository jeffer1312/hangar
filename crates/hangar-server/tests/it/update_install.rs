//! `hangar-server install-package`: a instalação nova direto pelo pacote, sem git. O binário de
//! verdade roda com um HOME vazio contra uma release falsa servida em processo.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hangar_package::{Manifest, PackageIndex, PlatformEntry};
use serde_json::Value;

const VERSION: &str = "0.1.0.42";
const LAUNCHER: &str = "binário do pacote";

fn sha(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

fn exe(name: &str) -> String { if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() } }

/// `.tar.gz` do pacote: `manifest.json`, o binário em `bin/`, um passo de máquina e um só de texto.
fn package(channel: &str) -> Vec<u8> {
    let step = "---\nid: 2026-01-01-maquina\ntitulo: Um passo de máquina\ncomando: false\nprova: ~/nunca\ntipo: maquina\n---\ntexto\n";
    let note = "---\ntitulo: Só texto\n---\nnada a rodar\n";
    let broken = "---\ncomando: true\n---\nsem título nem prova\n";
    let bin = format!("bin/{}", exe("hangar-server"));
    let files = [(bin.as_str(), LAUNCHER), ("docs/atualizacoes/2026-01-01-maquina.md", step), ("docs/atualizacoes/2026-01-02-texto.md", note),
        ("docs/atualizacoes/2026-01-03-torto.md", broken), ("install.sh", "#!/bin/sh\n")];
    let manifest = Manifest {
        version: VERSION.into(), commit: "c0ffee".into(), channel: channel.into(), protocol: 7, api: 1,
        os: hangar_server::update::platform(), dist_key: None,
        files: files.iter().map(|(p, c)| (p.to_string(), sha(c.as_bytes()))).collect(),
    };
    let manifest = serde_json::to_vec(&manifest).unwrap();
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
    for (path, content) in std::iter::once(("manifest.json", manifest.as_slice())).chain(files.iter().map(|(p, c)| (*p, c.as_bytes()))) {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, path, content).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

/// Release falsa: `<base>/package-<canal>/…` servido de uma pasta.
struct Release {
    root: tempfile::TempDir,
    base: String,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Release { fn drop(&mut self) { self.server.abort(); } }

impl Release {
    async fn new() -> Release {
        use axum::extract::{Path as UrlPath, State};
        let root = tempfile::tempdir().unwrap();
        let app = axum::Router::new().route("/{*path}", axum::routing::get(|State(root): State<PathBuf>, UrlPath(path): UrlPath<String>| async move {
            std::fs::read(root.join(path)).map_err(|_| axum::http::StatusCode::NOT_FOUND)
        })).with_state(root.path().to_path_buf());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        Release { root, base, server }
    }

    /// Publica o pacote do canal; `platform` é a chave no índice e `sha_override` troca o sha anunciado.
    fn publish(&self, channel: &str, platform: &str, sha_override: Option<&str>) {
        let dir = self.root.path().join(format!("package-{channel}"));
        std::fs::create_dir_all(&dir).unwrap();
        let bytes = package(channel);
        let asset = format!("hangar-{VERSION}.tar.gz");
        std::fs::write(dir.join(&asset), &bytes).unwrap();
        let entry = PlatformEntry { version: VERSION.into(), asset, sha256: sha_override.map_or_else(|| sha(&bytes), str::to_owned),
            protocol: 7, api: 1, commit: "c0ffee".into(), dist_key: None };
        let index = PackageIndex { platforms: BTreeMap::from([(platform.to_owned(), entry)]) };
        std::fs::write(dir.join(format!("package-{channel}.json")), serde_json::to_vec(&index).unwrap()).unwrap();
    }
}

async fn install(home: &Path, args: &[&str]) -> std::process::Output {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_hangar-server"));
    command.arg("install-package").args(args)
        .env_clear().env("HOME", home).env("USERPROFILE", home).env("LOCALAPPDATA", home.join("AppData").join("Local"))
        .env("PATH", std::env::var_os("PATH").unwrap_or_default());
    // Sem o SystemRoot o Winsock não inicializa e nenhuma conexão sai, nem para o 127.0.0.1.
    if let Some(root) = std::env::var_os("SystemRoot") { command.env("SystemRoot", root); }
    command.output().await.unwrap()
}

fn text(out: &std::process::Output) -> String { format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)) }

fn read_json(path: &Path) -> Value { serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap() }

#[tokio::test]
async fn install_package_installs_the_channel_package_from_scratch() {
    let release = Release::new().await;
    release.publish("beta", &hangar_server::update::platform(), None);
    let home = tempfile::tempdir().unwrap();
    let out = install(home.path(), &["--channel", "beta", "--release-base", &release.base]).await;
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let hangar = home.path().join(".hangar");
    let current = std::fs::canonicalize(hangar.join("current")).unwrap();
    assert_eq!(current, std::fs::canonicalize(hangar.join("versions").join(VERSION)).unwrap());
    assert!(text(&out).contains(VERSION), "a saída diz a versão: {}", text(&out));
    // O lançador estável é o binário do pacote, executável, fora da pasta da versão.
    let launcher = hangar.join("bin").join(exe("hangar-launcher"));
    assert_eq!(std::fs::read_to_string(&launcher).unwrap(), LAUNCHER);
    #[cfg(unix)]
    assert!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&launcher).unwrap().permissions()) & 0o111 != 0);
    // A marca diz pacote, sem checkout: a migração e o passo dela não rodam aqui.
    let marker = read_json(&hangar.join("migration.json"));
    assert_eq!((marker["mode"].as_str(), marker["version"].as_str()), (Some("package"), Some(VERSION)), "{marker}");
    assert_eq!(marker["checkout"], "");
    // Os passos só são marcados pelo install.sh, quando ele chega ao fim (`--mark-steps`).
    assert!(!home.path().join(".claude/.hangar-update/aplicados.json").exists(), "nenhum passo marcado antes do install.sh");
    assert!(!home.path().join("nunca").exists());
}

#[tokio::test]
async fn install_package_mark_steps_marks_every_step_of_current_and_warns_on_malformed() {
    let release = Release::new().await;
    release.publish("latest", &hangar_server::update::platform(), None);
    let home = tempfile::tempdir().unwrap();
    let out = install(home.path(), &["--mark-steps"]).await;
    assert_eq!(out.status.code(), Some(1), "sem current, nada a marcar: {}", text(&out));
    assert!(text(&out).contains("current"), "{}", text(&out));
    assert_eq!(install(home.path(), &["--release-base", &release.base]).await.status.code(), Some(0));
    let out = install(home.path(), &["--mark-steps"]).await;
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    // Instalação do zero já cumpre todo passo: todos registrados, nenhum rodou; o malformado aparece.
    let applied = read_json(&home.path().join(".claude/.hangar-update/aplicados.json"));
    assert_eq!(applied, serde_json::json!(["2026-01-01-maquina", "2026-01-02-texto"]));
    assert!(!home.path().join("nunca").exists());
    assert!(text(&out).contains("aviso:") && text(&out).contains("2026-01-03-torto.md"), "{}", text(&out));
    assert_eq!(install(home.path(), &["--mark-steps", "--channel", "beta"]).await.status.code(), Some(2), "--mark-steps não se mistura");
}

#[tokio::test]
async fn install_package_refuses_an_existing_installation() {
    let release = Release::new().await;
    release.publish("latest", &hangar_server::update::platform(), None);
    // Uma pasta qualquer já no lugar do `current`, o serviço do backend já gravado, ou o checkout que o servidor anotou.
    let current = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(current.path().join(".hangar/current")).unwrap();
    let unit = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(unit.path().join(".config/systemd/user")).unwrap();
    std::fs::write(unit.path().join(".config/systemd/user/hangar-backend.service"), "[Service]\n").unwrap();
    // No Windows o serviço é a tarefa, achada pelo `.vbs` em %LOCALAPPDATA%\hangar.
    std::fs::create_dir_all(unit.path().join("AppData/Local/hangar")).unwrap();
    std::fs::write(unit.path().join("AppData/Local/hangar/hangar-backend.vbs"), "WScript.Quit 0\n").unwrap();
    let git = tempfile::tempdir().unwrap();
    let checkout = git.path().join("hangar");
    std::fs::create_dir_all(checkout.join(".git")).unwrap();
    std::fs::create_dir_all(checkout.join("backend/app")).unwrap();
    std::fs::create_dir_all(git.path().join(".hangar")).unwrap();
    std::fs::write(git.path().join(".hangar/backend-dir"), checkout.join("backend").to_string_lossy().as_bytes()).unwrap();
    let service = if cfg!(windows) { "hangar-backend.vbs" } else { "hangar-backend.service" };
    for (home, says) in [(&current, "current"), (&unit, service), (&git, checkout.to_str().unwrap())] {
        let out = install(home.path(), &["--release-base", &release.base]).await;
        assert_eq!(out.status.code(), Some(4), "{}", text(&out));
        assert!(text(&out).contains(says), "o motivo cita {says}: {}", text(&out));
        assert!(!home.path().join(".hangar/versions").exists(), "nada foi baixado");
        assert!(!home.path().join(".hangar/migration.json").exists());
    }
}

#[tokio::test]
async fn install_package_without_a_package_for_this_system_exits_3() {
    let release = Release::new().await;
    release.publish("latest", "outro-sistema", None);
    let home = tempfile::tempdir().unwrap();
    let out = install(home.path(), &["--release-base", &release.base]).await;
    assert_eq!(out.status.code(), Some(3), "{}", text(&out));
    assert!(text(&out).contains(&hangar_server::update::platform()), "{}", text(&out));
    for left in ["current", "migration.json", "versions"] {
        assert!(std::fs::symlink_metadata(home.path().join(".hangar").join(left)).is_err(), "{left} não pode ficar");
    }
}

#[tokio::test]
async fn install_package_with_a_wrong_sha_changes_nothing() {
    let release = Release::new().await;
    release.publish("latest", &hangar_server::update::platform(), Some(&"0".repeat(64)));
    let home = tempfile::tempdir().unwrap();
    let out = install(home.path(), &["--release-base", &release.base]).await;
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("sha256"), "{}", text(&out));
    let hangar = home.path().join(".hangar");
    for left in [hangar.join("current"), hangar.join("migration.json"), hangar.join("bin"), home.path().join(".claude/.hangar-update/aplicados.json")] {
        assert!(std::fs::symlink_metadata(&left).is_err(), "{} não pode ficar", left.display());
    }
}

/// Uma instalação interrompida entre a marca e o `current` não trava a próxima tentativa.
#[tokio::test]
async fn install_package_resumes_after_a_marker_without_current() {
    let release = Release::new().await;
    release.publish("latest", &hangar_server::update::platform(), None);
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".hangar")).unwrap();
    std::fs::write(home.path().join(".hangar/migration.json"), serde_json::json!({"mode": "package", "date": "d", "checkout": "",
        "version": "0.1.0.1", "reason": null, "notes": [], "done": []}).to_string()).unwrap();
    let out = install(home.path(), &["--release-base", &release.base]).await;
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert_eq!(read_json(&home.path().join(".hangar/migration.json"))["version"], VERSION);
}

#[tokio::test]
async fn install_package_wrong_usage_exits_2() {
    let home = tempfile::tempdir().unwrap();
    for args in [&["--nada"][..], &["--channel"][..], &["--channel", "../x"][..]] {
        assert_eq!(install(home.path(), args).await.status.code(), Some(2), "{args:?}");
    }
    assert!(!home.path().join(".hangar").exists());
}

// Um binário sem o subcomando pedido não pode subir como servidor: o bootstrap leria
// `HANGAR_SERVER_LISTEN ausente` em vez do motivo.
#[test]
fn unknown_subcommand_is_refused_with_2() {
    let home = tempfile::tempdir().unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_hangar-server")).arg("instalar-coisa")
        .env_clear().env("HOME", home.path()).env("USERPROFILE", home.path())
        .stdin(std::process::Stdio::null()).output().unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(text(&out).contains("subcomando desconhecido: instalar-coisa"), "{}", text(&out));
    assert!(!home.path().join(".hangar").exists());
}
