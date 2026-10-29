//! Resultados capturados das funções Python sobre arquivos isolados.
use hangar_server::{accounts::environment::AccountEnvironment, harness_setup::health};
use serde_json::{Value, json};
#[test]
fn harness_health_configuration_matches_python() {
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/harness-health/checks.json")).unwrap();
    let mut failures = Vec::new();
    let mut count = 0;
    for case in fixture["cases"].as_array().unwrap() {
        let kind = case["kind"].as_str().unwrap();
        if !matches!(kind,"_mcp"|"_modelo_padrao"|"_hooks_codex"|"_fullscreen"|"_fullscreen_claude") {continue;}
        let home = tempfile::tempdir().unwrap();
        let root = home.path().to_string_lossy().into_owned();
        let env = AccountEnvironment::from_pairs([("HOME",root.as_str()),("USERPROFILE",root.as_str())]);
        for (relative, text) in case["files"].as_object().unwrap() {
            let path = home.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path,text.as_str().unwrap().replace("$ROOT",&root)).unwrap();
        }
        let cli = case["cli"].as_str().unwrap();
        let result = match kind {
            "_mcp" => health::mcp(&env,cli),
            "_modelo_padrao" => health::default_model(&env,cli),
            "_hooks_codex" => Ok(health::codex_hooks(&env)),
            "_fullscreen_claude" => Ok(health::fullscreen(&env,"claude")),
            _ => Ok(health::fullscreen(&env,cli)),
        };
        let actual = match result {Ok(value)=>json!({"ok":value}), Err(error)=>json!({"error":error})};
        if actual != case["expected"] {failures.push(format!("{}: {} != {}",case["name"],actual,case["expected"]));}
        count+=1;
    }
    assert_eq!(count,40);
    assert!(failures.is_empty(),"{}",failures.join("\n"));
}
