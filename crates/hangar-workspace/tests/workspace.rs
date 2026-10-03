use hangar_workspace::{Operation, execute};
use serde_json::{Value, json};
use std::{fs, process::Command};

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn op(cwd: &std::path::Path, name: &str, args: Value) -> Value {
    let mut args = args.as_object().unwrap().clone();
    args.insert("cwd".into(), json!(cwd.to_string_lossy()));
    execute(serde_json::from_value::<Operation>(json!({"op": name, "args": args})).unwrap())
        .unwrap()
}

#[test]
fn selected_commit_keeps_other_staged_files_and_read_digest_guards_save() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-b", "main"]);
    git(dir.path(), &["config", "user.name", "Teste"]);
    git(
        dir.path(),
        &["config", "user.email", "teste@example.invalid"],
    );
    fs::write(dir.path().join("ação.txt"), "primeira\n").unwrap();
    fs::write(dir.path().join("outro.txt"), "outro\n").unwrap();
    git(dir.path(), &["add", "."]);
    op(
        dir.path(),
        "commit",
        json!({"message":"Inicial", "paths":["ação.txt"]}),
    );
    assert!(
        op(dir.path(), "changed_files", json!({}))
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["path"] == "outro.txt" && f["staged"] == true)
    );
    let read = op(dir.path(), "read_file", json!({"path":"ação.txt"}));
    let write = op(
        dir.path(),
        "write_file",
        json!({"path":"ação.txt", "texto":"segunda\n", "digest_lido":read["digest"]}),
    );
    assert_ne!(read["digest"], write["digest"]);
    let request = serde_json::from_value(json!({"op":"write_file", "args":{"cwd":dir.path(), "path":"ação.txt", "texto":"perdida", "digest_lido":read["digest"]}})).unwrap();
    assert_eq!(execute(request).unwrap_err().status, 409);
    assert_eq!(
        fs::read_to_string(dir.path().join("ação.txt")).unwrap(),
        "segunda\n"
    );
}

#[test]
fn paths_and_binary_reads_are_refused_without_changing_disk() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join(".git")).unwrap();
    fs::write(dir.path().join(".git/config"), "segredo").unwrap();
    fs::write(dir.path().join("bin.dat"), "texto\0binário".as_bytes()).unwrap();
    for (path, status) in [("../fora", 400), (".git/config", 403), ("bin.dat", 415)] {
        let request = serde_json::from_value(
            json!({"op":"read_file", "args":{"cwd":dir.path(),"path":path}}),
        )
        .unwrap();
        assert_eq!(execute(request).unwrap_err().status, status);
    }
}
