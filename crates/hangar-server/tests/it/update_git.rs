//! Motor git nativo, contra checkout e servidor temporários.
use hangar_server::{
    accounts::{AccountService, environment::AccountEnvironment},
    auth::TrustedHosts,
    config::Config,
    routes::AppState,
};
use std::{path::Path, sync::Arc};
fn git(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_AUTHOR_NAME", "Contract")
        .env("GIT_AUTHOR_EMAIL", "contract@example.invalid")
        .env("GIT_COMMITTER_NAME", "Contract")
        .env("GIT_COMMITTER_EMAIL", "contract@example.invalid")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().into()
}
#[tokio::test]
async fn unmigrated_git_update_is_owned_without_python() {
    let temp = crate::physical_tempdir();
    let home = temp.path().join("home");
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(repo.join("backend")).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        repo.join("backend/.env"),
        "CP_UPDATE_BRANCH=\nCP_RUST_SERVER=0\n",
    )
    .unwrap();
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "fixture"]);
    hangar_server::session_open::cli::publish_backend_dir(&home, &repo.join("backend")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut state = AppState::new(Config {
        listen: address,
        upstream: crate::refused_address(),
        internal_secret: "fixture".into(),
        auth_token: "owner".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    state.accounts = AccountService::new(AccountEnvironment::from_map(
        [
            ("HOME".into(), home.display().to_string()),
            ("USERPROFILE".into(), home.display().to_string()),
        ]
        .into(),
    ));
    let router = hangar_server::routes::router(Arc::new(state));
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap()
    });
    let response = reqwest::Client::new()
        .get(format!("http://{address}/api/atualizacao"))
        .bearer_auth("owner")
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response
        .json::<serde_json::Value>()
        .await
        .unwrap_or_default();
    task.abort();
    assert_eq!(status, 200, "PEE074/086: motor git sem Python: {body}");
    assert_eq!(body["pre_voo"]["branch"], "main");
}


#[test]
fn channel_live_publication_survives_stale_parent_and_preserves_new_override() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "update_git::channel_live_publication_child", "--ignored", "--nocapture"])
        .output().unwrap();
    assert!(output.status.success(), "{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}
#[test]
#[ignore = "Isola a guarda dos filhos lançados simultaneamente por outros testes."]
fn channel_live_publication_child() {
    use hangar_server::update::git_engine::Service;
    use serde_json::json;
    let temp = crate::physical_tempdir();
    let root = temp.path().join("repo");
    let origin = temp.path().join("origin");
    let home = temp.path().join("home");
    std::fs::create_dir_all(root.join("backend")).unwrap();
    std::fs::create_dir_all(&origin).unwrap();
    git(&origin, &["init", "--bare"]);
    git(&root, &["init", "-b", "main"]);
    std::fs::write(root.join("file"), "fixture").unwrap();
    git(&root, &["add", "file"]);
    git(&root, &["commit", "-m", "fixture"]);
    git(
        &root,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&root, &["push", "origin", "HEAD:refs/heads/beta"]);
    std::fs::write(
        root.join("backend/.env"),
        "CP_UPDATE_BRANCH=\r\nCP_UPDATE_LAST_BRANCH=\r\nUNCHANGED=yes\r\n",
    )
    .unwrap();
    let environment = [
        ("CP_UPDATE_BRANCH".into(), "".into()),
        ("CP_UPDATE_LAST_BRANCH".into(), "".into()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        (
            "GIT_CONFIG_GLOBAL".into(),
            if cfg!(windows) { "NUL" } else { "/dev/null" }.into(),
        ),
    ]
    .into();
    let service = Service::new(
        home.clone(),
        root.clone(),
        home.join(".claude"),
        environment,
    );
    assert_eq!(
        service.write_channel(" beta ").unwrap(),
        json!({"branch":"beta","last_branch":"beta","checkout_branch":"main"})
    );
    assert_eq!(
        service.clone().target().unwrap(),
        "beta",
        "snapshot antigo do pai não desfaz publicação"
    );
    assert_eq!(
        service.write_channel("").unwrap(),
        json!({"branch":"","last_branch":"beta","checkout_branch":"main"})
    );
    let bytes = std::fs::read_to_string(root.join("backend/.env")).unwrap();
    assert!(bytes.contains("UNCHANGED=yes\r\n"));
    let held = service.take_turn().unwrap().unwrap();
    assert_eq!(service.write_channel("beta").unwrap_err().status, 409);
    drop(held);
    assert_eq!(
        service.write_channel("missing").unwrap_err().code,
        "update_channel_missing"
    );
    assert_eq!(service.write_channel("-invalid").unwrap_err().status, 400);
    let before = std::fs::read(root.join("backend/.env")).unwrap();
    let mut changed = service.clone();
    changed
        .environment
        .insert("CP_UPDATE_BRANCH".into(), "manual".into());
    assert_eq!(changed.target().unwrap(), "manual");
    assert_eq!(std::fs::read(root.join("backend/.env")).unwrap(), before);
    std::fs::write(
        root.join("backend/.env"),
        bytes.replace("CP_UPDATE_BRANCH=", "CP_UPDATE_BRANCH=manual-file"),
    )
    .unwrap();
    assert_eq!(
        service.target().unwrap(),
        "main",
        "edição manual desativa marcador; precedência ambiente permanece"
    );
}

#[tokio::test]
async fn migration_contract_native_fetch_has_no_python_prerequisite() {
    struct Recorder(std::sync::Mutex<Vec<String>>);
    impl hangar_server::update::StepRunner for Recorder {
        fn run(&self,command:&str,_:&std::path::Path)->hangar_server::update::StepRun {
            self.0.lock().unwrap().push(command.into());
            hangar_server::update::StepRun{code:Some(0),output:String::new()}
        }
    }
    let temp=crate::physical_tempdir();std::fs::create_dir(temp.path().join("backend")).unwrap();
    let runner=std::sync::Arc::new(Recorder(std::sync::Mutex::new(vec![])));
    assert_eq!(hangar_server::update::dev_mode::fetch_rust(runner.clone(),temp.path()).await,None);
    let calls=runner.0.lock().unwrap();assert_eq!(calls.len(),1);assert!(calls[0].contains("rust-release"));assert!(!calls[0].contains("app.rust_release"));
}

#[test]
fn startup_step_failure_finishes_preparation_without_aborting_startup() {
    use hangar_server::update::git_engine::Service;
    let root=crate::physical_tempdir();
    let repo=root.path().join("repo");let home=root.path().join("home");
    std::fs::create_dir_all(repo.join("docs/atualizacoes")).unwrap();
    std::fs::create_dir_all(repo.join("backend")).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(repo.join("docs/atualizacoes/one.md"),"---\nid: one\ntitulo: fixture\ncomando: exit 7\nprova: missing\n---\n").unwrap();
    let service=Service::new(home.clone(),repo,home.join(".claude"),Default::default());
    assert!(service.startup_steps().is_ok(),"o Python registra a falha e termina a preparação sem derrubar a subida");
    assert!(!hangar_server::update::steps::applied(&service.state_dir).unwrap().contains("one"));
}

