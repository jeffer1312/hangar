/// Um filho de `fork` ainda antes do `exec` (outro teste, ou o servidor lançando um processo) herda o
/// descritor da trava: fechar só o do pai a deixava presa até o `exec`.
#[cfg(target_os = "linux")]
#[test]
fn queue_lease_drop_releases_while_a_pre_exec_child_holds_the_descriptor() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().canonicalize().unwrap().join("key.lock");
    let lease = hangar_server::runtime::queue::acquire_lease(&path).unwrap();
    let child = crate::accounts_catalog::PreExecGuardChild::spawn(&path, false);
    drop(lease);
    let again = hangar_server::runtime::queue::acquire_lease(&path);
    assert!(again.is_ok(), "a trava solta no pai seguiu presa pelo descritor herdado: {:?}", again.err());
    drop(again);
    drop(child);
}
