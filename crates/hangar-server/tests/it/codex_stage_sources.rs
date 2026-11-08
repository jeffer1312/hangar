use hangar_server::{
    accounts::environment::AccountEnvironment,
    codex_setup::{phase::{Error, State}, stage_flow::{self, Actions, Configuration}, stage_sources},
};
use serde_json::{Value, json};
use std::path::Path;

struct ImportSourceProbe {
    staged: Option<Value>,
}

impl Actions for ImportSourceProbe {
    async fn history(&mut self) -> Result<Value, Error> { Ok(json!({})) }
    fn fingerprint(&mut self) -> Result<String, Error> { Ok("source".into()) }
    async fn open(&mut self, home: &Path, _: &Path, _: bool) -> Result<(), Error> {
        self.staged = Some(hangar_server::codex_setup::files::object(&home.join(".claude.json"))?);
        Err(Error { kind: "ProbeComplete", message: String::new() })
    }
    async fn detect(&mut self) -> Result<Vec<Value>, Error> { unreachable!() }
    async fn import(&mut self, _: Vec<Value>) -> Result<Value, Error> { unreachable!() }
    async fn close(&mut self) -> Result<(), Error> { unreachable!() }
    async fn configure(&mut self, _: Configuration<'_>, _: &mut Value) -> Result<(), Error> { unreachable!() }
    fn hooks(&mut self, _: &Value, _: &mut Value) -> Result<(), Error> { unreachable!() }
    fn checkpoint(&mut self, _: &mut Value) -> Result<(), Error> { unreachable!() }
}

#[tokio::test]
async fn codex_mcp_stage_uses_selected_config_without_legacy_fallback() {
    let current = json!({"mcpServers":{"open-design":{"type":"http","url":"https://design.example/mcp"}}});
    let old = json!({"mcpServers":{"old":{"type":"http","url":"https://old.example/mcp"}}});
    for (configured, use_selected) in [("unset", false), ("empty", false), ("tilde", true), ("absolute", true)] {
        for exists in [true, false] {
            let temp = crate::physical_tempdir();
            let home = temp.path();
            let selected = home.join(".claude");
            std::fs::create_dir(&selected).unwrap();
            std::fs::write(selected.join("settings.json"), "{}").unwrap();
            std::fs::write(home.join(".claude.json"), serde_json::to_vec(&old).unwrap()).unwrap();
            if exists { std::fs::write(selected.join(".claude.json"), serde_json::to_vec(&current).unwrap()).unwrap(); }
            let work = home.join("work");
            std::fs::create_dir(&work).unwrap();
            let mut pairs = vec![("HOME", home.to_string_lossy().into_owned()), ("USERPROFILE", home.to_string_lossy().into_owned())];
            let value = match configured {
                "unset" => None,
                "empty" => Some(String::new()),
                "tilde" => Some("~/.claude".into()),
                "absolute" => Some(selected.to_string_lossy().into_owned()),
                _ => unreachable!(),
            };
            if let Some(value) = value { pairs.push(("CLAUDE_CONFIG_DIR", value)); }
            let env = AccountEnvironment::from_pairs(pairs);
            let mut probe = ImportSourceProbe { staged: None };
            let result = stage_flow::run(&stage_flow::Paths { env: &env, home, account: &home.join(".codex"), work: &work, backups: &home.join("backups") }, &mut json!({}), &State::new(json!({"avisos":[]})), false, &mut probe).await;
            assert_eq!(result.unwrap_err().kind, "ProbeComplete");
            let staged = probe.staged.unwrap();
            if use_selected {
                assert!(staged["mcpServers"].get("old").is_none());
                if exists { assert_eq!(staged["mcpServers"]["open-design"]["url"], "https://design.example/mcp"); }
                else { assert!(staged.get("mcpServers").is_none()); }
            } else {
                assert_eq!(staged["mcpServers"]["old"]["url"], "https://old.example/mcp");
                assert!(staged["mcpServers"].get("open-design").is_none());
            }
        }
    }
    let temp = crate::physical_tempdir();
    let home = temp.path();
    let selected = home.join(".claude");
    std::fs::create_dir(&selected).unwrap();
    std::fs::write(selected.join("settings.json"), "{}").unwrap();
    std::fs::write(home.join(".claude.json"), serde_json::to_vec(&old).unwrap()).unwrap();
    let work = home.join("work");
    std::fs::create_dir(&work).unwrap();
    let env = AccountEnvironment::from_pairs([
        ("HOME", home.to_string_lossy().into_owned()),
        ("USERPROFILE", home.to_string_lossy().into_owned()),
        ("CLAUDE_CONFIG_DIR", selected.to_string_lossy().into_owned()),
    ]);
    std::fs::write(selected.join(".claude.json"), "configuração viva inválida").unwrap();
    let mut probe = ImportSourceProbe { staged: None };
    let result = stage_flow::run(&stage_flow::Paths { env: &env, home, account: &home.join(".codex"), work: &work, backups: &home.join("backups") }, &mut json!({}), &State::new(json!({"avisos":[]})), false, &mut probe).await;
    assert_eq!(result.unwrap_err().kind, "ValueError");
    assert!(probe.staged.is_none(), "fonte inválida precisa impedir a conversão");
}
proptest::proptest! {
 #![proptest_config(proptest::test_runner::Config::with_cases(64))]
 #[test]
 fn codex_service_stage_hooks_are_idempotent_for_external_commands(command in ".{0,1024}"){
  let input=json!({"Event":[{"private":true,"hooks":[{"type":"command","command":command},{"type":"prompt","prompt":"preservado"}]}]});
  let once=stage_sources::without_app_hooks(&input);let twice=stage_sources::without_app_hooks(&once);
  proptest::prop_assert_eq!(&once,&twice);
  proptest::prop_assert_eq!(&once["Event"][0]["private"],&json!(true));
  proptest::prop_assert!(once["Event"][0]["hooks"].as_array().unwrap().iter().any(|hook|hook["type"]=="prompt"));
 }
}
