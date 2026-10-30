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
fn git_preflight_matches_captured_python_on_real_repositories() {
    use hangar_server::update::git_engine::Service;
    use serde_json::Value;
    let fixtures: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/update-git/cases.json"
    ))
    .unwrap();
    for case in fixtures["preflight"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let temp = crate::physical_tempdir();
        let root = temp.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        git(&root, &["init", "-b", "main"]);
        std::fs::write(root.join("tracked"), "initial\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-m", "initial"]);
        let origin = temp.path().join("origin.git");
        git(
            &root,
            &[
                "clone",
                "--bare",
                root.to_str().unwrap(),
                origin.to_str().unwrap(),
            ],
        );
        git(
            &root,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git(&root, &["fetch", "origin"]);
        if name == "untracked" {
            std::fs::write(root.join("loose"), "untracked").unwrap();
        }
        if name == "tracked" {
            std::fs::write(root.join("tracked"), "modified").unwrap();
        }
        if matches!(name, "ahead" | "diverged") {
            std::fs::write(root.join("local"), "local").unwrap();
            git(&root, &["add", "."]);
            git(&root, &["commit", "-m", "local"]);
        }
        if matches!(name, "behind" | "diverged") {
            let clone = temp.path().join("remote");
            git(
                &root,
                &["clone", origin.to_str().unwrap(), clone.to_str().unwrap()],
            );
            std::fs::write(clone.join("remote"), "remote").unwrap();
            git(&clone, &["add", "."]);
            git(&clone, &["commit", "-m", "remote"]);
            git(&clone, &["push", "origin", "main"]);
            git(&root, &["fetch", "origin"]);
        }
        if name == "work-branch" {
            git(&root, &["checkout", "-b", "work/task"]);
        }
        if name == "return-from-channel" {
            git(&root, &["checkout", "-b", "beta"]);
        }
        let mut environment: std::collections::BTreeMap<String, String> = [
            (
                "CP_UPDATE_BRANCH".into(),
                if name == "switch-channel" {
                    "beta".into()
                } else {
                    "".into()
                },
            ),
            (
                "GIT_CONFIG_GLOBAL".into(),
                if cfg!(windows) {
                    "NUL".into()
                } else {
                    "/dev/null".into()
                },
            ),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        ]
        .into();
        environment.insert("HOME".into(), temp.path().display().to_string());
        environment.insert("USERPROFILE".into(), temp.path().display().to_string());
        let service = Service::new(
            temp.path().into(),
            root.clone(),
            temp.path().join("claude"),
            environment,
        );
        if name == "return-from-channel" {
            std::fs::create_dir_all(&service.state_dir).unwrap();
            std::fs::write(service.state_dir.join("branch"), "beta").unwrap();
        }
        let mut actual = service.preflight_with(Vec::new(), "manual").unwrap();
        assert_eq!(actual["commit"], git(&root, &["rev-parse", "HEAD"]));
        actual["commit"] = "<HEAD>".into();
        assert_eq!(actual, case["expected"], "PEE074 {name}");
    }
    for case in fixtures["output"].as_array().unwrap() {
        let output = hangar_server::update::process::ProcessOutput {
            code: case["code"].as_i64().unwrap() as i32,
            stdout: case["stdout"].as_str().unwrap().into(),
            stderr: case["stderr"].as_str().unwrap().into(),
        };
        assert_eq!(
            hangar_server::update::git_engine::tail(&output, 12),
            case["tail"]
        );
        assert_eq!(
            hangar_server::update::git_engine::failure_reason(&output),
            case["reason"]
        );
    }
}

#[test]
fn channel_branch_validation_matches_python() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/update-git/channel-cases.json"
    ))
    .unwrap();
    for case in fixture["validation"].as_array().unwrap() {
        assert_eq!(
            hangar_server::update::git_engine::valid_branch(case["branch"].as_str().unwrap()),
            case["valid"].as_bool().unwrap(),
            "{}",
            case["branch"]
        );
    }
}

#[test]
fn checkout_release_download_matches_python_and_restores_held_binaries() {
    use hangar_server::update::git_engine::releases::{self, DownloadError, Source};
    use serde_json::{Value, json};
    use std::cell::RefCell;
    struct Fixture<'a> {
        name: &'a str,
        manifest: Value,
        urls: RefCell<Vec<String>>,
    }
    impl Source for Fixture<'_> {
        fn get(&self, url: &str, _: std::time::Duration) -> Result<Vec<u8>, DownloadError> {
            self.urls.borrow_mut().push(url.into());
            if url.ends_with("server-latest.json") {
                if url.contains("server-beta/") && self.name.starts_with("branch-404") {
                    return Err(DownloadError { kind: "URLError",
                        status: Some(404),
                        message: "missing".into(),
                    });
                }
                if url.contains("server-beta/") && self.name == "branch-no-platform" {
                    return Ok(br#"{"files":{}}"#.to_vec());
                }
                return Ok(if self.name == "manifest-list" {
                    b"[]".to_vec()
                } else {
                    serde_json::to_vec(&self.manifest).unwrap()
                });
            }
            if self.name == "download-failure" {
                return Err(DownloadError { kind: "URLError",
                    status: None,
                    message: "fixture-download-failed".into(),
                });
            }
            Ok(if self.name == "hash-mismatch" {
                b"wrong".to_vec()
            } else {
                format!("new-{}", url.rsplit('/').next().unwrap()).into_bytes()
            })
        }
    }
    let platform = if cfg!(windows) { "windows-x86_64" } else { "linux-x86_64" };
    let ext = if cfg!(windows) { ".exe" } else { "" };
    let golden: Value = serde_json::from_str(if cfg!(windows) {include_str!("../../../../backend/tests/fixtures/contract/update-git/release-cases-windows.json")} else {include_str!("../../../../backend/tests/fixtures/contract/update-git/release-cases.json")})
    .unwrap();
    for row in golden["cases"].as_array().unwrap() {
        let name = row["name"].as_str().unwrap();
        let temp = crate::physical_tempdir();
        let dest = temp.path();
        let mut files = serde_json::Map::new();
        for binary in releases::NAMES {
            files.insert(format!("{platform}/{binary}"),json!({"name":binary,"sha256":releases::digest(format!("new-{binary}").as_bytes())}));
            if matches!(name, "same" | "replace" | "hold" | "branch-404-keeps") {
                std::fs::write(
                    dest.join(format!("{binary}{ext}")),
                    format!("{}-{binary}", if name == "same" { "new" } else { "old" }),
                )
                .unwrap();
            }
        }
        if name == "missing-entry" {
            files.remove(&format!("{platform}/hangar-cano"));
        }
        let fixture = Fixture {
            name,
            manifest: json!({"commit":"a".repeat(40),"files":files}),
            urls: RefCell::new(vec![]),
        };
        let mut held = vec![];
        let result = releases::fetch(
            &fixture,
            "https://release.invalid",
            Some(if name.starts_with("branch-") {
                "server-beta"
            } else {
                "server-latest"
            }),
            if name == "unsupported" {
                None
            } else {
                Some(platform)
            },
            dest,
            None,
            if name == "hold" {
                Some(&mut held)
            } else {
                None
            },
        );
        let disk = || {
            fn visit(root:&std::path::Path,dir:&std::path::Path,data:&mut serde_json::Map<String,Value>){
                for entry in std::fs::read_dir(dir).unwrap().flatten(){let path=entry.path();if path.is_dir(){visit(root,&path,data)}else{data.insert(path.strip_prefix(root).unwrap().to_string_lossy().replace('\\',"/"),json!(std::fs::read_to_string(path).unwrap()));}}
            }
            let mut data=serde_json::Map::new();visit(dest,dest,&mut data);Value::Object(data)
        };
        assert_eq!(json!(result), row["result"], "{name}");
        assert_eq!(json!(*fixture.urls.borrow()), row["urls"], "{name}");
        assert_eq!(disk(), row["before_publish"], "{name}");
        if name == "hold" {
            assert_eq!(json!(releases::publish(&held)), row["install_warnings"]);
            assert_eq!(disk(), row["after_publish"]);
            assert_eq!(
                json!(releases::restore(dest, platform)),
                row["restore_warnings"]
            );
        }
        assert_eq!(disk(), row["after_restore"], "{name}");
    }
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

#[test]
fn checkout_pinning_matches_python_on_real_git_trees() {
    use hangar_server::update::git_engine::{
        Service,
        releases::{DownloadError, Source},
    };
    use serde_json::{Value, json};
    struct Fixture(Value);
    impl Source for Fixture {
        fn get(&self, _: &str, _: std::time::Duration) -> Result<Vec<u8>, DownloadError> {
            Ok(serde_json::to_vec(&self.0).unwrap())
        }
    }
    let golden: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/update-git/pinning-cases.json"
    ))
    .unwrap();
    for row in golden["cases"].as_array().unwrap() {
        let name = row["name"].as_str().unwrap();
        let temp = crate::physical_tempdir();
        let root = temp.path().join("repo");
        let home = temp.path().join("home");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-b", "main"]);
        let write = |path: &str, text: &str| {
            let file = root.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
            git(&root, &["add", path]);
            git(&root, &["commit", "-m", "fixture"]);
            git(&root, &["rev-parse", "HEAD"])
        };
        let first = write("backend/app/rust_server.py", "RUST_SERVER_PROTOCOL = 35\n");
        if name != "no-workflow" {
            write(
                ".github/workflows/server.yml",
                "on:\n  push:\n    paths:\n      - \"crates/**\"\n      - \"backend/app/rust_server.py\"\n  workflow_dispatch:\n",
            );
        }
        let published = write("crates/server.rs", "published\n");
        let same = write("note.txt", "one\n");
        let pending = write("crates/server.rs", "pending\n");
        let mut top = write("note.txt", "two\n");
        if name == "reverted-build" {
            top = write("crates/server.rs", "published\n");
        }
        git(&root, &["update-ref", "refs/remotes/origin/main", &top]);
        let mut manifest = json!({"commit":published,"files":{"linux-x86_64/hangar-server":{},"linux-x86_64/hangar-cano":{},"linux-x86_64/hangar-auth":{}}});
        if name == "platform-entry" {
            manifest["platforms"] = json!({"linux-x86_64":{"commit":pending,"protocol":35}});
        }
        if name == "invalid-commit" {
            manifest["commit"] = json!("--wrong");
        }
        if name == "missing-platform" {
            manifest["platforms"] = json!({});
        }
        let environment = [
            (
                "CP_RUST_SERVER".into(),
                if name == "disabled" { "0" } else { "1" }.into(),
            ),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            (
                "GIT_CONFIG_GLOBAL".into(),
                if cfg!(windows) { "NUL" } else { "/dev/null" }.into(),
            ),
        ]
        .into();
        let service = Service::new(home.clone(), root, home.join(".claude"), environment);
        let (selected, release) =
            service.pinned_target("main", &Fixture(manifest), Some("linux-x86_64"));
        let labels = [
            (first, "first"),
            (published, "published"),
            (same, "same"),
            (pending, "pending"),
            (top, "top"),
            ("origin/main".into(), "origin/main"),
        ];
        let label = |selected: Option<String>| {
            selected.map(|value| labels.iter().find(|(sha, _)| *sha == value).unwrap().1)
        };
        assert_eq!(json!(label(selected)), row["selected"], "{name}");
        assert_eq!(json!(release.is_some()), row["release"], "{name}");
        assert_eq!(
            json!(service.protocol_at("HEAD")),
            row["protocol"],
            "{name}"
        );
        assert_eq!(
            json!(label(service.newest_protocol("origin/main", 35))),
            row["newest_35"],
            "{name}"
        );
    }
}

#[test]
fn checkout_dist_regular_publication_and_unsafe_archives_preserve_previous_screen() {
    use hangar_server::update::git_engine::Service;
    use serde_json::Value;
    let golden: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/update-git/dist-cases.json"
    ))
    .unwrap();
    for row in golden["cases"].as_array().unwrap() {
        let name = row["name"].as_str().unwrap();
        let temp = crate::physical_tempdir();
        let root = temp.path().join("repo");
        let home = temp.path().join("home");
        let dist = root.join("frontend/dist");
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(dist.join("index.html"), "old").unwrap();
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut tar = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(3);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(
            &mut header,
            if name == "incomplete" {
                "other.html"
            } else {
                "index.html"
            },
            b"new".as_slice(),
        )
        .unwrap();
        if name.ends_with("-link") {
            let mut link = tar::Header::new_gnu();
            link.set_entry_type(tar::EntryType::Symlink);
            link.set_size(0);
            link.set_mode(0o777);
            tar.append_link(
                &mut link,
                "linked.html",
                if name == "internal-link" {
                    "index.html"
                } else {
                    "../outside"
                },
            )
            .unwrap();
        }
        let bytes = tar.into_inner().unwrap().finish().unwrap();
        let service = Service::new(
            home.clone(),
            root.clone(),
            home.join(".claude"),
            Default::default(),
        );
        let result = service.publish_dist(&bytes);
        if name == "internal-link" {
            assert!(
                result
                    .as_ref()
                    .is_err_and(|e| e.contains("ui_archive_invalid")),
                "PEE078: proteção Rust conserva recusa de links"
            );
            assert_eq!(
                std::fs::read_to_string(dist.join("index.html")).unwrap(),
                "old"
            );
            assert!(!root.join("frontend/.dist-velho").exists());
            continue;
        }
        assert_eq!(
            result.is_err(),
            !row["error"].is_null(),
            "{name}: {result:?}"
        );
        assert_eq!(
            std::fs::read_to_string(dist.join("index.html")).unwrap(),
            row["index"].as_str().unwrap(),
            "{name}"
        );
        let previous = std::fs::read_to_string(root.join("frontend/.dist-velho/index.html")).ok();
        assert_eq!(serde_json::json!(previous), row["previous"], "{name}");
    }
}

#[test]
fn channel_http_validation_matches_python_envelopes() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/update-git/channel-http-cases.json"
    ))
    .unwrap();
    for row in fixture["cases"].as_array().unwrap() {
        let result = hangar_server::update::git_engine::http::parse_channel(
            row["body"].as_str().unwrap().as_bytes(),
        );
        if row["status"] == 200 {
            assert_eq!(serde_json::json!({"branch":result.unwrap()}), row["json"]);
        } else {
            assert_eq!(result.unwrap_err(), row["json"], "{}", row["name"]);
        }
    }
}

#[cfg(unix)]
#[test]
fn checkout_pipeline_success_and_prepare_rollback_match_python() {
 use hangar_server::update::git_engine::{Service,releases::{Source,DownloadError}};
 use serde_json::{Value,json};use std::{io::{Read,Write},os::unix::fs::PermissionsExt,sync::atomic::{AtomicBool,Ordering}};
 struct NoNetwork;impl Source for NoNetwork{fn get(&self,_:&str,_:std::time::Duration)->Result<Vec<u8>,DownloadError>{panic!("a preparação local não pede rede");}}
 let golden:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/pipeline-cases.json")).unwrap();
 for row in golden["cases"].as_array().unwrap(){let scenario=row["name"].as_str().unwrap();let temp=crate::physical_tempdir();let root=temp.path().join("repo");let origin=temp.path().join("origin");let home=temp.path().join("home");let bin=temp.path().join("bin");for dir in [&root,&origin,&bin]{std::fs::create_dir_all(dir).unwrap();}
 std::fs::create_dir_all(root.join("backend")).unwrap();std::fs::write(root.join("backend/.keep"),"fixture").unwrap();std::fs::write(root.join("content"),"old").unwrap();git(&root,&["init","-b","main"]);git(&root,&["add","."]);git(&root,&["commit","-m","old"]);let old=git(&root,&["rev-parse","HEAD"]);git(&origin,&["init","--bare"]);git(&root,&["remote","add","origin",origin.to_str().unwrap()]);git(&root,&["push","-u","origin","main"]);std::fs::write(root.join("content"),"new").unwrap();git(&root,&["add","."]);git(&root,&["commit","-m","new"]);git(&root,&["push","origin","main"]);git(&root,&["reset","--hard",&old]);std::fs::create_dir_all(root.join("frontend/dist")).unwrap();std::fs::write(root.join("frontend/dist/index.html"),"old-screen").unwrap();
 let write=|name:&str,body:&str|{let path=bin.join(name);std::fs::write(&path,format!("#!/bin/sh\n{body}\n")).unwrap();std::fs::set_permissions(path,std::fs::Permissions::from_mode(0o755)).unwrap();};
 write("systemctl",match scenario {
 "restart-failure" => "if [ \"$2\" = restart ] && [ \"$(cat content)\" = new ]; then echo fixture-restart-failed; exit 1; fi\nexit 0",
 "same-pid" => "if [ \"$2\" = show ]; then echo 4242; fi\nexit 0",
 _ => "exit 1"
 });
 write("launchctl",match scenario {
 "restart-failure" => "if [ \"$1\" = kickstart ] && [ \"$(cat content)\" = new ]; then echo fixture-restart-failed; exit 1; fi\nexit 0",
 "same-pid" => "if [ \"$1\" = print ]; then echo 'pid = 4242'; fi\nexit 0",
 _ => "exit 1"
 });write("node","exit 0");write("npm","mkdir -p frontend/dist\nprintf new-screen > frontend/dist/index.html");write("uv",if scenario=="rollback-prepare-failure"{"echo fixture-uv-failed; exit 1"}else if scenario=="prepare-failure"{"if [ \"$(cat ../content)\" = new ]; then echo fixture-uv-failed; exit 1; fi\nexit 0"}else{"exit 0"});
 if scenario=="work-branch" {git(&root,&["checkout","-b","feature"]);}
 let path=format!("{}:{}",bin.display(),std::env::var("PATH").unwrap());let environment=[("PATH".into(),path),("HOME".into(),home.display().to_string()),("USERPROFILE".into(),home.display().to_string()),("CP_RUST_SERVER".into(),"0".into()),("CP_UPDATE_BRANCH".into(),if scenario=="missing-target"{"missing"}else{""}.into()),("GIT_CONFIG_NOSYSTEM".into(),"1".into()),("GIT_CONFIG_GLOBAL".into(),"/dev/null".into())].into();let service=Service::new(home.clone(),root.clone(),home.join(".claude"),environment);
 let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();let port=listener.local_addr().unwrap().port();listener.set_nonblocking(true).unwrap();let stop=Arc::new(AtomicBool::new(false));let ended=stop.clone();let server=std::thread::spawn(move||{while !ended.load(Ordering::Relaxed){if let Ok((mut stream,_))=listener.accept(){stream.set_nonblocking(false).unwrap();stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();let mut bytes=[0;2048];let _=stream.read(&mut bytes);let _=stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");}else{std::thread::sleep(std::time::Duration::from_millis(5));}}});
 let state=service.execute(port,&NoNetwork,None);stop.store(true,Ordering::Relaxed);server.join().unwrap();let mut expected=row["state"].as_object().unwrap().clone();
 // O Python distingue o diagnóstico do launchd daquele do systemd capturado no corpus.
 if cfg!(target_os="macos") && scenario=="restart-failure" {
 expected.insert("erro".into(),json!("o servidor nao reiniciou: nao consegui reiniciar o servico: fixture-restart-failed"));
 }
 for (key,value) in &expected{assert_eq!(state.get(key).unwrap_or(&Value::Null),value,"{scenario}: {key}; {state:?}");}
 assert_eq!(if git(&root,&["rev-parse","HEAD"])==old{"old"}else{"new"},row["head"].as_str().unwrap());assert_eq!(std::fs::read_to_string(root.join("frontend/dist/index.html")).unwrap(),row["screen"].as_str().unwrap());assert_eq!(json!(root.join("frontend/.dist-velho").exists()),row["previous_exists"]);
 }
}

#[test]
fn checkout_shell_change_warning_matches_python() {
    use hangar_server::update::git_engine::Service;
    let fixture: serde_json::Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/shell-cases.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let name=case["name"].as_str().unwrap();
        let temp=crate::physical_tempdir(); let root=temp.path().join("repo"); let home=temp.path().join("home");
        std::fs::create_dir_all(root.join("shell")).unwrap(); std::fs::create_dir_all(&home).unwrap();
        git(&root,&["init","-q"]); std::fs::write(root.join("shell/main.cjs"),"old").unwrap(); git(&root,&["add","."]);git(&root,&["commit","-qm","base"]);let mut before=git(&root,&["rev-parse","HEAD"]);
        std::fs::write(root.join(if name=="test-only" {"shell/main.test.cjs"} else {"shell/main.cjs"}),"new").unwrap();git(&root,&["add","."]);git(&root,&["commit","-qm","next"]);let after=git(&root,&["rev-parse","HEAD"]);
        if name!="absent" {std::fs::create_dir_all(root.join("shell/node_modules/electron")).unwrap();}
        match name {"unchanged"=>before=after.clone(),"bad-revision"=>before="missing".into(),"empty-revision"=>before.clear(),_=>{}}
        let service=Service::new(home.clone(),root,home.join(".claude"),Default::default());
        assert_eq!(service.shell_changed(&before,&after),case["result"].as_bool().unwrap(),"{name}");
    }
}


#[test]
fn authentication_pair_matches_python_publication_and_restore() {
    use serde_json::{Value,json};
    use hangar_server::update::git_engine::releases::{self,Source,DownloadError};
    use std::{path::Path,time::Duration,sync::Mutex};
    struct Captured {platform:&'static str,name:String,urls:Mutex<Vec<String>>}
    impl Source for Captured {
        fn get(&self,url:&str,_:Duration)->Result<Vec<u8>,DownloadError>{
            self.urls.lock().unwrap().push(url.into());
            let binary=url.rsplit('/').next().unwrap();
            if binary=="server-latest.json" {
                if self.name.starts_with("branch-")&&self.name!="branch-missing-auth"&&url.contains("/server-beta/"){return Err(DownloadError { kind: "URLError",status:Some(404),message:"missing".into()});}
                let mut files=serde_json::Map::new();
                for n in ["hangar-auth","hangar-cano","hangar-server"] {files.insert(format!("{}/{n}",self.platform),json!({"name":n,"sha256":releases::digest(format!("new-{n}").as_bytes())}));}
                if self.name=="missing-auth"||(self.name=="branch-missing-auth"&&url.contains("/server-beta/")){files.remove(&format!("{}/hangar-auth",self.platform));}
                if self.name=="invalid-auth-sha"{files.get_mut(&format!("{}/hangar-auth",self.platform)).unwrap()["sha256"]=json!("invalid");}
                return Ok(serde_json::to_vec(&json!({"files":files})).unwrap());
            }
            let kind=binary.trim_start_matches("hangar-");
            if self.name.trim_start_matches("branch-")==format!("{kind}-download-failure"){return Err(DownloadError { kind: "URLError",status:None,message:"fixture-download-failed".into()});}
            Ok(if self.name==format!("{kind}-hash-failure"){b"wrong".to_vec()}else{format!("new-{binary}").into_bytes()})
        }
    }
    fn snapshot(root:&Path,at:&Path,files:&mut serde_json::Map<String,Value>){
        for entry in std::fs::read_dir(at).unwrap(){let path=entry.unwrap().path();if path.is_dir(){snapshot(root,&path,files)}else{files.insert(path.strip_prefix(root).unwrap().to_string_lossy().replace('\\',"/"),json!(std::fs::read_to_string(path).unwrap()));}}
    }
    let platform = if cfg!(windows) { "windows-x86_64" } else { "linux-x86_64" };
    let fixture:Value=serde_json::from_str(if cfg!(windows) {include_str!("../../../../backend/tests/fixtures/contract/update-git/auth-release-windows.json")} else {include_str!("../../../../backend/tests/fixtures/contract/update-git/auth-release.json")}).unwrap();
    for row in fixture["cases"].as_array().unwrap(){
        let name=row["name"].as_str().unwrap();
        let temp=crate::physical_tempdir();let dest=temp.path();
        for (file,data) in row["initial"].as_object().unwrap(){let path=dest.join(file);std::fs::create_dir_all(path.parent().unwrap()).unwrap();std::fs::write(path,data.as_str().unwrap()).unwrap();}
        let source=Captured{platform,name:name.into(),urls:Mutex::new(vec![])};let mut held=vec![];
        let failed=std::cell::Cell::new(false);
        let install=|data:&[u8],target:&Path| {
            let filename=target.file_name().unwrap().to_string_lossy();
            let selected=(name.ends_with("server-publish-failure")&&filename==if cfg!(windows) {"hangar-server.exe"} else {"hangar-server"}) || (name.ends_with("metadata-publish-failure")&&filename=="hangar-server.auth.json");
            if selected&&!failed.replace(true){return Err(std::io::Error::other("fixture-publication-failed"));}
            releases::install(data,target)
        };
        let result=releases::fetch_with_installer(&source,"https://release.invalid",Some(if name.starts_with("branch-"){ "server-beta" }else{"server-latest"}),Some(platform),dest,None,if name.starts_with("hold-"){Some(&mut held)}else{None},&install).unwrap();
        let normalize=|warnings:Vec<String>|json!(warnings.iter().map(|v|v.replace(dest.to_str().unwrap(),"<DEST>").replace("<DEST>\\auth\\", "<DEST>/auth/").replace("<DEST>\\auth/", "<DEST>/auth/")).collect::<Vec<_>>());
        assert_eq!(normalize(result),row["result"],"{name}: avisos");
        assert_eq!(json!(*source.urls.lock().unwrap()),row["urls"],"{name}: URLs");
        let mut files=serde_json::Map::new();snapshot(dest,dest,&mut files);assert_eq!(json!(files),row["before_publish"],"{name}: antes");
        if name.starts_with("hold-"){
            assert_eq!(normalize(releases::publish(&held)),row["publish_warnings"],"{name}");
            let mut files=serde_json::Map::new();snapshot(dest,dest,&mut files);assert_eq!(json!(files),row["after_publish"],"{name}: publicado");
            assert_eq!(normalize(releases::restore(dest,platform)),row["restore_warnings"],"{name}");
            let mut files=serde_json::Map::new();snapshot(dest,dest,&mut files);assert_eq!(json!(files),row["after_restore"],"{name}: restaurado");
        }
    }
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
fn updater_settings_and_live_channel_use_captured_dotenv_semantics() {
    let fixture:serde_json::Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/dotenv-read.json")).unwrap();
    let mut differences=vec![];
    for case in fixture["cases"].as_array().unwrap() {
        let root=crate::physical_tempdir();let home=root.path().join("home");let repo=root.path().join("repo");
        std::fs::create_dir_all(repo.join("backend")).unwrap();std::fs::create_dir_all(&home).unwrap();
        std::fs::write(repo.join("backend/.env"),case["raw"].as_str().unwrap()).unwrap();
        let environment=case["environment"].as_object().unwrap().iter().map(|(key,value)|(key.clone(),value.as_str().unwrap().to_owned())).collect();
        let service=hangar_server::update::git_engine::Service::new(home.clone(),repo,home.join(".claude"),environment);
        for (kind,value) in [("snapshot",service.value("CP_SERVER_ID")),("live",service.channel_value("CP_SERVER_ID"))] {
            if value.as_ref().ok().map(String::as_str)!=case["server_id"].as_str(){differences.push(format!("{} {kind}: {value:?} != {}",case["name"],case["server_id"]));}
        }
    }
    assert!(differences.is_empty(),"{}",differences.join("\n"));
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

#[test]
fn startup_preparation_matches_python_order_outcomes_and_notices() {
    use hangar_server::update::git_engine::periodic::{StartupSource,StartupOutcome,prepare_startup};
    use serde_json::{Value,json};
    struct Source { row:Value,calls:std::sync::Mutex<Vec<Value>> }
    impl StartupSource for Source {
        fn package(&self)->Result<bool,String>{self.calls.lock().unwrap().push(json!(["manifest"]));Ok(self.row["package"]==true)}
        fn pending_commands(&self)->Result<Vec<String>,String>{
            self.calls.lock().unwrap().push(json!(["pending",{"incluir_destrutivos":false}]));
            Ok(self.row["pending"].as_array().unwrap().iter().map(|row|row["comando"].as_str().unwrap().into()).collect())
        }
        fn apply(&self)->Result<Vec<String>,String>{
            self.calls.lock().unwrap().push(json!(["apply",{"incluir_destrutivos":false}]));
            if self.row["failed"]==true{return Err("fixture failure".into());}
            Ok(self.row["done"].as_array().unwrap().iter().map(|id|id.as_str().unwrap().into()).collect())
        }
    }
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/startup-steps.json")).unwrap();
    for row in fixture["cases"].as_array().unwrap(){
        let source=Source{row:row.clone(),calls:Default::default()};
        let outcome=prepare_startup(&source,row["windows"]==true);
        let output=outcome.message().map(|text|text+"\n").unwrap_or_default();
        assert_eq!(output,row["stdout"],"{}",row["name"]);
        assert_eq!(json!(*source.calls.lock().unwrap()),row["calls"],"{}",row["name"]);
        assert_eq!(matches!(outcome,StartupOutcome::Failed(_)),row["failed"]==true);
    }
}

#[test]
fn automatic_update_matches_python_refusals_order_and_warnings(){
 use hangar_server::update::git_engine::automatic::{AutomaticSource,reason,DIST_SHA_URL};
 use serde_json::{Value,json};use std::sync::Mutex;
 struct Source {input:Value,calls:Mutex<Vec<Value>>,warnings:Mutex<Vec<Value>>}
 impl Source {fn call(&self,name:&str,args:Value,kwargs:Value){self.calls.lock().unwrap().push(json!([name,args,kwargs]));}}
 impl AutomaticSource for Source {
  fn target(&self)->Result<String,String>{self.call("target",json!([]),json!({}));Ok(self.input["target"].as_str().unwrap().into())}
  fn preflight(&self)->Result<Value,String>{self.call("preflight",json!([]),json!({}));Ok(self.input["pre"].clone())}
  fn switches(&self,_:&Value)->bool{self.call("switch",json!([]),json!({}));self.input["switch"]==true}
  fn state(&self)->Value{self.call("state",json!([]),json!({}));self.input["state"].clone()}
  fn pinned(&self)->Option<String>{self.call("pin",json!(["main"]),json!({}));self.input["pin"].as_str().map(str::to_owned)}
  fn download(&self)->Result<Vec<u8>,String>{self.call("download",json!([DIST_SHA_URL]),json!({"timeout":15}));if self.input["dist_error"]==true{return Err("fixture failure".into());}let raw=self.input["dist_hex"].as_str().unwrap();Ok(raw.as_bytes().chunks(2).map(|b|u8::from_str_radix(std::str::from_utf8(b).unwrap(),16).unwrap()).collect())}
  fn target_head(&self)->Result<(i32,String),String>{self.call("git",json!(["rev-parse","origin/main"]),json!({"timeout":30}));Ok((self.input["git_code"].as_i64().unwrap() as i32,self.input["git_out"].as_str().unwrap().into()))}
  fn invalid_timestamp(&self,value:&Value){self.warnings.lock().unwrap().push(json!(["auto-update: ts invalido no estado da atualizacao (%r)",value]));}
 }
 let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/auto-update.json")).unwrap();let now=chrono::DateTime::parse_from_rfc3339(fixture["now"].as_str().unwrap()).unwrap();let mut failures=vec![];
 for row in fixture["cases"].as_array().unwrap(){let source=Source{input:row["input"].clone(),calls:Default::default(),warnings:Default::default()};let actual=reason(&source,now).unwrap();if json!(actual)!=row["reason"]||json!(*source.calls.lock().unwrap())!=row["calls"]||json!(*source.warnings.lock().unwrap())!=row["warnings"]{failures.push(format!("{}: motivo={actual:?}, chamadas={:?}, avisos={:?}",row["name"],source.calls,source.warnings));}}
 assert!(failures.is_empty(),"{}",failures.join("\n"));
}

#[cfg(unix)]
#[test]
fn checkout_prepare_matches_dependency_order_and_marker_bytes() {
 let output=std::process::Command::new(std::env::current_exe().unwrap())
  .args(["--exact","update_git::checkout_prepare_contract_child","--ignored","--nocapture"]).output().unwrap();
 assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stdout).to_string()+&String::from_utf8_lossy(&output.stderr));
}
#[cfg(unix)]
#[test]
#[ignore="Captura isolada: outros testes não herdam arquivos de executáveis falsos em gravação."]
fn checkout_prepare_contract_child() {
 use hangar_server::update::git_engine::{Service,releases::{Source,DownloadError}};
 use serde_json::{Value,json};use std::os::unix::fs::PermissionsExt;
 struct NoNetwork;impl Source for NoNetwork{fn get(&self,_:&str,_:std::time::Duration)->Result<Vec<u8>,DownloadError>{panic!("preparação local não usa rede");}}
 let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/prepare.json")).unwrap();
 for row in fixture["cases"].as_array().unwrap(){
  let name=row["name"].as_str().unwrap();let temp=crate::physical_tempdir();let root=temp.path().join("repo");let home=temp.path().join("home");let bin=temp.path().join("bin");let calls=temp.path().join("calls");
  std::fs::create_dir_all(root.join("backend")).unwrap();std::fs::create_dir_all(&bin).unwrap();
  let service=Service::new(home.clone(),root.clone(),home.join(".claude"),[("PATH".into(),bin.display().to_string()),("HOME".into(),home.display().to_string()),("FIXTURE_CALLS".into(),calls.display().to_string())].into());
  std::fs::create_dir_all(&service.state_dir).unwrap();let marker=service.state_dir.join("package-lock.sha");
  if name!="missing-lock"{std::fs::write(root.join("package-lock.json"),b"{}").unwrap();}
  if name!="no-modules"{std::fs::create_dir(root.join("node_modules")).unwrap();}
  if name!="missing-marker"{std::fs::write(&marker,if name=="invalid-marker-utf8"{vec![255]}else if name=="same-lock"{hangar_server::update::git_engine::releases::digest(b"{}").into_bytes()}else{b"old".to_vec()}).unwrap();}
  if name=="same-lock-controls"{std::fs::write(&marker,format!("\u{1c}{}\u{1f}",hangar_server::update::git_engine::releases::digest(b"{}"))).unwrap();}
  if name=="same-lock-unicode"{std::fs::write(&marker,format!("\u{2003}{}\u{3000}",hangar_server::update::git_engine::releases::digest(b"{}"))).unwrap();}
  for program in ["uv","npm","powershell"] {
   if name==format!("missing-{program}"){continue;}
   let fail=(name=="uv-failure"&&program=="uv")||(name.ends_with("npm-failure")&&program=="npm");
   let body=format!("#!/bin/sh\nprintf '%s|' '{program}' \"$PWD\" \"$@\" >> \"$FIXTURE_CALLS\"\nprintf '\\n' >> \"$FIXTURE_CALLS\"\n{}\n",if fail{"printf fixture-failed; exit 1"}else{"exit 0"});
   let path=bin.join(program);std::fs::write(&path,body).unwrap();std::fs::set_permissions(path,std::fs::Permissions::from_mode(0o755)).unwrap();
  }
  let result=service.prepare(if name.starts_with("windows"){"windows"}else{"systemd"},false,None,&NoNetwork,None);
  if name=="invalid-marker-utf8" {
   assert_eq!(row["result"]["error_type"],"UnicodeDecodeError");
   assert_eq!(result.unwrap_err(),"update_prepare_marker_invalid_utf8");
  }else if row["result"]["ok"]==true{assert!(result.is_ok(),"{name}: {result:?}");}
  else {assert_eq!(result.unwrap_err(),row["result"]["message"].as_str().unwrap(),"{name}");}
  let actual=std::fs::read_to_string(&calls).unwrap_or_default().lines().map(|line|{
   let parts=line.trim_end_matches('|').split('|').collect::<Vec<_>>();
   if parts[0]=="powershell"&&parts.iter().any(|v|v.contains("Stop-HangarFrontend")){return json!({"action":"stop-front"});}
   json!({"program":parts[0],"args":parts[2..],"cwd":if parts[1]==root.join("backend").to_str().unwrap(){json!("backend")}else if parts[0]=="powershell"{Value::Null}else{json!("repo")}})
  }).collect::<Vec<_>>();
  let expected=row["calls"].as_array().unwrap().iter().map(|call|{let mut call=call.clone();call.as_object_mut().unwrap().remove("timeout");call}).collect::<Vec<_>>();
  assert_eq!(actual,expected,"{name}: ordem e argumentos");
  let bytes=std::fs::read(&marker).ok().map(|bytes|bytes.iter().map(|byte|format!("{byte:02x}")).collect::<String>());
  assert_eq!(json!(bytes),row["marker_hex"],"{name}: marcador integral");
 }
}
