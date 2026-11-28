//! A marca de filha sem pai: gravar, tirar e renomear sem apagar as outras, e nunca por cima do ilegível.
use hangar_delegation::orphans::{Orphan, clear, path, read, record, rename};

fn names(dir: &std::path::Path) -> Vec<String> { read(dir).unwrap().into_keys().collect() }

#[test]
fn record_marks_each_child_with_its_parent_and_keeps_the_others() {
    let dir = tempfile::tempdir().unwrap();
    assert!(read(dir.path()).unwrap().is_empty(), "sem arquivo é vazio");
    record(dir.path(), "pai", &["a".to_owned()], 10.5).unwrap();
    record(dir.path(), "outro", &["b".to_owned(), "c".to_owned()], 20.0).unwrap();
    let raw: serde_json::Value = serde_json::from_slice(&std::fs::read(path(dir.path())).unwrap()).unwrap();
    assert_eq!(raw, serde_json::json!({"a": {"parent": "pai", "at": 10.5}, "b": {"parent": "outro", "at": 20.0},
        "c": {"parent": "outro", "at": 20.0}}));
    // A mesma filha órfã de novo: vale o pai mais recente.
    record(dir.path(), "novo", &["a".to_owned()], 30.0).unwrap();
    assert_eq!(read(dir.path()).unwrap()["a"], Orphan { parent: "novo".into(), at: 30.0 });
}

#[test]
fn record_without_children_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    record(dir.path(), "pai", &[], 1.0).unwrap();
    assert!(!path(dir.path()).exists());
}

#[test]
fn clear_takes_only_the_named_children_out() {
    let dir = tempfile::tempdir().unwrap();
    clear(dir.path(), &["a".to_owned()]).unwrap();
    assert!(!path(dir.path()).exists(), "nada a tirar não cria o arquivo");
    record(dir.path(), "pai", &["a".to_owned(), "b".to_owned(), "c".to_owned()], 1.0).unwrap();
    clear(dir.path(), &["a".to_owned(), "c".to_owned(), "sumiu".to_owned()]).unwrap();
    assert_eq!(names(dir.path()), ["b"]);
}

#[test]
fn rename_moves_the_mark_to_the_new_name() {
    let dir = tempfile::tempdir().unwrap();
    record(dir.path(), "pai", &["a".to_owned(), "b".to_owned()], 2.0).unwrap();
    rename(dir.path(), "a", "a2").unwrap();
    rename(dir.path(), "sumiu", "x").unwrap();
    assert_eq!(names(dir.path()), ["a2", "b"]);
    assert_eq!(read(dir.path()).unwrap()["a2"], Orphan { parent: "pai".into(), at: 2.0 });
}

#[test]
fn rename_drops_a_stale_mark_left_on_the_new_name() {
    let dir = tempfile::tempdir().unwrap();
    record(dir.path(), "pai", &["velha".to_owned(), "b".to_owned()], 3.0).unwrap();
    // O nome antigo sem marca: a do nome novo era de outra vida e sai.
    rename(dir.path(), "sem-marca", "velha").unwrap();
    assert_eq!(names(dir.path()), ["b"]);
    // Com marca: ela vence a que o nome novo tinha.
    record(dir.path(), "outro", &["velha".to_owned()], 4.0).unwrap();
    rename(dir.path(), "b", "velha").unwrap();
    assert_eq!(names(dir.path()), ["velha"]);
    assert_eq!(read(dir.path()).unwrap()["velha"], Orphan { parent: "pai".into(), at: 3.0 });
}

#[test]
fn an_unreadable_file_is_an_error_and_is_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(path(dir.path()), "{torto").unwrap();
    assert!(read(dir.path()).is_err());
    assert!(record(dir.path(), "pai", &["a".to_owned()], 1.0).is_err());
    assert!(clear(dir.path(), &["a".to_owned()]).is_err());
    assert!(rename(dir.path(), "a", "b").is_err());
    assert_eq!(std::fs::read_to_string(path(dir.path())).unwrap(), "{torto");
}
