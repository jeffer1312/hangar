use super::*;
use crate::config_sync::{bundle::Bundle,export::Context,paths::Roots};
use std::collections::BTreeMap;

struct NativeReport;
impl Actions for NativeReport {
    fn bridge(&self)->Step<'_,BridgeReport>{Box::pin(async {
        Ok(BridgeReport {
            stats:json!({"kimi":{"erro":1}}),
            errors:BTreeMap::from([("kimi".into(),"skills-bridge: PermissionDenied".into())]),
        })
    })}
    fn hook(&self,_name:&'static str)->Step<'_>{Box::pin(async{panic!("Hook não selecionado")})}
    fn codex(&self)->Step<'_>{Box::pin(async{panic!("Codex não selecionado")})}
}

/// Instala os hooks anotando a ordem; o guarda falha.
struct GuardFails(std::sync::Mutex<Vec<&'static str>>);
impl Actions for GuardFails {
    fn bridge(&self)->Step<'_,BridgeReport>{Box::pin(async{Ok(BridgeReport{stats:json!({}),errors:BTreeMap::new()})})}
    fn hook(&self,name:&'static str)->Step<'_>{
        self.0.lock().unwrap().push(name);
        Box::pin(async move{ if name=="ensure_guard_hooks_installed" {Err("settings sem escrita".into())} else {Ok(json!({}))} })
    }
    fn codex(&self)->Step<'_>{Box::pin(async{panic!("Codex não selecionado")})}
}

/// H-63: `claude_hooks` roda os 7 instaladores; a falha de um vira aviso `config_sync_after_failed`
/// com o passo e a causa, e não desfaz nem interrompe os outros.
#[tokio::test]
async fn hooks_item_runs_every_installer_and_a_failure_is_only_a_warning() {
    let home=tempfile::tempdir().unwrap();let root=home.path().display().to_string();
    let context=Context::new(Roots{home:root.clone(),claude:format!("{root}/.claude"),codex:format!("{root}/.codex"),hangar:format!("{root}/repo")});
    let mut context=Apply::new(context,Bundle::default(),"fixture",Default::default());
    let actions=GuardFails(Default::default());
    run(&mut context,&["claude_hooks".into()],false,&actions).await;
    assert_eq!(*actions.0.lock().unwrap(),HOOKS.to_vec());
    let warnings=context.report["claude_hooks"]["warnings"].as_array().unwrap().clone();
    assert_eq!(warnings.len(),1,"{warnings:?}");
    assert_eq!(warnings[0]["code"],"config_sync_after_failed");
    assert_eq!(warnings[0]["params"]["step"],"ensure_guard_hooks_installed");
    assert_eq!(warnings[0]["params"]["error"],"settings sem escrita");
}

#[tokio::test]
async fn after_apply_preserves_the_native_bridge_failure_cause() {
    let home=tempfile::tempdir().unwrap();let root=home.path().display().to_string();
    let context=Context::new(Roots{home:root.clone(),claude:format!("{root}/.claude"),codex:format!("{root}/.codex"),hangar:format!("{root}/repo")});
    let mut context=Apply::new(context,Bundle::default(),"fixture",Default::default());
    run(&mut context,&["claude_skills".into()],false,&NativeReport).await;
    let warning=&context.report["claude_skills"]["warnings"][0];
    assert_eq!(warning["code"],"config_sync_after_failed");
    assert_eq!(warning["params"]["step"],"skill_bridge:kimi");
    assert_eq!(warning["params"]["error"],"skills-bridge: PermissionDenied");
}
