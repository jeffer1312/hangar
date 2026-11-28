use super::*;

fn args(line: &str) -> Vec<String> { line.split(' ').map(str::to_owned).collect() }

/// Servidor de sessão vivo sem a ficha `<nome>.port` é órfão; o que tem ficha, o de reserva e o de outro socket não.
#[test]
fn a_session_server_without_its_registry_file_is_an_orphan() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("viva.port"), "1\n").unwrap();
    let found = orphans(dir.path(), [
        (10, args(r"C:\bin\tmux.exe server -s hangar -c claude")),
        (11, args(r"C:\bin\tmux.exe server -s viva -c claude")),
        (12, args(r"C:\bin\tmux.exe server -s __warm__")),
        (13, args(r"C:\bin\tmux.exe server -s teste -L sock")),
        (14, args(r"C:\bin\claude.exe server -s outro")),
        (15, args(r"C:\bin\psmux.exe server -s PM-1")),
    ]).unwrap();
    assert_eq!(found, vec![Orphan { name: "hangar".into(), pid: 10 }, Orphan { name: "PM-1".into(), pid: 15 }]);
}
