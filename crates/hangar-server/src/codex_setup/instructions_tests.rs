use super::*;
use crate::{
    accounts::environment::AccountEnvironment,
    machine_peers::dotenv::{SettingsEnvironment, SettingsSnapshot},
};
use serde_json::{Value, json};
use std::{fs, sync::Arc};

include!("instructions_project_tests.rs");
include!("instructions_limit_tests.rs");
include!("instructions_project_types_tests.rs");

struct FixtureLinks(bool);
impl Links for FixtureLinks {
    fn create(&self, source: &Path, target: &Path) -> io::Result<()> {
        if self.0 {
            Err(io::Error::other("fixture symlink"))
        } else {
            NativeLinks.create(source, target)
        }
    }
}
fn unhex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .chunks_exact(2)
        .map(|part| u8::from_str_radix(std::str::from_utf8(part).unwrap(), 16).unwrap())
        .collect()
}
fn snapshot(
    home: &Path,
    path: &Path,
    files: &mut serde_json::Map<String, Value>,
    records: &mut Vec<Value>,
    backups: &mut Vec<Value>,
) {
    let mut entries = fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if matches!(
            path.file_name().unwrap().to_str(),
            Some(".hangar-instrucoes.lock" | ".git")
        ) {
            continue;
        }
        if path.is_dir() && !path.is_symlink() {
            snapshot(home, &path, files, records, backups);
            continue;
        }
        let relative = path
            .strip_prefix(home)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if relative.contains(".hangar-instrucoes/") {
            let data = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            if relative.contains("/backups/") {
                backups.push(data);
            } else {
                records.push(data);
            }
        } else {
            let data = if path.is_symlink() {
                json!({"link":fs::read_link(&path).unwrap()})
            } else {
                json!({"hex":fs::read(&path).unwrap().iter().map(|b|format!("{b:02x}")).collect::<String>()})
            };
            files.insert(relative, data);
        }
    }
}

#[tokio::test]
async fn global_instructions_match_original_aliases_imports_backups_and_removal() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/codex-integration/instructions.json"
    ))
    .unwrap();
    let mut differences = Vec::new();
    for row in fixture["cases"].as_array().unwrap() {
        let home = tempfile::tempdir_in(crate::list::links::without_verbatim(std::env::temp_dir().canonicalize().unwrap())).unwrap();
        let claude = home.path().join(".claude");
        let codex = home.path().join(".codex");
        fs::create_dir_all(&claude).unwrap();
        fs::create_dir_all(&codex).unwrap();
        for (relative, data) in row["before"].as_object().unwrap() {
            let path = home.path().join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            if let Some(link) = data["link"].as_str() {
                std::os::unix::fs::symlink(link, &path).unwrap();
            } else {
                fs::write(path, unhex(data["hex"].as_str().unwrap())).unwrap();
            }
        }
        let env = AccountEnvironment::from_map(
            [
                ("HOME".into(), home.path().to_string_lossy().into_owned()),
                (
                    "USERPROFILE".into(),
                    home.path().to_string_lossy().into_owned(),
                ),
            ]
            .into(),
        );
        let config = RuntimeConfig::from_snapshot(
            env,
            Arc::new(SettingsSnapshot::from_parts(
                SettingsEnvironment::default(),
                String::new(),
            )),
        )
        .unwrap();
        let name = row["name"].as_str().unwrap();
        let source = claude.join("CLAUDE.md");
        let target = codex.join("AGENTS.override.md");
        let mut results = Vec::new();
        for round in 0..row["results"].as_array().unwrap().len() {
            if round > 0 {
                match name {
                    "remove-source" => {
                        fs::remove_file(&source).unwrap();
                    }
                    "change-source" => {
                        fs::write(&source, "Mudança\n").unwrap();
                    }
                    "modified-target" => {
                        let _ = fs::remove_file(&target);
                        fs::write(&target, "Pessoal alterado\n").unwrap();
                        fs::remove_file(&source).unwrap();
                    }
                    "modified-link" => {
                        let _ = fs::remove_file(&target);
                        std::os::unix::fs::symlink("../other", &target).unwrap();
                        fs::remove_file(&source).unwrap();
                    }
                    _ => {}
                }
            }
            results.push(match prepare_with(&config,&codex,None,true,&FixtureLinks(name=="copy-fallback")).await {
                Ok(())=>json!({"ok":true}),
                Err(error)=>json!({"error":match error {Error::Invalid(_)=>"ValueError",Error::Changed(_)|Error::Displaced(_)=>"AlteradoExternamente",Error::Io(_)=>"OSError"},"message":error.to_string()}),
            });
        }
        let mut files = serde_json::Map::new();
        let mut records = Vec::new();
        let mut backups = Vec::new();
        snapshot(
            home.path(),
            home.path(),
            &mut files,
            &mut records,
            &mut backups,
        );
        let actual = json!({"results":results,"files":files,"records":records,"backups":backups});
        let actual: Value = serde_json::from_str(
            &actual
                .to_string()
                .replace(home.path().to_str().unwrap(), "$ROOT"),
        )
        .unwrap();
        let mut expected = json!({"results":row["results"],"files":row["files"],"records":row["records"],"backups":row["backups"]});
        if name == "uppercase" && source.exists() {
            // A captura usa filesystem sensível à caixa; aqui os dois nomes são o mesmo arquivo.
            assert_eq!(source.canonicalize().unwrap(), claude.join("CLAUDE.MD").canonicalize().unwrap());
            expected["files"][".codex/AGENTS.override.md"]["link"] = json!("../.claude/CLAUDE.md");
        }
        if actual != expected {
            differences.push((name.to_owned(), actual, expected));
        }
    }
    assert!(
        differences.is_empty(),
        "instruções divergentes: {differences:?}"
    );
}
