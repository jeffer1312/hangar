//! ROT-18: no Windows a troca do arquivo espera o leitor aberto sem `FILE_SHARE_DELETE` (o `PermissionError`
//! do `atomico.substituir` do Python) e, esgotadas as tentativas, devolve o erro da última.
#![cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;
use std::time::Duration;

/// Só leitura compartilhada: sem `FILE_SHARE_DELETE` nem `FILE_SHARE_WRITE`, como o CLI lendo o JSON.
const FILE_SHARE_READ: u32 = 1;

fn held(path: &std::path::Path) -> std::fs::File {
    std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(path).unwrap()
}

#[test]
fn replace_waits_for_a_reader_that_lets_go() {
    let dir = tempfile::tempdir().unwrap();
    let (target, source) = (dir.path().join("config.json"), dir.path().join("novo.json"));
    std::fs::write(&target, "velho").unwrap();
    std::fs::write(&source, "novo").unwrap();
    let reader = held(&target);
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        drop(reader);
    });
    hangar_core::fs::replace(&source, &target).unwrap();
    release.join().unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "novo");
    assert!(!source.exists());
}

#[test]
fn replace_gives_up_with_the_error_of_the_last_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let (target, source) = (dir.path().join("config.json"), dir.path().join("novo.json"));
    std::fs::write(&target, "velho").unwrap();
    std::fs::write(&source, "novo").unwrap();
    let reader = held(&target);
    let error = hangar_core::fs::replace(&source, &target).unwrap_err();
    drop(reader);
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied, "{error}");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "velho");
    assert_eq!(std::fs::read_to_string(&source).unwrap(), "novo");
}

/// O escritor da entrada do Computer Use (`write_json` → `fs_replace::replace`) também precisa
/// esperar o leitor: é o caminho do `.claude.json`.
#[test]
fn computer_use_style_replace_waits_for_a_reader_that_lets_go() {
    let dir = tempfile::tempdir().unwrap();
    let (target, source) = (dir.path().join(".claude.json"), dir.path().join("novo.json"));
    std::fs::write(&target, "{}").unwrap();
    std::fs::write(&source, "{\"a\": 1}").unwrap();
    let reader = held(&target);
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        drop(reader);
    });
    let result = hangar_core::fs_replace::replace(&source, &target, Some(b"{}"), None);
    release.join().unwrap();
    result.unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "{\"a\": 1}");
}
