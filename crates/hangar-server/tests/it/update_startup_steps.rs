//! B-52: a subida nunca aplica passo de atualização quando roda de um pacote (quem aplica é a
//! atualização), e no Windows deixa para o botão Atualizar o passo que chama o instalador. O Python
//! não rodava passo nenhum no `app.main` (commits aacb04c51, e8bd30165).
use std::collections::BTreeMap;
use std::path::Path;

use hangar_server::update::git_engine::{Service, periodic::StartupOutcome};

/// Um passo pendente, não destrutivo, que deixaria a pasta `marker` se rodasse.
fn pending_step(root: &Path, command_key: &str, command: &str, marker: &Path) {
    let dir = root.join("docs").join("atualizacoes");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("2026-01-01-passo.md"), format!(
        "---\nid: 2026-01-01-passo\ntitulo: Passo de teste\n{command_key}: {command}\nprova: {}\ndestrutivo: false\n---\n\nTexto.\n",
        marker.display())).unwrap();
}

fn service(home: &Path, root: &Path) -> Service {
    Service::new(home.to_path_buf(), root.to_path_buf(), home.join(".claude"), BTreeMap::new())
}

#[test]
fn b52_a_package_applies_no_step_on_startup() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let real = home.join(".hangar").join("versions").join("v1");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::write(real.join("manifest.json"), r#"{"version":"v1","commit":"abc123","channel":"main"}"#).unwrap();
    let current = home.join(".hangar").join("current");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, &current).unwrap();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let out = std::process::Command::new("cmd.exe")
            .raw_arg(format!("/c mklink /J \"{}\" \"{}\"", current.display(), real.display())).output().unwrap();
        assert!(out.status.success(), "junction não criada: {out:?}");
    }
    let marker = dir.path().join("rodou");
    pending_step(&real, "comando", &format!("mkdir {}", marker.display()), &marker);
    let service = service(&home, &current);
    let outcome = service.startup_outcome();
    assert_eq!(outcome, StartupOutcome::Package);
    assert_eq!(outcome.message(), None);
    assert!(!marker.exists(), "o passo rodou na subida do pacote");
    assert!(!hangar_server::update::steps::registry_path(&service.state_dir).exists(), "nenhum passo registrado");
}

/// P-46: no checkout a subida aplica os passos não destrutivos pendentes e deixa os destrutivos para o
/// botão; um passo que falha vira aviso e o servidor segue subindo.
#[cfg(unix)]
#[test]
fn p46_a_checkout_applies_the_safe_steps_on_startup_and_a_failure_is_a_warning() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let root = dir.path().join("checkout");
    std::fs::create_dir_all(&home).unwrap();
    let docs = root.join("docs").join("atualizacoes");
    std::fs::create_dir_all(&docs).unwrap();
    let step = |id: &str, command: &str, proof: &Path, destructive: bool| std::fs::write(docs.join(format!("{id}.md")), format!(
        "---\nid: {id}\ntitulo: Passo {id}\ncomando: {command}\nprova: {}\ndestrutivo: {destructive}\n---\n\nTexto.\n", proof.display())).unwrap();
    let (safe, destructive) = (dir.path().join("seguro"), dir.path().join("destrutivo"));
    step("2026-01-01-seguro", &format!("mkdir {}", safe.display()), &safe, false);
    step("2026-01-02-destrutivo", &format!("mkdir {}", destructive.display()), &destructive, true);
    let service = service(&home, &root);
    let outcome = service.startup_outcome();
    assert_eq!(outcome, StartupOutcome::Applied(vec!["2026-01-01-seguro".into()]));
    assert!(safe.exists() && !destructive.exists(), "o destrutivo fica para o botão");

    step("2026-01-03-falha", "exit 3", &dir.path().join("nunca"), false);
    let outcome = service.startup_outcome();
    assert!(matches!(outcome, StartupOutcome::Failed(_)), "{outcome:?}");
    assert!(outcome.message().unwrap().ends_with("; o app segue subindo"), "{:?}", outcome.message());
    let registered = std::fs::read_to_string(hangar_server::update::steps::registry_path(&service.state_dir)).unwrap();
    assert!(registered.contains("2026-01-01-seguro") && !registered.contains("2026-01-03-falha"), "{registered}");
}

/// O instalador troca os arquivos do próprio servidor que está subindo: o passo fica para o botão.
#[cfg(windows)]
#[test]
fn b52_windows_defers_the_steps_that_run_the_installer() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let root = dir.path().join("checkout");
    std::fs::create_dir_all(&home).unwrap();
    let marker = dir.path().join("rodou");
    pending_step(&root, "comando_windows", &format!("powershell -ExecutionPolicy Bypass -File install.ps1 -Update && mkdir {}", marker.display()), &marker);
    let service = service(&home, &root);
    let outcome = service.startup_outcome();
    assert_eq!(outcome, StartupOutcome::Deferred);
    assert_eq!(outcome.message().as_deref(), Some("[hangar] passos de atualizacao pendentes que chamam o instalador ficam para o botao Atualizar"));
    assert!(!marker.exists(), "o passo do instalador rodou na subida");
    assert!(!hangar_server::update::steps::registry_path(&service.state_dir).exists(), "nenhum passo registrado");
}
