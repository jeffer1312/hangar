use hangar_server::{
    accounts::environment::AccountEnvironment,
    codex_setup::{files, fingerprints},
};
use serde_json::{Value, json};

#[test]
fn codex_service_native_wrapper_fingerprint_ignores_obsolete_python_copies() {
    let temp = crate::physical_tempdir();
    let home = temp.path().join("home");
    let repo = temp.path().join("repo");
    let account = home.join(".codex");
    let env = AccountEnvironment::from_pairs([
        ("HOME", home.to_string_lossy().into_owned()),
        ("USERPROFILE", home.to_string_lossy().into_owned()),
    ]);
    let before = fingerprints::compute(&env, &account, &repo, &json!({}), false, "fragments", false).unwrap();
    for path in [repo.join("scripts/codex-hook-json.py"), account.join(".hangar-hooks/codex-hook-json.py")] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "cópia antiga sem consumidor").unwrap();
    }
    let after = fingerprints::compute(&env, &account, &repo, &json!({}), false, "fragments", false).unwrap();
    assert_eq!(before, after);
}

#[cfg(unix)]
#[test]
fn codex_service_fingerprints_match_python_hash_inputs_and_metadata() {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/codex-integration/fingerprints.json"
    ))
    .unwrap();
    let mut failures = Vec::new();
    for case in corpus["cases"].as_array().unwrap() {
        let temp = crate::physical_tempdir();
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
        let mut chunks = case["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|part| {
                let chunk = part.as_str().unwrap();
                if let Some(relative) = chunk.strip_prefix("$ROOT/").and_then(|part| part.strip_suffix(":ausente"))
                    && relative == "home/.claude/CLAUDE.MD"
                    && root.join(relative).is_file()
                {
                    // A captura Linux distingue caixa; no APFS os dois nomes apontam para a mesma fonte.
                    let captured = &case["entries"]["home/.claude/CLAUDE.md"];
                    assert_eq!(root.join(relative).canonicalize().unwrap(), root.join("home/.claude/CLAUDE.md").canonicalize().unwrap());
                    return format!("{}:{}:{}", root.join(relative).display(), captured["hex"].as_str().unwrap().len()/2, captured["mtime_ns"].as_u64().unwrap());
                }
                // O tamanho de diretório pertence ao filesystem, não ao conteúdo capturado.
                if let Some((path, metadata)) = chunk.rsplit_once(':')
                    && let Some((path, _size)) = path.rsplit_once(':')
                    && let Some(relative) = path.strip_prefix("$ROOT/")
                    && case["entries"][relative]["directory"] == true
                {
                    return format!("{}:{}:{metadata}", root.join(relative).display(),
                        std::fs::metadata(root.join(relative)).unwrap().len());
                }
                chunk.replace("$ROOT", &root.to_string_lossy())
            })
            .collect::<Vec<String>>();
        if case["phase"] == "fragments" && case["sources"] == false {
            // D70: a assinatura acompanha o executável que agora implementa o wrapper.
            let old_source = format!("{}/repo/scripts/codex-hook-json.py:", root.display());
            let old_copy = format!("{}/home/.codex/.hangar-hooks/codex-hook-json.py:", root.display());
            let prefix = chunks.remove(0);
            chunks.retain(|chunk| !chunk.starts_with(&old_source) && !chunk.starts_with(&old_copy));
            let executable = std::env::current_exe().unwrap();
            let metadata = std::fs::metadata(&executable).unwrap();
            let nanos = metadata.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let at = chunks.iter().position(|chunk| {
                let path = if let Some(path) = chunk.strip_suffix(":ausente") { path }
                else { chunk.rsplit_once(':').unwrap().0.rsplit_once(':').unwrap().0 };
                std::path::Path::new(path) > executable.as_path()
            }).unwrap_or(chunks.len());
            chunks.insert(at, format!("{}:{}:{nanos}", executable.display(), metadata.len()));
            chunks.insert(0, prefix);
        }
        // A assinatura revisada acrescenta conteúdo às mesmas entradas capturadas.
        for chunk in chunks.iter_mut().skip(1) {
            if let Some((prefix,_))=chunk.rsplit_once(':') && let Some((path,_))=prefix.rsplit_once(':') {
                let path=std::path::Path::new(path);
                if path.is_file() && path!=std::env::current_exe().unwrap() {
                    let bytes=std::fs::read(path).unwrap();chunk.push_str(":sha256:");chunk.push_str(&files::hash(&bytes));
                }
            }
        }
        if case["phase"]=="skills" {
            // Estes casos capturados não habilitam plugins nem possuem marketplaces no TOML.
            chunks.push(if case["sources"]==true {"{\n  \"enabled\": []\n}\n"}else{"{\n  \"enabled\": [],\n  \"plugins\": {},\n  \"marketplaces\": {}\n}\n"}.into());
        }
        let expected_bytes = chunks.concat();
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
        let temp = crate::physical_tempdir();
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
        let temp = crate::physical_tempdir();
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
        let dir = crate::physical_tempdir();
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
  let dir=crate::physical_tempdir();let home=dir.path().join("home");let account=home.join(".codex");let repo=dir.path().join("repo");std::fs::create_dir_all(&account).unwrap();std::fs::create_dir_all(&repo).unwrap();let env=AccountEnvironment::from_pairs([("HOME",home.to_string_lossy().into_owned()),("USERPROFILE",home.to_string_lossy().into_owned())]);
  let before=fingerprints::compute(&env,&account,&repo,&json!({}),false,"fragments",false).unwrap();std::fs::write(account.join("config.toml"),format!("extra = {}\n",serde_json::to_string(&text).unwrap().replace('\u{7f}',"\\u007f"))).unwrap();let after=fingerprints::compute(&env,&account,&repo,&json!({}),false,"fragments",false).unwrap();proptest::prop_assert_eq!(before,after);
 }
}

#[test]
fn codex_service_fingerprint_detects_same_metadata_edits_and_plugin_selection() {
 let dir=crate::physical_tempdir();let home=dir.path().join("home");let account=home.join(".codex");let repo=dir.path().join("repo");
 std::fs::create_dir_all(&account).unwrap();std::fs::create_dir_all(home.join(".claude/skills/a")).unwrap();
 let env=AccountEnvironment::from_pairs([("HOME",home.to_string_lossy().into_owned()),("USERPROFILE",home.to_string_lossy().into_owned())]);
 let path=home.join(".claude/skills/a/SKILL.md");std::fs::write(&path,"first").unwrap();let modified=std::fs::metadata(&path).unwrap().modified().unwrap();
 let signature=||fingerprints::compute(&env,&account,&repo,&json!({}),false,"skills",false).unwrap();let before=signature();
 std::fs::write(&path,"other").unwrap();std::fs::OpenOptions::new().read(true).write(true).open(&path).unwrap().set_times(std::fs::FileTimes::new().set_modified(modified)).unwrap();assert_ne!(before,signature());
 let before=signature();std::fs::write(account.join("config.toml"),"[plugins.'a@m']\nenabled=true\n").unwrap();assert_ne!(before,signature());
}
