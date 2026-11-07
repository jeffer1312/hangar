use hangar_workspace::{Operation, execute};
use serde_json::{Value, json};
use std::{fs, process::Command};

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        // Config global ausente: gpgsign ou hooks da máquina não mudam o repositório montado pelo teste.
        .env("GIT_CONFIG_GLOBAL", std::env::temp_dir().join("hangar-test-sem-gitconfig"))
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

#[cfg(unix)]
#[test]
fn citation_found_elsewhere_through_a_symlink_never_reaches_git_internals() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::write(repo.join(".git/config"), "segredo").unwrap();
    std::os::unix::fs::symlink(".git", repo.join("gl")).unwrap();
    let jsonl = dir.path().join("t.jsonl");
    let cited = repo.join("gl/config");
    fs::write(
        &jsonl,
        json!({"cwd": repo, "text": format!("veja {}", cited.display())}).to_string() + "\n",
    )
    .unwrap();
    for write in [false, true] {
        let error = hangar_workspace::citations::resolve(&repo, hangar_workspace::citations::Transcript::File(&jsonl), "config", write).unwrap_err();
        assert_eq!(error.status, 403);
    }
}

#[test]
fn tilde_of_another_user_stays_relative_and_cannot_climb_out_of_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().join("sessao");
    fs::create_dir_all(&cwd).unwrap();
    let secret = dir.path().join("segredo.txt");
    fs::write(&secret, "x").unwrap();
    let process_cwd = std::env::current_dir().unwrap();
    let climb = "../".repeat(process_cwd.components().count() + 1);
    let path = format!("~ninguem/{climb}{}", secret.display().to_string().trim_start_matches('/'));
    let jsonl = dir.path().join("t.jsonl");
    fs::write(&jsonl, json!({"cwd": cwd, "text": path}).to_string() + "\n").unwrap();
    let error = hangar_workspace::citations::resolve(&cwd, hangar_workspace::citations::Transcript::File(&jsonl), &path, false).unwrap_err();
    assert_eq!(error.status, 403);
    let found = hangar_workspace::files::resolver(&cwd, std::slice::from_ref(&path), false).unwrap();
    assert!(found["ok"].as_object().unwrap().is_empty());
}

#[test]
fn remote_branch_named_like_an_option_is_not_passed_to_switch() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-b", "main"]);
    git(dir.path(), &["-c", "user.name=T", "-c", "user.email=t@t", "commit", "--allow-empty", "-m", "c"]);
    git(dir.path(), &["update-ref", "refs/remotes/origin/--detach", "HEAD"]);
    let request = serde_json::from_value(
        json!({"op":"switch_branch", "args":{"cwd":dir.path(),"branch":"--detach"}}),
    )
    .unwrap();
    assert_eq!(execute(request).unwrap_err().status, 400);
    let head = Command::new("git").arg("-C").arg(dir.path()).args(["symbolic-ref", "-q", "HEAD"]).output().unwrap();
    assert!(head.status.success(), "HEAD ficou destacado");
}

#[test]
fn empty_folder_path_means_the_root_like_python() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("projeto")).unwrap();
    let request = serde_json::from_value(json!({"op":"scan_dir", "args":{
        "root": dir.path(), "path": "", "roots": [dir.path()]}}))
    .unwrap();
    let listed = execute(request).unwrap();
    assert!(listed.to_string().contains("projeto"), "{listed}");
}

#[cfg(unix)]
#[test]
fn listed_symlink_shows_the_size_of_its_target() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("alvo.txt"), "1").unwrap();
    std::os::unix::fs::symlink(dir.path().join("alvo.txt"), dir.path().join("atalho.txt")).unwrap();
    let request = serde_json::from_value(json!({"op":"list_dir", "args":{
        "cwd": dir.path(), "path": null, "so_modificados": false}}))
    .unwrap();
    let listed = execute(request).unwrap();
    let link = listed["entries"].as_array().unwrap().iter().find(|e| e["name"] == "atalho.txt").unwrap().clone();
    assert_eq!(link["size"], 1);
}

#[test]
fn copy_entry_in_status_does_not_misread_its_source_as_an_entry() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["config", "user.name", "T"]);
    git(dir.path(), &["config", "user.email", "t@t"]);
    git(dir.path(), &["config", "status.renames", "copies"]);
    let lines = (1..=200).map(|n| n.to_string()).collect::<Vec<_>>().join("\n");
    fs::write(dir.path().join("R日本.txt"), &lines).unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "a"]);
    fs::write(dir.path().join("copia.txt"), &lines).unwrap();
    fs::write(dir.path().join("R日本.txt"), format!("{lines}\nmod\n")).unwrap();
    git(dir.path(), &["add", "."]);
    let done = op(dir.path(), "commit", json!({"message":"copia", "paths":["copia.txt"]}));
    assert_eq!(done["ok"], true);
}

/// Repositório recém-criado: o `diff HEAD` sai com erro, mas isso não é falha da consulta (nulo ia ao diário).
#[test]
fn diff_summary_of_repository_without_commits_is_empty_not_failed() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-b", "main"]);
    fs::write(dir.path().join("novo.txt"), "x\n").unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    assert_eq!(hangar_workspace::git::summary(Some(&cwd), true), json!({}));
    assert!(!hangar_workspace::git::summary(Some(&cwd), false).is_null());
}

#[test]
fn branches_say_which_ones_are_open_in_another_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("repo");
    fs::create_dir(&main).unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    git(&main, &["-c", "user.name=T", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "c"]);
    git(&main, &["branch", "livre"]);
    let other = dir.path().join("outra");
    git(&main, &["worktree", "add", "-q", "-b", "feat", other.to_str().unwrap()]);

    let from_main = op(&main, "list_branches", json!({}));
    let from_main_path = std::path::Path::new(from_main["elsewhere"]["feat"]["path"].as_str().unwrap());
    assert!(from_main_path.is_absolute());
    assert_eq!(hangar_workspace::real(from_main_path), hangar_workspace::real(&other));
    assert_eq!(from_main["elsewhere"]["feat"]["main"], json!(false));
    assert!(from_main["elsewhere"].get("main").is_none(), "a própria pasta não é outra worktree");
    assert!(from_main["elsewhere"].get("livre").is_none());

    let from_other = op(&other, "list_branches", json!({}));
    let from_other_path = std::path::Path::new(from_other["elsewhere"]["main"]["path"].as_str().unwrap());
    assert!(from_other_path.is_absolute());
    assert_eq!(hangar_workspace::real(from_other_path), hangar_workspace::real(&main));
    assert_eq!(from_other["elsewhere"]["main"]["main"], json!(true));
    assert!(from_other["elsewhere"].get("feat").is_none());
}

/// A pasta-mãe da worktree nova é a da pasta principal, também quando a sessão parte de outra worktree.
#[test]
fn worktree_parent_is_named_after_the_main_folder() {
    let dir = tempfile::tempdir().unwrap();
    let root = hangar_workspace::simplify(dir.path().canonicalize().unwrap());
    let main = root.join("repo");
    fs::create_dir(&main).unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    git(&main, &["-c", "user.name=T", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "c"]);
    let other = root.join("outra");
    git(&main, &["worktree", "add", "-q", "-b", "feat", other.to_str().unwrap()]);
    let location = root.join("worktrees");
    let expected = location.join("repo").to_string_lossy().into_owned();
    for cwd in [&main, &other] {
        assert_eq!(hangar_workspace::git::worktree_parent(cwd, &root, &location), Some(expected.clone()));
    }
    assert_eq!(hangar_workspace::git::worktree_parent(&root, &root, &location), None, "fora de repositório não há pasta");
}

/// Desfazer a worktree apaga só a branch local que a criação fez (a nova e a trazida do
/// remoto com `--track -b`), nunca a que já existia; a pasta ocupada ganha `-2`.
#[test]
fn undoing_a_worktree_deletes_only_the_branch_it_created() {
    use hangar_workspace::git::{create_branch_worktree, undo_worktree};
    let base = tempfile::tempdir().unwrap();
    // Sem o prefixo `\\?\` do Windows: o git não cria pasta por ele.
    let root = hangar_workspace::simplify(base.path().canonicalize().unwrap());
    let origin = root.join("origin");
    fs::create_dir_all(&origin).unwrap();
    git(&origin, &["init", "-q", "-b", "main"]);
    fs::write(origin.join("a.txt"), "a\n").unwrap();
    git(&origin, &["add", "."]);
    git(&origin, &["-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "-m", "a"]);
    git(&origin, &["branch", "remota"]);
    git(&root, &["clone", "-q", "origin", "repo"]);
    let repo = root.join("repo");
    let worktrees = root.join("worktrees");
    let local = |name: &str| Command::new("git").arg("-C").arg(&repo).args(["rev-parse", "--verify", "--quiet", &format!("refs/heads/{name}")])
        .status().unwrap().success();
    let made = create_branch_worktree(&repo, "remota", "remota", &root, false, None, &worktrees).unwrap();
    assert!(made.created && made.branch_created && local("remota"), "{made:?}");
    undo_worktree(&repo, &made, "remota").unwrap();
    assert!(!local("remota") && !worktrees.join("repo/remota").exists(), "a branch trazida do remoto sai junto");
    git(&repo, &["branch", "dev"]);
    let made = create_branch_worktree(&repo, "dev", "dev", &root, false, None, &worktrees).unwrap();
    assert!(made.created && !made.branch_created);
    undo_worktree(&repo, &made, "dev").unwrap();
    assert!(local("dev"), "a branch que já existia fica");
    fs::create_dir_all(worktrees.join("repo/nova")).unwrap();
    let made = create_branch_worktree(&repo, "nova", "nova", &root, true, None, &worktrees).unwrap();
    assert!(std::path::Path::new(&made.path) == worktrees.join("repo/nova-2") && made.branch_created);
    undo_worktree(&repo, &made, "nova").unwrap();
    assert!(!local("nova"));
}

/// Branch local atrás da remota: a lista diz quanto, e a worktree nova parte da remota sem herdar o upstream dela.
#[test]
fn new_worktree_starts_from_the_upstream_of_a_stale_branch() {
    use hangar_workspace::git::create_branch_worktree;
    let base = tempfile::tempdir().unwrap();
    let root = hangar_workspace::simplify(base.path().canonicalize().unwrap());
    let origin = root.join("origin");
    fs::create_dir_all(&origin).unwrap();
    let commit = |dir: &std::path::Path, message: &str| git(dir, &["-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "--allow-empty", "-m", message]);
    git(&origin, &["init", "-q", "-b", "main"]);
    commit(&origin, "a");
    git(&root, &["clone", "-q", "origin", "repo"]);
    let repo = root.join("repo");
    commit(&origin, "remoto");
    commit(&repo, "local");
    git(&repo, &["branch", "sem-remota"]);
    git(&repo, &["fetch", "-q"]);

    let info = op(&repo, "list_branches", json!({}));
    assert_eq!(info["upstreams"], json!({"main": {"upstream": "origin/main", "ahead": 1, "behind": 1}}));

    // A outra remota termina no mesmo nome: a base escolhida continua sendo a de `origin`.
    git(&repo, &["update-ref", "refs/remotes/other/origin/main", "HEAD"]);
    let worktrees = root.join("worktrees");
    let made = create_branch_worktree(&repo, "nova", "nova", &root, true, Some("origin/main"), &worktrees).unwrap();
    let read = |args: &[&str]| Command::new("git").arg("-C").arg(&made.path).args(args).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&read(&["log", "-1", "--format=%s"]).stdout).trim(), "remoto");
    assert!(!read(&["rev-parse", "--abbrev-ref", "@{upstream}"]).status.success(), "o pull não pode puxar a base");

    for base in ["origin/nada", "origin/main~1"] {
        let refused = create_branch_worktree(&repo, "outra", "outra", &root, true, Some(base), &worktrees).unwrap_err();
        assert_eq!(refused.detail, "branch base inexistente");
    }
}
