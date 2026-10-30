use hangar_server::{
    accounts::environment::AccountEnvironment,
    codex_setup::{files, fingerprints},
};
use serde_json::{Value, json};
#[cfg(unix)]
#[test]
fn codex_service_fingerprints_match_python_hash_inputs_and_metadata() {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/codex-integration/fingerprints.json"
    ))
    .unwrap();
    let mut failures = Vec::new();
    for case in corpus["cases"].as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for (name, entry) in case["entries"].as_object().unwrap() {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            if entry["directory"] == true {
                std::fs::create_dir_all(path).unwrap();
            } else {
                let text = entry["hex"].as_str().unwrap();
                let bytes: Vec<u8> = (0..text.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
                    .collect();
                std::fs::write(path, bytes).unwrap();
            }
        }
        for (name, entry) in case["entries"].as_object().unwrap() {
            let modified = std::time::UNIX_EPOCH
                + std::time::Duration::from_nanos(entry["mtime_ns"].as_u64().unwrap());
            std::fs::File::open(root.join(name))
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(modified))
                .unwrap();
        }
        let home = root.join("home");
        let env = AccountEnvironment::from_pairs([
            ("HOME", home.to_string_lossy().into_owned()),
            ("USERPROFILE", home.to_string_lossy().into_owned()),
        ]);
        let expected_bytes = case["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|part| {
                part.as_str()
                    .unwrap()
                    .replace("$ROOT", &root.to_string_lossy())
            })
            .collect::<String>();
        let expected = files::hash(expected_bytes.as_bytes());
        let actual = fingerprints::compute(
            &env,
            &home.join(".codex"),
            &root.join("repo"),
            &json!({}),
            false,
            case["phase"].as_str().unwrap(),
            case["sources"] == true,
        )
        .unwrap();
        if actual != expected {
            failures.push(json!({"variant":case["variant"],"phase":case["phase"],"sources":case["sources"],"actual":actual,"expected":expected,"python_chunks":case["chunks"]}));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
#[test]
fn codex_service_fingerprint_types_only_reject_fields_used_by_original() {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/codex-integration/fingerprint-types.json"
    ))
    .unwrap();
    let mut failures = Vec::new();
    for case in corpus["cases"].as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let account = home.join(".codex");
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&account).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(account.join("config.toml"), case["toml"].as_str().unwrap()).unwrap();
        let env = AccountEnvironment::from_pairs([
            ("HOME", home.to_string_lossy().into_owned()),
            ("USERPROFILE", home.to_string_lossy().into_owned()),
        ]);
        let actual = match fingerprints::compute(
            &env,
            &account,
            &repo,
            &json!({}),
            false,
            case["phase"].as_str().unwrap(),
            case["sources"] == true,
        ) {
            Ok(_) => json!({"ok":true}),
            Err(error) => json!({"error":error.kind,"message":error.message}),
        };
        if actual != case["result"] {
            failures.push(json!({"case":case,"actual":actual}));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
#[cfg(unix)]
#[test]
fn codex_service_fingerprint_path_failures_preserve_python_error_class() {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/codex-integration/fingerprint-paths.json"
    ))
    .unwrap();
    let mut failures = Vec::new();
    for case in corpus["cases"].as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let home = root.join("home");
        let account = home.join(".codex");
        let repo = root.join("repo");
        std::fs::create_dir_all(&account).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        let path = root.join(case["path"].as_str().unwrap());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        if case["kind"] == "file" {
            std::fs::write(path, "not a directory").unwrap();
        } else {
            std::os::unix::fs::symlink(root.join("absent"), path).unwrap();
        }
        let env = AccountEnvironment::from_pairs([
            ("HOME", home.to_string_lossy().into_owned()),
            ("USERPROFILE", home.to_string_lossy().into_owned()),
        ]);
        let actual = fingerprints::compute(
            &env,
            &account,
            &repo,
            &json!({}),
            false,
            case["phase"].as_str().unwrap(),
            case["sources"] == true,
        );
        match actual {
            Err(error) if case["result"]["error"] == error.kind => {
                if error.kind == "ValueError" {
                    assert_eq!(
                        error
                            .message
                            .replace(root.to_string_lossy().as_ref(), "$ROOT"),
                        case["result"]["message"]
                    );
                } else {
                    assert!(!error.message.is_empty());
                }
            }
            other => failures.push(json!({"case":case,"actual":format!("{other:?}")})),
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
#[test]
fn codex_service_fingerprint_registry_validates_common_dependencies() {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/codex-integration/fingerprint-registry.json"
    ))
    .unwrap();
    let mut failures = Vec::new();
    for case in corpus["cases"].as_array().unwrap() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let account = home.join(".codex");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&account).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        let env = AccountEnvironment::from_pairs([
            ("HOME", home.to_string_lossy().into_owned()),
            ("USERPROFILE", home.to_string_lossy().into_owned()),
        ]);
        let actual = match fingerprints::compute(
            &env,
            &account,
            &repo,
            &case["registry"],
            false,
            case["phase"].as_str().unwrap(),
            case["sources"] == true,
        ) {
            Ok(_) => json!({"ok":true}),
            Err(error) => json!({"error":error.kind,"message":error.message}),
        };
        if actual != case["result"] {
            failures.push(json!({"case":case,"actual":actual}));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
proptest::proptest! {
 #![proptest_config(proptest::test_runner::Config::with_cases(64))]
 #[test]
 fn codex_service_fingerprint_ignores_unselected_external_text(text in ".{0,1024}"){
  let dir=tempfile::tempdir().unwrap();let home=dir.path().join("home");let account=home.join(".codex");let repo=dir.path().join("repo");std::fs::create_dir_all(&account).unwrap();std::fs::create_dir_all(&repo).unwrap();let env=AccountEnvironment::from_pairs([("HOME",home.to_string_lossy().into_owned()),("USERPROFILE",home.to_string_lossy().into_owned())]);
  let before=fingerprints::compute(&env,&account,&repo,&json!({}),false,"fragments",false).unwrap();std::fs::write(account.join("config.toml"),format!("extra = {}\n",serde_json::to_string(&text).unwrap().replace('\u{7f}',"\\u007f"))).unwrap();let after=fingerprints::compute(&env,&account,&repo,&json!({}),false,"fragments",false).unwrap();proptest::prop_assert_eq!(before,after);
 }
}
