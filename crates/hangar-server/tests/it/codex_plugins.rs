use hangar_server::codex_setup::plugins::{self, Context, Runtime};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, sync::Mutex};
struct Fixture {
    name: String,
    calls: Mutex<Vec<String>>,
}
impl Runtime for Fixture {
    async fn prepare(&self, _root: &Path, _backups: &Path) -> Result<bool, String> {
        self.calls.lock().unwrap().push("ecc".into());
        if self.name == "ecc-failure" {
            Err("fixture ECC".into())
        } else {
            Ok(self.name == "ecc-ok")
        }
    }
}
fn decode(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn tree(root: &Path, current: &Path, out: &mut BTreeMap<String, String>) {
    for entry in std::fs::read_dir(current).unwrap() {
        let path = entry.unwrap().path();
        if path.is_symlink() {
            continue;
        }
        if path.is_dir() {
            tree(root, &path, out);
        } else {
            out.insert(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
                hex(&std::fs::read(path).unwrap()),
            );
        }
    }
}
#[cfg(unix)]
#[tokio::test]
async fn codex_plugins_match_original_files_backups_and_partial_failures() {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/codex-integration/plugin-adapt.json"
    ))
    .unwrap();
    let mut failures = Vec::new();
    for case in corpus["cases"].as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let account = root.join("account");
        let plugin = account.join("plugins/cache/fixture/v1");
        let backups = root.join("backups");
        let repo = root.join("repo");
        std::fs::create_dir_all(&plugin).unwrap();
        std::fs::create_dir_all(repo.join("scripts")).unwrap();
        std::fs::write(repo.join("scripts/codex-hook-json.py"), b"fixture wrapper").unwrap();
        let external = root.join("outside.json");
        std::fs::write(&external, b"original outside").unwrap();
        for (name, content) in case["source"].as_object().unwrap() {
            let path = plugin.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, decode(content.as_str().unwrap())).unwrap();
        }
        #[cfg(unix)]
        if case["name"] == "outside-hook" {
            std::os::unix::fs::symlink(&external, plugin.join("hooks.json")).unwrap();
        }
        #[cfg(unix)]
        if case["name"] == "dangling-root" {
            std::fs::remove_dir(&plugin).unwrap();
            std::os::unix::fs::symlink(root.join("missing-outside"), &plugin).unwrap();
        }
        let runtime = Fixture {
            name: case["name"].as_str().unwrap().into(),
            calls: Mutex::new(vec![]),
        };
        let mut changed = 0;
        let outcome = plugins::adapt(
            if case["name"] == "outside-root" {
                root
            } else {
                &plugin
            },
            &Context {
                account: &account,
                backups: &backups,
                repo: &repo,
                python: "/fixture/python",
                windows: false,
            },
            |data| {
                let mut data = data.clone();
                data["fixture"] = json!(true);
                Ok(data)
            },
            || changed += 1,
            &runtime,
        )
        .await;
        let result = match outcome {
            Ok(()) => json!({"ok":true}),
            Err(message) => json!({"message":message}),
        };
        let expected = if case["result"].get("error").is_some() {
            json!({"message":case["result"]["message"]})
        } else {
            case["result"].clone()
        };
        let mut files = BTreeMap::new();
        if plugin.is_dir() {
            tree(&plugin, &plugin, &mut files);
        }
        for content in files.values_mut() {
            let raw = decode(content);
            let text = String::from_utf8(raw).unwrap();
            *content = hex(text.replace(root.to_str().unwrap(), "$ROOT").as_bytes());
        }
        let mut saved = Vec::<Value>::new();
        if backups.exists() {
            for path in std::fs::read_dir(&backups).unwrap() {
                saved.push(
                    serde_json::from_slice(&std::fs::read(path.unwrap().path()).unwrap()).unwrap(),
                );
            }
        }
        saved.sort_by_key(|value| value["path"].as_str().unwrap().to_owned());
        let saved: Value = serde_json::from_str(
            &serde_json::to_string(&saved)
                .unwrap()
                .replace(root.to_str().unwrap(), "$ROOT"),
        )
        .unwrap();
        let wrapper = std::fs::read(account.join(".hangar-hooks/codex-hook-json.py"))
            .ok()
            .map(|raw| hex(&raw));
        let actual = json!({"result":result,"changed":changed,"calls":*runtime.calls.lock().unwrap(),"files":files,"backups":saved,"wrapper_hex":wrapper,"outside_hex":hex(&std::fs::read(external).unwrap())});
        let expected = json!({"result":expected,"changed":case["changed"],"calls":case["calls"],"files":case["files"],"backups":case["backups"],"wrapper_hex":case["wrapper_hex"],"outside_hex":case["outside_hex"]});
        if actual != expected {
            failures.push(format!("{}: {actual}", case["name"]));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(48))]
    #[test]
    fn codex_plugins_reject_outside_cache_before_mutation(name in "[a-zA-Z0-9_-]{1,60}") {
        let root=tempfile::tempdir().unwrap();let account=root.path().join("account");let outside=root.path().join(format!("outside-{name}"));
        std::fs::create_dir_all(account.join("plugins/cache")).unwrap();std::fs::create_dir(&outside).unwrap();
        let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let runner=Fixture{name:"ecc-ok".into(),calls:Mutex::new(vec![])};
        let result=rt.block_on(plugins::adapt(&outside,&Context{account:&account,backups:&root.path().join("backups"),repo:root.path(),python:"/fixture/python",windows:false},|_|panic!("normalização fora do cache"),||panic!("alteração fora do cache"),&runner));
        proptest::prop_assert_eq!(result,Err("Plugin fora do cache do Codex".into()));
        proptest::prop_assert!(runner.calls.lock().unwrap().is_empty());
        proptest::prop_assert!(!root.path().join("backups").exists());
    }
}
