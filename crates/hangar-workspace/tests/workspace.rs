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

mod task3_contract {
    use super::*;

    // Literais do helper original08db6bc92/9de832d1; os testes não dependem do cache de captura.
    const ORIGINAL_35: &str = r####"[{"id":"G01-head-info","group":"git_read","request":{"op":"head_info","args":{}},"response":{"ok":true,"result":["main",false]},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"G02-branch-of","group":"git_read","request":{"op":"branch_of","args":{}},"response":{"ok":true,"result":"main"},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"G03-git-summary","group":"git_read","request":{"op":"git_summary","args":{}},"response":{"ok":true,"result":{"dirty":2,"ahead":null,"behind":null}},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"G04-git-diffstat","group":"git_read","request":{"op":"git_diffstat","args":{}},"response":{"ok":true,"result":{"added":1,"removed":1}},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"G05-list-branches","group":"git_read","request":{"op":"list_branches","args":{}},"response":{"ok":true,"result":{"current":"main","branches":["main"],"remotes":[],"dirty":true,"elsewhere":{}}},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"G06-changed-files","group":"git_read","request":{"op":"changed_files","args":{}},"response":{"ok":true,"result":[{"path":"sub/ação.txt","code":" M","staged":false,"added":1,"removed":1},{"path":"novo.txt","code":"??","staged":false,"added":null,"removed":null}]},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"G07-git-log","group":"git_read","request":{"op":"git_log","args":{"n":50,"grep":null}},"response":{"ok":true,"result":[{"hash":"20bc9e7267b8e5a4a32370c1855c4db636e589e6","short":"20bc9e7","parents":[],"refs":"HEAD -> main","author":"Teste","ts":1577836800,"rel":"7 years ago","subject":"Inicial","body":"","local":false}]},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"G08-last-commit-message","group":"git_read","request":{"op":"last_commit_message","args":{}},"response":{"ok":true,"result":{"message":"Inicial"}},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"G09-folder-status","group":"git_read","request":{"op":"folder_status","args":{}},"response":{"ok":true,"result":{"dirty":2,"ahead":null,"behind":null,"repo":true,"current":"main","upstream":null,"toplevel":"/home/jefferson/.cache/hangar-workspace-port/contract-driver/task3-original-35.rsCOWlsU/scenes/G09-folder-status/repo","last_fetch":null}},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"G10-sequencer-state","group":"git_read","request":{"op":"sequencer_state","args":{}},"response":{"ok":true,"result":null},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"G11-file-diff-tracked","group":"git_read","request":{"op":"file_diff","args":{"path":"sub/ação.txt"}},"response":{"ok":true,"result":{"path":"sub/ação.txt","diff":"diff --git \"a/sub/a\\303\\247\\303\\243o.txt\" \"b/sub/a\\303\\247\\303\\243o.txt\"\nindex 512146f..ef2305a 100644\n--- \"a/sub/a\\303\\247\\303\\243o.txt\"\n+++ \"b/sub/a\\303\\247\\303\\243o.txt\"\n@@ -1,2 +1,2 @@\n primeira\n-linha\n+linha mudada\n","truncated":false}},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"G12-file-diff-untracked","group":"git_read","request":{"op":"file_diff","args":{"path":"novo.txt"}},"response":{"ok":true,"result":{"path":"novo.txt","diff":"diff --git a/novo.txt b/novo.txt\nnew file mode 100644\nindex 0000000..745c683\n--- /dev/null\n+++ b/novo.txt\n@@ -0,0 +1 @@\n+novo\n","truncated":false}},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"G13-path-diff-uncommitted","group":"git_read","request":{"op":"path_diff","args":{"path":"sub/ação.txt","escopo":"nao_commitado"}},"response":{"ok":true,"result":{"path":"sub/ação.txt","diff":"diff --git a/sub/ação.txt b/sub/ação.txt\nindex 512146f..ef2305a 100644\n--- a/sub/ação.txt\n+++ b/sub/ação.txt\n@@ -1,2 +1,2 @@\n primeira\n-linha\n+linha mudada\n","truncated":false,"original":"primeira\nlinha\n","escopo_pedido":"nao_commitado","escopo_usado":"nao_commitado","base":null,"motivo":null}},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"G14-path-diff-branch","group":"git_read","request":{"op":"path_diff","args":{"path":"sub/ação.txt","escopo":"branch"}},"response":{"ok":true,"result":{"path":"sub/ação.txt","diff":"diff --git a/sub/ação.txt b/sub/ação.txt\nindex 512146f..ef2305a 100644\n--- a/sub/ação.txt\n+++ b/sub/ação.txt\n@@ -1,2 +1,2 @@\n primeira\n-linha\n+linha mudada\n","truncated":false,"original":"primeira\nlinha\n","escopo_pedido":"branch","escopo_usado":"nao_commitado","base":null,"motivo":"arq_motivo_sem_base_conhecida"}},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"F01-list-root","group":"file_read","request":{"op":"list_dir","args":{"path":null,"so_modificados":false}},"response":{"ok":true,"result":{"entries":[{"name":"sub","path":"sub","is_dir":true,"size":0,"changed":"M","add":1,"del":1},{"name":"novo.txt","path":"novo.txt","is_dir":false,"size":5,"changed":"?","add":0,"del":0},{"name":"outro.txt","path":"outro.txt","is_dir":false,"size":6,"changed":null,"add":0,"del":0}],"truncated":false}},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"F02-list-modified-sub","group":"file_read","request":{"op":"list_dir","args":{"path":"sub","so_modificados":true}},"response":{"ok":true,"result":{"entries":[{"name":"ação.txt","path":"sub/ação.txt","is_dir":false,"size":22,"changed":"M","add":1,"del":1}],"truncated":false}},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"F03-read-file","group":"file_read","request":{"op":"read_file","args":{"path":"sub/ação.txt"}},"response":{"ok":true,"result":{"path":"sub/ação.txt","text":"primeira\nlinha mudada\n","size":22,"truncated":false,"digest":"0fe20587a6b919286e72d57b501eec339f1afdeeef022141171485dbac529e26"}},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"F04-search-names","group":"file_read","request":{"op":"search","args":{"q":"ação","mode":"names"}},"response":{"ok":true,"result":{"hits":[{"path":"sub/ação.txt","line":null,"text":null}],"truncated":false,"mode":"names"}},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"F05-search-contents","group":"file_read","request":{"op":"search","args":{"q":"primeira","mode":"contents"}},"response":{"ok":true,"result":{"hits":[{"path":"sub/ação.txt","line":1,"text":"primeira"}],"truncated":false,"mode":"contents"}},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"F06-resolver","group":"file_read","request":{"op":"resolver","args":{"caminhos":["ação.txt","sub/ação.txt","ausente"],"suffix":true}},"response":{"ok":true,"result":{"ok":{"ação.txt":{"relativo":"sub/ação.txt","real":"/home/jefferson/.cache/hangar-workspace-port/contract-driver/task3-original-35.rsCOWlsU/scenes/F06-resolver/repo/sub/ação.txt"},"sub/ação.txt":{"relativo":"sub/ação.txt","real":"/home/jefferson/.cache/hangar-workspace-port/contract-driver/task3-original-35.rsCOWlsU/scenes/F06-resolver/repo/sub/ação.txt"}},"faltam":["ausente"]}},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"M01-commit-selected","group":"mutation","request":{"op":"commit","args":{"message":"Selecionado","paths":["sub/ação.txt"],"amend":false,"new_branch":null}},"response":{"ok":true,"result":{"ok":true,"output":"[main a332a7e] Selecionado\n 1 file changed, 1 insertion(+), 1 deletion(-)"}},"effects":{"commands":[{"exit":0,"stdout":"?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 ef2305a0357501b0249fca16f547e8c35a84fc37 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/feature\na332a7e48eb1257e1f185d64844f686b4e925e84 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\na332a7e48eb1257e1f185d64844f686b4e925e84 Selecionado\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"M02-commit-amend","group":"mutation","request":{"op":"commit","args":{"message":"Reescrito","paths":[],"amend":true,"new_branch":null}},"response":{"ok":true,"result":{"ok":true,"output":"[main e3ccc75] Reescrito\n Date: Wed Jan 1 00:00:00 2020 +0000\n 2 files changed, 3 insertions(+)\n create mode 100644 outro.txt\n create mode 100644 \"sub/a\\303\\247\\303\\243o.txt\""}},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/feature\ne3ccc7544380580d0959dced7ff76076f46d7289 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\ne3ccc7544380580d0959dced7ff76076f46d7289 Reescrito\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"M03-commit-new-branch","group":"mutation","request":{"op":"commit","args":{"message":"Nova branch","paths":["novo.txt"],"amend":false,"new_branch":"nova"}},"response":{"ok":true,"result":{"ok":true,"output":"[nova 5a05f58] Nova branch\n 1 file changed, 1 insertion(+)\n create mode 100644 novo.txt"}},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 745c6831c41e73e44645b52e4ab81cfaf480dbe3 0\tnovo.txt\n100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/feature\n20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n5a05f58b1f6b44eb6e4a293b726d090c818d3bb5 refs/heads/nova\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n5a05f58b1f6b44eb6e4a293b726d090c818d3bb5 Nova branch\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"M04-discard-tracked","group":"mutation","request":{"op":"discard_file","args":{"path":"sub/ação.txt"}},"response":{"ok":true,"result":{"ok":true,"path":"sub/ação.txt"}},"effects":{"commands":[{"exit":0,"stdout":"?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/feature\n20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e68610a"}}},{"id":"M05-discard-untracked","group":"mutation","request":{"op":"discard_file","args":{"path":"novo.txt"}},"response":{"ok":true,"result":{"ok":true,"path":"novo.txt"}},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/feature\n20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"M06-switch-branch","group":"mutation","request":{"op":"switch_branch","args":{"branch":"feature"}},"response":{"ok":true,"result":{"current":"feature","output":"M\t\"sub/a\\303\\247\\303\\243o.txt\"\nSwitched to branch 'feature'"}},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/feature\n20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"M07-create-branch-at","group":"mutation","request":{"op":"create_branch_at","args":{"name":"nova","sha":null,"switch_after":false}},"response":{"ok":true,"result":{"ok":true,"output":""}},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/feature\n20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/nova\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"M08-create-tag-null","group":"mutation","request":{"op":"create_tag","args":{"name":"marca","sha":null,"message":null}},"response":{"ok":true,"result":{"ok":true,"output":""}},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/feature\n20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/tags/marca\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"M09-create-tag-blank","group":"mutation","request":{"op":"create_tag","args":{"name":"marca","sha":null,"message":"   "}},"response":{"ok":true,"result":{"ok":true,"output":""}},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/feature\n20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/tags/marca\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"M10-stash","group":"mutation","request":{"op":"git_action","args":{"action":"stash"}},"response":{"ok":true,"result":{"ok":true,"output":"Saved working directory and index state WIP on main: 20bc9e7 Inicial"}},"effects":{"commands":[{"exit":0,"stdout":"","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/feature\n20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\nfa0fa4c35c8e1c2a1805771aa11c9a9e81a57644 refs/stash\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\nfa0fa4c35c8e1c2a1805771aa11c9a9e81a57644 WIP on main: 20bc9e7 Inicial\nfa4af9edb96c9bc739e7889d67b78ded151e0b83 index on main: 20bc9e7 Inicial\n9d70f030a1631dc35b1b07af05bcc2b8023a868d untracked files on main: 20bc9e7 Inicial\n","stderr":""}],"files":{"outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e68610a"}}},{"id":"M11-stash-pop-empty","group":"mutation","request":{"op":"git_action","args":{"action":"stash-pop"}},"response":{"ok":true,"result":{"ok":false,"output":"No stash entries found."}},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/feature\n20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"M12-fetch-no-remote","group":"mutation","request":{"op":"git_action","args":{"action":"fetch"}},"response":{"ok":true,"result":{"ok":true,"output":""}},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/feature\n20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"M13-reset-soft","group":"mutation","request":{"op":"reset_to","args":{"sha":"20bc9e7267b8e5a4a32370c1855c4db636e589e6","mode":"soft"}},"response":{"ok":true,"result":{"ok":true,"output":""}},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/feature\n20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"M14-reset-mixed","group":"mutation","request":{"op":"reset_to","args":{"sha":"20bc9e7267b8e5a4a32370c1855c4db636e589e6","mode":"mixed"}},"response":{"ok":true,"result":{"ok":true,"output":"Unstaged changes after reset:\nM\tsub/ação.txt"}},"effects":{"commands":[{"exit":0,"stdout":" M sub/ação.txt\u0000?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/feature\n20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e6861206d75646164610a"}}},{"id":"M15-reset-hard","group":"mutation","request":{"op":"reset_to","args":{"sha":"20bc9e7267b8e5a4a32370c1855c4db636e589e6","mode":"hard"}},"response":{"ok":true,"result":{"ok":true,"output":"HEAD is now at 20bc9e7 Inicial"}},"effects":{"commands":[{"exit":0,"stdout":"?? novo.txt\u0000","stderr":""},{"exit":0,"stdout":"100644 a249728954f5919e45e42888985373d94bc58bc5 0\toutro.txt\n100644 512146f2d456dcb8aa28f59477043e87017dfd95 0\t\"sub/a\\303\\247\\303\\243o.txt\"\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/feature\n20bc9e7267b8e5a4a32370c1855c4db636e589e6 refs/heads/main\n","stderr":""},{"exit":0,"stdout":"20bc9e7267b8e5a4a32370c1855c4db636e589e6 Inicial\n","stderr":""}],"files":{"novo.txt":"6e6f766f0a","outro.txt":"6f7574726f0a","sub/ação.txt":"7072696d656972610a6c696e68610a"}}}]"####;

    #[cfg(unix)]
    #[test]
    fn original_35_public_operations_keep_complete_bodies_and_disk_effects() {
        const TEST:&str="task3_contract::original_35_public_operations_keep_complete_bodies_and_disk_effects";
        if std::env::var_os("HANGAR_TASK3_ORIGINAL_35_CHILD").is_none() {
            let child_home=tempfile::tempdir().unwrap();
            let mut command=Command::new(std::env::current_exe().unwrap());
            for (key,_) in std::env::vars().filter(|(key,_)|key.starts_with("GIT_")){command.env_remove(key);}
            let empty=child_home.path().join("empty-template"); fs::create_dir(&empty).unwrap();
            let child=command.args(["--exact",TEST,"--nocapture"]).env("HANGAR_TASK3_ORIGINAL_35_CHILD","1")
                .env("HOME",child_home.path()).env("GIT_CONFIG_GLOBAL","/dev/null").env("GIT_CONFIG_NOSYSTEM","1")
                .env("GIT_TEMPLATE_DIR",empty).env("GIT_AUTHOR_DATE","2020-01-01T00:00:00+00:00")
                .env("GIT_COMMITTER_DATE","2020-01-01T00:00:00+00:00").output().unwrap();
            assert!(child.status.success(),"{}\n{}",String::from_utf8_lossy(&child.stdout),String::from_utf8_lossy(&child.stderr)); return;
        }
        fn files(root:&Path,path:&Path,out:&mut std::collections::BTreeMap<String,String>) {
            for entry in fs::read_dir(path).unwrap() {
                let entry=entry.unwrap(); if path==root&&entry.file_name()==".git" {continue;}
                if entry.file_type().unwrap().is_dir(){files(root,&entry.path(),out);}else {
                    let bytes=fs::read(entry.path()).unwrap();
                    out.insert(entry.path().strip_prefix(root).unwrap().to_string_lossy().into_owned(),bytes.iter().map(|b|format!("{b:02x}")).collect());
                }
            }
        }
        let cases:Vec<Value>=serde_json::from_str(ORIGINAL_35).unwrap(); assert_eq!(cases.len(),35);
        for case in cases {
            let id=case["id"].as_str().unwrap(); let dir=tempfile::tempdir().unwrap(); let root=dir.path().join("repo"); fs::create_dir_all(root.join("sub")).unwrap();
            for args in [&["init","-b","main"][..],&["config","user.name","Teste"],&["config","user.email","teste@example.invalid"],
                &["config","maintenance.auto","false"],&["config","gc.auto","0"],&["config","commit.gpgSign","false"]] {g(&root,args);}
            fs::write(root.join("sub/ação.txt"),"primeira\nlinha\n").unwrap(); fs::write(root.join("outro.txt"),"outro\n").unwrap();
            g(&root,&["add","."]); g(&root,&["commit","-m","Inicial"]);
            assert_eq!(g(&root,&["rev-parse","HEAD"]).trim(),"20bc9e7267b8e5a4a32370c1855c4db636e589e6","{id}: fixture inicial");
            fs::write(root.join("sub/ação.txt"),"primeira\nlinha mudada\n").unwrap(); fs::write(root.join("novo.txt"),"novo\n").unwrap();
            if case["group"]=="mutation" {g(&root,&["branch","feature"]);}
            let mut request=case["request"].clone(); request["args"]["cwd"]=json!(root);
            let mut expected=case["response"].clone();
            match id {
                "G05-list-branches" => expected["result"]["upstreams"]=json!({}),
                "G09-folder-status" => expected["result"]["toplevel"]=json!(root),
                "F06-resolver" => for path in ["ação.txt","sub/ação.txt"] {expected["result"]["ok"][path]["real"]=json!(root.join("sub/ação.txt"));},
                "G07-git-log" => expected["result"][0]["rel"]=json!(g(&root,&["log","-1","--format=%ar"]).trim()),
                _ => {},
            }
            let result=execute(serde_json::from_value(request).unwrap());
            let actual=match result {Ok(result)=>json!({"ok":true,"result":result}),Err(error)=>json!({"ok":false,"error":error})};
            assert_eq!(actual,expected,"{id}: corpo integral");
            let commands=[&["status","--porcelain","-z"][..],&["ls-files","--stage"],&["show-ref"],&["log","--all","--format=%H %s"]];
            let effects=commands.iter().map(|args| {
                let out=output(&root,args);
                json!({"exit":out.status.code().unwrap_or(-1),"stdout":String::from_utf8(out.stdout).unwrap(),"stderr":String::from_utf8(out.stderr).unwrap()})
            }).collect::<Vec<_>>();
            assert_eq!(json!(effects),case["effects"]["commands"],"{id}: statusNUL/índice/refs/log integrais");
            let mut observed=std::collections::BTreeMap::new(); files(&root,&root,&mut observed);
            assert_eq!(json!(observed),case["effects"]["files"],"{id}: bytes integrais de todos os arquivos");
        }
    }
    use std::path::Path;

    fn output(cwd: &Path, args: &[&str]) -> std::process::Output {
        let mut command = Command::new("git");
        for (key, _) in std::env::vars().filter(|(key, _)| key.starts_with("GIT_")) { command.env_remove(key); }
        command.arg("-C").arg(cwd).args(args)
            .env("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" })
            .env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00Z")
            .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00Z").output().unwrap()
    }
    fn g(cwd: &Path, args: &[&str]) -> String {
        let result = output(cwd, args);
        assert!(result.status.success(), "{args:?}: {}", String::from_utf8_lossy(&result.stderr));
        String::from_utf8(result.stdout).unwrap()
    }
    fn repo(root: &Path) {
        fs::create_dir_all(root).unwrap();
        g(root, &["init", "-q", "-b", "main"]);
        g(root, &["config", "user.name", "Teste"]); g(root, &["config", "user.email", "teste@example.invalid"]);
        g(root, &["config", "core.hooksPath", ".git/hooks"]);
        g(root, &["config", "commit.gpgSign", "false"]);
        g(root, &["config", "core.autocrlf", "false"]);
        fs::write(root.join("tracked.txt"), "linha original\n").unwrap();
        g(root, &["add", "tracked.txt"]); g(root, &["commit", "-qm", "initial"]);
        g(root, &["branch", "feature"]);
    }
    fn run(cwd: &Path, name: &str, args: Value) -> hangar_workspace::Result<Value> {
        let mut args = args.as_object().unwrap().clone(); args.insert("cwd".into(), json!(cwd));
        execute(serde_json::from_value(json!({"op":name,"args":args})).unwrap())
    }

    #[test]
    fn git_reads_keep_branch_status_log_literals_and_revision_fields() {
        let dir = tempfile::tempdir().unwrap(); let root = dir.path(); repo(root);
        assert_eq!(op(root, "head_info", json!({})), json!(["main", false]));
        assert_eq!(op(root, "branch_of", json!({})), "main");
        let branches = op(root, "list_branches", json!({}));
        assert_eq!(branches["dirty"], false); assert_eq!(branches["current"], "main");
        assert_eq!(branches["branches"].as_array().unwrap().len(), 2);
        fs::write(root.join("tracked.txt"), "a\nb\nc\n").unwrap(); fs::write(root.join("novo.txt"), "novo\n").unwrap();
        let changed = op(root, "changed_files", json!({}));
        let tracked = changed.as_array().unwrap().iter().find(|v| v["path"] == "tracked.txt").unwrap();
        assert_eq!(tracked, &json!({"path":"tracked.txt","code":" M","staged":false,"added":3,"removed":1}));
        let new = changed.as_array().unwrap().iter().find(|v| v["path"] == "novo.txt").unwrap();
        assert_eq!(new, &json!({"path":"novo.txt","code":"??","staged":false,"added":null,"removed":null}));
        for (path, expected) in [("tracked.txt", "+a"), ("novo.txt", "+novo")] {
            let diff = op(root, "file_diff", json!({"path":path})); assert_eq!(diff["path"], path);
            assert_eq!(diff["truncated"], false); assert!(diff["diff"].as_str().unwrap().contains(expected));
        }
        let hash = g(root, &["rev-parse", "HEAD"]).trim().to_owned();
        assert_eq!(op(root, "sequencer_state", json!({})), Value::Null);
        assert_eq!(op(root, "commit_files", json!({"sha":hash})), json!([{"path":"tracked.txt","code":"A"}]));
        assert!(op(root, "commit_file_diff", json!({"sha":hash,"path":"tracked.txt"}))["diff"].as_str().unwrap().contains("+linha original"));
        assert!(op(root, "diff_vs_worktree", json!({"sha":hash}))["diff"].as_str().unwrap().contains("+a"));
        op(root, "commit", json!({"message":"corrige c++ (de novo)\n\nantes\u{1f}depois","paths":["novo.txt"]}));
        for query in ["c++", "(de novo)"] {
            let log = op(root, "git_log", json!({"grep":query,"n":50}));
            assert_eq!(log.as_array().unwrap().len(), 1);
            assert_eq!(log[0]["subject"], "corrige c++ (de novo)");
            assert!(log[0]["body"].as_str().unwrap().contains("antes\u{1f}depois"));
        }
        for query in ["--all", "c.+", "não existe"] { assert_eq!(op(root, "git_log", json!({"grep":query})), json!([])); }
        assert_eq!(op(root, "git_log", json!({})).as_array().unwrap().len(), 2);
        assert_eq!(op(root, "last_commit_message", json!({}))["message"], "corrige c++ (de novo)\n\nantes\u{1f}depois");
        g(root, &["switch", "--detach"]);
        let short = g(root, &["rev-parse", "--short", "HEAD"]).trim().to_owned();
        // A API original com Rust publica HEAD; o assert isolado Python de null não rege esta rota.
        assert_eq!(op(root, "list_branches", json!({})), json!({"current":"HEAD",
            "branches":[format!("(HEAD detached at {short})"),"main","feature"],"remotes":[],"dirty":true,"elsewhere":{},"upstreams":{}}));
    }

    #[test]
    fn git_refusals_keep_refs_index_and_disk_unchanged() {
        let dir = tempfile::tempdir().unwrap(); let root = dir.path(); repo(root);
        fs::write(root.join("tracked.txt"), "preservada\n").unwrap();
        let refs = g(root, &["show-ref"]); let index = g(root, &["ls-files", "--stage"]);
        for branch in ["nope", "main; rm -rf /", "--orphan x"] {
            assert_eq!(run(root, "switch_branch", json!({"branch":branch})).unwrap_err().status, 400);
        }
        for path in ["missing.txt", "../../etc/passwd", "--output=x"] {
            for name in ["file_diff", "discard_file"] { assert_eq!(run(root, name, json!({"path":path})).unwrap_err().status, 400); }
        }
        for bad in ["HEAD", "HEAD~1", "--all", "nope; rm -rf /", "HEAD; echo pwned"] {
            for name in ["commit_diff", "diff_vs_worktree", "revert_commit", "cherry_pick", "branches_containing"] {
                assert_eq!(run(root, name, json!({"sha":bad})).unwrap_err().status, 400, "{name}/{bad}");
            }
        }
        for mode in ["--hard; rm", "HARD", ""] { assert_eq!(run(root,"reset_to",json!({"sha":g(root,&["rev-parse","HEAD"]).trim(),"mode":mode})).unwrap_err().status,400); }
        for name in ["feature", "nome com espaço", "-D", "../x"] {
            assert_eq!(run(root,"commit",json!({"message":"m","paths":["tracked.txt"],"new_branch":name})).unwrap_err().status,400);
            assert_eq!(run(root,"create_branch_at",json!({"name":name})).unwrap_err().status,400);
        }
        assert_eq!(run(root,"commit",json!({"message":"m"})).unwrap_err().status,400);
        assert_eq!(run(root,"git_action",json!({"action":"rm -rf"})).unwrap_err().status,400);
        assert_eq!(g(root, &["show-ref"]), refs); assert_eq!(g(root, &["ls-files", "--stage"]), index);
        assert_eq!(fs::read_to_string(root.join("tracked.txt")).unwrap(), "preservada\n");
    }

    #[test]
    fn amend_selected_paths_refs_tags_and_reset_preserve_foreign_stage() {
        let dir = tempfile::tempdir().unwrap(); let root = dir.path(); repo(root);
        fs::write(root.join("foreign.txt"), "foreign\n").unwrap(); g(root,&["add","foreign.txt"]);
        let foreign = g(root, &["ls-files","--stage","foreign.txt"]);
        op(root,"commit",json!({"message":"renamed","amend":true}));
        assert!(!g(root,&["show","--name-only","--format=","HEAD"]).contains("foreign.txt"));
        assert_eq!(g(root,&["ls-files","--stage","foreign.txt"]),foreign);
        fs::write(root.join("novo.txt"), "N\n").unwrap();
        op(root,"commit",json!({"message":"folded","paths":["novo.txt"],"amend":true}));
        assert_eq!(g(root,&["log","--pretty=%s"]),"folded\n");
        assert!(g(root,&["show","--name-only","--format=","HEAD"]).contains("tracked.txt"));
        assert_eq!(g(root,&["ls-files","--stage","foreign.txt"]),foreign);
        op(root,"create_tag",json!({"name":"annotated","message":"release"}));
        op(root,"create_tag",json!({"name":"light","message":"   "}));
        assert_eq!(g(root,&["cat-file","-t","annotated"]).trim(),"tag");
        assert_eq!(g(root,&["cat-file","-t","light"]).trim(),"commit");
        assert_eq!(run(root,"create_tag",json!({"name":"light"})).unwrap_err().status,400);
        assert_eq!(run(root,"create_tag",json!({"name":"with space"})).unwrap_err().status,400);
        op(root,"commit",json!({"message":"new branch","paths":["foreign.txt"],"new_branch":"new"}));
        assert_eq!(op(root,"branch_of",json!({})),"new");
        let base = g(root,&["rev-parse","HEAD"]).trim().to_owned();
        fs::write(root.join("b.txt"),"B\n").unwrap(); op(root,"commit",json!({"message":"c2","paths":["b.txt"]}));
        op(root,"reset_to",json!({"sha":base,"mode":"soft"})); assert!(g(root,&["status","--porcelain"]).contains("A  b.txt"));
        op(root,"reset_to",json!({"sha":base,"mode":"mixed"})); assert!(g(root,&["status","--porcelain"]).contains("?? b.txt"));
        fs::write(root.join("tracked.txt"),"bad\n").unwrap(); op(root,"reset_to",json!({"sha":base,"mode":"hard"}));
        assert_eq!(fs::read_to_string(root.join("tracked.txt")).unwrap(),"linha original\n");
        assert!(root.join("b.txt").exists(),"untracked não é apagado por reset");
    }

    #[test]
    fn branch_switch_failure_does_not_leave_a_new_ref_and_empty_amend_is_refused() {
        let dir = tempfile::tempdir().unwrap(); let root = dir.path(); repo(root);
        let base = g(root,&["rev-parse","HEAD"]).trim().to_owned();
        fs::write(root.join("tracked.txt"),"B\n").unwrap(); g(root,&["commit","-qam","B"]);
        fs::write(root.join("tracked.txt"),"C\n").unwrap();
        assert_eq!(run(root,"create_branch_at",json!({"name":"orphan","sha":base,"switch_after":true})).unwrap_err().status,409);
        assert!(!g(root,&["branch","--format=%(refname:short)"]).contains("orphan"));
        assert_eq!(op(root,"branch_of",json!({})),"main");
        let empty = root.join("empty"); fs::create_dir(&empty).unwrap(); g(&empty,&["init","-q","-b","main"]);
        assert_eq!(run(&empty,"commit",json!({"message":"m","amend":true})).unwrap_err().status,409);
    }

    #[test]
    fn revision_conflicts_report_sequencer_and_abort_recovers_without_losing_work() {
        let dir = tempfile::tempdir().unwrap(); let root = dir.path(); repo(root);
        let added = g(root,&["rev-parse","HEAD"]).trim().to_owned();
        g(root,&["switch","feature"]); fs::write(root.join("tracked.txt"),"feature\n").unwrap(); g(root,&["commit","-qam","feature"]);
        let feat = g(root,&["rev-parse","HEAD"]).trim().to_owned();
        g(root,&["switch","main"]); fs::write(root.join("tracked.txt"),"main\n").unwrap(); g(root,&["commit","-qam","main"]);
        assert_eq!(run(root,"cherry_pick",json!({"sha":feat})).unwrap_err().status,409);
        assert_eq!(op(root,"sequencer_state",json!({})),"cherry-pick");
        let hits = op(root,"search",json!({"q":"tracked","mode":"names"})); assert_eq!(hits["hits"].as_array().unwrap().len(),1);
        assert_eq!(op(root,"git_action",json!({"action":"cherry-pick-abort"}))["ok"],true);
        assert_eq!(op(root,"sequencer_state",json!({})),Value::Null);
        assert_eq!(run(root,"revert_commit",json!({"sha":added})).unwrap_err().status,409);
        assert_eq!(op(root,"sequencer_state",json!({})),"revert");
        assert_eq!(op(root,"git_action",json!({"action":"revert-abort"}))["ok"],true);
        assert_eq!(fs::read_to_string(root.join("tracked.txt")).unwrap(),"main\n");
        for action in ["revert-abort","cherry-pick-abort"] { assert_eq!(op(root,"git_action",json!({"action":action}))["ok"],false); }
        fs::write(root.join("tracked.txt"),"pending\n").unwrap(); op(root,"git_action",json!({"action":"stash"}));
        assert_eq!(op(root,"changed_files",json!({})),json!([])); op(root,"git_action",json!({"action":"stash-pop"}));
        assert_eq!(fs::read_to_string(root.join("tracked.txt")).unwrap(),"pending\n");
        op(root,"discard_file",json!({"path":"tracked.txt"})); assert_eq!(fs::read_to_string(root.join("tracked.txt")).unwrap(),"main\n");
        fs::write(root.join("trash.txt"),"trash").unwrap(); op(root,"discard_file",json!({"path":"trash.txt"})); assert!(!root.join("trash.txt").exists());
    }

    #[test]
    fn status_and_numstat_parsers_keep_original_nulls_and_special_names() {
        use hangar_workspace::git::{parse_summary,numstat,scrub};
        for (raw, expected) in [
            ("## main...origin/main [ahead 5]\n",json!({"dirty":0,"ahead":5,"behind":0})),
            ("## main...origin/main [ahead 2, behind 1]\n M a.py\n?? b\n",json!({"dirty":2,"ahead":2,"behind":1})),
            ("## main...origin/main\n",json!({"dirty":0,"ahead":0,"behind":0})),
            ("## main...origin/main\nR  old -> new\n",json!({"dirty":1,"ahead":0,"behind":0})),
            ("## feature\n M a.py\n",json!({"dirty":1,"ahead":null,"behind":null})),
            ("## HEAD (no branch)\n M a.py\n",json!({"dirty":1,"ahead":null,"behind":null})),
            ("## main...origin/main [gone]\n",json!({"dirty":0,"ahead":null,"behind":null})),
            ("## No commits yet on main\n?? a\n",json!({"dirty":1,"ahead":null,"behind":null})),
            ("",Value::Null),("lixo\n",Value::Null),
        ] { assert_eq!(parse_summary(raw),expected,"{raw}"); }
        let nums = numstat("10\t2\ta.py\n3\t0\tdir/b.py\n-\t-\timg.png\n7\tx\nlixo\n");
        assert_eq!(nums.len(),2); assert_eq!(nums["a.py"],(10,2)); assert_eq!(nums["dir/b.py"],(3,0));
        for (remote, expected) in [
            ("https://user:p@ss@example.invalid/repo","https://***@example.invalid/repo"),
            ("https://user:p/ss@example.invalid/repo","https://***@example.invalid/repo"),
            ("https://user:pw@example.invalid:8443/repo@revision","https://***@example.invalid:8443/repo@revision"),
            ("https://host:8443/repo@rev","https://host:8443/repo@rev"),
            ("https://host/repo@rev","https://host/repo@rev"),
            ("https://[2001:db8::1]:8443/repo@rev","https://[2001:db8::1]:8443/repo@rev"),
            ("https://host:8?e=u@example.invalid","https://host:8?e=u@example.invalid"),
        ] { assert_eq!(scrub(remote),expected); }
    }

    #[test]
    fn files_keep_bytes_limits_binary_digest_and_atomic_mode() {
        let dir = tempfile::tempdir().unwrap(); let root = dir.path();
        let target = root.join("bytes.txt"); fs::write(&target,b"a\r\nb\xff\n").unwrap();
        let read=op(root,"read_file",json!({"path":"bytes.txt"}));
        assert_eq!(read["text"],"a\r\nb\u{fffd}\n"); assert_eq!(read["size"],6); assert_eq!(fs::read(&target).unwrap(),b"a\r\nb\xff\n");
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; fs::set_permissions(&target,fs::Permissions::from_mode(0o755)).unwrap(); }
        let saved=op(root,"write_file",json!({"path":"bytes.txt","texto":"um\ndois\n","digest_lido":read["digest"]}));
        op(root,"write_file",json!({"path":"bytes.txt","texto":"um\ndois\ntres\n","digest_lido":saved["digest"]}));
        assert_eq!(fs::read_to_string(&target).unwrap(),"um\ndois\ntres\n");
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; assert_eq!(fs::metadata(&target).unwrap().permissions().mode() & 0o777,0o755); }
        assert_eq!(fs::read_dir(root).unwrap().count(),1,"temporário de gravação não pode sobrar");
        let error=run(root,"write_file",json!({"path":"bytes.txt","texto":"perdida"})).unwrap_err(); assert_eq!(error.code.as_deref(),Some("erro_arq_sem_digest"));
        assert_eq!(run(root,"write_file",json!({"path":"bytes.txt","texto":"perdida","digest_lido":read["digest"]})).unwrap_err().status,409);
        fs::write(root.join("late.bin"),[vec![b'A';8192],vec![0;16],vec![b'B';100]].concat()).unwrap();
        assert_eq!(run(root,"read_file",json!({"path":"late.bin"})).unwrap_err().status,415);
        let huge=root.join("huge.bin"); let file=fs::File::create(&huge).unwrap(); file.set_len(512*1024+1024).unwrap();
        assert_eq!(run(root,"write_file",json!({"path":"huge.bin","texto":"perdida","digest_lido":"a".repeat(64)})).unwrap_err().status,413);
        assert_eq!(fs::metadata(&huge).unwrap().len(),512*1024+1024);
        fs::write(&huge,vec![b'x';512*1024+1024]).unwrap();
        let read=op(root,"read_file",json!({"path":"huge.bin"})); assert_eq!(read["truncated"],true); assert!(read["digest"].is_null());
        assert_eq!(read["text"].as_str().unwrap().len(),512*1024);
        for bad in ["../outside","--output=x","\0"] { assert_eq!(run(root,"read_file",json!({"path":bad})).unwrap_err().status,400); }
    }

    #[test]
    fn file_tree_search_and_resolver_preserve_subfolders_and_literal_paths() {
        let dir=tempfile::tempdir().unwrap(); let root=dir.path(); repo(root);
        fs::create_dir_all(root.join("backend/app")).unwrap();
        fs::write(root.join("backend/app/x.txt"),"a\n").unwrap(); fs::write(root.join(".env.example"),"X=1").unwrap();
        fs::write(root.join("a[b].txt"),"colchete\n").unwrap(); fs::write(root.join("ab.txt"),"irmão\n").unwrap();
        g(root,&["add","backend/app/x.txt",".env.example","a[b].txt","ab.txt"]); g(root,&["commit","-qm","files"]);
        fs::write(root.join("backend/app/x.txt"),"a\nb\n").unwrap(); fs::write(root.join("a[b].txt"),"colchete\nCOLCHETE\n").unwrap(); fs::write(root.join("ab.txt"),"irmão\nIRMAO\n").unwrap();
        let tree=op(&root.join("backend"),"list_dir",json!({}));
        assert_eq!(tree["entries"][0]["name"],"app"); assert_eq!(tree["entries"][0]["changed"],"M"); assert_eq!(tree["entries"][0]["add"],1);
        let tree=op(root,"list_dir",json!({"so_modificados":false}));
        assert_eq!(tree["entries"][0]["is_dir"],true);
        assert!(tree["entries"].as_array().unwrap().iter().any(|e|e["name"]==".env.example"));
        assert!(!tree["entries"].as_array().unwrap().iter().any(|e|e["name"]==".git"));
        let tree=op(root,"list_dir",json!({})); assert!(!tree["entries"].as_array().unwrap().iter().any(|e|e["name"]=="tracked.txt"));
        let diff=op(root,"path_diff",json!({"path":"a[b].txt","escopo":"nao_commitado"}));
        assert!(diff["diff"].as_str().unwrap().contains("+COLCHETE")); assert!(!diff["diff"].as_str().unwrap().contains("+IRMAO"));
        fs::write(root.join(".gitignore"),"ignored.txt\n").unwrap(); fs::write(root.join("ignored.txt"),"needle").unwrap();
        fs::write(root.join("sessão-única.md"),"usa --force aqui\n").unwrap();
        assert_eq!(op(root,"search",json!({"q":"ignored","mode":"names"}))["hits"],json!([]));
        assert_eq!(op(root,"search",json!({"q":"absent","mode":"contents"})),json!({"hits":[],"truncated":false,"mode":"contents"}));
        let found=op(root,"search",json!({"q":"--force","mode":"contents"})); assert_eq!(found["hits"][0],json!({"path":"sessão-única.md","line":1,"text":"usa --force aqui"}));
        assert_eq!(op(root,"search",json!({"q":"SESS","mode":"names"}))["hits"][0]["path"],"sessão-única.md");
        fs::remove_file(root.join("tracked.txt")).unwrap(); assert_eq!(op(root,"search",json!({"q":"tracked","mode":"names"}))["hits"],json!([]));
        for (q,mode,code) in [("   ","names","erro_arq_busca_vazia"),("\0","contents","erro_arq_busca_falhou"),("x","evil","erro_arq_modo_invalido")] {
            assert_eq!(run(root,"search",json!({"q":q,"mode":mode})).unwrap_err().code.as_deref(),Some(code));
        }
        fs::create_dir(root.join("a")).unwrap(); fs::create_dir(root.join("b")).unwrap();
        fs::write(root.join("a/nome.txt"),"primeiro").unwrap(); fs::write(root.join("b/nome.txt"),"segundo").unwrap();
        let resolved=op(root,"resolver",json!({"caminhos":["nome.txt","backend/app/x.txt","absent"],"suffix":true}));
        assert_eq!(resolved["ok"]["nome.txt"]["relativo"],"a/nome.txt"); assert_eq!(resolved["faltam"],json!(["absent"]));
    }

    #[test]
    fn citation_rows_override_the_file_keep_longest_match_and_line_cwd() {
        let dir=tempfile::tempdir().unwrap(); let root=dir.path();
        let cwd=root.join("born"); let old=root.join("old"); fs::create_dir(&cwd).unwrap(); fs::create_dir(&old).unwrap();
        fs::write(old.join("config.json.bak"),"original").unwrap();
        let log=root.join("active.jsonl"); fs::write(&log,json!({"cwd":cwd,"text":"uncited.json"}).to_string()).unwrap();
        let rows=vec![json!({"cwd":old,"text":"config.json.bak"}).to_string(),json!({"cwd":cwd,"text":"other"}).to_string(),json!({"snapshot":{"config.json.bak":{}}}).to_string()];
        let cwds=execute(serde_json::from_value(json!({"op":"citation_cwds","args":{"jsonl":log,"rows":rows,
            "needles":["config.json","config.json.bak"]}})).unwrap()).unwrap();
        assert_eq!(cwds,json!({"config.json.bak":[old]}));
        for write in [false,true] {
            let request=serde_json::from_value(json!({"op":"resolve_cited","args":{"cwd":cwd,"jsonl":log,"rows":rows,"path":"config.json.bak","write":write}})).unwrap();
            assert_eq!(execute(request).unwrap(),json!(old.join("config.json.bak")));
        }
    }

    #[test]
    fn scan_mkdir_and_plain_folders_keep_roots_flags_and_refusals() {
        let dir=tempfile::tempdir().unwrap(); let root=hangar_workspace::real(dir.path());
        let base=root.join("projects"); fs::create_dir(&base).unwrap();
        fs::create_dir_all(base.join("alpha/.git")).unwrap(); fs::write(base.join("alpha/CLAUDE.md"),"x").unwrap();
        fs::create_dir(base.join("beta")).unwrap(); fs::create_dir(base.join(".hidden")).unwrap(); fs::write(base.join("note.txt"),"common").unwrap();
        let call=|name:&str,mut args:Value| {
            args["root"]=json!(base); args["roots"]=json!([base]);
            execute(serde_json::from_value(json!({"op":name,"args":args})).unwrap())
        };
        let roots=execute(serde_json::from_value(json!({"op":"list_roots","args":{"roots":[base]}})).unwrap()).unwrap();
        assert_eq!(roots,json!([{"name":"projects","path":base}]));
        let scan=call("scan_dir",json!({})).unwrap(); assert!(scan["error"].is_null());
        let entries=scan["entries"].as_array().unwrap(); assert_eq!(entries.len(),2);
        let alpha=entries.iter().find(|e|e["name"]=="alpha").unwrap(); assert_eq!(alpha["is_git"],true); assert_eq!(alpha["has_claude_md"],true);
        let beta=entries.iter().find(|e|e["name"]=="beta").unwrap(); assert_eq!(beta["is_git"],false); assert_eq!(beta["has_claude_md"],false);
        let created=call("make_dir",json!({"name":"new-folder"})).unwrap();
        assert_eq!(created["name"],"new-folder"); assert_eq!(created["path"],json!(base.join("new-folder"))); assert_eq!(created["is_git"],false); assert_eq!(created["has_claude_md"],false); assert!(created["mtime"].is_number());
        assert_eq!(call("make_dir",json!({"name":"new-folder"})).unwrap_err().status,409);
        for bad in ["", ".git", "../escape", "a/b", "a\\b", "\0"] { assert_eq!(call("make_dir",json!({"name":bad})).unwrap_err().status,400); }
        for (path,status) in [(base.join("missing"),404),(root.clone(),400),(base.join("note.txt"),400)] {
            assert_eq!(call("scan_dir",json!({"path":path})).unwrap_err().status,status);
        }
        let denied=execute(serde_json::from_value(json!({"op":"scan_dir","args":{"root":root,"roots":[base]}})).unwrap()).unwrap_err(); assert_eq!(denied.status,403);
        let plain=op(&base,"list_dir",json!({}));
        assert!(plain["entries"].as_array().unwrap().iter().any(|e|e["name"]=="note.txt"));
        assert!(plain["entries"].as_array().unwrap().iter().all(|e|e["changed"].is_null()&&e["add"]==0&&e["del"]==0));
        assert_eq!(run(&base,"search",json!({"q":"x","mode":"names"})).unwrap_err().code.as_deref(),Some("erro_arq_nao_e_repo_git"));
    }

    #[cfg(unix)]
    #[test]
    fn file_safety_includes_ancestor_git_links_fifo_socket_and_literal_star() {
        use std::os::unix::{fs::{symlink,PermissionsExt},net::UnixListener};
        let dir=tempfile::tempdir().unwrap(); let root=dir.path().join(".git/project"); repo(&root);
        assert_eq!(op(&root,"read_file",json!({"path":"tracked.txt"}))["text"],"linha original\n");
        assert!(!op(&root,"list_dir",json!({"so_modificados":false}))["entries"].as_array().unwrap().is_empty());
        symlink(".git",root.join("alias")).unwrap();
        for path in [".git",".git/config","./.git/config","alias/config"] {
            for name in ["list_dir","read_file","write_file"] {
                let args = if name == "write_file" { json!({"path":path,"texto":"lost","digest_lido":"any"}) } else { json!({"path":path}) };
                assert_eq!(run(&root,name,args).unwrap_err().code.as_deref(),Some("erro_arq_area_do_git"));
            }
        }
        let fifo=root.join("fifo"); assert!(Command::new("mkfifo").arg(&fifo).status().unwrap().success());
        let socket=UnixListener::bind(root.join("socket")).unwrap();
        for name in ["fifo","socket"] { assert_eq!(run(&root,"read_file",json!({"path":name})).unwrap_err().code.as_deref(),Some("erro_arq_nao_e_arquivo")); }
        drop(socket);
        for name in ["*","other"] { fs::write(root.join(name),"before\n").unwrap(); }
        g(&root,&["--literal-pathspecs","add","*","other"]); g(&root,&["commit","-qm","literal"]);
        fs::write(root.join("*"),"before\nSTAR\n").unwrap(); fs::write(root.join("other"),"before\nOTHER\n").unwrap();
        let diff=op(&root,"path_diff",json!({"path":"*","escopo":"nao_commitado"})); assert!(diff["diff"].as_str().unwrap().contains("+STAR")); assert!(!diff["diff"].as_str().unwrap().contains("+OTHER"));
        fs::create_dir(root.join("locked")).unwrap(); fs::set_permissions(root.join("locked"),fs::Permissions::from_mode(0o000)).unwrap();
        let denied=run(&root,"list_dir",json!({"path":"locked","so_modificados":false}));
        fs::set_permissions(root.join("locked"),fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(denied.unwrap_err().code.as_deref(),Some("erro_arq_sem_permissao"));
    }

    #[test]
    fn search_limits_count_real_unique_hits_including_untracked_names() {
        let dir=tempfile::tempdir().unwrap(); let root=dir.path(); repo(root);
        for i in 0..210 { fs::write(root.join(format!("m{i:03}.txt")),"needle\n").unwrap(); }
        for (q,mode) in [("m","names"),("needle","contents")] {
            let found=op(root,"search",json!({"q":q,"mode":mode})); assert_eq!(found["truncated"],true);
            let hits=found["hits"].as_array().unwrap(); assert_eq!(hits.len(),200);
            let paths=hits.iter().map(|e|e["path"].as_str().unwrap()).collect::<std::collections::HashSet<_>>(); assert_eq!(paths.len(),200);
            assert!(paths.iter().all(|p|root.join(p).is_file()));
        }
        #[cfg(unix)] {
            for name in ["a:b.txt","a\"b.txt","line\nname.txt"] { fs::write(root.join(name),"special\n").unwrap(); }
            let hits=op(root,"search",json!({"q":"special","mode":"contents"}));
            let set=hits["hits"].as_array().unwrap().iter().map(|v|(v["path"].as_str().unwrap(),v["line"].as_u64().unwrap(),v["text"].as_str().unwrap())).collect::<std::collections::HashSet<_>>();
            assert_eq!(set,std::collections::HashSet::from([("a:b.txt",1,"special"),("a\"b.txt",1,"special"),("line\nname.txt",1,"special")]));
        }
    }

    #[test]
    fn network_remote_dwim_push_pull_and_divergence_keep_the_working_copy() {
        let dir=tempfile::tempdir().unwrap(); let base=dir.path(); let origin=base.join("origin"); repo(&origin);
        g(&origin,&["config","receive.denyCurrentBranch","ignore"]); g(base,&["clone","-q","origin","copy"]);
        let root=base.join("copy"); g(&root,&["config","user.name","Teste"]); g(&root,&["config","user.email","teste@example.invalid"]); g(&root,&["config","commit.gpgSign","false"]);
        assert_eq!(op(&root,"list_branches",json!({}))["remotes"],json!(["feature"]));
        op(&root,"switch_branch",json!({"branch":"feature"})); assert_eq!(op(&root,"branch_of",json!({})),"feature");
        op(&root,"switch_branch",json!({"branch":"main"}));
        fs::write(origin.join("tracked.txt"),"remote\n").unwrap(); g(&origin,&["commit","-qam","remote"]);
        assert_eq!(op(&root,"folder_pull",json!({}))["behind"],0); assert_eq!(fs::read_to_string(root.join("tracked.txt")).unwrap(),"remote\n");
        fs::write(root.join("local.txt"),"local\n").unwrap(); op(&root,"commit",json!({"message":"local","paths":["local.txt"]}));
        assert_eq!(op(&root,"push",json!({}))["ok"],true);
        op(&root,"create_branch_at",json!({"name":"published","switch_after":true}));
        assert_eq!(op(&root,"push",json!({}))["ok"],true); assert_eq!(g(&root,&["rev-parse","--abbrev-ref","@{upstream}"]).trim(),"origin/published");
        op(&root,"switch_branch",json!({"branch":"main"}));
        fs::write(root.join("local.txt"),"changed\n").unwrap(); assert_eq!(run(&root,"folder_pull",json!({})).unwrap_err().detail["code"],"erro_git_folder_dirty");
        g(&root,&["commit","-qam","local newer"]); fs::write(origin.join("tracked.txt"),"remote newer\n").unwrap(); g(&origin,&["commit","-qam","remote newer"]);
        let error=run(&root,"folder_pull",json!({})).unwrap_err(); assert_eq!(error.detail["code"],"erro_git_folder_diverged");
        assert_eq!(error.detail["params"]["ahead"],1); assert_eq!(error.detail["params"]["behind"],1); assert_eq!(fs::read_to_string(root.join("tracked.txt")).unwrap(),"remote\n");
    }

    #[test]
    fn folder_confirmations_precede_branch_changes_but_not_non_checkout_creation() {
        let dir=tempfile::tempdir().unwrap(); let root=dir.path(); repo(root);
        let sessions=json!(["fixture"]);
        op(root,"folder_create_branch",json!({"name":"base-copy","base":"feature","checkout":false,"sessions":[],"confirm_sessions":false}));
        assert_eq!(g(root,&["rev-parse","base-copy"]),g(root,&["rev-parse","feature"]));
        for name in ["main","invalid name","--flag"] {
            assert_eq!(run(root,"folder_create_branch",json!({"name":name,"checkout":false,"sessions":[],"confirm_sessions":false})).unwrap_err().status,400);
        }
        let error=run(root,"folder_switch",json!({"branch":"feature","sessions":sessions,"confirm_sessions":false})).unwrap_err();
        assert_eq!(error.detail["code"],"erro_git_folder_sessions"); assert_eq!(op(root,"branch_of",json!({})),"main");
        assert_eq!(op(root,"folder_switch",json!({"branch":"feature","sessions":sessions,"confirm_sessions":true}))["current"],"feature");
        fs::write(root.join("tracked.txt"),"preserved\n").unwrap();
        let error=run(root,"folder_switch",json!({"branch":"main","sessions":[],"confirm_sessions":true})).unwrap_err(); assert_eq!(error.detail["code"],"erro_git_folder_dirty");
        let st=op(root,"folder_create_branch",json!({"name":"without-switch","checkout":false,"sessions":sessions,"confirm_sessions":false})); assert_eq!(st["current"],"feature");
        assert_eq!(fs::read_to_string(root.join("tracked.txt")).unwrap(),"preserved\n");
    }

    #[cfg(unix)]
    #[test]
    fn separate_repositories_do_not_share_the_summary_wait() {
        use std::{io::Write,os::unix::fs::PermissionsExt,sync::mpsc,time::Duration};
        let dir=tempfile::tempdir().unwrap(); let root=dir.path(); let slow=root.join("slow"); let fast=root.join("fast"); repo(&slow); repo(&fast);
        let fifo=root.join("release"); assert!(Command::new("mkfifo").arg(&fifo).status().unwrap().success());
        let mut release=fs::OpenOptions::new().read(true).write(true).open(&fifo).unwrap();
        let marker=root.join("started"); let hook=slow.join(".git/hooks/fsmonitor");
        assert!(Command::new("mkfifo").arg(&marker).status().unwrap().success());
        let ready=fs::OpenOptions::new().read(true).write(true).open(&marker).unwrap();
        fs::write(&hook,format!("#!/bin/sh\nprintf ready > '{}'\nIFS= read -r value < '{}'\nprintf 'token\\000'\n",marker.display(),fifo.display())).unwrap();
        fs::set_permissions(&hook,fs::Permissions::from_mode(0o755)).unwrap(); g(&slow,&["config","core.fsmonitor",hook.to_str().unwrap()]);
        let slow_thread=std::thread::spawn(move || hangar_workspace::git::summary(slow.to_str(),false));
        use std::os::fd::AsRawFd;
        let mut event=libc::pollfd { fd:ready.as_raw_fd(),events:libc::POLLIN,revents:0 };
        let started=unsafe {libc::poll(&mut event,1,10_000)}==1 && event.revents&libc::POLLIN!=0;
        let (tx,rx)=mpsc::channel(); let fast_thread=std::thread::spawn(move || tx.send(hangar_workspace::git::summary(fast.to_str(),false)).unwrap());
        let result=rx.recv_timeout(Duration::from_secs(5));
        release.write_all(b"done\n").unwrap();
        let slow_result=slow_thread.join().unwrap(); fast_thread.join().unwrap();
        assert!(started,"a sonda de status não iniciou"); assert!(!slow_result.is_null());
        assert_eq!(result.unwrap(),json!({"dirty":0,"ahead":null,"behind":null}));
    }

    #[cfg(unix)]
    #[test]
    fn selected_commit_reads_status_at_most_twice_not_once_per_path() {
        use std::os::unix::fs::PermissionsExt;
        const TEST:&str="task3_contract::selected_commit_reads_status_at_most_twice_not_once_per_path";
        if let Some(root)=std::env::var_os("HANGAR_TASK3_STATUS_ROOT") {
            let root=Path::new(&root);
            let paths=(0..8).map(|n|format!("seleção-{n}.txt")).collect::<Vec<_>>();
            assert_eq!(op(root,"commit",json!({"message":"Seleção","paths":paths}))["ok"],true);
            let calls=fs::read_to_string(std::env::var_os("HANGAR_TASK3_STATUS_COUNT").unwrap()).unwrap();
            assert!(calls.lines().count()<=2,"status repetido por caminho: {calls}");
            assert_eq!(g(root,&["show","--name-only","--format=","HEAD"]).lines().count(),8);
            return;
        }
        let dir=tempfile::tempdir().unwrap(); let root=dir.path().join("repo"); repo(&root);
        for n in 0..8 {fs::write(root.join(format!("seleção-{n}.txt")),"selecionado").unwrap();}
        let bin=dir.path().join("bin"); fs::create_dir(&bin).unwrap();
        let shim=bin.join("git");
        fs::write(&shim,"#!/bin/sh\nfor arg in \"$@\"; do if [ \"$arg\" = status ]; then printf 'status\\n' >> \"$HANGAR_TASK3_STATUS_COUNT\"; fi; done\nexec /usr/bin/git \"$@\"\n").unwrap();
        fs::set_permissions(&shim,fs::Permissions::from_mode(0o755)).unwrap();
        let path=std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap()))).unwrap();
        let child=Command::new(std::env::current_exe().unwrap()).args(["--exact",TEST,"--nocapture"])
            .env("PATH",path).env("HANGAR_TASK3_STATUS_ROOT",root).env("HANGAR_TASK3_STATUS_COUNT",dir.path().join("status-count")).output().unwrap();
        assert!(child.status.success(),"{}\n{}",String::from_utf8_lossy(&child.stdout),String::from_utf8_lossy(&child.stderr));
    }

    #[cfg(unix)]
    #[test]
    fn remaining_quote_path_preserves_names_and_controls_filename_emitting_commands() {
        use std::os::unix::fs::PermissionsExt;
        const TEST:&str="task3_contract::remaining_quote_path_preserves_names_and_controls_filename_emitting_commands";
        const NAME:&str="sessão \"citada\".txt";
        if let Some(root)=std::env::var_os("HANGAR_TASK3_ARGV_ROOT") {
            let root=Path::new(&root);
            assert_eq!(op(root,"search",json!({"q":"sessão","mode":"names"})),json!({"mode":"names","truncated":false,"hits":[{"path":NAME,"line":null,"text":null}]}));
            assert_eq!(op(root,"search",json!({"q":"needle","mode":"contents"})),json!({"mode":"contents","truncated":false,"hits":[{"path":NAME,"line":1,"text":"needle"}]}));
            let trace=fs::read(std::env::var_os("HANGAR_TASK3_ARGV_TRACE").unwrap()).unwrap();
            let calls=trace.split(|b|*b==30).filter(|line|!line.is_empty()).map(|line|line.split(|b|*b==0).filter(|arg|!arg.is_empty()).map(|arg|String::from_utf8(arg.to_vec()).unwrap()).collect::<Vec<_>>()).collect::<Vec<_>>();
            assert_eq!(calls.len(),4);
            let emitters=calls.iter().filter(|args|args.iter().any(|arg|matches!(arg.as_str(),"grep"|"ls-files"))).collect::<Vec<_>>(); assert_eq!(emitters.len(),2);
            for args in emitters {assert!(args.windows(2).any(|pair|pair==["-c","core.quotePath=false"]));assert!(args.iter().any(|arg|arg=="-z"));}
            return;
        }
        let dir=tempfile::tempdir().unwrap(); let root=dir.path().join("repo"); repo(&root); fs::write(root.join(NAME),"needle\n").unwrap();
        let bin=dir.path().join("bin"); fs::create_dir(&bin).unwrap(); let shim=bin.join("git");
        fs::write(&shim,"#!/bin/sh\nprintf '\\036' >> \"$HANGAR_TASK3_ARGV_TRACE\"\nprintf '%s\\000' \"$@\" >> \"$HANGAR_TASK3_ARGV_TRACE\"\nexec /usr/bin/git \"$@\"\n").unwrap(); fs::set_permissions(shim,fs::Permissions::from_mode(0o755)).unwrap();
        let path=std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap()))).unwrap();
        let child=Command::new(std::env::current_exe().unwrap()).args(["--exact",TEST,"--nocapture"])
            .env("PATH",path).env("HANGAR_TASK3_ARGV_ROOT",root).env("HANGAR_TASK3_ARGV_TRACE",dir.path().join("argv")).output().unwrap();
        assert!(child.status.success(),"{}\n{}",String::from_utf8_lossy(&child.stdout),String::from_utf8_lossy(&child.stderr));
    }

    #[cfg(unix)]
    #[test]
    fn process_keeps_arguments_credentials_environment_and_closed_stdin_without_a_shell() {
        use std::{os::unix::fs::PermissionsExt,time::Duration};
        let dir=tempfile::tempdir().unwrap(); let script=dir.path().join("command");
        fs::write(&script,"#!/bin/sh\nprintf '%s\\n' \"$GIT_OPTIONAL_LOCKS\" \"$GIT_TERMINAL_PROMPT\" \"$CUSTOM_CREDENTIAL_CONFIG\" \"$1\"\nif IFS= read -r unexpected; then exit 17; fi\n").unwrap();
        fs::set_permissions(&script,fs::Permissions::from_mode(0o755)).unwrap();
        let output=hangar_workspace::process::run_program(Command::new(script).arg("literal; touch must-not-exist")
            .current_dir(dir.path()).env("CUSTOM_CREDENTIAL_CONFIG","configured-by-owner"),Duration::from_secs(5)).unwrap();
        assert_eq!(output.code,0); assert_eq!(output.stdout,"0\n0\nconfigured-by-owner\nliteral; touch must-not-exist\n"); assert!(!dir.path().join("must-not-exist").exists());
        let absent=hangar_workspace::process::run_program(&mut Command::new(dir.path().join("absent")),Duration::from_secs(5)).err().unwrap();
        assert_eq!(absent.status,503); assert_eq!(absent.code.as_deref(),Some("workspace_unavailable"));
    }

    #[test]
    fn remaining_metadata_empty_log_since_and_reference_parameters_keep_contract() {
        let dir=tempfile::tempdir().unwrap(); let root=dir.path();
        assert_eq!(op(root,"git_summary",json!({})),Value::Null); assert_eq!(op(root,"git_diffstat",json!({})),Value::Null);
        assert_eq!(op(root,"git_log_since",json!({"desde":0.0})),json!([]));
        assert_eq!(op(root,"folder_status",json!({})),json!({"repo":false}));
        repo(root);
        assert_eq!(op(root,"git_summary",json!({})),json!({"dirty":0,"ahead":null,"behind":null}));
        assert_eq!(op(root,"git_diffstat",json!({})),json!({"added":0,"removed":0}));
        assert_eq!(op(root,"folder_status",json!({})),json!({"repo":true,"current":"main","upstream":null,"dirty":0,
            "ahead":null,"behind":null,"toplevel":root,"last_fetch":null}));
        let linked=root.join("linked"); g(root,&["worktree","add","-q",linked.to_str().unwrap(),"feature"]);
        assert_eq!(op(&linked,"head_info",json!({})),json!(["feature",true]));
        assert_eq!(op(root,"git_action",json!({"action":"status"}))["ok"],true);
        assert_eq!(op(root,"git_log",json!({}))[0]["body"],"");
        let hash=g(root,&["rev-parse","HEAD"]).trim().to_owned();
        op(root,"create_branch_at",json!({"name":"without-switch","sha":hash})); assert_eq!(op(root,"branch_of",json!({})),"main");
        op(root,"create_branch_at",json!({"name":"with-switch","sha":hash,"switch_after":true})); assert_eq!(op(root,"branch_of",json!({})),"with-switch");
        fs::write(root.join("recent.txt"),"recent\n").unwrap(); g(root,&["add","recent.txt"]);
        let mut recent=Command::new("git");
        for (key,_) in std::env::vars().filter(|(key,_)|key.starts_with("GIT_")){recent.env_remove(key);}
        let committed=recent.arg("-C").arg(root).args(["commit","-qm","feat(x): novo entra"])
            .env("GIT_CONFIG_GLOBAL",if cfg!(windows){"NUL"}else{"/dev/null"}).env("GIT_CONFIG_NOSYSTEM","1")
            .env("GIT_AUTHOR_DATE","2022-01-01T00:00:00Z").env("GIT_COMMITTER_DATE","2022-01-01T00:00:00Z").output().unwrap(); assert!(committed.status.success());
        let log=op(root,"git_log_since",json!({"desde":1609459200.0})); assert_eq!(log.as_array().unwrap().len(),1); assert_eq!(log[0]["subject"],"feat(x): novo entra"); assert!(log[0]["short"].as_str().unwrap().len()>=7);
        for values in [json!({"name":"fixture"}),json!({"name":"fixture","git_dirty":3,"git_ahead":2,"git_behind":0,"git_added":128,"git_removed":24})] {
            let row:hangar_api::session::SessionRow=serde_json::from_value(values.clone()).unwrap(); let wire=serde_json::to_value(row).unwrap();
            for field in ["git_dirty","git_ahead","git_behind","git_added","git_removed"] {assert_eq!(wire[field],values[field]);}
        }
    }

    #[cfg(unix)]
    #[test]
    fn remaining_path_guards_scans_and_permissions_preserve_the_boundary() {
        use std::os::unix::fs::{symlink,PermissionsExt};
        let dir=tempfile::tempdir().unwrap(); let root=dir.path().join("repo"); repo(&root);
        fs::write(dir.path().join("outside.txt"),"private\n").unwrap(); symlink(dir.path().join("outside.txt"),root.join("escape")).unwrap();
        symlink(".git",root.join("alias")).unwrap();
        for path in ["..","../outside.txt","--output=x","\0",root.join("tracked.txt").to_str().unwrap()] {
            for operation in ["list_dir","read_file"] {assert_eq!(run(&root,operation,json!({"path":path})).unwrap_err().status,400);}
        }
        for path in ["../outside.txt",".git/config"] {
            assert!(run(&root,"write_file",json!({"path":path,"texto":"lost","digest_lido":"any"})).is_err());
        }
        assert_eq!(fs::read_to_string(dir.path().join("outside.txt")).unwrap(),"private\n");
        let empty=dir.path().join("empty"); fs::create_dir(&empty).unwrap(); g(&empty,&["init","-q","-b","main"]); fs::write(empty.join("new.txt"),"x").unwrap();
        assert!(op(&empty,"list_dir",json!({"so_modificados":false}))["entries"].as_array().unwrap().iter().any(|e|e["name"]=="new.txt"&&e["add"]==0&&e["del"]==0));
        for path in ["../outside.txt","--output=/tmp/x","escape",dir.path().join("outside.txt").to_str().unwrap()] {
            assert_eq!(run(&root,"path_diff",json!({"path":path,"escopo":"nao_commitado"})).unwrap_err().status,400);
        }
        for path in [":/",":(top)",":(glob)**","*","."] {assert_eq!(run(&root,"path_diff",json!({"path":path,"escopo":"nao_commitado"})).unwrap_err().status,404);}
        for path in [".git/config",".git/HEAD",".git/logs/HEAD","./.git/config","alias/config"] {
            assert_eq!(run(&root,"path_diff",json!({"path":path,"escopo":"nao_commitado"})).unwrap_err().status,403);
        }
        fs::write(root.join(".gitignore"),"node_modules\n").unwrap(); fs::create_dir(root.join(".github")).unwrap(); fs::write(root.join(".github/ci.yml"),"on: push\n").unwrap();
        for path in [".gitignore",".github/ci.yml"] {assert!(!op(&root,"path_diff",json!({"path":path,"escopo":"nao_commitado"}))["diff"].as_str().unwrap().is_empty());}
        let scan_root=dir.path().join("projects"); fs::create_dir(&scan_root).unwrap();
        let scan=|path:&Path|execute(serde_json::from_value(json!({"op":"scan_dir","args":{"root":scan_root,"path":path,"roots":[scan_root]}})).unwrap());
        fs::create_dir_all(scan_root.join("alpha/backend")).unwrap(); fs::create_dir(scan_root.join("beta")).unwrap();
        symlink(dir.path(),scan_root.join("escape")).unwrap();
        fs::File::open(scan_root.join("alpha")).unwrap().set_modified(std::time::UNIX_EPOCH+std::time::Duration::from_secs(100)).unwrap();
        fs::File::open(scan_root.join("beta")).unwrap().set_modified(std::time::UNIX_EPOCH+std::time::Duration::from_secs(200)).unwrap();
        assert_eq!(scan(&scan_root).unwrap()["entries"].as_array().unwrap().iter().map(|v|v["name"].as_str().unwrap()).collect::<Vec<_>>(),["beta","alpha"]);
        assert_eq!(scan(&scan_root.join("alpha")).unwrap()["entries"][0]["name"],"backend");
        assert_eq!(scan(&scan_root.join("escape")).unwrap_err().status,400);
        fs::create_dir(scan_root.join("locked")).unwrap(); fs::set_permissions(scan_root.join("locked"),fs::Permissions::from_mode(0)).unwrap();
        let denied=scan(&scan_root.join("locked")); fs::set_permissions(scan_root.join("locked"),fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(denied.unwrap(),json!({"entries":[],"error":"permission_denied"}));
        let plain=dir.path().join("plain"); fs::create_dir_all(plain.join("objects")).unwrap(); fs::create_dir(plain.join("refs")).unwrap(); fs::write(plain.join("config"),"common\n").unwrap();
        assert_eq!(op(&plain,"read_file",json!({"path":"config"}))["text"],"common\n"); assert_eq!(op(&plain,"list_dir",json!({}))["entries"].as_array().unwrap().len(),3);
    }

    #[test]
    fn git_path_diff_keeps_mainline_base_original_text_caps_and_subfolder() {
        let dir=tempfile::tempdir().unwrap(); let base=dir.path(); let origin=base.join("origin"); repo(&origin);
        g(&origin,&["switch","-c","develop"]); fs::write(origin.join("tracked.txt"),"um\ndois\n").unwrap(); g(&origin,&["commit","-qam","develop"]);
        g(base,&["clone","-q","-b","develop","origin","copy"]); let root=base.join("copy");
        g(&root,&["config","user.name","Teste"]); g(&root,&["config","user.email","teste@example.invalid"]); g(&root,&["config","commit.gpgSign","false"]);
        g(&root,&["remote","set-head","origin","main"]);
        fs::write(root.join("tracked.txt"),"um\ndois\ndisk\n").unwrap();
        let fall=op(&root,"path_diff",json!({"path":"tracked.txt","escopo":"branch"}));
        assert_eq!(fall["escopo_usado"],"nao_commitado"); assert_eq!(fall["motivo"],"arq_motivo_sem_commit_proprio");
        assert!(fall["diff"].as_str().unwrap().contains("+disk")); assert!(!fall["diff"].as_str().unwrap().contains("+dois"));
        g(&root,&["switch","-c","task"]); fs::write(root.join("tracked.txt"),"um\ndois\ntres\n").unwrap();
        op(&root,"commit",json!({"message":"third","paths":["tracked.txt"]})); op(&root,"push",json!({}));
        fs::write(root.join("tracked.txt"),"um\ndois\ntres\nquatro\n").unwrap();
        let branch=op(&root,"path_diff",json!({"path":"tracked.txt","escopo":"branch"}));
        assert_eq!(branch["original"],"um\ndois\n"); assert_eq!(branch["escopo_usado"],"branch"); assert!(branch["base"].is_string());
        let diff=branch["diff"].as_str().unwrap(); assert!(diff.contains("+tres")&&diff.contains("+quatro")&&!diff.contains("+dois"));
        let disk=op(&root,"path_diff",json!({"path":"tracked.txt","escopo":"nao_commitado"})); assert_eq!(disk["original"],"um\ndois\ntres\n");
        assert!(!disk["diff"].as_str().unwrap().contains("+tres"));
        fs::create_dir(root.join("sub")).unwrap(); fs::write(root.join("sub/tracked.txt"),"da subpasta\n").unwrap();
        g(&root,&["add","sub/tracked.txt"]);
        op(&root,"commit",json!({"message":"subfolder","paths":["sub/tracked.txt"]}));
        fs::write(root.join("sub/tracked.txt"),"da subpasta\nmexido\n").unwrap();
        assert_eq!(op(&root.join("sub"),"path_diff",json!({"path":"tracked.txt","escopo":"nao_commitado"}))["original"],"da subpasta\n");
        fs::create_dir(root.join("new-folder")).unwrap(); fs::write(root.join("new-folder/com espaço.txt"),"nasceu agora\n").unwrap();
        let new=op(&root,"path_diff",json!({"path":"new-folder/com espaço.txt","escopo":"branch"})); assert_eq!(new["original"],""); assert!(new["diff"].as_str().unwrap().contains("+nasceu agora"));
        let hash=g(&root,&["rev-parse","HEAD"]).trim().to_owned(); fs::write(root.join("tracked.txt"),"x".repeat(201_000)).unwrap();
        let diff=op(&root,"diff_vs_worktree",json!({"sha":hash})); assert_eq!(diff["truncated"],true); assert_eq!(diff["diff"].as_str().unwrap().chars().count(),200_000);
        op(&root,"commit",json!({"message":"large","paths":["tracked.txt"]})); let large=g(&root,&["rev-parse","HEAD"]).trim().to_owned();
        let diff=op(&root,"commit_diff",json!({"sha":large})); assert_eq!(diff["truncated"],true); assert_eq!(diff["diff"].as_str().unwrap().chars().count(),200_000);
        assert_eq!(op(&root,"commit_diff",json!({"sha":hash}))["truncated"],false);
        let plain=base.join("local"); repo(&plain); fs::write(plain.join("tracked.txt"),"new\n").unwrap();
        assert_eq!(op(&plain,"path_diff",json!({"path":"tracked.txt","escopo":"branch"}))["motivo"],"arq_motivo_sem_base_conhecida");
        let empty=base.join("empty"); fs::create_dir(&empty).unwrap(); g(&empty,&["init","-q","-b","main"]); fs::write(empty.join("new.txt"),"new\n").unwrap();
        assert_eq!(op(&empty,"path_diff",json!({"path":"new.txt","escopo":"branch"}))["motivo"],"arq_motivo_sem_commit");
    }

    #[test]
    fn successful_revisions_containing_refs_and_unicode_rename_keep_effects() {
        let dir=tempfile::tempdir().unwrap(); let root=dir.path(); repo(root);
        let base=g(root,&["rev-parse","HEAD"]).trim().to_owned(); g(root,&["switch","feature"]);
        fs::write(root.join("sessão-única.md"),"conteúdo\n").unwrap(); op(root,"commit",json!({"message":"feature add","paths":["sessão-única.md"]}));
        let revision=g(root,&["rev-parse","HEAD"]).trim().to_owned(); g(root,&["switch","main"]);
        assert_eq!(op(root,"cherry_pick",json!({"sha":revision}))["ok"],true); assert_eq!(fs::read_to_string(root.join("sessão-única.md")).unwrap(),"conteúdo\n");
        g(root,&["mv","sessão-única.md","sessão-nova.md"]);
        op(root,"commit",json!({"message":"rename","paths":["sessão-nova.md"]}));
        assert!(g(root,&["show","--name-status","--format=","HEAD"]).starts_with('R')); assert_eq!(g(root,&["status","--porcelain"]),"");
        let renamed=g(root,&["rev-parse","HEAD"]).trim().to_owned();
        assert_eq!(op(root,"revert_commit",json!({"sha":renamed}))["ok"],true); assert!(root.join("sessão-única.md").is_file()); assert!(!root.join("sessão-nova.md").exists());
        g(root,&["branch","origin"]); g(root,&["update-ref","refs/remotes/origin/main",&base]); g(root,&["symbolic-ref","refs/remotes/origin/HEAD","refs/remotes/origin/main"]);
        let containing=op(root,"branches_containing",json!({"sha":base}));
        assert_eq!(containing["local"],json!(["feature","main","origin"])); assert_eq!(containing["remote"],json!(["origin/main"]));
        let head=g(root,&["rev-parse","HEAD"]).trim().to_owned(); assert_eq!(op(root,"branches_containing",json!({"sha":head}))["local"],json!(["main","origin"]));
    }

    #[cfg(unix)]
    #[test]
    fn listing_keeps_symlink_own_mark_and_unquoted_backslash_space_and_numstat() {
        use std::os::unix::fs::{symlink,PermissionsExt};
        let dir=tempfile::tempdir().unwrap(); let root=dir.path(); repo(root);
        for name in ["com espaço.txt","nota\\ação.txt","ação \"citada\".txt"] {fs::write(root.join(name),"one\n").unwrap();g(root,&["add","--",name]);}
        g(root,&["commit","-qm","names"]);
        for name in ["com espaço.txt","nota\\ação.txt","ação \"citada\".txt"] {fs::write(root.join(name),"one\ntwo\n").unwrap();}
        let list=op(root,"list_dir",json!({})); let changed=op(root,"changed_files",json!({}));
        for name in ["com espaço.txt","nota\\ação.txt","ação \"citada\".txt"] {
            let entry=list["entries"].as_array().unwrap().iter().find(|v|v["name"]==name).unwrap(); assert_eq!(entry["path"],name); assert_eq!(entry["add"],1);
            let item=changed.as_array().unwrap().iter().find(|v|v["path"]==name).unwrap(); assert_eq!(item["added"],1); assert_eq!(op(root,"read_file",json!({"path":name}))["text"],"one\ntwo\n");
        }
        symlink("tracked.txt",root.join("new-link.txt")).unwrap();
        assert!(op(root,"list_dir",json!({}))["entries"].as_array().unwrap().iter().any(|v|v["path"]=="new-link.txt"));
        g(root,&["add","new-link.txt"]); g(root,&["commit","-qm","link"]); fs::write(root.join("tracked.txt"),"changed\n").unwrap();
        assert!(!op(root,"list_dir",json!({}))["entries"].as_array().unwrap().iter().any(|v|v["path"]=="new-link.txt"));
        let unreadable=root.join("no-read.txt"); fs::write(&unreadable,"preserved").unwrap(); fs::set_permissions(&unreadable,fs::Permissions::from_mode(0)).unwrap();
        let missing_digest=run(root,"write_file",json!({"path":"no-read.txt","texto":"lost"}));
        let unreadable_diff=run(root,"path_diff",json!({"path":"no-read.txt","escopo":"nao_commitado"}));
        fs::set_permissions(&unreadable,fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(missing_digest.unwrap_err().code.as_deref(),Some("erro_arq_sem_digest")); assert!(unreadable_diff.is_err()); assert_eq!(fs::read_to_string(unreadable).unwrap(),"preserved");
    }

    #[cfg(any(target_os="linux",windows))]
    struct ObservedProcess {
        pid:u32,
        #[cfg(target_os="linux")]
        handle:std::os::fd::OwnedFd,
        #[cfg(windows)]
        handle:std::os::windows::io::OwnedHandle,
    }
    #[cfg(any(target_os="linux",windows))]
    impl ObservedProcess {
        #[cfg(target_os="linux")]
        fn birth(pid:u32)->u64 {
            fs::read_to_string(format!("/proc/{pid}/stat")).unwrap().rsplit_once(')').unwrap().1
                .split_whitespace().nth(19).unwrap().parse().unwrap()
        }
        #[cfg(windows)]
        fn creation(handle:windows_sys::Win32::Foundation::HANDLE)->u64 {
            use windows_sys::Win32::{Foundation::FILETIME,System::Threading::GetProcessTimes};
            let mut created:FILETIME=unsafe {std::mem::zeroed()}; let mut exited=created; let mut kernel=created; let mut user=created;
            assert_ne!(unsafe {GetProcessTimes(handle,&mut created,&mut exited,&mut kernel,&mut user)},0);
            (u64::from(created.dwHighDateTime)<<32)|u64::from(created.dwLowDateTime)
        }
        #[cfg(windows)]
        fn birth(pid:u32)->u64 {
            use windows_sys::Win32::{Foundation::CloseHandle,System::Threading::{OpenProcess,PROCESS_QUERY_LIMITED_INFORMATION}};
            let handle=unsafe {OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,0,pid)}; assert!(!handle.is_null());
            let birth=Self::creation(handle); unsafe {CloseHandle(handle)}; birth
        }
        fn open(pid:u32,birth:u64)->Self {
            #[cfg(target_os="linux")]
            let handle=unsafe {
                use std::os::fd::FromRawFd;
                let fd=libc::syscall(libc::SYS_pidfd_open,pid,0) as i32;
                assert!(fd>=0,"pidfd_open: {}",std::io::Error::last_os_error());
                let handle=std::os::fd::OwnedFd::from_raw_fd(fd);
                assert_eq!(Self::birth(pid),birth,"PID foi reutilizado antes da observação"); handle
            };
            #[cfg(windows)]
            let handle=unsafe {
                use std::os::windows::io::FromRawHandle;
                use windows_sys::Win32::System::Threading::{OpenProcess,PROCESS_QUERY_LIMITED_INFORMATION,PROCESS_TERMINATE,PROCESS_SYNCHRONIZE};
                let raw=OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION|PROCESS_TERMINATE|PROCESS_SYNCHRONIZE,0,pid);
                assert!(!raw.is_null()); let handle=std::os::windows::io::OwnedHandle::from_raw_handle(raw);
                assert_eq!(Self::creation(raw),birth,"PID foi reutilizado antes da observação"); handle
            };
            Self {pid,handle}
        }
        fn exited(&self,timeout_ms:i32)->bool {
            #[cfg(target_os="linux")] {
                use std::os::fd::AsRawFd;
                let mut event=libc::pollfd {fd:self.handle.as_raw_fd(),events:libc::POLLIN,revents:0};
                let result=unsafe {libc::poll(&mut event,1,timeout_ms)}; assert!(result>=0);
                result==1 && event.revents&libc::POLLIN!=0
            }
            #[cfg(windows)] {
                use std::os::windows::io::AsRawHandle;
                unsafe {windows_sys::Win32::System::Threading::WaitForSingleObject(self.handle.as_raw_handle(),timeout_ms as u32)==0}
            }
        }
        fn stop(&self) {
            #[cfg(target_os="linux")] unsafe {
                use std::os::fd::AsRawFd;
                if libc::syscall(libc::SYS_pidfd_send_signal,self.handle.as_raw_fd(),libc::SIGKILL,std::ptr::null::<libc::siginfo_t>(),0)!=0 {
                    eprintln!("limpeza pidfd {}: {}",self.pid,std::io::Error::last_os_error());
                }
            }
            #[cfg(windows)] unsafe {
                use std::os::windows::io::AsRawHandle;
                if windows_sys::Win32::System::Threading::TerminateProcess(self.handle.as_raw_handle(),1)==0 {
                    eprintln!("limpeza handle {}: {}",self.pid,std::io::Error::last_os_error());
                }
            }
        }
    }

    #[cfg(any(target_os="linux",windows))]
    #[test]
    fn executor_death_ends_command_and_descendant_without_python_fixture() {
        use std::{process::Stdio,time::Duration};
        const TEST:&str="task3_contract::executor_death_ends_command_and_descendant_without_python_fixture";
        let exe=std::env::current_exe().unwrap();
        if let Ok(stage)=std::env::var("HANGAR_TASK3_PROCESS_STAGE") {
            let root=std::env::var_os("HANGAR_TASK3_PROCESS_ROOT").unwrap(); let root=Path::new(&root);
            let signal=|| {
                let socket=std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
                socket.connect(std::env::var("HANGAR_TASK3_PROCESS_SIGNAL").unwrap()).unwrap();
                socket.send(format!("{stage}:{}:{}",std::process::id(),ObservedProcess::birth(std::process::id())).as_bytes()).unwrap();
            };
            match stage.as_str() {
                "worker" => { hangar_workspace::process::run_program(Command::new(&exe).args(["--exact",TEST]).env("HANGAR_TASK3_PROCESS_STAGE","command"),Duration::from_secs(60)).unwrap(); }
                "command" => {
                    fs::write(root.join("command.pid"),std::process::id().to_string()).unwrap(); signal();
                    Command::new(&exe).args(["--exact",TEST]).env("HANGAR_TASK3_PROCESS_STAGE","descendant").spawn().unwrap().wait().unwrap();
                }
                "descendant" => { fs::write(root.join("descendant.pid"),std::process::id().to_string()).unwrap(); signal(); loop { std::thread::park(); } }
                _ => panic!("etapa de prova inválida"),
            }
            return;
        }
        let dir=tempfile::tempdir().unwrap();
        struct Cleanup { worker:std::process::Child,processes:Vec<ObservedProcess> }
        impl Drop for Cleanup {
            fn drop(&mut self){
                let _=self.worker.kill();let _=self.worker.wait();
                for process in &self.processes {if !process.exited(0){process.stop();assert!(process.exited(10_000),"processo próprio não encerrou: {}",process.pid);}}
            }
        }
        let signals=std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        signals.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
        let worker=Command::new(exe).args(["--exact",TEST]).env("HANGAR_TASK3_PROCESS_STAGE","worker")
            .env("HANGAR_TASK3_PROCESS_ROOT",dir.path()).env("HANGAR_TASK3_PROCESS_SIGNAL",signals.local_addr().unwrap().to_string())
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let mut guard=Cleanup {worker,processes:Vec::new()};
        let mut observed=std::collections::HashSet::new();
        for _ in 0..2 {
            let mut message=[0;128]; let (n,_)=signals.recv_from(&mut message).expect("processo de prova não sinalizou nascimento");
            let mut fields=std::str::from_utf8(&message[..n]).unwrap().split(':');
            let role=fields.next().unwrap();
            assert!(matches!(role,"command"|"descendant") && observed.insert(role.to_owned()));
            let pid=fields.next().unwrap().parse::<u32>().unwrap(); let birth=fields.next().unwrap().parse::<u64>().unwrap();
            guard.processes.push(ObservedProcess::open(pid,birth));
            assert_eq!(fs::read_to_string(dir.path().join(format!("{role}.pid"))).unwrap(),pid.to_string());
        }
        assert!(guard.processes.iter().all(|process|!process.exited(0)));
        guard.worker.kill().unwrap(); guard.worker.wait().unwrap();
        for process in &guard.processes {
            assert!(process.exited(10_000),"comando/descendente sobreviveu ao executor: {}",process.pid);
        }
    }
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
