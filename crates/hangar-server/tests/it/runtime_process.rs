//! Módulo comum de processo do cano. Só Linux: os testes usam `/bin/sleep` e `/proc`.
//! Nada aqui pode tocar processo real da máquina: chave e dono são únicos por execução.
#![cfg(target_os = "linux")]

use hangar_server::runtime::process::*;
use std::collections::HashSet;

fn unique_key() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64;
    format!("{:016x}0000", nanos ^ ((std::process::id() as u64) << 32))
}

#[tokio::test]
async fn spawn_listens_and_kill_ends_the_group() {
    let dir = tempfile::tempdir().unwrap();
    crate::use_cano_bin();
    let key = unique_key();
    let owner = dir.path().to_string_lossy().into_owned();
    let mut env: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| k != "HANGAR_CANO_KEY" && k != "HANGAR_CANO_OWNER").collect();
    env.push(("HANGAR_CANO_KEY".into(), key.clone()));
    env.push(("HANGAR_CANO_OWNER".into(), owner));
    let spec = LaunchSpec { provider: Provider::Codex, key: key.clone(), cwd: dir.path().into(),
        program: vec!["sleep".into(), "300".into()], env,
        cano_extra: serde_json::Map::from_iter([("config_marca".into(), serde_json::json!("m1"))]),
        sidecar_dir: dir.path().into() };
    let cano = spawn(&spec).await.unwrap();
    assert_eq!(cano.versao, 2);
    assert_eq!(cano.extra["config_marca"], "m1");
    assert!(cano.escuta.starts_with("unix:"));
    assert!(matches!(liveness(cano.pid, &spec.key), Liveness::Ours));
    let log = dir.path().join(format!("cano-{}.log", &key[..16]));
    assert!(log.exists());
    let children = std::fs::read_to_string(format!("/proc/{0}/task/{0}/children", cano.pid)).unwrap();
    let agent: u32 = children.split_whitespace().next().expect("o cano sobe o programa").parse().unwrap();
    kill(&cano, &spec.key, &spec.sidecar_dir).await.unwrap();
    assert!(matches!(liveness(cano.pid, &spec.key), Liveness::Dead));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::path::Path::new(&format!("/proc/{agent}/cmdline")).exists()
        && !std::fs::read(format!("/proc/{agent}/cmdline")).unwrap_or_default().is_empty() {
        assert!(std::time::Instant::now() < deadline, "o programa do cano sobreviveu ao kill do grupo");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(!log.exists());
    assert!(std::fs::read_dir(dir.path()).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().starts_with("cano-")));
}

#[test]
fn a_live_pid_of_another_program_is_foreign() {
    let mut other = std::process::Command::new("/bin/sleep").arg("300").spawn().unwrap();
    assert!(matches!(liveness(other.id(), &unique_key()), Liveness::Foreign));
    other.kill().unwrap();
    other.wait().unwrap();
}

#[tokio::test]
async fn kill_refuses_a_reused_pid() {
    let mut other = std::process::Command::new("/bin/sleep").arg("300").spawn().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let key = unique_key();
    let other_session = dir.path().join("cano-outrasessao0000-ab12.sock");
    std::fs::write(&other_session, "").unwrap();
    let cano = Cano { pid: other.id(), escuta: "unix:/x".into(), token: "t".into(), ts: 0.0, versao: 2,
        extra: Default::default() };
    assert!(kill(&cano, &key, dir.path()).await.is_ok()); // `Foreign`: não mata
    assert!(other.try_wait().unwrap().is_none());
    // Chave curta não pode varrer o socket de outra sessão.
    assert!(kill(&cano, "", dir.path()).await.is_ok());
    assert!(other_session.exists());
    other.kill().unwrap();
    other.wait().unwrap();
}

#[test]
fn orphans_are_processes_with_a_dead_key_and_our_owner() {
    let owner_dir = tempfile::tempdir().unwrap();
    let owner = owner_dir.path().to_string_lossy().into_owned();
    let spawn = |key: &str, owner_var: Option<&str>, home: &str| {
        let mut cmd = std::process::Command::new("/bin/sleep");
        cmd.arg("300").env("HANGAR_CANO_KEY", key).env("HOME", home).env_remove("HANGAR_CANO_OWNER");
        if let Some(o) = owner_var { cmd.env("HANGAR_CANO_OWNER", o); }
        cmd.spawn().unwrap()
    };
    let mut ours = spawn(&unique_key(), Some(&owner), "/nao-e-o-dono");
    let mut legacy = spawn(&unique_key(), None, &owner); // cano antigo: só o HOME prova o dono
    let live_key = unique_key();
    let mut alive = spawn(&live_key, Some(&owner), "/x");
    let mut foreign = spawn(&unique_key(), Some("/outro-dono"), &owner);
    std::thread::sleep(std::time::Duration::from_millis(100)); // o exec do sleep já trocou o environ
    assert_eq!(kill_orphans(&HashSet::from([live_key]), &owner), 2);
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(ours.wait().unwrap().signal(), Some(libc::SIGTERM));
    assert_eq!(legacy.wait().unwrap().signal(), Some(libc::SIGTERM));
    assert!(alive.try_wait().unwrap().is_none());
    assert!(foreign.try_wait().unwrap().is_none());
    for c in [&mut alive, &mut foreign] { c.kill().unwrap(); c.wait().unwrap(); }
}

#[test]
fn startup_sweep_keeps_the_keys_of_both_session_folders() {
    let home = tempfile::tempdir().unwrap();
    let owner = home.path().to_string_lossy().into_owned();
    let (claude, codex) = (home.path().join("claude-headless"), home.path().join("codex-sessions"));
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::create_dir_all(&codex).unwrap();
    let (claude_key, codex_key, terminal_key, gone_key) = (unique_key() + "a", unique_key() + "b", unique_key() + "c", unique_key() + "d");
    std::fs::write(claude.join("s1.json"), serde_json::json!({"name":"s1","key":claude_key}).to_string()).unwrap();
    std::fs::write(codex.join("c1.json"), serde_json::json!({"name":"c1","key":codex_key,"headless":true}).to_string()).unwrap();
    // Codex com terminal não tem cano: a chave dele não protege processo nenhum.
    std::fs::write(codex.join("c2.json"), serde_json::json!({"name":"c2","key":terminal_key,"headless":false}).to_string()).unwrap();
    let spawn = |key: &str| std::process::Command::new("/bin/sleep").arg("300").env("HANGAR_CANO_KEY", key)
        .env("HANGAR_CANO_OWNER", &owner).spawn().unwrap();
    let mut children: Vec<_> = [&claude_key, &codex_key, &terminal_key, &gone_key].into_iter().map(|key| spawn(key)).collect();
    std::thread::sleep(std::time::Duration::from_millis(100));
    // Pasta ilegível: nada é varrido, porque a sessão viva dela pareceria órfã.
    assert_eq!(sweep_orphans(&claude.join("s1.json"), &codex, &owner), None);
    assert!(children.iter_mut().all(|child| child.try_wait().unwrap().is_none()));
    assert_eq!(sweep_orphans(&claude, &codex, &owner), Some(2));
    use std::os::unix::process::ExitStatusExt;
    assert!(children[0].try_wait().unwrap().is_none() && children[1].try_wait().unwrap().is_none());
    assert_eq!(children[2].wait().unwrap().signal(), Some(libc::SIGTERM));
    assert_eq!(children[3].wait().unwrap().signal(), Some(libc::SIGTERM));
    for child in &mut children[..2] { child.kill().unwrap(); child.wait().unwrap(); }
}

#[test]
fn startup_sweep_keeps_the_keys_of_a_transfer_in_progress() {
    let home = tempfile::tempdir().unwrap();
    let owner = home.path().to_string_lossy().into_owned();
    let (claude, codex) = (home.path().join("claude-headless"), home.path().join("codex-sessions"));
    std::fs::create_dir_all(codex.join("prepared")).unwrap();
    // Transferência para Codex sem terminal em curso: o processo novo só aparece em `prepared/`.
    let prepared_key = unique_key() + "e";
    std::fs::write(codex.join("prepared").join("t1.json"),
        serde_json::json!({"name":"c1","key":prepared_key,"headless":true,"transfer_id":"t1"}).to_string()).unwrap();
    let mut child = std::process::Command::new("/bin/sleep").arg("300").env("HANGAR_CANO_KEY", &prepared_key)
        .env("HANGAR_CANO_OWNER", &owner).spawn().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert_eq!(sweep_orphans(&claude, &codex, &owner), Some(0));
    assert!(child.try_wait().unwrap().is_none(), "o processo da transferência em curso fica");
    child.kill().unwrap(); child.wait().unwrap();
}
