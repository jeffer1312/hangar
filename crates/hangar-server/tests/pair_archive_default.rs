//! B-11: sem `HANGAR_PAIR_ARCHIVE` (o Python não está mais na frente para defini-la), os contratos de
//! grupo arquivados ficam em `~/.hangar/pair-arquivo`, o cofre do Hangar, e não na conta
//! (`pair.py:66-68`, `rust_server.py:229-230`). Processo próprio: tira a variável do processo inteiro.
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use hangar_server::list::bridge::{ListBridge, ListEnv, parse_dirs};

fn no_archive_variable() {
    // Só este binário de teste mexe no ambiente, antes de qualquer leitura.
    unsafe { std::env::remove_var("HANGAR_PAIR_ARCHIVE") }
}

fn dirs(home: &Path) -> hangar_server::list::discover_other::Dirs {
    parse_dirs(&serde_json::json!({"home": home, "claude": home.join(".claude"), "codex_home": home.join(".codex"),
        "pi_sessions": home.join(".pi/agent/sessions"), "omp_config": home.join(".omp"),
        "omp_agent": home.join(".omp/agent"), "kimi_home": home.join(".kimi-code")}).to_string()).unwrap()
}

/// O serviço de grupos do processo (`groups::from_env`) arquiva o contrato em `~/.hangar/pair-arquivo`.
#[tokio::test]
async fn b11_group_contracts_are_archived_under_the_hangar_vault_by_default() {
    no_archive_variable();
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let dirs = dirs(&home);
    let pair = dirs.claude.join(".hangar-pair");
    std::fs::create_dir_all(&pair).unwrap();
    std::fs::write(pair.join("grupo-g1.md"), "regras do grupo").unwrap();
    let list = Arc::new(ListBridge::new(ListEnv { mux: hangar_server::list::mux::Mux::with_program("missing-test-mux", Duration::from_secs(5)),
        capture_program: "missing-test-mux".into(), procs: Arc::new(hangar_server::list::procs::SystemProcs::default()), dirs: Some(dirs.clone()) },
        hangar_server::list::facts::FactsClient::new()));
    let groups = hangar_server::groups::from_env(Some(&dirs), "maquina".into(), Arc::new(hangar_server::groups::orq::OrqVault::new()),
        list, hangar_server::diag::DiagClient::new(), Arc::new(hangar_mission::Store::new(home.join(".hangar").join("mission")))).unwrap();
    groups.archive_contracts("g1").await.unwrap();
    assert!(!pair.join("grupo-g1.md").exists(), "o contrato saiu do .hangar-pair");
    let archived: Vec<String> = std::fs::read_dir(home.join(".hangar").join("pair-arquivo")).unwrap().flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(archived.len(), 1, "{archived:?}");
    assert!(archived[0].starts_with("grupo-g1-") && archived[0].ends_with(".md"), "{archived:?}");
    assert_eq!(std::fs::read_to_string(home.join(".hangar").join("pair-arquivo").join(&archived[0])).unwrap(), "regras do grupo");
}

/// A orquestração lê o mesmo arquivo para saber que um grupo acabou (`orchestration::context::ended`).
#[test]
fn b11_orchestration_reads_the_default_archive_to_know_a_group_ended() {
    no_archive_variable();
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let pair = home.join(".claude").join(".hangar-pair");
    std::fs::create_dir_all(&pair).unwrap();
    assert!(!hangar_server::orchestration::context::ended(&home, &pair, "g2"), "sem arquivo, o grupo não acabou");
    let archive = home.join(".hangar").join("pair-arquivo");
    std::fs::create_dir_all(&archive).unwrap();
    std::fs::write(archive.join("regras-g2-20261116-120000.md"), "x").unwrap();
    assert!(hangar_server::orchestration::context::ended(&home, &pair, "g2"));
}
