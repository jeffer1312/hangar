use hangar_server::{config_sync::{apply::Apply,bundle::Bundle,export::Context,paths::Roots},orchestration::codec};
use serde_json::json;
use std::{collections::BTreeMap,path::Path};

fn files(root:&Path,folder:&Path,output:&mut BTreeMap<String,String>) {
    for entry in std::fs::read_dir(folder).unwrap() {
        let path=entry.unwrap().path();
        if path.is_dir() {files(root,&path,output);} else {
            output.insert(path.strip_prefix(root).unwrap().to_string_lossy().replace('\\',"/"),std::fs::read_to_string(path).unwrap());
        }
    }
}

#[test]
fn preferences_import_uses_runtime_validation_secrets_and_effects() {
    compare(include_str!("../../../../backend/tests/fixtures/contract/config_sync/preferences-import.json"));
}

#[test]
fn preferences_import_preserves_python_equality_before_validation() {
    compare(include_str!("../../../../backend/tests/fixtures/contract/config_sync/preferences-equality.json"));
}

fn compare(raw: &str) {
    let fixture=codec::loads(raw).unwrap();
    let mut differences=Vec::new();
    for (index,row) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let home=tempfile::tempdir().unwrap();
        let root=home.path().to_string_lossy().into_owned();
        let roots=Roots{home:root.clone(),claude:format!("{root}/.claude"),codex:format!("{root}/.codex"),hangar:format!("{root}/repo")};
        let mut context=Context::new(roots);
        context.preferences=home.path().join(".claude/runtime-config.json");
        std::fs::create_dir_all(context.preferences.parent().unwrap()).unwrap();
        std::fs::write(&context.preferences,codec::dumps(&row["initial"],false)).unwrap();
        let mut bundle=Bundle::default();
        bundle.items.insert("hangar_prefs".into(),json!({"prefs":codec::clone_value(&row["incoming"])}));
        let mut apply=Apply::new(context,bundle,"fixture",Default::default());
        let result=apply.apply_item("hangar_prefs");
        apply.finish_item("hangar_prefs",result);
        let report=crate::config_metadata::normalize_home(&apply.report(),home.path());
        let mut expected=row["report"].clone();
        for warning in expected["items"]["hangar_prefs"]["warnings"].as_array_mut().unwrap() {
            if warning["params"]["error"].as_str().is_some_and(|error|error.starts_with("'utf-8' codec can't encode")) {
                // D35 conserva a recusa e os efeitos; o diagnóstico técnico é nativo.
                warning["params"]["error"]=json!("runtime-config.json contém surrogate Unicode isolado; arquivo preservado");
            }
        }
        if report!=expected {differences.push(format!("{index}: relatório {report} versus {expected}"));}
        let mut disk=BTreeMap::new();files(home.path(),home.path(),&mut disk);
        if serde_json::to_value(disk).unwrap()!=row["files"] {differences.push(format!("{index}: arquivos"));}
    }
    assert!(differences.is_empty(),"importação de preferências: {differences:?}");
}
