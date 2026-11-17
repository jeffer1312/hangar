//! P5: a porta do `deploy.sh` sai do Rust, na mesma ordem do `settings.port` do Python: ambiente,
//! `.env`, 8765; valor que não é porta é erro.
use hangar_deploy::port;

#[test]
fn the_environment_wins_then_the_env_file_then_the_default() {
    let dir = tempfile::tempdir().unwrap();
    let (home, backend) = (dir.path().join("home"), dir.path().join("checkout/backend"));
    std::fs::create_dir_all(&backend).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    assert_eq!(port(&home, Some(&backend), None), Ok(8765), "sem nada: o padrão");
    std::fs::write(backend.join(".env"), "CP_AUTH_TOKEN=t\nCP_PORT=8800\n").unwrap();
    assert_eq!(port(&home, Some(&backend), None), Ok(8800), "máquina não migrada: o .env da backend/");
    assert_eq!(port(&home, Some(&backend), Some("9000")), Ok(9000), "o ambiente vence o .env");
    let config = home.join(".hangar/config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join(".env"), "CP_PORT=8900\n").unwrap();
    assert_eq!(port(&home, Some(&backend), None), Ok(8900), "máquina migrada: o .env dos dados do usuário");
}

#[test]
fn a_value_that_is_not_a_port_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    for bad in ["", "abc", "0", "70000", "-1"] {
        assert!(port(dir.path(), None, Some(bad)).is_err(), "{bad:?}");
    }
    let backend = dir.path().join("backend");
    std::fs::create_dir_all(&backend).unwrap();
    std::fs::write(backend.join(".env"), "CP_PORT=porta\n").unwrap();
    assert!(port(dir.path(), Some(&backend), None).is_err());
}
