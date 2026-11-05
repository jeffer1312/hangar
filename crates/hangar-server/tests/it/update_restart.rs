//! Reinício isolado: somente executáveis falsos, sem gerenciador de serviço real.
#![cfg(unix)]
use hangar_server::update::git_engine::Service;
#[test]
fn update_manual_restart_propagates_state_refusal_and_preserves_bytes() {
 let temp=tempfile::tempdir().unwrap();let service=Service::new(temp.path().join("home"),temp.path().join("repo"),temp.path().join("claude"),Default::default());
 std::fs::create_dir_all(&service.state_dir).unwrap();let path=service.state().path().to_path_buf();let raw=br#"{"ok":true,"extra":"\ud800"}"#;std::fs::write(&path,raw).unwrap();
 assert_eq!(service.restart_service("manual",8123).unwrap_err(),"update_state_invalid_unicode: texto contém surrogate Unicode isolado");assert_eq!(std::fs::read(path).unwrap(),raw);
}
